//! kmd 功率墙原子流:`set-power-command --kmd` 的执行核心。
//!
//! 一个调用完成全周期,漏洞驱动(pmxdrv)**只在内存里停留一个命令周期**,
//! 按 PowerRoot 武装态自动分双臂:
//!
//! ```text
//! 服务注册(PMXDRV_KMD,先清残留)→ 启动 → 走查+布局探测(全自动,静态
//! probe 本机在役镜像)→ GPU 表链走查 → 臂选择:
//!   root 臂(身份门过 = 移动/board 配置形态):
//!     写 UPPER(单 u32,物理帧,读回校验)→ 原生 tgp 写(窗随 UPPER)
//!   board 臂(身份门全拒 = 桌面形态,PowerRoot 未武装,见 kmd/board.rs):
//!     活体窗三元组定位 Board 控制表(root 8 页 + 指针一跳 + 候选页池邻域)
//!     → 逐候选探测:写窗 max(读回)→ percent 到达验证(多假设;桌面
//!     range GET 读静态 info 行,percent 读回 ≥ 目标是唯一可靠判据)
//!     → 不到达即回滚+恢复(每轮自愈)→ percent 安全线写 current
//!     (0xAD95F5ED;Turing 上 watt SET 0xAFFC2279 毒,本臂绝不触碰)
//! → 租约写(回显面,最后,两臂共用)→ 复验 → 服务停止+注销
//! ```
//!
//! 安全设计:
//! - 入口要求 `--force`(调用方责任线,与 set-power-command 一致);
//! - 绝对上限 500 W 硬拒(任何旗标都不过);
//! - root 臂定位门:init==1 ∧ key<0x40 ∧ UPPER∈[50W,500W](防误配对象);
//! - board 臂定位门:三元组候选 ≤8(读回+帧校验)+ 逐候选"写-percent
//!   到达验-回滚+恢复"探测,胜出行保持抬升;无一到达全回滚拒写;
//! - board 臂窗内目标直接拒(不需要内核写,percent/NVML 即可);
//! - 写后读回不一致 → 立即回滚到存档值并报错;
//! - 任一步失败 → 已注册的服务照常注销(finally 语义),不留残余。
//!
//! 前置(调用方环境):管理员令牌;显卡低电压锁已解(影响负载能否吃满,
//! 不影响本流程)。UPPER/Board max 写均为易失(重启回落)——这正是"每开机
//! 一跑"的单命令形态与安全性的互相成全。

use quick_error::quick_error;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::board;
use super::layout_probe::{NvlddmkmLayout, probe as probe_layout};
use super::pagewalk::{
    LoadedModule, PeFingerprint, PhysicalMemory, WalkError, discover_root, find_loaded_module,
    read_virtual,
};
use super::pmxdrv::PmxDrv;

quick_error! {
    /// kmd 功率墙流错误。
    #[derive(Debug, Clone)]
    pub enum KmdPowerError {
        Service(msg: String) {
            display("服务管理失败: {msg}")
        }
        Walk(err: WalkError) {
            display("内核走查失败: {err}")
        }
        Layout(msg: String) {
            display("布局推导失败: {msg}")
        }
        Locate(msg: String) {
            display("PowerRoot 定位失败: {msg}")
        }
        Write(msg: String) {
            display("UPPER 写失败: {msg}")
        }
        /// 失败现场全量透传:错误 + 完整探测 steps(CLI 错误通道原样展示 ——
        /// 探测细节是判读的一手证据,不允许只报结论)。
        Failed(msg: String) {
            display("{msg}")
        }
    }
}

/// 单 u32 物理读(PTE 页缓存包一层,把走查的 8 字节重复合访压掉);
/// 4096 载荷直通。map→拷贝→unmap 内联,传输窗口即用即弃。
struct CachedPhys {
    drv: PmxDrv,
    cache: RefCell<HashMap<u64, [u8; 4096]>>,
}

impl CachedPhys {
    fn new(drv: PmxDrv) -> Self {
        Self {
            drv,
            cache: RefCell::new(HashMap::new()),
        }
    }
}

impl PhysicalMemory for CachedPhys {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), super::pagewalk::PhysError> {
        use super::pagewalk::PhysError;
        if out.is_empty() || out.len() > 4096 {
            return Err(PhysError::BadLength(out.len()));
        }
        let frame = pa & !0xFFF;
        let off = (pa - frame) as usize;
        // 页内跨界的 4096 载荷读不支持(与 pmxdrv 传输约束一致)
        if off + out.len() > 4096 {
            return Err(PhysError::BadLength(out.len()));
        }
        // 8 字节(PTE)走缓存;其余直通。两路都落到一次 map→copy→unmap。
        let cached = out.len() == 8;
        if cached && let Some(page) = self.cache.borrow().get(&frame) {
            out.copy_from_slice(&page[off..off + 8]);
            return Ok(());
        }
        let mut page = [0u8; 4096];
        let mapped = self
            .drv
            .map_physical(frame, 1)
            .map_err(|_| PhysError::Unreadable(pa))?;
        let window = unsafe { std::slice::from_raw_parts(mapped as *const u8, 4096) };
        page.copy_from_slice(window);
        let _ = self.drv.unmap_physical(mapped);
        if cached {
            let mut map = self.cache.borrow_mut();
            if map.len() > 16_384 {
                map.clear();
            }
            map.insert(frame, page);
        }
        out.copy_from_slice(&page[off..off + out.len()]);
        Ok(())
    }
}

/// 定位到的 PowerRoot 及身份现场。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootInfo {
    pub major_va: u64,
    pub root_va: u64,
    pub entry_index: u32,
    pub frame_root: u64,
    pub frame_upper: u64,
    pub init: u8,
    pub elig: u8,
    pub amount_active: u8,
    pub base: u32,
    pub amount: u32,
    pub key: u8,
    pub lower: u32,
    pub upper: u32,
}

/// 破解臂:root(移动/board 配置形态,PowerRoot 已武装,写 UPPER)或
/// board(桌面形态,PowerRoot 未武装,写 Board 控制表滑条窗 max)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KmdArm {
    Root,
    Board,
}

impl KmdArm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Board => "board",
        }
    }
}

/// Board 臂结果(全部为已验证事实)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardWindowOutcome {
    /// 三元组所在页(页对齐 VA)与物理帧。
    pub page_va: u64,
    pub frame: u64,
    /// 页内偏移:max=写目标槽(主槽),cur/def/min=活体锚(cur=None =
    /// 无 control 槽的行)。
    pub max_off: usize,
    /// 本次抬升写入的全部 max 槽(行 = 1 槽;宽记录页 = 全部镜像槽)。
    pub max_offs: Vec<usize>,
    /// 行类型:control(活体控制行)/ info(静态策略行)/ wide(宽记录页)。
    pub row_kind: String,
    pub cur_off: Option<usize>,
    pub def_off: usize,
    pub min_off: Option<usize>,
    /// 写前活体窗(GET 面)。
    pub live_current_mw: u32,
    pub live_default_mw: u32,
    pub live_max_mw: u32,
    pub live_min_mw: Option<u32>,
    pub max_before_mw: u32,
    pub max_after_mw: u32,
    /// 窗跟验证通过(percent 读回到达目标;兼容保留名,oracle 详见
    /// board_arm 文档)。
    pub window_followed: bool,
    /// 探测胜出的候选数(1 = 恰一行的 max 抬升后 percent 能顶到目标)。
    pub followers: usize,
    /// percent 写(percent 安全面;None = 失败,steps 有说明)。
    pub percent_written: Option<u32>,
    /// percent 后控制值读回。
    pub current_readback_mw: Option<u32>,
    /// 扫查到的候选总数(放行时必为 1)。
    pub candidates_seen: usize,
}

/// 全周期结果(全部为已验证事实)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmdSetWallOutcome {
    pub arm: KmdArm,
    pub layout_summary: String,
    /// root 臂现场(board 臂为 None)。
    pub root: Option<RootInfo>,
    /// board 臂现场(root 臂为 None)。
    pub board: Option<BoardWindowOutcome>,
    /// 墙字段前值/后值(root=UPPER,board=窗 max)。
    pub wall_before_mw: u32,
    pub wall_after_mw: u32,
    pub tgp_written_w: u32,
    pub tgp_write_note: String,
    pub lease_written_mw: Option<u32>,
    pub lease_note: String,
    pub service: String,
    /// 各步骤日志(诊断/留档)。
    pub steps: Vec<String>,
}

/// 绝对上限:任何旗标都不过(安全设计,防手滑)。
pub const ABSOLUTE_WALL_CAP_MW: u32 = 500_000;

const SERVICE_NAME: &str = "PMXDRV_KMD";

// ---------------------------------------------------------------- 服务管理

/// 注册并启动内核服务;同名残留先停后删(本命令专属名,清理安全)。
fn service_register_and_start(binary_path: &Path) -> Result<(), KmdPowerError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Services::CloseServiceHandle;
    use windows_sys::Win32::System::Services::{
        ControlService, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, SC_HANDLE,
        SC_MANAGER_CREATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START,
        SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER, StartServiceW,
    };
    const SERVICE_ALL_ACCESS: u32 = 0xF01FF;

    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    // ImagePath 用 \??\ + 普通绝对路径(canonicalize 的 \?\ 前缀会叠加成非法名)
    let abs = if binary_path.is_absolute() {
        binary_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| KmdPowerError::Service(format!("cwd 解析失败: {e}")))?
            .join(binary_path)
    };
    let path_w = wide(&format!(r"\??\{}", abs.to_string_lossy()));
    let name_w = wide(SERVICE_NAME);
    let mgr = unsafe {
        OpenSCManagerW(
            std::ptr::null(),
            std::ptr::null(),
            SC_MANAGER_CREATE_SERVICE,
        )
    };
    if mgr.is_null() {
        return Err(KmdPowerError::Service(format!(
            "OpenSCManager 失败(需管理员): win32 {}",
            unsafe { GetLastError() }
        )));
    }
    let existing: SC_HANDLE = unsafe { OpenServiceW(mgr, name_w.as_ptr(), SERVICE_ALL_ACCESS) };
    if !existing.is_null() {
        unsafe {
            ControlService(existing, SERVICE_CONTROL_STOP, std::ptr::null_mut());
            DeleteService(existing);
            CloseServiceHandle(existing);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let svc = unsafe {
        CreateServiceW(
            mgr,
            name_w.as_ptr(),
            name_w.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if svc.is_null() {
        let err = unsafe { GetLastError() };
        unsafe { CloseServiceHandle(mgr) };
        return Err(KmdPowerError::Service(format!(
            "CreateService 失败: win32 {err}"
        )));
    }
    if unsafe { StartServiceW(svc, 0, std::ptr::null()) } == 0 {
        let err = unsafe { GetLastError() };
        unsafe {
            DeleteService(svc);
            CloseServiceHandle(svc);
        }
        if err == 2 {
            // ERROR_FILE_NOT_FOUND:内核取不到原始卷的镜像 → 复制 System32 重试一次
            if let Ok(fallback) = fallback_copy(binary_path) {
                let path2_w = wide(&format!(r"\??\{}", fallback.to_string_lossy()));
                let svc2 = unsafe {
                    CreateServiceW(
                        mgr,
                        name_w.as_ptr(),
                        name_w.as_ptr(),
                        SERVICE_ALL_ACCESS,
                        SERVICE_KERNEL_DRIVER,
                        SERVICE_DEMAND_START,
                        SERVICE_ERROR_NORMAL,
                        path2_w.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                };
                if !svc2.is_null() && unsafe { StartServiceW(svc2, 0, std::ptr::null()) } != 0 {
                    unsafe {
                        CloseServiceHandle(svc2);
                        CloseServiceHandle(mgr);
                    }
                    return Ok(());
                }
                if !svc2.is_null() {
                    unsafe {
                        DeleteService(svc2);
                        CloseServiceHandle(svc2);
                    }
                }
            }
            return Err(KmdPowerError::Service(format!(
                "StartService 失败(System32 回退后仍败): win32 {err}"
            )));
        }
        unsafe { CloseServiceHandle(mgr) };
        return Err(KmdPowerError::Service(format!(
            "StartService 失败(驱动装载被拒?): win32 {err}"
        )));
    }
    unsafe {
        CloseServiceHandle(svc);
        CloseServiceHandle(mgr);
    }
    Ok(())
}

/// System32\drivers 回退复制(内核取镜像的可靠卷)。
fn fallback_copy(binary_path: &Path) -> Result<PathBuf, KmdPowerError> {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let dst = Path::new(&windir).join(r"System32\drivers").join(
        binary_path
            .file_name()
            .ok_or_else(|| KmdPowerError::Service("驱动路径无文件名".into()))?,
    );
    std::fs::copy(binary_path, &dst)
        .map_err(|e| KmdPowerError::Service(format!("System32 回退复制失败: {e}")))?;
    Ok(dst)
}

/// 停止并注销服务(finally 语义:必须执行)。返回是否确认 STOPPED ——
/// 驱动可能拒绝卸载(引用计数/驱动自身不支持二次卸载),此时服务条目已
/// 标记删除,重启后消失;返回 false 供步骤日志诚实上报。
fn service_stop_and_delete() -> bool {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, ControlService, DeleteService, OpenSCManagerW, OpenServiceW,
        QueryServiceStatus, SC_HANDLE, SERVICE_CONTROL_STOP, SERVICE_STATUS,
    };
    const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
    const SERVICE_STOPPED: u32 = 1;
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let name_w = wide(SERVICE_NAME);
    let mgr = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SERVICE_ALL_ACCESS) };
    if mgr.is_null() {
        return false;
    }
    let svc: SC_HANDLE = unsafe { OpenServiceW(mgr, name_w.as_ptr(), SERVICE_ALL_ACCESS) };
    let mut stopped = false;
    if !svc.is_null() {
        unsafe {
            let mut status: SERVICE_STATUS = std::mem::zeroed();
            ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
            for _ in 0..20 {
                let mut s: SERVICE_STATUS = std::mem::zeroed();
                if QueryServiceStatus(svc, &mut s) != 0 && s.dwCurrentState == SERVICE_STOPPED {
                    stopped = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            DeleteService(svc);
            CloseServiceHandle(svc);
        }
    }
    unsafe { CloseServiceHandle(mgr) };
    stopped
}

/// PMXDRV_KMD 服务是否在(本命令专属注册名)。
fn service_exists() -> bool {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_HANDLE,
    };
    const SERVICE_QUERY_STATUS: u32 = 0x4;
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let mgr = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SERVICE_QUERY_STATUS) };
    if mgr.is_null() {
        return false;
    }
    let svc: SC_HANDLE =
        unsafe { OpenServiceW(mgr, wide(SERVICE_NAME).as_ptr(), SERVICE_QUERY_STATUS) };
    let exists = !svc.is_null();
    if !svc.is_null() {
        unsafe { CloseServiceHandle(svc) };
    }
    unsafe { CloseServiceHandle(mgr) };
    exists
}

// ---------------------------------------------------------------- 走查+定位

/// 连接通道:模块定位 → 布局探测(对本机在役镜像)→ 页表根。
fn connect_lane() -> Result<(LoadedModule, CachedPhys, u64, NvlddmkmLayout), KmdPowerError> {
    let module = find_loaded_module("nvlddmkm.sys")
        .map_err(|e| KmdPowerError::Locate(format!("nvlddmkm 不在系统模块列表: {e}")))?;
    let img = std::fs::read(&module.path).map_err(|e| {
        KmdPowerError::Layout(format!("读在役镜像 {} 失败: {e}", module.path.display()))
    })?;
    let layout =
        probe_layout(&img).map_err(|e| KmdPowerError::Layout(format!("fail-closed: {e:?}")))?;
    let fingerprint = PeFingerprint::from_image(&img)
        .map_err(|e| KmdPowerError::Layout(format!("在役镜像 PE 头无效: {e:?}")))?;
    let drv = PmxDrv::connect()
        .map_err(|e| KmdPowerError::Locate(format!("PMXDRV 设备打开失败(服务须运行): {e}")))?;
    let phys = CachedPhys::new(drv);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    let walk_root = discovery
        .unique_root()
        .map_err(|e| KmdPowerError::Locate(format!("走查根不唯一: {e}")))?;
    Ok((module, phys, walk_root, layout))
}

fn rd_u64(phys: &CachedPhys, root: u64, va: u64) -> Option<u64> {
    read_virtual(phys, root, va, 8)
        .ok()
        .map(|b| u64::from_le_bytes(b[0..8].try_into().unwrap()))
}

fn rd_u32(phys: &CachedPhys, root: u64, va: u64) -> Option<u32> {
    read_virtual(phys, root, va, 4)
        .ok()
        .map(|b| u32::from_le_bytes(b[0..4].try_into().unwrap()))
}

fn is_kernel_va(v: u64) -> bool {
    (0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&v)
}

/// GPU 表链一个条目(已解引用,未做任何身份过滤)。
struct ChainEntry {
    index: u32,
    major_va: u64,
    root_va: u64,
}

/// 全局槽 → state → GPU 表 → 逐条目 Major/root(只验内核指针形,不过滤;
/// root 臂身份门与 board 臂 root 锚共用这一段读链)。
fn walk_gpu_chain(
    phys: &CachedPhys,
    walk_root: u64,
    layout: &NvlddmkmLayout,
    module_base: u64,
) -> Result<Vec<ChainEntry>, KmdPowerError> {
    let state = rd_u64(phys, walk_root, module_base + layout.global_slot_rva)
        .filter(|v| is_kernel_va(*v))
        .ok_or_else(|| KmdPowerError::Locate("全局槽不是内核指针".into()))?;
    let table = rd_u64(phys, walk_root, state + u64::from(layout.state_table_off))
        .filter(|v| is_kernel_va(*v))
        .ok_or_else(|| KmdPowerError::Locate("GPU 表指针无效".into()))?;
    let count = rd_u32(phys, walk_root, table + u64::from(layout.table_count_off)).unwrap_or(0);
    if count == 0 || count > 32 {
        return Err(KmdPowerError::Locate(format!("GPU 表 count 异常: {count}")));
    }
    let mut entries = Vec::new();
    for i in 0..count {
        let Some(major) = rd_u64(
            phys,
            walk_root,
            table
                + u64::from(layout.entry_major_off)
                + u64::from(i) * u64::from(layout.entry_stride),
        )
        .filter(|v| is_kernel_va(*v)) else {
            continue;
        };
        let Some(root_va) = rd_u64(phys, walk_root, major + u64::from(layout.major_root_off))
            .filter(|v| is_kernel_va(*v))
        else {
            continue;
        };
        entries.push(ChainEntry {
            index: i,
            major_va: major,
            root_va,
        });
    }
    Ok(entries)
}

/// root 臂定位失败分类:Unarmed = 链路通但全部条目未过身份门(桌面形态
/// 预期:构造期无 board 配置对象,init 永不置位)→ 触发 board 臂回退;
/// Fatal = 链路/择一本身断了(count 异常、表指针无效、多候选歧义)。
enum RootLocateFail {
    Unarmed,
    Fatal(KmdPowerError),
}

/// root 臂定位 PowerRoot。身份门:init=1 ∧ key<0x40 ∧ UPPER∈[10W,500W]
/// (硬域,fail-closed)。**D 状态感知**:非 D1(如 D2=55W)时 UPPER 不是
/// 滑条顶——候选收集后用活体墙(0x67F31384 控制表 current,秒级新鲜)精确
/// 匹配择一;无活体参照时恰一候选才接受,多候选歧义即拒。
fn locate_root(
    phys: &CachedPhys,
    walk_root: u64,
    layout: &NvlddmkmLayout,
    chain: &[ChainEntry],
    live_wall_mw: Option<u32>,
    steps: &mut Vec<String>,
) -> Result<RootInfo, RootLocateFail> {
    let mut candidates: Vec<RootInfo> = Vec::new();
    for entry in chain {
        let root_va = entry.root_va;
        let init =
            rd_u32(phys, walk_root, root_va + u64::from(layout.root_init_off)).unwrap_or(0xFF);
        let key = rd_u32(phys, walk_root, root_va + u64::from(layout.root_key_off)).unwrap_or(0xFF);
        let upper = rd_u32(phys, walk_root, root_va + u64::from(layout.root_upper_off));
        if init & 0xFF != 1 || key & 0xFF >= 0x40 {
            continue;
        }
        let Some(upper) = upper.filter(|u| (10_000..=ABSOLUTE_WALL_CAP_MW).contains(u)) else {
            continue;
        };
        let lower = rd_u32(phys, walk_root, root_va + u64::from(layout.root_lower_off));
        if lower.is_some_and(|l| l > upper) {
            continue;
        }
        let frame_root = read_virtual(phys, walk_root, root_va, 4)
            .ok()
            .and_then(|_| super::pagewalk::translate(phys, walk_root, root_va).ok())
            .map(|pa| pa & !0xFFF)
            .unwrap_or(0);
        let frame_upper = read_virtual(
            phys,
            walk_root,
            root_va + u64::from(layout.root_upper_off),
            4,
        )
        .ok()
        .and_then(|_| {
            super::pagewalk::translate(phys, walk_root, root_va + u64::from(layout.root_upper_off))
                .ok()
        })
        .map(|pa| pa & !0xFFF)
        .unwrap_or(0);
        candidates.push(RootInfo {
            major_va: entry.major_va,
            root_va,
            entry_index: entry.index,
            frame_root,
            frame_upper,
            init: (init & 0xFF) as u8,
            elig: rd_u32(
                phys,
                walk_root,
                root_va + u64::from(layout.root_init_off + 1),
            )
            .unwrap_or(0xFF) as u8,
            amount_active: rd_u32(
                phys,
                walk_root,
                root_va + u64::from(layout.root_init_off + 2),
            )
            .unwrap_or(0xFF) as u8,
            base: rd_u32(phys, walk_root, root_va + u64::from(layout.root_base_off))
                .unwrap_or(u32::MAX),
            amount: rd_u32(phys, walk_root, root_va + u64::from(layout.root_amount_off))
                .unwrap_or(u32::MAX),
            key: (key & 0xFF) as u8,
            lower: lower.unwrap_or(u32::MAX),
            upper,
        });
    }
    if candidates.is_empty() {
        return Err(RootLocateFail::Unarmed);
    }
    // 择一:活体墙精确匹配 > 恰一候选 > 歧义拒绝
    if let Some(live) = live_wall_mw {
        let exact: Vec<_> = candidates.iter().filter(|c| c.upper == live).collect();
        if let Some(&c) = exact.first() {
            steps.push(format!(
                "活体墙 {live} mW 精确匹配 entry{}(候选 {} 个)",
                c.entry_index,
                candidates.len()
            ));
            return Ok(c.clone());
        }
    }
    if candidates.len() == 1 {
        steps.push("唯一身份门候选(无活体参照)".into());
        return Ok(candidates.remove(0));
    }
    Err(RootLocateFail::Fatal(KmdPowerError::Locate(format!(
        "{} 个候选歧义且活体墙无精确匹配(live={live_wall_mw:?})— 拒写",
        candidates.len()
    ))))
}

/// 单 u32 物理写 + 走查读回校验,不符即时回滚(root 臂 UPPER 与 board 臂
/// 窗 max 共用一条写原语)。
fn write_u32_phys(
    phys: &CachedPhys,
    walk_root: u64,
    va: u64,
    value: u32,
    restore: u32,
    label: &str,
    steps: &mut Vec<String>,
) -> Result<(), KmdPowerError> {
    let pa = super::pagewalk::translate(phys, walk_root, va).map_err(KmdPowerError::Walk)?;
    let frame = pa & !0xFFF;
    let off_in_page = (va & 0xFFF) as usize;
    let mapped = phys
        .drv
        .map_physical(frame, 1)
        .map_err(|e| KmdPowerError::Write(format!("{label} 帧映射失败: {e}")))?;
    unsafe {
        ((mapped + off_in_page as u64) as *mut u32).write_volatile(value);
    }
    phys.drv
        .unmap_physical(mapped)
        .map_err(|e| KmdPowerError::Write(format!("{label} 反映射失败: {e}")))?;
    if rd_u32(phys, walk_root, va) != Some(value) {
        if let Ok(m2) = phys.drv.map_physical(frame, 1) {
            unsafe {
                ((m2 + off_in_page as u64) as *mut u32).write_volatile(restore);
            }
            let _ = phys.drv.unmap_physical(m2);
        }
        return Err(KmdPowerError::Write(format!(
            "{label} 读回不符 — 已回滚到 {restore}"
        )));
    }
    steps.push(format!("{label} {restore} → {value} ✓ 读回一致"));
    Ok(())
}

/// Board 窗臂:root 未武装(桌面形态)时的破解路径 —— 目标是 Board 控制表
/// 滑条窗 max(percent/NVML 写路径的窗钳源头)。活体窗定位(扫查含候选页池
/// 邻域;控制行三元组优先,info 行 {max,default,min} 降级匹配殿后)→
/// **逐候选探测**:写 max(读回)→ **到达验证** —— nvidia-smi -pl 瓦特
/// 地面真值优先(拒绝时 stderr 的范围读数直接指认钳源),不可用退 percent
/// 多假设(2070 三轮实证:range GET 读静态 info 行,"GET 跟随"失灵)→
/// 不到达即回滚 max + 恢复 current(每轮自愈)→ 胜出行保持抬升(Turing 上
/// watt SET 0xAFFC2279 毒,**本臂绝不触碰 set_tgp_watt**)。返回(结果
/// 现场, 墙字段 VA 供终验)。
fn board_arm(
    phys: &CachedPhys,
    walk_root: u64,
    chain: &[ChainEntry],
    gpu: Option<&nvapi::hi::Gpu>,
    wall_mw: u32,
    steps: &mut Vec<String>,
) -> Result<(BoardWindowOutcome, u64), KmdPowerError> {
    steps.push(
        "root 臂身份门全拒(PowerRoot 未武装 = 桌面形态预期:构造期无 board 配置对象,init 永不置位)→ 切 Board 窗臂"
            .into(),
    );
    let Some(gpu) = gpu else {
        return Err(KmdPowerError::Locate(
            "Board 臂需要活体窗 GET(0x67F31384/0x8B3E7343):NVAPI GPU 不可达".into(),
        ));
    };
    let range = gpu
        .tgp_watt_range()
        .map_err(|e| KmdPowerError::Locate(format!("tgp 窗 GET 失败: {e}")))?
        .ok_or_else(|| {
            KmdPowerError::Locate("驱动不暴露 tgp 窗(range=None)— Board 臂缺活体锚,拒".into())
        })?;
    let (Some(default_mw), Some(max_mw)) = (range.default_mw, range.max_mw) else {
        return Err(KmdPowerError::Locate(format!(
            "活体窗不完整(default={:?} max={:?})— Board 臂拒",
            range.default_mw, range.max_mw
        )));
    };
    let current_mw = gpu
        .tgp_watt_status()
        .ok()
        .flatten()
        .and_then(|s| s.current_mw)
        .unwrap_or(default_mw);
    let live = board::BoardLiveValues {
        current_mw,
        default_mw,
        max_mw,
        min_mw: range.min_mw,
    };
    steps.push(format!(
        "活体窗: current={current_mw} default={default_mw} max={max_mw} min={:?}(GET 安全面)",
        live.min_mw
    ));
    if !(10_000..=ABSOLUTE_WALL_CAP_MW).contains(&max_mw) {
        return Err(KmdPowerError::Locate(format!(
            "活体窗顶 {max_mw} 超硬域 [10W,500W] — 不像 Board 窗,拒"
        )));
    }
    if wall_mw <= max_mw {
        return Err(KmdPowerError::Locate(format!(
            "目标 {wall_mw} ≤ 活体窗顶 {max_mw}:窗内目标不需要内核写,直接 set-public-tgp-percent / NVML"
        )));
    }
    let Some(root_va) = chain.first().map(|e| e.root_va) else {
        return Err(KmdPowerError::Locate(
            "GPU 链为空 — Board 臂无 root 锚".into(),
        ));
    };
    let scan = board::locate_board_window_candidates(phys, walk_root, root_va, &live);
    steps.push(format!(
        "Board 窗扫查: 可读 {} 页 / 跳过 {} 页 / 候选 {}",
        scan.pages_scanned,
        scan.pages_unreadable,
        scan.candidates.len()
    ));
    for note in &scan.notes {
        steps.push(format!("  [scan] {note}"));
    }
    let cands =
        board::probeable_candidates(&scan, board::PROBE_CAP).map_err(KmdPowerError::Locate)?;
    // 候选校验:max 槽读回必须等于活体窗顶 + 帧翻译可用,不过者剔除;
    // 控制行(cur Some)优先,info 行(静态策略行,无 control 槽)殿后
    let mut valid: Vec<board::BoardWindowCandidate> = Vec::new();
    for cand in cands {
        let max_va = cand.page_va + cand.hit.max_off as u64;
        if rd_u32(phys, walk_root, max_va) == Some(live.max_mw) && cand.frame != 0 {
            valid.push(cand.clone());
        } else {
            steps.push(format!(
                "候选剔除: 页 {:#x} max@+{:#x}(读回/帧校验不过)",
                cand.page_va, cand.hit.max_off
            ));
        }
    }
    valid.sort_by_key(|c| c.hit.cur_off.is_none());
    let mut targets: Vec<ProbeTarget> = valid.into_iter().map(ProbeTarget::Row).collect();

    // 宽记录页候选(行候选之后):2070/3060 共现实证 —— 功率通道控制表的
    // 行内 {min, default, max} 各 2-3 副本、步距 0x4C/0x50(2070: 0xAE0/
    // 0xB30/0xB80;3060: 0x8F0/0x93C/0x988),远超行判据的 0x20 聚拢;
    // 钳源若读这张表,页内全部 max 槽必须同抬(镜像一致)。
    {
        let (worklist, _) = board::scan_domain_pages(phys, walk_root, root_va);
        let co =
            board::value_cooccurrence_scan(phys, walk_root, &worklist, &live, board::PAGE_BUDGET);
        let row_pages: std::collections::HashSet<u64> =
            targets.iter().map(|t| t.page_va()).collect();
        let mut wide_added = 0usize;
        for hit in &co {
            if targets.len() >= board::PROBE_CAP {
                break;
            }
            if row_pages.contains(&hit.page_va) {
                continue; // 行候选已覆盖该页
            }
            let Some((_, offs)) = hit.values.iter().find(|(v, _)| *v == live.max_mw) else {
                continue;
            };
            let mut max_offs: Vec<usize> = offs.clone();
            max_offs.sort_unstable();
            max_offs.truncate(16);
            let frame = super::pagewalk::translate(phys, walk_root, hit.page_va)
                .map(|pa| pa & !0xFFF)
                .unwrap_or(0);
            if frame == 0 {
                continue;
            }
            targets.push(ProbeTarget::Wide {
                page_va: hit.page_va,
                frame,
                max_offs,
            });
            wide_added += 1;
        }
        steps.push(format!(
            "宽行候选: 共现 {} 页(≥2 活体值),纳入探测 {wide_added} 页(排在行候选之后)",
            co.len()
        ));
    }

    // 逐目标探测:写全部 max 槽 → **到达验证**。oracle 优先级:nvidia-smi
    // -pl 瓦特地面真值(接受 = 窗真抬了,current 同时落位;拒绝时的范围
    // 读数直接指认钳源)→ percent 多假设(安全面)。失败轮回滚全部槽 +
    // 恢复 current,零残留。
    let mut success: Option<(ProbeTarget, String, Option<u32>)> = None;
    for (round, target) in targets.iter().enumerate() {
        if success.is_some() {
            break;
        }
        steps.push(format!(
            "探测 {}/{}: {}",
            round + 1,
            targets.len(),
            target.label()
        ));
        let slots = target.max_slots();
        let mut written: Vec<u64> = Vec::new();
        let mut write_err = None;
        for va in &slots {
            match write_u32_phys(
                phys,
                walk_root,
                *va,
                wall_mw,
                live.max_mw,
                "Board max",
                steps,
            ) {
                Ok(()) => written.push(*va),
                Err(e) => {
                    write_err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = write_err {
            for va in &written {
                let _ = write_u32_phys(
                    phys,
                    walk_root,
                    *va,
                    live.max_mw,
                    wall_mw,
                    "Board max 回滚",
                    steps,
                );
            }
            steps.push(format!("  → 写入失败,跳过该目标({e})"));
            continue;
        }
        let (reached, ok_writes) = window_reach_probe(gpu, wall_mw, &live, steps);
        if let Some((how, rb)) = reached {
            steps.push(format!(
                "  → 窗跟验证 ✓({how} 读回到达 {wall_mw} mW)— 保持抬升"
            ));
            success = Some((target.clone(), how, rb));
        } else {
            steps.push("  → 各 oracle 读回均 < 目标(镜像/窗未跟)— 回滚 max".into());
            for va in &slots {
                write_u32_phys(
                    phys,
                    walk_root,
                    *va,
                    live.max_mw,
                    wall_mw,
                    "Board max 回滚",
                    steps,
                )?;
            }
            if ok_writes > 0 {
                current_restore(gpu, &live, steps);
            }
        }
    }
    let Some((primary, how, readback)) = success else {
        return Err(KmdPowerError::Write(
            "全部目标(行+宽页)探测后无一让窗跟 oracle 到达 — 已全部回滚。把失败输出里的 \
             nvidia-smi 拒绝行(范围读数)与 locate-trace 共现地图发回判读"
                .into(),
        ));
    };
    let max_va = primary.max_slots()[0];
    steps.push(format!(
        "Board 臂收束: {} 抬至 {wall_mw} ✓,current 经 {how} 顶到 {readback:?}",
        primary.label()
    ));
    Ok((
        BoardWindowOutcome {
            page_va: primary.page_va(),
            frame: primary.frame(),
            max_off: primary.primary_off(),
            max_offs: primary.max_offs(),
            row_kind: primary.kind().to_string(),
            cur_off: primary.cur_off(),
            def_off: primary.def_off(),
            min_off: primary.min_off(),
            live_current_mw: live.current_mw,
            live_default_mw: live.default_mw,
            live_max_mw: live.max_mw,
            live_min_mw: live.min_mw,
            max_before_mw: live.max_mw,
            max_after_mw: wall_mw,
            window_followed: true,
            followers: 1,
            percent_written: None,
            current_readback_mw: readback,
            candidates_seen: scan.candidates.len(),
        },
        max_va,
    ))
}

/// 一个探测目标:行候选(控制行/info 行,单 max 槽)或宽记录页(共现页,
/// 页内全部 max 槽同抬 —— 镜像必须一起动)。
#[derive(Debug, Clone)]
enum ProbeTarget {
    Row(board::BoardWindowCandidate),
    Wide {
        page_va: u64,
        frame: u64,
        max_offs: Vec<usize>,
    },
}

impl ProbeTarget {
    fn page_va(&self) -> u64 {
        match self {
            Self::Row(c) => c.page_va,
            Self::Wide { page_va, .. } => *page_va,
        }
    }

    fn frame(&self) -> u64 {
        match self {
            Self::Row(c) => c.frame,
            Self::Wide { frame, .. } => *frame,
        }
    }

    /// 本目标的全部 max 槽 VA(行 = 1 槽;宽页 = 全部镜像槽)。
    fn max_slots(&self) -> Vec<u64> {
        match self {
            Self::Row(c) => vec![c.page_va + c.hit.max_off as u64],
            Self::Wide {
                page_va, max_offs, ..
            } => max_offs.iter().map(|o| page_va + *o as u64).collect(),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Row(c) => format!(
                "[行] 页 {:#x}(帧 {:#x})max@+{:#x} cur@{:?} def@+{:#x} min@{:?} 跨度 {}B",
                c.page_va,
                c.frame,
                c.hit.max_off,
                c.hit.cur_off,
                c.hit.def_off,
                c.hit.min_off,
                c.hit.span
            ),
            Self::Wide {
                page_va,
                frame,
                max_offs,
            } => format!(
                "[宽页] 页 {:#x}(帧 {:#x})max 槽 {} 个 @{:?}(镜像同抬)",
                page_va,
                frame,
                max_offs.len(),
                max_offs
            ),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Row(c) if c.hit.cur_off.is_some() => "control",
            Self::Row(_) => "info",
            Self::Wide { .. } => "wide",
        }
    }

    fn primary_off(&self) -> usize {
        match self {
            Self::Row(c) => c.hit.max_off,
            Self::Wide { max_offs, .. } => max_offs.first().copied().unwrap_or(0),
        }
    }

    fn max_offs(&self) -> Vec<usize> {
        match self {
            Self::Row(c) => vec![c.hit.max_off],
            Self::Wide { max_offs, .. } => max_offs.clone(),
        }
    }

    fn cur_off(&self) -> Option<usize> {
        match self {
            Self::Row(c) => c.hit.cur_off,
            Self::Wide { .. } => None,
        }
    }

    fn def_off(&self) -> usize {
        match self {
            Self::Row(c) => c.hit.def_off,
            Self::Wide { .. } => 0,
        }
    }

    fn min_off(&self) -> Option<usize> {
        match self {
            Self::Row(c) => c.hit.min_off,
            Self::Wide { .. } => None,
        }
    }
}

/// percent 试探序列(整数 percent,ceil 保证换算值 ≥ 目标):100%
/// (percent-of-max 语义)→ 按旧窗顶折算 → 按 default 折算
/// (percent-of-default 语义)。驱动若钳 100% 或拒 >100,读回会说明;
/// 上限 500% 防极端比值。全程安全面 0xAD95F5ED。
fn percent_probe_percents(wall_mw: u32, live: &board::BoardLiveValues) -> Vec<u32> {
    let mut vals = vec![100u32];
    let by_max = (u64::from(wall_mw) * 100).div_ceil(u64::from(live.max_mw));
    let by_def = (u64::from(wall_mw) * 100).div_ceil(u64::from(live.default_mw));
    for p in [by_max, by_def] {
        let p = p.min(500) as u32;
        if !vals.contains(&p) {
            vals.push(p);
        }
    }
    vals
}

/// percent 到达探测(percent_reach 候补通道):逐假设写 → 读回 current,
/// ≥ 目标即成功。返回 (Some((percent, 读回)) = 到达; 成功写入次数)。
fn percent_reach_probe(
    gpu: &nvapi::hi::Gpu,
    wall_mw: u32,
    live: &board::BoardLiveValues,
    steps: &mut Vec<String>,
) -> (Option<(u32, Option<u32>)>, usize) {
    let mut ok_writes = 0usize;
    for p in percent_probe_percents(wall_mw, live) {
        match gpu.set_power_limits([nvapi::Percentage(p)]) {
            Ok(()) => {
                ok_writes += 1;
                let rb = gpu
                    .tgp_watt_status()
                    .ok()
                    .flatten()
                    .and_then(|s| s.current_mw);
                steps.push(format!(
                    "  percent {p}% → current 读回 {rb:?}(目标 {wall_mw})"
                ));
                if rb.is_some_and(|c| c >= wall_mw) {
                    return (Some((p, rb)), ok_writes);
                }
            }
            Err(e) => steps.push(format!("  percent {p}% 写失败:{e}")),
        }
    }
    (None, ok_writes)
}

/// 窗跟 oracle + current 写:NVIDIA-smi -pl(NVML 瓦特,**地面真值** ——
/// 接受即窗真的抬了,拒绝时 stderr 的范围读数直接指认钳源;成功时 current
/// 同时落位)→ 不可用时退 percent 多假设(安全面)。current 写成功次数
/// 供失败恢复判断。
fn window_reach_probe(
    gpu: &nvapi::hi::Gpu,
    wall_mw: u32,
    live: &board::BoardLiveValues,
    steps: &mut Vec<String>,
) -> (Option<(String, Option<u32>)>, usize) {
    let wall_w = (wall_mw / 1000).to_string();
    match std::process::Command::new("nvidia-smi")
        .args(["-pl", &wall_w])
        .output()
    {
        Ok(out) if out.status.success() => {
            let rb = gpu
                .tgp_watt_status()
                .ok()
                .flatten()
                .and_then(|s| s.current_mw);
            steps.push(format!(
                "  nvidia-smi -pl {wall_w} ✓ → current 读回 {rb:?}(目标 {wall_mw})"
            ));
            if rb.is_some_and(|c| c >= wall_mw) {
                return (Some((format!("nvidia-smi -pl {wall_w}"), rb)), 1);
            }
        }
        Ok(out) => {
            // nvidia-smi 的报错常走 stdout 而非 stderr —— 两路都抓,
            // 范围读数(out of range [min, max])是钳源窗口的直接证据
            let err = String::from_utf8_lossy(&out.stderr);
            let sout = String::from_utf8_lossy(&out.stdout);
            let msg = format!("{}{}", err.trim(), sout.trim());
            if msg.is_empty() {
                steps.push(format!(
                    "  nvidia-smi -pl 被拒(exit {:?},stdout/stderr 全空)",
                    out.status.code()
                ));
            } else {
                steps.push(format!(
                    "  nvidia-smi -pl 被拒:{msg}(这一行是钳源窗口的直接读数,失败也有判读价值)"
                ));
            }
        }
        Err(e) => steps.push(format!("  nvidia-smi 无法执行({e})— 退 percent 多假设")),
    }
    let (reached, ok_writes) = percent_reach_probe(gpu, wall_mw, live, steps);
    (
        reached.map(|(p, rb)| (format!("percent {p}%"), rb)),
        ok_writes,
    )
}

/// 失败候选的 current 恢复:nvidia-smi -pl 探测前瓦特 → 失败再走 percent
/// 双假设(percent-of-max 折算;current == default 时补 100%);读回 ±2W
/// 内算命中。
fn current_restore(gpu: &nvapi::hi::Gpu, live: &board::BoardLiveValues, steps: &mut Vec<String>) {
    let w = (live.current_mw / 1000).to_string();
    let pl_ok = std::process::Command::new("nvidia-smi")
        .args(["-pl", &w])
        .output()
        .is_ok_and(|out| out.status.success());
    if pl_ok {
        let rb = gpu
            .tgp_watt_status()
            .ok()
            .flatten()
            .and_then(|s| s.current_mw);
        if rb.is_some_and(|c| c.abs_diff(live.current_mw) <= 2_000) {
            steps.push(format!("  current 恢复 {rb:?}(nvidia-smi -pl {w})"));
            return;
        }
    }
    percent_restore(gpu, live, steps);
}

/// 失败候选的 current 恢复:按 percent-of-max 折算探针写回,current ==
/// default 时补 100% 假设(percent-of-default 语义);读回 ±2W 内算命中。
fn percent_restore(gpu: &nvapi::hi::Gpu, live: &board::BoardLiveValues, steps: &mut Vec<String>) {
    let mut vals =
        vec![((u64::from(live.current_mw) * 100).div_ceil(u64::from(live.max_mw))).min(100) as u32];
    if live.current_mw == live.default_mw && !vals.contains(&100) {
        vals.push(100);
    }
    for p in vals {
        if gpu.set_power_limits([nvapi::Percentage(p)]).is_ok() {
            let rb = gpu
                .tgp_watt_status()
                .ok()
                .flatten()
                .and_then(|s| s.current_mw);
            if rb.is_some_and(|c| c.abs_diff(live.current_mw) <= 2_000) {
                steps.push(format!("  current 恢复 {rb:?}(percent {p}%)"));
                return;
            }
        }
    }
    steps.push("  current 恢复未精确命中(读回 ≠ 探测前值)— 手动 nvidia-smi -pl 校正".into());
}

// ---------------------------------------------------------------- 公开入口

/// 全周期入口:`set-power-command --kmd` 的执行核心。
///
/// `wall_mw` 目标功率墙(mW);`pmxdrv_path` 驱动二进制(注册服务用)。
/// 失败时服务照常注销;UPPER 已写且读回不符则即时回滚。
///
/// # Errors
/// 任一步骤失败即 [`KmdPowerError`](步骤语义见变体)。
#[allow(clippy::too_many_lines)]
pub fn set_power_wall_kmd(
    wall_mw: u32,
    pmxdrv_path: &Path,
) -> Result<KmdSetWallOutcome, KmdPowerError> {
    if wall_mw > ABSOLUTE_WALL_CAP_MW {
        return Err(KmdPowerError::Write(format!(
            "目标 {wall_mw} mW 超绝对上限 {ABSOLUTE_WALL_CAP_MW}(硬拒,无旗标可过)"
        )));
    }
    if !pmxdrv_path.is_file() {
        return Err(KmdPowerError::Service(format!(
            "驱动二进制不存在: {}",
            pmxdrv_path.display()
        )));
    }

    // 1) 设备已在位(用户自管服务在跑)→ 复用实例;但 PMXDRV_KMD 存在即
    //    本命令上次崩溃的残留,认领所有权,流程结束清掉。
    //    否则注册+启动瞬时服务(残留自清)。
    let mut steps: Vec<String> = Vec::new();
    let registered = if PmxDrv::connect().is_ok() {
        steps.push("设备 \\\\.\\PMXDRV 已在位 - 复用运行中的实例".into());
        service_exists() // 残留认领
    } else {
        service_register_and_start(pmxdrv_path)?;
        steps.push(format!("服务 {SERVICE_NAME} 注册+启动 ✓"));
        true
    };

    // 2..7) 主流程;finally:自注册或崩溃残留才注销(用户自管服务不动)
    let result = set_power_wall_inner(wall_mw, &mut steps);
    if registered {
        if service_stop_and_delete() {
            steps.push(format!("服务 {SERVICE_NAME} 停止+注销 ✓"));
        } else {
            steps.push(format!(
                "服务 {SERVICE_NAME} 已标记删除,但驱动仍在运行(拒绝卸载)—— sc stop {SERVICE_NAME} 或重启清理;设备本身仍可复用"
            ));
        }
    }

    match result {
        Ok(mut o) => {
            o.steps = steps;
            Ok(o)
        }
        Err(e) => Err(KmdPowerError::Failed(format!(
            "{e}\n失败步骤留档(判读的一手证据):{}",
            steps.iter().map(|s| format!("\n  {s}")).collect::<String>()
        ))),
    }
}

fn set_power_wall_inner(
    wall_mw: u32,
    steps: &mut Vec<String>,
) -> Result<KmdSetWallOutcome, KmdPowerError> {
    // 2) 走查+布局探测
    let (module, phys, walk_root, layout) = connect_lane()?;
    steps.push(format!(
        "布局自动探测: 槽={:#x} state+{:#x} count=+{:#x} M→root={:#x} root_init={:#x}",
        layout.global_slot_rva,
        layout.state_table_off,
        layout.table_count_off,
        layout.major_root_off,
        layout.root_init_off
    ));
    steps.push(format!(
        "走查根 {walk_root:#x}(nvlddmkm @{:#x})",
        module.base
    ));

    // 3) 活体面(GET 全安全:0x8B3E7343 status / 0x67F31384 range)
    let gpu = first_hi_gpu();
    let live_wall = gpu
        .as_ref()
        .and_then(|g| g.tgp_watt_status().ok())
        .flatten()
        .and_then(|st| st.current_mw);
    steps.push(format!("活体墙(tgp control current)= {live_wall:?}"));

    let chain = walk_gpu_chain(&phys, walk_root, &layout, module.base)?;

    // 4) 臂选择:root 身份门全拒(桌面形态)= 自动回退 board 臂
    let (arm, root_info, board_outcome, field_va, tgp_written, tgp_note) =
        match locate_root(&phys, walk_root, &layout, &chain, live_wall, steps) {
            Ok(root) => {
                steps.push(format!(
                "PowerRoot @ {:#016X}(entry{}): init={} key={} UPPER={} LOWER={} base={} amount={}",
                root.root_va,
                root.entry_index,
                root.init,
                root.key,
                root.upper,
                root.lower,
                root.base,
                root.amount
            ));
                let upper_va = root.root_va + u64::from(layout.root_upper_off);
                if wall_mw != root.upper {
                    write_u32_phys(
                        &phys, walk_root, upper_va, wall_mw, root.upper, "UPPER", steps,
                    )?;
                } else {
                    steps.push(format!("UPPER 已是 {wall_mw}(幂等写跳过)"));
                }
                let (tgp_written, tgp_note) = root_arm_tgp_write(gpu.as_ref(), wall_mw, steps);
                (
                    KmdArm::Root,
                    Some(root),
                    None,
                    upper_va,
                    tgp_written,
                    tgp_note,
                )
            }
            Err(RootLocateFail::Unarmed) => {
                let (outcome, field_va) =
                    board_arm(&phys, walk_root, &chain, gpu.as_ref(), wall_mw, steps)?;
                (
                    KmdArm::Board,
                    None,
                    Some(outcome),
                    field_va,
                    0,
                    "board 臂:current 走 percent 安全写(毒 SET 0xAFFC2279 不触碰)".into(),
                )
            }
            Err(RootLocateFail::Fatal(e)) => return Err(e),
        };

    // 5) 租约写(回显面,最后;两臂共用)
    let (lease_written, lease_note) = match gpu.as_ref() {
        Some(gpu) => match gpu.set_power_command_checked(0, 0xFE, wall_mw) {
            Ok(report) => (
                Some(wall_mw),
                format!("租约 channel0 = {} mW ✓(checked 写)", report.applied),
            ),
            Err(_) => match gpu.set_power_command(0, 0xFE, wall_mw) {
                Ok(()) => (
                    Some(wall_mw),
                    "租约 channel0 直写 ✓(checked 拒绝,越过其包络)".into(),
                ),
                Err(e) => (None, format!("租约写失败(回显面仍旧值,可手动补): {e}")),
            },
        },
        None => (None, "租约写跳过:GPU 0 不可达".into()),
    };
    steps.push(lease_note.clone());

    // 6) 复验(两臂同一判据:墙字段仍 == 目标)
    let final_field = rd_u32(&phys, walk_root, field_va);
    if final_field != Some(wall_mw) {
        return Err(KmdPowerError::Write(format!(
            "复验失败:墙字段={final_field:?} ≠ {wall_mw}(被外部改写?)"
        )));
    }
    steps.push(format!("复验墙字段={} ✓", final_field.unwrap()));

    let wall_before_mw = match (&root_info, &board_outcome) {
        (Some(root), _) => root.upper,
        (_, Some(board)) => board.max_before_mw,
        (None, None) => unreachable!("两臂必有其一场"),
    };
    Ok(KmdSetWallOutcome {
        arm,
        layout_summary: format!(
            "slot={:#x} table+{:#x} root_init={:#x}",
            layout.global_slot_rva, layout.state_table_off, layout.root_init_off
        ),
        root: root_info,
        board: board_outcome,
        wall_before_mw,
        wall_after_mw: wall_mw,
        tgp_written_w: tgp_written,
        tgp_write_note: tgp_note,
        lease_written_mw: lease_written,
        lease_note,
        service: SERVICE_NAME.to_string(),
        steps: Vec::new(),
    })
}

/// root 臂原生 tgp 写(控制值 < 目标时才写;已达标绝不写 —— 防窗钳把控制
/// 压回)。board 臂不经过这里(Turing watt SET 毒)。
fn root_arm_tgp_write(
    gpu: Option<&nvapi::hi::Gpu>,
    wall_mw: u32,
    steps: &mut Vec<String>,
) -> (u32, String) {
    let tgp_w = wall_mw / 1000;
    let live_control = gpu
        .and_then(|g| g.tgp_watt_status().ok())
        .flatten()
        .and_then(|st| st.current_mw);
    let (written, note) = match live_control {
        Some(cur) if cur >= wall_mw => (
            0,
            format!("tgp 控制已 {cur} mW ≥ 目标 {wall_mw} —— 跳过写(防窗钳压低)"),
        ),
        Some(_) => match gpu.map(|g| g.set_tgp_watt(tgp_w, 2)) {
            Some(Ok(applied_mw)) if applied_mw >= wall_mw => (
                tgp_w,
                format!("tgp {tgp_w} W 写入 ✓(读回 {} W)", applied_mw / 1000),
            ),
            Some(Ok(applied_mw)) => (
                0,
                format!(
                    "tgp 写被窗钳回 {} W(< 目标)—— 窗未跟 UPPER,用 set-pwr-cur-limit 复查",
                    applied_mw / 1000
                ),
            ),
            Some(Err(e)) => (0, format!("tgp 写失败(UPPER 已生效,可手动补): {e}")),
            None => (0, "tgp 写跳过:GPU 0 不可达".into()),
        },
        None => (0, "tgp 写跳过:控制值不可读".into()),
    };
    steps.push(note.clone());
    (written, note)
}

/// 第一块卡(`PhysicalGpu::enumerate()[0]`,NVAPI 不可达时 None)。
fn first_hi_gpu() -> Option<nvapi::hi::Gpu> {
    let first = nvapi::PhysicalGpu::enumerate().ok()?.into_iter().next()?;
    Some(nvapi::hi::Gpu::new(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(max: u32, default: u32) -> board::BoardLiveValues {
        board::BoardLiveValues {
            current_mw: default,
            default_mw: default,
            max_mw: max,
            min_mw: None,
        }
    }

    #[test]
    fn percent_probe_values_ceil_and_dedup() {
        // 2070 活体窗:260W → 100% / 119%(按窗顶 ceil)/ 149%(按 default ceil)
        assert_eq!(
            percent_probe_percents(260_000, &live(219_000, 175_000)),
            [100, 119, 149]
        );
        // 整除时 ceil 不进位:300000/219000=136.98→137
        assert_eq!(
            percent_probe_percents(300_000, &live(219_000, 175_000)),
            [100, 137, 172]
        );
        // 目标恰为窗顶:按窗顶假设坍缩为 100,按 default 假设仍要 126
        assert_eq!(
            percent_probe_percents(219_000, &live(219_000, 175_000)),
            [100, 126]
        );
        // 极端比值触 500% 上限
        assert_eq!(
            percent_probe_percents(500_000, &live(219_000, 100_000))[2],
            500
        );
    }
}
