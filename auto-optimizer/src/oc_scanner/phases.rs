use super::pressure::{PressureTestConfig, run_pressure_test};
use super::runtime::{MinLoadPulse, run_output};
use crate::progressbar::ScanProgress;
use crate::scan_log::{ScanDomain, ScanLogWriter, TestPhase};
use crate::scan_strategy::{FluctuationStrategy, StepController};
use crate::scan_support::{handle_lock_vfp, local_time_hms, voltage_frequency_check};
use clap::ArgMatches;
use nvoc_core::sync_memory_pstate_as_p0;
use nvoc_core::{
    ClockDomain, Error, GpuTarget, KilohertzDelta, Microvolts, NvapiLockedVoltageTarget, PState,
    QueryVfpPointVoltage, ResetVfpDeltas, SetVfpVoltageLock, VfpResetDomain,
    set_nvapi_pstate_clock_offsets, set_nvapi_vfp_curve_delta,
};
use std::io;
use std::thread::sleep;
use std::time::Duration;

pub(super) struct CommonPhaseArgs<'a> {
    // Borrowed values that stay constant across short/long/memory phases for
    // one scan setup.
    pub(super) matches: &'a ArgMatches,
    pub(super) minimum_delta_core_freq_step: i32,
    pub(super) fluctuation: FluctuationStrategy,
    pub(super) test_exe: &'a str,
    pub(super) minload_exe: &'a str,
    pub(super) delimiter: &'a str,
    pub(super) test_duration: u64,
    pub(super) endurance_coefficient: u64,
    pub(super) progress: Option<&'a ScanProgress>,
    pub(super) cuda_device: Option<u32>,
    pub(super) stressor_extra_args: &'a [String],
    pub(super) wakeup_load_needed: bool,
    pub(super) stressor_profile: &'a str,
    pub(super) stressor_config: Option<&'a str>,
}

struct PressureRunSpec {
    // Per-stressor-run values. Keeping these separate from CommonPhaseArgs
    // makes each call site state what changes between short/long/memory runs.
    point: usize,
    flat_curve_flag: bool,
    vfp_set_range: usize,
    init_core_oc_value: i32,
    test_code: String,
    timeout_loops: u64,
    is_legacy_global_offset: bool,
    test_duration_secs: u64,
}

fn begin_test_result_log(
    l: &mut ScanLogWriter,
    domain: ScanDomain,
    phase: TestPhase,
    test_code: usize,
    point: usize,
    voltage: Option<Microvolts>,
    delta: KilohertzDelta,
) -> io::Result<String> {
    l.write_pending_test_result(domain, phase, test_code, point, voltage, delta)
}

fn finish_test_result_log(
    l: &mut ScanLogWriter,
    started_at: String,
    domain: ScanDomain,
    phase: TestPhase,
    test_code: usize,
    point: usize,
    voltage: Option<Microvolts>,
    delta: KilohertzDelta,
    result_code: i32,
) -> io::Result<()> {
    l.write_completed_test_result(
        started_at,
        domain,
        phase,
        test_code,
        point,
        voltage,
        delta,
        result_code,
    )
}

impl<'a> CommonPhaseArgs<'a> {
    fn pressure_config(
        &self,
        _gpu: &GpuTarget<'_>,
        spec: PressureRunSpec,
    ) -> PressureTestConfig<'a> {
        // This is the only mapping layer between phase-level scan state and
        // pressure-runner process state; avoid recreating long positional calls.
        PressureTestConfig {
            point: spec.point,
            flat_curve_flag: spec.flat_curve_flag,
            vfp_set_range: spec.vfp_set_range,
            init_core_oc_value: spec.init_core_oc_value,
            minimum_delta_core_freq_step: self.minimum_delta_core_freq_step,
            fluctuation: self.fluctuation.clone(),
            test_exe: self.test_exe,
            minload_exe: self.minload_exe,
            test_code: spec.test_code,
            timeout_loops: spec.timeout_loops,
            is_legacy_global_offset: spec.is_legacy_global_offset,
            test_duration_secs: spec.test_duration_secs,
            progress: self.progress,
            cuda_device: self.cuda_device,
            stressor_extra_args: self.stressor_extra_args,
            wakeup_load_needed: self.wakeup_load_needed,
            stressor_profile: self.stressor_profile,
            stressor_config: self.stressor_config,
            #[cfg(windows)]
            target_gpu_id: _gpu.id.0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) struct LegacyPhaseArgs<'a> {
    // Legacy cards use a single global P0 graphics offset, but still reuse the
    // same stressor/progress plumbing as VFP scans.
    pub(super) common: CommonPhaseArgs<'a>,
    pub(super) point: usize,
    pub(super) flat_curve_flag: bool,
    pub(super) vfp_set_range: usize,
    #[allow(dead_code)]
    pub(super) freq_step_exp: usize,
}

fn pre_load_vf_recheck(gpu: &GpuTarget<'_>, point: usize) -> bool {
    println!("Waiting for pre-load volt-freq recheck");

    // voltage_frequency_check 可能仍返回 Result，我们这里捕获错误并当作失败处理
    let checks = match voltage_frequency_check(std::slice::from_ref(gpu), point) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read V/F info: {e}");
            return false;
        }
    };

    if checks.iter().all(|check| check.precise) {
        println!("[SCANNER] Pre-load V/F check passed at point {}", point);
        return true; // 检查通过
    }

    let summary = checks
        .iter()
        .map(|check| {
            format!(
                "GPU {} precise={} matched_point={:?}",
                check.gpu_id, check.precise, check.matched_point
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    eprintln!("V/F check failed at point {point}: {summary}");
    false // 检查失败
}

/// Apply VFP curve delta with voltage lock, then retry with exponential backoff
/// until `pre_load_vf_recheck` passes or `max_attempts` is exhausted.
#[allow(clippy::too_many_arguments)]
fn set_vfp_and_recheck(
    gpu: &GpuTarget<'_>,
    point: usize,
    vfp_set_range: usize,
    flat_curve_flag: bool,
    init_core_oc_value: i32,
    minimum_delta_core_freq_step: i32,
    max_attempts: i32,
    minload_exe: &str,
    cuda_device: Option<u32>,
    wakeup_load_needed: bool,
) -> Result<(), Error> {
    for attempt in 1..=max_attempts {
        let _pulse = if wakeup_load_needed {
            Some(MinLoadPulse::wake(minload_exe, cuda_device))
        } else {
            None
        };

        set_nvapi_vfp_curve_delta(
            gpu,
            point,
            vfp_set_range,
            flat_curve_flag,
            init_core_oc_value,
            Some(init_core_oc_value - minimum_delta_core_freq_step),
        )?;

        // After PnP reset the voltage lock is lost — re-lock at the target point
        if let Ok(locked_v) = run_output(gpu, QueryVfpPointVoltage { point }) {
            let _ = run_output(
                gpu,
                SetVfpVoltageLock {
                    voltage_target: NvapiLockedVoltageTarget::Voltage(locked_v),
                    feedback: false,
                },
            );
        }

        if pre_load_vf_recheck(gpu, point) {
            println!("V/F recheck passed on attempt {attempt}");
            return Ok(());
        }
        eprintln!("Retrying set_nvapi_vfp_curve_delta... (attempt {attempt})");
        let wait_secs = 2u64.saturating_pow(attempt.saturating_sub(1).min(6) as u32);
        sleep(Duration::from_secs(wait_secs));
    }
    Err(Error::Custom(format!(
        "V/F recheck failed after {max_attempts} attempts"
    )))
}

fn log_point_test_header<D: std::fmt::Display>(
    test_code: usize,
    point: usize,
    voltage: Microvolts,
    delta_label: &str,
    delta_value: D,
) {
    let now = local_time_hms();
    println!(
        "[{}] Test #{} on point: #{}, voltage: #{}, {}: #+{}. ",
        now, test_code, point, voltage, delta_label, delta_value
    );
}

pub(super) fn run_legacy_short_phase(
    l: &mut ScanLogWriter,
    gpu: &GpuTarget<'_>,
    args: &LegacyPhaseArgs<'_>,
    controller: &mut StepController,
    test_code: &mut usize,
    resuming_flag: &mut bool,
) -> Result<(), Error> {
    println!("Starting short test phase...");

    if *resuming_flag {
        *resuming_flag = false;
        println!(
            "Initial OC offset: {}kHz, current safe limit: {}kHz",
            controller.f_current, controller.f_max
        );
        if controller.is_converged() {
            println!("Skipping short test phase entirely (already converged).");
        }
    }

    loop {
        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(controller.f_current),
            )],
        )?;

        controller.test_progress_num += 1;
        *test_code += 1;

        println!(
            "[{}] Short Test #{} freq_delta: +{}kHz. ",
            local_time_hms(),
            *test_code,
            controller.f_current
        );
        println!("[DEBUG] StepController: {:?}", controller);

        let pressure_cfg = args.common.pressure_config(
            gpu,
            PressureRunSpec {
                point: args.point,
                flat_curve_flag: args.flat_curve_flag,
                vfp_set_range: args.vfp_set_range,
                init_core_oc_value: controller.f_current,
                test_code: format!("legacy{}{}", args.common.delimiter, *test_code),
                timeout_loops: args.common.test_duration,
                is_legacy_global_offset: true,
                test_duration_secs: args.common.test_duration,
            },
        );
        let started_at = begin_test_result_log(
            l,
            ScanDomain::Legacy,
            TestPhase::Short,
            *test_code,
            args.point,
            None,
            KilohertzDelta(controller.f_current),
        )?;
        let test_flag = run_pressure_test(gpu, args.common.matches, &pressure_cfg);
        finish_test_result_log(
            l,
            started_at,
            ScanDomain::Legacy,
            TestPhase::Short,
            *test_code,
            args.point,
            None,
            KilohertzDelta(controller.f_current),
            test_flag,
        )?;

        if test_flag != 0 {
            println!(
                "Short Test #{} FAILED at +{}kHz",
                *test_code, controller.f_current
            );
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
            )?;
            let decrease = controller.on_test_failed(args.common.minimum_delta_core_freq_step);
            println!("Decreasing target freq by {}kHz", decrease);
            println!("[DEBUG] StepController: {:?}", controller);
            continue;
        }

        println!(
            "Short Test #{} SUCCEEDED at +{}kHz",
            *test_code, controller.f_current
        );
        match controller.on_test_passed(args.common.minimum_delta_core_freq_step) {
            Some(increase) => {
                println!("Increasing target freq by {}kHz", increase);
                println!("[DEBUG] StepController: {:?}", controller);
            }
            None => break,
        }

        if controller.is_converged() {
            break;
        }
    }

    controller.reset_search_progress();
    println!(
        "Short test phase finished. Current freq_delta: +{}kHz",
        controller.f_current
    );
    Ok(())
}

pub(super) fn run_legacy_long_phase(
    l: &mut ScanLogWriter,
    gpu: &GpuTarget<'_>,
    args: &LegacyPhaseArgs<'_>,
    controller: &mut StepController,
    test_code: &mut usize,
) -> Result<(), Error> {
    println!("Initiating Long Test...");

    loop {
        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(controller.f_current),
            )],
        )?;

        *test_code += 1;
        println!(
            "[{}] Long Test #{} freq_delta: +{}kHz. ",
            local_time_hms(),
            *test_code,
            controller.f_current
        );

        let pressure_cfg = args.common.pressure_config(
            gpu,
            PressureRunSpec {
                point: args.point,
                flat_curve_flag: args.flat_curve_flag,
                vfp_set_range: args.vfp_set_range,
                init_core_oc_value: controller.f_current,
                test_code: format!("legacy{}{}", args.common.delimiter, *test_code),
                timeout_loops: args.common.endurance_coefficient * args.common.test_duration,
                is_legacy_global_offset: true,
                test_duration_secs: args.common.endurance_coefficient * args.common.test_duration,
            },
        );
        let started_at = begin_test_result_log(
            l,
            ScanDomain::Legacy,
            TestPhase::Long,
            *test_code,
            args.point,
            None,
            KilohertzDelta(controller.f_current),
        )?;
        let long_flag = run_pressure_test(gpu, args.common.matches, &pressure_cfg);
        finish_test_result_log(
            l,
            started_at,
            ScanDomain::Legacy,
            TestPhase::Long,
            *test_code,
            args.point,
            None,
            KilohertzDelta(controller.f_current),
            long_flag,
        )?;

        if long_flag != 0 {
            println!(
                "Long Test #{} FAILED at +{}kHz",
                *test_code, controller.f_current
            );
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
            )?;
            controller.apply_long_failure_step(args.common.minimum_delta_core_freq_step, false);
            println!(
                "Decreasing target freq by {}kHz",
                args.common.minimum_delta_core_freq_step
            );
            println!("[DEBUG] StepController: {:?}", controller);
            continue;
        }

        println!(
            "Long Test #{} SUCCEEDED at +{}kHz",
            *test_code, controller.f_current
        );
        println!("[DEBUG] StepController: {:?}", controller);
        break;
    }

    Ok(())
}

pub(super) struct GpuBoostPhaseArgs<'a> {
    // GPU Boost V3 phases apply per-point VFP deltas around the currently
    // tested voltage point.
    pub(super) common: CommonPhaseArgs<'a>,
    pub(super) vfp_set_range: usize,
    #[allow(dead_code)]
    pub(super) freq_step_exp: usize,
    pub(super) is_50_series: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_gpuboostv3_short_phase(
    l: &mut ScanLogWriter,
    gpu: &GpuTarget<'_>,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: Microvolts,
    flat_curve_flag: bool,
    controller: &mut StepController,
    resuming_flag: &mut bool,
) -> Result<usize, Error> {
    let mut test_code = 0;

    if *resuming_flag {
        *resuming_flag = false;
        if controller.is_converged() {
            println!("Skipping short test...");
            println!("Initiating Long Test...");
            controller.reset_search_progress();
            return Ok(test_code);
        } else {
            println!(
                "Initial OC offset:{}kHz, current safe limit:{}kHz",
                controller.f_current, controller.f_max
            );
        }
    }

    loop {
        set_vfp_and_recheck(
            gpu,
            point,
            args.vfp_set_range,
            flat_curve_flag,
            controller.f_current,
            args.common.minimum_delta_core_freq_step,
            10,
            args.common.minload_exe,
            args.common.cuda_device,
            args.common.wakeup_load_needed,
        )?;

        controller.test_progress_num += 1;
        test_code += 1;

        log_point_test_header(
            test_code,
            point,
            v,
            "freq_delta",
            KilohertzDelta(controller.f_current),
        );

        let pressure_cfg = args.common.pressure_config(
            gpu,
            PressureRunSpec {
                point,
                flat_curve_flag,
                vfp_set_range: args.vfp_set_range,
                init_core_oc_value: controller.f_current,
                test_code: format!("{}{}{}", point, args.common.delimiter, test_code),
                timeout_loops: args.common.test_duration,
                is_legacy_global_offset: false,
                test_duration_secs: args.common.test_duration,
            },
        );
        let started_at = begin_test_result_log(
            l,
            ScanDomain::Core,
            TestPhase::Short,
            test_code,
            point,
            Some(v),
            KilohertzDelta(controller.f_current),
        )?;
        let test_flag = run_pressure_test(gpu, args.common.matches, &pressure_cfg);
        println!("{}", test_flag);
        finish_test_result_log(
            l,
            started_at,
            ScanDomain::Core,
            TestPhase::Short,
            test_code,
            point,
            Some(v),
            KilohertzDelta(controller.f_current),
            test_flag,
        )?;

        if test_flag != 0 {
            run_output(
                gpu,
                ResetVfpDeltas {
                    domain: VfpResetDomain::Core,
                },
            )?;
            println!(
                "Test #{} FAILED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
                test_code,
                point,
                v,
                KilohertzDelta(controller.f_current)
            );
            let decrease = controller.on_test_failed(args.common.minimum_delta_core_freq_step);
            // if args.is_50_series {
            //     controller
            //         .apply_50_series_failure_penalty(args.common.minimum_delta_core_freq_step);
            //     println!(
            //         "Additional safety: Decreasing target freq by {}kHz",
            //         args.common.minimum_delta_core_freq_step
            //     );
            // }
            println!("Decreasing target freq by {}kHz", decrease);
            println!("[DEBUG] StepController: {:?}", controller);
            continue;
        }

        println!(
            "Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            test_code,
            point,
            v,
            KilohertzDelta(controller.f_current)
        );
        match controller.on_test_passed(args.common.minimum_delta_core_freq_step) {
            Some(increase) => {
                println!("Increasing target freq by {}kHz", increase);
                println!("[DEBUG] StepController: {:?}", controller);
            }
            None => break,
        }

        if controller.is_converged() {
            println!(
                "Short test phase finished. Current freq_delta: +{}kHz",
                controller.f_current
            );
            break;
        }
    }
    controller.reset_search_progress();
    Ok(test_code)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_gpuboostv3_long_phase(
    l: &mut ScanLogWriter,
    gpu: &GpuTarget<'_>,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: Microvolts,
    flat_curve_flag: bool,
    controller: &mut StepController,
    test_code: &mut usize,
) -> Result<(), Error> {
    let mut long_duration_flag;
    println!("Initiating Long Test...");

    loop {
        set_vfp_and_recheck(
            gpu,
            point,
            args.vfp_set_range,
            flat_curve_flag,
            controller.f_current,
            args.common.minimum_delta_core_freq_step,
            5,
            args.common.minload_exe,
            args.common.cuda_device,
            args.common.wakeup_load_needed,
        )?;

        *test_code += 1;
        log_point_test_header(
            *test_code,
            point,
            v,
            "freq_delta",
            KilohertzDelta(controller.f_current),
        );

        let pressure_cfg = args.common.pressure_config(
            gpu,
            PressureRunSpec {
                point,
                flat_curve_flag,
                vfp_set_range: args.vfp_set_range,
                init_core_oc_value: controller.f_current,
                test_code: format!("{}{}{}", point, args.common.delimiter, *test_code),
                timeout_loops: args.common.endurance_coefficient * args.common.test_duration,
                is_legacy_global_offset: false,
                test_duration_secs: args.common.endurance_coefficient * args.common.test_duration,
            },
        );
        let started_at = begin_test_result_log(
            l,
            ScanDomain::Core,
            TestPhase::Long,
            *test_code,
            point,
            Some(v),
            KilohertzDelta(controller.f_current),
        )?;
        long_duration_flag = run_pressure_test(gpu, args.common.matches, &pressure_cfg);
        finish_test_result_log(
            l,
            started_at,
            ScanDomain::Core,
            TestPhase::Long,
            *test_code,
            point,
            Some(v),
            KilohertzDelta(controller.f_current),
            long_duration_flag,
        )?;
        if long_duration_flag != 0 {
            run_output(
                gpu,
                ResetVfpDeltas {
                    domain: VfpResetDomain::Core,
                },
            )?;
            println!(
                "Long Test #{} FAILED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
                *test_code,
                point,
                v,
                KilohertzDelta(controller.f_current)
            );
            controller.apply_long_failure_step(
                args.common.minimum_delta_core_freq_step,
                args.is_50_series,
            );
            println!(
                "Decreasing target freq by {}kHz",
                args.common.minimum_delta_core_freq_step
            );
            // if args.is_50_series {
            //     println!(
            //         "Additional safety: Decreasing target freq by {}kHz",
            //         args.common.minimum_delta_core_freq_step
            //     )
            // }
            println!("[DEBUG] StepController: {:?}", controller);
            continue;
        }

        println!(
            "Long Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            *test_code,
            point,
            v,
            KilohertzDelta(controller.f_current)
        );
        println!("[DEBUG] StepController: {:?}", controller);
        break;
    }

    Ok(())
}

pub(super) struct MemOcPhaseArgs<'a> {
    // Memory OC reuses the upper voltage point and tests memory clock offsets
    // while keeping core VFP setup stable.
    pub(super) common: CommonPhaseArgs<'a>,
    pub(super) point: usize,
    pub(super) vfp_set_range: usize,
    pub(super) minimum_delta_mem_freq_step: i32,
    #[allow(dead_code)]
    pub(super) mem_freq_step_exp: usize,
}

pub(super) fn run_mem_oc_phase(
    l: &mut ScanLogWriter,
    gpu: &GpuTarget<'_>,
    gpus: &Vec<GpuTarget<'_>>,
    args: &MemOcPhaseArgs<'_>,
    mem_voltage: Microvolts,
    controller: &mut StepController,
) -> Result<(), Error> {
    let mut mem_test_code: usize = 0;

    loop {
        let _pulse = if args.common.wakeup_load_needed {
            Some(MinLoadPulse::wake(
                args.common.minload_exe,
                args.common.cuda_device,
            ))
        } else {
            None
        };
        match handle_lock_vfp(gpus, args.common.matches, args.point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Memory,
                KilohertzDelta(controller.f_current),
            )],
        )?;

        sync_memory_pstate_as_p0(gpu)?;

        controller.test_progress_num += 1;
        mem_test_code += 1;

        log_point_test_header(
            mem_test_code,
            args.point,
            mem_voltage,
            "mem_freq_delta",
            KilohertzDelta(controller.f_current),
        );

        let pressure_cfg = args.common.pressure_config(
            gpu,
            PressureRunSpec {
                point: args.point,
                flat_curve_flag: false,
                vfp_set_range: args.vfp_set_range,
                init_core_oc_value: 0,
                test_code: format!("{}{}{}", args.point, args.common.delimiter, mem_test_code),
                timeout_loops: args.common.endurance_coefficient * args.common.test_duration,
                is_legacy_global_offset: true,
                test_duration_secs: args.common.endurance_coefficient * args.common.test_duration,
            },
        );
        let started_at = begin_test_result_log(
            l,
            ScanDomain::Memory,
            TestPhase::Long,
            mem_test_code,
            args.point,
            Some(mem_voltage),
            KilohertzDelta(controller.f_current),
        )?;
        let mem_test_flag = run_pressure_test(gpu, args.common.matches, &pressure_cfg);
        finish_test_result_log(
            l,
            started_at,
            ScanDomain::Memory,
            TestPhase::Long,
            mem_test_code,
            args.point,
            Some(mem_voltage),
            KilohertzDelta(controller.f_current),
            mem_test_flag,
        )?;

        if mem_test_flag != 0 {
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))],
            )?;
            println!(
                "Long Test #{} FAILED on point: #{}, voltage: #{}, mem_freq_delta: #+{}. ",
                mem_test_code,
                args.point,
                mem_voltage,
                KilohertzDelta(controller.f_current)
            );

            let decrease = controller.on_test_failed(args.minimum_delta_mem_freq_step);
            println!("Decreasing target mem_freq by {}kHz", decrease);
            println!("[DEBUG] StepController: {:?}", controller);
            continue;
        }

        println!(
            "Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            mem_test_code,
            args.point,
            mem_voltage,
            KilohertzDelta(controller.f_current)
        );
        match controller.on_test_passed(args.minimum_delta_mem_freq_step) {
            Some(increase) => {
                println!("Increasing target freq by {}kHz", increase);
                println!("[DEBUG] StepController: {:?}", controller);
            }
            None => break,
        }

        if controller.is_converged() {
            break;
        }
    }

    Ok(())
}
