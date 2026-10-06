//! kmd 功率墙原子流:`set-power-command --kmd` 的执行核心。
//!
//! 一个调用完成全周期,漏洞驱动(pmxdrv)**只在内存里停留一个命令周期**:
//!
//! ```text
//! 服务注册(PMXDRV_KMD,先清残留)→ 启动 → 走查+布局探测(全自动,静态
//! probe 本机在役镜像)→ GPU 表链定位 PowerRoot(身份门)→ 写 UPPER(单
//! u32,物理帧,读回校验)→ 原生 tgp 写(窗随 UPPER)→ 租约写(回显面,
//! 最后)→ 复验 → 服务停止+注销
//! ```
//!
//! 安全设计:
//! - 入口要求 `--force`(调用方责任线,与 set-power-command 一致);
//! - 绝对上限 500 W 硬拒(任何旗标都不过);
//! - 定位门:init==1 ∧ key<0x40 ∧ UPPER∈[50W,500W](防误配对象);
//! - 写后读回不一致 → 立即回滚 UPPER 到存档值并报错;
//! - 任一步失败 → 已注册的服务照常注销(finally 语义),不留残余。
//!
//! 前置(调用方环境):管理员令牌;显卡低电压锁已解(影响负载能否吃满,
//! 不影响本流程)。UPPER 写为易失(重启回落)——这正是"每开机一跑"的
//! 单命令形态与安全性的互相成全。

use quick_error::quick_error;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::layout_probe::{probe as probe_layout, NvlddmkmLayout};
use super::pagewalk::{
    discover_root, find_loaded_module, read_virtual, LoadedModule, PeFingerprint, PhysicalMemory,
    WalkError,
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
        if cached
            && let Some(page) = self.cache.borrow().get(&frame)
        {
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

/// 全周期结果(全部为已验证事实)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmdSetWallOutcome {
    pub layout_summary: String,
    pub root: RootInfo,
    pub upper_before_mw: u32,
    pub upper_after_mw: u32,
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
        ControlService, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW,
        StartServiceW, SC_HANDLE, SC_MANAGER_CREATE_SERVICE, SERVICE_CONTROL_STOP,
        SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER,
    };
    const SERVICE_ALL_ACCESS: u32 = 0xF01FF;

    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
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
    let mgr = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CREATE_SERVICE) };
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
        return Err(KmdPowerError::Service(format!("CreateService 失败: win32 {err}")));
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
    use windows_sys::Win32::System::Services::{CloseServiceHandle, 
        ControlService, DeleteService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
        SERVICE_CONTROL_STOP, SERVICE_STATUS, SC_HANDLE,
    };
    const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
    const SERVICE_STOPPED: u32 = 1;
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
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
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
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

/// GPU 表链定位 PowerRoot。身份门:init=1 ∧ key<0x40 ∧ UPPER∈[10W,500W]
/// (硬域,fail-closed)。**D 状态感知**:非 D1(如 D2=55W)时 UPPER 不是
/// 滑条顶——候选收集后用活体墙(0x67F31384 控制表 current,秒级新鲜)精确
/// 匹配择一;无活体参照时恰一候选才接受,多候选歧义即拒。
fn locate_root(
    phys: &CachedPhys,
    walk_root: u64,
    layout: &NvlddmkmLayout,
    module_base: u64,
    live_wall_mw: Option<u32>,
    steps: &mut Vec<String>,
) -> Result<RootInfo, KmdPowerError> {
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
    let mut candidates: Vec<RootInfo> = Vec::new();
    for i in 0..count {
        let Some(major) = rd_u64(
            phys,
            walk_root,
            table + u64::from(layout.entry_major_off) + u64::from(i) * u64::from(layout.entry_stride),
        )
        .filter(|v| is_kernel_va(*v)) else {
            continue;
        };
        let Some(root_va) =
            rd_u64(phys, walk_root, major + u64::from(layout.major_root_off)).filter(|v| is_kernel_va(*v))
        else {
            continue;
        };
        let init = rd_u32(phys, walk_root, root_va + u64::from(layout.root_init_off)).unwrap_or(0xFF);
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
        let frame_upper = read_virtual(phys, walk_root, root_va + u64::from(layout.root_upper_off), 4)
            .ok()
            .and_then(|_| {
                super::pagewalk::translate(phys, walk_root, root_va + u64::from(layout.root_upper_off))
                    .ok()
            })
            .map(|pa| pa & !0xFFF)
            .unwrap_or(0);
        candidates.push(RootInfo {
            major_va: major,
            root_va,
            entry_index: i,
            frame_root,
            frame_upper,
            init: (init & 0xFF) as u8,
            elig: rd_u32(phys, walk_root, root_va + u64::from(layout.root_init_off + 1))
                .unwrap_or(0xFF) as u8,
            amount_active: rd_u32(phys, walk_root, root_va + u64::from(layout.root_init_off + 2))
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
        return Err(KmdPowerError::Locate(
            "全部 GPU 表条目都未过身份门(布局假设与活体不符?)".into(),
        ));
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
    Err(KmdPowerError::Locate(format!(
        "{} 个候选歧义且活体墙无精确匹配(live={live_wall_mw:?})— 拒写",
        candidates.len()
    )))
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

    result.map(|mut o| {
        o.steps = steps;
        o.upper_before_mw = o.root.upper;
        o
    })
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
    steps.push(format!("走查根 {walk_root:#x}(nvlddmkm @{:#x})", module.base));

    // 3) 活体墙(D 状态感知)+ 定位(身份门)
    let live_wall = first_hi_gpu().and_then(|gpu| {
        gpu.tgp_watt_status()
            .ok()
            .flatten()
            .and_then(|st| st.current_mw)
    });
    steps.push(format!(
        "活体墙(tgp control current)= {live_wall_mw:?}",
        live_wall_mw = live_wall
    ));
    let root = locate_root(&phys, walk_root, &layout, module.base, live_wall, steps)?;
    steps.push(format!(
        "PowerRoot @ {:#016X}(entry{}): init={} key={} UPPER={} LOWER={} base={} amount={}",
        root.root_va, root.entry_index, root.init, root.key,
        root.upper, root.lower, root.base, root.amount
    ));

    // 4) 写 UPPER(单 u32,物理帧;读回校验,不符即时回滚)
    let upper_va = root.root_va + u64::from(layout.root_upper_off);
    if wall_mw != root.upper {
        let pa = super::pagewalk::translate(&phys, walk_root, upper_va).map_err(KmdPowerError::Walk)?;
        let frame = pa & !0xFFF;
        let off_in_page = (upper_va & 0xFFF) as usize;
        let mapped = phys
            .drv
            .map_physical(frame, 1)
            .map_err(|e| KmdPowerError::Write(format!("帧映射失败: {e}")))?;
        unsafe {
            ((mapped + off_in_page as u64) as *mut u32).write_volatile(wall_mw);
        }
        phys.drv
            .unmap_physical(mapped)
            .map_err(|e| KmdPowerError::Write(format!("反映射失败: {e}")))?;
        if rd_u32(&phys, walk_root, upper_va) != Some(wall_mw) {
            // 即时回滚
            if let Ok(m2) = phys.drv.map_physical(frame, 1) {
                unsafe {
                    ((m2 + off_in_page as u64) as *mut u32).write_volatile(root.upper);
                }
                let _ = phys.drv.unmap_physical(m2);
            }
            return Err(KmdPowerError::Write(format!(
                "读回不符 — 已回滚 UPPER 到 {}",
                root.upper
            )));
        }
        steps.push(format!("UPPER {} → {} ✓ 读回一致", root.upper, wall_mw));
    } else {
        steps.push(format!("UPPER 已是 {wall_mw}(幂等写跳过)"));
    }

    // 5) 原生 tgp 写(控制值 < 目标时才写;已达标绝不写 —— 防窗钳把控制压回)
    let tgp_w = wall_mw / 1000;
    let live_control = first_hi_gpu().and_then(|gpu| {
        gpu.tgp_watt_status().ok().flatten().and_then(|st| st.current_mw)
    });
    let (tgp_written, tgp_note) = match live_control {
        Some(cur) if cur >= wall_mw => {
            (0, format!("tgp 控制已 {cur} mW ≥ 目标 {wall_mw} —— 跳过写(防窗钳压低)"))
        }
        Some(_) => match first_hi_gpu().map(|g| g.set_tgp_watt(tgp_w, 2)) {
            Some(Ok(applied)) if applied * 1000 >= wall_mw => {
                (tgp_w, format!("tgp {tgp_w} W 写入 ✓(读回 {applied} W)"))
            }
            Some(Ok(applied)) => (
                0,
                format!(
                    "tgp 写被窗钳回 {applied} W(< 目标)—— 窗未跟 UPPER,用 set-pwr-cur-limit 复查"
                ),
            ),
            Some(Err(e)) => (0, format!("tgp 写失败(UPPER 已生效,可手动补): {e}")),
            None => (0, "tgp 写跳过:GPU 0 不可达".into()),
        },
        None => (0, "tgp 写跳过:控制值不可读".into()),
    };
    steps.push(tgp_note.clone());

    // 6) 租约写(回显面,最后;checked 版自带包络,越其包络时降级为直写)
    let (lease_written, lease_note) = match first_hi_gpu() {
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

    // 7) 复验
    let final_upper = rd_u32(&phys, walk_root, upper_va);
    if final_upper != Some(wall_mw) {
        return Err(KmdPowerError::Write(format!(
            "复验失败:UPPER={final_upper:?} ≠ {wall_mw}(被外部改写?)"
        )));
    }
    steps.push(format!("复验 UPPER={} ✓", final_upper.unwrap()));

    Ok(KmdSetWallOutcome {
        layout_summary: format!(
            "slot={:#x} table+{:#x} root_init={:#x}",
            layout.global_slot_rva, layout.state_table_off, layout.root_init_off
        ),
        upper_before_mw: root.upper,
        upper_after_mw: wall_mw,
        root,
        tgp_written_w: tgp_written,
        tgp_write_note: tgp_note,
        lease_written_mw: lease_written,
        lease_note,
        service: SERVICE_NAME.to_string(),
        steps: Vec::new(),
    })
}

/// 第一块卡(`PhysicalGpu::enumerate()[0]`,NVAPI 不可达时 None)。
fn first_hi_gpu() -> Option<nvapi::hi::Gpu> {
    let first = nvapi::PhysicalGpu::enumerate().ok()?.into_iter().next()?;
    Some(nvapi::hi::Gpu::new(first))
}
