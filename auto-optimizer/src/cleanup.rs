use nvml_wrapper::enums::device::FanControlPolicy;
use nvoc_core::{
    ClockDomain, CoolerPolicy, CoolerTarget, Error, GpuTarget, QueryFanInfo, ResetCoolerLevels,
    ResetFanSpeed, ResetVfpDeltas, ResetVfpFrequencyLock, ResetVfpLock, SetCoolerLevels,
    SetFanSpeed, VfpResetDomain, run,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoscanExit {
    Success,
    Error,
}

pub fn cleanup_autoscan_exit(gpus: &[GpuTarget<'_>], exit: AutoscanExit) {
    for gpu in gpus {
        match exit {
            AutoscanExit::Success => cleanup_success(gpu),
            AutoscanExit::Error => cleanup_error(gpu),
        }
    }
}

fn cleanup_error(gpu: &GpuTarget<'_>) {
    force_fan_full(gpu);
    reset_vfp(gpu);
    reset_locks(gpu);
}

fn cleanup_success(gpu: &GpuTarget<'_>) {
    reset_vfp(gpu);
    reset_locks(gpu);
    reset_fan_auto(gpu);
}

fn force_fan_full(gpu: &GpuTarget<'_>) {
    if gpu.has_nvapi() {
        ignore_cleanup_error(
            gpu,
            "set NVAPI cooler level to 100%",
            run(
                gpu,
                SetCoolerLevels {
                    policy: CoolerPolicy::Manual,
                    level: 100,
                    cooler_target: CoolerTarget::All,
                },
            ),
        );
    }

    if gpu.has_nvml() {
        for fan_index in nvml_fan_indices(gpu) {
            ignore_cleanup_error(
                gpu,
                &format!("set NVML fan {} to 100%", fan_index + 1),
                run(
                    gpu,
                    SetFanSpeed {
                        fan_index,
                        policy: FanControlPolicy::Manual,
                        level: 100,
                    },
                ),
            );
        }
    }
}

fn reset_fan_auto(gpu: &GpuTarget<'_>) {
    if gpu.has_nvapi() {
        ignore_cleanup_error(
            gpu,
            "reset NVAPI cooler control",
            run(gpu, ResetCoolerLevels),
        );
    }

    if gpu.has_nvml() {
        for fan_index in nvml_fan_indices(gpu) {
            ignore_cleanup_error(
                gpu,
                &format!("reset NVML fan {} control", fan_index + 1),
                run(gpu, ResetFanSpeed { fan_index }),
            );
        }
    }
}

fn reset_vfp(gpu: &GpuTarget<'_>) {
    ignore_cleanup_error(
        gpu,
        "reset VFP deltas",
        run(
            gpu,
            ResetVfpDeltas {
                domain: VfpResetDomain::All,
            },
        ),
    );
}

fn reset_locks(gpu: &GpuTarget<'_>) {
    ignore_cleanup_error(gpu, "reset VFP voltage lock", run(gpu, ResetVfpLock));
    ignore_cleanup_error(
        gpu,
        "reset graphics VFP frequency lock",
        run(
            gpu,
            ResetVfpFrequencyLock {
                domain: ClockDomain::Graphics,
            },
        ),
    );
    ignore_cleanup_error(
        gpu,
        "reset memory VFP frequency lock",
        run(
            gpu,
            ResetVfpFrequencyLock {
                domain: ClockDomain::Memory,
            },
        ),
    );
}

fn nvml_fan_indices(gpu: &GpuTarget<'_>) -> Vec<u32> {
    match run(gpu, QueryFanInfo) {
        Ok(report) => (0..report.output.count).collect(),
        Err(err) => {
            log_cleanup_error(gpu, "query NVML fan info", err);
            Vec::new()
        }
    }
}

fn ignore_cleanup_error<T>(gpu: &GpuTarget<'_>, label: &str, result: Result<T, Error>) {
    if let Err(err) = result {
        log_cleanup_error(gpu, label, err);
    }
}

fn log_cleanup_error(gpu: &GpuTarget<'_>, label: &str, err: Error) {
    eprintln!(
        "Warning: autoscan cleanup failed for GPU {} during {}: {}",
        gpu.id.0, label, err
    );
}
