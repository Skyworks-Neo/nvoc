use nvoc_core::{Error, GpuOperation, GpuTarget, run as nvoc_run};
use std::thread::sleep;
use std::time::Duration;

pub(super) fn run_output<O: GpuOperation>(gpu: &GpuTarget<'_>, op: O) -> Result<O::Output, Error> {
    nvoc_run(gpu, op).map(|report| report.output)
}

/// 原生唤醒：对 GC6/GCOFF 掉电的 dGPU 调用 force_gc6_exit（NVAPI 0x55590CB2），
/// 一次性拉回 D0。取代旧的 MinLoadPulse 方案（spawn Vulkan minload 子进程 +
/// 固定 3s sleep + taskkill）—— 无子进程、毫秒级完成、无残留进程风险。
///
/// 唤醒非持久（空闲约 5-20s 后会重新 GCOFF），因此只对紧随其后的操作序列
/// 有效；NVAPI 写操作另由 core::operation::run 的预唤醒钩子按
/// GpuType::needs_gc6_wake() 自动兜底，读操作序列则需在本侧显式调用。
/// 桌面端（无 GC6）驱动返回 NoImplementation(-104) 等，按 best-effort 忽略。
pub(super) fn native_wake(gpu: &GpuTarget<'_>) {
    match gpu.force_wake() {
        Ok(()) => eprintln!("NativeWake: force_gc6_exit ok."),
        Err(e) => eprintln!("NativeWake: force_gc6_exit failed (continuing): {:?}", e),
    }
}

// Retry a generic operation with exponential backoff on any NVAPI error.
// When GPUnotpowered is detected, natively wake every GPU in `gpus` via
// force_gc6_exit before retrying (see native_wake). The error itself proves
// the GPU is power-gated, so no generation gate is needed here — desktop
// GPUs never return GPUnotpowered.
pub(super) fn retry_operation_with_backoff<T, F>(
    gpus: &[GpuTarget<'_>],
    mut op: F,
    label: &str,
    attempts: usize,
    base_wait_secs: u64,
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
                        "{}: GPUnotpowered detected, natively waking via force_gc6_exit...",
                        label
                    );
                    for gpu in gpus {
                        native_wake(gpu);
                    }
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
