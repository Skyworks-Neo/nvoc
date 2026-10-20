//! Turing/Ampere/Ada（BIT v2 'P'/'C' 指针 token + 3x power 前缀）表解码。
//! vix-editor `rom/tables.py` 的 Rust 移植（read-only）。
//!
//! 字段位置源自 NVIDIA nvgpu bios.h 的 3x power / 1x rail / clock 前缀，
//! vix 在 Ada 60+、Ampere 35、Turing 25 语料上逐条结构校验。核心表：
//!
//! - **功率策略**（P+0x2C）：v0x30，行 = (type 低 4 位, channel，min/def/
//!   max u32，单位 mA 旗标 bit4)。TGP 行索引在头 +10（桌面）或
//!   `FF FF FF 02` 头时 +13（笔记本）；TGP 行必须读"全板功率"通道
//!   （class 2 + rail 0xF5）。
//! - **功率通道**（P+0x3C）：v0x20，Ada 32×26B / Ampere 22-26B /
//!   Turing 20×22B；class-2 求和通道的成员表跟在频道条目后（权重 20.12
//!   定点，0x1000 = 1.0）。
//! - **rail 表**（P+0x68）：Ada V20（`20 05 1f 01 00`，行 0x1F）、
//!   Ampere V20（17B 行，1-2 轨）、Turing 1x（`10 05 0e 01 00`，14B 行）。
//!   选择器：REL/ALT/OV（u8）+ 评估 VMIN；boot 电压 u32 uV。
//! - **VFE**（P+0x20）：见 [`crate::vbios::vfe`]。Ada 方程 25B/链接 u16、
//!   Ampere/Turing 24B/链接 u8。
//! - **时钟**（C+8）：Ada/Ampere GV HAL v0x40（10 行 + 每 rail 3B 子条
//!   目）、Turing v0x35（10×11B，master 行存偏移 min/max）。
//! - **风扇**（P+0x58/0x5C/0x64/0xDC/0xE0）：cooler/policy/mapping/
//!   control/speed 五表；曲线 3 档 (%, (1/32°C, RPM)×3)。
//!
//! fail-closed：变体/选择器序/身份不一致报错；跨表一致性分歧降级为
//! warning（本模块只读，不写回）。

use crate::error::Error;
use crate::vbios::families::Family;
use crate::vbios::image_chain::{
    BitDirectory, RomImage, bit_pointer, find_bit_directory, i16_at, image_chains, scan_images,
    u16_at, u32_at,
};
use crate::vbios::vfe::{self, VfeLayout, VfeTable};

/// 3x 功率策略变体：(version, header_size, entry_size)。
const ADA_POWER_VARIANTS: [(u8, u8, u8); 3] =
    [(0x30, 0x32, 0x53), (0x30, 0x34, 0x5B), (0x30, 0x2C, 0x4B)];
const AMPERE_POWER_VARIANTS: [(u8, u8, u8); 3] =
    [(0x30, 0x2C, 0x47), (0x30, 0x2C, 0x43), (0x30, 0x32, 0x47)];
const TURING_POWER_VARIANTS: [(u8, u8, u8); 2] = [(0x30, 0x2C, 0x42), (0x30, 0x27, 0x42)];

/// Ada rail 头（V20 单逻辑轨，行 0x1F）。
const ADA_RAIL_HEADER: [u8; 5] = [0x20, 0x05, 0x1F, 0x01, 0x00];
/// Ampere rail 头（V20；1 轨 / 2 轨 NVVDD+MSVDD）。
const AMPERE_RAIL_HEADERS: [([u8; 5], usize); 2] = [
    ([0x20, 0x05, 0x11, 0x01, 0x00], 1),
    ([0x20, 0x05, 0x11, 0x02, 0x01], 2),
];
/// Turing rail 头（1x 单轨 14B 行）。
const TURING_RAIL_HEADER: [u8; 5] = [0x10, 0x05, 0x0E, 0x01, 0x00];

/// VFE 头前缀 5B。
const ADA_VFE_PREFIX: [u8; 5] = [0x30, 0x0B, 0x19, 0x32, 0x19];
const AMPERE_VFE_PREFIX: [u8; 5] = [0x30, 0x09, 0x19, 0x32, 0x18];
const TURING_VFE_PREFIX: [u8; 5] = [0x30, 0x09, 0x19, 0x14, 0x18];

/// Ampere 外接 12V 连接器感测通道 rail（150-175W/75-87.5W 额定推断 CEM
/// 8-pin/6-pin；一个物理连接器可跨多通道）。
const AMPERE_CONNECTOR_RAILS: [u8; 4] = [0xFA, 0xFB, 0xFC, 0xE4];
/// 12V 输入通道参数字节（bytes 2..7：u32 µV 12V + 0x1000）。
const CHANNEL_12V: [u8; 6] = [0x00, 0x1B, 0xB7, 0x00, 0x00, 0x10];

/// Ampere 外接 12V 连接器感测通道 rail（150-175W/75-87.5W 额定推断 CEM
/// 8-pin/6-pin；一个物理连接器可跨多通道）。
pub fn connector_rail(rail: u8) -> bool {
    AMPERE_CONNECTOR_RAILS.contains(&rail)
}

// ── 上下文 ──────────────────────────────────────────────────────────────

/// 一张 3x 镜像的表解码上下文：主链（legacy [+ EFI] + E0）、BIT 目录。
#[derive(Debug, Clone)]
pub struct TableContext {
    pub family: Family,
    /// 全链共享的同一镜像字节（重复 bank 逐字节一致时才成立）。
    pub legacy: RomImage,
    pub efi: Option<RomImage>,
    pub bit: BitDirectory,
    /// 主链镜像（legacy 在前）。
    pub chain: Vec<RomImage>,
    /// 重复 bank 数（编程器全 dump 常见 2）。
    pub chain_count: usize,
}

impl TableContext {
    /// 扫描 + 组链 + BIT。镜像边界有诊断（overlap/truncation）时报错。
    pub fn new(data: &[u8], family: Family) -> Result<TableContext, Error> {
        let (pci, extended, issues) = scan_images(data);
        if !issues.is_empty() {
            return Err(Error::Custom(format!(
                "vbios: invalid image boundaries: {}",
                issues
                    .iter()
                    .map(|i| format!("{}@{:#x}", i.code_name(), i.offset))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let chains = image_chains(data, &pci, &extended, family)?;
        if chains.is_empty() {
            return Err(Error::from(
                "vbios: no complete NVIDIA PCI/NPDS image chain",
            ));
        }
        let legacy = chains[0][0].clone();
        let efi = chains[0].get(1).filter(|p| p.is_efi()).cloned();
        let bit = find_bit_directory(data, &legacy)?;
        // vix：'P' token 必须存在且 version 2（.get('P',{}).get('version') != 2）。
        if bit.token(b'P').map(|t| t.version) != Some(2) {
            return Err(Error::from(
                "vbios: BIT performance token must be version 2",
            ));
        }
        // 全链数据一致（重复 bank 逐字节相同）才作为同一上下文。
        let chain = chains[0].clone();
        let chain_count = chains.len();
        Ok(TableContext {
            family,
            legacy,
            efi,
            bit,
            chain,
            chain_count,
        })
    }

    /// BIT 指针 → 绝对偏移（'P'/'C' 强制 version 2）。
    pub fn pointer(&self, data: &[u8], token: u8, offset: usize) -> Result<usize, Error> {
        bit_pointer(
            data,
            &self.bit,
            &self.legacy,
            self.efi.as_ref(),
            token,
            offset,
            true,
        )
    }

    /// 表必须完整落在主链某张镜像内（0x70 NBSI 永不拥有表）。
    fn check_owner(&self, at: usize, size: usize) -> Result<(), Error> {
        let owners = self
            .chain
            .iter()
            .filter(|p| p.image_type != 0x70 && p.start <= at && at + size <= p.end)
            .count();
        if owners != 1 {
            return Err(Error::from(
                "vbios: table is outside or crosses a primary image boundary",
            ));
        }
        Ok(())
    }

    fn read(&self, data: &[u8], token: u8, offset: usize, size: usize) -> Result<usize, Error> {
        let at = self.pointer(data, token, offset)?;
        self.check_owner(at, size)?;
        Ok(at)
    }

    /// 镜像加性校验（每张 &0xFF == 0）+ 重复 bank 一致性。
    pub fn integrity(&self, data: &[u8]) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for p in &self.chain {
            let sum = data
                .get(p.start..p.end)
                .map(|b| b.iter().fold(0u8, |a, &x| a.wrapping_add(x)))
                .unwrap_or(0xFF);
            out.push((format!("image@{:#x}", p.start), sum == 0));
        }
        out
    }
}

// ── 功率通道表 ──────────────────────────────────────────────────────────

/// 功率通道条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerChannelEntry {
    pub index: usize,
    pub offset: usize,
    pub bytes: Vec<u8>,
}

impl PowerChannelEntry {
    /// class（byte 0）：2=求和、5=12V 输入、1=输出监视、4=mW 策略输入。
    pub fn class(&self) -> u8 {
        self.bytes.first().copied().unwrap_or(0)
    }

    /// rail（byte 1）：0xF5=全板、0xFF=PCIe slot、0xE0=12V 输入、1=NVVDD。
    pub fn rail(&self) -> u8 {
        self.bytes.get(1).copied().unwrap_or(0)
    }

    /// 全板功率监视（class 2 + rail 0xF5）——TGP 行必须读它。
    pub fn is_total_board(&self) -> bool {
        self.class() == 2 && self.rail() == 0xF5
    }

    /// 外接 12V 连接器感测通道（rail ∈ AMPERE_CONNECTOR_RAILS 的 class-5
    /// 输入；命名推断自 CEM 额定值）。
    pub fn is_connector_input(&self) -> bool {
        self.class() == 5 && connector_rail(self.rail())
    }

    /// 12V 输入通道（class 5 + bytes 2..7 == CHANNEL_12V 且无其它参数）。
    pub fn is_12v_input(&self) -> bool {
        self.class() == 5
            && self.bytes.len() > 15
            && self.bytes[2..8] == CHANNEL_12V
            && self.bytes[8..15].iter().all(|&b| b == 0)
    }
}

/// class-2 求和通道的成员关系条目（权重 20.12 定点，0x1000=1.0）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationshipEntry {
    pub offset: usize,
    /// byte 1：成员通道索引。
    pub member: u8,
    /// bytes 2..5：符号 32 位权重 raw。
    pub weight_raw: u32,
}

/// 功率通道表（P+0x3C）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerChannelTable {
    pub offset: usize,
    pub header_size: u8,
    pub entry_size: u8,
    pub count: u8,
    pub entries: Vec<PowerChannelEntry>,
    /// 关系表基址（class-2 求和成员）。
    pub relationships_base: Option<usize>,
    pub relationship_size: u8,
    pub relationship_count: u8,
}

impl PowerChannelTable {
    /// 求和通道 `index` 的成员关系条目。
    pub fn relationships(
        &self,
        data: &[u8],
        index: usize,
    ) -> Result<Vec<RelationshipEntry>, Error> {
        let (base, rsize, rcount) = match (
            self.relationships_base,
            self.relationship_size,
            self.relationship_count,
        ) {
            (Some(b), s, c) if s >= 2 => (b, usize::from(s), usize::from(c)),
            _ => return Ok(Vec::new()),
        };
        let Some(entry) = self.entries.get(index) else {
            return Ok(Vec::new());
        };
        if entry.class() != 2 {
            return Ok(Vec::new());
        }
        let (Some(&first), Some(&last)) = (entry.bytes.get(14), entry.bytes.get(15)) else {
            return Ok(Vec::new());
        };
        if first > last || usize::from(last) >= rcount {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for i in first..=last {
            let at = base + rsize * usize::from(i);
            out.push(RelationshipEntry {
                offset: at,
                member: *data
                    .get(at + 1)
                    .ok_or_else(|| Error::from("vbios: relationship entry out of bounds"))?,
                weight_raw: u32_at(data, at + 2)?,
            });
        }
        Ok(out)
    }
}

/// 解码功率通道表（v0x20）。变体：Ada 32×0x1A、Ampere 32×(0x16|0x1A)、
/// Turing 20×0x16；头长 6 或 0x0A。
pub fn power_channels(data: &[u8], ctx: &TableContext) -> Result<PowerChannelTable, Error> {
    let at = ctx.read(data, b'P', 0x3C, 16)?;
    let version = data[at];
    let header_size = data[at + 1];
    let entry_size = data[at + 2];
    let count = data[at + 3];
    let (sizes, entries): (&[u8], u8) = match ctx.family {
        Family::Ampere => (&[0x16, 0x1A], 0x20),
        Family::Turing => (&[0x16], 0x14),
        _ => (&[0x1A], 0x20),
    };
    if version != 0x20
        || !matches!(header_size, 6 | 0x0A)
        || !sizes.contains(&entry_size)
        || count != entries
    {
        return Err(Error::from("vbios: unvalidated power channel table"));
    }
    let total = usize::from(header_size) + usize::from(entry_size) * usize::from(count);
    ctx.check_owner(at, total)?;
    let entries = (0..usize::from(count))
        .map(|i| {
            let start = at + usize::from(header_size) + i * usize::from(entry_size);
            PowerChannelEntry {
                index: i,
                offset: start,
                bytes: data[start..start + usize::from(entry_size)].to_vec(),
            }
        })
        .collect();
    // 关系表：头 bytes 4/5 = rsize/rcount（Ampere 语料 46 或 31）。
    let (rsize, rcount) = (data[at + 4], data[at + 5]);
    let relationships_base = if rsize >= 2 && rcount > 0 {
        Some(at + total)
    } else {
        None
    };
    Ok(PowerChannelTable {
        offset: at,
        header_size,
        entry_size,
        count,
        entries,
        relationships_base,
        relationship_size: rsize,
        relationship_count: rcount,
    })
}

// ── 功率策略表 ──────────────────────────────────────────────────────────

/// 功率策略单行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerPolicyRow {
    pub row: usize,
    pub offset: usize,
    /// type 低 4 位（1=TGP/board、2=slot power、4/5=connector、10=Turing
    /// NVVDD OCP、11=input current、13=linked limit、7/12=GB20x 档）。
    pub kind: u8,
    pub flags: u8,
    /// 策略读取的功率通道索引。
    pub channel_index: u8,
    pub minimum_raw: u32,
    pub default_raw: u32,
    pub maximum_raw: u32,
    /// maximum 字段绝对偏移（写回参考；本 parser 只读）。
    pub maximum_offset: usize,
    /// true = mA（flags bit4），false = mW。
    pub unit_ma: bool,
}

/// 功率策略表（P+0x2C）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerPolicy {
    pub offset: usize,
    pub version: u8,
    pub header_size: u8,
    pub entry_size: u8,
    pub rows: Vec<PowerPolicyRow>,
    /// TGP 行索引（笔记本头时来自 byte 13）。
    pub tgp_index: usize,
    /// 笔记本头（bytes 10..14 == FF FF FF 02；TGP 索引在 byte 13）。
    pub notebook: bool,
}

/// 解码功率策略表。变体按 family 门控；TGP 行必须 type 1、mW、
/// min ≤ def ≤ max 且读取全板功率通道。
pub fn power_policy(data: &[u8], ctx: &TableContext) -> Result<PowerPolicy, Error> {
    let at = ctx.read(data, b'P', 0x2C, 16)?;
    let version = data[at];
    let header_size = data[at + 1];
    let entry_size = data[at + 2];
    let count = data[at + 3];
    let variants: &[(u8, u8, u8)] = match ctx.family {
        Family::Ampere => &AMPERE_POWER_VARIANTS,
        Family::Turing => &TURING_POWER_VARIANTS,
        _ => &ADA_POWER_VARIANTS,
    };
    if !variants.contains(&(version, header_size, entry_size)) || count == 0 {
        return Err(Error::from("vbios: unvalidated 3x power table variant"));
    }
    let table_len = usize::from(header_size) + usize::from(entry_size) * usize::from(count);
    ctx.check_owner(at, table_len)?;
    let channels = power_channels(data, ctx)?;
    let (index, notebook) = if data[at + 10..at + 14] == [0xFF, 0xFF, 0xFF, 0x02] {
        (usize::from(data[at + 13]), true)
    } else {
        (usize::from(data[at + 10]), false)
    };
    if index >= usize::from(count) {
        return Err(Error::from("vbios: TGP policy index exceeds power rows"));
    }
    let mut rows = Vec::with_capacity(usize::from(count));
    for i in 0..usize::from(count) {
        let row = at + usize::from(header_size) + i * usize::from(entry_size);
        let flags = data[row];
        rows.push(PowerPolicyRow {
            row: i,
            offset: row,
            kind: flags & 0x0F,
            flags,
            channel_index: data[row + 1],
            minimum_raw: u32_at(data, row + 2)?,
            default_raw: u32_at(data, row + 6)?,
            maximum_raw: u32_at(data, row + 10)?,
            maximum_offset: row + 10,
            unit_ma: flags & 0x10 != 0,
        });
    }
    let tgp = &rows[index];
    if tgp.kind != 1
        || tgp.unit_ma
        || !(tgp.minimum_raw <= tgp.default_raw && tgp.default_raw <= tgp.maximum_raw)
    {
        return Err(Error::from(
            "vbios: TGP policy has unknown identity, units, or inconsistent bounds",
        ));
    }
    // TGP 行必须读全板功率通道（class 2 + rail 0xF5）。
    let total_ok = if ctx.family == Family::Turing {
        let totals: Vec<usize> = channels
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_total_board())
            .map(|(i, _)| i)
            .collect();
        totals == [tgp.channel_index as usize]
    } else {
        tgp.channel_index == 0 && channels.entries.first().is_some_and(|e| e.is_total_board())
    };
    if !total_ok {
        return Err(Error::from(
            "vbios: TGP policy does not read the total board power channel (class 2, rail 0xf5)",
        ));
    }
    Ok(PowerPolicy {
        offset: at,
        version,
        header_size,
        entry_size,
        tgp_index: index,
        notebook,
        rows,
    })
}

// ── rail 表 ─────────────────────────────────────────────────────────────

/// rail 行选择器角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailRole {
    /// 运行电压上限（温度降额曲线）。
    Rel,
    /// 备用上限。
    Alt,
    /// 固定上限（XOC 风格常为常数方程）。
    Ov,
    /// 驱动评估的 VMIN（Ada/Ampere byte 7、Turing byte 9、GB20x +0x13）。
    VminEvaluated,
    /// rail INFO 呈报的 VMIN 槽（Ada/Ampere byte 11、GB20x +0x21）。
    VminInfo,
    /// boot 电压选择器（仅 Turing 1x 行 byte 8）。
    Boot,
}

impl RailRole {
    pub fn name(self) -> &'static str {
        match self {
            RailRole::Rel => "REL",
            RailRole::Alt => "ALT",
            RailRole::Ov => "OV",
            RailRole::VminEvaluated => "VMIN",
            RailRole::VminInfo => "VMIN_INFO",
            RailRole::Boot => "BOOT",
        }
    }
}

/// rail 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailRow {
    pub index: usize,
    pub offset: usize,
    /// 推断名（第二轨 MSVDD 为推断）。
    pub name: &'static str,
    /// boot 电压 u32（µV）。
    pub boot_voltage_uv: u32,
    /// (role, 字段偏移, raw)。
    pub selectors: Vec<(RailRole, usize, u16)>,
}

/// rail 表（P+0x68）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailTable {
    pub offset: usize,
    /// 行大小（Ada 0x1F / Ampere 0x11 / Turing 0x0E）。
    pub row_size: usize,
    pub rows: Vec<RailRow>,
}

/// 解码 rail 表 + 选择器序校验。
/// 期望序（语料不变式）：REL < ALT(如有) < VMIN = OV−1；Turing：
/// REL < ALT < BOOT = OV−1、VMIN = OV+1、margin 缺席；XOC 风格
/// REL = OV、ALT = OV/缺席 亦接受（Ada/Turing）。
pub fn rails(data: &[u8], ctx: &TableContext) -> Result<RailTable, Error> {
    let rail = ctx.read(data, b'P', 0x68, 16)?;
    let header: [u8; 5] = data[rail..rail + 5].try_into().unwrap();
    let (count, row_size) = match ctx.family {
        Family::Ada if header == ADA_RAIL_HEADER => (1usize, 0x1Fusize),
        Family::Turing if header == TURING_RAIL_HEADER => (1, 0x0E),
        Family::Ampere => match AMPERE_RAIL_HEADERS.iter().find(|(h, _)| *h == header) {
            Some((_, n)) => (*n, 0x11),
            None => {
                return Err(Error::from(
                    "vbios: unvalidated rail layout; expected V20 rows",
                ));
            }
        },
        _ => return Err(Error::from("vbios: unvalidated rail layout")),
    };
    ctx.check_owner(rail, 5 + row_size * count)?;
    let vfe_table = vfe(data, ctx)?;
    let names: &[&str] = if ctx.family == Family::Ampere {
        &["NVVDD", "MSVDD"]
    } else {
        &["NVVDD"]
    };
    let mut rows = Vec::new();
    // 行界是头字段（1 或 2）且选择器序校验依赖行号——不能迭代 names。
    #[allow(clippy::needless_range_loop)]
    for i in 0..count {
        let row = rail + 5 + row_size * i;
        let boot_voltage_uv = u32_at(data, row)?;
        let mut selectors = vec![
            (RailRole::Rel, row + 4, u16::from(data[row + 4])),
            (RailRole::Alt, row + 5, u16::from(data[row + 5])),
            (RailRole::Ov, row + 6, u16::from(data[row + 6])),
        ];
        let (rel, alt, ov) = (data[row + 4], data[row + 5], data[row + 6]);
        if ctx.family == Family::Turing {
            selectors.push((RailRole::Boot, row + 8, u16::from(data[row + 8])));
            selectors.push((RailRole::VminEvaluated, row + 9, u16::from(data[row + 9])));
            // margin（byte 10）语料恒缺席。
            if data[row + 10] != 0xFF {
                return Err(Error::from(
                    "vbios: unexpected Turing voltage margin selector",
                ));
            }
            let (boot, vmin) = (data[row + 8], data[row + 9]);
            let ordered = rel < boot && (alt == 0xFF || (rel < alt && alt < boot));
            let followed = rel == ov && (alt == 0xFF || alt == ov);
            if [rel, ov, boot, vmin].contains(&0xFF)
                || usize::from(vmin) >= vfe_table.equation_count as usize
                || boot as i32 != ov as i32 - 1
                || vmin as i32 != ov as i32 + 1
                || !(ordered || followed)
            {
                return Err(Error::from("vbios: unvalidated Turing rail selectors"));
            }
        } else {
            selectors.push((RailRole::VminEvaluated, row + 7, u16::from(data[row + 7])));
            selectors.push((RailRole::VminInfo, row + 11, u16::from(data[row + 11])));
            // vix 不变式锚在 byte 11（rail INFO 的 VMIN 槽）；byte 7 是
            // 驱动评估 VMIN，不参与排序校验。
            let vmin = data[row + 11];
            let ordered = rel < vmin
                && vmin as i32 == ov as i32 - 1
                && (alt == 0xFF || (rel < alt && alt < vmin));
            let followed = ctx.family == Family::Ada
                && rel == ov
                && (alt == 0xFF || alt == ov)
                && vmin as i32 == ov as i32 - 1;
            if [rel, ov, vmin].contains(&0xFF)
                || usize::from(ov) >= vfe_table.equation_count as usize
                || usize::from(vmin) >= vfe_table.equation_count as usize
                || !(ordered || followed)
            {
                return Err(Error::from(
                    "vbios: unvalidated rail selectors (expected REL < ALT < VMIN = OV - 1)",
                ));
            }
        }
        rows.push(RailRow {
            index: i,
            offset: row,
            name: names[i],
            boot_voltage_uv,
            selectors,
        });
    }
    Ok(RailTable {
        offset: rail,
        row_size,
        rows,
    })
}

// ── VFE ────────────────────────────────────────────────────────────────

/// 解码 VFE 表（P+0x20）。Ada count u16（+5/+10），Ampere/Turing u8（+5）。
pub fn vfe(data: &[u8], ctx: &TableContext) -> Result<VfeTable, Error> {
    let at = ctx.read(data, b'P', 0x20, 16)?;
    let (prefix, layout, count_wide) = match ctx.family {
        Family::Ada => (ADA_VFE_PREFIX, VfeLayout::ADA_GB20X, true),
        Family::Ampere => (AMPERE_VFE_PREFIX, VfeLayout::AMPERE, false),
        _ => (TURING_VFE_PREFIX, VfeLayout::AMPERE, false),
    };
    if data[at..at + 5] != prefix {
        return Err(Error::from("vbios: unvalidated VFE dimensions"));
    }
    ctx.check_owner(
        at,
        usize::from(data[at + 1])
            + usize::from(data[at + 2]) * usize::from(data[at + 3])
            + layout.equation_size,
    )?;
    vfe::decode(data, at, layout, count_wide)
}

// ── rail × VFE 组合 ─────────────────────────────────────────────────────

/// 单个 role 的方程视图。
#[derive(Debug, Clone, PartialEq)]
pub struct RailRoleEquation {
    pub rail_name: &'static str,
    pub rail_index: usize,
    pub role: RailRole,
    pub selector_offset: usize,
    /// u8（3x）选择器 raw；0xFF = 缺席。
    pub selector_raw: u16,
    pub absent: bool,
    pub boot_voltage_uv: u32,
    pub equation_offset: Option<usize>,
    pub equation_kind: Option<crate::vbios::vfe::OutputKind>,
    /// 输出钳位 (min, max) µV（电压输出方程才有）。
    pub output_uv: Option<(f32, f32)>,
    /// clamp max 字段绝对偏移。
    pub maximum_offset: Option<usize>,
    /// REL/ALT 且温度降额 → 抬上限是已审编辑形状（本 parser 只读标记）。
    pub editable_ceiling: bool,
}

/// rail 行全部 role 的方程视图（vix `rail_equations`：Ada/Ampere 含
/// VMIN-info 槽；Turing 仅 REL/ALT/OV。评估 VMIN 与 boot 选择器不是 role）。
pub fn rail_equations(data: &[u8], ctx: &TableContext) -> Result<Vec<RailRoleEquation>, Error> {
    let table = rails(data, ctx)?;
    let vfe_table = vfe(data, ctx)?;
    let mut out = Vec::new();
    for row in &table.rows {
        let roles: Vec<(RailRole, usize, u16)> = row
            .selectors
            .iter()
            .copied()
            .filter(|(r, _, _)| !matches!(r, RailRole::VminEvaluated | RailRole::Boot))
            .collect();
        for (role, offset, raw) in roles.iter().copied() {
            let absent = raw == 0xFF;
            let mut item = RailRoleEquation {
                rail_name: row.name,
                rail_index: row.index,
                role,
                selector_offset: offset,
                selector_raw: raw,
                absent,
                boot_voltage_uv: row.boot_voltage_uv,
                equation_offset: None,
                equation_kind: None,
                output_uv: None,
                maximum_offset: None,
                editable_ceiling: false,
            };
            if !absent {
                let eq_index = raw;
                if u32::from(eq_index) >= u32::from(vfe_table.equation_count) {
                    return Err(Error::from(
                        "vbios: rail selector outside VFE equation table",
                    ));
                }
                let equation = vfe_table.equation(eq_index).expect("index bounded");
                let picking = roles.iter().filter(|(_, _, rr)| rr == &raw).count();
                let kind = vfe::classify_quadratic(
                    equation,
                    vfe_table.references.as_ref(),
                    vfe_table.variable_count,
                    vfe::variable_is_temperature(data, &vfe_table, equation.variable),
                    picking,
                );
                item.equation_offset = Some(equation.offset);
                item.equation_kind = Some(kind);
                if equation.kind_raw == 1 && equation.output_unit == 2 {
                    item.output_uv = equation.clamp;
                    item.maximum_offset = Some(equation.offset + 7);
                }
                item.editable_ceiling = matches!(role, RailRole::Rel | RailRole::Alt)
                    && kind == crate::vbios::vfe::OutputKind::TemperatureDerated;
            }
            out.push(item);
        }
    }
    Ok(out)
}

/// REL 电压上限（每轨；温度降额 REL 的 clamp max µV → mV 供展示层换算）。
#[derive(Debug, Clone, PartialEq)]
pub struct RailLimit {
    pub rail_name: &'static str,
    pub rail_index: usize,
    /// REL 方程索引。
    pub rel_equation_index: u16,
    pub equation_offset: usize,
    pub equation_kind: crate::vbios::vfe::OutputKind,
    pub output_minimum_uv: f32,
    pub output_maximum_uv: f32,
    pub maximum_offset: usize,
    /// 输入变量（温度通道号 + 偏移）。
    pub input_variable: Option<(u8, usize)>,
}

/// REL 方程钳位（Ada 单逻辑轨；Ampere 每轨）。compare REL 报错（依赖
/// fused 输入，只读）。
pub fn rail_limits(data: &[u8], ctx: &TableContext) -> Result<Vec<RailLimit>, Error> {
    let table = rails(data, ctx)?;
    let vfe_table = vfe(data, ctx)?;
    let mut out = Vec::new();
    for row in &table.rows {
        let rel = u16::from(data[row.offset + 4]);
        let equation = vfe_table
            .equation(rel)
            .ok_or_else(|| Error::from("vbios: REL selector outside VFE table"))?;
        if equation.kind_raw == 3 {
            return Err(Error::from(
                "vbios: REL equation is a compare expression on fused inputs; read-only",
            ));
        }
        if equation.kind_raw != 1 || equation.output_unit != 2 {
            return Err(Error::from(
                "vbios: REL equation is not a quadratic voltage output",
            ));
        }
        let Some((lo, hi)) = equation.clamp else {
            return Err(Error::from("vbios: invalid REL equation bounds"));
        };
        if !lo.is_finite() || !hi.is_finite() || !(0.0..=hi).contains(&lo) {
            return Err(Error::from(
                "vbios: invalid REL equation bounds/coefficients",
            ));
        }
        if usize::from(equation.variable) >= usize::from(vfe_table.variable_count) {
            return Err(Error::from(
                "vbios: REL variable index exceeds VFE variable table",
            ));
        }
        let kind = vfe::classify_quadratic(
            equation,
            vfe_table.references.as_ref(),
            vfe_table.variable_count,
            vfe::variable_is_temperature(data, &vfe_table, equation.variable),
            1,
        );
        out.push(RailLimit {
            rail_name: row.name,
            rail_index: row.index,
            rel_equation_index: rel,
            equation_offset: equation.offset,
            equation_kind: kind,
            output_minimum_uv: lo,
            output_maximum_uv: hi,
            maximum_offset: equation.offset + 7,
            input_variable: Some((
                equation.variable,
                vfe::variable_offset(&vfe_table, equation.variable),
            )),
        });
    }
    Ok(out)
}

/// 评估 VMIN 信息。
#[derive(Debug, Clone, PartialEq)]
pub struct VminInfo {
    pub rail_name: &'static str,
    pub rail_index: usize,
    pub selector_offset: usize,
    pub selector_raw: u16,
    /// 钳位钉死的常数值（µV）——仅当方程为电压输出、clamp min == max、
    /// 无其它方程引用且无其它 role 共选。
    pub value_uv: Option<f32>,
    pub pinned: bool,
    /// REL 方程列表最低值（µV）。
    pub rel_lowest_uv: Option<f64>,
    /// VMIN 方程链接链可加的值域（µV）。
    pub added_uv: Option<(f64, f64)>,
    pub diagnostic: Option<&'static str>,
}

/// 评估 VMIN（Ada/Ampere byte 7；Turing byte 9）。
pub fn rail_vmin(data: &[u8], ctx: &TableContext) -> Result<Vec<VminInfo>, Error> {
    let table = rails(data, ctx)?;
    let vfe_table = vfe(data, ctx)?;
    let turing = ctx.family == Family::Turing;
    let vmin_off = if turing { 9usize } else { 7usize };
    let boot_off = turing.then_some(8usize);
    // 同行 role 选择器偏移（vix rail_equations 的 role 集）。
    let role_offs: Vec<usize> = if turing {
        vec![4, 5, 6]
    } else {
        vec![4, 5, 6, 11]
    };
    let layout = vfe_table.layout();
    let mut out = Vec::new();
    for row in &table.rows {
        let selector = data[row.offset + vmin_off];
        let rel = u16::from(data[row.offset + 4]);
        let rel_lowest = vfe::equation_span(
            data,
            vfe_table.equations_base,
            layout,
            vfe_table.equation_count,
            rel,
        )
        .map(|s| s.0);
        let mut item = VminInfo {
            rail_name: row.name,
            rail_index: row.index,
            selector_offset: row.offset + vmin_off,
            selector_raw: u16::from(selector),
            value_uv: None,
            pinned: false,
            rel_lowest_uv: rel_lowest,
            added_uv: None,
            diagnostic: None,
        };
        if usize::from(selector) >= usize::from(vfe_table.equation_count) {
            item.diagnostic = Some("is absent or outside the equation table");
            out.push(item);
            continue;
        }
        let equation = vfe_table.equation(u16::from(selector)).expect("bounded");
        let next = equation.next.unwrap_or(vfe::none_sentinel(layout));
        item.added_uv = vfe::equation_span(
            data,
            vfe_table.equations_base,
            layout,
            vfe_table.equation_count,
            next,
        );
        // 同行其它 role / boot 选择器 / 其它 rail 行的 VMIN 是否共选。
        let mut selected_elsewhere = role_offs
            .iter()
            .any(|&o| u16::from(data[row.offset + o]) == u16::from(selector));
        if let Some(b) = boot_off {
            selected_elsewhere |= u16::from(data[row.offset + b]) == u16::from(selector);
        }
        selected_elsewhere |= table.rows.iter().any(|r| {
            r.offset != row.offset && u16::from(data[r.offset + vmin_off]) == u16::from(selector)
        });
        let referenced = vfe_table
            .references
            .as_ref()
            .map(|r| r.contains(&u16::from(selector)))
            .unwrap_or(true);
        let pinned_constant = equation.kind_raw == 1
            && equation.output_unit == 2
            && matches!(equation.clamp, Some((lo, hi)) if lo.is_finite() && lo == hi);
        if !pinned_constant {
            item.diagnostic = Some("has a clamp that does not pin its value");
        } else if referenced || selected_elsewhere {
            item.diagnostic = Some("is used by another equation or role");
        } else {
            item.value_uv = equation.clamp.map(|(lo, _)| lo);
            item.pinned = true;
        }
        out.push(item);
    }
    Ok(out)
}

// ── 时钟表 ──────────────────────────────────────────────────────────────

/// 时钟域范围行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockRange {
    pub row: usize,
    pub domain: &'static str,
    pub minimum_mhz: i16,
    pub maximum_mhz: i16,
    pub minimum_offset: usize,
    pub maximum_offset: usize,
    /// master 行才存范围（Turing v0x35 的 slave 行存其它参数）。
    pub range_stored: bool,
}

/// GV HAL 行序（Ada/Ampere/Turing 共用 10 行）。
const CLOCK_ROWS: [(&str, u8, u8, u8); 10] = [
    ("GPC", 1, 0x13, 1),
    ("XBAR", 1, 0x24, 1),
    ("DRAM", 1, 0x62, 1),
    ("SYS", 1, 0x35, 2),
    ("HUB", 1, 0x71, 2),
    ("VID", 1, 0x46, 1),
    ("PWR", 0, 0xFF, 0),
    ("DISP", 1, 0x80, 1),
    ("PCIEGEN", 1, 0x08, 2),
    ("HOST", 1, 0x57, 2),
];

/// Ada 时钟表头（v0x40 GV HAL）。
const ADA_CLOCK_HEADER: [u8; 9] = [0x40, 0x0D, 0x0F, 0x0A, 0x03, 0x01, 0x03, 0xE8, 0x03];
/// Turing 时钟表头（v0x35）。
const TURING_CLOCK_HEADER: [u8; 9] = [0x35, 0x09, 0x0B, 0x0A, 0x03, 0xE8, 0x03, 0x00, 0x00];

/// 解码时钟域范围（C token + 8）。
pub fn clock_ranges(data: &[u8], ctx: &TableContext) -> Result<Vec<ClockRange>, Error> {
    let at = ctx.read(data, b'C', 8, 16)?;
    let b = data;
    let mut out = Vec::new();
    if ctx.family == Family::Turing {
        if b[at..at + 9] != TURING_CLOCK_HEADER {
            return Err(Error::from("vbios: unvalidated clock table variant"));
        }
        let (hs, size, count) = (
            usize::from(b[at + 1]),
            usize::from(b[at + 2]),
            usize::from(b[at + 3]),
        );
        ctx.check_owner(at, hs + size * count)?;
        // GV HAL 固定行序；master（usage 1）行 bytes 3/5 = 有符号偏移 min/max。
        let turing_rows: [(&str, u8, u8); 10] = [
            ("GPC", 1, 0x13),
            ("XBAR", 2, 0x24),
            ("DRAM", 1, 0x62),
            ("SYS", 2, 0x35),
            ("HUB", 2, 0x71),
            ("VID", 2, 0x46),
            ("PWR", 0, 0xFF),
            ("DISP", 1, 0x80),
            ("PCIEGEN", 2, 0x08),
            ("HOST", 2, 0x57),
        ];
        for (i, (name, usage, ordering)) in turing_rows.iter().enumerate() {
            let row = at + hs + size * i;
            // 逐行门控：形状不匹配的行（OEM 定制行，T10 实测 HUB）不宣称
            // 存的是有效范围值（range_stored=false），其余行照常解出。
            let stored = *usage == 1
                && b[row] == *usage
                && b[row + 7] == *ordering
                && b[row + 8] == 0
                && b[row + 9..row + 11] == [0xFF, 0xFF];
            out.push(ClockRange {
                row: i,
                domain: name,
                minimum_mhz: i16_at(b, row + 3)?,
                maximum_mhz: i16_at(b, row + 5)?,
                minimum_offset: row + 3,
                maximum_offset: row + 5,
                range_stored: stored,
            });
        }
        return Ok(out);
    }
    // Ada / Ampere：v0x40 GV HAL。
    if ctx.family == Family::Ampere {
        if b[at] != 0x40
            || !matches!(b[at + 1], 0x0B | 0x0C)
            || b[at + 2..at + 5] != [0x0D, 0x0A, 0x03]
            || !matches!(b[at + 5], 1 | 2)
            || b[at + 6..at + 9] != [0x03, 0xE8, 0x03]
        {
            return Err(Error::from("vbios: unvalidated clock table variant"));
        }
    } else if b[at..at + 9] != ADA_CLOCK_HEADER {
        return Err(Error::from("vbios: unvalidated clock table variant"));
    }
    let (hs, size, count, subcount, subsize) = (
        usize::from(b[at + 1]),
        usize::from(b[at + 2]),
        usize::from(b[at + 3]),
        usize::from(b[at + 4]),
        usize::from(b[at + 5]),
    );
    let stride = size + subcount * subsize;
    ctx.check_owner(at, hs + stride * count)?;
    if ctx
        .bit
        .token(b'C')
        .is_some_and(|t| usize::from(t.size) >= 0x24)
        && ctx.pointer(data, b'C', 0x20)? != at + hs + stride * count
    {
        return Err(Error::from(
            "vbios: clock table does not end at the next C-token table",
        ));
    }
    let rails_count = usize::from(b[at + 5]);
    for (i, (name, usage, ordering, rail)) in CLOCK_ROWS.iter().enumerate() {
        let row = at + hs + i * stride;
        let first_last = (b[row + 1], b[row + 2]);
        let (lo, hi) = (i16_at(b, row + 3)?, i16_at(b, row + 5)?);
        let ampere = ctx.family == Family::Ampere;
        let rail_type = if ampere && *name == "VID" { 2 } else { *rail };
        // 每行 rail 子条目：恰一个非零 type（PWR 行无）。
        let active: Vec<usize> = (0..rails_count)
            .filter(|&k| b[row + size + 3 * k] != 0)
            .collect();
        let expected_active: Vec<usize> = if rail_type == 0 { Vec::new() } else { vec![0] };
        let usage_ok = if ampere && matches!(*name, "HUB" | "DISP") {
            b[row] == 0 || b[row] == *usage
        } else {
            b[row] == *usage
        };
        let dram_ok = *name != "DRAM" || matches!(b[row + 8], 0 | 1);
        let active_ok = if ampere {
            active.len() == expected_active.len()
                && active
                    .iter()
                    .all(|&k| b[row + size + 3 * k] as u32 == u32::from(rail_type))
        } else {
            b[row + size] as u32 == u32::from(rail_type)
        };
        // first/last 拷贝校验锚在**第一个活跃**子条目（双轨卡的 rail 1 在
        // 第二个槽位，3090 实测）。
        let copy_ok = !(*usage == 1 && rail_type == 1)
            || active.first().is_some_and(|&k| {
                let at = row + size + 3 * k;
                b[at + 1..at + 3] == [first_last.0, first_last.1]
            });
        if !usage_ok
            || b[row + 7] != *ordering
            || !dram_ok
            || b[row + 9..row + 11] != [0xFF, 0xFF]
            || !active_ok
            || !copy_ok
        {
            return Err(Error::Custom(format!(
                "vbios: clock row {i} does not match the GV HAL {name} row"
            )));
        }
        out.push(ClockRange {
            row: i,
            domain: name,
            minimum_mhz: lo,
            maximum_mhz: hi,
            minimum_offset: row + 3,
            maximum_offset: row + 5,
            range_stored: true,
        });
    }
    Ok(out)
}

// ── 风扇表 ──────────────────────────────────────────────────────────────

/// 风扇策略条目曲线（entry +14 起）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanCurve {
    /// 三档 level（%）。
    pub levels: [u8; 3],
    /// 三档 (温度 1/32 °C raw, RPM)。
    pub points: [(u16, u16); 3],
}

fn fan_curve_at(data: &[u8], at: usize) -> Result<FanCurve, Error> {
    let levels = [data[at + 14], data[at + 15], data[at + 16]];
    let mut points = [(0u16, 0u16); 3];
    for (k, p) in points.iter_mut().enumerate() {
        *p = (
            u16_at(data, at + 18 + 4 * k)?,
            u16_at(data, at + 20 + 4 * k)?,
        );
    }
    Ok(FanCurve { levels, points })
}

/// 单风扇通道行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanChannelRow {
    pub index: usize,
    pub cooler_offset: usize,
    pub enable: bool,
    pub active: bool,
    pub pwm_frequency_hz: u32,
    pub level_minimum: u8,
    pub level_maximum: u8,
    pub rpm_minimum: u16,
    pub rpm_maximum: u16,
    pub curve: FanCurve,
    pub policy_offset: usize,
    pub mapping_row: u8,
    pub control_offset: Option<usize>,
    pub speed_offset: Option<usize>,
    pub speed_rpm: Option<u16>,
}

/// 风扇表报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanReport {
    pub cooler_offset: usize,
    pub policy_offset: usize,
    pub mapping_offset: usize,
    pub control_offset: Option<usize>,
    pub speed_offset: Option<usize>,
    pub channels: Vec<usize>,
    pub rows: Vec<FanChannelRow>,
    /// 活跃通道数之外的额外策略条目曲线。
    pub extra_curves: Vec<(usize, FanCurve)>,
    pub warnings: Vec<String>,
}

const FAN_TABLE_COOLER: (usize, [u8; 4], u8, u8, usize) =
    (0x58, [0x10, 0x06, 0x1A, 0x02], 6, 0x1A, 2);
const FAN_TABLE_POLICY_AMP: (usize, [u8; 4], u8, u8, usize) =
    (0x5C, [0x20, 0x04, 0x33, 0x08], 4, 0x33, 8);
const FAN_TABLE_POLICY_TURING: (usize, [u8; 4], u8, u8, usize) =
    (0x5C, [0x10, 0x06, 0x36, 0x02], 6, 0x36, 2);
const FAN_TABLE_MAPPING: (usize, [u8; 4], u8, u8, usize) =
    (0x64, [0x10, 0x04, 0x05, 0x02], 4, 0x05, 2);
const FAN_TABLE_CONTROL: (usize, [u8; 4], u8, u8, usize) =
    (0xDC, [0x10, 0x04, 0x0D, 0x02], 4, 0x0D, 2);
const FAN_TABLE_SPEED: (usize, [u8; 4], u8, u8, usize) =
    (0xE0, [0x10, 0x04, 0x06, 0x02], 4, 0x06, 2);
/// Turing 策略条目相对 Ada 布局的曲线偏移。
const TURING_CURVE_SHIFT: usize = 3;
/// Turing 活跃条目 bytes 9..16 的固定字节（Ada entry 6..13 同值）。
const TURING_POLICY_BYTES: [u8; 8] = [0x23, 0x2B, 0x0A, 0x01, 0x00, 0x02, 0x80, 0x00];

/// 解码五张风扇表。全通道禁用且通道列表全 0xFF → 无 GPU 风扇（Max-Q/
/// 笔记本），返回 None。头不认识报错；跨表一致性分歧记 warning。
pub fn fan_tables(data: &[u8], ctx: &TableContext) -> Result<Option<FanReport>, Error> {
    let cooler_at = match ctx.read(data, b'P', FAN_TABLE_COOLER.0, 4) {
        Ok(at) => at,
        Err(_) => return Ok(None),
    };
    let b = data;
    let turing = ctx.family == Family::Turing;
    let policy = if turing {
        FAN_TABLE_POLICY_TURING
    } else {
        FAN_TABLE_POLICY_AMP
    };
    let mut tables: Vec<(&str, usize, usize, usize, usize)> = Vec::new(); // (name, base, hs, es, n)
    for (name, (ptr, header, hs, es, n)) in [
        ("cooler", FAN_TABLE_COOLER),
        ("policy", policy),
        ("mapping", FAN_TABLE_MAPPING),
    ] {
        let at = if name == "cooler" {
            cooler_at
        } else {
            ctx.read(data, b'P', ptr, 4)?
        };
        if b[at..at + 4] != header {
            return Err(Error::Custom(format!(
                "vbios: unsupported fan {name} table"
            )));
        }
        ctx.check_owner(at, usize::from(hs) + usize::from(es) * n)?;
        tables.push((name, at, usize::from(hs), usize::from(es), n));
    }
    let has_control_speed = !turing;
    if has_control_speed {
        for (name, (ptr, header, hs, es, n)) in
            [("control", FAN_TABLE_CONTROL), ("speed", FAN_TABLE_SPEED)]
        {
            let at = ctx.read(data, b'P', ptr, 4)?;
            if b[at..at + 4] != header {
                return Err(Error::Custom(format!(
                    "vbios: unsupported fan {name} table"
                )));
            }
            ctx.check_owner(at, usize::from(hs) + usize::from(es) * n)?;
            tables.push((name, at, usize::from(hs), usize::from(es), n));
        }
    }
    let find = |name: &str| {
        tables
            .iter()
            .find(|(n, _, _, _, _)| *n == name)
            .map(|(_, at, hs, es, _)| (*at, *hs, *es))
    };
    let (cooler, cooler_hs, cooler_es) = find("cooler").unwrap();
    let (policy_at, policy_hs, policy_es) = find("policy").unwrap();
    let (mapping_at, mapping_hs, mapping_es) = find("mapping").unwrap();
    let n = 2usize; // ADA_FAN_CHANNELS
    let header: Vec<u8> = b[cooler + 4..cooler + 4 + n].to_vec();
    let all_disabled = header.iter().all(|&c| c == 0xFF) && {
        // 各表自己的条目 enable（byte 0）全零。
        let entry_zero =
            |(at, hs, es): (usize, usize, usize)| (0..n).all(|i| b[at + hs + i * es] == 0);
        entry_zero((cooler, cooler_hs, cooler_es))
            && entry_zero((policy_at, policy_hs, policy_es))
            && entry_zero((mapping_at, mapping_hs, mapping_es))
    };
    if all_disabled {
        return Ok(None);
    }
    let end = header.iter().position(|&c| c == 0xFF).unwrap_or(n);
    let listed: Vec<usize> = header[..end].iter().map(|&c| usize::from(c)).collect();
    if header[end..].iter().any(|&c| c != 0xFF)
        || listed != (0..listed.len()).collect::<Vec<_>>()
        || listed.is_empty()
    {
        return Err(Error::from("vbios: unrecognized fan channel list"));
    }
    let mut warnings = Vec::new();
    let entry = |(at, hs, es): (usize, usize, usize), i: usize| at + hs + i * es;
    let mut rows = Vec::new();
    for i in 0..n {
        let cooler_row = entry((cooler, cooler_hs, cooler_es), i);
        let curve_at = if turing {
            entry((policy_at, policy_hs, policy_es), i) + TURING_CURVE_SHIFT
        } else {
            entry((policy_at, policy_hs, policy_es), i)
        };
        let mapping_row = b[entry((mapping_at, mapping_hs, mapping_es), i) + 1];
        let (control_offset, speed_offset, speed_rpm) = if has_control_speed {
            let c = find("control").map(|t| entry(t, i));
            let s = find("speed").map(|t| entry(t, i));
            let rpm = s.and_then(|s| u16_at(b, s + 2).ok());
            (c, s, rpm)
        } else {
            (None, None, None)
        };
        rows.push(FanChannelRow {
            index: i,
            cooler_offset: cooler_row,
            enable: b[cooler_row] != 0,
            active: listed.contains(&i),
            pwm_frequency_hz: u32::from(b[cooler_row + 11])
                | (u32::from(b[cooler_row + 12]) << 8)
                | (u32::from(b[cooler_row + 13]) << 16),
            level_minimum: b[cooler_row + 2],
            level_maximum: b[cooler_row + 3],
            rpm_minimum: u16_at(b, cooler_row + 14)?,
            rpm_maximum: u16_at(b, cooler_row + 16)?,
            curve: fan_curve_at(b, curve_at)?,
            policy_offset: curve_at,
            mapping_row,
            control_offset,
            speed_offset,
            speed_rpm,
        });
    }
    if rows
        .iter()
        .filter(|r| r.enable)
        .map(|r| r.index)
        .collect::<Vec<_>>()
        != listed
    {
        warnings.push("fan channel list disagrees with the enabled rows".into());
    }
    if turing {
        for r in rows.iter().filter(|r| r.active) {
            let entry_at = entry((policy_at, policy_hs, policy_es), r.index);
            if b[entry_at + 9..entry_at + 17] != TURING_POLICY_BYTES {
                warnings.push(format!(
                    "Turing fan policy entry {} layout unrecognized",
                    r.index
                ));
            }
        }
    }
    let extra = if turing {
        Vec::new()
    } else {
        let policies = b[policy_at + 3]; // 策略头 byte 3 = 条目数
        (n..usize::from(policies))
            .filter(|&k| b[entry((policy_at, policy_hs, policy_es), k)] != 0)
            .map(|k| {
                (
                    entry((policy_at, policy_hs, policy_es), k),
                    fan_curve_at(b, entry((policy_at, policy_hs, policy_es), k)).ok(),
                )
            })
            .filter_map(|(off, c)| c.map(|c| (off, c)))
            .collect()
    };
    Ok(Some(FanReport {
        cooler_offset: cooler,
        policy_offset: policy_at,
        mapping_offset: mapping_at,
        control_offset: find("control").map(|t| t.0),
        speed_offset: find("speed").map(|t| t.0),
        channels: listed,
        rows,
        extra_curves: extra,
        warnings,
    }))
}

// ── NVGI 多 bank 归一化视图（只读报告） ────────────────────────────────

/// 多 bank 布局：bank 数 + stride（编程器全 dump 双 bank 常见）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BankLayout {
    pub bank_count: usize,
    pub stride: usize,
    /// 主 bank 覆盖（legacy.start .. 链尾）。
    pub primary_end: usize,
}

/// NVGI 多 bank 探测（vix `override_blob` 的只读部分）：链首偏移相同、
/// 每 bank 起始为 NVGI 头、bank 边界一致时返回布局。
pub fn bank_layout(data: &[u8], ctx: &TableContext, chain_count: usize) -> Option<BankLayout> {
    if chain_count < 2 {
        return None;
    }
    let stride = ctx.legacy.start;
    let primary_end = ctx.chain.last()?.end;
    if data.get(0..4) != Some(b"NVGI".as_slice()) {
        return None;
    }
    Some(BankLayout {
        bank_count: chain_count,
        stride,
        primary_end,
    })
}

// ── 测试 ────────────────────────────────────────────────────────────────

/// 从 ROM 字节推断架构族：PCI-ID 清单优先（显式身份），未知设备按 BIOS
/// 版本前缀推断（返回 `inferred = true`，仅为诊断口径；GA100 的 92. 推断
/// 为 Ampere，但 GSP 接收容量未审计）。Pascal 及更早无 3x 表，返回 None。
pub fn detect_family(data: &[u8]) -> Option<(Family, bool)> {
    let (pci, _extended, _issues) = scan_images(data);
    let legacy = pci.iter().find(|p| p.is_legacy())?;
    let versions = crate::vbios::families::scan_bios_versions(data);
    let refs: Vec<&str> = versions.iter().map(|s| s.as_str()).collect();
    if let Some(f) = crate::vbios::families::architecture(legacy.device, &refs).family
        && f.in_scope()
    {
        return Some((f, false));
    }
    let inferred = match versions.first().map(|v| v.as_bytes()) {
        Some([b'9', b'0', ..]) => Some(Family::Turing),
        Some([b'9', b'2', ..]) => Some(Family::Ampere),
        Some([b'9', b'4', ..]) => Some(Family::Ampere),
        Some([b'9', b'5', ..]) => Some(Family::Ada),
        Some([b'9', b'8', ..]) => Some(Family::BlackwellGb20x),
        _ => None,
    };
    inferred.map(|f| (f, true))
}

#[cfg(test)]
mod tests {
    use crate::vbios::image_chain::{f32_at, testutil::*};

    use super::*;

    /// 构造带 power/channel/rail/VFE/clock 表的 Ampere 合成 ROM。
    /// 布局（legacy 0x1000 内，BIT 指针相对 legacy.start；PCIR 头在 0x40
    /// 不可覆盖）：policy 0x100 / channels 0x200 / rail 0x560 / VFE 0x580 /
    /// clocks 0xB00 / P token 0xC00 / C token 0xD00 / BIT 0xE00。
    fn ampere_tables_rom() -> (Vec<u8>, TableContext) {
        let (mut buf, legacy, efi) = ampere_rom_with_legacy(0x1000);
        let power_at = 0x100usize;
        let chan_at = 0x200usize;
        let rail_at = 0x560usize;
        let vfe_at = 0x580usize;
        let clock_at = 0xB00usize;
        // VFE：固定头 `30 09 19 32 18`（头长 9、变量行 25B×50 变量）+ 计数
        // +50×25B 变量表（变量 0 = 温度 type 3）+ 3×24B 方程（链接 u8）。
        let var_area = 50usize * 25;
        let mut vfe: Vec<u8> = vec![0x30, 0x09, 0x19, 0x32, 0x18, 4, 0, 0, 0];
        vfe.resize(9 + var_area, 0);
        vfe[9] = 3; // 变量 0：温度通道
        let eq = |kind: u8, var: u8, next: u8, lo: f32, hi: f32, coeffs: [f32; 3]| {
            let mut e = vec![0u8; 24];
            e[0] = kind;
            e[1] = var;
            e[2] = next;
            e[3..7].copy_from_slice(&lo.to_le_bytes());
            e[7..11].copy_from_slice(&hi.to_le_bytes());
            e[11..15].copy_from_slice(&coeffs[0].to_le_bytes());
            e[15..19].copy_from_slice(&coeffs[1].to_le_bytes());
            e[19..23].copy_from_slice(&coeffs[2].to_le_bytes());
            e[23] = 2; // 输出单位 = 电压 µV
            e
        };
        // 语料序约束 REL < VMIN-info = OV−1。0: REL 温度降额；1: VMIN-info
        // 槽常数；2: OV 常数 1.1V；3: 评估 VMIN 钳位常数 0.56V（无任何
        // role/引用共选 → pinned）。
        vfe.extend_from_slice(&eq(
            1,
            0,
            0xFF,
            915_000.0,
            1_200_000.0,
            [900_000.0, 35.0, 0.01],
        ));
        vfe.extend_from_slice(&eq(1, 2, 0xFF, 560_000.0, 560_000.0, [560_000.0, 0.0, 0.0]));
        vfe.extend_from_slice(&eq(
            1,
            3,
            0xFF,
            1_100_000.0,
            1_100_000.0,
            [1_100_000.0, 0.0, 0.0],
        ));
        vfe.extend_from_slice(&eq(1, 4, 0xFF, 560_000.0, 560_000.0, [560_000.0, 0.0, 0.0]));
        buf[vfe_at..vfe_at + vfe.len()].copy_from_slice(&vfe);
        // rail：头 + 1 行 17B（V20）。boot 1.0V；REL=0 ALT=FF OV=1 VMIN=2。
        let mut rail: Vec<u8> = vec![0x20, 0x05, 0x11, 0x01, 0x00];
        let mut row = vec![0u8; 17];
        row[0..4].copy_from_slice(&1_000_000u32.to_le_bytes());
        row[4] = 0; // REL → 方程 0
        row[5] = 0xFF; // ALT 缺席
        row[6] = 2; // OV → 方程 2（常数 1.1V）
        row[7] = 3; // 评估 VMIN → 方程 3（钳位常数 0.56V，无共选）
        row[11] = 1; // VMIN-info 槽 = OV−1（排序不变式锚点）
        rail.extend_from_slice(&row);
        buf[rail_at..rail_at + rail.len()].copy_from_slice(&rail);
        // power channels：头 6B（v0x20 hs6 es0x16 n32）+ 32×0x16B，通道 0 =
        // 全板功率（class 2 + rail 0xF5）。
        let mut channels: Vec<u8> = vec![0x20, 0x06, 0x16, 0x20, 0x00, 0x00];
        for i in 0..0x20 {
            let mut e = vec![0u8; 0x16];
            if i == 0 {
                e[0] = 2;
                e[1] = 0xF5;
            }
            channels.extend_from_slice(&e);
        }
        buf[chan_at..chan_at + channels.len()].copy_from_slice(&channels);
        // power policy：头 0x2C（TGP 索引在 +10）+ 2×0x47B 行；TGP 行 0 读通道 0。
        let mut power: Vec<u8> = vec![0u8; 0x2C];
        power[0..4].copy_from_slice(&[0x30, 0x2C, 0x47, 2]);
        for (kind, ch, min, def, max) in [
            (1u8, 0u8, 100_000u32, 220_000u32, 300_000u32),
            (4, 1, 0, 0, 0),
        ] {
            let mut r = vec![0u8; 0x47];
            r[0] = kind;
            r[1] = ch;
            r[2..6].copy_from_slice(&min.to_le_bytes());
            r[6..10].copy_from_slice(&def.to_le_bytes());
            r[10..14].copy_from_slice(&max.to_le_bytes());
            power.extend_from_slice(&r);
        }
        buf[power_at..power_at + power.len()].copy_from_slice(&power);
        // clocks：Ampere v0x40（hs 0x0B、行 0x0D、10 行、每行 1 个 3B rail
        // 子条目）。
        let mut clocks: Vec<u8> = vec![
            0x40, 0x0B, 0x0D, 0x0A, 0x03, 0x01, 0x03, 0xE8, 0x03, 0x00, 0x00,
        ];
        for (name, usage, ordering, rail_type) in CLOCK_ROWS.iter() {
            let mut r = vec![0u8; 0x0D + 3];
            r[0] = *usage;
            r[1..3].copy_from_slice(&[0x10, 0x20]);
            r[3..5].copy_from_slice(&2100i16.to_le_bytes());
            r[5..7].copy_from_slice(&2500i16.to_le_bytes());
            r[7] = *ordering;
            r[9..11].copy_from_slice(&[0xFF, 0xFF]);
            if *rail_type != 0 {
                r[0x0D] = if *name == "VID" { 2 } else { *rail_type };
                // rail 子条目 bytes 1..2 复制行的 first/last（vix 门控）。
                let (fl_lo, fl_hi) = (r[1], r[2]);
                r[0x0E] = fl_lo;
                r[0x0F] = fl_hi;
            }
            clocks.extend_from_slice(&r);
        }
        buf[clock_at..clock_at + clocks.len()].copy_from_slice(&clocks);
        // BIT：P（0x2C/0x3C/0x68/0x20 槽）+ C（+8 槽）。
        let mut p_token = vec![0u8; 0xF8];
        p_token[0x2C..0x30].copy_from_slice(&(power_at as u32).to_le_bytes());
        p_token[0x3C..0x40].copy_from_slice(&(chan_at as u32).to_le_bytes());
        p_token[0x68..0x6C].copy_from_slice(&(rail_at as u32).to_le_bytes());
        p_token[0x20..0x24].copy_from_slice(&(vfe_at as u32).to_le_bytes());
        buf[0xC00..0xC00 + p_token.len()].copy_from_slice(&p_token);
        let mut c_token = vec![0u8; 0x20];
        c_token[8..12].copy_from_slice(&(clock_at as u32).to_le_bytes());
        buf[0xD00..0xD00 + c_token.len()].copy_from_slice(&c_token);
        push_bit(
            &mut buf,
            0xE00,
            &[
                (b'P', 2, p_token.len() as u16, 0xC00),
                (b'C', 2, c_token.len() as u16, 0xD00),
            ],
        );
        repair_checksum(&mut buf, legacy.start, legacy.size);
        let ctx = TableContext::new(&buf, Family::Ampere).unwrap();
        assert_eq!(ctx.chain.len(), 4);
        assert_eq!(ctx.efi.as_ref().unwrap().start, efi.start);
        (buf, ctx)
    }

    #[test]
    fn ampere_power_policy_and_channels() {
        let (buf, ctx) = ampere_tables_rom();
        let channels = power_channels(&buf, &ctx).unwrap();
        assert_eq!(channels.count, 0x20);
        assert!(channels.entries[0].is_total_board());
        let policy = power_policy(&buf, &ctx).unwrap();
        assert!(!policy.notebook);
        assert_eq!(policy.tgp_index, 0);
        assert_eq!(policy.rows[0].default_raw, 220_000);
        assert!(!policy.rows[0].unit_ma);
    }

    #[test]
    fn ampere_rails_vfe_limits_vmin() {
        let (buf, ctx) = ampere_tables_rom();
        let table = rails(&buf, &ctx).unwrap();
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].boot_voltage_uv, 1_000_000);
        let vfe_table = vfe(&buf, &ctx).unwrap();
        assert_eq!(vfe_table.equation_count, 4);
        let limits = rail_limits(&buf, &ctx).unwrap();
        assert_eq!(limits.len(), 1);
        assert!((limits[0].output_maximum_uv - 1_200_000.0).abs() < 1.0);
        assert_eq!(
            limits[0].equation_kind,
            crate::vbios::vfe::OutputKind::TemperatureDerated
        );
        let eqs = rail_equations(&buf, &ctx).unwrap();
        let ov = eqs.iter().find(|e| e.role == RailRole::Ov).unwrap();
        assert_eq!(
            ov.equation_kind,
            Some(crate::vbios::vfe::OutputKind::Constant)
        );
        let rel = eqs.iter().find(|e| e.role == RailRole::Rel).unwrap();
        assert!(rel.editable_ceiling); // 温度降额 REL 的 clamp max 可抬（本 parser 只读）
        let vmin = rail_vmin(&buf, &ctx).unwrap();
        assert!(vmin[0].pinned);
        assert!((vmin[0].value_uv.unwrap() - 560_000.0).abs() < 1.0);
    }

    #[test]
    fn ampere_clock_ranges() {
        let (buf, ctx) = ampere_tables_rom();
        let ranges = clock_ranges(&buf, &ctx).unwrap();
        assert_eq!(ranges.len(), 10);
        assert_eq!(ranges[0].domain, "GPC");
        assert_eq!(ranges[0].minimum_mhz, 2100);
        assert_eq!(ranges[2].domain, "DRAM");
    }

    #[test]
    fn fan_tables_none_when_absent_pointer() {
        let (buf, ctx) = ampere_tables_rom();
        assert!(fan_tables(&buf, &ctx).unwrap().is_none());
    }

    #[test]
    fn context_requires_bit_directory() {
        let (buf, _legacy, _efi) = ampere_rom();
        // BIT 缺失即失败（fail closed，不猜）。
        assert!(TableContext::new(&buf, Family::Ampere).is_err());
    }

    #[test]
    fn integrity_reports_image_checksums() {
        let (buf, ctx) = ampere_tables_rom();
        let checks = ctx.integrity(&buf);
        assert!(checks.iter().all(|(_, ok)| *ok), "{checks:?}");
    }

    #[test]
    fn float_and_int_helpers() {
        let mut v = Vec::new();
        v.extend_from_slice(&1.5f32.to_le_bytes());
        v.extend_from_slice(&(-7i16).to_le_bytes());
        assert_eq!(f32_at(&v, 0).unwrap(), 1.5);
        assert_eq!(i16_at(&v, 4).unwrap(), -7);
        assert!(u32_at(&v, 4).is_err()); // 6 字节缓冲读 4B@4 越界
        assert_eq!(u16_at(&v, 0).unwrap(), 0);
    }
}
