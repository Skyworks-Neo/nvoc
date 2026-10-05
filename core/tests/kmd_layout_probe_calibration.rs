//! layout_probe 对真实 nvlddmkm 镜像的校准测试。
//!
//! 地面真值(2026-10-06 三代逆向实证,见
//! docs/reverse-engineering/nvapi/kmd-power-policy-layout-r610-74.md):
//!
//! | 字段 | 610.74 | 616.92 |
//! |---|---|---|
//! | state 槽 | 0x13AAD58 | 0x13B2E18 |
//! | state→表 | 0x200 | 0x208 |
//! | 表 count | 0x48440 | 0x48C48 |
//! | 表 Major | 0x48240 | 0x48A48 |
//! | Major→root | 0x2510 | 0x25B0 |
//! | root init | 0x3CE0 | 0x3D10 |
//! | root UPPER | 0x3CF4 | 0x3D24 |
//!
//! 镜像不入库:610.74 取本机 DriverStore,616.92 取 reverse/(untracked);
//! 缺文件时 skip(不算失败)。
#![cfg(windows)]

use nvoc_core::kmd::layout_probe::probe;
use std::path::Path;

struct GroundTruth {
    path: &'static str,
    timestamp: u32,
    size_of_image: u32,
    slot: u64,
    state_d: u32,
    count: u32,
    major: u32,
    major_root: u32,
    init: u32,
    upper: u32,
}

const CASES: &[GroundTruth] = &[
    GroundTruth {
        path: r"C:\Windows\System32\DriverStore\FileRepository\nvami.inf_amd64_1a0e2bbd9919af1d\nvlddmkm.sys",
        timestamp: 0x6A46A468,
        size_of_image: 0x07370000,
        slot: 0x13AAD58,
        state_d: 0x200,
        count: 0x48440,
        major: 0x48240,
        major_root: 0x2510,
        init: 0x3CE0,
        upper: 0x3CF4,
    },
    GroundTruth {
        path: r"D:\git-repo\nvoc\reverse\nvlddmkm_616.92.sys",
        timestamp: 0x6A9B4070,
        size_of_image: 0x06D3E000,
        slot: 0x13B2E18,
        state_d: 0x208,
        count: 0x48C48,
        major: 0x48A48,
        major_root: 0x25B0,
        init: 0x3D10,
        upper: 0x3D24,
    },
];

#[test]
fn probe_matches_ground_truth() {
    for case in CASES {
        if !Path::new(case.path).exists() {
            println!("skip(镜像不在位): {}", case.path);
            continue;
        }
        let img = std::fs::read(case.path).unwrap_or_else(|e| panic!("读 {} 失败: {e}", case.path));
        let layout = probe(&img)
            .unwrap_or_else(|e| panic!("probe {:#x} 失败: {e:?}(fail-closed 生效)", case.timestamp));
        println!("=== {:#x} anchors ===", case.timestamp);
        for a in &layout.anchors {
            println!("  {a}");
        }
        assert_eq!(layout.timestamp, case.timestamp, "timestamp");
        assert_eq!(layout.size_of_image, case.size_of_image, "size_of_image");
        assert_eq!(
            layout.global_slot_rva, case.slot,
            "{:#x} state 槽",
            case.timestamp
        );
        assert_eq!(layout.state_table_off, case.state_d, "state→表");
        assert_eq!(layout.table_count_off, case.count, "表 count");
        assert_eq!(layout.entry_major_off, case.major, "表 Major");
        assert_eq!(layout.entry_id_off, case.major + 8, "表 GPU-ID");
        assert_eq!(layout.entry_stride, 0x10);
        assert_eq!(layout.major_root_off, case.major_root, "Major→root");
        assert_eq!(layout.root_init_off, case.init, "root init");
        assert_eq!(
            layout.root_upper_off,
            case.upper,
            "{:#x} root UPPER",
            case.timestamp
        );
        // 结构不变量
        assert_eq!(layout.root_elig_off, layout.root_init_off + 1);
        assert_eq!(layout.root_amount_active_off, layout.root_init_off + 2);
        assert_eq!(layout.root_base_off, layout.root_init_off + 4);
        assert_eq!(layout.root_amount_off, layout.root_init_off + 8);
        assert_eq!(layout.root_key_off, layout.root_init_off + 0xC);
        assert_eq!(layout.root_lower_off, layout.root_init_off + 0x10);
    }
}
