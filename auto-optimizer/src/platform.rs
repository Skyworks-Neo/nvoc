pub const fn default_vfp_temp_csv_path() -> &'static str {
    "./ws/vfp-tem.csv"
}

pub const fn default_vfp_csv_path() -> &'static str {
    "./ws/vfp.csv"
}

pub const fn default_vfp_init_csv_path() -> &'static str {
    "./ws/vfp-init.csv"
}

pub const fn default_vfp_log_path() -> &'static str {
    "./ws/vfp.log"
}

pub const fn default_test_exe_path() -> &'static str {
    #[cfg(windows)]
    {
        "./test/test_cuda_windows.bat"
    }
    #[cfg(not(windows))]
    {
        "./test/test_opencl_linux.sh"
    }
}

#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn panic_windows_only(feature: &str) -> ! {
    panic!("{feature} is only supported on Windows in this repository")
}

/// Returns `true` when the process has the privilege level required to write
/// GPU state through NVAPI / NVML (an elevated token on Windows, root on POSIX).
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        type Handle = *mut core::ffi::c_void;
        type Bool = i32;
        type Dword = u32;
        type TokenInformationClass = u32;

        #[repr(C)]
        struct TokenElevation {
            token_is_elevated: Dword,
        }

        const TOKEN_QUERY: Dword = 0x0008;
        const TOKEN_ELEVATION_CLASS: TokenInformationClass = 20;

        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn OpenProcessToken(
                process_handle: Handle,
                desired_access: Dword,
                token_handle: *mut Handle,
            ) -> Bool;
            fn GetTokenInformation(
                token_handle: Handle,
                token_information_class: TokenInformationClass,
                token_information: *mut core::ffi::c_void,
                token_information_length: Dword,
                return_length: *mut Dword,
            ) -> Bool;
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> Handle;
            fn CloseHandle(handle: Handle) -> Bool;
        }

        let mut token: Handle = core::ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle for the current process.
        // OpenProcessToken and GetTokenInformation are called with valid buffers and sizes.
        // CloseHandle is called only for a successfully opened token handle.
        unsafe {
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }

            let mut elevation = TokenElevation {
                token_is_elevated: 0,
            };
            let mut return_length: Dword = 0;
            let ok = GetTokenInformation(
                token,
                TOKEN_ELEVATION_CLASS,
                (&mut elevation as *mut TokenElevation).cast(),
                core::mem::size_of::<TokenElevation>() as Dword,
                &mut return_length,
            ) != 0;
            let _ = CloseHandle(token);

            ok && elevation.token_is_elevated != 0
        }
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid() always succeeds and is async-signal-safe.
        unsafe { geteuid() == 0 }
    }
}
