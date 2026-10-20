//! 内核地址空间的只读走查:复刻 xOCD 2.0.0 `NvidiaKernelReader` 的算法。
//!
//! 取证见 `docs/reverse-engineering/nvapi/xocd-oc-tool-audit.md` §8;原实现把
//! 任意物理读交给 Intel ME 后门驱动 `\\.\pmxdrv`(IOCTL 2239160 映射控制),
//! 本车道用 [`super::pmxdrv`] 的同款传输接上同一算法,四步一一对应:
//!
//! 1. `NtQuerySystemInformation(11)` 取 `nvlddmkm.sys` 内核基址(纯用户态 API,
//!    不需要任何内核读);
//! 2. 物理扫低 1 MiB(4096..0x100000,步长 4096)找 low-stub 页:页首 u64 过屏
//!    蔽后等于 `4295360745`(0x1_0006_00E9),即 Windows 低内存 stub 的机器码
//!    指纹;这些页里只有部分含自举页表根线索;
//! 3. 候选根校验:用候选根从虚拟地址四级走查翻译镜像基址,读回 4096 字节,
//!    与**磁盘上**的 `nvlddmkm.sys` PE 头比对(TimeDateStamp + SizeOfImage,
//!    严格档再加 256 字节前缀)——落盘文件与活体映像必须一致,否则拒绝;
//! 4. 走查(移位 [39,30,21,12],present 位 + PS 大页位处理)按页读取,得到
//!    可读的内核虚拟内存视图。
//!
//! 本文件是平台中立的:物理读经 [`PhysicalMemory`] 抽象,单测用内存模拟页表
//! 覆盖翻译/大页/寻根逻辑(无需驱动、Linux CI 可跑);Windows 上的实现见
//! [`super::pmxdrv`]。只读:**没有**任何写路径,写留给后续能力位实验。

use std::collections::HashSet;

use quick_error::quick_error;

/// 物理地址帧掩码(40 位物理地址,present 之上的帧域)。
pub const FRAME_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// low-stub 页首 u64 的指纹低字节掩码(bit 8..15 为 don't-care)。
pub const LOW_STUB_MASK: u64 = 0xFFFF_FFFF_FFFF_00FF;
/// low-stub 指纹期望值(十进制 4295360745 = 0x1_0006_00E9)。
pub const LOW_STUB_SIGNATURE: u64 = 0x1_0006_00E9;
/// 内核指针下界(0xFFFF_8000_0000_0000)。
pub const KERNEL_POINTER_MIN: u64 = 0xFFFF_8000_0000_0000;

/// 低内存扫描范围(与原实现一致:4096..1 MiB,255 页)。
const LOW_SCAN_START: u64 = 0x1000;
const LOW_SCAN_END: u64 = 0x10_0000;
/// 主路径取 low-stub 页偏移 160 处的 u64 作候选根(原实现证据页布局)。
const LOW_STUB_ROOT_OFFSET: usize = 160;
/// 单次虚拟读上限(与原实现一致,64 KiB)。
const MAX_VIRTUAL_READ: usize = 65536;
/// 四级走查移位(原实现常量)。
const WALK_SHIFTS: [u32; 4] = [39, 30, 21, 12];

quick_error! {
    /// 物理内存通道的失败。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PhysError {
        Unreadable(pa: u64) {
            display("physical address 0x{:X} is not readable", pa)
        }
        BadLength(len: usize) {
            display("physical reads support 8- or 4096-byte lengths (got {})", len)
        }
        Transport(msg: String) {
            display("physical read channel failed: {}", msg)
        }
    }
}

quick_error! {
    /// 页表走查的失败。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WalkError {
        InvalidRoot(root: u64) {
            display("page-table root 0x{:X} is not a valid frame", root)
        }
        NotKernelPointer(va: u64) {
            display("address 0x{:X} is not a kernel pointer", va)
        }
        NotPresent(va: u64) {
            display("page entry not present while translating 0x{:X}", va)
        }
        NullFrame {
            display("page entry resolves to a null frame")
        }
        InvalidLargePml4 {
            display("PML4 entry has the large-page bit set (invalid)")
        }
        TooLong(len: usize) {
            display("virtual reads are limited to 64 KiB (got {})", len)
        }
        Read(err: PhysError) {
            from() source(err) display("{}", err)
        }
    }
}

quick_error! {
    /// PE 指纹解析的失败。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PeError {
        TooShort {
            display("image is too short for a PE header")
        }
        NotPe {
            display("not a PE32+ image (bad MZ/PE/optional-header magic)")
        }
        ImplausibleSize(size: u32) {
            display("implausible SizeOfImage 0x{:X}", size)
        }
    }
}

quick_error! {
    /// 已加载内核模块定位(NtQuerySystemInformation)的失败。
    #[derive(Debug)]
    pub enum ImageError {
        Privilege(status: i32) {
            display("cannot enable SeDebugPrivilege: NTSTATUS 0x{:08X}", status)
        }
        Query(status: i32) {
            display("NtQuerySystemInformation(SystemModuleInformation) failed: NTSTATUS 0x{:08X}", status)
        }
        Layout(msg: String) {
            display("unexpected loaded-module list layout: {}", msg)
        }
        NotFound(name: String) {
            display("loaded kernel module {} is not in the system module list", name)
        }
        KernelPointer(base: u64) {
            display("module base 0x{:X} is not a kernel pointer", base)
        }
        Pe(err: PeError) {
            from() source(err) display("{}", err)
        }
        Io(err: std::io::Error) {
            from() source(err) display("IO error: {}", err)
        }
    }
}

/// 物理内存读取通道。实现者按需支持 8 字节(页表项)与 4096 字节(整页)两种
/// 长度,页对齐由调用方保证;不可读返回 `Err`,由上层跳过。
pub trait PhysicalMemory {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), PhysError>;
}

/// 内核指针判据(原实现常量)。
pub fn is_kernel_pointer(address: u64) -> bool {
    address >= KERNEL_POINTER_MIN
}

/// x86-64 四级页表翻译:返回 `address` 对应的物理地址。
///
/// 与原实现逐点一致:present 位缺失即失败;第 30/21 级(1 GiB/2 MiB)允许
/// PS 大页并返回 `帧|页内偏移`;第 39 级(PML4)出现 PS 位属非法;帧为 0
/// 视为失败。
pub fn translate(physical: &dyn PhysicalMemory, root: u64, address: u64) -> Result<u64, WalkError> {
    if root < 0x1000 || (root & !FRAME_MASK) != 0 {
        return Err(WalkError::InvalidRoot(root));
    }
    if !is_kernel_pointer(address) {
        return Err(WalkError::NotKernelPointer(address));
    }
    let mut table = root;
    for &shift in &WALK_SHIFTS {
        let entry_pa = table + ((address >> shift) & 0x1FF) * 8;
        let mut raw = [0u8; 8];
        physical.read(entry_pa, &mut raw)?;
        let entry = u64::from_le_bytes(raw);
        if entry & 1 == 0 {
            return Err(WalkError::NotPresent(address));
        }
        if (shift == 21 || shift == 30) && entry & 0x80 != 0 {
            let mask = (1u64 << shift) - 1;
            return Ok((entry & FRAME_MASK & !mask) | (address & mask));
        }
        if shift == 39 && entry & 0x80 != 0 {
            return Err(WalkError::InvalidLargePml4);
        }
        table = entry & FRAME_MASK;
        if table == 0 {
            return Err(WalkError::NullFrame);
        }
    }
    Ok(table + (address & 0xFFF))
}

/// 经候选根读取内核虚拟内存(按页翻译,支持跨页,单次上限 64 KiB)。
pub fn read_virtual(
    physical: &dyn PhysicalMemory,
    root: u64,
    address: u64,
    len: usize,
) -> Result<Vec<u8>, WalkError> {
    if len == 0 || len > MAX_VIRTUAL_READ {
        return Err(WalkError::TooLong(len));
    }
    if !is_kernel_pointer(address) || address.checked_add(len as u64).is_none() {
        return Err(WalkError::NotKernelPointer(address));
    }
    let mut out = vec![0u8; len];
    let mut done = 0usize;
    while done < len {
        let va = address + done as u64;
        let offset = (va & 0xFFF) as usize;
        let pa = translate(physical, root, va)?;
        let mut page = [0u8; 4096];
        physical.read(pa & !0xFFF, &mut page)?;
        let n = (len - done).min(4096 - offset);
        out[done..done + n].copy_from_slice(&page[offset..offset + n]);
        done += n;
    }
    Ok(out)
}

/// 磁盘 PE 指纹:活体映像校验只用这四样(时间戳/映像大小/前缀/偏移解析),
/// 与 xOCD 的比对字段一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeFingerprint {
    /// COFF TimeDateStamp。
    pub timestamp: u32,
    /// OptionalHeader.SizeOfImage。
    pub size_of_image: u32,
    /// 映像前 256 字节(严格档比对)。
    pub header_prefix: Vec<u8>,
}

/// 解析 PE32+ 头偏移(MZ + e_lfanew + `PE\0\0` + optional header magic
/// 0x20B),与原实现的判据一致;失败返回 `None`。
pub fn pe_offset_of(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 64 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    if offset < 64 || offset > bytes.len().checked_sub(84)? {
        return None;
    }
    if u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) != 0x0000_4550 {
        return None;
    }
    if u16::from_le_bytes(bytes[offset + 24..offset + 26].try_into().unwrap()) != 0x020B {
        return None;
    }
    Some(offset)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

impl PeFingerprint {
    /// 从磁盘映像文件内容构建指纹;SizeOfImage 合理性范围(4 KiB..512 MiB)
    /// 与原实现一致。
    pub fn from_image(image: &[u8]) -> Result<Self, PeError> {
        let offset = pe_offset_of(image).ok_or(PeError::NotPe)?;
        let timestamp = u32_at(image, offset + 8);
        let size_of_image = u32_at(image, offset + 80);
        if !(4096..=512 * 1024 * 1024).contains(&size_of_image) {
            return Err(PeError::ImplausibleSize(size_of_image));
        }
        if image.len() < 256 {
            return Err(PeError::TooShort);
        }
        Ok(Self {
            timestamp,
            size_of_image,
            header_prefix: image[..256].to_vec(),
        })
    }
}

/// 活体头比对结果(逐项记录,诊断用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveHeaderCheck {
    pub timestamp_matches: bool,
    pub size_matches: bool,
    pub prefix_matches: bool,
}

impl LiveHeaderCheck {
    pub fn all(&self) -> bool {
        self.timestamp_matches && self.size_matches && self.prefix_matches
    }
}

/// 用已确立的根读回活体镜像头并与磁盘指纹比对。
pub fn verify_live_header(
    physical: &dyn PhysicalMemory,
    root: u64,
    image_base: u64,
    fingerprint: &PeFingerprint,
) -> Result<LiveHeaderCheck, WalkError> {
    let head = read_virtual(physical, root, image_base, 4096)?;
    let Some(offset) = pe_offset_of(&head) else {
        return Ok(LiveHeaderCheck {
            timestamp_matches: false,
            size_matches: false,
            prefix_matches: false,
        });
    };
    Ok(LiveHeaderCheck {
        timestamp_matches: u32_at(&head, offset + 8) == fingerprint.timestamp,
        size_matches: u32_at(&head, offset + 80) == fingerprint.size_of_image,
        prefix_matches: head.len() >= 256
            && head[..256] == fingerprint.header_prefix[..fingerprint.header_prefix.len().min(256)],
    })
}

/// low-stub 页内的候选根:8 字节对齐 u64、高 12 位为 0、屏蔽后 >= 4096,
/// 去重返回。逐点复刻原实现的筛选条件。
pub fn low_stub_root_candidates(page: &[u8]) -> Vec<u64> {
    if page.len() != 4096
        || u64::from_le_bytes(page[0..8].try_into().unwrap()) & LOW_STUB_MASK != LOW_STUB_SIGNATURE
    {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut i = 8usize;
    while i <= page.len() - 8 {
        let value = u64::from_le_bytes(page[i..i + 8].try_into().unwrap());
        if value & 0xFFF0_0000_0000_0000 == 0 {
            let frame = value & FRAME_MASK;
            if frame >= 0x1000 && seen.insert(frame) {
                out.push(frame);
            }
        }
        i += 8;
    }
    out
}

/// 根发现过程的全部观测(全量返回,裁决在上层)。
#[derive(Debug, Default)]
pub struct RootDiscovery {
    /// 命中的 low-stub 页物理地址。
    pub low_stubs: Vec<u64>,
    /// 低内存扫描页数(固定 255)。
    pub scanned_pages: usize,
    /// 其中可读页数。
    pub readable_pages: usize,
    /// 去重后实际测试过的候选数。
    pub candidates_tested: usize,
    /// 通过验证的根(唯一才是好结果)。
    pub roots: Vec<u64>,
    /// 过程事件(与原实现的 RootDiscoveryEvents 同义)。
    pub events: Vec<String>,
}

impl RootDiscovery {
    /// 唯一根裁决;不唯一时返回完整诊断串。
    pub fn unique_root(&self) -> Result<u64, String> {
        if self.roots.len() == 1 {
            Ok(self.roots[0])
        } else {
            Err(format!(
                "no unique live/disk-matched page-table root: {} matches, {} low-stub signatures, {}/{} readable low-memory pages, {} candidates tested",
                self.roots.len(),
                self.low_stubs.len(),
                self.readable_pages,
                self.scanned_pages,
                self.candidates_tested
            ))
        }
    }
}

struct Discoverer<'a> {
    physical: &'a dyn PhysicalMemory,
    image_base: u64,
    fingerprint: &'a PeFingerprint,
    tested: HashSet<u64>,
    report: RootDiscovery,
}

impl Discoverer<'_> {
    fn probe(&mut self, candidate: u64, strict: bool, source: &str) {
        if candidate < 0x1000 || !self.tested.insert(candidate) {
            return;
        }
        self.report.candidates_tested = self.tested.len();
        match read_virtual(self.physical, candidate, self.image_base, 4096) {
            Ok(head) => {
                let matched = match pe_offset_of(&head) {
                    Some(offset) if head.len() >= offset + 84 => {
                        let timestamp_ok = u32_at(&head, offset + 8) == self.fingerprint.timestamp;
                        let size_ok = u32_at(&head, offset + 80) == self.fingerprint.size_of_image;
                        timestamp_ok
                            && size_ok
                            && (!strict
                                || head[..256]
                                    == self.fingerprint.header_prefix
                                        [..self.fingerprint.header_prefix.len().min(256)])
                    }
                    _ => false,
                };
                if matched {
                    self.report.roots.push(candidate);
                    self.report.events.push(format!(
                        "validated {}, root=0x{:016X}, strictHeader={}",
                        source, candidate, strict
                    ));
                } else if self.report.events.len() < 32 {
                    self.report
                        .events
                        .push(format!("rejected {}: live/disk header mismatch", source));
                }
            }
            Err(err) => {
                if self.report.events.len() < 32 {
                    self.report
                        .events
                        .push(format!("rejected {}: {}", source, err));
                }
            }
        }
    }
}

/// 根发现:低内存扫 low-stub 页 → 偏移 160 主路径(时间戳+大小校验)→
/// 前 4 个 stub 的全体帧候选回退(严格档:加 256 字节前缀校验)。
///
/// 逐点复刻原实现;读失败按页跳过(计数进报告),不中止。
pub fn discover_root(
    physical: &dyn PhysicalMemory,
    image_base: u64,
    fingerprint: &PeFingerprint,
) -> RootDiscovery {
    let mut discoverer = Discoverer {
        physical,
        image_base,
        fingerprint,
        tested: HashSet::new(),
        report: RootDiscovery::default(),
    };
    let mut page = [0u8; 4096];
    let mut stubs: Vec<(u64, [u8; 4096])> = Vec::new();
    let mut pa = LOW_SCAN_START;
    while pa < LOW_SCAN_END {
        discoverer.report.scanned_pages += 1;
        if physical.read(pa, &mut page).is_ok() {
            discoverer.report.readable_pages += 1;
            if u64::from_le_bytes(page[0..8].try_into().unwrap()) & LOW_STUB_MASK
                == LOW_STUB_SIGNATURE
            {
                discoverer.report.low_stubs.push(pa);
                let candidate = u64::from_le_bytes(
                    page[LOW_STUB_ROOT_OFFSET..LOW_STUB_ROOT_OFFSET + 8]
                        .try_into()
                        .unwrap(),
                ) & FRAME_MASK;
                let source = format!("low-stub 0x{:X}, offset {}", pa, LOW_STUB_ROOT_OFFSET);
                discoverer.probe(candidate, false, &source);
                if stubs.len() < 4 {
                    stubs.push((pa, page));
                }
            }
        }
        pa += 0x1000;
    }
    if discoverer.report.roots.is_empty() {
        for (pa, stub) in &stubs {
            for candidate in low_stub_root_candidates(stub) {
                let source = format!("low-stub fallback 0x{:X}", pa);
                discoverer.probe(candidate, true, &source);
            }
        }
    }
    if discoverer.report.roots.is_empty() {
        discoverer.report.events.push(
            "automatic physical-root scanning found no candidate: no arbitrary physical page was accepted".into(),
        );
    }
    discoverer.report
}

/// 已加载的内核模块(NtQuerySystemInformation(SystemModuleInformation) 视图)。
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModule {
    /// 内核映像基址。
    pub base: u64,
    /// 解析后的磁盘路径。
    pub path: std::path::PathBuf,
    /// `SeDebugPrivilege` 是否成功启用(诊断用;查询本身不依赖它)。
    pub debug_privilege: bool,
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut std::ffi::c_void,
        len: u32,
        ret_len: *mut u32,
    ) -> i32;
    fn RtlAdjustPrivilege(privilege: u32, enable: u8, current_thread: u8, previous: *mut u8)
    -> i32;
}

/// 在系统模块列表里定位已加载的内核模块,路径转换为 DOS 盘符形式。
///
/// 布局(与原实现一致,Win11 x64 实证):u32 模块数 @0、条目数组 @8、条目
/// 步长 296、ImageBase @+16、FullPathName @+40(256 字节 ASCII)。
#[cfg(windows)]
pub fn find_loaded_module(file_name: &str) -> Result<LoadedModule, ImageError> {
    /// STATUS_INFO_LENGTH_MISMATCH(缓冲区不够,按需扩)。
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
    const SYSTEM_MODULE_INFORMATION: u32 = 11;
    const ENTRY_STRIDE: usize = 296;
    const SE_DEBUG_PRIVILEGE: u32 = 20;

    let mut previous: u8 = 0;
    let privilege_status = unsafe { RtlAdjustPrivilege(SE_DEBUG_PRIVILEGE, 1, 0, &mut previous) };
    let debug_privilege = privilege_status >= 0;

    let mut size: u32 = 128 * 1024;
    for _ in 0..5 {
        let mut buffer = vec![0u8; size as usize];
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_MODULE_INFORMATION,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                size,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            size = size.max(needed.saturating_add(4096)).saturating_mul(2);
            continue;
        }
        if status < 0 {
            return Err(ImageError::Query(status));
        }
        let count = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
        if count == 0 || count > 16384 {
            return Err(ImageError::Layout(format!(
                "implausible module count {}",
                count
            )));
        }
        if 8 + count * ENTRY_STRIDE > buffer.len() {
            return Err(ImageError::Layout(format!(
                "module list truncated: {} entries > {} bytes",
                count,
                buffer.len()
            )));
        }
        for index in 0..count {
            let entry = 8 + index * ENTRY_STRIDE;
            let name_bytes = &buffer[entry + 40..entry + 40 + 256];
            let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
            let full_path = String::from_utf8_lossy(&name_bytes[..name_end]).to_string();
            let entry_name = std::path::Path::new(&full_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if entry_name.eq_ignore_ascii_case(file_name) {
                let base = u64::from_le_bytes(buffer[entry + 16..entry + 24].try_into().unwrap());
                if !is_kernel_pointer(base) {
                    return Err(ImageError::KernelPointer(base));
                }
                return Ok(LoadedModule {
                    base,
                    path: resolve_nt_path(&full_path)?,
                    debug_privilege,
                });
            }
        }
        return Err(ImageError::NotFound(file_name.to_string()));
    }
    Err(ImageError::Layout(
        "loaded-module list did not stabilize".into(),
    ))
}

/// `\SystemRoot\...` / `\??\...` 路径转 DOS 绝对路径。
#[cfg(windows)]
fn resolve_nt_path(full_path: &str) -> Result<std::path::PathBuf, ImageError> {
    let candidate = if let Some(rest) = full_path.strip_prefix(r"\SystemRoot\") {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        std::path::Path::new(&root).join(rest)
    } else if let Some(rest) = full_path.strip_prefix(r"\??\") {
        std::path::PathBuf::from(rest)
    } else {
        std::path::PathBuf::from(full_path)
    };
    if !candidate.is_absolute() {
        return Err(ImageError::Layout(format!(
            "kernel module path is not absolute: {}",
            full_path
        )));
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 内存模拟物理内存:按 4 KiB 页存取,缺席页 = 不可读。
    struct MockPhysical {
        pages: HashMap<u64, [u8; 4096]>,
    }

    impl PhysicalMemory for MockPhysical {
        fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), PhysError> {
            let page = self
                .pages
                .get(&(pa & !0xFFF))
                .ok_or(PhysError::Unreadable(pa))?;
            let offset = (pa & 0xFFF) as usize;
            match out.len() {
                4096 => {
                    if offset != 0 {
                        return Err(PhysError::BadLength(out.len()));
                    }
                    out.copy_from_slice(page);
                    Ok(())
                }
                8 => {
                    out.copy_from_slice(&page[offset..offset + 8]);
                    Ok(())
                }
                other => Err(PhysError::BadLength(other)),
            }
        }
    }

    fn set_entry(pages: &mut HashMap<u64, [u8; 4096]>, table: u64, index: u64, entry: u64) {
        let page = pages.entry(table).or_insert([0u8; 4096]);
        let at = (index * 8) as usize;
        page[at..at + 8].copy_from_slice(&entry.to_le_bytes());
    }

    const VA: u64 = 0xFFFF_8000_1234_5000;
    const ROOT: u64 = 0x9000;

    fn mapping() -> (MockPhysical, u64) {
        let mut pages = HashMap::new();
        set_entry(&mut pages, ROOT, (VA >> 39) & 0x1FF, 0xA000 | 0x3);
        set_entry(&mut pages, 0xA000, (VA >> 30) & 0x1FF, 0xB000 | 0x3);
        set_entry(&mut pages, 0xB000, (VA >> 21) & 0x1FF, 0xC000 | 0x3);
        set_entry(&mut pages, 0xC000, (VA >> 12) & 0x1FF, 0xD000 | 0x3);
        pages.insert(0xD000, [0u8; 4096]);
        (MockPhysical { pages }, ROOT)
    }

    #[test]
    fn translate_small_page() {
        let (physical, root) = mapping();
        assert_eq!(translate(&physical, root, VA + 0x123).unwrap(), 0xD123);
    }

    #[test]
    fn translate_two_megabyte_page() {
        let (mut physical, root) = mapping();
        set_entry(
            &mut physical.pages,
            0xB000,
            (VA >> 21) & 0x1FF,
            0x20_0000 | 0x80 | 0x3,
        );
        // 2 MiB 帧对齐(bit21),有效位取 address 的 21 位页内偏移。
        assert_eq!(translate(&physical, root, VA + 0x123).unwrap(), 0x34_5123);
    }

    #[test]
    fn translate_rejects_bad_inputs() {
        let (mut physical, root) = mapping();
        // mapping() 只建了 VA 所在的那一页,下一张 4 KiB 页没有 PT 表项。
        assert_eq!(
            translate(&physical, root, VA + 0x1000),
            Err(WalkError::NotPresent(VA + 0x1000))
        );
        assert_eq!(translate(&physical, 0, VA), Err(WalkError::InvalidRoot(0)));
        assert_eq!(
            translate(&physical, root, 0x1000),
            Err(WalkError::NotKernelPointer(0x1000))
        );
        set_entry(
            &mut physical.pages,
            ROOT,
            (VA >> 39) & 0x1FF,
            0xA000 | 0x80 | 0x3,
        );
        assert_eq!(
            translate(&physical, root, VA),
            Err(WalkError::InvalidLargePml4)
        );
    }

    #[test]
    fn read_virtual_spans_pages() {
        let (mut physical, root) = mapping();
        physical.pages.get_mut(&0xD000).unwrap().fill(0xAA);
        let next_pt_index = ((VA >> 12) & 0x1FF) + 1; // 紧随其后的 4 KiB 页
        set_entry(&mut physical.pages, 0xC000, next_pt_index, 0xE000 | 0x3);
        let mut next = [0xBBu8; 4096];
        next[0] = 0xCC;
        physical.pages.insert(0xE000, next);
        let bytes = read_virtual(&physical, root, VA + 0xFF8, 16).unwrap();
        assert_eq!(&bytes[..8], &[0xAA; 8]);
        assert_eq!(bytes[8], 0xCC);
        assert_eq!(&bytes[9..], &[0xBB; 7]);
        assert!(read_virtual(&physical, root, VA, 0).is_err());
        assert!(read_virtual(&physical, root, VA, 65537).is_err());
    }

    fn fake_image() -> Vec<u8> {
        let mut image = vec![0u8; 4096];
        image[0] = b'M';
        image[1] = b'Z';
        image[60..64].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(&0x0000_4550u32.to_le_bytes());
        image[0x80 + 24..0x80 + 26].copy_from_slice(&0x020Bu16.to_le_bytes());
        image[0x80 + 8..0x80 + 12].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        image[0x80 + 80..0x80 + 84].copy_from_slice(&0x2000u32.to_le_bytes());
        image
    }

    const BASE: u64 = 0xFFFF_8000_0001_0000;

    fn stub_page(root_candidate: u64) -> [u8; 4096] {
        let mut page = [0u8; 4096];
        page[0..8].copy_from_slice(&LOW_STUB_SIGNATURE.to_le_bytes());
        page[160..168].copy_from_slice(&root_candidate.to_le_bytes());
        page
    }

    /// 把 BASE 指向 `frame` 页(content 为数据页内容),走查链的中间表由
    /// `tables` 指定(不同候选根必须用不同中间表,否则相互覆盖)。
    fn map_base(
        pages: &mut HashMap<u64, [u8; 4096]>,
        root: u64,
        tables: [u64; 3],
        frame: u64,
        data: [u8; 4096],
    ) {
        set_entry(pages, root, (BASE >> 39) & 0x1FF, tables[0] | 0x3);
        set_entry(pages, tables[0], (BASE >> 30) & 0x1FF, tables[1] | 0x3);
        set_entry(pages, tables[1], (BASE >> 21) & 0x1FF, tables[2] | 0x3);
        set_entry(pages, tables[2], (BASE >> 12) & 0x1FF, frame | 0x3);
        pages.insert(frame, data);
    }

    #[test]
    fn discover_root_validates_against_disk_image() {
        let image = fake_image();
        let fingerprint = PeFingerprint::from_image(&image).unwrap();
        let mut pages = HashMap::new();
        let mut data = [0u8; 4096];
        data.copy_from_slice(&image);
        map_base(&mut pages, ROOT, [0xA000, 0xB000, 0xC000], 0xD000, data);
        map_base(
            &mut pages,
            0xA100,
            [0xA200, 0xA300, 0xA400],
            0xE000,
            [0u8; 4096],
        ); // 错误内容的第二候选
        pages.insert(0x1000, stub_page(ROOT));
        pages.insert(0x2000, stub_page(0xA100));
        let physical = MockPhysical { pages };
        let report = discover_root(&physical, BASE, &fingerprint);
        assert_eq!(report.low_stubs, vec![0x1000, 0x2000]);
        assert_eq!(report.unique_root().unwrap(), ROOT);
        assert!(report.events.iter().any(|e| e.contains("validated")));
        assert!(report.events.iter().any(|e| e.contains("rejected")));
        assert_eq!(report.scanned_pages, 255);
    }

    #[test]
    fn discover_root_falls_back_to_full_stub_scan() {
        let image = fake_image();
        let fingerprint = PeFingerprint::from_image(&image).unwrap();
        let mut pages = HashMap::new();
        let mut data = [0u8; 4096];
        data.copy_from_slice(&image);
        map_base(&mut pages, ROOT, [0xA000, 0xB000, 0xC000], 0xD000, data);
        let mut stub = stub_page(0x5000); // 偏移 160 指向坏根
        stub[0x200..0x208].copy_from_slice(&ROOT.to_le_bytes()); // 回退扫描命中
        pages.insert(0x1000, stub);
        let physical = MockPhysical { pages };
        let report = discover_root(&physical, BASE, &fingerprint);
        assert_eq!(report.unique_root().unwrap(), ROOT);
        assert!(
            report
                .events
                .iter()
                .any(|e| e.contains("strictHeader=true"))
        );
    }

    #[test]
    fn low_stub_candidates_filter_and_dedup() {
        let mut page = [0u8; 4096];
        page[0..8].copy_from_slice(&LOW_STUB_SIGNATURE.to_le_bytes());
        page[8..16].copy_from_slice(&0x9000u64.to_le_bytes());
        page[16..24].copy_from_slice(&0x9000u64.to_le_bytes()); // 重复
        page[24..32].copy_from_slice(&0xF000_0000_0000_1000u64.to_le_bytes()); // 高位非零
        page[32..40].copy_from_slice(&0x800u64.to_le_bytes()); // 小于 4096
        page[40..48].copy_from_slice(&0x1234_5678_9ABCu64.to_le_bytes()); // 掩码帧
        assert_eq!(
            low_stub_root_candidates(&page),
            vec![0x9000, 0x1234_5678_9000]
        );
        let mut bad = page;
        bad[0] ^= 0xFF;
        assert!(low_stub_root_candidates(&bad).is_empty());
        assert!(low_stub_root_candidates(&page[..64]).is_empty());
    }
}
