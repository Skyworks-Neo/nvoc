//! GB20x 其余解析面——vix editor `rom/blackwell.py`（GPL-3）剩余函数的
//! Rust 移植（read-only 解码；OCP 行的 sha256 关联信任模型不移植——
//! 那是 vix 写入面的"是否与实测 GET 同形"门，解码只需要结构）。
//!
//! 常量与几何全部来自 vix 源（2026-11 快照），注释保留其 GSP-RM 依据。
#![allow(dead_code)]
#![allow(clippy::type_complexity)]

use crate::error::Error;
use crate::vbios::gb20x::Gb20xLayout;
use crate::vbios::image_chain::RomImage;

fn u16_at(data: &[u8], a: usize) -> Result<u16, Error> {
    Ok(u16::from(*data.get(a).ok_or("vbios: oob")?)
        | u16::from(*data.get(a + 1).ok_or("vbios: oob")?) << 8)
}

fn u32_at(data: &[u8], a: usize) -> Result<u32, Error> {
    let b = data.get(a..a + 4).ok_or("vbios: oob")?;
    Ok(u32::from_le_bytes(b.try_into().unwrap()))
}

fn i32_at(data: &[u8], a: usize) -> Result<i32, Error> {
    Ok(u32_at(data, a)? as i32)
}

fn f32_at(data: &[u8], a: usize) -> Result<f32, Error> {
    Ok(f32::from_le_bytes(
        data.get(a..a + 4).ok_or("vbios: oob")?.try_into().unwrap(),
    ))
}

// ── 常量（vix blackwell.py 头部）─────────────────────────────────────────

/// 单 rail 镜像（NVVDD-only）PCI device id。
pub const SINGLE_RAIL_IDS: [u16; 3] = [0x2BB1, 0x2D04, 0x2F04];
/// 实测 GET 覆盖的桌面 device id。
const REVIEWED_IDS: [u16; 5] = [0x2BB4, 0x2B85, 0x2C02, 0x2C05, 0x2C33];
/// NVVDD 输出监视的功率通道 26B（七源逐字节相同；Max-Q=通道 27）。
const NVVDD_CHANNEL: [u8; 26] = [
    0x01, 0x01, 0x00, 0x1b, 0xb7, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// NVVDD 通道读的功率设备 25B。
const NVVDD_POWER_DEVICE: [u8; 25] = [
    0x02, 0x00, 0x00, 0x00, 0x00, 0x14, 0x02, 0x00, 0x09, 0x00, 0x00, 0xff, 0x60, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// MSVDD 输出监视通道（Max-Q=28）。
const MSVDD_CHANNEL: [u8; 26] = [
    0x01, 0x10, 0x00, 0x1b, 0xb7, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// MSVDD 功率设备。
const MSVDD_POWER_DEVICE: [u8; 25] = [
    0x02, 0x20, 0x00, 0x00, 0x00, 0x14, 0x02, 0xff, 0x00, 0x00, 0x00, 0x01, 0x6d, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// 辅助供电连接器输入通道（class-5 单输入 12V rail 0xda）。
const CONNECTOR_INPUT_CHANNEL: [u8; 26] = [
    0x05, 0xda, 0x00, 0x1b, 0xb7, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// 可能被 override profile 点名的策略行（连接器行永不在此列）。
const PROFILE_POLICIES: [usize; 4] = [2, 7, 8, 9];
/// 笔记本 TGP 行（row 2）字节 0..12 指纹前缀。
const NOTEBOOK_TGP_ROW_PREFIX: [u8; 13] = [
    0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x2b, 0x07, 0x00, 0xcc, 0x10, 0xff,
];
/// row 2 +73（u32 mW）候选 base TGP 偏移。
const NOTEBOOK_BASE_TGP_OFFSET: usize = 73;
/// 可持有 VFE 选择器的 rail 行 u16 字位（vix RAIL_SELECTORS）。
pub const RAIL_SELECTORS: [usize; 22] = [
    0x0D, 0x0F, 0x11, 0x13, 0x15, 0x17, 0x1D, 0x1F, 0x21, 0x23, 0x29, 0x2B, 0x2D, 0x2F, 0x31, 0x33,
    0x35, 0x37, 0x39, 0x3B, 0x3D, 0x3F,
];
/// 驱动评估 VMIN 的选择器槽。
pub const VMIN_SELECTOR: usize = 0x13;
/// 钩子承载 role（REL/ALT/OV/VMIN）。
pub const HOOK_ROLES: [(&str, usize); 4] = [
    ("REL", 0x0D),
    ("ALT", 0x0F),
    ("OV", 0x11),
    ("VMIN", VMIN_SELECTOR),
];
/// 运行时工具接受的 VFE 头两段（字节 0..6 与 7..14）。
const HOOK_VFE_HEADER: [([u8; 6], [u8; 7]); 1] = [(
    [0x30, 0x0e, 0x1d, 0xff, 0x19, 0x00],
    [0x00, 0x00, 0x01, 0x08, 0x05, 0x00, 0x20],
)];
/// 运行时工具接受的最小 BIT P token size。
const HOOK_BIT_P_SIZE: u16 = 0xF8;
/// stock 模板变量 29B（type 9 全零域）。
pub const VFE_HOOK_VARIABLE: [u8; 29] = [
    0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0x07, 0x08, 0x00, 0x04, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0,
];
/// 未用变量槽 29B（type 0，域 0xff@9）。
pub const VFE_FREE_VARIABLE: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const VFE_HOOK_LIMIT_UV: f32 = 1_250_000.0;
/// 钩子方程字节 3..22（钳 ±1.25V、常数 0、斜率 1、曲率 0）。
fn vfe_hook_body() -> [u8; 20] {
    let body: [f32; 5] = [-VFE_HOOK_LIMIT_UV, VFE_HOOK_LIMIT_UV, 0.0, 1.0, 0.0];
    let mut b = [0u8; 20];
    for (i, v) in body.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    b
}
/// 未用 VFE 方程槽 25B（type 0、u16 链接 0xFFFF、零浮点、尾 0xFF）。
pub fn vfe_free_slot_25() -> [u8; 25] {
    let mut b = [0u8; 25];
    b[1] = 0xFF;
    b[2] = 0xFF;
    b[23] = 0;
    b[24] = 0xFF;
    b
}
/// 钩子方程 25B：kind 1 on `var`，next=`target`，VFE_HOOK_BODY，µV 输出。
pub fn vfe_hook_equation(var: u8, target: u16) -> [u8; 25] {
    let mut b = [0u8; 25];
    b[0] = 1;
    b[1] = var;
    b[2] = (target & 0xFF) as u8;
    b[3..23].copy_from_slice(&vfe_hook_body());
    b[23] = 2;
    b[24] = (target >> 8) as u8;
    b
}
/// compare 方程输出旗标 bit4：判据是 11/12 处的方程值而非浮点。
pub const VFE_CRITERION_EQUATION: u8 = 0x10;
/// 不设限的输出钳位字节（-FLT_MAX,+FLT_MAX）。
pub const VFE_UNCLAMPED: [u8; 8] = [0xff, 0xff, 0x7f, 0xff, 0xff, 0x7f, 0x7f, 0x7f];
/// 每通道风扇五表：名 → (BIT P 偏移, 头, 头大小, 条目大小, 条目数, 对象)。
pub const FAN_TABLES: [(&str, usize, [u8; 4], usize, usize, usize, u32); 5] = [
    ("cooler", 0x58, [0x10, 9, 0x1A, 4], 9, 0x1A, 4, 0x1D),
    ("policy", 0x5C, [0x20, 4, 0x33, 0x0B], 4, 0x33, 0x0B, 0x1E),
    ("mapping", 0x64, [0x10, 4, 5, 4], 4, 5, 4, 0x20),
    ("control", 0xDC, [0x10, 4, 0x0D, 4], 4, 0x0D, 4, 0x80),
    ("speed", 0xE0, [0x10, 4, 6, 4], 4, 6, 4, 0x83),
];
/// 策略条目旗标字节偏移（51B 条目的最后一字节）。
pub const FAN_POLICY_FLAG_OFFSET: usize = 0x32;
/// 热策略表（P+0x50，对象 0x1b）：v0x20、头 12|22、48B 条目。
const THERMAL_POLICY: (u8, [usize; 2], usize) = (0x20, [12, 22], 48);
/// 出厂时钟记录头（P+0x98，对象 0x26）。
const FACTORY_CLOCK_HEADER: [u8; 7] = [0x10, 7, 5, 2, 5, 2, 0];
/// PERF_CF 控制器/策略表几何。
const PERF_CF_CONTROLLERS: (u8, [usize; 2], usize, u32) = (0x10, [6, 14], 57, 0x60);
const PERF_CF_POLICIES: (u8, usize, usize, u32) = (0x10, 5, 10, 0x61);
/// LPWR 特性表：名 → (BIT P 偏移, 对象, 头大小, 条目大小, 使能字宽)。
pub const LPWR_TABLES: [(&str, usize, u32, usize, usize, usize); 8] = [
    ("gr_pg", 0x80, 0x39, 10, 5, 4),
    ("ms", 0x84, 0x3A, 7, 16, 4),
    ("gr_rg", 0xD4, 0x7E, 12, 1, 1),
    ("difr", 0xE8, 0x8E, 11, 18, 4),
    ("nvdec", 0x11C, 0xB4, 10, 2, 2),
    ("nvenc", 0x120, 0xB5, 10, 2, 2),
    ("ofa", 0x124, 0xB6, 10, 2, 2),
    ("nvjpg", 0x128, 0xB7, 10, 2, 2),
];
/// P+0x104 表（对象 0xA7）已审条目类型。
const P104_ENTRY_TYPES: [u8; 3] = [0, 1, 3];

// ── 基础访问 ─────────────────────────────────────────────────────────────

fn opt_pointer(layout: &Gb20xLayout, data: &[u8], token: u8, off: usize) -> Option<usize> {
    let entry = layout.bit.token(token)?;
    if off + 4 > usize::from(entry.size) {
        return None;
    }
    let raw = u32_at(data, entry.data + off).ok()?;
    if raw == 0 {
        return None;
    }
    layout.pointer(data, token, off).ok()
}

fn p_pointer(layout: &Gb20xLayout, data: &[u8], off: usize) -> Result<usize, Error> {
    layout.pointer(data, b'P', off)
}

fn owner(layout: &Gb20xLayout, at: usize, size: usize, id: u32) -> Result<(), Error> {
    let o = layout.object_for(at, size)?;
    if o.id != id {
        return Err(Error::Custom(format!(
            "vbios: object {id:#x} ownership violated at {at:#x}"
        )));
    }
    Ok(())
}

// ── 功率行全量（含 OCP/输入电流/slot/connector 面）───────────────────────

/// 一条功率策略行的完整解码（GB20x 0x66B 行）。
#[derive(Debug, Clone, PartialEq)]
pub struct PowerRowFull {
    pub row: usize,
    pub offset: usize,
    /// 行首字节（type；bit4=mA 旗标在 3x，GB20x 用 +1 字节 bit0）。
    pub kind: u8,
    /// 语义名（7=输出 OCP、11=输入电流、1=TGP/board、2=slot/connector、
    /// 12/13=runtime/linked、0=空）。
    pub kind_name: &'static str,
    /// 行 +1 的单位位（bit0 = mA）。
    pub unit_ma: bool,
    /// 监视器索引（行 +5）。
    pub monitor: u8,
    pub minimum_raw: u32,
    pub default_raw: u32,
    pub maximum_raw: u32,
    pub minimum_offset: usize,
    pub default_offset: usize,
    pub maximum_offset: usize,
    /// u32 判别码（行 +1..+5；输出 OCP 行 =1）。
    pub discriminator_u32: u32,
    pub ordered: bool,
}

fn kind_name(kind: u8) -> &'static str {
    match kind {
        0 => "zeroed",
        1 => "board-power",
        2 => "slot-connector",
        3 => "board-input",
        4 | 5 => "connector",
        6 => "summary",
        7 => "output-ocp",
        9 => "current-rail",
        10 => "nvvdd-ocp-3x",
        11 => "input-current",
        12 => "runtime-limit",
        13 => "linked-limit",
        _ => "unknown",
    }
}

/// 功率通道 26B 的语义 role 名（3x `PowerChannelEntry::role_name` 同款
/// 字节判定；GB20x 通道与 Ada 通道逐字节兼容——vix OUTPUT_CHAINS 依据）。
pub fn channel_role_name(bytes: &[u8]) -> &'static str {
    if bytes.len() < 15 {
        return "";
    }
    let twelve_v_sense =
        bytes[2] == 0x00 && bytes[3] == 0x1B && bytes[4] == 0xB7 && bytes[5] == 0x00;
    let class = bytes[0];
    let rail = bytes[1];
    if class == 2 && rail == 0xF5 {
        return "total-board";
    }
    if twelve_v_sense {
        match (class, rail) {
            (5, 0xFF) => return "pcie-slot",
            (5, 0xE0) => return "12v-input",
            (5, 0xF7) => return "12v-connector",
            (1, 0x01) => {
                return match bytes[14] {
                    0x05 => "nvvdd-power",
                    0x06 => "nvvdd-current",
                    _ => "nvvdd-out",
                };
            }
            _ => {}
        }
    }
    if class == 5 && matches!(rail, 0xDA | 0xDB | 0xDC | 0xFA | 0xFB | 0xFC | 0xE4) {
        return "connector";
    }
    ""
}

/// 监视器号 → 通道 26B（power_channel(index) 的只读版）。
pub fn power_channel_bytes(data: &[u8], layout: &Gb20xLayout, index: u8) -> Option<Vec<u8>> {
    let at = p_pointer(layout, data, 0x3C).ok()?;
    if usize::from(index) >= 0x20 {
        return None;
    }
    data.get(at + 0x11 + 0x1A * usize::from(index)..at + 0x11 + 0x1A * (usize::from(index) + 1))
        .map(|c| c.to_vec())
}

/// 功率策略 20 行全量（vix power_rows + 行语义命名；OCP=行 13/14、
/// 输入电流=行 9、slot=行 18/19、连接器=形匹配扫描）。
pub fn power_rows_full(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<PowerRowFull>, Error> {
    let at = p_pointer(layout, data, 11 * 4)?;
    let (version, hs, es, count) = (
        data[at],
        usize::from(data[at + 1]),
        usize::from(data[at + 2]),
        usize::from(data[at + 3]),
    );
    if !matches!((version, hs, es), (0x40, 0x38, 0x66) | (0x40, 0x3D, 0x66)) || count != 20 {
        return Err(Error::from("vbios: unsupported power policy table"));
    }
    owner(layout, at, hs + es * count, 0x13)?;
    let mut out = Vec::new();
    for i in 0..count {
        let row = at + hs + es * i;
        let kind = data[row];
        let unit_ma = data[row + 1] & 1 != 0;
        let (lo, def, hi) = (
            u32_at(data, row + 13)?,
            u32_at(data, row + 17)?,
            u32_at(data, row + 21)?,
        );
        out.push(PowerRowFull {
            row: i,
            offset: row,
            kind,
            kind_name: kind_name(kind & 0x0F),
            unit_ma,
            monitor: data[row + 5],
            minimum_raw: lo,
            default_raw: def,
            maximum_raw: hi,
            minimum_offset: row + 13,
            default_offset: row + 17,
            maximum_offset: row + 21,
            discriminator_u32: u32_at(data, row + 1)?,
            ordered: lo <= def && def <= hi,
        });
    }
    Ok(out)
}

/// 笔记本功率布局判定（row2=TGP、行 7/8 空）。
pub fn notebook_power_layout(data: &[u8], layout: &Gb20xLayout) -> Result<bool, Error> {
    let at = p_pointer(layout, data, 11 * 4)?;
    let rows = power_rows_full(data, layout)?;
    Ok((data[at + 21..at + 24] == [0xFF, 0xFF, 0x02]
        || data[at + 21..at + 24] == [0xFF, 0x02, 0x02])
        && rows.get(2).is_some_and(|r| r.kind == 1)
        && [7usize, 8].iter().all(|&i| {
            rows.get(i)
                .is_some_and(|r| r.kind == 0 && r.maximum_raw == 0)
        }))
}

/// 单 rail（NVVDD-only）判定。
pub fn single_rail(data: &[u8], layout: &Gb20xLayout) -> bool {
    crate::vbios::gb20x::rail_count(data, layout).unwrap_or(0) == 1
}

/// 运行时功率限开关槽（笔记本布局 P 头 +22）。
pub fn runtime_limit_slot(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<(usize, u8, u8)>, Error> {
    if !notebook_power_layout(data, layout)? {
        return Ok(None);
    }
    let rows = power_rows_full(data, layout)?;
    let row2 = rows.get(2).ok_or("vbios: power row 2 absent")?;
    if u32_at(data, row2.offset + 1)? & 1 != 0 {
        return Ok(None);
    }
    let at = p_pointer(layout, data, 11 * 4)? + 22;
    Ok(Some((at, data[at + 1], u8::from(data[at] == 0xFF))))
}

/// 笔记本 base TGP 候选（row2 +73 u32 mW）。
pub fn notebook_base_tgp(data: &[u8], layout: &Gb20xLayout) -> Result<Option<u32>, Error> {
    if !notebook_power_layout(data, layout)? {
        return Ok(None);
    }
    let rows = power_rows_full(data, layout)?;
    let row = &rows[2];
    let at = row.offset + NOTEBOOK_BASE_TGP_OFFSET;
    let value = u32_at(data, at)?;
    if data[row.offset..row.offset + 12] != NOTEBOOK_TGP_ROW_PREFIX[..12]
        || !(row.default_raw..=row.maximum_raw).contains(&value)
    {
        return Ok(None);
    }
    Ok(Some(value))
}

/// 通道/设备链校验（监视器 → 通道 → 设备 == 该 rail 的已审链）。
fn output_channel_chain(data: &[u8], layout: &Gb20xLayout, monitor: u8, rail: &str) -> bool {
    let (chan_want, dev_want): (&[u8], &[u8]) = match rail {
        "NVVDD" => (&NVVDD_CHANNEL, &NVVDD_POWER_DEVICE),
        "MSVDD" => (&MSVDD_CHANNEL, &MSVDD_POWER_DEVICE),
        _ => return false,
    };
    let Ok(at) = p_pointer(layout, data, 0x3C) else {
        return false;
    };
    if monitor >= 0x20 {
        return false;
    }
    let chan = match data
        .get(at + 0x11 + 0x1A * usize::from(monitor)..at + 0x11 + 0x1A * (usize::from(monitor) + 1))
    {
        Some(c) => c,
        None => return false,
    };
    if chan != chan_want {
        return false;
    }
    let dev_idx = usize::from(chan[14]);
    if dev_idx >= 0x0B {
        return false;
    }
    let Ok(dev_at) = p_pointer(layout, data, 0x28) else {
        return false;
    };
    data.get(dev_at + 0x18 + 0x19 * dev_idx..dev_at + 0x18 + 0x19 * (dev_idx + 1))
        .is_some_and(|d| d == dev_want)
}

/// 输出 OCP 面（行 13/14；vix output_ocp_policies 的结构解码——不做
/// sha256 关联，`association` 恒 structural-only）。
pub fn output_ocp_policies(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<PowerRowFull>, Error> {
    let rows = power_rows_full(data, layout)?;
    Ok([13usize, 14]
        .into_iter()
        .filter_map(|i| rows.get(i).cloned())
        .collect())
}

/// 输入电流策略（行 9，type 11 mA；行 7 联动限）。
pub fn input_current_policy(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<PowerRowFull>, Error> {
    let rows = power_rows_full(data, layout)?;
    Ok(rows.get(9).filter(|r| r.kind == 11 && r.unit_ma).cloned())
}

/// slot 功率策略（0x2C33→行 19、0x2D04→行 18、其余 19）。
pub fn slot_power_policy(data: &[u8], layout: &Gb20xLayout) -> Result<Option<PowerRowFull>, Error> {
    let rows = power_rows_full(data, layout)?;
    let idx = match layout.legacy.device {
        0x2C33 => 19,
        0x2D04 => 18,
        _ => 19,
    };
    Ok(rows.get(idx).cloned())
}

/// 连接器输入行（type 2 mW + 监视器通道 == CONNECTOR_INPUT_CHANNEL）。
pub fn connector_power_policies(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Vec<PowerRowFull>, Error> {
    let rows = power_rows_full(data, layout)?;
    let mut out = Vec::new();
    for r in rows {
        if PROFILE_POLICIES.contains(&r.row) || r.kind != 2 || r.unit_ma {
            continue;
        }
        if output_channel_chain_raw(data, layout, r.monitor, &CONNECTOR_INPUT_CHANNEL) {
            out.push(r);
        }
    }
    Ok(out)
}

/// 只比通道字节（连接器行用）。
fn output_channel_chain_raw(
    data: &[u8],
    layout: &Gb20xLayout,
    monitor: u8,
    chan_want: &[u8],
) -> bool {
    let Ok(at) = p_pointer(layout, data, 0x3C) else {
        return false;
    };
    if monitor >= 0x20 {
        return false;
    }
    data.get(at + 0x11 + 0x1A * usize::from(monitor)..at + 0x11 + 0x1A * (usize::from(monitor) + 1))
        .is_some_and(|c| c == chan_want)
}

// ── 热策略 / 出厂时钟 / PERF_CF / LPWR ──────────────────────────────────

/// 热策略表（P+0x50 obj 0x1B）条目：class@0、byte1、三温度 i16 @+2（1/32℃）。
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalPolicyEntry {
    pub index: usize,
    pub offset: usize,
    pub policy_class: u8,
    pub byte1: u8,
    /// 三温度（1/32 ℃，i16 LE）。
    pub temperatures_raw: [i16; 3],
}

/// 热策略表（无引用 → None）。
pub fn thermal_policies(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<Vec<ThermalPolicyEntry>>, Error> {
    let Some(at) = opt_pointer(layout, data, b'P', 0x50) else {
        return Ok(None);
    };
    let (ver, hs, es, count) = (
        data[at],
        usize::from(data[at + 1]),
        usize::from(data[at + 2]),
        usize::from(data[at + 3]),
    );
    if ver != THERMAL_POLICY.0
        || !THERMAL_POLICY.1.contains(&hs)
        || es != THERMAL_POLICY.2
        || count == 0
    {
        return Err(Error::from("vbios: unsupported thermal policy table"));
    }
    owner(layout, at, hs + es * count, 0x1B)?;
    let mut out = Vec::new();
    for i in 0..count {
        let e = at + hs + es * i;
        out.push(ThermalPolicyEntry {
            index: i,
            offset: e,
            policy_class: data[e],
            byte1: data[e + 1],
            temperatures_raw: [
                i16::from_le_bytes([data[e + 2], data[e + 3]]),
                i16::from_le_bytes([data[e + 4], data[e + 5]]),
                i16::from_le_bytes([data[e + 6], data[e + 7]]),
            ],
        });
    }
    Ok(Some(out))
}

/// 出厂时钟记录条目（u8 旗标 + i32/u32 kHz）。
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryClockEntry {
    pub index: usize,
    pub offset: usize,
    pub flag: u8,
    pub khz: i64,
}

/// 出厂时钟记录（P+0x98 obj 0x26；A=偏移 i32、B=时钟 u32）。
pub fn factory_clocks(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<(Vec<FactoryClockEntry>, Vec<FactoryClockEntry>)>, Error> {
    let Some(at) = opt_pointer(layout, data, b'P', 0x98) else {
        return Ok(None);
    };
    let header = &data[at..at + 7];
    if header != FACTORY_CLOCK_HEADER {
        return Ok(None); // 未审头：不解读条目
    }
    owner(layout, at, 27, 0x26)?;
    let mut offsets = Vec::new();
    let mut clocks = Vec::new();
    for i in 0..2 {
        offsets.push(FactoryClockEntry {
            index: i,
            offset: at + 7 + 5 * i,
            flag: data[at + 7 + 5 * i],
            khz: i64::from(i32_at(data, at + 7 + 5 * i + 1)?),
        });
        clocks.push(FactoryClockEntry {
            index: i,
            offset: at + 17 + 5 * i,
            flag: data[at + 17 + 5 * i],
            khz: i64::from(u32_at(data, at + 17 + 5 * i + 1)?),
        });
    }
    Ok(Some((offsets, clocks)))
}

// ── 温度墙 / 出厂时钟 patch（GB20x 写面；与 3x 同形，几何随 GB20x）──

/// 一条热策略温度墙写点。
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalTempPatch {
    pub entry_index: usize,
    /// (镜像内偏移, 旧值, 新值) ×3。
    pub writes: Vec<(usize, i16, i16)>,
}

/// 修复首个 legacy 镜像加性校验（patch 车道共用收尾）。
fn repair_legacy_image_checksum(img: &mut [u8]) -> Result<(), Error> {
    let base = crate::vbios::legacy::find_image_base(img)
        .ok_or_else(|| Error::from("vbios: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(())
}

/// GB20x 热策略温度墙写：条目 `entry_index` 的三温度（1/32 ℃ i16 LE，
/// entry+2/+4/+6；条目尺寸 48B）全部改写。定位复用 [`thermal_policies`]。
pub fn patch_thermal_policy_temps(
    img: &mut [u8],
    layout: &Gb20xLayout,
    entry_index: usize,
    temps_raw: [i16; 3],
) -> Result<ThermalTempPatch, Error> {
    let entries = thermal_policies(img, layout)?
        .ok_or_else(|| Error::from("vbios: thermal policy table not present"))?;
    let e = entries
        .iter()
        .find(|e| e.index == entry_index)
        .ok_or_else(|| {
            Error::from(format!(
                "vbios: thermal policy entry {entry_index} not found (populated: {})",
                entries
                    .iter()
                    .map(|x| x.index.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        })?;
    let mut writes = Vec::with_capacity(3);
    for (i, raw) in temps_raw.iter().enumerate() {
        let off = e.offset + 2 + 2 * i;
        let old = i16::from_le_bytes([img[off], img[off + 1]]);
        writes.push((off, old, *raw));
        img[off..off + 2].copy_from_slice(&raw.to_le_bytes());
    }
    repair_legacy_image_checksum(img)?;
    Ok(ThermalTempPatch {
        entry_index,
        writes,
    })
}

/// 一条出厂时钟写点。
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryClockPatch {
    /// false = A 表（i32 出厂偏移），true = B 表（u32 频率）。
    pub is_clock: bool,
    pub entry_index: usize,
    /// kHz 值域的镜像内偏移（旗标字节后）。
    pub offset: usize,
    pub old_khz: i64,
    pub new_khz: i64,
}

/// GB20x 出厂时钟写：`clocks`（is_clock=true，B 表 u32）/`offsets`
/// （false，A 表 i32）表条目 `entry_index`（0..2）的 kHz 改写（旗标字节
/// 保留）。**P0 频率源头**；定位复用 [`factory_clocks`]（头不合 = None =
/// 拒绝盲写）。
pub fn patch_factory_clock(
    img: &mut [u8],
    layout: &Gb20xLayout,
    is_clock: bool,
    entry_index: usize,
    khz: i64,
) -> Result<FactoryClockPatch, Error> {
    let (offsets, clocks) = factory_clocks(img, layout)?.ok_or_else(|| {
        Error::from("vbios: factory clock record not present or header unaudited")
    })?;
    let entries = if is_clock { &clocks } else { &offsets };
    let e = entries
        .iter()
        .find(|e| e.index == entry_index)
        .ok_or_else(|| {
            Error::from(format!(
                "vbios: factory clock entry {entry_index} not found (0..{})",
                entries.len()
            ))
        })?;
    let off = e.offset + 1;
    img[off..off + 4].copy_from_slice(&(khz as u32).to_le_bytes());
    repair_legacy_image_checksum(img)?;
    Ok(FactoryClockPatch {
        is_clock,
        entry_index,
        offset: off,
        old_khz: e.khz,
        new_khz: khz,
    })
}

/// PERF_CF 控制器行。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfController {
    pub index: usize,
    pub offset: usize,
    pub kind: u8,
    pub target_raw: u32,
    pub jump_raw: u32,
    pub domain: u8,
    pub hysteresis_up: u8,
    pub hysteresis_down: u8,
}

/// PERF_CF 策略行。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfPolicy {
    pub index: usize,
    pub offset: usize,
    pub flags: u8,
    pub kind: u8,
    pub active: bool,
    pub conflicts: u32,
    pub controllers: u32,
}

/// PERF_CF 控制器（P+0xa8 obj 0x60）+ 策略（P+0xac obj 0x61）。
pub fn perf_cf(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<(Vec<PerfCfController>, Vec<PerfCfPolicy>)>, Error> {
    let (Some(ca), Some(pa)) = (
        opt_pointer(layout, data, b'P', 0xA8),
        opt_pointer(layout, data, b'P', 0xAC),
    ) else {
        return Ok(None);
    };
    let (ver, hs, es, count) = (
        data[ca],
        usize::from(data[ca + 1]),
        usize::from(data[ca + 2]),
        usize::from(data[ca + 3]),
    );
    let (ev, ehs, ees, owner_id) = PERF_CF_CONTROLLERS;
    if ver != ev || !ehs.contains(&hs) || es != ees || count < 3 {
        return Err(Error::from("vbios: unsupported PERF_CF controller table"));
    }
    owner(layout, ca, hs + es * count, owner_id)?;
    let mut controllers = Vec::new();
    for i in 0..count {
        let e = ca + hs + es * i;
        controllers.push(PerfCfController {
            index: i,
            offset: e,
            kind: data[e] & 0x0F,
            target_raw: u32_at(data, e + 3)?,
            jump_raw: u32_at(data, e + 7)?,
            domain: data[e + 19],
            hysteresis_up: data[e + 20],
            hysteresis_down: data[e + 21],
        });
    }
    let (pv, ph, pe, po) = PERF_CF_POLICIES;
    let (ver, phs, pes, pcount) = (
        data[pa],
        usize::from(data[pa + 1]),
        usize::from(data[pa + 2]),
        usize::from(data[pa + 3]),
    );
    if (ver, phs, pes) != (pv, ph, pe) || pcount == 0 || data[pa + 4] != 0 {
        return Err(Error::from("vbios: unsupported PERF_CF policy table"));
    }
    owner(layout, pa, ph + pes * pcount, po)?;
    let mut policies = Vec::new();
    for i in 0..pcount {
        let e = pa + ph + pes * i;
        policies.push(PerfCfPolicy {
            index: i,
            offset: e,
            flags: data[e],
            kind: data[e] & 0x0F,
            active: data[e] & 0x80 != 0,
            conflicts: u32_at(data, e + 2)?,
            controllers: u32_at(data, e + 6)?,
        });
    }
    Ok(Some((controllers, policies)))
}

/// 一张 LPWR 特性表。
#[derive(Debug, Clone, PartialEq)]
pub struct LpwrTable {
    pub name: &'static str,
    pub offset: usize,
    pub object_id: u32,
    pub entries: Vec<u64>,
}

/// LPWR 特性表族（7 张；未引用 → 不在返回里）。
pub fn lpwr_tables(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<LpwrTable>, Error> {
    let mut out = Vec::new();
    for (name, bit_off, obj, hs, es, word) in LPWR_TABLES {
        let Some(at) = opt_pointer(layout, data, b'P', bit_off) else {
            continue;
        };
        let count = usize::from(data[at + 3]);
        if data[at] != 0x10
            || usize::from(data[at + 1]) != hs
            || usize::from(data[at + 2]) != es
            || count == 0
        {
            return Err(Error::from(format!("vbios: unsupported LPWR {name} table")));
        }
        owner(layout, at, hs + es * count, obj)?;
        let mut entries = Vec::new();
        for i in 0..count {
            let e = at + hs + es * i;
            let v = match word {
                1 => u64::from(data[e]),
                2 => u64::from(u16_at(data, e)?),
                _ => u64::from(u32_at(data, e)?),
            };
            entries.push(v);
        }
        out.push(LpwrTable {
            name,
            offset: at,
            object_id: obj,
            entries,
        });
    }
    Ok(out)
}

// ── 时钟编程 / DRAM 点 / GPC 曲线偏移 ───────────────────────────────────

/// 时钟编程表（C+0x0c v0x50 obj 4）DRAM 行条目。
#[derive(Debug, Clone, PartialEq)]
pub struct DramPoint {
    pub index: usize,
    pub offset: usize,
    pub flags: u8,
    pub mhz: u16,
}

/// DRAM 点（时钟表 DRAM 行点名；0x03|0x13 旗标、时钟严格升序）。
pub fn dram_points(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<DramPoint>, Error> {
    let ranges = crate::vbios::gb20x::clock_ranges(data, layout)?;
    let dram = ranges
        .iter()
        .find(|r| r.domain == "DRAM")
        .ok_or("vbios: no DRAM clock row")?;
    let row = dram.minimum_offset - 3;
    let (first, last) = (usize::from(data[row + 1]), usize::from(data[row + 2]));
    let at = layout.pointer(data, b'C', 0x0C)?;
    owner(layout, at, 1, 4)?;
    let (hs, size, count, c1, s1, c2, s2) = (
        usize::from(data[at + 1]),
        usize::from(data[at + 2]),
        usize::from(data[at + 3]),
        usize::from(data[at + 4]),
        usize::from(data[at + 5]),
        usize::from(data[at + 6]),
        usize::from(data[at + 7]),
    );
    if data[at] != 0x50 || hs != 8 {
        return Err(Error::from("vbios: unsupported clock programming table"));
    }
    let stride = size + c1 * s1 + c2 * s2;
    if !(first <= last && last < count && last - first >= 2) {
        return Err(Error::from("vbios: unsupported DRAM programming entries"));
    }
    let mut out = Vec::new();
    for i in first..=last {
        let e = at + hs + stride * i;
        out.push(DramPoint {
            index: i,
            offset: e,
            flags: data[e],
            mhz: u16_at(data, e + 1)?,
        });
    }
    Ok(out)
}

/// GPC 曲线偏移槽（vix gpc_curve_slot）。
#[derive(Debug, Clone, PartialEq)]
pub struct GpcCurveSlot {
    pub root: u16,
    pub root_offset: usize,
    pub variable: u8,
    pub link_offsets: Vec<usize>,
    pub slot: Option<u16>,
    pub slot_offset: Option<usize>,
    /// 当前偏移（MHz；无链接 = 0）。
    pub current_mhz: i32,
}

/// VFE 引用计数（next + min/max 子 + compare 子/判据；未知 kind → None）。
fn vfe_referrers(
    data: &[u8],
    base: usize,
    size: usize,
    width: usize,
    count: u16,
) -> Option<Vec<Vec<(u16, &'static str)>>> {
    let mut result: Vec<Vec<(u16, &str)>> = vec![Vec::new(); usize::from(count)];
    for i in 0..count {
        let e = data.get(base + size * usize::from(i)..base + size * usize::from(i) + size)?;
        if !matches!(e[0], 0 | 1 | 2 | 3 | 4 | 7) {
            return None;
        }
        if e[0] == 0 {
            continue;
        }
        let high = |k: usize| -> u16 {
            if width == 2 {
                u16::from(e[k + 4]) << 8
            } else {
                0
            }
        };
        let mut links = vec![(u16::from(e[2]) | high(2), "next")];
        match e[0] {
            2 => links.extend([
                (u16::from(e[11]) | high(11), "child"),
                (u16::from(e[12]) | high(12), "child"),
            ]),
            3 => {
                links.extend([
                    (u16::from(e[15]) | high(15), "child"),
                    (u16::from(e[16]) | high(16), "child"),
                ]);
                if e[23] & VFE_CRITERION_EQUATION != 0 {
                    links.push((u16::from(e[11]) | u16::from(e[12]) << 8, "criterion"));
                }
            }
            _ => {}
        }
        let none = ((1u32 << (8 * width)) - 1) as u16;
        for (x, kind) in links {
            if x != none
                && let Some(slot) = result.get_mut(usize::from(x))
            {
                slot.push((i, kind));
            }
        }
    }
    Some(result)
}

/// GPC 曲线偏移槽（compare MHz 根 + 常量 MHz 偏移槽）。
pub fn gpc_curve(data: &[u8], layout: &Gb20xLayout) -> Result<GpcCurveSlot, Error> {
    let ranges = crate::vbios::gb20x::clock_ranges(data, layout)?;
    let gpc = ranges
        .iter()
        .find(|r| r.domain == "GPC")
        .ok_or("vbios: no GPC clock row")?;
    let row = gpc.minimum_offset - 3;
    let (first, last) = (usize::from(data[row + 1]), usize::from(data[row + 2]));
    let at = layout.pointer(data, b'C', 0x0C)?;
    owner(layout, at, 1, 4)?;
    let (hs, size, count, c1, s1, c2, s2) = (
        usize::from(data[at + 1]),
        usize::from(data[at + 2]),
        usize::from(data[at + 3]),
        usize::from(data[at + 4]),
        usize::from(data[at + 5]),
        usize::from(data[at + 6]),
        usize::from(data[at + 7]),
    );
    let stride = size + c1 * s1 + c2 * s2;
    if data[at] != 0x50 || hs != 8 || size < 5 || first != last || first >= count {
        return Err(Error::from("vbios: unreviewed GPC clock programming entry"));
    }
    let root = u16_at(data, at + hs + stride * first + 3)?;
    let vfe_at = layout.pointer(data, b'P', 0x20)?;
    if data[vfe_at..vfe_at + 5] != [0x30, 14, 29, 0xFF, 25] {
        return Err(Error::from("vbios: unvalidated Blackwell VFE layout"));
    }
    owner(layout, vfe_at, 1, 0x14)?;
    let base = vfe_at
        + usize::from(data[vfe_at + 1])
        + usize::from(data[vfe_at + 2]) * usize::from(data[vfe_at + 3]);
    let vcount = u16::from(data[vfe_at + 5]) | u16::from(data[vfe_at + 10]) << 8;
    let refs =
        vfe_referrers(data, base, 25, 2, vcount).ok_or("vbios: unreviewed VFE equation kind")?;
    if usize::from(root) >= usize::from(vcount) {
        return Err(Error::from("vbios: GPC root outside the VFE table"));
    }
    let rat = base + 25 * usize::from(root);
    let e = &data[rat..rat + 25];
    if e[0] != 3 || e[23] & 0x0F != 1 || !refs[usize::from(root)].is_empty() {
        return Err(Error::from("vbios: unreviewed GPC root equation"));
    }
    let none = 0xFFFFu16;
    let mut links = vec![rat + 2, rat + 24];
    let nxt = u16::from(e[2]) | u16::from(e[24]) << 8;
    let (slot, current) = if nxt == none {
        let slot = (0..vcount).find(|&i| data[base + 25 * usize::from(i)] == 0);
        let Some(slot) = slot else {
            return Err(Error::from("vbios: no free VFE slot for GPC offset"));
        };
        let clean = vfe_free_slot_25();
        if data[base + 25 * usize::from(slot)..base + 25 * usize::from(slot) + 25] != clean {
            return Err(Error::from("vbios: GPC offset slot not clean"));
        }
        (Some(slot), 0)
    } else {
        let s = base + 25 * usize::from(nxt);
        let value = f32_at(data, s + 11)?;
        let mut want = [0u8; 25];
        want[0] = 1;
        want[1] = e[1];
        want[2] = 0xFF;
        for (k, src) in [3usize, 7, 11]
            .into_iter()
            .zip([nxt_f(value), nxt_f(value), nxt_f(value)])
        {
            want[k..k + 4].copy_from_slice(&src.to_le_bytes());
        }
        want[15..23].copy_from_slice(&0f32.to_le_bytes().repeat(2)[..8]);
        want[23] = 1;
        if data[s..s + 25] != want || refs[usize::from(nxt)].len() != 1 || value != value.trunc() {
            return Err(Error::from(
                "vbios: GPC root links non-offset-shape equation",
            ));
        }
        links[1] = s + 24;
        (Some(nxt), value as i32)
    };
    Ok(GpcCurveSlot {
        root,
        root_offset: rat,
        variable: e[1],
        link_offsets: links,
        slot,
        slot_offset: slot.map(|s| base + 25 * usize::from(s)),
        current_mhz: current,
    })
}

fn nxt_f(v: f32) -> f32 {
    v
}

// ── 风扇五表 ─────────────────────────────────────────────────────────────

/// 一条风扇通道（五表条目拼装）。
#[derive(Debug, Clone, PartialEq)]
pub struct FanChannel {
    pub index: usize,
    pub offset: usize,
    pub enable: u8,
    pub active: bool,
    pub pwm_frequency_hz: u32,
    pub pwm_frequency_offset: usize,
    pub level_minimum: u8,
    pub level_maximum: u8,
    pub rpm_minimum: u16,
    pub rpm_maximum: u16,
    pub curve_levels: [u8; 3],
    /// 3×(温度 1/32℃, RPM)。
    pub curve_points: [(u16, u16); 3],
    pub policy_offset: usize,
    pub policy_enable: u8,
    pub mapping_offset: usize,
    pub mapping_enable: u8,
    pub mapping_row: u8,
    pub control_offset: usize,
    pub control_enable: u8,
    pub speed_offset: usize,
    pub speed_enable: u8,
    pub speed_rpm: u16,
    pub flag_offset: usize,
    pub flag_byte: u8,
    pub stored: bool,
}

/// 风扇五表全景（cooler/policy/mapping/control/speed；无 GPU 风扇 → None）。
pub fn fan_coolers(data: &[u8], layout: &Gb20xLayout) -> Result<Option<Vec<FanChannel>>, Error> {
    let mut starts = Vec::new();
    for (name, bit_off, header, hs, es, count, obj) in FAN_TABLES {
        let start = if name == "cooler" {
            p_pointer(layout, data, bit_off)?
        } else {
            match opt_pointer(layout, data, b'P', bit_off) {
                Some(a) => a,
                None => return Err(Error::from(format!("vbios: fan {name} table absent"))),
            }
        };
        if data[start..start + 4] != header {
            return Err(Error::from(format!("vbios: unsupported fan {name} table")));
        }
        owner(layout, start, hs + es * count, obj)?;
        starts.push((name, start, hs, es));
    }
    let cooler_at = starts[0].1;
    let header_bytes = [
        data[cooler_at + 4],
        data[cooler_at + 5],
        data[cooler_at + 6],
        data[cooler_at + 7],
    ];
    let all_ff = header_bytes == [0xFF; 4];
    let any_enabled = (0..4).any(|i| {
        starts
            .iter()
            .any(|(n, s, hs, es)| *n != "cooler" && data[s + hs + es * i] != 0)
    });
    if all_ff && !any_enabled {
        return Ok(None); // 系统风扇（笔记本）
    }
    let end = header_bytes.iter().position(|&b| b == 0xFF).unwrap_or(4);
    let listed: Vec<usize> = header_bytes[..end]
        .iter()
        .map(|&b| usize::from(b))
        .collect();
    if header_bytes[end..].iter().any(|&b| b != 0xFF)
        || listed != (0..listed.len()).collect::<Vec<_>>()
        || listed.is_empty()
    {
        return Err(Error::from("vbios: unrecognized fan channel list"));
    }
    let tbl = |name: &str, i: usize| -> usize {
        let (n, s, hs, es) = starts.iter().find(|(n, _, _, _)| *n == name).unwrap();
        let _ = n;
        s + hs + es * i
    };
    let mut rows = Vec::new();
    for i in 0..4usize {
        let (row, pol, mapped) = (tbl("cooler", i), tbl("policy", i), tbl("mapping", i));
        let (control, speed) = (tbl("control", i), tbl("speed", i));
        rows.push(FanChannel {
            index: i,
            offset: row,
            enable: data[row],
            active: listed.contains(&i),
            pwm_frequency_hz: u32::from(u16_at(data, row + 11)?),
            pwm_frequency_offset: row + 11,
            level_minimum: data[row + 2],
            level_maximum: data[row + 3],
            rpm_minimum: u16_at(data, row + 14)?,
            rpm_maximum: u16_at(data, row + 16)?,
            curve_levels: [data[pol + 14], data[pol + 15], data[pol + 16]],
            curve_points: [
                (u16_at(data, pol + 18)?, u16_at(data, pol + 20)?),
                (u16_at(data, pol + 22)?, u16_at(data, pol + 24)?),
                (u16_at(data, pol + 26)?, u16_at(data, pol + 28)?),
            ],
            policy_offset: pol,
            policy_enable: data[pol],
            mapping_offset: mapped,
            mapping_enable: data[mapped],
            mapping_row: data[mapped + 1],
            control_offset: control,
            control_enable: data[control],
            speed_offset: speed,
            speed_enable: data[speed],
            speed_rpm: u16_at(data, speed + 2)?,
            flag_offset: pol + FAN_POLICY_FLAG_OFFSET,
            flag_byte: data[pol + FAN_POLICY_FLAG_OFFSET],
            stored: data[pol + 1] == 1
                && data[mapped + 1] == i as u8
                && data[control + 2] == i as u8
                && data[control + 5] == 0x30 | (1 << i)
                && data[speed + 1] == i as u8,
        });
    }
    Ok(Some(rows))
}

// ── VFE 引用命名面（equation_index_names）───────────────────────────────

/// VFE 方程索引 → 命名它的表（clock programming/FLL/voltage policy/P+0x104）。
pub fn equation_index_names(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Option<std::collections::BTreeMap<u16, &'static str>>, Error> {
    let mut names: std::collections::BTreeMap<u16, &str> = Default::default();
    let name = |at: usize,
                table: &'static str,
                names: &mut std::collections::BTreeMap<u16, &'static str>| {
        let v = u16_at(data, at).unwrap_or(0xFFFF);
        names.entry(v).or_insert(table);
    };
    if let Some(at) = opt_pointer(layout, data, b'C', 0x0C) {
        let hs = usize::from(data[at + 1]);
        let size = usize::from(data[at + 2]);
        let count = usize::from(data[at + 3]);
        let (c1, s1, c2, s2) = (
            usize::from(data[at + 4]),
            usize::from(data[at + 5]),
            usize::from(data[at + 6]),
            usize::from(data[at + 7]),
        );
        let stride = size + c1 * s1 + c2 * s2;
        if data[at] != 0x50 || hs != 8 || owner(layout, at, 1, 4).is_err() {
            return Ok(None); // 未审布局：其他用户不能排除
        }
        for i in 0..count {
            for k in 0..stride.saturating_sub(1) {
                name(at + hs + stride * i + k, "clock programming", &mut names);
            }
        }
    }
    if let Some(at) = opt_pointer(layout, data, b'C', 0x10) {
        if data[at..at + 3] != [0x20, 7, 13] || owner(layout, at, 1, 5).is_err() {
            return Ok(None);
        }
        let count = usize::from(data[at + 3]);
        name(at + 5, "FLL", &mut names);
        for i in 0..count {
            for k in 0..12 {
                name(at + 7 + 13 * i + k, "FLL", &mut names);
            }
        }
    }
    if let Some(at) = opt_pointer(layout, data, b'P', 0x70) {
        if data[at..at + 3] != [0x20, 4, 5]
            || !(layout.legacy.start <= at
                && at + 4 + 5 * usize::from(data[at + 3]) <= layout.legacy.end)
        {
            return Ok(None);
        }
        for i in 0..usize::from(data[at + 3]) {
            for k in 1..4 {
                name(at + 4 + 5 * i + k, "voltage policy", &mut names);
            }
        }
    }
    if let Some(at) = opt_pointer(layout, data, b'P', 0x104) {
        if data[at..at + 3] != [0x10, 5, 21] || owner(layout, at, 1, 0xA7).is_err() {
            return Ok(None);
        }
        for i in 0..usize::from(data[at + 3]) {
            let entry = at + 5 + 21 * i;
            if !P104_ENTRY_TYPES.contains(&data[entry]) {
                return Ok(None);
            }
            for k in [5usize, 7] {
                name(entry + k, "P+0x104", &mut names);
            }
        }
    }
    Ok(Some(names))
}

// ── 运行时电压 offset 钩子（mVolt+ 安装的形态）───────────────────────────

/// 钩子报告（vix runtime_hooks）。
#[derive(Debug, Clone, PartialEq)]
pub struct HookReport {
    pub state: &'static str,
    pub slots: Vec<HookSlot>,
    /// 未被钩子占用的 stock 模板变量（运行时工具读它作基准）。
    pub reference: Option<u8>,
    pub diagnostic: Option<String>,
    pub blocked: Option<String>,
    /// VFE 头与 BIT P 尺寸是否为运行时工具接受的形态。
    pub layout_ok: bool,
    /// 未用变量/方程是否成尾块。
    pub unused_tail: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookSlot {
    pub rail: &'static str,
    pub role: &'static str,
    pub selector_offset: usize,
    pub word: u16,
    pub hooked: bool,
    pub hook: Option<u16>,
    pub variable: Option<u8>,
    pub target: u16,
}

fn vfe_geometry(data: &[u8], layout: &Gb20xLayout) -> Result<(usize, usize, usize, usize), Error> {
    let vfe_at = layout.pointer(data, b'P', 0x20)?;
    if data[vfe_at..vfe_at + 5] != [0x30, 14, 29, 0xFF, 25] {
        return Err(Error::from("vbios: unvalidated Blackwell VFE layout"));
    }
    owner(layout, vfe_at, 1, 0x14)?;
    let variables = vfe_at + usize::from(data[vfe_at + 1]);
    let equations = variables + 29 * 255;
    let count = usize::from(u16::from(data[vfe_at + 5]) | u16::from(data[vfe_at + 10]) << 8);
    Ok((vfe_at, variables, equations, count))
}

/// 钩子状态机（vix _hook_report 全逻辑：off/on/partial + blocked 原因）。
pub fn runtime_hooks(data: &[u8], layout: &Gb20xLayout) -> Result<HookReport, Error> {
    let (vfe_at, variables, equations, count) = vfe_geometry(data, layout)?;
    let rail = p_pointer(layout, data, 0x68)?;
    let rows = crate::vbios::gb20x::rail_count(data, layout)?;
    let p_size = layout.bit.token(b'P').map(|t| t.size).unwrap_or(0);
    let layout_ok = data[vfe_at..vfe_at + 6] == HOOK_VFE_HEADER[0].0
        && data[vfe_at + 7..vfe_at + 14] == HOOK_VFE_HEADER[0].1
        && p_size >= HOOK_BIT_P_SIZE;

    let var = |i: usize| -> Vec<u8> { data[variables + 29 * i..variables + 29 * i + 29].to_vec() };
    let eq = |i: usize| -> Vec<u8> { data[equations + 25 * i..equations + 25 * i + 25].to_vec() };

    let mut slots = Vec::new();
    let mut words: Vec<u16> = Vec::new();
    for i in 0..rows {
        let row = rail + 5 + 65 * i;
        let label = if i == 0 { "NVVDD" } else { "MSVDD" };
        for (role, offset) in HOOK_ROLES {
            let word = u16_at(data, row + offset)?;
            words.push(word);
            if usize::from(word) < count {
                let e = eq(usize::from(word));
                let body = vfe_hook_body();
                let shaped = e[0] == 1 && e[3..11] == body[0..8];
                slots.push(HookSlot {
                    rail: label,
                    role,
                    selector_offset: row + offset,
                    word,
                    hooked: shaped,
                    hook: None,
                    variable: None,
                    target: word,
                });
            }
        }
    }
    let free_v: Vec<usize> = (0..255).filter(|&i| var(i)[0] == 0).collect();
    let free_e: Vec<usize> = (0..count).filter(|&i| eq(i)[0] == 0).collect();
    let tail = |free: &[usize], total: usize| -> bool {
        free.iter()
            .enumerate()
            .all(|(k, &v)| v == total - free.len() + k)
    };
    let unused_tail = tail(&free_v, 255) && tail(&free_e, count);
    let mut report = HookReport {
        state: "off",
        slots,
        reference: None,
        diagnostic: None,
        blocked: None,
        layout_ok,
        unused_tail,
    };

    let refs = vfe_referrers(data, equations, 25, 2, count as u16);

    if report.slots.iter().any(|s| s.hooked) {
        // 有钩形方程：验证完整集（on）或报 partial
        let mut problem: Option<String> = None;
        if !layout_ok {
            problem = Some(
                "the VFE header or BIT P token differs from the layout runtime tools accept".into(),
            );
        }
        if problem.is_none() {
            for s in report.slots.iter_mut() {
                let e = eq(usize::from(s.word));
                let target = u16::from(e[2]) | u16::from(e[24]) << 8;
                let want = vfe_hook_equation(e[1], target);
                let _ = vfe_hook_body();
                if e != want {
                    problem = Some(format!(
                        "the {} {} selector does not select a hook",
                        s.rail, s.role
                    ));
                    break;
                }
                if e[1] == 0xFF || var(usize::from(e[1])) != VFE_HOOK_VARIABLE {
                    problem = Some(format!(
                        "the variable of the {} {} hook is not the stock template copy",
                        s.rail, s.role
                    ));
                    break;
                }
                if usize::from(target) >= count
                    || target == s.word
                    || eq(usize::from(target))[0] == 0
                {
                    problem = Some(format!(
                        "the {} {} hook links no equation in use",
                        s.rail, s.role
                    ));
                    break;
                }
                s.hook = Some(s.word);
                s.variable = Some(e[1]);
                s.target = target;
            }
        }
        if problem.is_none() {
            let hooks: Vec<u16> = report.slots.iter().filter_map(|s| s.hook).collect();
            let used: Vec<u8> = report.slots.iter().filter_map(|s| s.variable).collect();
            if hooks
                .iter()
                .any(|h| hooks.iter().filter(|x| **x == *h).count() > 1)
                || used
                    .iter()
                    .any(|v| used.iter().filter(|x| **x == *v).count() > 1)
            {
                problem = Some("two selectors share a hook or a hook variable".into());
            } else if report
                .slots
                .iter()
                .any(|s| hooks.contains(&s.target) && s.hook.is_some() && s.target != s.word)
            {
                problem = Some("a hook links another hook".into());
            } else if hooks
                .iter()
                .any(|h| words.iter().filter(|w| **w == *h).count() != 1)
            {
                problem = Some("a hook is named by another rail row word too".into());
            } else if refs.is_none() {
                problem = Some("the VFE table holds an unreadable equation kind".into());
            } else if hooks.iter().any(|h| {
                refs.as_ref().unwrap()[usize::from(*h)]
                    .iter()
                    .any(|(r, _)| !hooks.contains(r))
            }) {
                problem = Some("another equation links a hook".into());
            } else if (0..count)
                .any(|i| !hooks.contains(&(i as u16)) && eq(i)[0] != 0 && used.contains(&eq(i)[1]))
            {
                problem = Some("another equation reads a hook variable".into());
            } else {
                let templates: Vec<usize> =
                    (0..255).filter(|&i| var(i) == VFE_HOOK_VARIABLE).collect();
                if !templates.iter().any(|t| !used.contains(&(*t as u8))) {
                    problem =
                        Some("no stock template variable is left as the runtime reference".into());
                } else {
                    report.state = "on";
                    report.reference = templates
                        .iter()
                        .find(|t| !used.contains(&(**t as u8)))
                        .map(|&t| t as u8);
                    return Ok(report);
                }
            }
        }
        for s in report.slots.iter_mut() {
            s.hook = None;
            s.variable = None;
            s.target = s.word;
        }
        report.state = "partial";
        report.diagnostic = problem.clone();
        report.blocked = problem;
        return Ok(report);
    }

    // off：能否安装（blocked 原因）
    let targets: Vec<usize> = report.slots.iter().map(|s| usize::from(s.word)).collect();
    let new_v = &free_v[..free_v.len().min(report.slots.len())];
    let new_e = &free_e[..free_e.len().min(report.slots.len())];
    let mut reason: Option<String> = None;
    if !layout_ok {
        reason = Some("the VFE header or BIT P token differs from the runtime layout".into());
    } else if report.slots.is_empty() {
        reason = Some("no REL/ALT/OV/VMIN selector is present".into());
    } else if targets.iter().any(|&t| eq(t)[0] == 0) {
        reason = Some("a REL/ALT/OV/VMIN selector names no equation in use".into());
    } else if !(0..255).any(|i| var(i) == VFE_HOOK_VARIABLE) {
        reason = Some("the image has no stock template variable".into());
    } else if !unused_tail {
        reason = Some("unused VFE variables or equations do not form one tail block".into());
    } else if free_v.len() < report.slots.len() || free_e.len() < report.slots.len() {
        reason = Some(format!(
            "the VFE table has {} unused variables and {} unused equations; hooks need {} of each",
            free_v.len(),
            free_e.len(),
            report.slots.len()
        ));
    } else if refs.is_none() {
        reason = Some("the VFE table holds an unreadable equation kind".into());
    } else if let Some(r) = refs {
        if new_e
            .iter()
            .any(|&i| !r[i].is_empty() || words.contains(&(i as u16)))
            || (0..count).any(|i| eq(i)[0] != 0 && new_v.contains(&usize::from(eq(i)[1])))
        {
            reason = Some("an unused VFE entry the hooks would take is already named".into());
        } else {
            let names = equation_index_names(data, layout)?;
            match names {
                None => {
                    reason = Some(
                        "equation-naming tables unreadable; other users cannot be ruled out".into(),
                    )
                }
                Some(names) if new_e.iter().any(|i| names.contains_key(&(*i as u16))) => {
                    let table = new_e
                        .iter()
                        .find_map(|i| names.get(&(*i as u16)))
                        .copied()
                        .unwrap_or("?");
                    reason = Some(format!(
                        "an unused VFE equation is named by the {table} table"
                    ));
                }
                _ => {}
            }
        }
    }
    report.blocked = reason;
    Ok(report)
}

/// follow-OV 位置（REL=ALT=OV 同常数的专用方程或拷贝槽语义）。
#[derive(Debug, Clone, PartialEq)]
pub struct FollowOv {
    pub index: u16,
    pub offset: usize,
    pub value_uv: f32,
    /// 专用（无其他用户）= 直接改值；否则需拷贝槽迁移。
    pub dedicated: bool,
    pub slot: Option<u16>,
}

/// follow-OV 方程定位（vix follow_ov_equation 的解码面）。
pub fn follow_ov(data: &[u8], layout: &Gb20xLayout) -> Result<Option<FollowOv>, Error> {
    let rail = p_pointer(layout, data, 0x68)?;
    let (_, _, equations, count) = vfe_geometry(data, layout)?;
    let index = u16_at(data, rail + 5 + 0x11)?;
    if usize::from(index) >= count {
        return Ok(None);
    }
    let at = equations + 25 * usize::from(index);
    let eqv = crate::vbios::gb20x::vfe(data, layout)?;
    let value = eqv
        .equation(index)
        .and_then(crate::vbios::vfe::constant_equation_uv);
    let Some(value) = value else {
        return Ok(None);
    };
    let refs =
        vfe_referrers(data, equations, 25, 2, count as u16).ok_or("vbios: unreviewed VFE kind")?;
    let names = equation_index_names(data, layout)?;
    let Some(names) = names else {
        return Ok(None);
    };
    let rail_rows = crate::vbios::gb20x::rail_count(data, layout)?;
    let own: Vec<usize> = (0..rail_rows)
        .flat_map(|i| [0x0D, 0x0F, 0x11].map(|o| rail + 5 + 65 * i + o))
        .collect();
    let mut used = refs[usize::from(index)]
        .iter()
        .map(|(r, _)| usize::from(*r))
        .collect::<Vec<_>>();
    for row in (0..rail_rows).map(|i| rail + 5 + 65 * i) {
        for &offset in RAIL_SELECTORS.iter() {
            let at2 = row + offset;
            if own.contains(&at2) {
                continue;
            }
            if let Ok(w) = u16_at(data, at2) {
                used.push(usize::from(w));
            }
        }
    }
    used.extend(names.keys().map(|k| usize::from(*k)));
    let dedicated = !used.contains(&usize::from(index));
    Ok(Some(FollowOv {
        index,
        offset: at,
        value_uv: value,
        dedicated,
        slot: None,
    }))
}

// RomImage 引用保活（legacy.device 访问）。
const _: fn(&RomImage) -> u16 = |r| r.device;

/// 风扇强制曲线写点。
#[derive(Debug, Clone, PartialEq)]
pub struct FanForcePatch {
    pub channel: usize,
    /// (镜像内偏移, 旧值, 新值)——levels(1B)/rpms(2B) 混排。
    pub writes: Vec<(usize, u16, u16)>,
}

/// 风扇强制最大散热写：策略条目三档 level（duty%，位于曲线基址 +14/15/16）
/// 全部改写为 `level`（None = 该通道 cooler 行的 level_maximum = 卡自身
/// 100%），三点 RPM 改写为 `rpm`（None = cooler 行 rpm_maximum），并把
/// cooler 行 rpm_maximum 只升不降地抬到 max(RPM)。**零转速随曲线首点
/// RPM>0 自动消失**——大功率 vBIOS 的散热压制面。
pub fn patch_fan_force(
    img: &mut [u8],
    layout: &Gb20xLayout,
    channel: usize,
    level: Option<u8>,
    rpm: Option<u16>,
) -> Result<FanForcePatch, Error> {
    let rows = fan_coolers(img, layout)?
        .ok_or_else(|| Error::from("vbios: fan tables not present (no GPU-driven fan)"))?;
    let r = rows.iter().find(|r| r.index == channel).ok_or_else(|| {
        Error::from(format!(
            "vbios: fan channel {channel} not found (channels: {})",
            rows.iter()
                .map(|x| x.index.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
    })?;
    let level = level.unwrap_or(r.level_maximum);
    let rpm = rpm.unwrap_or(r.rpm_maximum);
    let mut writes = Vec::new();
    // levels：曲线基址 +14/15/16（1B duty%）。
    for i in 0..3 {
        let off = r.policy_offset + 14 + i;
        let old = u16::from(img[off]);
        writes.push((off, old, u16::from(level)));
        img[off] = level;
    }
    // 点 RPM：曲线基址 +20/24/28（u16，temp 原值保留）。
    for i in 0..3 {
        let off = r.policy_offset + 20 + 4 * i;
        let old = u16_at(img, off)?;
        writes.push((off, old, rpm));
        img[off..off + 2].copy_from_slice(&rpm.to_le_bytes());
    }
    // cooler 行 rpm_maximum 只升不降。
    if r.rpm_maximum < rpm {
        let off = r.offset + 16;
        writes.push((off, r.rpm_maximum, rpm));
        img[off..off + 2].copy_from_slice(&rpm.to_le_bytes());
    }
    // 修复首个 legacy 镜像加性校验。
    let base = crate::vbios::legacy::find_image_base(img)
        .ok_or_else(|| Error::from("vbios: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(FanForcePatch { channel, writes })
}

/// PERF_CF 控制器 target 写点（raw 值域随 kind：1=温度阶梯阈值、
/// 3=µV 电压上限——TU106 kind3 target=1200000 = 1.2V VMAX）。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfTargetPatch {
    pub controller_index: usize,
    pub offset: usize,
    pub old_raw: u32,
    pub new_raw: u32,
}

/// PERF_CF 控制器 target_raw 写（u32 LE @entry+3）。定位复用
/// [`perf_cf_controllers`]（头不合 → 报错拒绝盲写）。
pub fn patch_perf_cf_target(
    img: &mut [u8],
    layout: &Gb20xLayout,
    controller_index: usize,
    target_raw: u32,
) -> Result<PerfCfTargetPatch, Error> {
    let (controllers, _) = perf_cf(img, layout)?.ok_or_else(|| {
        Error::from("vbios: PERF_CF controller table not present or header unaudited")
    })?;
    let c = controllers
        .iter()
        .find(|c| c.index == controller_index)
        .ok_or_else(|| {
            Error::from(format!(
                "vbios: PERF_CF controller {controller_index} not found (0..{})",
                controllers.len()
            ))
        })?;
    let off = c.offset + 3;
    img[off..off + 4].copy_from_slice(&target_raw.to_le_bytes());
    let base = crate::vbios::legacy::find_image_base(img)
        .ok_or_else(|| Error::from("vbios: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(PerfCfTargetPatch {
        controller_index,
        offset: off,
        old_raw: c.target_raw,
        new_raw: target_raw,
    })
}

/// PERF_CF 策略激活位写点（flags bit 0x80）。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfPolicyPatch {
    pub policy_index: usize,
    pub offset: usize,
    pub old_flags: u8,
    pub new_flags: u8,
}

/// PERF_CF 策略激活位写（flags 其余位保留）。
pub fn patch_perf_cf_policy(
    img: &mut [u8],
    layout: &Gb20xLayout,
    policy_index: usize,
    active: bool,
) -> Result<PerfCfPolicyPatch, Error> {
    let (_, policies) = perf_cf(img, layout)?.ok_or_else(|| {
        Error::from("vbios: PERF_CF policy table not present or header unaudited")
    })?;
    let pol = policies
        .iter()
        .find(|p| p.index == policy_index)
        .ok_or_else(|| {
            Error::from(format!(
                "vbios: PERF_CF policy {policy_index} not found (0..{})",
                policies.len()
            ))
        })?;
    let off = pol.offset;
    let new_flags = if active {
        pol.flags | 0x80
    } else {
        pol.flags & !0x80
    };
    img[off] = new_flags;
    let base = crate::vbios::legacy::find_image_base(img)
        .ok_or_else(|| Error::from("vbios: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(PerfCfPolicyPatch {
        policy_index,
        offset: off,
        old_flags: pol.flags,
        new_flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_and_factory_patch_roundtrip() {
        // 真镜像（GB206 Blackwell）端到端：定位→改写→重解析→邻字节/校验和。
        let Ok(d0) = std::fs::read("../GB206.rom") else {
            eprintln!("skip: GB206.rom not present");
            return;
        };
        let Ok(layout) = crate::vbios::gb20x::parse_layout(&d0) else {
            eprintln!("skip: GB206.rom does not parse as GB20x");
            return;
        };
        let Some(entries) = thermal_policies(&d0, &layout).expect("ok") else {
            eprintln!("skip: no thermal policy table on this image");
            return;
        };
        let e0 = entries
            .iter()
            .find(|e| e.temperatures_raw != [0, 0, 0])
            .expect("a populated thermal entry");

        // 温度墙：该条目三点 → 95 ℃（3040 raw）。
        let mut img = d0.clone();
        let patch =
            patch_thermal_policy_temps(&mut img, &layout, e0.index, [95 * 32; 3]).expect("patch");
        assert_eq!(patch.writes.len(), 3);
        let after = thermal_policies(&img, &layout)
            .expect("ok")
            .expect("present");
        let a0 = after
            .iter()
            .find(|e| e.index == e0.index)
            .expect("entry after");
        assert_eq!(a0.temperatures_raw, [95 * 32; 3]);
        for (b, a) in entries.iter().zip(after.iter()) {
            if b.index != e0.index {
                assert_eq!(b.temperatures_raw, a.temperatures_raw);
            }
        }

        // 出厂时钟：clocks 表填充项 → 2100 MHz；其余项/A 表不动。
        let Some((_offs_before, cls_before)) = factory_clocks(&img, &layout).expect("ok") else {
            eprintln!("skip: no factory clock record on this image");
            return;
        };
        let Some(ci) = cls_before.iter().position(|e| e.khz > 0) else {
            eprintln!("skip: no populated factory clock");
            return;
        };
        let old_khz = cls_before[ci].khz;
        let fpatch = patch_factory_clock(&mut img, &layout, true, cls_before[ci].index, 2_100_000)
            .expect("factory patch");
        assert_eq!(fpatch.old_khz, old_khz);
        let (_, cls_after) = factory_clocks(&img, &layout).expect("ok").expect("present");
        assert_eq!(cls_after[ci].khz, 2_100_000);

        // 镜像加性校验归零。
        let base = crate::vbios::legacy::find_image_base(&img).expect("base");
        let size =
            (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
        let end = (base + size).min(img.len());
        let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        assert_eq!(sum, 0, "image checksum must stay zero after patch");
    }
}
