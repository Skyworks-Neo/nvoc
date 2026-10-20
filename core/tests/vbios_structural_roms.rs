//! 实机 ROM 结构化解码回归（opportunistic：`reverse/VBIOS` 不存在则跳过，
//! 与 legacy_vbios_parser 的 `*_when_present` 惯例一致）。
//!
//! 断言值来源三重校准：ROM 文件名标称功率、vix-editor（Python）同源解析
//! 输出、vix 文档记载（如 5080 Laptop 的 5000/80000/175000 mW nvidia-smi
//! 三元组、新 4090 VMIN 链入其它方程）。跑法：
//! `cargo test -p nvoc-core --test vbios_structural_roms`

use nvoc_core::vbios_families::Family;
use nvoc_core::vbios_gb20x;
use nvoc_core::vbios_image_chain::scan_images;
use nvoc_core::vbios_tables_v3;

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

fn ctx_3x(name: &str, family: Family) -> Option<(Vec<u8>, vbios_tables_v3::TableContext)> {
    let data = load(name)?;
    let ctx = vbios_tables_v3::TableContext::new(&data, family)
        .unwrap_or_else(|e| panic!("{name}: chain/BIT: {e}"));
    Some((data, ctx))
}

fn gb20x(name: &str) -> Option<(Vec<u8>, vbios_gb20x::Gb20xLayout)> {
    let data = load(name)?;
    let layout = vbios_gb20x::parse_layout(&data).unwrap_or_else(|e| panic!("{name}: layout: {e}"));
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
    let policy = vbios_tables_v3::power_policy(&data, &ctx).unwrap();
    assert_eq!(policy.rows.len(), 20);
    assert_eq!(policy.tgp_index, 2);
    let tgp = &policy.rows[2];
    // 文件名标称 219W。
    assert_eq!(
        (tgp.minimum_raw, tgp.default_raw, tgp.maximum_raw),
        (105_000, 175_000, 219_000)
    );
    let limits = vbios_tables_v3::rail_limits(&data, &ctx).unwrap();
    assert_eq!(
        limits[0].output_maximum_uv, 1_068_750.0,
        "vix 同源值 1068750 µV"
    );
    let vmin = vbios_tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(vmin[0].pinned, "2070 VMIN should be clamp-pinned");
    let clocks = vbios_tables_v3::clock_ranges(&data, &ctx).unwrap();
    assert!(clocks.iter().any(|c| c.domain == "GPC" && c.range_stored));
    assert!(vbios_tables_v3::fan_tables(&data, &ctx).unwrap().is_some());
}

#[test]
fn ampere_3090_dual_rail() {
    let Some((data, ctx)) = ctx_3x(
        "3090_GA102_940226C035_371W_10DE_2204_1B4C_1454.rom",
        Family::Ampere,
    ) else {
        return;
    };
    let policy = vbios_tables_v3::power_policy(&data, &ctx).unwrap();
    let tgp = &policy.rows[policy.tgp_index];
    assert_eq!((tgp.default_raw, tgp.maximum_raw), (350_000, 371_000));
    let rails = vbios_tables_v3::rails(&data, &ctx).unwrap();
    assert_eq!(rails.rows.len(), 2, "GA102 双轨：NVVDD + MSVDD");
    assert_eq!((rails.rows[0].name, rails.rows[1].name), ("NVVDD", "MSVDD"));
    let limits = vbios_tables_v3::rail_limits(&data, &ctx).unwrap();
    for l in &limits {
        assert_eq!(l.output_maximum_uv, 1_081_250.0, "vix 同源值 1081250 µV");
        assert_eq!(l.equation_kind, nvoc_core::vbios_vfe::OutputKind::Constant);
    }
    let vmin = vbios_tables_v3::rail_vmin(&data, &ctx).unwrap();
    assert!(vmin.iter().all(|v| v.pinned));
    assert_eq!(vmin[0].value_uv.unwrap(), 707_000.0);
    assert_eq!(vmin[1].value_uv.unwrap(), 726_000.0);
    // 时钟：XBAR/SYS 等 master 行全解出（与 vix 输出逐值对拍）。
    let clocks = vbios_tables_v3::clock_ranges(&data, &ctx).unwrap();
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
    let policy = vbios_tables_v3::power_policy(&data, &ctx).unwrap();
    let tgp = &policy.rows[policy.tgp_index];
    assert_eq!((tgp.minimum_raw, tgp.maximum_raw), (10_000, 1_000_000));
    let limits = vbios_tables_v3::rail_limits(&data, &ctx).unwrap();
    assert!((limits[0].output_maximum_uv - 1_080_000.0).abs() < 1.0);
    assert_eq!(
        limits[0].equation_kind,
        vbios_tables_v3::rail_limits(&data, &ctx).unwrap()[0].equation_kind
    );
    let vmin = vbios_tables_v3::rail_vmin(&data, &ctx).unwrap();
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
    let vmin = vbios_tables_v3::rail_vmin(&data, &ctx).unwrap();
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
    assert!(vbios_tables_v3::rail_limits(&data, &ctx).is_err());
    let policy = vbios_tables_v3::power_policy(&data, &ctx).unwrap();
    assert!(policy.notebook, "ASUS 4060 笔记本头布局");
}

#[test]
fn gb20x_5060ti_directory_integrity_and_power() {
    let Some((data, layout)) = gb20x("5060Ti_98061F00C3.rom") else {
        return;
    };
    let integrity = vbios_gb20x::integrity(&data, &layout);
    assert!(integrity.valid, "{integrity:?}");
    assert!(integrity.directory_sha384_valid);
    assert!(layout.legacy_dir.is_some());
    let policy = vbios_gb20x::power_policy(&data, &layout).unwrap();
    assert!(!policy.notebook);
    let tgp = &policy.rows[8];
    assert_eq!(tgp.default_raw, 180_000);
    let limits = vbios_gb20x::rail_limits(&data, &layout).unwrap();
    assert!((limits[0].output_maximum_uv - 1_120_000.0).abs() < 1.0);
    // VMIN 是 min/max 方程树 → 只读（fail-closed）。
    let vmin = vbios_gb20x::rail_vmin(&data, &layout).unwrap();
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
    let policy = vbios_gb20x::power_policy(&data, &layout).unwrap();
    assert!(policy.notebook);
    assert_eq!(
        (
            policy.rows[2].minimum_raw,
            policy.rows[2].default_raw,
            policy.rows[2].maximum_raw
        ),
        (5_000, 80_000, 175_000)
    );
    let rails = vbios_gb20x::rails(&data, &layout).unwrap();
    assert_eq!(rails.rows.len(), 2);
    // MSVDD VMIN 常数 625 mV pinned（NVVDD 为 min/max 树）。
    let vmin = vbios_gb20x::rail_vmin(&data, &layout).unwrap();
    assert!(
        (vmin[1].value_uv.unwrap() - 625_000.0).abs() < 1.0,
        "{vmin:?}"
    );
    // 该镜像的时钟域索引恰为 vix 已审第二种（SFF）布局 → 9 行可解。
    let clocks = vbios_gb20x::clock_ranges(&data, &layout).unwrap();
    assert_eq!(clocks.len(), 9);
}

#[test]
fn gb20x_5090_laptop_tgp_matches_filename() {
    let Some((data, layout)) = gb20x("5090_es_98.03.3C.00.07.rom") else {
        return;
    };
    let policy = vbios_gb20x::power_policy(&data, &layout).unwrap();
    assert_eq!(policy.rows[2].default_raw, 95_000);
    assert_eq!(policy.rows[2].maximum_raw, 175_000);
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
            let versions = nvoc_core::vbios_families::scan_bios_versions(&data);
            let arch = nvoc_core::vbios_families::architecture(
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
