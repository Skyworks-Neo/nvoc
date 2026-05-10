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

/// Optional path for stressor game config used by resolution sync.
/// cli-stressor itself does not require this file, so default is None.
pub const fn stressor_3d_conf_path() -> Option<&'static str> {
    None
}

#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn panic_windows_only(feature: &str) -> ! {
    panic!("{feature} is only supported on Windows in this repository")
}

/// Returns `true` when the process has the privilege level required to write
/// GPU state through NVAPI / NVML (Administrator on Windows, root on POSIX).
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn IsUserAnAdmin() -> i32;
        }
        // SAFETY: IsUserAnAdmin reads only the caller's token; no preconditions.
        unsafe { IsUserAnAdmin() != 0 }
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
