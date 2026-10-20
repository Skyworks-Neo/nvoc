//! GB20x（50 系）"object directory v3" 容器解码——vix-editor
//! `rom/blackwell.py` 的 Rust 移植（read-only）。
//!
//! 镜像序列：legacy（type 0）→ EFI（type 3，便携 GOP 允许 0000:0000）→
//! 签名 meta（E0/NPDS）→ 扩展 E0（last）。meta 内含对象目录 v3（marker
//! `<II` = version 3 / header_size 16）：60B 条目 = (id u32, offset u32,
//! size u32, SHA384 48B)，目录自身 SHA384 摘要钉在 `meta.start + 0x209`；
//! legacy 镜像有同构目录（首对象 0xFA/0xBD，摘要 `legacy.start + 0x209`）。
//!
//! 表布局与 3x 世代不同处：功率策略 v0x40（P+0x2C，20 行，min/def/max 在
//! 行 +13）；rail v0x30（65B 行，选择器 u16 @+0x0D/+0x0F/+0x11/+0x13/
//! +0x21，boot u32 @+0x19）；VFE 头 `30 0e 1d ff 19`（255 变量×29B，方程
//! 25B/链接 u16，REL 允许 kind 7 多项式）；时钟 v0x50（9×15B，域身份描述
//! 符门控）；功率 override profiles（P+0xF4）。

use sha2::{Digest, Sha384};

use crate::error::Error;
use crate::vbios_image_chain::{
    BitDirectory, RomImage, bit_pointer, f32_at, find_bit_directory, image_chains, scan_images,
    u16_at, u32_at,
};
use crate::vbios_tables_v3::{ClockRange, RailRole, VminInfo};
use crate::vbios_vfe::{self, VfeLayout, VfeTable};

/// GB20x rail 表头前缀（v0x30，行 0x41；byte 3 = rail 数）。
const RAIL_HEADER_PREFIX: [u8; 3] = [0x30, 0x05, 0x41];
/// GB20x VFE 头（0x0E 头长、0x1D 变量行、0xFF 变量、0x19 旗标）。
const VFE_HEADER: [u8; 5] = [0x30, 0x0E, 0x1D, 0xFF, 0x19];
/// 功率通道表头（P+0x3C：v0x20、头 0x11、行 0x1A、32 行）。
const POWER_CHANNEL_HEADER: [u8; 4] = [0x20, 0x11, 0x1A, 0x20];
/// 功率设备表头（P+0x28：v0x30、头 0x18、行 0x19、11 行）。
const POWER_DEVICE_HEADER: [u8; 4] = [0x30, 0x18, 0x19, 0x0B];
/// 功率 override profile 表头（P+0xF4）。
const PROFILE_HEADER: [u8; 10] = [0x10, 0x0A, 0x05, 0x04, 0x07, 0x08, 0x01, 0x10, 0x09, 0x08];

// ── 对象目录 v3 ─────────────────────────────────────────────────────────

/// 目录对象条目（60B 头）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryObject {
    pub index: usize,
    pub id: u32,
    pub header: usize,
    pub data: usize,
    pub size: u32,
    /// SHA384 字段偏移（header + 12）。
    pub hash_offset: usize,
}

/// 对象目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDirectory {
    pub header: usize,
    pub header_size: u32,
    pub count: u32,
    pub data_base: usize,
    /// 目录 SHA384 摘要偏移（`region.start + 0x209`）。
    pub digest_offset: usize,
    pub objects: Vec<DirectoryObject>,
}

/// 在 `range`（起、止，止=PCIR 头偏移）内定位唯一对象目录。
/// `first_ids`：meta 目录 [0xFB, 0xBE]、legacy 目录 [0xFA, 0xBD]。
fn find_object_directory(
    data: &[u8],
    range: (usize, usize),
    device: u16,
    first_ids: [u32; 2],
) -> Result<Option<ObjectDirectory>, Error> {
    let (start, limit) = range;
    let mut candidates = Vec::new();
    let mut at = start;
    while at + 8 <= limit {
        let Some(pos) = data[at..limit]
            .windows(8)
            .position(|w| w == [3, 0, 0, 0, 16, 0, 0, 0])
            .map(|p| at + p)
        else {
            break;
        };
        at = pos + 1;
        let header_size = u32_at(data, pos + 4)?;
        let count = u32_at(data, pos + 8)?;
        let dir_device = u32_at(data, pos + 12)?;
        if !(2..=1024).contains(&count) || dir_device != u32::from(device) {
            continue;
        }
        let base = pos + header_size as usize + count as usize * 60;
        if !(pos < base && base <= limit) {
            continue;
        }
        let mut entries = Vec::with_capacity(count as usize);
        let mut ok = true;
        for index in 0..count as usize {
            let header = pos + header_size as usize + index * 60;
            if header + 60 > data.len() {
                ok = false;
                break;
            }
            let oid = u32_at(data, header)?;
            let offset = u32_at(data, header + 4)?;
            let size = u32_at(data, header + 8)?;
            let object_at = base + offset as usize;
            if size == 0 || object_at < base || object_at + size as usize > data.len() {
                ok = false;
                break;
            }
            entries.push(DirectoryObject {
                index,
                id: oid,
                header,
                data: object_at,
                size,
                hash_offset: header + 12,
            });
        }
        if !ok
            || entries.len() != count as usize
            || entries[0].id != first_ids[0]
            || entries[1].id != first_ids[1]
            || entries[1].data != limit
            || entries
                .iter()
                .map(|o| o.id)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != entries.len()
        {
            continue;
        }
        let digest_offset = start + 0x209;
        if digest_offset + 48 > pos {
            continue;
        }
        candidates.push(ObjectDirectory {
            header: pos,
            header_size,
            count,
            data_base: base,
            digest_offset,
            objects: entries,
        });
    }
    if candidates.len() > 1 {
        return Err(Error::Custom(format!(
            "vbios: expected one object directory, found {}",
            candidates.len()
        )));
    }
    Ok(candidates.into_iter().next())
}

// ── 容器布局 ────────────────────────────────────────────────────────────

/// GB20x 容器布局。
#[derive(Debug, Clone)]
pub struct Gb20xLayout {
    pub legacy: RomImage,
    pub efi: RomImage,
    pub meta: RomImage,
    pub extended: RomImage,
    pub bit: BitDirectory,
    pub meta_dir: ObjectDirectory,
    /// legacy 自身目录（旧镜像可能缺）。
    pub legacy_dir: Option<ObjectDirectory>,
}

impl Gb20xLayout {
    pub fn image_regions(&self) -> [(usize, usize); 2] {
        [
            (self.meta.start, self.meta.end),
            (self.extended.start, self.extended.end),
        ]
    }

    pub fn pointer(&self, data: &[u8], token: u8, offset: usize) -> Result<usize, Error> {
        let at = bit_pointer(
            data,
            &self.bit,
            &self.legacy,
            Some(&self.efi),
            token,
            offset,
            true,
        )?;
        if at + 16 > self.image_regions()[1].1 {
            return Err(Error::from("vbios: table pointer is outside BIOS data"));
        }
        Ok(at)
    }

    /// 拥有 at..at+size 的唯一目录对象。
    pub fn object_for(&self, at: usize, size: usize) -> Result<&DirectoryObject, Error> {
        let owners: Vec<&DirectoryObject> = self
            .meta_dir
            .objects
            .iter()
            .filter(|o| o.data <= at && at + size <= o.data + o.size as usize)
            .collect();
        if owners.len() != 1 {
            return Err(Error::Custom(format!(
                "vbios: expected one object for {at:#x}+{size}"
            )));
        }
        Ok(owners[0])
    }
}

/// 解析 GB20x 布局（镜像身份/序列/目录 fail-closed）。
pub fn parse_layout(data: &[u8]) -> Result<Gb20xLayout, Error> {
    let (pci, extended, _issues) = scan_images(data);
    let mut legacy_list: Vec<&RomImage> = pci
        .iter()
        .filter(|p| p.image_type == 0 && p.vendor == 0x10DE)
        .collect();
    if legacy_list.len() != 1 {
        return Err(Error::from("vbios: expected one NVIDIA legacy PCI image"));
    }
    let legacy = legacy_list.remove(0).clone();
    let efi_list: Vec<&RomImage> = pci
        .iter()
        .filter(|p| p.start == legacy.end && p.image_type == 3)
        .collect();
    if efi_list.len() != 1 {
        return Err(Error::from(
            "vbios: expected an EFI image immediately after the legacy image",
        ));
    }
    let efi = efi_list[0].clone();
    let efi_identity_ok = match (efi.vendor, efi.device) {
        (0, 0) => true,
        (0x10DE, 0) => true,
        (0x10DE, d) => d == legacy.device,
        _ => false,
    };
    if !efi_identity_ok {
        return Err(Error::from(
            "vbios: EFI PCI identity disagrees with the legacy image",
        ));
    }
    // meta 是 PCIR-listed 的 E0 镜像（55AA + PCIR magic、type 0xE0）；
    // 其后的 NPDS（'VN'）才是扩展数据镜像。
    let meta_list: Vec<&RomImage> = pci
        .iter()
        .chain(extended.iter())
        .filter(|p| p.start == efi.end && p.image_type == 0xE0)
        .collect();
    if meta_list.len() != 1 {
        return Err(Error::from(
            "vbios: expected the signed data image after EFI",
        ));
    }
    let meta = meta_list[0].clone();
    if meta.vendor != 0x10DE || meta.device & 0xFF80 != legacy.device & 0xFF80 {
        return Err(Error::from(
            "vbios: metadata chip identity disagrees with the legacy image",
        ));
    }
    let ext_list: Vec<&RomImage> = extended
        .iter()
        .filter(|p| p.start == meta.end && p.image_type == 0xE0)
        .collect();
    if ext_list.len() != 1 || ext_list[0].vendor != 0x10DE {
        return Err(Error::from(
            "vbios: expected one bounded NPDS data image after signed metadata",
        ));
    }
    let ext = ext_list[0].clone();
    if !ext.last || ext.device != meta.device {
        return Err(Error::from(
            "vbios: NPDS identity or last-image flag is inconsistent",
        ));
    }
    let bit = find_bit_directory(data, &legacy)?;
    let meta_dir =
        find_object_directory(data, (meta.start, meta.header), legacy.device, [0xFB, 0xBE])?
            .ok_or_else(|| {
                Error::from("vbios: expected one object directory in the signed data image")
            })?;
    let legacy_dir = find_object_directory(
        data,
        (legacy.start, legacy.header),
        legacy.device,
        [0xFA, 0xBD],
    )?;
    // image_chains 的 Ampere/Ada 语义在这里不适用，但仍要求能组链（身份
    // 门与 3x 共用）。失败不致命——GB20x 有自己的序列检查（上面）。
    let _ = image_chains(
        data,
        &pci,
        &extended,
        crate::vbios_families::Family::BlackwellGb20x,
    );
    Ok(Gb20xLayout {
        legacy,
        efi,
        meta,
        extended: ext,
        bit,
        meta_dir,
        legacy_dir,
    })
}

// ── 完整性（SHA384） ────────────────────────────────────────────────────

fn sha384_ok(data: &[u8], object: &DirectoryObject) -> bool {
    let Some(body) = data.get(object.data..object.data + object.size as usize) else {
        return false;
    };
    let Some(expected) = data.get(object.hash_offset..object.hash_offset + 48) else {
        return false;
    };
    Sha384::digest(body).as_slice() == expected
}

fn directory_digest_ok(data: &[u8], dir: &ObjectDirectory) -> bool {
    let Some(body) = data.get(dir.header..dir.data_base) else {
        return false;
    };
    let Some(expected) = data.get(dir.digest_offset..dir.digest_offset + 48) else {
        return false;
    };
    Sha384::digest(body).as_slice() == expected
}

/// legacy 目录完整性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDirectoryIntegrity {
    pub object_count: usize,
    pub bad_object_ids: Vec<u32>,
    pub directory_sha384_valid: bool,
}

/// GB20x 完整性报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gb20xIntegrity {
    pub object_count: usize,
    pub bad_object_ids: Vec<u32>,
    pub directory_sha384_valid: bool,
    /// (meta, extended) 两张数据镜像的加性和（须为 0）。
    pub image_checksums: Vec<u32>,
    pub legacy_directory: Option<LegacyDirectoryIntegrity>,
    pub valid: bool,
}

/// 全量校验：对象 SHA384、目录 SHA384、数据镜像与全部 PCI 镜像加性和。
pub fn integrity(data: &[u8], layout: &Gb20xLayout) -> Gb20xIntegrity {
    let bad: Vec<u32> = layout
        .meta_dir
        .objects
        .iter()
        .filter(|o| !sha384_ok(data, o))
        .map(|o| o.id)
        .collect();
    let directory_ok = directory_digest_ok(data, &layout.meta_dir);
    let image_checksums: Vec<u32> = layout
        .image_regions()
        .iter()
        .map(|(s, e)| {
            data.get(*s..*e)
                .map(|b| b.iter().fold(0u8, |a, &x| a.wrapping_add(x)))
                .unwrap_or(0xFF) as u32
        })
        .collect();
    let legacy = layout
        .legacy_dir
        .as_ref()
        .map(|d| LegacyDirectoryIntegrity {
            object_count: d.objects.len(),
            bad_object_ids: d
                .objects
                .iter()
                .filter(|o| !sha384_ok(data, o))
                .map(|o| o.id)
                .collect(),
            directory_sha384_valid: directory_digest_ok(data, d),
        });
    let valid = bad.is_empty()
        && directory_ok
        && image_checksums == [0, 0]
        && legacy
            .as_ref()
            .is_none_or(|l| l.bad_object_ids.is_empty() && l.directory_sha384_valid);
    Gb20xIntegrity {
        object_count: layout.meta_dir.objects.len(),
        bad_object_ids: bad,
        directory_sha384_valid: directory_ok,
        image_checksums,
        legacy_directory: legacy,
        valid,
    }
}

// ── 功率策略 v0x40 ──────────────────────────────────────────────────────

/// 功率策略行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gb20xPowerRow {
    pub row: usize,
    pub offset: usize,
    pub kind: u8,
    pub minimum_raw: u32,
    pub default_raw: u32,
    pub maximum_raw: u32,
    pub maximum_offset: usize,
}

/// 功率策略表（P+0x2C，v0x40，20 行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gb20xPowerPolicy {
    pub offset: usize,
    pub version: u8,
    pub header_size: u8,
    pub entry_size: u8,
    /// 笔记本布局（头 +21..24 == FF FF 02、行 2 为 TGP、行 7/8 空）。
    pub notebook: bool,
    pub rows: Vec<Gb20xPowerRow>,
}

/// 解析功率策略。桌面布局 TGP = 行 8（type 12）；笔记本 = 行 2（type 1）。
pub fn power_policy(data: &[u8], layout: &Gb20xLayout) -> Result<Gb20xPowerPolicy, Error> {
    let at = layout.pointer(data, b'P', 11 * 4)?;
    let version = data[at];
    let header_size = data[at + 1];
    let entry_size = data[at + 2];
    let count = data[at + 3];
    if !matches!(
        (version, header_size, entry_size),
        (0x40, 0x38, 0x66) | (0x40, 0x3D, 0x66)
    ) || count != 20
    {
        return Err(Error::from("vbios: unsupported power policy table"));
    }
    layout.object_for(
        at,
        usize::from(header_size) + usize::from(entry_size) * usize::from(count),
    )?;
    let mut rows = Vec::with_capacity(usize::from(count));
    for i in 0..usize::from(count) {
        let row = at + usize::from(header_size) + i * usize::from(entry_size);
        rows.push(Gb20xPowerRow {
            row: i,
            offset: row,
            kind: data[row],
            minimum_raw: u32_at(data, row + 13)?,
            default_raw: u32_at(data, row + 17)?,
            maximum_raw: u32_at(data, row + 21)?,
            maximum_offset: row + 21,
        });
    }
    let notebook = data[at + 21..at + 24] == [0xFF, 0xFF, 0x02]
        && rows[2].kind == 1
        && [7usize, 8]
            .iter()
            .all(|&i| rows[i].kind == 0 && rows[i].maximum_raw == 0);
    Ok(Gb20xPowerPolicy {
        offset: at,
        version,
        header_size,
        entry_size,
        notebook,
        rows,
    })
}

/// 功率通道表（P+0x3C，32×26B，对象 0x16）。
pub fn power_channels(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<[u8; 26]>, Error> {
    let at = layout.pointer(data, b'P', 0x3C)?;
    if data[at..at + 4] != POWER_CHANNEL_HEADER {
        return Err(Error::from("vbios: unsupported power channel table"));
    }
    layout.object_for(at, 0x11 + 0x1A * 0x20)?;
    let mut out = Vec::with_capacity(0x20);
    for i in 0..0x20usize {
        let start = at + 0x11 + 0x1A * i;
        let mut e = [0u8; 26];
        e.copy_from_slice(&data[start..start + 0x1A]);
        out.push(e);
    }
    Ok(out)
}

/// 功率设备表（P+0x28，11×25B，对象 0x12）。
pub fn power_devices(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<[u8; 25]>, Error> {
    let at = layout.pointer(data, b'P', 0x28)?;
    if data[at..at + 4] != POWER_DEVICE_HEADER {
        return Err(Error::from("vbios: unsupported power device table"));
    }
    layout.object_for(at, 0x18 + 0x19 * 0x0B)?;
    let mut out = Vec::with_capacity(0x0B);
    for i in 0..0x0Busize {
        let start = at + 0x18 + 0x19 * i;
        let mut e = [0u8; 25];
        e.copy_from_slice(&data[start..start + 0x19]);
        out.push(e);
    }
    Ok(out)
}

// ── rail v0x30 ──────────────────────────────────────────────────────────

/// rail 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gb20xRailRow {
    pub index: usize,
    pub offset: usize,
    pub name: &'static str,
    pub boot_voltage_uv: u32,
    pub boot_offset: usize,
    /// (role, 偏移, u16 raw)。
    pub selectors: Vec<(RailRole, usize, u16)>,
}

/// rail 表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gb20xRailTable {
    pub offset: usize,
    pub row_size: usize,
    pub rows: Vec<Gb20xRailRow>,
}

/// rail 数（v0x30：1 或 2；1 轨时 byte 4 须为 0）。
pub fn rail_count(data: &[u8], layout: &Gb20xLayout) -> Result<usize, Error> {
    let rail = layout.pointer(data, b'P', 0x68)?;
    let header = &data[rail..rail + 5];
    if header[..3] != RAIL_HEADER_PREFIX
        || !matches!(header[3], 1 | 2)
        || (header[3] == 1 && header[4] != 0)
    {
        return Err(Error::from(
            "vbios: unsupported rail table; v0x30 with one or two rows is required",
        ));
    }
    let end = rail + 5 + 0x41 * usize::from(header[3]);
    if end > layout.legacy.end {
        return Err(Error::from("vbios: rail table exceeds legacy image"));
    }
    Ok(usize::from(header[3]))
}

/// 解析 rail 表。
pub fn rails(data: &[u8], layout: &Gb20xLayout) -> Result<Gb20xRailTable, Error> {
    let rail = layout.pointer(data, b'P', 0x68)?;
    let count = rail_count(data, layout)?;
    let names = ["NVVDD", "MSVDD"];
    let mut rows = Vec::new();
    // 行界是头字段（1 或 2）且行内校验依赖行号——不能迭代 names。
    #[allow(clippy::needless_range_loop)]
    for i in 0..count {
        let row = rail + 5 + 0x41 * i;
        if data[row] != 1 || data[row + 1] as usize != i + 1 {
            return Err(Error::from("vbios: unvalidated rail logical identity"));
        }
        rows.push(Gb20xRailRow {
            index: i,
            offset: row,
            name: names[i],
            boot_voltage_uv: u32_at(data, row + 0x19)?,
            boot_offset: row + 0x19,
            selectors: vec![
                (RailRole::Rel, row + 0x0D, u16_at(data, row + 0x0D)?),
                (RailRole::Alt, row + 0x0F, u16_at(data, row + 0x0F)?),
                (RailRole::Ov, row + 0x11, u16_at(data, row + 0x11)?),
                (
                    RailRole::VminEvaluated,
                    row + 0x13,
                    u16_at(data, row + 0x13)?,
                ),
                (RailRole::VminInfo, row + 0x21, u16_at(data, row + 0x21)?),
            ],
        });
    }
    Ok(Gb20xRailTable {
        offset: rail,
        row_size: 0x41,
        rows,
    })
}

/// GB20x VFE 表（P+0x20）。
pub fn vfe(data: &[u8], layout: &Gb20xLayout) -> Result<VfeTable, Error> {
    let at = layout.pointer(data, b'P', 0x20)?;
    if data[at..at + 5] != VFE_HEADER {
        return Err(Error::from(
            "vbios: unvalidated Blackwell VFE output layout",
        ));
    }
    layout.object_for(at, 1)?;
    vbios_vfe::decode(data, at, VfeLayout::ADA_GB20X, true)
}

// ── rail × VFE ──────────────────────────────────────────────────────────

/// 单 role 方程视图（选择器 u16）。
#[derive(Debug, Clone, PartialEq)]
pub struct Gb20xRailEquation {
    pub rail_name: &'static str,
    pub rail_index: usize,
    pub role: RailRole,
    pub selector_offset: usize,
    pub selector_raw: u16,
    pub absent: bool,
    pub boot_voltage_uv: u32,
    pub equation_offset: Option<usize>,
    pub equation_kind: Option<vbios_vfe::OutputKind>,
    pub output_uv: Option<(f32, f32)>,
    pub maximum_offset: Option<usize>,
    /// 复核过的 REL（kind 1|7）/ ALT（kind 1）电压输出（vix layout_recognized）。
    pub layout_recognized: bool,
}

/// 可能持有 VFE 选择器的 rail 行 u16 字位（vix RAIL_SELECTORS）。
const RAIL_SELECTOR_WORDS: [usize; 14] = [
    0x0D, 0x0F, 0x11, 0x13, 0x15, 0x17, 0x1D, 0x1F, 0x21, 0x23, 0x29, 0x2B, 0x2D, 0x2F,
];

/// rail 全 role 方程视图（vix `rail_equations`：REL 允许 kind 1|7、
/// ALT/OV 仅 kind 1，单位码须为 2）。
pub fn rail_equations(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<Gb20xRailEquation>, Error> {
    let table = rails(data, layout)?;
    let vfe_table = vfe(data, layout)?;
    let mut out = Vec::new();
    for row in &table.rows {
        for (role, offset, raw) in &row.selectors {
            let absent = *raw == 0xFFFF;
            let mut item = Gb20xRailEquation {
                rail_name: row.name,
                rail_index: row.index,
                role: *role,
                selector_offset: *offset,
                selector_raw: *raw,
                absent,
                boot_voltage_uv: row.boot_voltage_uv,
                equation_offset: None,
                equation_kind: None,
                output_uv: None,
                maximum_offset: None,
                layout_recognized: false,
            };
            if !absent {
                let at = vfe_table.equations_base + usize::from(*raw) * 25;
                layout.object_for(at, 25)?;
                let equation = vfe_table
                    .equation(*raw)
                    .ok_or_else(|| Error::from("vbios: selector outside equation table"))?;
                let (lo, hi) = equation.clamp.unwrap_or((f32::NAN, f32::NAN));
                let finite = lo.is_finite() && hi.is_finite() && lo <= hi;
                item.equation_offset = Some(at);
                item.maximum_offset = Some(at + 7);
                item.output_uv = finite.then_some((lo, hi));
                let allowed = match role {
                    RailRole::Rel => matches!(equation.kind_raw, 1 | 7),
                    RailRole::Alt | RailRole::Ov => equation.kind_raw == 1,
                    _ => false,
                };
                if finite && allowed && equation.output_unit == 2 {
                    item.layout_recognized = true;
                }
                item.equation_kind = Some(vbios_vfe::classify_quadratic(
                    equation,
                    vfe_table.references.as_ref(),
                    vfe_table.variable_count,
                    vbios_vfe::variable_is_temperature(data, &vfe_table, equation.variable),
                    1,
                ));
            }
            out.push(item);
        }
    }
    Ok(out)
}

/// REL 钳位上限（vix `rail_limits`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Gb20xRailLimit {
    pub rail_name: &'static str,
    pub rail_index: usize,
    pub rel_equation_index: u16,
    pub equation_offset: usize,
    pub output_minimum_uv: f32,
    pub output_maximum_uv: f32,
    pub maximum_offset: usize,
}

/// REL 方程 clamp（kind 1|7、电压输出、对象 0x14 拥有）。
pub fn rail_limits(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<Gb20xRailLimit>, Error> {
    let table = rails(data, layout)?;
    let vfe_table = vfe(data, layout)?;
    let mut out = Vec::new();
    for row in &table.rows {
        let rel = u16_at(data, row.offset + 0x0D)?;
        let at = vfe_table.equations_base + usize::from(rel) * 25;
        if layout.object_for(at, 25)?.id != vfe_object_id(data, layout) {
            return Err(Error::from("vbios: rail equation is not in object 0x14"));
        }
        if data[at + 23] & 0x0F != 2 {
            return Err(Error::from(
                "vbios: rail equation is not a voltage output in object 0x14",
            ));
        }
        let (lo, hi) = (f32_at(data, at + 3)?, f32_at(data, at + 7)?);
        if !lo.is_finite() || !hi.is_finite() || lo < 0.0 || lo > hi {
            return Err(Error::from("vbios: invalid voltage equation bounds"));
        }
        out.push(Gb20xRailLimit {
            rail_name: row.name,
            rail_index: row.index,
            rel_equation_index: rel,
            equation_offset: at,
            output_minimum_uv: lo,
            output_maximum_uv: hi,
            maximum_offset: at + 7,
        });
    }
    Ok(out)
}

/// 对象 0x14（VFE）的 id 探测。
fn vfe_object_id(data: &[u8], layout: &Gb20xLayout) -> u32 {
    let Ok(at) = layout.pointer(data, b'P', 0x20) else {
        return 0;
    };
    layout.object_for(at, 1).map(|o| o.id).unwrap_or(0)
}

/// 评估 VMIN（rail 行 +0x13 的方程；与 3x 语义一致：常数方程 + 无引用 +
/// 无其它选择器共选才 pinned）。
pub fn rail_vmin(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<VminInfo>, Error> {
    let table = rails(data, layout)?;
    let vfe_table = vfe(data, layout)?;
    let refs = vfe_table.references.clone();
    let layout_ok = refs.is_some();
    let all_words: Vec<u16> = table
        .rows
        .iter()
        .flat_map(|r| {
            RAIL_SELECTOR_WORDS
                .iter()
                .filter_map(|&o| u16_at(data, r.offset + o).ok())
        })
        .collect();
    let mut out = Vec::new();
    for row in &table.rows {
        let selector = u16_at(data, row.offset + 0x13)?;
        let rel = u16_at(data, row.offset + 0x0D)?;
        let rel_lowest = vbios_vfe::equation_span(
            data,
            vfe_table.equations_base,
            vfe_table.layout(),
            vfe_table.equation_count,
            rel,
        )
        .map(|s| s.0);
        let mut item = VminInfo {
            rail_name: row.name,
            rail_index: row.index,
            selector_offset: row.offset + 0x13,
            selector_raw: selector,
            value_uv: None,
            pinned: false,
            rel_lowest_uv: rel_lowest,
            added_uv: Some((0.0, 0.0)),
            diagnostic: None,
        };
        if usize::from(selector) >= usize::from(vfe_table.equation_count) {
            item.diagnostic = Some("is absent or outside the equation table");
            out.push(item);
            continue;
        }
        let equation = vfe_table.equation(selector).expect("bounded");
        item.added_uv = equation.next.and_then(|n| {
            vbios_vfe::equation_span(
                data,
                vfe_table.equations_base,
                vfe_table.layout(),
                vfe_table.equation_count,
                n,
            )
        });
        let Some(value) = equation
            .next
            .is_none()
            .then(|| vbios_vfe::constant_equation_uv(equation))
            .flatten()
        else {
            item.diagnostic = Some(if equation.kind_raw == 2 {
                "is a min/max equation tree"
            } else {
                "is not a constant voltage equation in object 0x14"
            });
            out.push(item);
            continue;
        };
        if !layout_ok
            || refs.as_ref().is_some_and(|r| r.contains(&selector))
            || all_words.iter().filter(|&&w| w == selector).count() > 1
        {
            item.diagnostic = Some("is used by another equation or rail selector");
        } else {
            item.value_uv = Some(value);
            item.pinned = true;
        }
        out.push(item);
    }
    Ok(out)
}

// ── 时钟 v0x50 ──────────────────────────────────────────────────────────

/// 时钟域身份描述符（vix `descriptors` + 笔记本 DRAM 旗标 0x1182）。
const CLOCK_DESCRIPTORS: [u16; 9] = [0x43, 0x84, 0x182, 0xC5, 0x106, 0x147, 0x8, 0x200, 0x1C1];
/// 两种已审域索引布局（桌面 5090/5080 与 SFF；元组为各行 (first,last)）。
const CLOCK_LAYOUTS: [[(u8, u8); 9]; 2] = [
    [
        (13, 13),
        (17, 17),
        (8, 12),
        (15, 15),
        (14, 14),
        (16, 16),
        (22, 26),
        (0, 7),
        (18, 21),
    ],
    [
        (12, 12),
        (16, 16),
        (8, 11),
        (14, 14),
        (13, 13),
        (15, 15),
        (21, 25),
        (0, 7),
        (17, 20),
    ],
];

/// 解析时钟范围表（C+8，v0x50）。域身份按已知布局门控；未知布局报错
/// （vix：不得把 SFF VID 误认成 GB202 GPC）。
pub fn clock_ranges(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<ClockRange>, Error> {
    let at = layout.pointer(data, b'C', 8)?;
    if layout.object_for(at, 1)?.id != 3 || data[at] != 0x50 {
        return Err(Error::from("vbios: unsupported clock offset table"));
    }
    let hs = usize::from(data[at + 1]);
    let entry_size = usize::from(data[at + 2]);
    let count = usize::from(data[at + 3]);
    let subcount = usize::from(data[at + 4]);
    let subsize = usize::from(data[at + 5]);
    if (hs, entry_size, count, subcount) != (13, 15, 9, 3) || !matches!(subsize, 1 | 2) {
        return Err(Error::from("vbios: unsupported clock offset rows"));
    }
    let stride = entry_size + subcount * subsize;
    layout.object_for(at, hs + stride * count)?;
    let ranges: Vec<(u8, u8)> = (0..count)
        .map(|i| {
            (
                data[at + hs + stride * i + 1],
                data[at + hs + stride * i + 2],
            )
        })
        .collect();
    let Some(_layout_index) = CLOCK_LAYOUTS.iter().position(|l| *l == ranges.as_slice()) else {
        return Err(Error::from("vbios: unsupported clock domain index layout"));
    };
    let names = ["GPC", "XBAR", "DRAM", "SYS", "VID"];
    let mut out = Vec::new();
    // 循环界是动态 count=9 而名字表只有前 5 行——不能直接迭代 names。
    #[allow(clippy::needless_range_loop)]
    for i in 0..count {
        let row = at + hs + stride * i;
        if data[row] != 2 {
            return Err(Error::from(
                "vbios: unsupported clock domain identity descriptor",
            ));
        }
        let descriptor = u16_at(data, row + 9)?;
        let expected = if i == 2 {
            // 笔记本 DRAM 行置 bit 12，其余同桌面。
            [CLOCK_DESCRIPTORS[2], 0x1182]
        } else {
            [CLOCK_DESCRIPTORS[i], CLOCK_DESCRIPTORS[i]]
        };
        if !expected.contains(&descriptor) {
            return Err(Error::from(
                "vbios: unsupported clock domain identity descriptor",
            ));
        }
        // vix 只对前 5 行赋名（GPC/XBAR/DRAM/SYS/VID）；其余行以域索引
        // 区间标识（静态表），不冒认 PWR/PCIEGEN/HOST。
        static UNKNOWN_DOMAINS: [&str; 4] = ["domain-5", "domain-6", "domain-7", "domain-8"];
        out.push(ClockRange {
            row: i,
            domain: if i < names.len() {
                names[i]
            } else {
                UNKNOWN_DOMAINS[i - names.len()]
            },
            minimum_mhz: crate::vbios_image_chain::i16_at(data, row + 3)?,
            maximum_mhz: crate::vbios_image_chain::i16_at(data, row + 5)?,
            minimum_offset: row + 3,
            maximum_offset: row + 5,
            range_stored: true,
        });
    }
    Ok(out)
}

// ── 功率 override profiles ──────────────────────────────────────────────

/// override 单条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileEntry {
    /// 策略行号（2/7/8/9）。
    pub policy: u8,
    /// false = maximum、true = default。
    pub default_field: bool,
    pub value_raw: u32,
    pub value_offset: usize,
}

/// override profile。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerOverrideProfile {
    pub index: usize,
    pub offset: usize,
    pub entries: Vec<ProfileEntry>,
}

/// 功率 override profiles（P+0xF4）。selector 0/1 分别对应 maximum/default
/// 档；profile 与基础表完全一致并不能证明固件当前选中它。
pub fn power_profiles(
    data: &[u8],
    layout: &Gb20xLayout,
) -> Result<Vec<PowerOverrideProfile>, Error> {
    let at = layout.pointer(data, b'P', 0xF4)?;
    if data[at..at + 10] != PROFILE_HEADER {
        return Err(Error::from(
            "vbios: unsupported power override profile table",
        ));
    }
    layout.object_for(at, 0xFE + 16 * 0x49)?;
    let mut profiles = Vec::new();
    for i in 0..16usize {
        let row = at + 0xFE + i * 0x49;
        let flag = data[row] & 0x0F;
        if flag == 0 {
            continue;
        }
        if flag != 1 {
            return Err(Error::from(
                "vbios: unsupported power override profile type",
            ));
        }
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for j in 0..8usize {
            let entry = row + 1 + j * 9;
            let kind = data[entry] & 0x0F;
            if kind == 0 {
                continue;
            }
            let selector = u32_at(data, entry + 1)?;
            let value = u32_at(data, entry + 5)?;
            let policy = (selector & 0xFF) as u8;
            let field = (selector >> 8) as u8;
            if kind != 1 || !matches!(policy, 2 | 7 | 8 | 9) || !matches!(field, 0 | 1) {
                return Err(Error::from("vbios: unsupported power override entry"));
            }
            if !seen.insert((policy, field)) {
                return Err(Error::from("vbios: duplicate power override selector"));
            }
            entries.push(ProfileEntry {
                policy,
                default_field: field == 1,
                value_raw: value,
                value_offset: entry + 5,
            });
        }
        profiles.push(PowerOverrideProfile {
            index: i,
            offset: row,
            entries,
        });
    }
    Ok(profiles)
}

/// 与基础表边界完全一致的 profiles（vix `matching_power_profiles`：
/// policy 2/7/8/9 的 maximum/default 四元组逐值相等）。
pub fn matching_power_profiles(data: &[u8], layout: &Gb20xLayout) -> Result<Vec<usize>, Error> {
    let rows = power_policy(data, layout)?;
    let mut expected = std::collections::HashMap::new();
    for p in [2u8, 7, 8, 9] {
        let r = &rows.rows[usize::from(p)];
        expected.insert((p, false), r.maximum_raw);
        expected.insert((p, true), r.default_raw);
    }
    Ok(power_profiles(data, layout)?
        .into_iter()
        .filter(|prof| {
            prof.entries
                .iter()
                .map(|e| ((e.policy, e.default_field), e.value_raw))
                .collect::<std::collections::HashMap<_, _>>()
                == expected
        })
        .map(|p| p.index)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_selector_words_cover_documented_set() {
        // vix RAIL_SELECTORS：+0x0D..+0x17（3 间隔）、+0x1D/+0x1F/+0x21/+0x23、
        // +0x29..+0x3F 偶位——这里收录与 VMIN 共选判定相关的全部 u16 字。
        assert!(RAIL_SELECTOR_WORDS.contains(&0x13));
        assert!(RAIL_SELECTOR_WORDS.contains(&0x21));
    }

    #[test]
    fn power_policy_row_offsets_match_vix() {
        // vix：min/def/max 在行 +13/+17/+21。
        let r = Gb20xPowerRow {
            row: 0,
            offset: 0,
            kind: 1,
            minimum_raw: 1,
            default_raw: 2,
            maximum_raw: 3,
            maximum_offset: 21,
        };
        assert_eq!(r.maximum_offset, 21);
    }

    #[test]
    fn descriptor_table_shape() {
        assert_eq!(CLOCK_DESCRIPTORS.len(), 9);
        assert_eq!(CLOCK_LAYOUTS.len(), 2);
        assert_eq!(u16_at(&[0x43, 0x00], 0).unwrap(), 0x0043);
        let _ = f32_at;
        let _ = u32_at;
        let _: Option<VminInfo> = None;
        let _ = VfeLayout::ADA_GB20X;
        let _ = vbios_vfe::constant_equation_uv;
    }
}
