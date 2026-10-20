//! PCI/NPDS 镜像链扫描与 BIT 目录（vix-editor `rom/structure.py` 的 Rust
//! 移植，read-only）。为 Turing/Ampere/Ada/GB20x 的表解码提供容器层：
//!
//! - [`scan_images`]：512B 步进枚举 `55 AA`（PCIR）与 `VN`（NPDS）镜像头，
//!   边界全部来自头字段（image length ×512），配 overlap/truncation 诊断。
//!   这补上了 vbios::legacy"多镜像只解码第一份"的缺口。
//! - [`npde_offset`]：NPDS 之后的 NPDE 扩展头（16B 对齐），承载 last-image
//!   位（byte 0x0A bit7）——Turing 链尾判定依赖它。
//! - [`find_bit_directory`]：严格 BIT 解析（头字节和校验 = 0、token 不重号、
//!   token 数据必须落在 legacy 镜像内、全文恰一个 BIT）。
//! - [`image_chains`]：legacy → EFI(3) → E0(NPDS) 连续链组装，含 Ampere/
//!   Turing 无 EFI 变体（legacy 直接标 last 后接数据镜像）、GA10x 共享
//!   NPDS 芯片 ID 0x2200、Turing 0x70（NBSI）链尾特例。
//! - [`bit_pointer`]：BIT 指针 → 绝对偏移；GSP 逻辑视图 = legacy 后跳过
//!   EFI 直接接 E0 数据镜像（`ptr ≥ legacy.size` 时 `+ efi.size`）。
//!
//! 证据源：vix-editor（60+ Ada / 35 Ampere / 25 Turing 语料校验）；nvgpu
//! bios.h 的 3x power/1x rail 前缀。所有解码 fail closed——形状不认识的
//! 结构报错而不是猜。

use std::collections::HashMap;

use crate::error::Error;
use crate::vbios::families::Family;

// ── 基础读取助手 ────────────────────────────────────────────────────────

pub(crate) fn u16_at(data: &[u8], at: usize) -> Result<u16, Error> {
    let b = data
        .get(at..at + 2)
        .ok_or_else(|| Error::Custom(format!("vbios: read outside image at {at:#x} (2 bytes)")))?;
    Ok(u16::from(b[0]) | (u16::from(b[1]) << 8))
}

pub(crate) fn u32_at(data: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from(u16_at(data, at)?) | (u32::from(u16_at(data, at + 2)?) << 16))
}

pub(crate) fn i16_at(data: &[u8], at: usize) -> Result<i16, Error> {
    Ok(u16_at(data, at)? as i16)
}

pub(crate) fn f32_at(data: &[u8], at: usize) -> Result<f32, Error> {
    Ok(f32::from_bits(u32_at(data, at)?))
}

// ── 镜像枚举 ────────────────────────────────────────────────────────────

/// 镜像头种类：`PCIR`（55 AA 传统 Option ROM 头）或 `NPDS`（`VN` 签名的
/// NVIDIA 扩展数据镜像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Pcir,
    Npds,
}

impl ImageKind {
    pub fn magic(self) -> &'static [u8; 4] {
        match self {
            ImageKind::Pcir => b"PCIR",
            ImageKind::Npds => b"NPDS",
        }
    }
}

/// 一张枚举出的镜像。`end` 为排他边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomImage {
    pub start: usize,
    pub end: usize,
    /// 头声明的长度（512 的倍数）。
    pub size: usize,
    pub vendor: u16,
    pub device: u16,
    /// PCIR code type（0=x86 legacy、3=EFI）或 NPDS 类型字节
    /// （0xE0=数据镜像、0x70=NBSI）。
    pub image_type: u8,
    /// last-image 旗标（PCIR/NPDS byte 0x15 bit7）。
    pub last: bool,
    pub kind: ImageKind,
    /// PCIR/NPDS 头的绝对偏移。
    pub header: usize,
}

impl RomImage {
    pub fn is_legacy(&self) -> bool {
        self.kind == ImageKind::Pcir && self.image_type == 0
    }

    pub fn is_efi(&self) -> bool {
        self.kind == ImageKind::Pcir && self.image_type == 3
    }

    /// 含 NPDS 头的扩展数据镜像（type 0xE0 或 0x70）。
    pub fn is_npds(&self) -> bool {
        self.kind == ImageKind::Npds
    }
}

/// 扫描诊断（vix issues）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageIssueCode {
    InvalidImageHeader,
    TruncatedImage,
    OverlappingImages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageIssue {
    pub code: ImageIssueCode,
    pub offset: usize,
}

impl ImageIssue {
    pub fn code_name(&self) -> &'static str {
        match self.code {
            ImageIssueCode::InvalidImageHeader => "INVALID_IMAGE_HEADER",
            ImageIssueCode::TruncatedImage => "TRUNCATED_IMAGE",
            ImageIssueCode::OverlappingImages => "OVERLAPPING_IMAGES",
        }
    }
}

/// 512B 步进枚举全部 PCIR/NPDS 镜像。签名样字节若后随的不是对应 magic
/// 则视为载荷内容跳过。返回 (pci 镜像, npds 扩展镜像, 诊断)。
pub fn scan_images(data: &[u8]) -> (Vec<RomImage>, Vec<RomImage>, Vec<ImageIssue>) {
    let mut pci = Vec::new();
    let mut extended = Vec::new();
    let mut issues = Vec::new();
    if data.len() < 0x1A {
        return (pci, extended, issues);
    }
    for start in (0..data.len() - 0x1A + 1).step_by(512) {
        let sig = &data[start..start + 2];
        let (kind, magic) = match sig {
            [0x55, 0xAA] => (ImageKind::Pcir, b"PCIR"),
            [b'V', b'N'] => (ImageKind::Npds, b"NPDS"),
            _ => continue,
        };
        let header = match u16_at(data, start + 0x18) {
            Ok(h) => start + h as usize,
            Err(_) => continue,
        };
        if data.get(header..header + 4) != Some(magic.as_slice()) {
            // 载荷里长得像签名的字节不构成镜像。
            continue;
        }
        if header < start + 0x1A || header + 0x18 > data.len() {
            issues.push(ImageIssue {
                code: ImageIssueCode::InvalidImageHeader,
                offset: start,
            });
            continue;
        }
        // PCIR/NPDS 布局：0=签名、4=vendor、6=device、0x0A=头长、
        // 0x10=镜像长度(×512B)、0x14=code type、0x15=flags(bit7=last)。
        let vendor = u16_at(data, header + 4).unwrap_or(0);
        let device = u16_at(data, header + 6).unwrap_or(0);
        let header_size = u16_at(data, header + 0x0A).unwrap_or(0);
        let size = usize::from(u16_at(data, header + 0x10).unwrap_or(0)) * 512;
        let end = start + size;
        if usize::from(header_size) < 0x18
            || size == 0
            || end > data.len()
            || header + usize::from(header_size) > end
        {
            issues.push(ImageIssue {
                code: ImageIssueCode::TruncatedImage,
                offset: start,
            });
            continue;
        }
        let entry = RomImage {
            start,
            end,
            size,
            vendor,
            device,
            image_type: data[header + 0x14],
            last: data[header + 0x15] & 0x80 != 0,
            kind,
            header,
        };
        if kind == ImageKind::Pcir {
            pci.push(entry);
        } else {
            extended.push(entry);
        }
    }
    let mut ordered: Vec<&RomImage> = pci.iter().chain(extended.iter()).collect();
    ordered.sort_by_key(|p| p.start);
    for (a, b) in ordered.iter().zip(ordered.iter().skip(1)) {
        if a.end > b.start {
            issues.push(ImageIssue {
                code: ImageIssueCode::OverlappingImages,
                offset: b.start,
            });
        }
    }
    (pci, extended, issues)
}

/// NPDS 之后的 NPDE 扩展头偏移（16B 对齐），头合法且含 last-image 字节时
/// 才返回 Some。NPDE 布局：0='NPDE'、4=版本(0x100/0x101)、6=头长(≥0x0B)、
/// 0x0A=flags（bit7 = last）。
pub fn npde_offset(data: &[u8], image: &RomImage) -> Option<usize> {
    let header = image.header;
    let at = (header + usize::from(u16_at(data, header + 0x0A).ok()?) + 0xF) & !0xF;
    if data.get(at..at + 4) != Some(b"NPDE".as_slice())
        || at + 0x0B > image.end
        || !matches!(u16_at(data, at + 4).ok()?, 0x100 | 0x101)
        || u16_at(data, at + 6).ok()? < 0x0B
    {
        return None;
    }
    Some(at)
}

// ── BIT 目录（严格版） ──────────────────────────────────────────────────

/// BIT 单 token：`(id, version, size, pointer)`，`data` 为绝对偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitToken {
    pub id: u8,
    pub version: u8,
    pub size: u16,
    /// token 数据的绝对偏移（legacy.start + pointer）。
    pub data: usize,
}

/// 一个 BIT 目录：头偏移 + token 表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitDirectory {
    pub offset: usize,
    pub tokens: Vec<BitToken>,
}

impl BitDirectory {
    pub fn token(&self, id: u8) -> Option<&BitToken> {
        self.tokens.iter().find(|t| t.id == id)
    }
}

/// 在 legacy 镜像内定位**唯一** BIT 目录并解析 token。头布局（vix，60+ 语
/// 料校验）：签名 6B `FF B8 "BIT\0"`、+8 头长、+9 token 长、+10 token 数；
/// 头字节和 mod 256 必须为 0。token 6B `(id, ver, len:u16, ptr:u16)`，
/// `ptr` 为 legacy 相对。
pub fn find_bit_directory(data: &[u8], legacy: &RomImage) -> Result<BitDirectory, Error> {
    const SIG: [u8; 6] = [0xFF, 0xB8, b'B', b'I', b'T', 0x00];
    let range = data
        .get(legacy.start..legacy.end)
        .ok_or_else(|| Error::from("vbios: legacy image out of bounds"))?;
    let mut candidates = Vec::new();
    let mut at = 0usize;
    while let Some(pos) = range[at..]
        .windows(SIG.len())
        .position(|w| w == SIG)
        .map(|p| at + p)
    {
        at = pos + 1;
        let abs = legacy.start + pos;
        if abs + 12 > legacy.end {
            return Err(Error::from("vbios: truncated BIT header"));
        }
        let hs = usize::from(data[abs + 8]);
        let ts = usize::from(data[abs + 9]);
        let count = usize::from(data[abs + 10]);
        if hs < 12 || ts < 6 || count == 0 || abs + hs + ts * count > legacy.end {
            return Err(Error::from("vbios: invalid BIT dimensions"));
        }
        // 头字节和 mod 256（vix：sum(...) & 255）。
        if data[abs..abs + hs]
            .iter()
            .fold(0u8, |a, &x| a.wrapping_add(x))
            != 0
        {
            return Err(Error::from("vbios: BIT header checksum failed"));
        }
        let mut tokens = Vec::with_capacity(count);
        for i in 0..count {
            let t = abs + hs + i * ts;
            let id = data[t];
            let version = data[t + 1];
            let size = u16_at(data, t + 2)?;
            let pointer = u16_at(data, t + 4)?;
            if tokens.iter().any(|e: &BitToken| e.id == id) {
                return Err(Error::from("vbios: duplicate BIT token ID"));
            }
            if u32::from(pointer) + u32::from(size) > legacy.size as u32 {
                return Err(Error::from("vbios: BIT token points outside legacy image"));
            }
            tokens.push(BitToken {
                id,
                version,
                size,
                data: legacy.start + usize::from(pointer),
            });
        }
        candidates.push(BitDirectory {
            offset: abs,
            tokens,
        });
    }
    if candidates.len() != 1 {
        return Err(Error::Custom(format!(
            "vbios: expected one BIT, found {}",
            candidates.len()
        )));
    }
    Ok(candidates.remove(0))
}

/// BIT 指针字段 → 绝对偏移。GSP 逻辑视图：legacy 镜像后跳过 EFI 直接接
/// E0 数据镜像，故 `ptr ≥ legacy.size` 时加 `efi.size`（vix TableRom.
/// pointer / BlackwellRom.pointer）。`require_v2` 用于 'P'/'C' 指针 token。
pub fn bit_pointer(
    data: &[u8],
    bit: &BitDirectory,
    legacy: &RomImage,
    efi: Option<&RomImage>,
    token: u8,
    offset: usize,
    require_v2: bool,
) -> Result<usize, Error> {
    let t = bit
        .token(token)
        .ok_or_else(|| Error::Custom(format!("vbios: BIT token {:?} is absent", token as char)))?;
    if require_v2 && matches!(token, b'P' | b'C') && t.version != 2 {
        return Err(Error::from("vbios: unsupported BIT pointer-token version"));
    }
    if offset + 4 > usize::from(t.size) {
        return Err(Error::from("vbios: BIT pointer is outside token"));
    }
    let ptr = u32_at(data, t.data + offset)?;
    if ptr == 0 {
        return Err(Error::from("vbios: requested table is absent"));
    }
    let mut at = legacy.start + ptr as usize;
    if ptr as usize >= legacy.size {
        at += efi.map_or(0, |e| e.size);
    }
    Ok(at)
}

// ── 链组装 ──────────────────────────────────────────────────────────────

/// 组装 legacy → [+ EFI] → E0(NPDS) 连续链。family 决定变体规则：
///
/// - 通用：legacy 必须紧跟 type-3 EFI 镜像，EFI PCI 身份允许 (0,0)（通用
///   GOP 未打身份）/ (0x10DE,0) / (0x10DE, legacy.device)。
/// - Ampere/Turing 额外接受无 EFI 链：legacy 自带 last 旗标后直接接 E0。
///   Ampere 的 NPDS 可统一盖 GA102 基 ID 0x2200（GA102/104/106 共用），
///   且链上所有数据镜像 device 必须一致。
/// - Turing：链尾 E0 不带 last 旗标，由其后 type-0x70（NBSI）镜像收尾；
///   若 E0 的 NPDE 自带 last 位则链被拒绝（两种读法并存即拒绝）。
pub fn image_chains(
    data: &[u8],
    pci: &[RomImage],
    extended: &[RomImage],
    family: Family,
) -> Result<Vec<Vec<RomImage>>, Error> {
    let all: Vec<RomImage> = pci.iter().chain(extended.iter()).cloned().collect();
    let by_start: HashMap<usize, usize> =
        all.iter().enumerate().map(|(i, p)| (p.start, i)).collect();
    let mut chains = Vec::new();
    for legacy in pci
        .iter()
        .filter(|p| p.vendor == 0x10DE && p.image_type == 0)
    {
        let efi = by_start.get(&legacy.end).map(|&i| &all[i]);
        let mut images: Vec<RomImage>;
        let mut cursor;
        if matches!(family, Family::Ampere | Family::Turing)
            && legacy.last
            && efi.is_some_and(|e| e.image_type == 0xE0 && e.is_npds())
        {
            images = vec![legacy.clone()];
            cursor = legacy.end;
        } else {
            let e =
                efi.ok_or_else(|| Error::from("vbios: legacy image lacks a contiguous EFI image"))?;
            if !e.is_efi() || e.kind != ImageKind::Pcir {
                return Err(Error::from(
                    "vbios: legacy image lacks a contiguous EFI image",
                ));
            }
            let efi_identity_ok = match (e.vendor, e.device) {
                (0, 0) => true,
                (0x10DE, 0) => true,
                (0x10DE, d) => d == legacy.device,
                _ => false,
            };
            if !efi_identity_ok {
                return Err(Error::from(
                    "vbios: EFI image PCI identity disagrees with legacy image",
                ));
            }
            images = vec![legacy.clone(), e.clone()];
            cursor = e.end;
        }
        let first = images.len();
        let mut chip: Vec<u16> = vec![legacy.device & 0xFF80];
        if family == Family::Ampere {
            chip.push(0x2200);
        }
        while let Some(&i) = by_start.get(&cursor) {
            let image = &all[i];
            if !(image.image_type == 0xE0 && image.is_npds() && image.vendor == 0x10DE) {
                break;
            }
            if !chip.contains(&(image.device & 0xFF80))
                || (family == Family::Ampere
                    && images.len() > first
                    && image.device != images[first].device)
            {
                return Err(Error::from(
                    "vbios: NPDS chip identity disagrees with legacy image",
                ));
            }
            cursor = image.end;
            let last = image.last;
            images.push(image.clone());
            if last {
                break;
            }
        }
        if family == Family::Turing
            && images.len() > first
            && !images.last().is_some_and(|p| p.last)
            && let Some(&ti) = by_start.get(&cursor)
        {
            let tail = &all[ti];
            if tail.image_type == 0x70
                && tail.is_npds()
                && tail.vendor == 0x10DE
                && tail.last
                && (tail.device == 0 || chip.contains(&(tail.device & 0xFF80)))
            {
                // 读 NPDE last 位的 loader 会在数据镜像处提前结束链：
                // 两种读法并存的镜像直接拒绝，而不是选一种。
                match images.last().map(|p| npde_offset(data, p)) {
                    Some(Some(at)) if data[at + 0x0A] & 0x80 == 0 => {
                        images.push(tail.clone());
                    }
                    _ => {
                        return Err(Error::from(
                            "vbios: Turing chain: last E0 image carries an NPDE last-image byte",
                        ));
                    }
                }
            }
        }
        if images.len() == first || !images.last().is_some_and(|p| p.last) {
            return Err(Error::from(
                "vbios: NPDS data chain lacks a complete last image",
            ));
        }
        chains.push(images);
    }
    Ok(chains)
}

// ── 合成镜像测试助手（同仓库其它模块的测试共用） ────────────────────────

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// 在 buf 末尾追加一张镜像：签名 + 载荷 + PCIR/NPDS 头 + 校验字节修复。
    /// 头置于 +0x40（vix 要求 header ≥ start+0x1A）。返回 (start, end, header)。
    pub fn push_image(
        buf: &mut Vec<u8>,
        sig: [u8; 2],
        image_type: u8,
        device: u16,
        last: bool,
        header_size: u16,
        npde_last: Option<bool>,
    ) -> (usize, usize, usize) {
        debug_assert!(
            buf.len().is_multiple_of(512),
            "images must start 512-aligned"
        );
        let start = buf.len();
        // 镜像长度 = 头位 0x40 + 头长 + NPDE（如有），向上取整到 512。
        let raw = 0x40 + usize::from(header_size) + usize::from(npde_last.is_some()) * 0x10;
        let size = raw.div_ceil(512) * 512;
        buf.resize(start + size, 0xFF);
        buf[start..start + 2].copy_from_slice(&sig);
        let header = start + 0x40;
        buf[start + 0x18..start + 0x1A].copy_from_slice(&0x40u16.to_le_bytes());
        buf[header..header + 4].copy_from_slice(if sig == [0x55, 0xAA] {
            b"PCIR".as_slice()
        } else {
            b"NPDS".as_slice()
        });
        buf[header + 4..header + 6].copy_from_slice(&0x10DEu16.to_le_bytes());
        buf[header + 6..header + 8].copy_from_slice(&device.to_le_bytes());
        buf[header + 0x0A..header + 0x0C].copy_from_slice(&header_size.to_le_bytes());
        buf[header + 0x10..header + 0x12].copy_from_slice(&((size / 512) as u16).to_le_bytes());
        buf[header + 0x14] = image_type;
        buf[header + 0x15] = if last { 0x80 } else { 0x00 };
        if let Some(nl) = npde_last {
            // NPDE 紧跟 NPDS 头，16B 对齐。
            let at = (header + usize::from(header_size) + 0xF) & !0xF;
            buf[at..at + 4].copy_from_slice(b"NPDE");
            buf[at + 4..at + 6].copy_from_slice(&0x101u16.to_le_bytes());
            buf[at + 6..at + 8].copy_from_slice(&0x0Cu16.to_le_bytes());
            buf[at + 0x0A] = if nl { 0x80 } else { 0x00 };
        }
        repair_checksum(buf, start, size);
        (start, start + size, header)
    }

    /// 加性校验修复：镜像最后一个字节 = 使整镜像字节和 ≡ 0 (mod 256)。
    pub fn repair_checksum(buf: &mut [u8], start: usize, size: usize) {
        let end = start + size;
        buf[end - 1] = 0;
        let sum = buf[start..end].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        buf[end - 1] = sum.wrapping_neg();
    }

    /// 在 buf 的 `at`（512 对齐）处写入 BIT：12B 头（6B 签名 `FF B8 BIT\0` +
    /// 2B 保留 + hs/ts/count/校验和，头字节和 ≡ 0）+ 6B×n token。空间须已
    /// 预留（0xFF 区），不移动任何既有字节。
    pub fn push_bit(buf: &mut [u8], at: usize, tokens: &[(u8, u8, u16, u16)]) -> usize {
        debug_assert!(at.is_multiple_of(512));
        let header = 12usize;
        let total = header + 6 * tokens.len();
        assert!(at + total <= buf.len(), "BIT space not reserved");
        buf[at..at + 6].copy_from_slice(&[0xFF, 0xB8, b'B', b'I', b'T', 0x00]);
        buf[at + 8] = header as u8;
        buf[at + 9] = 6;
        buf[at + 10] = tokens.len() as u8;
        for (i, (id, ver, size, ptr)) in tokens.iter().enumerate() {
            let t = at + header + i * 6;
            buf[t] = *id;
            buf[t + 1] = *ver;
            buf[t + 2..t + 4].copy_from_slice(&size.to_le_bytes());
            buf[t + 4..t + 6].copy_from_slice(&ptr.to_le_bytes());
        }
        // 头字节和 ≡ 0：余量补在 +11 校验位。
        let sum: u8 = buf[at..at + header - 1]
            .iter()
            .fold(0u8, |a, &x| a.wrapping_add(x));
        buf[at + header - 1] = sum.wrapping_neg();
        assert_eq!(
            buf[at..at + header]
                .iter()
                .fold(0u8, |a, &x| a.wrapping_add(x)),
            0
        );
        at
    }

    /// 构造最小 Ampere 链：legacy(0x2204, 0x1000B) + EFI(0000:0000) +
    /// E0×2（last）。返回 (buf, legacy_img, efi_img)。legacy 内部 0xFF 填充，
    /// 供测试写入 BIT/表。
    pub fn ampere_rom() -> (Vec<u8>, RomImage, RomImage) {
        ampere_rom_with_legacy(0x1000)
    }

    pub fn ampere_rom_with_legacy(legacy_size: usize) -> (Vec<u8>, RomImage, RomImage) {
        assert!(legacy_size >= 0x100 && legacy_size.is_multiple_of(512));
        let mut buf = vec![0xFFu8; legacy_size];
        // legacy 镜像头（PCIR 置于 +0x40）。
        buf[0..2].copy_from_slice(&[0x55, 0xAA]);
        buf[0x18..0x1A].copy_from_slice(&0x40u16.to_le_bytes());
        let h = 0x40;
        buf[h..h + 4].copy_from_slice(b"PCIR");
        buf[h + 4..h + 6].copy_from_slice(&0x10DEu16.to_le_bytes());
        buf[h + 6..h + 8].copy_from_slice(&0x2204u16.to_le_bytes());
        buf[h + 0x0A..h + 0x0C].copy_from_slice(&0x18u16.to_le_bytes());
        buf[h + 0x10..h + 0x12].copy_from_slice(&((legacy_size / 512) as u16).to_le_bytes());
        buf[h + 0x14] = 0;
        repair_checksum(&mut buf, 0, legacy_size);
        // EFI + 两张 E0 数据镜像。
        let es = push_image(&mut buf, [0x55, 0xAA], 3, 0, false, 0x18, None).0;
        let (d1s, _d1e, _d1h) = push_image(&mut buf, [b'V', b'N'], 0xE0, 0x2200, false, 0x18, None);
        let (_d2s, _d2e, _d2h) = push_image(&mut buf, [b'V', b'N'], 0xE0, 0x2200, true, 0x18, None);
        let (pci, extended, issues) = scan_images(&buf);
        assert!(issues.is_empty(), "{issues:?}");
        let legacy = pci.iter().find(|p| p.start == 0).unwrap().clone();
        let efi = pci.iter().find(|p| p.start == es).unwrap().clone();
        assert_eq!(legacy.size, legacy_size);
        assert_eq!(extended.len(), 2);
        assert_eq!(extended[0].start, d1s);
        let chains = image_chains(&buf, &pci, &extended, Family::Ampere).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), 4);
        (buf, legacy, efi)
    }
}

// ── 测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn scan_finds_all_images_and_rejects_overlap() {
        let (buf, legacy, efi) = ampere_rom();
        let (pci, extended, issues) = scan_images(&buf);
        assert!(issues.is_empty());
        assert_eq!(pci.len(), 2);
        assert_eq!(extended.len(), 2);
        assert!(legacy.is_legacy());
        assert!(efi.is_efi());
        assert_eq!(extended[1].device, 0x2200);
        assert!(extended[1].last);
        // 人为制造 overlap：把第一张镜像声明得越过第二张的起点。
        let mut bad = buf.clone();
        let h = legacy.header;
        let nine = 9u16.to_le_bytes(); // 9×512 = 0x1200 > EFI 镜像起点
        bad[h + 0x10..h + 0x12].copy_from_slice(&nine);
        let (_, _, issues) = scan_images(&bad);
        assert!(
            issues
                .iter()
                .any(|i| i.code == ImageIssueCode::OverlappingImages),
            "{issues:?}"
        );
    }

    #[test]
    fn bit_directory_strict_parsing() {
        let (mut buf, legacy, _efi) = ampere_rom_with_legacy(0x1000);
        let bit = push_bit(
            &mut buf,
            0x800,
            &[(b'P', 2, 0x20, 0x40), (b'C', 2, 0x18, 0x60)],
        );
        repair_checksum(&mut buf, legacy.start, legacy.size);
        let img = scan_images(&buf)
            .0
            .into_iter()
            .find(|p| p.start == legacy.start)
            .unwrap();
        let dir = find_bit_directory(&buf, &img).unwrap();
        assert_eq!(dir.offset, bit);
        assert_eq!(dir.token(b'P').unwrap().data, img.start + 0x40); // token@0x40
        assert_eq!(dir.token(b'C').unwrap().version, 2);
        // 双 BIT 应被拒绝。
        let mut two = buf.clone();
        let _ = push_bit(&mut two, 0xA00, &[(b'P', 2, 8, 0x40)]);
        repair_checksum(&mut two, legacy.start, legacy.size);
        assert!(find_bit_directory(&two, &img).is_err());
    }

    #[test]
    fn pointer_resolution_skips_efi() {
        let (mut buf, legacy, efi) = ampere_rom_with_legacy(0x1000);
        // token 数据在 0x100/0x120（避开 PCIR 头）；P 的 u32 槽值指向
        // legacy 内 0x40，C 的槽值指向 E0 区（legacy.size+0x40）。
        push_bit(
            &mut buf,
            0x800,
            &[(b'P', 2, 0x10, 0x100), (b'C', 2, 0x10, 0x120)],
        );
        buf[0x100..0x104].copy_from_slice(&0x40u32.to_le_bytes());
        buf[0x120..0x124].copy_from_slice(&((legacy.size + 0x40) as u32).to_le_bytes());
        repair_checksum(&mut buf, legacy.start, legacy.size);
        let img = scan_images(&buf)
            .0
            .into_iter()
            .find(|p| p.start == legacy.start)
            .unwrap();
        let dir = find_bit_directory(&buf, &img).unwrap();
        let p = bit_pointer(&buf, &dir, &img, Some(&efi), b'P', 0, true).unwrap();
        assert_eq!(p, legacy.start + 0x40);
        let c = bit_pointer(&buf, &dir, &img, Some(&efi), b'C', 0, true).unwrap();
        assert_eq!(c, efi.end + 0x40);
    }

    #[test]
    fn turing_chain_requires_0x70_tail_without_npde_last() {
        // Turing：E0 不带 last、链尾由 0x70 收尾。
        let mut buf = Vec::new();
        push_image(&mut buf, [0x55, 0xAA], 0, 0x1F08, false, 0x18, None);
        push_image(&mut buf, [0x55, 0xAA], 3, 0x1F08, false, 0x18, None);
        push_image(
            &mut buf,
            [b'V', b'N'],
            0xE0,
            0x1F00,
            false,
            0x18,
            Some(false),
        );
        push_image(
            &mut buf,
            [b'V', b'N'],
            0xE0,
            0x1F00,
            false,
            0x18,
            Some(false),
        );
        push_image(&mut buf, [b'V', b'N'], 0x70, 0x0000, true, 0x18, None);
        let (pci, extended, issues) = scan_images(&buf);
        assert!(issues.is_empty(), "{issues:?}");
        let chains = image_chains(&buf, &pci, &extended, Family::Turing).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), 5);
        assert_eq!(chains[0].last().unwrap().image_type, 0x70);
        // 无 EFI、legacy 直接 last 的 Turing 变体。
        let mut buf2 = Vec::new();
        push_image(&mut buf2, [0x55, 0xAA], 0, 0x1F08, true, 0x18, None);
        push_image(
            &mut buf2,
            [b'V', b'N'],
            0xE0,
            0x1F00,
            false,
            0x18,
            Some(false),
        );
        push_image(
            &mut buf2,
            [b'V', b'N'],
            0xE0,
            0x1F00,
            false,
            0x18,
            Some(false),
        );
        push_image(&mut buf2, [b'V', b'N'], 0x70, 0x0000, true, 0x18, None);
        let (pci, extended, issues) = scan_images(&buf2);
        assert!(issues.is_empty(), "{issues:?}");
        let chains = image_chains(&buf2, &pci, &extended, Family::Turing).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), 4);
    }

    #[test]
    fn ampere_chain_requires_identity_consistency() {
        // 数据镜像 device 与 legacy 无关但共享 0x2200 时 OK；乱写则拒绝。
        let (buf, _l, _e) = ampere_rom();
        let (pci, extended, _) = scan_images(&buf);
        assert!(image_chains(&buf, &pci, &extended, Family::Ampere).is_ok());
        // Ada 规则不接受 0x2200 旁路（此处 legacy=0x2204，数据镜像 0x2200
        // 与 0x2204&0xFF80=0x2200 实际同族——改用不匹配 device 验证拒绝）。
        let mut bad = Vec::new();
        push_image(&mut bad, [0x55, 0xAA], 0, 0x2684, false, 0x18, None);
        push_image(&mut bad, [0x55, 0xAA], 3, 0x2684, false, 0x18, None);
        push_image(&mut bad, [b'V', b'N'], 0xE0, 0x2531, false, 0x18, None);
        push_image(&mut bad, [b'V', b'N'], 0xE0, 0x2531, true, 0x18, None);
        let (pci, extended, _) = scan_images(&bad);
        assert!(image_chains(&bad, &pci, &extended, Family::Ada).is_err());
    }
}
