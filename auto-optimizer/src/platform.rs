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
