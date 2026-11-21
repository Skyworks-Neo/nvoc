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

    /// 通道标称电压（bytes 2..6，u32 µV）——12V 感测族 = 12,000,000。
    pub fn voltage_uv(&self) -> Option<u32> {
        if self.bytes.len() >= 6 {
            Some(u32::from_le_bytes(
                self.bytes[2..6].try_into().expect("bounds checked"),
            ))
        } else {
            None
        }
    }

    /// 感测倍率（bytes 6..8，u16 Q4.12 定点，0x1000 = 1.0）。**只在 12V
    /// 感测族（`voltage_uv() == 12,000,000` 且 class 1/5）上作 shunt 校准
    /// 解读**：原始感测值 × scale = 实际值。实测分布——参考卡恒 1.0；
    /// 3090 NVVDD 相位输出监视出厂 4.0；shunt mod / 非标 shunt 的 12V
    /// 连接器通道出现 1.3333（4090 CC07B）与 2.0（4070S 非公）。其它
    /// class 的同位字节语义未证实（vix 观察 class-2 求和通道在该位置存
    /// 别的值），勿据此解读。
    pub fn sense_scale(&self) -> Option<f32> {
        if self.bytes.len() >= 8 {
            let raw = u16::from_le_bytes(self.bytes[6..8].try_into().expect("bounds checked"));
            Some(f32::from(raw) / 4096.0)
        } else {
            None
        }
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

    /// 监视点角色（vix tables.py 的通道字节模板匹配；空串 = 未识别）。
    /// 模板：12V 感测族共享 `?? ?? 00 1B B7 00 …` 电压前缀（u32 12V µV；
    /// **不含** bytes 6..8 的感测倍率位——非 1.0 shunt 校准会改写它，
    /// vix 模板钉死 `00 10` 恰好漏掉全部非标样本）；NVVDD 输出监视
    /// `01 01 …` 且 byte14 = 05(power)/06(current)；全板 = `02 F5`。
    pub fn role_name(&self) -> &'static str {
        let b = &self.bytes;
        if b.len() < 8 {
            return "";
        }
        let twelve_v_sense = b[2] == 0x00 && b[3] == 0x1B && b[4] == 0xB7 && b[5] == 0x00;
        if self.class() == 2 && self.rail() == 0xF5 {
            return "total-board";
        }
        if twelve_v_sense {
            match (self.class(), self.rail()) {
                (5, 0xFF) => return "pcie-slot",
                (5, 0xE0) => return "12v-input",
                // Ada 外接连接器（12V-2x6）输入感测：rail 0xF7 通道在两块
                // 非标样本上携带 ≠1.0 感测倍率（4070S x2.0、4090 CC07B
                // x1.3333），参考卡恒 1.0。命名从上下文推断。
                (5, 0xF7) => return "12v-connector",
                (1, 0x01) => {
                    return match b.get(14) {
                        Some(0x05) => "nvvdd-power",
                        Some(0x06) => "nvvdd-current",
                        _ => "nvvdd-out",
                    };
                }
                _ => {}
            }
        }
        if self.class() == 5 && connector_rail(self.rail()) {
            return "connector";
        }
        ""
    }

    /// 12V 输入通道（class 5 + rail 0xE0 + bytes 2..6 == 12V µV 前缀且无
    /// 其它参数；倍率位 bytes 6..8 不参与——非标 shunt 会改写）。
    pub fn is_12v_input(&self) -> bool {
        self.class() == 5
            && self.bytes.len() > 15
            && self.bytes[2..6] == CHANNEL_12V[..4]
            && self.rail() == 0xE0
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

// ── 电压墙（3x：REL/ALT/OV/VMIN 全 role × VFE 方程）─────────────────────
//
// 与 GB20x（gb20x::voltage_walls）同语义：rail 行 u8 选择器 → VFE 方程
// （Ada 25B u16 链 / Ampere·Turing 24B u8 链）；kind@+0、min 钳@+3、
// max 钳@+7、常数@+11、单位码@+23&0xF（2=µV）。

/// 一面 3x 电压墙（与 gb20x::Gb20xVoltageWall 同形；独立定义避免模块环）。
#[derive(Debug, Clone, PartialEq)]
pub struct VoltageWall3x {
    pub rail_name: &'static str,
    pub rail_index: usize,
    /// rel/alt/ov/vmin/vmin-info。
    pub role_name: &'static str,
    pub selector_offset: usize,
    pub selector_raw: u16,
    pub absent: bool,
    pub equation_offset: Option<usize>,
    pub equation_kind_raw: Option<u8>,
    pub clamp_uv: Option<(f32, f32)>,
    pub constant_uv: Option<f32>,
    pub tree_state: Option<&'static str>,
    pub pinned_uv: Option<f32>,
    pub minimum_offset: Option<usize>,
    pub maximum_offset: Option<usize>,
    pub constant_offset: Option<usize>,
    pub editable_ceiling: bool,
}

fn wall_role_name(role: RailRole) -> &'static str {
    match role {
        RailRole::Rel => "rel",
        RailRole::Alt => "alt",
        RailRole::Ov => "ov",
        RailRole::VminEvaluated => "vmin",
        RailRole::VminInfo => "vmin-info",
        RailRole::Boot => "boot",
    }
}

/// 全 rail × 全 role 电压墙（3x 族）。
pub fn voltage_walls(data: &[u8], ctx: &TableContext) -> Result<Vec<VoltageWall3x>, Error> {
    let table = rails(data, ctx)?;
    let vfe_table = vfe(data, ctx)?;
    let mut out = Vec::new();
    for row in &table.rows {
        for (role, offset, raw) in &row.selectors {
            let absent = *raw == 0xFFFF;
            let mut w = VoltageWall3x {
                rail_name: row.name,
                rail_index: row.index,
                role_name: wall_role_name(*role),
                selector_offset: *offset,
                selector_raw: *raw,
                absent,
                equation_offset: None,
                equation_kind_raw: None,
                clamp_uv: None,
                constant_uv: None,
                tree_state: None,
                pinned_uv: None,
                minimum_offset: None,
                maximum_offset: None,
                constant_offset: None,
                editable_ceiling: false,
            };
            if !absent {
                let at = vfe_table.equations_base + usize::from(*raw) * vfe_table.equation_size;
                if let Some(equation) = vfe_table.equation(*raw) {
                    w.equation_offset = Some(at);
                    w.equation_kind_raw = Some(equation.kind_raw);
                    // 非电压输出（单位码 != 2）：不视为墙（如 Turing 的 boot/
                    // margin 方程族），保留 kind/偏移供诊断。
                    if equation.output_unit != 2 {
                        out.push(w);
                        continue;
                    }
                    // 电压合理域门：非电压量纲却带电压单位码的奇异方程
                    // （Turing boot 1.79e-20 mV 实测）不当作墙。
                    if let Some((_, hi)) = equation.clamp
                        && !(50_000.0..=3_000_000.0).contains(&hi)
                    {
                        out.push(w);
                        continue;
                    }
                    w.clamp_uv = equation.clamp;
                    w.minimum_offset = Some(at + 3);
                    w.maximum_offset = Some(at + 7);
                    w.constant_offset = Some(at + 11);
                    w.constant_uv = crate::vbios::vfe::constant_equation_uv(equation);
                    if matches!(equation.kind_raw, 2 | 3)
                        && equation.next.is_none()
                        && let Some((lo, hi)) = equation.clamp
                        && lo.is_finite()
                        && hi.is_finite()
                    {
                        if lo == hi && (f64::from(lo) - f64::from(lo).round()).abs() < 1.0 {
                            w.tree_state = Some("pinned");
                            w.pinned_uv = Some(lo);
                        } else {
                            w.tree_state = Some("stock");
                        }
                    }
                    let allowed = match role {
                        RailRole::Rel => matches!(equation.kind_raw, 1 | 7),
                        RailRole::Alt | RailRole::Ov | RailRole::VminEvaluated => {
                            equation.kind_raw == 1
                        }
                        _ => false,
                    };
                    w.editable_ceiling =
                        allowed && equation.output_unit == 2 && w.clamp_uv.is_some();
                }
            }
            out.push(w);
        }
    }
    Ok(out)
}

/// 一面被改写的 3x 电压墙（回显）。
#[derive(Debug, Clone, PartialEq)]
pub struct VoltageWallPatch3x {
    pub rail_name: &'static str,
    pub role_name: &'static str,
    pub writes: Vec<(usize, f32)>,
    pub old_clamp_uv: Option<(f32, f32)>,
    pub old_constant_uv: Option<f32>,
}

/// 定位 3x 电压墙写入点（只读；常数墙三点 / 曲线墙 max 钳）。
pub fn locate_voltage_wall_writes(
    data: &[u8],
    ctx: &TableContext,
    role: RailRole,
    millivolts: f64,
) -> Result<Vec<VoltageWallPatch3x>, Error> {
    let table = rails(data, ctx)?;
    let vfe_table = vfe(data, ctx)?;
    let value_uv = (millivolts * 1000.0) as f32;
    if !value_uv.is_finite() || value_uv <= 0.0 {
        return Err(Error::from("vbios: voltage must be a positive number"));
    }
    let mut out = Vec::new();
    for row in &table.rows {
        let Some((_, _, sel_raw)) = row
            .selectors
            .iter()
            .find(|(r, _, _)| *r == role)
            .map(|(r, o, v)| (*r, *o, *v))
        else {
            continue;
        };
        if sel_raw == 0xFFFF {
            continue;
        }
        let at = vfe_table.equations_base + usize::from(sel_raw) * vfe_table.equation_size;
        if at + vfe_table.equation_size > data.len() {
            return Err(Error::from("vbios: rail equation outside image"));
        }
        let equation = vfe_table
            .equation(sel_raw)
            .ok_or_else(|| Error::from("vbios: selector outside equation table"))?;
        if equation.output_unit != 2 {
            return Err(Error::from("vbios: wall equation output is not voltage µV"));
        }
        let Some((lo, hi)) = equation.clamp else {
            return Err(Error::from("vbios: wall equation has no clamp"));
        };
        if !lo.is_finite() || !hi.is_finite() {
            return Err(Error::from("vbios: wall equation clamp is not finite"));
        }
        let is_constant = crate::vbios::vfe::constant_equation_uv(equation).is_some();
        let mut writes = Vec::new();
        if is_constant {
            for off in [at + 3, at + 7, at + 11] {
                writes.push((off, value_uv));
            }
        } else {
            if value_uv < lo {
                return Err(Error::from(format!(
                    "vbios: {millivolts} mV is below the wall's existing min clamp ({lo} µV)"
                )));
            }
            writes.push((at + 7, value_uv));
        }
        out.push(VoltageWallPatch3x {
            rail_name: row.name,
            role_name: wall_role_name(role),
            writes,
            old_clamp_uv: Some((lo, hi)),
            old_constant_uv: crate::vbios::vfe::constant_equation_uv(equation),
        });
    }
    if out.is_empty() {
        return Err(Error::from(format!(
            "vbios: no rail carries a {} wall on this image",
            wall_role_name(role)
        )));
    }
    Ok(out)
}

/// 就地应用（整镜像车道）：写字节 + 所在镜像加性校验归零。
pub fn patch_voltage_wall_millivolts(
    img: &mut [u8],
    ctx: &TableContext,
    role: RailRole,
    millivolts: f64,
) -> Result<Vec<VoltageWallPatch3x>, Error> {
    let patches = locate_voltage_wall_writes(img, ctx, role, millivolts)?;
    for p in &patches {
        for (off, uv) in &p.writes {
            img[*off..*off + 4].copy_from_slice(&uv.to_le_bytes());
        }
    }
    let base = crate::vbios::legacy::find_image_base(img)
        .ok_or_else(|| Error::from("vbios: 55 AA base not found for checksum repair"))?;
    let size = (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
    let end = (base + size).min(img.len());
    img[end - 1] = 0;
    let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    img[end - 1] = sum.wrapping_neg();
    Ok(patches)
}

// ── 3x surfaces（vix tables.py 剩余面：热策略/出厂时钟/PERF_CF/LPWR/
//    DRAM 点/GPC 曲线/运行时功率槽/override profiles）────────────────────
//
// 与 GB20x（gb20x_surfaces）同形；差异全在 vix 常量：
//   thermal：turing/ampere 头 0x0A、ada 0x0B，条目 0x28，≤16 条
//   factory：Turing 头 6B（10 06 05 02 05 02）、Ampere/Ada 7B（尾 00/01）
//   perf_cf：控制器 v0x10 头 6B、条目 24|28|32|49B（byte0..22 同位）
//   lpwr：gr_pg(0x80,10,5,u32) ms(0x84,7,15,u32) gr_rg(0xD4,9,1,u8)
//         difr(0xE8,11,17,u32)，≤5 条
//   dram/gpc：Ada/Ampere v0x40（Turing v0x35 未审）
//   profiles：P+0xF4 头 (10 0a 05 04 07 08 01 10 09 08)，Ada-only 非空

/// 一条 3x 热策略条目。
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalPolicyEntry3x {
    pub index: usize,
    pub offset: usize,
    pub policy_class: u8,
    pub byte1: u8,
    pub temperatures_raw: [i16; 3],
}

/// 3x 热策略表（P+0x50 v0x10；无引用 → None）。
pub fn thermal_policies(
    data: &[u8],
    ctx: &TableContext,
) -> Result<Option<Vec<ThermalPolicyEntry3x>>, Error> {
    let Ok(at) = ctx.read(data, b'P', 0x50, 16) else {
        return Ok(None);
    };
    let (ver, hs, es, count) = (
        data[at],
        usize::from(data[at + 1]),
        usize::from(data[at + 2]),
        usize::from(data[at + 3]),
    );
    let hs_want = match ctx.family {
        Family::Ada => 0x0B,
        _ => 0x0A,
    };
    if ver != 0x10 || hs != hs_want || es != 0x28 || count == 0 || count > 16 {
        return Err(Error::from("vbios: unsupported thermal policy table"));
    }
    let mut out = Vec::new();
    for i in 0..count {
        let e = at + hs + es * i;
        out.push(ThermalPolicyEntry3x {
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

/// 一条出厂时钟条目。
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryClockEntry3x {
    pub index: usize,
    pub offset: usize,
    pub flag: u8,
    pub khz: i64,
}

/// 3x 出厂时钟记录（P+0x98；头不合 → None=未审）。
#[allow(clippy::type_complexity)]
pub fn factory_clocks(
    data: &[u8],
    ctx: &TableContext,
) -> Result<Option<(Vec<FactoryClockEntry3x>, Vec<FactoryClockEntry3x>)>, Error> {
    let Ok(at) = ctx.read(data, b'P', 0x98, 16) else {
        return Ok(None);
    };
    let hs = usize::from(data[at + 1]);
    let turing = ctx.family == Family::Turing;
    let ok = if turing {
        hs == 6 && data[at..at + 6] == [0x10, 6, 5, 2, 5, 2]
    } else {
        hs == 7 && data[at..at + 6] == [0x10, 7, 5, 2, 5, 2] && matches!(data[at + 6], 0 | 1)
    };
    if !ok {
        return Ok(None); // 未审头：不解读条目
    }
    let mut offsets = Vec::new();
    let mut clocks = Vec::new();
    for i in 0..2 {
        offsets.push(FactoryClockEntry3x {
            index: i,
            offset: at + hs + 5 * i,
            flag: data[at + hs + 5 * i],
            khz: i64::from(u32_at(data, at + hs + 5 * i + 1)? as i32),
        });
        clocks.push(FactoryClockEntry3x {
            index: i,
            offset: at + hs + 10 + 5 * i,
            flag: data[at + hs + 10 + 5 * i],
            khz: i64::from(u32_at(data, at + hs + 10 + 5 * i + 1)?),
        });
    }
    Ok(Some((offsets, clocks)))
}

// ── 温度墙 / 出厂时钟 patch（3x 写面：热墙与 P0 频率的 vBIOS 源头）──

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

/// 3x 热策略温度墙写：条目 `entry_index` 的三温度（1/32 ℃ i16 LE，位于
/// entry+2/+4/+6）全部改写。定位复用 [`thermal_policies`]；写后修复镜像
/// 加性校验。**驱动节流墙源头**——TU106 样本 #0 class1 [89/89/89] = 主
/// 温度墙，#2 class1 [81/65/88] = 三阈值策略（慢降/睡眠/关断族）。
pub fn patch_thermal_policy_temps_3x(
    img: &mut [u8],
    ctx: &TableContext,
    entry_index: usize,
    temps_raw: [i16; 3],
) -> Result<ThermalTempPatch, Error> {
    let entries = thermal_policies(img, ctx)?
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
    /// false = offsets 表（i32 出厂偏移），true = clocks 表（u32 频率）。
    pub is_clock: bool,
    pub entry_index: usize,
    /// kHz 值域的镜像内偏移（旗标字节后）。
    pub offset: usize,
    pub old_khz: i64,
    pub new_khz: i64,
}

/// 3x 出厂时钟写：`clocks`（is_clock=true）/`offsets`（false）表条目
/// `entry_index`（0..2）的 kHz 改写（旗标字节保留，值域 = 旗标后 u32/i32
/// LE）。**卡面标称 boost 的源头**——TU106 样本 clocks[1] = 1650000 kHz =
/// 1650 MHz boost；服务器卡 P0 频率上限的第一序 patch 面。定位复用
/// [`factory_clocks`]（头不合 = None = 未审，拒绝盲写）。
pub fn patch_factory_clock_3x(
    img: &mut [u8],
    ctx: &TableContext,
    is_clock: bool,
    entry_index: usize,
    khz: i64,
) -> Result<FactoryClockPatch, Error> {
    let (offsets, clocks) = factory_clocks(img, ctx)?.ok_or_else(|| {
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

/// PERF_CF 控制器行（3x；字节 0..22 各条目尺寸同位）。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfController3x {
    pub index: usize,
    pub offset: usize,
    pub kind: u8,
    pub target_raw: u32,
    pub jump_raw: u32,
    pub domain: u8,
    pub hysteresis_up: u8,
    pub hysteresis_down: u8,
}

/// PERF_CF 策略行（与 GB20x 同形）。
#[derive(Debug, Clone, PartialEq)]
pub struct PerfCfPolicy3x {
    pub index: usize,
    pub offset: usize,
    pub flags: u8,
    pub kind: u8,
    pub active: bool,
    pub conflicts: u32,
    pub controllers: u32,
}

/// 3x PERF_CF 控制器（P+0xa8）+ 策略（P+0xac）。
#[allow(clippy::type_complexity)]
pub fn perf_cf(
    data: &[u8],
    ctx: &TableContext,
) -> Result<Option<(Vec<PerfCfController3x>, Vec<PerfCfPolicy3x>)>, Error> {
    let (Ok(ca), Ok(pa)) = (ctx.read(data, b'P', 0xA8, 8), ctx.read(data, b'P', 0xAC, 8)) else {
        return Ok(None);
    };
    let (ver, hs, es, count) = (
        data[ca],
        usize::from(data[ca + 1]),
        usize::from(data[ca + 2]),
        usize::from(data[ca + 3]),
    );
    if ver != 0x10 || hs != 6 || !matches!(es, 24 | 28 | 32 | 49) || count < 3 {
        return Err(Error::from("vbios: unsupported PERF_CF controller table"));
    }
    let mut controllers = Vec::new();
    for i in 0..count {
        let e = ca + hs + es * i;
        controllers.push(PerfCfController3x {
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
    let (pver, phs, pes, pcount) = (
        data[pa],
        usize::from(data[pa + 1]),
        usize::from(data[pa + 2]),
        usize::from(data[pa + 3]),
    );
    if (pver, phs, pes) != (0x10, 5, 10) || pcount == 0 || data[pa + 4] != 0 {
        return Err(Error::from("vbios: unsupported PERF_CF policy table"));
    }
    let mut policies = Vec::new();
    for i in 0..pcount {
        let e = pa + phs + pes * i;
        policies.push(PerfCfPolicy3x {
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

/// 一张 3x LPWR 特性表。
#[derive(Debug, Clone, PartialEq)]
pub struct LpwrTable3x {
    pub name: &'static str,
    pub offset: usize,
    pub entries: Vec<u64>,
}

const LPWR_TABLES_3X: [(&str, usize, usize, usize, usize); 4] = [
    ("gr_pg", 0x80, 10, 5, 4),
    ("ms", 0x84, 7, 15, 4),
    ("gr_rg", 0xD4, 9, 1, 1),
    ("difr", 0xE8, 11, 17, 4),
];

/// 3x LPWR 表族（未引用 → 不在返回里；Turing 自然只出 gr_pg/ms）。
pub fn lpwr_tables(data: &[u8], ctx: &TableContext) -> Result<Vec<LpwrTable3x>, Error> {
    let mut out = Vec::new();
    for (name, bit_off, hs, es, word) in LPWR_TABLES_3X {
        let Ok(at) = ctx.read(data, b'P', bit_off, 8) else {
            continue;
        };
        if u32_at(data, at)? == 0 {
            continue;
        }
        let count = usize::from(data[at + 3]);
        if data[at] != 0x10
            || usize::from(data[at + 1]) != hs
            || usize::from(data[at + 2]) != es
            || count == 0
            || count > 5
        {
            return Err(Error::from(format!("vbios: unsupported LPWR {name} table")));
        }
        let mut entries = Vec::new();
        for i in 0..count {
            let e = at + hs + es * i;
            entries.push(match word {
                1 => u64::from(*data.get(e).ok_or("vbios: oob")?),
                _ => u64::from(u32_at(data, e)?),
            });
        }
        out.push(LpwrTable3x {
            name,
            offset: at,
            entries,
        });
    }
    Ok(out)
}

/// 3x DRAM 点（Ada/Ampere 时钟编程 v0x40；Turing v0x35 未审）。
#[derive(Debug, Clone, PartialEq)]
pub struct DramPoint3x {
    pub index: usize,
    pub offset: usize,
    pub flags: u8,
    pub mhz: u16,
}

pub fn dram_points(data: &[u8], ctx: &TableContext) -> Result<Vec<DramPoint3x>, Error> {
    if ctx.family == Family::Turing {
        return Err(Error::from(
            "vbios: DRAM points are Ada/Ampere-only (v0x35 unreviewed)",
        ));
    }
    let ranges = clock_ranges(data, ctx)?;
    let dram = ranges
        .iter()
        .find(|r| r.domain == "DRAM")
        .ok_or("vbios: no DRAM clock row")?;
    let row = dram.minimum_offset - 3;
    let (first, last) = (usize::from(data[row + 1]), usize::from(data[row + 2]));
    let at = ctx.read(data, b'C', 0x0C, 8)?;
    if data[at] != 0x40 {
        return Err(Error::from("vbios: unsupported clock programming version"));
    }
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
    if !(first <= last && last < count && last - first >= 2) {
        return Err(Error::from("vbios: unsupported DRAM programming entries"));
    }
    let mut out = Vec::new();
    let mut prev: Option<u16> = None;
    for i in first..=last {
        let e = at + hs + stride * i;
        let flags = data[e];
        let mhz = u16_at(data, e + 1)?;
        if !matches!(flags, 0x03 | 0x13) {
            return Err(Error::from("vbios: DRAM point flag outside reviewed shape"));
        }
        if prev.is_some_and(|p| mhz <= p) {
            return Err(Error::from("vbios: DRAM points not strictly rising"));
        }
        prev = Some(mhz);
        out.push(DramPoint3x {
            index: i,
            offset: e,
            flags,
            mhz,
        });
    }
    Ok(out)
}

/// GPC 曲线偏移槽（3x；Ada 条目 15B root@+3|+13<<8、Ampere 13B root@+3）。
#[derive(Debug, Clone, PartialEq)]
pub struct GpcCurveSlot3x {
    pub root: u16,
    pub root_offset: usize,
    pub slot: Option<u16>,
    pub slot_offset: Option<usize>,
    pub current_mhz: i32,
}

pub fn gpc_curve(data: &[u8], ctx: &TableContext) -> Result<GpcCurveSlot3x, Error> {
    if ctx.family == Family::Turing {
        return Err(Error::from("vbios: GPC curve offset is Ada/Ampere-only"));
    }
    let ranges = clock_ranges(data, ctx)?;
    let gpc = ranges
        .iter()
        .find(|r| r.domain == "GPC")
        .ok_or("vbios: no GPC clock row")?;
    let row = gpc.minimum_offset - 3;
    let (first, last) = (usize::from(data[row + 1]), usize::from(data[row + 2]));
    let at = ctx.read(data, b'C', 0x0C, 8)?;
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
    if data[at] != 0x40 || hs != 8 || !matches!(size, 13 | 15) || first != last || first >= count {
        return Err(Error::from("vbios: unreviewed GPC clock programming entry"));
    }
    let root_of = |i: usize| -> Result<u16, Error> {
        let e = at + hs + stride * i;
        Ok(u16::from(data[e + 3])
            | if size == 15 {
                u16::from(data[e + 13]) << 8
            } else {
                0
            })
    };
    let root = root_of(first)?;
    for i in 0..count {
        if i != first && data[at + hs + stride * i] & 0x0F == 1 && root_of(i)? == root {
            return Err(Error::from(
                "vbios: another programming entry names the GPC root",
            ));
        }
    }
    let vfe_at = ctx.read(data, b'P', 0x20, 16)?;
    let esize = usize::from(data[vfe_at + 4]);
    let width = usize::from(esize == 25);
    let vcount = u16::from(data[vfe_at + 5])
        | if width == 2 {
            u16::from(data[vfe_at + 10]) << 8
        } else {
            0
        };
    let base = vfe_at
        + usize::from(data[vfe_at + 1])
        + usize::from(data[vfe_at + 2]) * usize::from(data[vfe_at + 3]);
    let none = ((1u32 << (8 * width)) - 1) as u16;
    let mut refs = vec![0usize; usize::from(vcount)];
    for i in 0..vcount {
        let e = data
            .get(base + esize * usize::from(i)..base + esize * usize::from(i) + esize)
            .ok_or("vbios: oob")?;
        if !matches!(e[0], 0 | 1 | 2 | 3 | 4 | 7) {
            return Err(Error::from("vbios: unreviewed VFE equation kind"));
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
        let mut links = vec![(u16::from(e[2]) | high(2))];
        match e[0] {
            2 => links.extend([u16::from(e[11]) | high(11), u16::from(e[12]) | high(12)]),
            3 => {
                links.extend([u16::from(e[15]) | high(15), u16::from(e[16]) | high(16)]);
                if e[23] & 0x10 != 0 {
                    links.push(u16::from(e[11]) | u16::from(e[12]) << 8);
                }
            }
            _ => {}
        }
        for x in links {
            if x != none
                && let Some(slot) = refs.get_mut(usize::from(x))
            {
                *slot += 1;
            }
        }
    }
    if usize::from(root) >= usize::from(vcount) {
        return Err(Error::from("vbios: GPC root outside the VFE table"));
    }
    let rat = base + esize * usize::from(root);
    let e = &data[rat..rat + esize];
    if e[0] != 3 || e[23] & 0x0F != 1 || refs[usize::from(root)] != 0 {
        return Err(Error::from("vbios: unreviewed GPC root equation"));
    }
    let nxt = u16::from(e[2])
        | if width == 2 {
            u16::from(e[esize - 1]) << 8
        } else {
            0
        };
    let (slot, current) = if nxt == none {
        let slot = (0..vcount).find(|&i| data[base + esize * usize::from(i)] == 0);
        (slot, 0)
    } else {
        let s = base + esize * usize::from(nxt);
        let value = f32::from_le_bytes(
            data.get(s + 11..s + 15)
                .ok_or("vbios: oob")?
                .try_into()
                .unwrap(),
        );
        if refs[usize::from(nxt)] != 1
            || !value.is_finite()
            || value != value.trunc()
            || value.abs() > 500.0
        {
            return Err(Error::from(
                "vbios: GPC root links non-offset-shape equation",
            ));
        }
        (Some(nxt), value as i32)
    };
    Ok(GpcCurveSlot3x {
        root,
        root_offset: rat,
        slot,
        slot_offset: slot.map(|x| base + esize * usize::from(x)),
        current_mhz: current,
    })
}

/// 3x 运行时功率限开关槽（turing/ampere 笔记本；P 头 +10）。
pub fn runtime_limit_slot(
    data: &[u8],
    ctx: &TableContext,
) -> Result<Option<(usize, usize, u8)>, Error> {
    if ctx.family == Family::Ada {
        return Ok(None);
    }
    let policy = power_policy(data, ctx)?;
    if !policy.notebook {
        return Ok(None);
    }
    // TGP 行必须是首个 class-1 行（R525 语义）。
    let rows = &policy.rows;
    if policy.tgp_index >= rows.len() || rows[policy.tgp_index].kind != 1 {
        return Ok(None);
    }
    if rows.iter().take(policy.tgp_index).any(|r| r.kind == 1) {
        return Ok(None);
    }
    let at = ctx.read(data, b'P', 0x2C, 16)? + 10;
    Ok(Some((at, policy.tgp_index, u8::from(data[at] != 0xFF))))
}

/// 一条 3x override profile 条目。
#[derive(Debug, Clone, PartialEq)]
pub struct PowerProfileEntry3x {
    pub policy: usize,
    /// "maximum" | "default"。
    pub field: &'static str,
    pub value_raw: u32,
    pub value_offset: usize,
}

/// 3x override profile（P+0xF4；非空仅 Ada 已审，其余族非空报错）。
pub fn power_profiles(
    data: &[u8],
    ctx: &TableContext,
) -> Result<Vec<Vec<PowerProfileEntry3x>>, Error> {
    let entry = ctx.bit.token(b'P').ok_or("vbios: no BIT P token")?;
    if usize::from(entry.size) < 0xF8 {
        return Ok(Vec::new());
    }
    let at = ctx.read(data, b'P', 0xF4, 16)?;
    if data[at..at + 10] != [0x10, 0x0a, 0x05, 0x04, 0x07, 0x08, 0x01, 0x10, 0x09, 0x08] {
        return Err(Error::from("vbios: unvalidated power profile header"));
    }
    if !data[at + 0xFE..at + 0xFE + 16 * 0x49]
        .iter()
        .any(|&b| b != 0)
        && ctx.family != Family::Ada
    {
        return Ok(Vec::new());
    }
    let count = power_policy(data, ctx)?.rows.len();
    let mut out = Vec::new();
    for i in 0..16usize {
        let row = at + 0xFE + i * 0x49;
        if !data[row..row + 0x49].iter().any(|&b| b != 0) {
            continue;
        }
        if data[row] != 1 {
            return Err(Error::from("vbios: unsupported override profile flag"));
        }
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut ended = false;
        for j in 0..8usize {
            let e = row + 1 + j * 9;
            if !data[e..e + 9].iter().any(|&b| b != 0) {
                ended = true;
                continue;
            }
            let selector = u32_at(data, e + 1)?;
            let value = u32_at(data, e + 5)?;
            let (policy, field) = ((selector & 0xFF) as usize, (selector >> 8) as usize);
            let field_name = match field {
                0 => "maximum",
                1 => "default",
                _ => {
                    return Err(Error::from("vbios: unsupported override profile field"));
                }
            };
            if ended || data[e] != 1 || policy >= count || !seen.insert((policy, field)) {
                return Err(Error::from("vbios: unsupported override profile entry"));
            }
            entries.push(PowerProfileEntry3x {
                policy,
                field: field_name,
                value_raw: value,
                value_offset: e + 5,
            });
        }
        if entries.is_empty() {
            return Err(Error::from("vbios: override profile without entries"));
        }
        out.push(entries);
    }
    Ok(out)
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
    fn thermal_and_factory_patch_roundtrip() {
        // 真镜像（TU106 Turing）端到端：定位→改写→重解析→邻字节/校验和。
        let Ok(d0) = std::fs::read("../1.rom") else {
            eprintln!("skip: 1.rom not present");
            return;
        };
        let Some((family, _)) = detect_family(&d0) else {
            panic!("1.rom: family undetected");
        };
        let ctx = TableContext::new(&d0, family).expect("ctx");
        let entries = thermal_policies(&d0, &ctx).expect("ok").expect("present");
        let e0 = entries
            .iter()
            .find(|e| e.index == 0)
            .expect("thermal entry 0");
        if e0.temperatures_raw == [0, 0, 0] {
            eprintln!("skip: entry 0 has no populated temps on this image");
            return;
        }

        // 温度墙：entry 0 三点 → 95 ℃（1/32 ℃ = 3040）。
        let mut img = d0.clone();
        let patch = patch_thermal_policy_temps_3x(&mut img, &ctx, 0, [95 * 32; 3]).expect("patch");
        assert_eq!(patch.writes.len(), 3);
        let after_ctx = TableContext::new(&img, family).expect("ctx after");
        let after = thermal_policies(&img, &after_ctx)
            .expect("ok")
            .expect("present");
        let a0 = after
            .iter()
            .find(|e| e.index == 0)
            .expect("thermal entry 0 after");
        assert_eq!(a0.temperatures_raw, [95 * 32; 3]);
        // 其余条目逐值不动。
        for (b, a) in entries.iter().zip(after.iter()) {
            if b.index != 0 {
                assert_eq!(b.temperatures_raw, a.temperatures_raw);
            }
        }

        // 出厂时钟：clocks[1] → 2100 MHz；clocks[0]/offsets 不动。
        let (offs_before, cls_before) = factory_clocks(&img, &after_ctx)
            .expect("ok")
            .expect("present");
        let target_idx = cls_before
            .iter()
            .position(|e| e.khz > 0)
            .expect("a populated clock entry");
        let old_khz = cls_before[target_idx].khz;
        let fpatch = patch_factory_clock_3x(
            &mut img,
            &after_ctx,
            true,
            cls_before[target_idx].index,
            2_100_000,
        )
        .expect("factory patch");
        assert_eq!(fpatch.old_khz, old_khz);
        let (offs_after, cls_after) = factory_clocks(&img, &after_ctx)
            .expect("ok")
            .expect("present");
        assert_eq!(cls_after[target_idx].khz, 2_100_000);
        for (b, a) in cls_before.iter().zip(cls_after.iter()) {
            if b.index != cls_before[target_idx].index {
                assert_eq!(b.khz, a.khz);
            }
        }
        for (b, a) in offs_before.iter().zip(offs_after.iter()) {
            assert_eq!(b.khz, a.khz);
        }

        // 镜像加性校验归零。
        let base = crate::vbios::legacy::find_image_base(&img).expect("base");
        let size =
            (u32::from(u16::from(img[base + 2]) | u16::from(img[base + 3]) << 8) as usize) * 512;
        let end = (base + size).min(img.len());
        let sum = img[base..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        assert_eq!(sum, 0, "image checksum must stay zero after patch");
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
