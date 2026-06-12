use nvoc_core::{Error, GpuOperation, GpuTarget, run as nvoc_run};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

pub(super) fn run_output<O: GpuOperation>(gpu: &GpuTarget<'_>, op: O) -> Result<O::Output, Error> {
    nvoc_run(gpu, op).map(|report| report.output)
}

/// Spawns a minimal Vulkan load process to wake a power-gated GPU on Optimus
/// laptops, then kills the process when dropped.
pub(super) struct MinLoadPulse(Option<Child>);

impl MinLoadPulse {
    pub(super) fn wake(test_exe: &str, cuda_device: Option<u32>) -> Self {
        let mut cmd = Command::new(test_exe);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        if let Some(dev) = cuda_device {
            cmd.env("CUDA_DEVICE_ORDER", "PCI_BUS_ID");
            cmd.env("CUDA_VISIBLE_DEVICES", dev.to_string());
        }
        match cmd.spawn() {
            Ok(child) => {
                eprintln!("MinLoadPulse: spawned PID {} to wake GPU.", child.id(),);
                sleep(Duration::from_secs(3));
                Self(Some(child))
            }
            Err(e) => {
                eprintln!("MinLoadPulse: failed to spawn: {}", e);
                Self(None)
            }
        }
    }
}

impl Drop for MinLoadPulse {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let pid = child.id();
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("MinLoadPulse: killed PID {}.", pid);
        }
    }
}

// Retry a generic operation with exponential backoff on any NVAPI error.
// When GPUnotpowered is detected, automatically spawns a minimal Vulkan load
// via minload_exe to wake the GPU before retrying.
pub(super) fn retry_operation_with_backoff<T, F>(
    mut op: F,
    label: &str,
    attempts: usize,
    base_wait_secs: u64,
    minload_exe: &str,
    cuda_device: Option<u32>,
) -> Result<T, Error>
where
    F: FnMut() -> Result<T, Error>,
{
    let mut last_err: Option<Error> = None;
    for attempt in 0..attempts {
        if attempt > 0 {
            eprintln!(
                "Retrying {} (attempt {}/{})...",
                label,
                attempt + 1,
                attempts
            );
        }
        match op() {
            Ok(v) => {
                if attempt > 0 {
                    eprintln!("{} succeeded on retry (attempt {}).", label, attempt + 1);
                }
                return Ok(v);
            }
            Err(e) => {
                eprintln!("{} failed: {:?}", label, e);
                let s_lower = format!("{:?}", &e).to_lowercase();
                last_err = Some(e);

                if s_lower.contains("gpunotpowered") {
                    eprintln!(
                        "{}: GPUnotpowered detected, launching min-load pulse...",
                        label
                    );
                    let _pulse = MinLoadPulse::wake(minload_exe, cuda_device);
                    match op() {
                        Ok(v) => {
                            eprintln!("{} succeeded on GPU wake retry.", label);
                            return Ok(v);
                        }
                        Err(e2) => {
                            eprintln!("{} still failed after GPU wake: {:?}", label, e2);
                            last_err = Some(e2);
                        }
                    }
                }

                if attempt + 1 < attempts {
                    let exp = (1u64 << attempt).saturating_mul(base_wait_secs);
                    let wait = exp.min(60);
                    eprintln!("NVAPI error detected; sleeping {}s before retry...", wait);
                    sleep(Duration::from_secs(wait));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| Error::Custom(format!("{}: retry exhausted", label))))
}
