mod phases;
mod pressure;
mod runtime;

// Keep this file as the scan orchestrator. The submodules own the lower-level
// phase loops, stressor process runtime, and retry/wake helpers.
use self::phases::{
    CommonPhaseArgs, GpuBoostPhaseArgs, LegacyPhaseArgs, MemOcPhaseArgs, run_gpuboostv3_long_phase,
    run_gpuboostv3_short_phase, run_legacy_long_phase, run_legacy_short_phase, run_mem_oc_phase,
};
use self::runtime::{MinLoadPulse, retry_operation_with_backoff, run_output};
use super::oc_profile_function::{
    apply_autoscan_profile, break_point_continue, check_voltage_points, export_single_point,
    key_point_extractor,
};
use super::progressbar::{ActiveScanProgressGuard, ScanProgress};
use super::scan_log::{GpuVoltageRange, ScanArea, ScanKind, ScanLogWriter, ScanMode};
use super::scan_strategy::{FluctuationMode, FluctuationStrategy, StepController};
use super::scan_support::local_time_hms;
use super::scan_support::{handle_lock_vfp, handle_test_voltage_limits, print_scan_separator};
use clap::ArgMatches;
use nvoc_core::{
    ClockDomain, Error, GpuOcParams, GpuTarget, KilohertzDelta, PState, QueryGpuInfo,
    QueryGpuStatus, QueryVfpPointVoltage, SetVfpPointDelta, VfPoint, VfPointType, fetch_gpu_type,
    set_nvapi_pstate_clock_offsets,
};
use std::sync::Arc;

// use standard println!/eprintln!; do not route prints through progressbar helper

pub fn autoscan_gpuboostv3(gpus: &Vec<GpuTarget<'_>>, matches: &ArgMatches) -> Result<(), Error> {
    use super::autoscan_config::GpuBoostAutoscanConfig;
    let cfg = GpuBoostAutoscanConfig::from_autoscan_matches(matches)?;
    let common = &cfg.common;
    // Borrow stressor settings here so every phase uses the same CUDA device
    // mapping and extra wrapper args.
    let cuda_device = common.stressor.cuda_device;
    let stressor_extra_args = common.stressor.extra_args.as_slice();
    let mut is_ultrafast = cfg.is_ultrafast;
    if is_ultrafast {
        println!("Ultrafast mode interpolation active...");
    }

    let test_exe = common.test_exe.as_str();
    let minload_exe = common.minload_exe.as_str();
    let log_filename = common.log.as_str();
    let mut l = ScanLogWriter::open_append(log_filename)?;
    let delimiter: String = String::from("--");

    let mut p1 = 0;
    let mut p2 = 0;
    let mut p3 = 0;
    let mut p4 = 0;
    let mut ultrafast_point_extraction_flag = false;

    let (lower_voltage_point, upper_voltage_point) = match check_voltage_points(log_filename)? {
        Some((
            read_lower_voltage_point,
            read_upper_voltage_point,
            maybe_p1,
            maybe_p2,
            maybe_p3,
            maybe_p4,
        )) => {
            println!(
                "Volt scan skipped. Parsed: Low = {}, Up = {}",
                read_lower_voltage_point, read_upper_voltage_point
            );

            if is_ultrafast {
                println!("Ultrafast mode active...");
                if let (Some(v1), Some(v2), Some(v3), Some(v4)) =
                    (maybe_p1, maybe_p2, maybe_p3, maybe_p4)
                {
                    p1 = v1;
                    p2 = v2;
                    p3 = v3;
                    p4 = v4;
                    println!("Active Points:{},{},{},{}", p1, p2, p3, p4);
                    ultrafast_point_extraction_flag = true;
                }
            }

            (
                read_lower_voltage_point as usize,
                read_upper_voltage_point as usize,
            )
        }

        None => {
            println!("Voltage scan initialized because values were missing in the log.");
            // New logs need a read-only voltage range probe before any per-point
            // VFP writes are attempted.
            let voltage_limits = retry_operation_with_backoff(
                || handle_test_voltage_limits(gpus, matches, print_scan_separator),
                "ProbeVoltageLimits",
                8,
                1,
                minload_exe,
                cuda_device,
            )?;
            let lvp = voltage_limits
                .iter()
                .map(|limits| limits.lower_point)
                .max()
                .ok_or_else(|| Error::from("no GPU voltage limits were probed"))?;
            let uvp = voltage_limits
                .iter()
                .map(|limits| limits.upper_point)
                .min()
                .ok_or_else(|| Error::from("no GPU voltage limits were probed"))?;
            if lvp > uvp {
                return Err(Error::Custom(format!(
                    "selected GPUs have no common voltage point range: lower point {lvp}, upper point {uvp}"
                )));
            }

            let mut gpu_ranges = Vec::new();
            for limits in &voltage_limits {
                let gpu = gpus
                    .iter()
                    .find(|gpu| gpu.id.0 == limits.gpu_id)
                    .ok_or_else(|| Error::from("probed GPU was not found in selected targets"))?;
                let minimum_voltage = run_output(
                    gpu,
                    QueryVfpPointVoltage {
                        point: limits.lower_point,
                    },
                )?;
                let maximum_voltage = run_output(
                    gpu,
                    QueryVfpPointVoltage {
                        point: limits.upper_point,
                    },
                )?;
                println!(
                    "GPU {} minimum_voltage_point: {} @ {}",
                    limits.gpu_id, limits.lower_point, minimum_voltage
                );
                println!(
                    "GPU {} maximum_voltage_point: {} @ {}",
                    limits.gpu_id, limits.upper_point, maximum_voltage
                );
                gpu_ranges.push(GpuVoltageRange {
                    gpu_id: limits.gpu_id,
                    lower_point: limits.lower_point,
                    upper_point: limits.upper_point,
                    minimum_voltage_uv: minimum_voltage.0,
                    maximum_voltage_uv: maximum_voltage.0,
                });
            }
            println!("common_voltage_point_range: {}-{}", lvp, uvp);
            l.write_voltage_range(lvp, uvp, gpu_ranges)?;

            (lvp, uvp)
        }
    };

    let init_vmem_oc_value = 0;

    for gpu in gpus {
        set_nvapi_pstate_clock_offsets(
            gpu,
            [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))],
        )?;

        let info = run_output(gpu, QueryGpuInfo)?;
        let gpu_type = fetch_gpu_type(&info);

        run_output(
            gpu,
            SetVfpPointDelta {
                point: upper_voltage_point,
                delta: KilohertzDelta(45000),
            },
        )?;
        let _pulse = MinLoadPulse::wake(minload_exe, cuda_device);
        match handle_lock_vfp(gpus, matches, upper_voltage_point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        // 从 GpuType 读取该世代的固定 OC 扫描参数
        let GpuOcParams {
            minimum_delta_core_freq_step,
            core_oc_safe_limit,
            init_core_oc_value,
            safe_elasticity_per_cycle,
            fluctuation_coefficient,
            is_50_series,
            testing_step,
            freq_step_exp_core,
        } = gpu_type.as_ref().map(|t| t.oc_params()).unwrap_or_default();

        let freq_step_exp = freq_step_exp_core;

        // let scan_params = ScanParams {
        //     is_50_series,
        //     enable_arch_safety_policy: true,
        //     ..ScanParams::default()
        // };

        // Retry QueryGpuStatus with backoff to handle transient GPU power state issues
        let status = retry_operation_with_backoff(
            || run_output(gpu, QueryGpuStatus),
            "QueryGpuStatus (initial VFP load)",
            8, // attempts
            1, // base_wait_secs
            minload_exe,
            cuda_device,
        )?;
        let points = status.vfp.ok_or(Error::VfpUnsupported)?.graphics;

        let mut point = lower_voltage_point;
        let mut resuming_flag = false;
        let mut last_succeeded_freq = init_core_oc_value;
        let mut last_failed_freq = core_oc_safe_limit;
        let recovery_method_switch: bool = common.recovery_method.unwrap_or(is_50_series);

        let (succeeded_freq, failed_freq, last_voltage_point, ultrafast_flag) =
            break_point_continue(log_filename, testing_step)?;
        println!("Extracted Values:");

        if let Some(freq_s) = succeeded_freq {
            println!("  - Last freq_delta succeeded: {} MHz", freq_s);
            last_succeeded_freq = (freq_s * 1000.0) as i32; // Update if present
        }

        if let Some(freq_f) = failed_freq {
            println!("  - Last freq_delta failed: {} MHz", freq_f);
            last_failed_freq = (freq_f * 1000.0) as i32; // Update if present
        }

        if let Some(voltage_point) = last_voltage_point {
            if voltage_point < lower_voltage_point || voltage_point > upper_voltage_point {
                eprintln!(
                    "Warning: ignoring resume point {} outside current voltage range {}-{}.",
                    voltage_point, lower_voltage_point, upper_voltage_point
                );
            } else {
                println!("  - Last voltage point: {}", voltage_point);
                point = voltage_point; // Update if present
                resuming_flag = true;
            }
        }

        if let Some(ultrafast_flag) = ultrafast_flag {
            println!("Inheriting last scanner flag...");
            is_ultrafast = ultrafast_flag; // Update if present
            resuming_flag = true;
        }

        if is_ultrafast {
            if !ultrafast_point_extraction_flag {
                // Use the parsed --initcsv path; the default remains
                // ./ws/vfp-init.csv through clap/platform defaults.
                (p1, p2, p3, p4) = key_point_extractor(
                    gpus,
                    lower_voltage_point,
                    upper_voltage_point,
                    cfg.init_csv.as_str(),
                )?;
            }

            if p1.saturating_sub(6) < lower_voltage_point {
                p1 = lower_voltage_point + 6;
            }
            if p2 < lower_voltage_point {
                p2 = lower_voltage_point + 10;
            }
            if p3 > upper_voltage_point {
                p3 = upper_voltage_point.saturating_sub(10);
            }
            if p4 > upper_voltage_point {
                p4 = upper_voltage_point;
            }

            // stair bias
            if is_50_series && p1 == p2 {
                p1 = p1.saturating_sub(6);
                p2 += 6;
            }
            if is_50_series && p2 == p3 {
                p2 = p2.saturating_sub(6);
                p3 += 6;
            }

            if p2 > p3 {
                std::mem::swap(&mut p2, &mut p3);
            }

            println!("key points detected:{},{},{},{}", p1, p2, p3, p4);
            l.write_key_points([p1, p2, p3, p4])?;

            println!("Scan in ultrafast mode...");
            l.write_scan_mode(ScanMode::Ultrafast)?;
        } else {
            println!("Scan in normal mode...");
            l.write_scan_mode(ScanMode::Normal)?;
        }

        let mut controller;

        let mut fc = last_succeeded_freq;
        let mut fm = last_failed_freq;
        if fm < fc {
            println!("log parsing error... Restoring default value");
            fm = core_oc_safe_limit;
            fc -= safe_elasticity_per_cycle;
        }
        controller = StepController::init_from_resume(
            fc,
            fm,
            safe_elasticity_per_cycle,
            minimum_delta_core_freq_step,
            freq_step_exp,
        );

        print_scan_separator();
        if resuming_flag {
            println!("Resuming on point {}:", point);
        } else {
            println!("Initiating on lowest point: #{}", point);
        }
        print_scan_separator();

        if point == 0 {
            return Ok(());
        }

        let scan_progress = Arc::new(ScanProgress::new(lower_voltage_point, upper_voltage_point));
        let _scan_progress_guard = ActiveScanProgressGuard::enter(scan_progress.clone());
        scan_progress.set_total_point(point, lower_voltage_point, upper_voltage_point);

        let mut v;
        let mut default_frequency;
        let mut prev_endpoint_delta: Option<i32> = None;

        //prepare GPU OC parameter for extreme OC...
        if let Err(e) = apply_autoscan_profile(gpu, matches, 80) {
            eprintln!("apply_autoscan_profile failed: {:?}, continuing scan...", e);
        }

        let endurance_coefficient = 2;
        let vfp_set_range = 3;
        let mut test_duration: u64 = 10;
        if is_ultrafast {
            test_duration += test_duration / 2;
        };
        let mut flat_curve_flag: bool;
        let phase_args = GpuBoostPhaseArgs {
            common: CommonPhaseArgs {
                matches,
                minimum_delta_core_freq_step,
                fluctuation: FluctuationStrategy::Toggle {
                    mode: FluctuationMode::PositiveOnly,
                    coefficient: fluctuation_coefficient,
                },
                test_exe,
                minload_exe,
                delimiter: delimiter.as_str(),
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
                progress: Some(scan_progress.as_ref()),
                cuda_device,
                stressor_extra_args,
            },
            vfp_set_range,
            freq_step_exp,
            is_50_series,
        };

        // core oc scanning
        println!("New Test Initiated at {}", local_time_hms());
        while point <= upper_voltage_point {
            if is_ultrafast {
                if (point < p1 && p1 != 0) || (point == p1 && resuming_flag) {
                    point = p1;
                } else if (point < p2 && p2 != 0) || (point == p2 && resuming_flag) {
                    point = p2;
                } else if (point < p3 && p3 != 0) || (point == p3 && resuming_flag) {
                    point = p3;
                } else if (point < p4 && p4 != 0) || (point == p4 && resuming_flag) {
                    point = p4;
                } else {
                    println!("ultra_fast_scan_finished...");
                    break;
                }
            } else {
                point += testing_step;
                if resuming_flag {
                    point -= testing_step;
                }
            }

            if point > upper_voltage_point {
                break;
            }

            scan_progress.set_total_point(point, lower_voltage_point, upper_voltage_point);

            v = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .voltage;
            default_frequency = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .default_frequency;

            let _pulse = MinLoadPulse::wake(minload_exe, cuda_device);
            match handle_lock_vfp(gpus, matches, point, true) {
                Ok(_) => {
                    flat_curve_flag = false;
                }
                Err(_e) => {
                    flat_curve_flag = true;
                }
            }

            // if scan_params.enable_arch_safety_policy {
            //     scan_strategy::apply_arch_safety_policy(
            //         &scan_params,
            //         ArchSafetyPhase::PrePointTest,
            //         v.0,
            //         &mut controller.f_current,
            //         &mut controller.f_max,
            //         &mut core_oc_safe_limit_ref,
            //         safe_elasticity_per_cycle,
            //     );
            // }

            let mut test_code = run_gpuboostv3_short_phase(
                &mut l,
                gpu,
                &phase_args,
                point,
                v,
                flat_curve_flag,
                &mut controller,
                &mut resuming_flag,
            )?;
            println!(
                "Short Test #{} finished on point: #{} , voltage: #{}, delta: #+{}. ",
                test_code,
                point,
                v,
                KilohertzDelta(controller.f_current)
            );
            run_gpuboostv3_long_phase(
                &mut l,
                gpu,
                &phase_args,
                point,
                v,
                flat_curve_flag,
                &mut controller,
                &mut test_code,
            )?;
            l.write_point_finished(ScanArea::Core, point)?;
            println!(
                "Core OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                v,
                KilohertzDelta(controller.f_current)
            );

            let p_save = VfPoint {
                point_type: VfPointType::Prog,
                voltage: v,
                frequency: default_frequency + KilohertzDelta(controller.f_current),
                delta: KilohertzDelta(controller.f_current),
                default_frequency,
            };
            let _ = export_single_point(p_save, matches);
            // interpolate when not in ultrafast mode.
            if !is_ultrafast {
                let prev_delta = prev_endpoint_delta.unwrap_or(controller.f_current);
                let current_delta = controller.f_current;
                let bin = minimum_delta_core_freq_step.max(1) as i64;

                for step in 1..testing_step {
                    // Linear interpolation between previous and current endpoint delta.
                    let numerator = prev_delta as i64 * step as i64
                        + current_delta as i64 * (testing_step - step) as i64;
                    let denominator = testing_step as i64;
                    let bin_denominator = denominator * bin;

                    let interpolated_delta = if numerator % bin_denominator == 0 {
                        (numerator / denominator) as i32
                    } else {
                        (numerator.div_euclid(bin_denominator) * bin) as i32
                    };

                    let v_prev = points
                        .get(&(point - step))
                        .ok_or(Error::Str("invalid point index"))?
                        .voltage;
                    let p_save_prev = VfPoint {
                        point_type: VfPointType::Prog,
                        voltage: v_prev,
                        frequency: default_frequency + KilohertzDelta(interpolated_delta),
                        delta: KilohertzDelta(interpolated_delta),
                        default_frequency,
                    };
                    let _ = export_single_point(p_save_prev, matches);
                }
            }
            prev_endpoint_delta = Some(controller.f_current);

            // if scan_params.enable_arch_safety_policy {
            //     scan_strategy::apply_arch_safety_policy(
            //         &scan_params,
            //         ArchSafetyPhase::PostPointTest,
            //         v.0,
            //         &mut controller.f_current,
            //         &mut controller.f_max,
            //         &mut core_oc_safe_limit_ref,
            //         safe_elasticity_per_cycle,
            //     );
            // }
            controller.f_current -= safe_elasticity_per_cycle;
            controller.f_max += safe_elasticity_per_cycle;
            println!(
                "Reset init core oc value {}, OC safe limit to {}",
                controller.f_current, controller.f_max
            );
        }

        //memory oc
        if cfg.vmem_scan {
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))],
            )?;

            let mut mem_oc_safe_limit = 0;
            let minimum_delta_mem_freq_step = 1000;
            let mem_freq_step_exp = 8;

            // Retry QueryGpuStatus with backoff for memory OC readout
            let status = retry_operation_with_backoff(
                || run_output(gpu, QueryGpuStatus),
                "QueryGpuStatus (memory OC readout)",
                5, // attempts
                1, // base_wait_secs
                minload_exe,
                cuda_device,
            )?;
            let readout_f = status.clone().clocks;
            let mut clocks = Vec::new();
            for (clock_name, freq) in readout_f {
                // Store the clock name and frequency in a data structure.
                clocks.push((clock_name.to_string(), freq));
            }
            if let Some((_, memory_clock)) = clocks.iter().find(|(name, _)| name.contains("Memory"))
            {
                println!(
                    "{}: {}",
                    nvoc_cli_common::color::stylize_title("Memory Clock"),
                    nvoc_cli_common::color::stylize(&format!("{}", memory_clock), false)
                );
                println!(
                    "{} {}",
                    nvoc_cli_common::color::stylize_title("Memory OC test start at"),
                    nvoc_cli_common::color::stylize(
                        &format!(
                            "+{} MHz(+{}%)",
                            init_vmem_oc_value / 1000,
                            100 * init_vmem_oc_value / memory_clock.0 as i32
                        ),
                        false
                    )
                );
                mem_oc_safe_limit = memory_clock.0 as i32 / 8;
            };

            point = upper_voltage_point;
            let mem_voltage = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .voltage;

            let mem_phase_args = MemOcPhaseArgs {
                common: CommonPhaseArgs {
                    matches,
                    minimum_delta_core_freq_step,
                    fluctuation: FluctuationStrategy::Toggle {
                        mode: FluctuationMode::PositiveOnly,
                        coefficient: fluctuation_coefficient,
                    },
                    test_exe,
                    minload_exe,
                    delimiter: delimiter.as_str(),
                    recovery_method_switch,
                    test_duration,
                    endurance_coefficient,
                    progress: Some(scan_progress.as_ref()),
                    cuda_device,
                    stressor_extra_args,
                },
                point,
                vfp_set_range,
                minimum_delta_mem_freq_step,
                mem_freq_step_exp,
            };

            let mut mem_controller = StepController::new(
                init_vmem_oc_value,
                mem_oc_safe_limit,
                0,
                minimum_delta_mem_freq_step,
                mem_freq_step_exp,
            );

            run_mem_oc_phase(
                &mut l,
                gpu,
                gpus,
                &mem_phase_args,
                mem_voltage,
                &mut mem_controller,
            )?;
            l.write_point_finished(ScanArea::Memory, point)?;
            println!(
                "mem OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                mem_voltage,
                KilohertzDelta(mem_controller.f_current)
            );
        }
    }
    l.write_scan_completed(ScanKind::GpuBoostV3)?;
    Ok(())
}

pub fn autoscan_legacy(gpus: &Vec<GpuTarget<'_>>, matches: &ArgMatches) -> Result<(), Error> {
    use super::autoscan_config::LegacyAutoscanConfig;
    let cfg = LegacyAutoscanConfig::from_legacy_matches(matches)?;
    let common = &cfg.common;
    // Legacy scans use the same stressor wrapper settings but skip all VFP
    // curve-specific config.
    let cuda_device = common.stressor.cuda_device;
    let stressor_extra_args = common.stressor.extra_args.as_slice();
    let test_exe = common.test_exe.as_str();
    let minload_exe = common.minload_exe.as_str();
    let log_filename = common.log.as_str();
    let mut l = ScanLogWriter::open_append(log_filename)?;
    let delimiter: String = String::from("--");

    // Legacy GPU: single global offset, no V-F curve scanning
    // Use a fixed "point" value just as a placeholder for test_pressure interface
    let point: usize = 50;

    for gpu in gpus {
        // 从 GpuType 读取该世代的固定 OC 扫描参数
        let info = run_output(gpu, QueryGpuInfo)?;
        let gpu_type = fetch_gpu_type(&info);

        let GpuOcParams {
            minimum_delta_core_freq_step,
            core_oc_safe_limit,
            init_core_oc_value,
            safe_elasticity_per_cycle,
            fluctuation_coefficient,
            is_50_series: _, // legacy 路径不区分架构世代
            testing_step: _,
            freq_step_exp_core,
        } = gpu_type.as_ref().map(|t| t.oc_params()).unwrap_or_default();

        let core_oc_safe_limit_ref = core_oc_safe_limit;

        let freq_step_exp = freq_step_exp_core;

        // --- Breakpoint resume logic (mirrors v3) ---
        let mut resuming_flag = false;
        let mut last_succeeded_freq = init_core_oc_value;
        let mut last_failed_freq = core_oc_safe_limit_ref;

        let (succeeded_freq, failed_freq, last_voltage_point, _ultrafast_flag) =
            break_point_continue(log_filename, 1 /* single point, step=1 */)?;
        if let Some(freq_s) = succeeded_freq {
            println!("  - Last freq_delta succeeded: {} MHz", freq_s);
            last_succeeded_freq = (freq_s * 1000.0) as i32;
        }
        if let Some(freq_f) = failed_freq {
            println!("  - Last freq_delta failed: {} MHz", freq_f);
            last_failed_freq = (freq_f * 1000.0) as i32;
        }
        if last_voltage_point.is_some() {
            // For legacy, any breakpoint means we can resume
            resuming_flag = true;
            println!("Resuming legacy scan from breakpoint...");
        }

        // Apply breakpoint-restored values
        let mut controller;
        {
            let mut fc = last_succeeded_freq;
            let mut fm = last_failed_freq;
            if fm < fc {
                println!("log parsing error... Restoring default value");
                fm = core_oc_safe_limit_ref;
                fc -= safe_elasticity_per_cycle;
                controller = StepController::new(
                    fc,
                    fm,
                    safe_elasticity_per_cycle,
                    minimum_delta_core_freq_step,
                    freq_step_exp,
                );
            } else {
                controller = StepController::init_from_resume(
                    fc,
                    fm,
                    safe_elasticity_per_cycle,
                    minimum_delta_core_freq_step,
                    freq_step_exp,
                );
            }
        }

        if let Err(e) = apply_autoscan_profile(gpu, matches, 80) {
            eprintln!("apply_autoscan_profile failed: {:?}, continuing scan...", e);
        }

        let recovery_method_switch: bool = common.recovery_method.unwrap_or(false);

        let endurance_coefficient = 2;
        let vfp_set_range = 0; // unused for legacy but required by test_pressure signature
        let test_duration: u64 = 10;
        let flat_curve_flag = false; // not applicable for legacy

        let mut test_code: usize = 0;

        println!("Legacy Scan Initiated at {}", local_time_hms());
        l.write_scan_mode(ScanMode::Legacy)?;
        print_scan_separator();
        println!("autoscan_legacy: single global core OC offset mode (Maxwell / pre-Pascal)");
        println!(
            "Initial OC offset: {}kHz, safe limit: {}kHz",
            controller.f_current, controller.f_max
        );
        print_scan_separator();

        let phase_args = LegacyPhaseArgs {
            common: CommonPhaseArgs {
                matches,
                minimum_delta_core_freq_step,
                fluctuation: FluctuationStrategy::Toggle {
                    mode: FluctuationMode::PositiveOnly,
                    coefficient: fluctuation_coefficient,
                },
                test_exe,
                minload_exe,
                delimiter: delimiter.as_str(),
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
                progress: None,
                cuda_device,
                stressor_extra_args,
            },
            point,
            flat_curve_flag,
            vfp_set_range,
            freq_step_exp,
        };

        for gpu in gpus {
            // Memory: keep at stock for legacy scan
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))],
            )?;

            run_legacy_short_phase(
                &mut l,
                gpu,
                &phase_args,
                &mut controller,
                &mut test_code,
                &mut resuming_flag,
            )?;

            run_legacy_long_phase(&mut l, gpu, &phase_args, &mut controller, &mut test_code)?;

            l.write_point_finished(ScanArea::Legacy, point)?;
            println!(
                "Legacy OC scan finished. Final freq_delta: +{}kHz",
                controller.f_current
            );

            // Restore GPU to stock offset after scan
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
            )?;
        }
    }

    l.write_scan_completed(ScanKind::Legacy)?;
    Ok(())
}
