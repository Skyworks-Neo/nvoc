//! BIT token 级子表——envytools nvbios（mem.c/info.c/nvbios.c/d.c/
//! i2cscript.c/T.c/L.c）Rust 移植的指针级解码。TU106（BIT 17 token）实测
//! 校准，含与 envytools 的两处分歧定案：
//!
//! 1. BIT 'I'（NVINIT）在 Turing 上是 **u16 指针阵列**（17×u16=0x22 与
//!    token len 精确吻合；envytools 按 u32 重叠读出垃圾），envytools 的
//!    some_script@+14 / init96@+16 偏移恰为 u16 槽 7/8，支持本读法。
//! 2. 'M' v2 子表指针在**奇数字节**偏移 +1/+3/+5/+9/+0xD（envytools
//!    parse_at 同款）；group_count 为 u8@+0（TU106=10）。

use crate::error::Error;
use crate::vbios::legacy::{bit_entries, find_bit_signature, find_image_base, relocate_ptr};

/// BIT 目录一行（id/ver/len/offset，offset 为镜像相对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitTokenRow {
    pub id: u8,
    /// ASCII 可打印时给出字符。
    pub id_char: Option<char>,
    pub ver: u8,
    pub len: u16,
    pub offset: u16,
}

/// 枚举 BIT 目录全部 token（含 len=0 占位如 'N'）。
pub fn directory(data: &[u8]) -> Result<Vec<BitTokenRow>, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("bitsubs: 55 AA base not found"))?;
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("bitsubs: BIT signature not found"))?;
    Ok(bit_entries(data, sig, img)?
        .into_iter()
        .map(|e| BitTokenRow {
            id: e.id,
            id_char: char::from_u32(u32::from(e.id)).filter(|c| c.is_ascii_graphic()),
            ver: e.ver,
            len: e.len,
            offset: e.offset,
        })
        .collect())
}

fn locate(
    data: &[u8],
    what: &str,
) -> Result<(usize, usize, Vec<crate::vbios::legacy::BitEntry>), Error> {
    let img = find_image_base(data)
        .ok_or_else(|| Error::from(format!("{what}: 55 AA base not found")))?;
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from(format!("{what}: BIT signature not found")))?;
    let entries = bit_entries(data, sig, img)?;
    Ok((img, sig, entries))
}

/// token 数据区绝对偏移（镜像相对 offset + img 基址；token 内指针再走
/// relocate_ptr）。
fn token_abs(entries: &[crate::vbios::legacy::BitEntry], img: usize, id: u8) -> Option<usize> {
    let e = entries.iter().find(|e| e.id == id)?;
    Some(img + usize::from(e.offset))
}

fn u16_at(data: &[u8], a: usize) -> Option<u16> {
    Some(u16::from(*data.get(a)?) | u16::from(*data.get(a + 1)?) << 8)
}

fn hex(data: &[u8], a: usize, n: usize) -> String {
    data.get(a..(a + n).min(data.len()))
        .unwrap_or(&[])
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── BIT 'M'（内存配置指针表）────────────────────────────────────────────

/// 'M' 表子表名（v2：奇数偏移 u16 指针；v1：仅 RESTRICT@+3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemTablePtrs {
    pub restrict: u16,
    pub ram_type: u16,
    pub train: u16,
    pub train_pattern: u16,
    pub unk0d: u16,
}

/// 'M' v2 MEM TYPE 子表条目（首字节低 4 位 = RAM 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemTypeEntry {
    pub index: usize,
    /// envytools mem_type：0=DDR2、1=DDR3、2=GDDR3、3=GDDR5、6=HBM2、
    /// 8=GDDR5X、9=GDDR6、15=Skip。
    pub ram_type: u8,
    pub ram_type_name: &'static str,
    pub raw_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemTable {
    pub offset: usize,
    pub version: u8,
    /// ram_restrict_group_count（u8；v1@+2、v2@+0）。
    pub group_count: u8,
    pub ptrs: MemTablePtrs,
    /// TYPE 子表（v0x10 时逐条目解 RAM 类型）。
    pub type_table: Option<MemTypeTable>,
    /// TRAIN 子表头（ver/hlen/rlen/sub_len/sub_cnt/entries + mclk u16@6）。
    pub train_header: Option<[u8; 6]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemTypeTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<MemTypeEntry>,
}

/// envytools mem_type 的 RAM 类型名。
pub fn mem_type_name(t: u8) -> &'static str {
    match t {
        0 => "DDR2",
        1 => "DDR3",
        2 => "GDDR3",
        3 => "GDDR5",
        6 => "HBM2",
        8 => "GDDR5X",
        9 => "GDDR6",
        15 => "Skip",
        _ => "Unknown",
    }
}

/// 解析 BIT 'M'（v1/v2；版本取自 BIT 目录条目——表数据区首字节是
/// group_count，没有版本字节）。
pub fn find_mem_table(data: &[u8]) -> Result<Option<MemTable>, Error> {
    let (img, _, entries) = locate(data, "mem")?;
    let Some(me) = entries.iter().find(|e| e.id == b'M') else {
        return Ok(None);
    };
    let Some(t) = token_abs(&entries, img, b'M') else {
        return Ok(None);
    };
    let ver = me.ver;
    if !matches!(ver, 1 | 2) {
        return Ok(None);
    }
    let len = usize::from(me.len);
    let g = |a: usize| -> u16 {
        if a + 2 <= len {
            u16_at(data, t + a).unwrap_or(0)
        } else {
            0
        }
    };
    let ptrs = if ver == 2 {
        MemTablePtrs {
            restrict: g(0x01),
            ram_type: g(0x03),
            train: g(0x05),
            train_pattern: g(0x09),
            unk0d: g(0x0D),
        }
    } else {
        MemTablePtrs {
            restrict: g(0x03),
            ram_type: 0,
            train: 0,
            train_pattern: 0,
            unk0d: 0,
        }
    };
    let group_count = if ver == 2 {
        data.get(t).copied().unwrap_or(0)
    } else {
        data.get(t + 2).copied().unwrap_or(0)
    };
    let mut type_table = None;
    if ptrs.ram_type != 0
        && let Some(tt) = relocate_ptr(data, img, u32::from(ptrs.ram_type))
    {
        type_table = parse_mem_type(data, tt);
    }
    let mut train_header = None;
    if ptrs.train != 0
        && let Some(tr) = relocate_ptr(data, img, u32::from(ptrs.train))
    {
        train_header = data.get(tr..tr + 6).map(|s| {
            let mut h = [0u8; 6];
            h.copy_from_slice(s);
            h
        });
    }
    Ok(Some(MemTable {
        offset: t,
        version: ver,
        group_count,
        ptrs,
        type_table,
        train_header,
    }))
}

fn parse_mem_type(data: &[u8], t: usize) -> Option<MemTypeTable> {
    let version = *data.get(t)?;
    if version != 0x10 {
        return None;
    }
    let hlen = usize::from(*data.get(t + 1)?);
    let rlen = usize::from(*data.get(t + 2)?);
    let cnt = usize::from(*data.get(t + 3)?);
    if rlen == 0 || cnt > 64 {
        return None;
    }
    let mut entries = Vec::new();
    for i in 0..cnt {
        let e = t + hlen + i * rlen;
        let Some(b0) = data.get(e).copied() else {
            break;
        };
        entries.push(MemTypeEntry {
            index: i,
            ram_type: b0 & 0x0F,
            ram_type_name: mem_type_name(b0 & 0x0F),
            raw_hex: hex(data, e, rlen),
        });
    }
    Some(MemTypeTable {
        offset: t,
        version,
        entries,
    })
}

// ── BIT 'I'（NVINIT 脚本指针阵列）───────────────────────────────────────

/// 'I' 表槽位名（envytools nvbios.c 顺序；u16 阵列读法，槽 7/8 =
/// envytools 的 +14/+16 some_script / init96）。
pub const NVINIT_SLOT_NAMES: [&str; 9] = [
    "init_script",
    "macro_index",
    "macro",
    "condition",
    "io_condition",
    "io_flag_condition",
    "init_function",
    "some_script",
    "init96",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvinitTable {
    pub offset: usize,
    /// 命名槽位指针（镜像相对；0 = 缺席）。最多前 9 槽。
    pub slots: Vec<(u16, u16)>,
    pub slots_abs: Vec<Option<usize>>,
}

/// 解析 BIT 'I'（v1，u16 指针阵列）。
pub fn find_nvinit(data: &[u8]) -> Result<Option<NvinitTable>, Error> {
    let (img, _, entries) = locate(data, "nvinit")?;
    let Some(t) = token_abs(&entries, img, b'I') else {
        return Ok(None);
    };
    let len = usize::from(entries.iter().find(|e| e.id == b'I').map_or(0, |e| e.len));
    if len < 2 {
        return Ok(None);
    }
    let mut slots = Vec::new();
    let mut slots_abs = Vec::new();
    for i in 0..(len / 2).min(NVINIT_SLOT_NAMES.len()) {
        let v = u16_at(data, t + i * 2).unwrap_or(0);
        slots.push((i as u16, v));
        slots_abs.push(if v == 0 {
            None
        } else {
            relocate_ptr(data, img, u32::from(v))
        });
    }
    Ok(Some(NvinitTable {
        offset: t,
        slots,
        slots_abs,
    }))
}

// ── BIT 'C'（PLL limits 指针）───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PllLimits {
    /// 'C' token 数据偏移。
    pub token_offset: usize,
    /// +8 处的 u16 指针与重定位后目标。
    pub target: u16,
    pub target_abs: Option<usize>,
    pub version: u8,
    pub header: [u8; 4],
}

/// 解析 BIT 'C'：PLL limits 表指针在 +8（u16；envytools nvbios.c L1119）。
/// TU106 实测目标 ver 0x35（envytools 版本族外，只报头）。
pub fn find_pll_limits(data: &[u8]) -> Result<Option<PllLimits>, Error> {
    let (img, _, entries) = locate(data, "pll limits")?;
    let Some(t) = token_abs(&entries, img, b'C') else {
        return Ok(None);
    };
    let Some(target) = u16_at(data, t + 8) else {
        return Ok(None);
    };
    if target == 0 {
        return Ok(None);
    }
    let abs = relocate_ptr(data, img, u32::from(target));
    let mut header = [0u8; 4];
    if let Some(a) = abs {
        for (i, h) in header.iter_mut().enumerate() {
            *h = data.get(a + i).copied().unwrap_or(0);
        }
    }
    Ok(Some(PllLimits {
        token_offset: t,
        target,
        target_abs: abs,
        version: header[0],
        header,
    }))
}

// ── BIT 'd'（DP INFO）───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpInfo {
    pub offset: usize,
    pub version: u8,
    pub header_length: u8,
    pub target_size: u8,
    pub level_entry_tables: u8,
    pub level_entry_size: u8,
    pub level_entry_count: u8,
    pub flags: u8,
    /// v0x42：regular/low vswing 表项数。
    pub regular_vswing: Option<u16>,
    pub low_vswing: Option<u16>,
}

/// 解析 BIT 'd'：token 数据 = u16 指针 → DP INFO 表（v0x40-0x42）。
pub fn find_dp_info(data: &[u8]) -> Result<Option<DpInfo>, Error> {
    let (img, _, entries) = locate(data, "dp info")?;
    let Some(t) = token_abs(&entries, img, b'd') else {
        return Ok(None);
    };
    let Some(ptr) = u16_at(data, t) else {
        return Ok(None);
    };
    if ptr == 0 {
        return Ok(None);
    }
    let Some(dp) = relocate_ptr(data, img, u32::from(ptr)) else {
        return Ok(None);
    };
    let version = data.get(dp).copied().unwrap_or(0);
    if !matches!(version, 0x40..=0x42) {
        return Ok(None);
    }
    let g = |i: usize| data.get(dp + i).copied().unwrap_or(0);
    Ok(Some(DpInfo {
        offset: dp,
        version,
        header_length: g(1),
        target_size: g(4),
        level_entry_tables: g(5),
        level_entry_size: g(6),
        level_entry_count: g(7),
        flags: g(8),
        regular_vswing: if version >= 0x42 {
            u16_at(data, dp + 9)
        } else {
            None
        },
        low_vswing: if version >= 0x42 {
            u16_at(data, dp + 11)
        } else {
            None
        },
    }))
}

// ── BIT '2'（i2c script）/'T'（TMDS）/'U'（display script）/'V'（virtual strap）

/// '2' token：unk u16@0 + devinit 脚本指针 u16@2。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct I2cScript {
    pub unk00: u16,
    pub script_ptr: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SimplePtrTable {
    pub offset: usize,
    /// token 数据原始字节（≤16B）。
    pub raw_hex: String,
    /// 前 8 个 u16 槽（镜像相对指针或数据）。
    pub words: [u16; 8],
}

/// 解析 BIT '2'（t_len=4）。
pub fn find_i2c_script(data: &[u8]) -> Result<Option<I2cScript>, Error> {
    let (img, _, entries) = locate(data, "i2c script")?;
    let Some(t) = token_abs(&entries, img, b'2') else {
        return Ok(None);
    };
    let Some(a) = u16_at(data, t) else {
        return Ok(None);
    };
    let Some(b) = u16_at(data, t + 2) else {
        return Ok(None);
    };
    Ok(Some(I2cScript {
        unk00: a,
        script_ptr: b,
    }))
}

/// 解析任意小 token 为原始 u16 槽阵列（'T'/'U'/'V'/'A' 等 envytools 无
/// 布局细节的表；如实携带不臆测）。
pub fn find_token_words(data: &[u8], id: u8) -> Result<Option<SimplePtrTable>, Error> {
    let (img, _, entries) = locate(data, "token words")?;
    let Some(t) = token_abs(&entries, img, id) else {
        return Ok(None);
    };
    let len = usize::from(entries.iter().find(|e| e.id == id).map_or(0, |e| e.len));
    if len == 0 {
        return Ok(None);
    }
    let mut words = [0u16; 8];
    for (i, w) in words.iter_mut().enumerate() {
        if i * 2 + 2 > len {
            break;
        }
        *w = u16_at(data, t + i * 2).unwrap_or(0);
    }
    Ok(Some(SimplePtrTable {
        offset: t,
        raw_hex: hex(data, t, len.min(16)),
        words,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TU106 1.rom 实测：17 token 目录；'M' v2 group_count=10、TYPE 表
    /// 16 条（GDDR6×7 + Skip×9）；'I' u16 阵列 init_script=0x462F、
    /// macro 槽为 0；'C' → ver 0x35；'d' → DP INFO v0x42。
    #[test]
    fn bitsubs_tu106_when_present() {
        let Ok(d) = std::fs::read("../1.rom") else {
            eprintln!("skip: 1.rom not present");
            return;
        };
        let dir = directory(&d).expect("dir");
        // 16 条 = 17 token 减 'N'（len=0/offset=0 占位被 plausibility 门滤除）。
        assert_eq!(dir.len(), 16);
        assert!(dir.iter().any(|t| t.id_char == Some('P') && t.len == 0xC4));

        let m = find_mem_table(&d).expect("ok").expect("mem table");
        assert_eq!(m.version, 2);
        assert_eq!(m.group_count, 10);
        assert_eq!(m.ptrs.ram_type, 0x4AF3);
        let tt = m.type_table.expect("mem type");
        assert_eq!(tt.entries.len(), 16);
        assert_eq!(tt.entries[0].ram_type_name, "GDDR6");
        assert!(
            tt.entries
                .iter()
                .filter(|e| e.ram_type_name == "Skip")
                .count()
                >= 9
        );
        assert!(m.train_header.is_some());

        let i = find_nvinit(&d).expect("ok").expect("nvinit");
        assert_eq!(i.slots.len(), 9);
        assert_eq!(i.slots[0].1, 0x462F);
        assert_eq!(i.slots[2].1, 0);
        assert!(i.slots_abs[0].is_some());

        let c = find_pll_limits(&d).expect("ok").expect("pll limits");
        assert_eq!(c.target, 0x739F);
        assert!(c.target_abs.is_some());

        let dp = find_dp_info(&d).expect("ok").expect("dp info");
        assert_eq!(dp.version, 0x42);
        assert_eq!(dp.header_length, 0x0D);
        assert_eq!(dp.target_size, 0x13);
        assert_eq!(dp.level_entry_tables, 4);
        assert_eq!(dp.level_entry_size, 3);
        assert_eq!(dp.level_entry_count, 0x0A);
        assert_eq!(dp.regular_vswing, Some(0x2328));
        assert_eq!(dp.low_vswing, Some(0x2328));

        let i2cs = find_i2c_script(&d).expect("ok").expect("i2c script");
        assert_eq!(i2cs.unk00, 0);
        assert_eq!(i2cs.script_ptr, 0);

        let u = find_token_words(&d, b'U').expect("ok").expect("'U'");
        assert_eq!(u.words[0], 0x4DB8);
        let v = find_token_words(&d, b'V').expect("ok").expect("'V'");
        assert_eq!(v.words[0], 0x4E9F);
    }
}
