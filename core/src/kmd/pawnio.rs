//! PawnIO 设备直连:内核态物理内存读写的用户态半边。
//!
//! 不走 `PawnIOLib.dll`(避免多一个 DLL 加载面),直接对 `\Device\PawnIO`
//! 设备做三个 IOCTL——调用形态与 G-Helper 的 `PawnIOWrapper.cs` 一致(该实现
//! 在其百万级用户环境长期实跑,协议常量也已对上本仓库 `reverse/PawnIO-master`
//! 驱动源码):
//!
//! | IOCTL | 码 | 负载 |
//! |---|---|---|
//! | LOAD_BINARY | 0xA1B22084 | 输入 = 模块 blob(格式见 `core/kmd/README.md`),每句柄一次 |
//! | EXECUTE_FN | 0xA1B22104 | 输入 = 32 字节 ASCII 函数名(补零,`ioctl_` 前缀)+ 以 u64 为单位的实参;输出 = u64 数组 |
//! | VERSION | 0xA1B22184 | 输出 = ULONG 版本(major<<16 \| minor<<8 \| patch) |
//!
//! 模块返回的 NTSTATUS 即 IOCTL 完成状态:失败时 `DeviceIoControl` 返回
//! FALSE,本层映射为 [`PawnIoError::Win32`](错误码含签名校验失败等)。
//! 物理读经 [`PawnIoPhysMem`] 供给 [`super::pagewalk`] 的走查使用。

use std::ffi::c_void;
use std::ptr;

use quick_error::quick_error;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::pagewalk::{PhysError, PhysicalMemory};

/// 设备路径(G-Helper 同款 GLOBALROOT 形式;驱动同时建了 `\DosDevices\PawnIO`,
/// `\\.\PawnIO` 也可达)。
const DEVICE_PATH: &str = r"\\?\GLOBALROOT\Device\PawnIO";
/// `GENERIC_READ | GENERIC_WRITE`。
const DEVICE_ACCESS: u32 = 0xC000_0000;
/// `FILE_SHARE_READ | FILE_SHARE_WRITE`。
const DEVICE_SHARE: u32 = 0x3;
/// `OPEN_EXISTING`。
const DEVICE_OPEN_EXISTING: u32 = 0x3;
/// 设备类型(驱动 `pawnio_um.h`,G-Helper 解码同为 41394)。
const DEV_TYPE: u32 = 0xA1B2 << 16;
/// `IOCTL_PIO_LOAD_BINARY`(`CTL_CODE(0xA1B2, 0x821, METHOD_BUFFERED, FILE_ANY_ACCESS)`)。
pub const IOCTL_LOAD_BINARY: u32 = DEV_TYPE | (0x821 << 2);
/// `IOCTL_PIO_EXECUTE_FN`。
pub const IOCTL_EXECUTE_FN: u32 = DEV_TYPE | (0x841 << 2);
/// `IOCTL_PIO_VERSION`。
pub const IOCTL_VERSION: u32 = DEV_TYPE | (0x861 << 2);
/// 执行负载里函数名固定 32 字节(驱动 `vm_execute_function` 的解析布局)。
const FUNCTION_NAME_LEN: usize = 32;

quick_error! {
    /// PawnIO 通道错误。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PawnIoError {
        NotInstalled {
            display("PawnIO device not found (win32 error 2/3): driver not installed or not started")
        }
        AccessDenied {
            display("PawnIO device open denied (win32 error 5): try running elevated")
        }
        InvalidParam(msg: String) {
            display("invalid PawnIO call: {}", msg)
        }
        Win32(code: u32) {
            display("PawnIO IOCTL failed (win32 error {})", code)
        }
    }
}

/// 一个 PawnIO 执行器句柄(打开设备即一个执行器;每句柄至多加载一个模块)。
pub struct PawnIo {
    handle: HANDLE,
}

impl PawnIo {
    /// 打开 `\Device\PawnIO`。
    pub fn connect() -> Result<Self, PawnIoError> {
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
                2 | 3 => PawnIoError::NotInstalled,
                5 => PawnIoError::AccessDenied,
                other => PawnIoError::Win32(other),
            });
        }
        Ok(Self { handle })
    }

    /// 读取驱动版本(`major << 16 | minor << 8 | patch`)。
    pub fn version(&self) -> Result<u32, PawnIoError> {
        let mut out = [0u8; 4];
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_VERSION,
                ptr::null(),
                0,
                out.as_mut_ptr() as *mut c_void,
                out.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(PawnIoError::Win32(unsafe { GetLastError() }));
        }
        Ok(u32::from_le_bytes(out))
    }

    /// 加载模块 blob(每句柄一次;二次加载由驱动拒绝)。
    pub fn load_module(&self, blob: &[u8]) -> Result<(), PawnIoError> {
        if blob.len() < 4 {
            return Err(PawnIoError::InvalidParam(
                "module blob shorter than 4 bytes".into(),
            ));
        }
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_LOAD_BINARY,
                blob.as_ptr() as *const c_void,
                blob.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(PawnIoError::Win32(unsafe { GetLastError() }));
        }
        Ok(())
    }

    /// 执行已加载模块的公开函数(须 `ioctl_` 前缀、ASCII、短于 32 字节)。
    ///
    /// `input` 为 u64 实参;`output_len` 为模块声明的输出槽数(u64 条目),返回
    /// 驱动实际回填的条目。
    pub fn execute(
        &self,
        function: &str,
        input: &[u64],
        output_len: usize,
    ) -> Result<Vec<u64>, PawnIoError> {
        if !function.is_ascii()
            || !function.starts_with("ioctl_")
            || function.len() >= FUNCTION_NAME_LEN
        {
            return Err(PawnIoError::InvalidParam(format!(
                "function name {:?} must be ASCII, start with ioctl_ and be shorter than {} bytes",
                function, FUNCTION_NAME_LEN
            )));
        }
        let mut input_bytes = vec![0u8; FUNCTION_NAME_LEN + input.len() * 8];
        input_bytes[..function.len()].copy_from_slice(function.as_bytes());
        for (index, value) in input.iter().enumerate() {
            let at = FUNCTION_NAME_LEN + index * 8;
            input_bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        let mut output_bytes = vec![0u8; output_len * 8];
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_EXECUTE_FN,
                input_bytes.as_ptr() as *const c_void,
                input_bytes.len() as u32,
                output_bytes.as_mut_ptr() as *mut c_void,
                output_bytes.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(PawnIoError::Win32(unsafe { GetLastError() }));
        }
        let words = (returned as usize / 8).min(output_len);
        Ok((0..words)
            .map(|index| {
                u64::from_le_bytes(output_bytes[index * 8..index * 8 + 8].try_into().unwrap())
            })
            .collect())
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// 整页读 ioctl 名(`core/kmd/PhysMem.p`,输入 1:物理地址;输出 512:u64)。
pub const IOCTL_READ_PHYS_PAGE: &str = "ioctl_read_phys_page";
/// 8 字节读 ioctl 名(输入 1:物理地址;输出 1:u64)。
pub const IOCTL_READ_PHYS_QWORD: &str = "ioctl_read_phys_qword";

/// [`PhysicalMemory`] 的 PawnIO 通道实现:物理读经自研 PhysMem 模块转发到
/// 驱动的 `physical_read_qword` 原语(实现是 `MmGetVirtualForPhysical` +
/// `__try/__except`,无效地址返回错误而不是蓝屏,见
/// `reverse/PawnIO-master/PawnIO/src/natives_impl_windows.cpp`)。
pub struct PawnIoPhysMem<'a> {
    io: &'a PawnIo,
}

impl<'a> PawnIoPhysMem<'a> {
    pub fn new(io: &'a PawnIo) -> Self {
        Self { io }
    }

    fn read_words(&self, function: &str, pa: u64, words: usize) -> Result<Vec<u64>, PhysError> {
        self.io
            .execute(function, &[pa], words)
            .map_err(|err| match &err {
                PawnIoError::NotInstalled
                | PawnIoError::AccessDenied
                | PawnIoError::InvalidParam(_) => PhysError::Transport(err.to_string()),
                PawnIoError::Win32(_) => PhysError::Unreadable(pa),
            })
    }
}

impl PhysicalMemory for PawnIoPhysMem<'_> {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), PhysError> {
        match out.len() {
            4096 if pa & 0xFFF == 0 => {
                let words = self.read_words(IOCTL_READ_PHYS_PAGE, pa, 512)?;
                for (index, word) in words.iter().enumerate() {
                    out[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
                }
                Ok(())
            }
            8 => {
                let words = self.read_words(IOCTL_READ_PHYS_QWORD, pa, 1)?;
                out.copy_from_slice(&words[0].to_le_bytes());
                Ok(())
            }
            other => Err(PhysError::BadLength(other)),
        }
    }
}
