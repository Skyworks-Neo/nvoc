//! 实机 ROM 结构化解码回归（opportunistic：`reverse/VBIOS` 不存在则跳过，
//! 与 vbios::legacy 的 `*_when_present` 惯例一致）。
//!
//! 断言值来源三重校准：ROM 文件名标称功率、vix-editor（Python）同源解析
//! 输出、vix 文档记载（如 5080 Laptop 的 5000/80000/175000 mW nvidia-smi
//! 三元组、新 4090 VMIN 链入其它方程）。跑法：
//! `cargo test -p nvoc-core --test vbios_structural_roms`

use nvoc_core::vbios::families::Family;
use nvoc_core::vbios::gb20x;
use nvoc_core::vbios::image_chain::scan_images;
use nvoc_core::vbios::tables_v3;

const ROM_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../reverse/VBIOS");

fn load(name: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(ROM_DIR).join(name);
    match std::fs::read(&path) {
        Ok(data) => Some(data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skip {name}: fixture not present");
            None
        }
        Err(e) => panic!("read {name}: {e}"),
    }
}

fn ctx_3x(name: &str, family: Family) -> Option<(Vec<u8>, tables_v3::TableContext)> {
    let data = load(name)?;
    let ctx = tables_v3::TableContext::new(&data, family)
        .unwrap_or_else(|e| panic!("{name}: chain/BIT: {e}"));
    Some((data, ctx))
}

fn gb20x(name: &str) -> Option<(Vec<u8>, gb20x::Gb20xLayout)> {
    let data = load(name)?;
    let layout = gb20x::parse_layout(&data).unwrap_or_else(|e| panic!("{name}: layout: {e}"));
    Some((data, layout))
}

#[test]
fn turing_2070_full_decode() {
    let Some((data, ctx)) = ctx_3x(
        "2070_TU106_90061800EE_219W_10DE_1F07_1043_8671.rom",
        Family::Turing,
    ) else {
        return;
    };
    let policy = tables_v3::power_policy(&data, &ctx).unwrap();
    assert_eq!(policy.rows.len(), 20);
    assert_eq!(policy.tgp_index, 2);
    let tgp = &policy.rows[2];
    // 文件名标称 219W。
    assert_eq!(
        (tgp.minimum_raw, tgp.default_raw, tgp.maximum_raw),
        (105_000, 175_000, 219_000)
    );
    let limits = tables_v3::rail_limits(&data, &ctx).unwrap();
    assert_eq!(
        limits[0].output_maximum_uv, 1_068_750.0,
        "vix 同源值 1068750 µV"
    );
    let vmin = tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(vmin[0].pinned, "2070 VMIN should be clamp-pinned");
    let clocks = tables_v3::clock_ranges(&data, &ctx).unwrap();
    assert!(clocks.iter().any(|c| c.domain == "GPC" && c.range_stored));
    assert!(tables_v3::fan_tables(&data, &ctx).unwrap().is_some());
}

#[test]
fn ampere_3090_dual_rail() {
    let Some((data, ctx)) = ctx_3x(
        "3090_GA102_940226C035_371W_10DE_2204_1B4C_1454.rom",
        Family::Ampere,
    ) else {
        return;
    };
    let policy = tables_v3::power_policy(&data, &ctx).unwrap();
    let tgp = &policy.rows[policy.tgp_index];
    assert_eq!((tgp.default_raw, tgp.maximum_raw), (350_000, 371_000));
    let rails = tables_v3::rails(&data, &ctx).unwrap();
    assert_eq!(rails.rows.len(), 2, "GA102 双轨：NVVDD + MSVDD");
    assert_eq!((rails.rows[0].name, rails.rows[1].name), ("NVVDD", "MSVDD"));
    let limits = tables_v3::rail_limits(&data, &ctx).unwrap();
    for l in &limits {
        assert_eq!(l.output_maximum_uv, 1_081_250.0, "vix 同源值 1081250 µV");
        assert_eq!(l.equation_kind, nvoc_core::vbios::vfe::OutputKind::Constant);
    }
    let vmin = tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(vmin.iter().all(|v| v.pinned));
    assert_eq!(vmin[0].value_uv.unwrap(), 707_000.0);
    assert_eq!(vmin[1].value_uv.unwrap(), 726_000.0);
    // 时钟：XBAR/SYS 等 master 行全解出（与 vix 输出逐值对拍）。
    let clocks = tables_v3::clock_ranges(&data, &ctx).unwrap();
    let xbar = clocks.iter().find(|c| c.domain == "XBAR").unwrap();
    assert_eq!((xbar.minimum_mhz, xbar.maximum_mhz), (-1000, 1000));
    let dram = clocks.iter().find(|c| c.domain == "DRAM").unwrap();
    assert_eq!((dram.minimum_mhz, dram.maximum_mhz), (-1000, 3000));
}

#[test]
fn ada_4090_1000w_vmin_pinned() {
    let Some((data, ctx)) = ctx_3x(
        "4090_AD102_9502188083_1000W_10DE_2684_1043_889C.rom",
        Family::Ada,
    ) else {
        return;
    };
    let policy = tables_v3::power_policy(&data, &ctx).unwrap();
    let tgp = &policy.rows[policy.tgp_index];
    assert_eq!((tgp.minimum_raw, tgp.maximum_raw), (10_000, 1_000_000));
    let limits = tables_v3::rail_limits(&data, &ctx).unwrap();
    assert!((limits[0].output_maximum_uv - 1_080_000.0).abs() < 1.0);
    assert_eq!(
        limits[0].equation_kind,
        tables_v3::rail_limits(&data, &ctx).unwrap()[0].equation_kind
    );
    let vmin = tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(vmin[0].pinned);
    assert!((vmin[0].value_uv.unwrap() - 811_000.0).abs() < 1.0);
}

#[test]
fn ada_4090_max_vmin_shared_is_read_only() {
    // vix 文档：较新的 4090 镜像 VMIN 链入其它方程 → 不 pinned。
    let Some((data, ctx)) = ctx_3x("4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom", Family::Ada)
    else {
        return;
    };
    let vmin = tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(!vmin[0].pinned);
    assert_eq!(
        vmin[0].diagnostic,
        Some("is used by another equation or role")
    );
}

#[test]
fn ada_laptop_compare_rel_read_only() {
    // vix：compare REL 依赖 fused 输入，无电压界 → 只读（4060 移动端实测）。
    let Some((data, ctx)) = ctx_3x("ASUS_4060_OC_95071640F7.rom", Family::Ada) else {
        return;
    };
    assert!(tables_v3::rail_limits(&data, &ctx).is_err());
    let policy = tables_v3::power_policy(&data, &ctx).unwrap();
    assert!(policy.notebook, "ASUS 4060 笔记本头布局");
}

#[test]
fn gb20x_5060ti_directory_integrity_and_power() {
    let Some((data, layout)) = gb20x("5060Ti_98061F00C3.rom") else {
        return;
    };
    let integrity = gb20x::integrity(&data, &layout);
    assert!(integrity.valid, "{integrity:?}");
    assert!(integrity.directory_sha384_valid);
    assert!(layout.legacy_dir.is_some());
    let policy = gb20x::power_policy(&data, &layout).unwrap();
    assert!(!policy.notebook);
    let tgp = &policy.rows[8];
    assert_eq!(tgp.default_raw, 180_000);
    let limits = gb20x::rail_limits(&data, &layout).unwrap();
    assert!((limits[0].output_maximum_uv - 1_120_000.0).abs() < 1.0);
    // VMIN 是 min/max 方程树 → 只读（fail-closed）。
    let vmin = gb20x::rail_vmin(&data, &layout).unwrap();
    assert!(!vmin[0].pinned);
    assert_eq!(vmin[0].diagnostic, Some("is a min/max equation tree"));
}

#[test]
fn gb20x_5080_laptop_notebook_layout() {
    // vix 文档原文：5080 Laptop 行 2 持 5000/80000/175000 mW（nvidia-smi
    // min/default/max 三元组）。
    let Some((data, layout)) = gb20x("5080_es_98.03.34.00.16.rom") else {
        return;
    };
    let policy = gb20x::power_policy(&data, &layout).unwrap();
    assert!(policy.notebook);
    assert_eq!(
        (
            policy.rows[2].minimum_raw,
            policy.rows[2].default_raw,
            policy.rows[2].maximum_raw
        ),
        (5_000, 80_000, 175_000)
    );
    let rails = gb20x::rails(&data, &layout).unwrap();
    assert_eq!(rails.rows.len(), 2);
    // MSVDD VMIN 常数 625 mV pinned（NVVDD 为 min/max 树）。
    let vmin = gb20x::rail_vmin(&data, &layout).unwrap();
    assert!(
        (vmin[1].value_uv.unwrap() - 625_000.0).abs() < 1.0,
        "{vmin:?}"
    );
    // 该镜像的时钟域索引恰为 vix 已审第二种（SFF）布局 → 9 行可解。
    let clocks = gb20x::clock_ranges(&data, &layout).unwrap();
    assert_eq!(clocks.len(), 9);
}

#[test]
fn gb20x_5090_laptop_tgp_matches_filename() {
    let Some((data, layout)) = gb20x("5090_es_98.03.3C.00.07.rom") else {
        return;
    };
    let policy = gb20x::power_policy(&data, &layout).unwrap();
    assert_eq!(policy.rows[2].default_raw, 95_000);
    assert_eq!(policy.rows[2].maximum_raw, 175_000);
}

#[test]
fn vix_gui_field_map_on_4090_48g() {
    // vix-editor GUI（4090_48G 截图）逐值对照：slot 66W、NVVDD threshold
    // 600/900W、12V 45A×2、OCP 378A；角色名来自通道模板识别。
    let Some(data) = load("4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom") else {
        eprintln!("skip: 4090_48G rom not present");
        return;
    };
    let ctx = tables_v3::TableContext::new(&data, Family::Ada).expect("ctx");
    let channels = tables_v3::power_channels(&data, &ctx).unwrap();
    let role = |i: usize| channels.entries[i].role_name();
    assert_eq!(role(4), "pcie-slot");
    assert_eq!(role(9), "12v-input");
    let policy = tables_v3::power_policy(&data, &ctx).unwrap();
    let r = |i: usize| &policy.rows[i];
    assert_eq!(
        (r(5).channel_index, r(5).default_raw, r(5).maximum_raw),
        (4, 66_000, 66_000)
    );
    assert_eq!(
        (r(16).channel_index, r(16).default_raw, r(16).maximum_raw),
        (18, 600_000, 900_000)
    );
    assert_eq!(
        (r(9).channel_index, r(9).default_raw, r(9).maximum_raw),
        (9, 45_000, 45_000)
    );
    assert_eq!((r(13).channel_index, r(13).default_raw), (19, 378_000));
    // 风扇 extra curves（GUI "Extra fan curve 1/2"：98..108°C / 92..102°C）。
    let fans = tables_v3::fan_tables(&data, &ctx).unwrap().expect("fans");
    assert_eq!(fans.extra_curves.len(), 2);
    let e1 = &fans.extra_curves[0].1;
    assert_eq!(e1.points[0], (98 * 32, 2000));
    assert_eq!(e1.points[2], (108 * 32, 6000));
    let e2 = &fans.extra_curves[1].1;
    assert_eq!(e2.points[0].0, 92 * 32);
    assert_eq!(e2.points[2].0, 102 * 32);
}

#[test]
fn shunt_sense_scales_nonunity_samples() {
    // 通道条目 bytes 6..8（u16 Q4.12，0x1000=1.0）= 感测倍率：3090 NVVDD
    // 相位输出出厂 4.0；4070S 非公 12V-2x6 连接器 2.0；4090 CC07B shunt
    // mod 连接器 1.3333；参考卡恒 1.0。
    let scales = |name: &str, idx: usize| -> Option<(&'static str, f32)> {
        let data = load(name)?;
        let ctx = tables_v3::TableContext::new(&data, Family::Ampere)
            .or_else(|_| tables_v3::TableContext::new(&data, Family::Ada))
            .expect("ctx");
        let ch = &tables_v3::power_channels(&data, &ctx).unwrap().entries[idx];
        Some((ch.role_name(), ch.sense_scale().expect("scale")))
    };
    // 逐样本条件断言：任何一张样本 ROM 缺席（CI 无 reverse/VBIOS）就跳过
    // 该样本，不拖垮整个测试。
    if let Some(got) = scales("3090_GA102_940226C035_371W_10DE_2204_1B4C_1454.rom", 19) {
        assert_eq!(got, ("nvvdd-current", 4.0));
    }
    if let Some(got) = scales("AD104_4070S_10DE_2783_1458_4138_320W.rom", 3) {
        assert_eq!(got, ("12v-connector", 2.0));
    }
    // 0x1555 = 5461 → Q4.12 量化值 1.333252（4/3 的最近定点表示）。
    if let Some(got) = scales("4090_48G_AD102_MAX_450W_1075mV_95023CC07B.rom", 2) {
        assert_eq!(got, ("12v-connector", 5461.0 / 4096.0));
    }
    // 参考 4090 全通道 1.0。
    if let Some(data) = load("4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom") {
        let ctx = tables_v3::TableContext::new(&data, Family::Ada).expect("ctx");
        let channels = tables_v3::power_channels(&data, &ctx).unwrap();
        assert!(
            channels
                .entries
                .iter()
                .all(|c| c.sense_scale().is_none_or(|s| (s - 1.0).abs() < 1e-6))
        );
    }
}

#[test]
fn memtweak_v20_bitfields_decode_monotonic() {
    // NVRM 617.42 内嵌 RM sub_1C67060 逆向布局，3090 实测值固化：
    // 高频段 set0 最紧（CL3 p44=25 p45=26），低频段 set6 松（CL2 p44=10
    // p45=12），CCDL/CCDS = Config3 byte3 低/高 nibble。
    let Some(data) = load("3090_GA102_940226C035_371W_10DE_2204_1B4C_1454.rom") else {
        eprintln!("skip: 3090 rom not present");
        return;
    };
    let tweak = nvoc_core::vbios::legacy::find_memory_tweak_table(&data)
        .expect("parse ok")
        .expect("table present");
    assert_eq!(tweak.ver, 0x20);
    assert_eq!(tweak.decoded.len(), tweak.entries.len());
    let s0 = &tweak.decoded[0];
    assert_eq!((s0.cl_ramtype2, s0.param44, s0.param45), (3, 25, 26));
    assert_eq!((s0.ccdl, s0.ccds), (3, 2));
    let s6 = &tweak.decoded[6];
    assert_eq!((s6.cl_ramtype2, s6.param44, s6.param45), (2, 10, 12));
    assert_eq!((s6.ccdl, s6.ccds), (2, 2));
}

#[test]
fn all_in_scope_roms_have_clean_image_chains() {
    let dir = std::path::Path::new(ROM_DIR);
    if !dir.is_dir() {
        eprintln!("skip: {ROM_DIR} not present");
        return;
    }
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rom") {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let (pci, _npds, issues) = scan_images(&data);
        assert!(issues.is_empty(), "{}: {issues:?}", path.display());
        if let Some(legacy) = pci.iter().find(|p| p.is_legacy()) {
            let versions = nvoc_core::vbios::families::scan_bios_versions(&data);
            let arch = nvoc_core::vbios::families::architecture(
                legacy.device,
                &versions.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            );
            if arch.in_scope {
                checked += 1;
            }
        }
    }
    assert!(checked >= 15, "expected ≥15 in-scope ROMs, saw {checked}");
}
