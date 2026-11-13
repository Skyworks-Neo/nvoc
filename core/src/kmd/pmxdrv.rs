//! Intel PMxDrv(`pmxdrv.sys`)设备直连:本车道内核态物理读的主传输层。
//!
//! 驱动来源:xOCD 内嵌资源(xOCD.exe+0x38E5F4,43632 字节),PDB 路径
//! `C:\MyProjects\git\pmx-cse-new\x64\Release\pmxdrv.pdb` —— Intel CSE/ME
//! 工具链的 `PMxDrv32e` 驱动(2019-07-07 构建,Authenticode 签名
//! "Intel(R) Embedded Subsystems and IP Blocks Group",签名有效)。接口由
//! `reverse/xocd/pmxdrv.sys` 全量逆向得出(idalib,`sub_140001000` 起):
//!
//! | IOCTL | 值 | 负载(经 16 字节指针转发) |
//! |---|---|---|
//! | MAP_PHYS | 0x222878 | 请求 `{tag=24, u64 pa@4, u32 pages@12, u64 va_out@16}` |
//! | UNMAP_PHYS | 0x22287C | 请求 `{tag=24, u64 va@16}` |
//! | PORT_IO | 0x222864 | `{tag=16, op@4(1/2/3 in b/w/d, 4/5/6 out), u16 port@8, val@12}`(未接线) |
//! | PCI_CFG | 0x222868 | `{tag=28, mode@4, bdf/off@8, bus@10, and@12, or@16, old@20, val@24}`(未接线) |
//! | LAST_ERROR | 0x222890 | 驱动写 u32 错误码到请求 `+4` |
//!
//! 调用形态(逆向实证,也是它被归为"漏洞驱动"的原因):DeviceIoControl 走
//! METHOD_BUFFERED,输入恒 16 字节 = `[u64 用户态请求指针][u32 aux<=0x3F][pad]`,
//! 驱动**不加探测直接解引用该指针**——所以请求结构就是本进程的普通堆内存;
//! `MAP_PHYS` 经 `\Device\PhysicalMemory` 节对象 `ZwMapViewOfSection` 到
//! **当前进程**,返回的 `va_out` 是本进程用户态地址,读完 `UNMAP_PHYS` 即可。
//! 映射页保护是 PAGE_READWRITE:传输层天然可写,本车道只读、写路径继续封存。

use std::ffi::c_void;
use std::ptr;

use quick_error::quick_error;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::pagewalk::{PhysError, PhysicalMemory};

/// Win32 符号链接(驱动 DriverEntry 建 `\Device\Pmxdrv` + `\DosDevices\PMXDRV`)。
const DEVICE_PATH: &str = r"\\.\PMXDRV";
/// `GENERIC_READ | GENERIC_WRITE`。
const DEVICE_ACCESS: u32 = 0xC000_0000;
/// `FILE_SHARE_READ | FILE_SHARE_WRITE`。
const DEVICE_SHARE: u32 = 0x3;
/// `OPEN_EXISTING`。
const DEVICE_OPEN_EXISTING: u32 = 0x3;
/// 码表(两代已知构建共用)。
///
/// `CTL_CODE(FILE_DEVICE_UNKNOWN, fn, METHOD_BUFFERED, FILE_ANY_ACCESS)`:
/// MAP=`0xAAB8?`——准确值 `0x222AB8`、UNMAP=`0x222ABC`、LAST_ERROR=`0x222AD0`
/// (func 0xAAE/0xAAF/0xAB4)。**本机两代构建实测同一码表**:Intel 2019 款
/// (xOCD 内嵌,`sub edx, 222A80h` 汇编实证)与 PAIPTAC 重建款(system32 在役
/// 件,idalib case 表十进制 2239104=0x222A80 起步 22 码)是同一 0x222A80 族,
/// PAIPTAC 只是超集(map/unmap/lasterror 码位逐一同位);2026-10-06 在役
/// PAIPTAC 构建实跑走查全绿。
///
/// 教训(记录防重蹈):初版实现曾把反编译器的十进制 2239104 误算成
/// 0x222840,推出一套"0x2228 族"错码并得到 87 拒绝——错在十进制转十六进制
/// 的手算,不在反编译器;`sub edx,222A80h` 的汇编与 case 表十进制值自洽。
const IOCTL_MAP_PHYS: u32 = 0x222AB8;
const IOCTL_UNMAP_PHYS: u32 = 0x222ABC;
const IOCTL_LAST_ERROR: u32 = 0x222AD0;
/// 64 位进程的固定输入长度(驱动 prologue 校验,`sub_140001884`)。
const INPUT_LEN: usize = 16;
/// 请求结构 tag(=结构长);不匹配驱动直接拒绝。
const REQ_MAP_LEN: u32 = 24;
/// 驱动映射失败的哨兵值(32 位分支另有 0xDEAD = VA 超 32 位)。
const MAP_FAIL_MARK: u64 = 0xBEEF;

quick_error! {
    /// PMxDrv 通道错误。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PmxError {
        NotInstalled {
            display("PMxDrv device not found: driver not installed/started (sc create PMXDRV type= kernel binPath=... pmxdrv.sys && sc start PMXDRV, elevated)")
        }
        AccessDenied {
            display("PMxDrv device open denied: try running elevated")
        }
        Win32(code: u32) {
            display("PMxDrv IOCTL failed (win32 error {})", code)
        }
        MapFailed(code: Option<u32>) {
            display("PMxDrv map rejected (driver code {:?}: request tag/size or unreachable physical range)", code)
        }
    }
}

/// 一个 PMxDrv 设备句柄。
pub struct PmxDrv {
    handle: HANDLE,
}

impl PmxDrv {
    /// 打开 `\\.\PMXDRV`(驱动须已由服务方式加载)。
    pub fn connect() -> Result<Self, PmxError> {
        let wide: Vec<u16> = DEVICE_PATH
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DEVICE_ACCESS,
                DEVICE_SHARE,
                ptr::null(),
                DEVICE_OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle.is_null() || handle as isize == -1 {
            let code = unsafe { GetLastError() };
            return Err(match code {
                2 | 3 => PmxError::NotInstalled,
                5 => PmxError::AccessDenied,
                other => PmxError::Win32(other),
            });
        }
        Ok(Self { handle })
    }

    /// 发一次「指针转发」IOCTL:输入 16 字节,内容是用户态请求结构的地址。
    fn forward(&self, ioctl: u32, request: &mut [u8]) -> Result<(), PmxError> {
        debug_assert!(request.len() >= 24);
        let mut input = [0u8; INPUT_LEN];
        input[..8].copy_from_slice(&(request.as_ptr() as u64).to_le_bytes());
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                ioctl,
                input.as_ptr() as *const c_void,
                input.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(PmxError::Win32(unsafe { GetLastError() }));
        }
        Ok(())
    }

    /// 读驱动的内部错误码(`LAST_ERROR` 把请求 `+4` 复位为 0 后回填)。
    fn last_error(&self) -> Option<u32> {
        let mut request = [0u8; 24];
        self.forward(IOCTL_LAST_ERROR, &mut request).ok()?;
        Some(u32::from_le_bytes(request[4..8].try_into().unwrap()))
    }

    /// 映射 `pages` 页物理内存到本进程,返回用户态地址。
    pub fn map_physical(&self, pa: u64, pages: u32) -> Result<u64, PmxError> {
        let mut request = [0u8; 24];
        request[0..4].copy_from_slice(&REQ_MAP_LEN.to_le_bytes());
        request[4..12].copy_from_slice(&pa.to_le_bytes());
        request[12..16].copy_from_slice(&pages.to_le_bytes());
        self.forward(IOCTL_MAP_PHYS, &mut request)
            .map_err(|err| self.map_failure(err))?;
        let va = u64::from_le_bytes(request[16..24].try_into().unwrap());
        if va == 0 || va == MAP_FAIL_MARK {
            return Err(self.map_failure(PmxError::MapFailed(None)));
        }
        Ok(va)
    }

    /// 把失败翻译成带驱动错误码的 [`PmxError::MapFailed`](安装类错误除外)。
    fn map_failure(&self, fallback: PmxError) -> PmxError {
        match fallback {
            PmxError::NotInstalled | PmxError::AccessDenied => fallback,
            _ => PmxError::MapFailed(self.last_error()),
        }
    }

    /// 解除 `map_physical` 的映射。
    pub fn unmap_physical(&self, va: u64) -> Result<(), PmxError> {
        let mut request = [0u8; 24];
        request[0..4].copy_from_slice(&REQ_MAP_LEN.to_le_bytes());
        request[16..24].copy_from_slice(&va.to_le_bytes());
        self.forward(IOCTL_UNMAP_PHYS, &mut request)
    }
}

impl Drop for PmxDrv {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// [`PhysicalMemory`] 的 PMxDrv 实现:每次读都走
/// map(包含目标的 1 页) → 用户态拷贝 → unmap。
///
/// 驱动以 `PAGE_READWRITE` 映射;本实现只从映射窗口拷出字节,从不写入。
pub struct PmxDrvPhysMem<'a> {
    drv: &'a PmxDrv,
}

impl<'a> PmxDrvPhysMem<'a> {
    pub fn new(drv: &'a PmxDrv) -> Self {
        Self { drv }
    }
}

impl PhysicalMemory for PmxDrvPhysMem<'_> {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), PhysError> {
        if out.is_empty() || out.len() > 4096 {
            return Err(PhysError::BadLength(out.len()));
        }
        let base = pa & !0xFFF;
        let offset = (pa - base) as usize;
        if offset + out.len() > 4096 {
            return Err(PhysError::BadLength(out.len()));
        }
        let va = self.drv.map_physical(base, 1).map_err(|err| match err {
            PmxError::NotInstalled | PmxError::AccessDenied => {
                PhysError::Transport(err.to_string())
            }
            _ => PhysError::Unreadable(pa),
        })?;
        // 映射窗口是我们进程的用户地址;短读只可能因目标页本就不可访问。
        let window = unsafe { std::slice::from_raw_parts(va as *const u8, 4096) };
        let copied = window
            .get(offset..offset + out.len())
            .ok_or(PhysError::Unreadable(pa))
            .map(|chunk| out.copy_from_slice(chunk));
        let unmap = self.drv.unmap_physical(va);
        copied?;
        unmap.map_err(|err| PhysError::Transport(err.to_string()))
    }
}
