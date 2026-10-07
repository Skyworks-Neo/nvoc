//! layout_probe 对本机在役镜像的现场诊断(2026-10-07,排查
//! "全部 GPU 表条目都未过身份门"):打印完整推导布局 + 锚点 trail,
//! 并逐字段对照 610.47 台式版(TU104/nv_dispi)地面真值。
//! 地面真值来源:idalib 逆向(2026-10-07,F7 签名唯一命中 0x4ED848 →
//! 生成器 0x4ED6B0;state 链 sub_1401060D0 实证)——与 610.74 数据面零漂移。
//! 镜像不入库:缺文件时 skip(不算失败)。路径可用 NVOC_PROBE_IMAGE 覆盖。
#![cfg(windows)]

use nvoc_core::kmd::layout_probe::probe;
use std::path::Path;

struct GroundTruth {
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

const LIVE_610_47: GroundTruth = GroundTruth {
    timestamp: 0x6A0CBED2,
    size_of_image: 0x0736D000,
    slot: 0x13AAD58,
    state_d: 0x200,
    count: 0x48440,
    major: 0x48240,
    major_root: 0x2510,
    init: 0x3CE0,
    upper: 0x3CF4,
};

#[test]
fn probe_live_image_diagnose() {
    let path = std::env::var("NVOC_PROBE_IMAGE").unwrap_or_else(|_| {
        r"C:\WINDOWS\system32\DriverStore\FileRepository\nv_dispi.inf_amd64_c8bc842500fab35b\nvlddmkm.sys".into()
    });
    if !Path::new(&path).exists() {
        println!("skip(镜像不在位): {path}");
        return;
    }
    let img = std::fs::read(&path).unwrap_or_else(|e| panic!("读 {path} 失败: {e}"));
    let layout = probe(&img).unwrap_or_else(|e| panic!("probe 失败(fail-closed 生效): {e:?}"));
    println!("=== 在役镜像 {path} ===");
    println!(
        "timestamp={:#x} size_of_image={:#x} image_base={:#x}",
        layout.timestamp, layout.size_of_image, layout.image_base
    );
    println!("--- 锚点 trail ---");
    for a in &layout.anchors {
        println!("  {a}");
    }
    println!("--- 推导布局 ---");
    println!("global_slot_rva = {:#x}", layout.global_slot_rva);
    println!("state_table_off = {:#x}", layout.state_table_off);
    println!("table_count_off = {:#x}", layout.table_count_off);
    println!("entry_major_off = {:#x}", layout.entry_major_off);
    println!("entry_id_off    = {:#x}", layout.entry_id_off);
    println!("entry_stride    = {:#x}", layout.entry_stride);
    println!("major_root_off  = {:#x}", layout.major_root_off);
    println!("root_init_off   = {:#x}", layout.root_init_off);
    println!("root_upper_off  = {:#x}", layout.root_upper_off);
    println!("generator_rva   = {:#x}", layout.generator_rva);
    println!(
        "rm_get/set_cmd  = {:?}/{:?}",
        layout.rm_get_cmd, layout.rm_set_cmd
    );
    println!(
        "get/set_handler = {:?}/{:?}",
        layout.get_handler_rva, layout.set_handler_rva
    );

    // 与地面真值逐字段对照(全部打印,不短路;末尾汇总)
    let g = &LIVE_610_47;
    let checks: Vec<(&str, bool, String, String)> = vec![
        (
            "timestamp",
            layout.timestamp == g.timestamp,
            format!("{:#x}", layout.timestamp),
            format!("{:#x}", g.timestamp),
        ),
        (
            "size_of_image",
            layout.size_of_image == g.size_of_image,
            format!("{:#x}", layout.size_of_image),
            format!("{:#x}", g.size_of_image),
        ),
        (
            "global_slot_rva",
            layout.global_slot_rva == g.slot,
            format!("{:#x}", layout.global_slot_rva),
            format!("{:#x}", g.slot),
        ),
        (
            "state_table_off",
            layout.state_table_off == g.state_d,
            format!("{:#x}", layout.state_table_off),
            format!("{:#x}", g.state_d),
        ),
        (
            "table_count_off",
            layout.table_count_off == g.count,
            format!("{:#x}", layout.table_count_off),
            format!("{:#x}", g.count),
        ),
        (
            "entry_major_off",
            layout.entry_major_off == g.major,
            format!("{:#x}", layout.entry_major_off),
            format!("{:#x}", g.major),
        ),
        (
            "entry_id_off",
            layout.entry_id_off == g.major + 8,
            format!("{:#x}", layout.entry_id_off),
            format!("{:#x}", g.major + 8),
        ),
        (
            "major_root_off",
            layout.major_root_off == g.major_root,
            format!("{:#x}", layout.major_root_off),
            format!("{:#x}", g.major_root),
        ),
        (
            "root_init_off",
            layout.root_init_off == g.init,
            format!("{:#x}", layout.root_init_off),
            format!("{:#x}", g.init),
        ),
        (
            "root_upper_off",
            layout.root_upper_off == g.upper,
            format!("{:#x}", layout.root_upper_off),
            format!("{:#x}", g.upper),
        ),
    ];
    let mut bad = Vec::new();
    println!("--- 与 610.47 地面真值对照(推导 vs idalib)---");
    for (name, ok, got, want) in &checks {
        println!(
            "  [{}] {name}: {got} (期望 {want})",
            if *ok { "✓" } else { "✗" }
        );
        if !ok {
            bad.push(name);
        }
    }
    assert!(
        bad.is_empty(),
        "layout_probe 与 idalib 地面真值失配的字段: {bad:?}"
    );
}
