//! BIT 'P' v2（PERF_PTRS）的功率/传感子表——envytools nvbios power.c 的
//! Rust 移植，指针经 [`legacy::relocate_ptr`] 同款 EFI 间隙重定位。
//!
//! TU106 1.rom 实测校准：PowerBudget v0x30 的 cap_entry=2 指向总板功率条
//! （min 105000 / avg 175000 / peak 219000 mW——与该卡 175W 标称、219W
//! 上限完全吻合），PowerSense v0x20（7 条模式+extdev 引用）、
//! PowerBaseClock v0x20（17 条 per-Pstate × 4 子条目）。VOLTAGE/VOLT MAP
//! 在 Volta+ 消失（rails 表族接替），解码按 envytools 版本门保留。
//!
//! 与 envytools 的已知分歧：BaseClock v0x20 子条目时钟编码 envytools 记
//! ×4，GP102 实测 boost/turbo 槽 ×2 才落 1480/1582（1080 Ti FE 标称）——
//! 只报告原始 u16，不做 MHz 换算断言。

use crate::error::Error;
use crate::vbios::legacy::{bit_entries, find_bit_signature, find_image_base, relocate_ptr};

/// 解析 BIT 'P' 指定槽位：非零 u32 → 重定位后的目标偏移。
fn slot_target(data: &[u8], slot: usize) -> Result<Option<(u32, usize)>, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("power_p2: 55 AA base not found"))?;
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("power_p2: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P')
        .ok_or_else(|| Error::from("power_p2: BIT 'P' entry not found"))?;
    let ptab = img + usize::from(p.offset);
    if ptab + slot + 4 > data.len() {
        return Ok(None);
    }
    let raw = u32::from(data[ptab + slot])
        | u32::from(data[ptab + slot + 1]) << 8
        | u32::from(data[ptab + slot + 2]) << 16
        | u32::from(data[ptab + slot + 3]) << 24;
    if raw == 0 {
        return Ok(None);
    }
    Ok(relocate_ptr(data, img, raw).map(|abs| (raw, abs)))
}

fn u16_at(data: &[u8], a: usize) -> Option<u16> {
    Some(u16::from(*data.get(a)?) | u16::from(*data.get(a + 1)?) << 8)
}

fn u32_at(data: &[u8], a: usize) -> Option<u32> {
    Some(
        u32::from(*data.get(a)?)
            | u32::from(*data.get(a + 1)?) << 8
            | u32::from(*data.get(a + 2)?) << 16
            | u32::from(*data.get(a + 3)?) << 24,
    )
}

// ── PowerBudget（P+0x2C，nvidia-smi 功率墙）──────────────────────────────

/// 一条功率预算：min/avg/peak mW（rlen==6 的精简条目只有 avg）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBudgetEntry {
    pub index: usize,
    pub offset: usize,
    /// 条目首字节 tag（TU106 总板条目 = 0x21）。
    pub tag: u8,
    pub min_mw: Option<u32>,
    pub avg_mw: Option<u32>,
    pub peak_mw: Option<u32>,
    /// rlen ≥ 0x16 时的第四个 u32（envytools unkn12）。
    pub unkn12: Option<u32>,
    pub raw_hex: String,
}

/// POWER BUDGET 表。`cap_entry` = nvidia-smi 采用的条目号（v0x20 → 头+9、
/// v0x30 → 头+0xA）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBudget {
    pub offset: usize,
    pub version: u8,
    pub cap_entry: Option<u8>,
    pub entries: Vec<PowerBudgetEntry>,
}

/// 解析 POWER BUDGET（P+0x2C；v0x10/0x20/0x30）。
pub fn find_power_budget(data: &[u8]) -> Result<Option<PowerBudget>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x2C)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if !matches!(version, 0x10 | 0x20 | 0x30) {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let cap_entry = match version {
        0x20 => data.get(t + 9).copied(),
        0x30 => data.get(t + 0xA).copied(),
        _ => None,
    };
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hlen + i * rlen;
        let Some(tag) = data.get(e).copied() else {
            break;
        };
        let (min_mw, avg_mw, peak_mw, unkn12) = if rlen == 6 {
            (None, u32_at(data, e + 2), None, None)
        } else {
            (
                u32_at(data, e + 2),
                u32_at(data, e + 6),
                u32_at(data, e + 0xA),
                if rlen >= 0x16 {
                    u32_at(data, e + 0x12)
                } else {
                    None
                },
            )
        };
        let end = (e + rlen).min(data.len());
        entries.push(PowerBudgetEntry {
            index: i,
            offset: e,
            tag,
            min_mw,
            avg_mw,
            peak_mw,
            unkn12,
            raw_hex: data[e..end].iter().map(|b| format!("{b:02x}")).collect(),
        });
    }
    Ok(Some(PowerBudget {
        offset: t,
        version,
        cap_entry,
        entries,
    }))
}

// ── PowerBudget 写面（BIOS override：vix loader 配套 patch）───────────────

/// 一条被改写的功率预算条目（回显用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBudgetPatch {
    pub index: usize,
    /// 条目在镜像内的绝对偏移（min 域；peak 在 +0xA）。
    pub offset: usize,
    pub tag: u8,
    pub old_peak_mw: Option<u32>,
    pub new_peak_mw: Option<u32>,
}

/// 一个 direct-patch 点位：P+0x2C cap 条目 peak 域（+0xA）的镜像内绝对
/// 偏移 + 旧值。`set-vbios-direct-patch` 用它构造 romOverride 片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBudgetCapPoint {
    pub index: usize,
    /// peak/max 域（entry+0xA）的镜像内绝对偏移。
    pub peak_offset: usize,
    pub tag: u8,
    pub old_peak_mw: Option<u32>,
}

/// 定位 POWER BUDGET（P+0x2C）的 cap 条目 peak 域点位（只读）。目标条目
/// 选择与 [`patch_power_budget_peak_mw`] 完全一致（cap_entry 槽位 →
/// tag==0x21 回退 → 都没有拒绝）——两车道（整镜像 patch 与 romOverride
/// 片段 patch）必须打同一个字节。
pub fn locate_power_budget_cap_peak(data: &[u8]) -> Result<Vec<PowerBudgetCapPoint>, Error> {
    let t = match slot_target(data, 0x2C)? {
        None => return Err(Error::from("power_p2: P+0x2C power budget table absent")),
        Some((_, t)) => t,
    };
    let version = data.get(t).copied().unwrap_or(0);
    if !matches!(version, 0x10 | 0x20 | 0x30) {
        return Err(Error::from("power_p2: unsupported power budget version"));
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    if rlen == 0 || hlen == 0 {
        return Err(Error::from("power_p2: degenerate power budget header"));
    }
    if rlen == 6 {
        return Err(Error::from(
            "power_p2: compact avg-only budget has no cap ceiling to patch",
        ));
    }
    let entry_off = |i: usize| t + hlen + i * rlen;
    let valid = |i: usize| i < cnt.min(64) && entry_off(i) + 0xE <= data.len();
    let cap_entry = match version {
        0x20 => data.get(t + 9).copied(),
        0x30 => data.get(t + 0xA).copied(),
        _ => None,
    };
    let targets: Vec<usize> = if cap_entry.is_some_and(|c| valid(usize::from(c))) {
        vec![usize::from(cap_entry.unwrap())]
    } else {
        (0..cnt.min(64))
            .filter(|&i| valid(i) && data.get(entry_off(i)).is_some_and(|&tag| tag == 0x21))
            .collect()
    };
    if targets.is_empty() {
        return Err(Error::from(
            "power_p2: no cap entry (cap_entry slot or tag 0x21) — refusing blind full-table targeting",
        ));
    }
    Ok(targets
        .into_iter()
        .map(|i| PowerBudgetCapPoint {
            index: i,
            peak_offset: entry_off(i) + 0xA,
            tag: data[entry_off(i)],
            old_peak_mw: u32_at(data, entry_off(i) + 0xA),
        })
        .collect())
}

/// 一个 OCP 策略行点位（direct patch 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcpPolicyPoint {
    pub row: usize,
    /// maximum 域（u32）镜像内绝对偏移。
    pub maximum_offset: usize,
    /// default 域（u32）镜像内绝对偏移（--nvvdd-ocp-amps-default 目标）。
    pub default_offset: usize,
    /// type 低 4 位（10 = NVVDD OCP——tables_v3 文档化语义）。
    pub kind: u8,
    /// flags bit4（mA）；GB20x 几何无此位（恒 false，行字节即 kind）。
    pub unit_ma: bool,
    pub old_maximum_raw: u32,
    pub old_default_raw: u32,
}

/// 定位功率策略表（P+0x2C）里的 OCP 行（只读）。双几何：
/// 3x（v0x30, hs 0x2C|0x27, es 0x42；kind@+0 flags 同字节，unit bit4，
/// max@+10）与 GB20x（v0x40, hs 0x38|0x3D, es 0x66；行字节即 kind，
/// max@+21）。选择 kind==10（文档化 NVVDD OCP）的行；没有则列出全部行
/// （kind/max 偏移/default 值）供 `--patch offset=hex` 手动打点——绝不猜。
pub fn locate_ocp_policy_peaks(data: &[u8]) -> Result<Vec<OcpPolicyPoint>, Error> {
    let t = match slot_target(data, 0x2C)? {
        None => return Err(Error::from("power_p2: P+0x2C power policy table absent")),
        Some((_, t)) => t,
    };
    let version = data.get(t).copied().unwrap_or(0);
    let hs = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let es = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let (def_in_row, max_in_row, gb20x) = match (version, hs, es) {
        (0x30, 0x2C, 0x42) | (0x30, 0x27, 0x42) => (6usize, 10usize, false),
        (0x40, 0x38, 0x66) | (0x40, 0x3D, 0x66) => (17usize, 21usize, true),
        _ => {
            return Err(Error::from(
                "power_p2: unsupported power policy variant for OCP locate",
            ));
        }
    };
    let row_off = |i: usize| t + hs + i * es;
    let mut points = Vec::new();
    let mut listing = Vec::new();
    for i in 0..cnt.min(64) {
        let r = row_off(i);
        if r + max_in_row + 4 > data.len() {
            break;
        }
        let kind = data[r] & 0x0F;
        let unit_ma = !gb20x && data[r] & 0x10 != 0;
        let raw = u32_at(data, r + max_in_row).unwrap_or(0);
        let old_def = u32_at(data, r + def_in_row).unwrap_or(0);
        listing.push(format!(
            "row {i:2} kind={kind:2} max@{:#x} default={raw}",
            r + max_in_row
        ));
        if kind == 10 {
            points.push(OcpPolicyPoint {
                row: i,
                maximum_offset: r + max_in_row,
                default_offset: r + def_in_row,
                kind,
                unit_ma,
                old_maximum_raw: raw,
                old_default_raw: old_def,
            });
        }
    }
    if points.is_empty() {
        return Err(Error::from(format!(
            "power_p2: no kind-10 (NVVDD OCP) policy row on this image; rows for manual --patch: {}",
            listing.join(" | ")
        )));
    }
    Ok(points)
}

/// 把 POWER BUDGET（P+0x2C，nvidia-smi Max Power Limit 源头）的 **cap 条目**
/// 的 peak/max 域（entry+0xA，u32 LE，mW）改写为 `peak_mw`，并修复所在镜像
/// 的加性校验（镜像末字节 = 使整镜像字节和 ≡ 0 mod 256，与 vix loader 的
/// "valid checksum" 要求一致）。
///
/// 目标条目选择（fail-closed，2026-11-16 代码 43 教训）：见
/// [`locate_power_budget_cap_peak`]——**绝不盲扫全表**：2070 实测把 tag
/// 0x00 占位条目和 tag 0x15 子轨条目（16.5/21.75/33.75 W 的 VRM 轨功率）
/// 一并抬高会让驱动 override 加载时一致性校验拒绝 → GPU 代码 43。
///
/// min（+2）/default-avg（+6）与其余条目保持原值——只抬总板上限。
pub fn patch_power_budget_peak_mw(
    img: &mut [u8],
    peak_mw: u32,
) -> Result<Vec<PowerBudgetPatch>, Error> {
    // 只读定位与写镜像共用同一目标选择器，两条车道打同一个字节。
    let points = locate_power_budget_cap_peak(img)?;
    let peak_bytes = peak_mw.to_le_bytes();
    let mut patched = Vec::new();
    for p in points {
        let old = u32_at(img, p.peak_offset);
        img[p.peak_offset..p.peak_offset + 4].copy_from_slice(&peak_bytes);
        patched.push(PowerBudgetPatch {
            index: p.index,
            offset: p.peak_offset - 0xA,
            tag: p.tag,
            old_peak_mw: old,
            new_peak_mw: Some(peak_mw),
        });
    }
    // 修复包含该表的镜像（首个 55 AA legacy image，长度 = 头部 +2 u16 ×512）。
    let base = find_image_base(img)
        .ok_or_else(|| Error::from("power_p2: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(patched)
}

// ── PowerSense（P+0x28，功率计配置）──────────────────────────────────────

/// 一条功率传感配置：mode + EXTDEV 表设备号 + 载荷原始字节。
/// 载荷按 EXTDEV 类型解释（INA219/209 = mohm+config；INA3221 = 3×mohm+
/// config），EXTDEV 缺席/全 UNUSED 时（Turing+ 内置分流）保持 raw。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerSenseEntry {
    pub index: usize,
    pub offset: usize,
    pub mode: u8,
    pub extdev_id: u8,
    pub raw_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerSense {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<PowerSenseEntry>,
}

/// 解析 POWER SENSE（P+0x28；v0x10 mode@1/extdev@2/raw@3，v0x20 mode@0/
/// extdev@1/raw@5）。
pub fn find_power_sense(data: &[u8]) -> Result<Option<PowerSense>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x28)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if !matches!(version, 0x10 | 0x20) {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let (mode_at, extdev_at, raw_at) = if version == 0x10 {
        (1, 2, 3)
    } else {
        (0, 1, 5)
    };
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hlen + i * rlen;
        let Some(mode) = data.get(e + mode_at).copied() else {
            break;
        };
        let Some(extdev_id) = data.get(e + extdev_at).copied() else {
            break;
        };
        let end = (e + rlen).min(data.len());
        entries.push(PowerSenseEntry {
            index: i,
            offset: e,
            mode,
            extdev_id,
            raw_hex: data[(e + raw_at).min(end)..end]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        });
    }
    Ok(Some(PowerSense {
        offset: t,
        version,
        entries,
    }))
}

// ── PowerBaseClock（P+0x38，per-Pstate 基准/加速时钟）────────────────────

/// 一条基准时钟：pstate + selen×secount 子条目原始字节。
/// 子条目内 u16 的 MHz 换算存在 ×2/×4 编码分歧（见模块注释），不换算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBaseClockEntry {
    pub index: usize,
    pub offset: usize,
    pub pstate_code: u8,
    /// v0x10：请求功率 ×10 mW。
    pub req_power_mw: Option<u32>,
    /// v0x10：请求 slowdown 功率 ×10 mW。
    pub req_slowdown_power_mw: Option<u32>,
    /// 子条目原始字节（每条 selen 字节，共 secount 条）。
    pub subentries_hex: Vec<String>,
}

/// POWER BASE CLOCK 表头命名条目索引（0xFF=缺席）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerBaseClockRoles {
    pub boost: u8,
    pub turbo_boost: u8,
    pub rated_tdp: u8,
    pub vrhot: u8,
    pub max_batt: u8,
    /// 仅 v0x10 头（hlen≥0x14 起）。
    pub slowdown_pwr: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBaseClock {
    pub offset: usize,
    pub version: u8,
    pub secount: u8,
    pub selen: u8,
    pub roles: PowerBaseClockRoles,
    pub entries: Vec<PowerBaseClockEntry>,
}

/// 解析 POWER BASE CLOCK（P+0x38；v0x10/v0x20）。
pub fn find_power_base_clock(data: &[u8]) -> Result<Option<PowerBaseClock>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x38)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if !matches!(version, 0x10 | 0x20) {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let (cnt, secount, selen) = if version == 0x10 {
        (
            usize::from(data.get(t + 5).copied().unwrap_or(0)),
            data.get(t + 4).copied().unwrap_or(0),
            data.get(t + 3).copied().unwrap_or(0),
        )
    } else {
        (
            usize::from(data.get(t + 3).copied().unwrap_or(0)),
            data.get(t + 4).copied().unwrap_or(0),
            data.get(t + 5).copied().unwrap_or(0),
        )
    };
    let g = |a: usize| -> u8 { data.get(t + a).copied().unwrap_or(0xFF) };
    let roles = if version == 0x10 {
        PowerBaseClockRoles {
            boost: g(0x0F),
            turbo_boost: if hlen > 0x10 { g(0x10) } else { 0xFF },
            rated_tdp: if hlen > 0x11 { g(0x11) } else { 0xFF },
            vrhot: g(0x0B),
            max_batt: g(0x0C),
            slowdown_pwr: if hlen > 0x12 { g(0x12) } else { 0xFF },
        }
    } else {
        PowerBaseClockRoles {
            boost: g(0x06),
            turbo_boost: g(0x07),
            rated_tdp: g(0x08),
            vrhot: g(0x0C),
            max_batt: g(0x0D),
            slowdown_pwr: 0xFF,
        }
    };
    let stride = rlen + usize::from(selen) * usize::from(secount);
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hlen + i * stride;
        let Some(pstate) = data.get(e).copied() else {
            break;
        };
        if pstate == 0x00 || pstate == 0xFF {
            continue;
        }
        let mut subentries_hex = Vec::new();
        for s in 0..usize::from(secount) {
            let sub = e + rlen + s * usize::from(selen);
            let end = (sub + usize::from(selen)).min(data.len());
            subentries_hex.push(
                data[sub.min(end)..end]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            );
        }
        let (req_power_mw, req_slowdown_power_mw) = if version == 0x10 {
            (
                u16_at(data, e + 1).map(|v| u32::from(v) * 10),
                u16_at(data, e + 3).map(|v| u32::from(v) * 10),
            )
        } else {
            (None, None)
        };
        entries.push(PowerBaseClockEntry {
            index: i,
            offset: e,
            pstate_code: pstate,
            req_power_mw,
            req_slowdown_power_mw,
            subentries_hex,
        });
    }
    Ok(Some(PowerBaseClock {
        offset: t,
        version,
        secount,
        selen,
        roles,
        entries,
    }))
}

// ── VOLTAGE（P+0x0C，pre-Volta VID/PWM 电压梯）───────────────────────────

/// 解码后的 VOLTAGE 表摘要。版本跨度大（0x10-0x50），统一给出几何 +
/// 关键字段；条目保持原始 hex（VID 语义逐版本不同）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoltageTable {
    pub offset: usize,
    pub version: u8,
    pub header_length: u8,
    pub entry_length: u8,
    pub entry_count: u8,
    /// GPIO VID mask（v0x50 PWM 模式无）。
    pub gpio_mask: Option<u8>,
    /// v0x40+：基准电压 µV。
    pub volt_uv: Option<u32>,
    /// v0x40+：步进 µV。
    pub step_uv: Option<u16>,
    /// v0x40：使用上限 µV。
    pub max_uv_used: Option<u32>,
    /// v0x50：bit0=PWM 模式、bit1=头生成 GPIO。
    pub flags: Option<u8>,
    pub entries_hex: Vec<String>,
}

/// VID GPIO tag 序（envytools nvbios.c vidtag）。
pub const VID_GPIO_TAGS: [u8; 8] = [0x04, 0x05, 0x06, 0x1A, 0x73, 0x74, 0x75, 0x76];

/// 解析 VOLTAGE（P+0x0C；v0x10/0x12/0x20/0x30/0x40/0x50）。
pub fn find_voltage(data: &[u8]) -> Result<Option<VoltageTable>, Error> {
    let Some((_, t)) = slot_target(data, 0x0C)? else {
        return Ok(None);
    };
    Ok(parse_voltage(data, t))
}

fn parse_voltage(data: &[u8], t: usize) -> Option<VoltageTable> {
    let version = *data.get(t)?;
    let g = |a: usize| -> Option<u8> { data.get(t + a).copied() };
    let (hlen, elen, cnt, mask, volt_uv, step_uv, max_uv_used, flags) = match version {
        0x10 | 0x12 => (Some(5u8), g(1)?, g(2)?, g(4), None, None, None, None),
        0x20 => (Some(g(1)?), g(3)?, g(2)?, g(5), None, None, None, None),
        0x30 => (Some(g(1)?), g(2)?, g(3)?, g(4), None, None, None, None),
        0x40 => (
            Some(g(1)?),
            g(2)?,
            g(3)?,
            g(11),
            u32_at(data, t + 4),
            u16_at(data, t + 8),
            u32_at(data, t + 14),
            None,
        ),
        0x50 => {
            let flags = g(4)?;
            let pwm = flags & 1 != 0;
            (
                Some(g(1)?),
                g(2)?,
                g(3)?,
                if pwm { None } else { g(6) },
                u32_at(data, t + 18).map(|v| v & 0x00FF_FFFF),
                None,
                None,
                Some(flags),
            )
        }
        _ => return None,
    };
    let hlen = hlen?;
    let mut entries_hex = Vec::new();
    let start = t + usize::from(hlen);
    for i in 0..usize::from(cnt).min(256) {
        let e = start + i * usize::from(elen);
        if e + usize::from(elen) > data.len() {
            break;
        }
        entries_hex.push(
            data[e..e + usize::from(elen)]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        );
    }
    Some(VoltageTable {
        offset: t,
        version,
        header_length: hlen,
        entry_length: elen,
        entry_count: cnt,
        gpio_mask: mask,
        volt_uv,
        step_uv,
        max_uv_used,
        flags,
        entries_hex,
    })
}

// ── VOLT MAPPING（P+0x20，per-Pstate 电压范围）───────────────────────────

/// 一条电压映射。公式系数 c0..c5 的语义随 mode：
/// 0=线性（c0 + c1·S/10 + c2·S²/100000 µV）、1=S/T 多项式（×5⁶>>18 族）、
/// 2=恒 min、3=(min+max)/2。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoltMapEntry {
    pub index: usize,
    pub offset: usize,
    pub mode: Option<u8>,
    pub link: Option<u8>,
    pub min_uv: u32,
    pub max_uv: u32,
    pub coeffs: [u32; 6],
    pub raw_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoltMapTable {
    pub offset: usize,
    pub version: u8,
    /// v0x20 头里的三个 max-entry 槽。
    pub max_entries: Option<[u8; 3]>,
    pub entries: Vec<VoltMapEntry>,
}

/// 解析 VOLT MAPPING（P+0x20；v0x10=5×u32，v0x20=mode/link+8×u32；
/// TU106 实测 v0x30——envytools 亦未收录，只报头几何）。
pub fn find_volt_map(data: &[u8]) -> Result<Option<VoltMapTable>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x20)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if !matches!(version, 0x10 | 0x20 | 0x30) {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let elen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let max_entries = if version == 0x20 {
        Some([
            data.get(t + 7).copied().unwrap_or(0),
            data.get(t + 8).copied().unwrap_or(0),
            data.get(t + 0xC).copied().unwrap_or(0),
        ])
    } else {
        None
    };
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hlen + i * elen;
        if e + elen > data.len() {
            break;
        }
        let (mode, link, min_uv, max_uv, coeffs) = match version {
            0x10 => (
                None,
                None,
                u32_at(data, e).unwrap_or(0),
                u32_at(data, e + 4).unwrap_or(0),
                [
                    u32_at(data, e + 8).unwrap_or(0),
                    u32_at(data, e + 0xC).unwrap_or(0),
                    u32_at(data, e + 0x10).unwrap_or(0),
                    0,
                    0,
                    0,
                ],
            ),
            0x20 => (
                data.get(e).copied(),
                data.get(e + 1).copied(),
                u32_at(data, e + 2).unwrap_or(0),
                u32_at(data, e + 6).unwrap_or(0),
                [
                    u32_at(data, e + 0xA).unwrap_or(0),
                    u32_at(data, e + 0xE).unwrap_or(0),
                    u32_at(data, e + 0x12).unwrap_or(0),
                    u32_at(data, e + 0x16).unwrap_or(0),
                    u32_at(data, e + 0x1A).unwrap_or(0),
                    u32_at(data, e + 0x1E).unwrap_or(0),
                ],
            ),
            _ => (None, None, 0, 0, [0; 6]),
        };
        entries.push(VoltMapEntry {
            index: i,
            offset: e,
            mode,
            link,
            min_uv,
            max_uv,
            coeffs,
            raw_hex: data[e..e + elen]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        });
    }
    Ok(Some(VoltMapTable {
        offset: t,
        version,
        max_entries,
        entries,
    }))
}

// ── ThermalDevices（P+0x14）/ ThermalCoolers（P+0x18）────────────────────

/// 一条热设备绑定：class_id（1=GPU）+ I2C 端口号 + 旗标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalDeviceEntry {
    pub index: usize,
    pub class_id: u8,
    pub i2c_device: u8,
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalDevicesTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<ThermalDeviceEntry>,
}

/// 解析 THERMAL DEVICES（P+0x14；v0x10，条目 3B）。
pub fn find_thermal_devices(
    data: &[u8],
) -> Result<Option<ThermalDevicesTable>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x14)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if version != 0x10 {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hlen + i * rlen;
        let Some(class_id) = data.get(e).copied() else {
            break;
        };
        entries.push(ThermalDeviceEntry {
            index: i,
            class_id,
            i2c_device: data.get(e + 1).copied().unwrap_or(0),
            flags: data.get(e + 2).copied().unwrap_or(0),
        });
    }
    Ok(Some(ThermalDevicesTable {
        offset: t,
        version,
        entries,
    }))
}

/// 一条风扇校准/热冷却器条目（enable+mode+pwm_freq+duty 域）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalCoolerEntry {
    pub index: usize,
    pub offset: usize,
    pub enable: u8,
    /// 高 4 位 mode_high / 低 3 位 mode_low。
    pub mode_high: u8,
    pub mode_low: u8,
    /// PWM 频率 Hz（envious 注：blob 不使用）。
    pub pwm_freq: u16,
    /// 最大占空比（/4096）。
    pub duty_max: u16,
    /// 占空比偏移（s16）。
    pub duty_offset: i16,
    pub raw_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalCoolersTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<ThermalCoolerEntry>,
}

/// 解析 THERMAL COOLERS（P+0x18；v0x10，条目 ≥0x10B）。
pub fn find_thermal_coolers(
    data: &[u8],
) -> Result<Option<ThermalCoolersTable>, crate::error::Error> {
    let Some((_, t)) = slot_target(data, 0x18)? else {
        return Ok(None);
    };
    let version = data.get(t).copied().unwrap_or(0);
    if version != 0x10 {
        return Ok(None);
    }
    let hlen = usize::from(data.get(t + 1).copied().unwrap_or(0));
    let rlen = usize::from(data.get(t + 2).copied().unwrap_or(0));
    let cnt = usize::from(data.get(t + 3).copied().unwrap_or(0));
    let mut entries = Vec::new();
    for i in 0..cnt.min(16) {
        let e = t + hlen + i * rlen;
        let Some(enable) = data.get(e).copied() else {
            break;
        };
        let mode = data.get(e + 1).copied().unwrap_or(0);
        entries.push(ThermalCoolerEntry {
            index: i,
            offset: e,
            enable,
            mode_high: mode >> 4,
            mode_low: mode & 7,
            pwm_freq: u16_at(data, e + 8).unwrap_or(0),
            duty_max: u16_at(data, e + 0xA).unwrap_or(0),
            duty_offset: u16_at(data, e + 0xC).unwrap_or(0) as i16,
            raw_hex: data[e..(e + rlen).min(data.len())]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        });
    }
    Ok(Some(ThermalCoolersTable {
        offset: t,
        version,
        entries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TU106 1.rom 实测：budget v0x30 cap_entry=2 → 105/175/219 kW·mW 三
    /// 元组与 2070 标称吻合；sense v0x20 7 条；base clock v0x20 17 条；
    /// thermal devices v0x10 18 条（class 1=GPU@0）；coolers v0x10 2 条。
    /// VOLTAGE/VOLT MAP 槽为零（Volta+ rails 接替）。
    /// patch 面：**仅 cap 条目** peak/max 改写 + 镜像加性校验归零 + 重新
    /// 解析读回新值（min/default/其余条目必须保持原值——2070 代码 43
    /// 教训：全表扫会炸掉 tag 0x00/0x15 占位与子轨条目）。
    #[test]
    fn power_budget_patch_roundtrip() {
        let Ok(d0) = std::fs::read("../1.rom") else {
            eprintln!("skip: 1.rom not present");
            return;
        };
        let before = find_power_budget(&d0).expect("ok").expect("budget");
        let cap_idx = usize::from(before.cap_entry.expect("cap entry"));
        let old_min = before.entries[cap_idx].min_mw;
        let old_avg = before.entries[cap_idx].avg_mw;

        let mut img = d0.clone();
        let patched = patch_power_budget_peak_mw(&mut img, 9_999_000).expect("patched");
        // 恰好一条：cap 条目，别无其他。
        assert_eq!(patched.len(), 1);
        assert_eq!(patched[0].index, cap_idx);
        assert_eq!(patched[0].old_peak_mw, Some(219_000));

        // 重解析：cap 条目 peak 抬到 9999 W，min/default 不动，其余条目
        // （含 tag 0x00 占位/子轨）逐字节保持。
        let after = find_power_budget(&img).expect("ok").expect("budget");
        let cap = &after.entries[cap_idx];
        assert_eq!(cap.peak_mw, Some(9_999_000));
        assert_eq!(cap.min_mw, old_min);
        assert_eq!(cap.avg_mw, old_avg);
        for (b, a) in before.entries.iter().zip(after.entries.iter()) {
            if b.index != cap_idx {
                assert_eq!(b.raw_hex, a.raw_hex, "entry {} must be untouched", b.index);
            }
        }

        // 所在镜像（首个 55 AA，长度 = 头 +2 u16 ×512）加性和 ≡ 0。
        let base = crate::vbios::legacy::find_image_base(&img).expect("base");
        let size = (usize::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8)) * 512;
        let end = (base + size).min(img.len());
        let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        assert_eq!(sum, 0, "image checksum must stay zero after patch");
    }

    #[test]
    fn power_p2_tu106_when_present() {
        let Ok(d) = std::fs::read("../1.rom") else {
            eprintln!("skip: 1.rom not present");
            return;
        };
        let budget = find_power_budget(&d).expect("ok").expect("budget");
        assert_eq!(budget.version, 0x30);
        assert_eq!(budget.cap_entry, Some(2));
        let cap = &budget.entries[usize::from(budget.cap_entry.unwrap())];
        assert_eq!(cap.min_mw, Some(105_000));
        assert_eq!(cap.avg_mw, Some(175_000));
        assert_eq!(cap.peak_mw, Some(219_000));

        let sense = find_power_sense(&d).expect("ok").expect("sense");
        assert_eq!(sense.version, 0x20);
        assert_eq!(sense.entries.len(), 7);
        assert_eq!(sense.entries[0].mode, 0x03);

        let bc = find_power_base_clock(&d).expect("ok").expect("base clock");
        assert_eq!(bc.version, 0x20);
        assert_eq!(bc.secount, 4);
        assert_eq!(bc.roles.boost, 0);
        assert_eq!(bc.roles.turbo_boost, 2);
        assert_eq!(bc.roles.rated_tdp, 1);
        assert!(bc.entries.iter().any(|e| e.pstate_code == 0x0F));

        let td = find_thermal_devices(&d)
            .expect("ok")
            .expect("thermal devices");
        assert_eq!(td.version, 0x10);
        assert_eq!(td.entries.len(), 18);
        assert_eq!(td.entries[0].class_id, 1);

        // P+0x18（envytools THERMAL COOLERS/fan_calib）在 TU106 为零槽
        // ——风扇校准走 +0x58 FanCooler（legacy::find_fan_cooler 已覆盖）。
        assert!(find_thermal_coolers(&d).expect("ok").is_none());

        assert!(find_voltage(&d).expect("ok").is_none());

        // P+0x20 在 TU106 是 envytools 版本族外的 v0x30 表（几何可读、
        // 条目不臆测）——Volta+ rails 表族接替前的中间形态。
        let vm = find_volt_map(&d).expect("ok").expect("volt map");
        assert_eq!(vm.version, 0x30);
        assert_eq!(vm.entries.len(), 20);
        assert!(vm.entries.iter().all(|e| e.mode.is_none()));
    }
}
