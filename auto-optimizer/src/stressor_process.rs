use nvoc_core::{Error, GpuTarget, QueryGpuInfo, run};
use std::process::Command;

#[cfg(feature = "stressor-bundled")]
pub const WORKER_ENV: &str = "NVOC_STRESSOR_CUDA_RS_WORKER";
// Internal marker used in scan configuration instead of a filesystem path.
// It never reaches Command::new; bundled_command resolves it to current_exe().
pub const BUNDLED_SENTINEL: &str = "@bundled:cli-stressor-cuda-rs";

pub fn is_bundled(path: &str) -> bool {
    path == BUNDLED_SENTINEL
}

pub fn external_command(
    executable: &str,
    profile: Option<&str>,
    config: Option<&str>,
    duration_secs: f64,
    cuda_device: Option<u32>,
    extra_args: &[String],
) -> Command {
    let mut command = Command::new(executable);
    add_stressor_args(
        &mut command,
        profile,
        config,
        duration_secs,
        cuda_device,
        extra_args,
    );
    command
}

fn add_stressor_args(
    command: &mut Command,
    profile: Option<&str>,
    config: Option<&str>,
    duration_secs: f64,
    cuda_device: Option<u32>,
    extra_args: &[String],
) {
    match (profile, config) {
        (_, Some(path)) => {
            command.args(["--config", path]);
        }
        (Some(profile), None) => {
            command.args(["--profile", profile]);
        }
        (None, None) => {}
    }
    command.args(["--duration", &duration_secs.to_string()]);
    if let Some(device) = cuda_device {
        command.args(["--gpu-index", &device.to_string()]);
    }
    command.args(extra_args);
}

#[cfg(feature = "stressor-bundled")]
pub fn bundled_command(
    profile: Option<&str>,
    config: Option<&str>,
    duration_secs: f64,
    cuda_device: Option<u32>,
    extra_args: &[String],
) -> Result<Command, Error> {
    // The optimizer deliberately executes a new copy of itself. CUDA is only
    // initialized in that child, so a fatal CUDA failure cannot poison the
    // long-running optimizer process.
    let executable = std::env::current_exe()
        .map_err(|e| Error::Custom(format!("failed to resolve current executable: {e}")))?;
    let mut command = Command::new(executable);
    // main() checks this before parsing optimizer commands and dispatches the
    // child directly into the embedded cli-stressor-cuda-rs runner.
    command.env(WORKER_ENV, "1");
    add_stressor_args(
        &mut command,
        profile,
        config,
        duration_secs,
        cuda_device,
        extra_args,
    );
    Ok(command)
}

#[cfg(not(feature = "stressor-bundled"))]
pub fn bundled_command(
    _: Option<&str>,
    _: Option<&str>,
    _: f64,
    _: Option<u32>,
    _: &[String],
) -> Result<Command, Error> {
    Err(Error::Custom(
        "bundled stressor support is disabled; rebuild with feature stressor-bundled".into(),
    ))
}

pub fn resolve_profile(gpu: &GpuTarget<'_>, requested: &str) -> Result<String, Error> {
    // Explicit profile names pass through unchanged. Only "auto" needs an
    // NVAPI query to choose between the embedded VRAM-sized configurations.
    if requested != "auto" {
        return Ok(requested.to_string());
    }

    let info = run(gpu, QueryGpuInfo)?.output;
    let vram_kib = u64::from(info.physical_frame_buffer.0);
    profile_for_vram_kib(vram_kib)
}

fn profile_for_vram_kib(vram_kib: u64) -> Result<String, Error> {
    const SIX_GIB_KIB: u64 = 6 * 1024 * 1024;
    const EIGHT_GIB_KIB: u64 = 8 * 1024 * 1024;

    if vram_kib < SIX_GIB_KIB {
        return Err(Error::Custom(format!(
            "automatic stress profile requires at least 6 GiB VRAM (detected {} KiB); pass --stressor-profile explicitly to override",
            vram_kib
        )));
    }
    if vram_kib <= EIGHT_GIB_KIB {
        Ok("low-vram".to_string())
    } else {
        Ok("standard".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_sentinel_is_exact() {
        assert!(is_bundled(BUNDLED_SENTINEL));
        assert!(!is_bundled("cli-stressor-cuda-rs"));
    }

    #[test]
    fn external_command_uses_structured_cli_arguments() {
        let command = external_command(
            "cli-stressor-cuda-rs",
            Some("low-vram"),
            None,
            25.0,
            Some(2),
            &["--disable-fp8".into()],
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--profile",
                "low-vram",
                "--duration",
                "25",
                "--gpu-index",
                "2",
                "--disable-fp8"
            ]
        );
    }

    #[test]
    fn automatic_profiles_follow_vram_boundaries() {
        assert!(profile_for_vram_kib(6 * 1024 * 1024 - 1).is_err());
        assert_eq!(profile_for_vram_kib(6 * 1024 * 1024).unwrap(), "low-vram");
        assert_eq!(profile_for_vram_kib(8 * 1024 * 1024).unwrap(), "low-vram");
        assert_eq!(
            profile_for_vram_kib(8 * 1024 * 1024 + 1).unwrap(),
            "standard"
        );
    }
}
