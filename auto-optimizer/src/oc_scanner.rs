use crate::basic_func::TestResolution::R1680x1050;
use crate::basic_func::{TestResolution, get_second_largest_resolution, local_time_hms};
use crate::error::Error;
use crate::handle_reset_nvml_cooler_single_gpu;
use crate::human::print_scan_separator;
use crate::nvidia_gpu_type::{GpuOcParams, fetch_gpu_type};
use crate::oc_get_set_function_nvapi::{
    core_reset_vfp, get_resolution, get_voltage_by_point, handle_lock_vfp,
    handle_test_voltage_limits, set_resolution, set_vfp_curve, set_vfp_curve_warn,
    voltage_frequency_check,
};
use crate::oc_profile_function::{
    apply_autoscan_profile, break_point_continue, check_voltage_points, export_single_point,
    key_point_extractor,
};
use crate::platform::stressor_3d_conf_path;
use clap::ArgMatches;
use core::time;
use num_traits::pow;
use nvapi_hi::Gpu;
use nvapi_hi::{ClockDomain, KilohertzDelta, PState};
use nvml_wrapper::Nvml;
use std::cmp::min;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{fs, iter};

mod scan_shared {
    use super::*;

    pub(super) fn pick_initial_resolution(
        resuming_flag: bool,
        test_conf_file_path: Option<&str>,
    ) -> TestResolution {
        if resuming_flag {
            if let Some(path) = test_conf_file_path {
                return get_resolution(path).unwrap_or(TestResolution::R1920x1200);
            }
            return TestResolution::R1920x1200;
        }

        match get_second_largest_resolution() {
            Some(resolution) => {
                let (width, height) = resolution.dimensions();
                println!("Second largest fitting resolution: {}x{}", width, height);
                resolution
            }
            None => {
                println!("No fitting resolution found.");
                TestResolution::R1920x1200
            }
        }
    }

    pub(super) fn sync_resolution(test_conf_file_path: Option<&str>, resolution: TestResolution) {
        if let Some(path) = test_conf_file_path {
            match set_resolution(path, resolution) {
                Ok(_) => println!("Resolution updated successfully to {:?}.", resolution),
                Err(e) => eprintln!("Failed to update resolution: {}", e),
            }
        } else {
            println!("Skipping resolution sync because no stressor game config is available.");
        }
    }
}

mod pressure_runner {
    use super::*;

    pub(super) struct TestPressureConfig<'a> {
        pub(super) point: usize,
        pub(super) flat_curve_flag: bool,
        pub(super) vfp_set_range: usize,
        pub(super) init_core_oc_value: i32,
        pub(super) minimum_delta_core_freq_step: i32,
        pub(super) fluctuation_coefficient: i32,
        pub(super) fluctuation_mode: usize,
        pub(super) test_resolution: TestResolution,
        pub(super) is_mem_test: bool,
        pub(super) test_exe: &'a str,
        pub(super) test_code: String,
        pub(super) timeout_loops: u64,
        pub(super) test_conf_file_path: Option<&'a str>,
        pub(super) recovery_method: bool,
        pub(super) is_legacy_global_offset: bool,
    }

    fn test_initialization(gpu: &&Gpu, cfg: &TestPressureConfig<'_>) {
        if cfg.is_legacy_global_offset {
            gpu.inner()
                .set_pstates(
                    [(
                        PState::P0,
                        ClockDomain::Graphics,
                        KilohertzDelta(cfg.init_core_oc_value),
                    )]
                    .iter()
                    .cloned(),
                )
                .unwrap_or_else(|e| {
                    eprintln!("Warning:{}, initializing Error. GPU crashed...", e);
                });
            return;
        }

        let main_delta = cfg.init_core_oc_value - cfg.minimum_delta_core_freq_step;
        let lower_delta = Some(cfg.init_core_oc_value - 2 * cfg.minimum_delta_core_freq_step);
        set_vfp_curve_warn(
            gpu,
            cfg.point,
            cfg.vfp_set_range,
            cfg.flat_curve_flag,
            main_delta,
            lower_delta,
        );
    }

    fn apply_fluctuation(
        gpu: &&Gpu,
        cfg: &TestPressureConfig<'_>,
        fluctuation_h_l_flag: bool,
    ) -> bool {
        let (fluctuation_freq, new_h_l_flag) = if !fluctuation_h_l_flag {
            let freq = if cfg.fluctuation_mode == 3 {
                0
            } else {
                -cfg.fluctuation_coefficient * cfg.minimum_delta_core_freq_step
            };
            println!("fluctuation state changing to LOW...");
            (freq, true)
        } else {
            let freq = if cfg.fluctuation_mode == 2 || cfg.fluctuation_mode == 3 {
                cfg.fluctuation_coefficient * cfg.minimum_delta_core_freq_step
            } else {
                0
            };
            println!("fluctuation state changing to HIGH...");
            (freq, false)
        };

        if cfg.is_legacy_global_offset {
            gpu.inner()
                .set_pstates(
                    [(
                        PState::P0,
                        ClockDomain::Graphics,
                        KilohertzDelta(cfg.init_core_oc_value + fluctuation_freq),
                    )]
                    .iter()
                    .cloned(),
                )
                .unwrap_or_else(|e| {
                    eprintln!("Warning:{}, fluctuation Error. GPU crashed...", e);
                });
        } else {
            let main_delta = cfg.init_core_oc_value + fluctuation_freq;
            let lower_delta =
                Some(cfg.init_core_oc_value - cfg.minimum_delta_core_freq_step + fluctuation_freq);
            set_vfp_curve_warn(
                gpu,
                cfg.point,
                cfg.vfp_set_range,
                cfg.flat_curve_flag,
                main_delta,
                lower_delta,
            );
        }

        new_h_l_flag
    }

    fn force_kill_process(process: &mut Child, reason: &str) {
        match process.kill() {
            Ok(_) => {
                let _ = process.wait();
                eprintln!("Force-killed test process due to {}.", reason);
            }
            Err(e) => match process.try_wait() {
                Ok(Some(status)) => {
                    eprintln!(
                        "Test process already exited with code {:?} while handling {}.",
                        status.code(),
                        reason
                    );
                }
                Ok(None) => {
                    eprintln!("Failed to force-kill test process due to {}: {}", reason, e);
                }
                Err(wait_err) => {
                    eprintln!(
                        "Failed to force-kill test process due to {}: {} (try_wait error: {})",
                        reason, e, wait_err
                    );
                }
            },
        }
    }

    pub(super) fn run(
        gpu: &&Gpu,
        matches: &ArgMatches,
        cfg: &TestPressureConfig<'_>,
    ) -> (i32, TestResolution) {
        let app_path = String::from(cfg.test_exe);
        let mut arg = cfg.test_code.clone() + " " + &cfg.timeout_loops.to_string();
        let timeout_budget_secs = cfg.timeout_loops * 6;
        println!("Timeout: {}s", timeout_budget_secs);
        if cfg.recovery_method {
            arg += " --aggressive-recovery";
        }

        let mut count = 0;
        let mut resolution = cfg.test_resolution;

        loop {
            match Command::new(app_path.clone())
                .args(arg.split_whitespace())
                .spawn()
            {
                Ok(mut process) => {
                    let mut exit_code = 1;
                    let test_start_at = Instant::now();
                    let mut last_fluctuation = Instant::now();
                    let mut in_test_check_number = 0;
                    let mut fluctuation_h_l_flag = false;
                    let mut thrm_or_pwr_limit_number = 0;
                    let mut thrm_or_pwr_limit_flag = false;
                    println!("Current resolution: {:?}", resolution);

                    scan_shared::sync_resolution(cfg.test_conf_file_path, resolution);

                    core_reset_vfp(gpu).unwrap_or_else(|err| {
                        eprintln!("Warning: Failed to reset GPU due to {:?}", err);
                    });
                    sleep(Duration::from_secs(1));
                    test_initialization(gpu, cfg);
                    sleep(Duration::from_secs(1));

                    loop {
                        if last_fluctuation.elapsed()
                            >= Duration::from_millis(1500)
                        {
                            in_test_check_number += 1;
                            println!("inducing freq fluctuation...");

                            fluctuation_h_l_flag =
                                apply_fluctuation(gpu, cfg, fluctuation_h_l_flag);

                            println!("Processing in-test v-f check #{}...", in_test_check_number);

                            if !cfg.is_legacy_global_offset {
                                match voltage_frequency_check(matches.clone(), cfg.point) {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        println!("Considering trig'd thrm/pwr capping!!!");
                                        thrm_or_pwr_limit_number += 1;
                                    }
                                    Err(e) => {
                                        eprintln!("Warning: Failed to read v-f info: {}", e);
                                        force_kill_process(&mut process, "v-f check read failure");
                                        break;
                                    }
                                }

                                match get_voltage_by_point(gpu, cfg.point) {
                                    Ok(v) => {
                                        println!("Voltage at point {}: {}", cfg.point, v);
                                        gpu.set_vfp_lock_voltage(Some(v)).unwrap_or_else(|err| {
                                            eprintln!(
                                                "Warning: Failed to set voltage due to {:?}",
                                                err
                                            );
                                        });
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "Warning: Failed to get voltage at point {}: {}",
                                            cfg.point, e
                                        );
                                        force_kill_process(&mut process, "voltage fetch failure");
                                        break;
                                    }
                                }
                            }

                            last_fluctuation = Instant::now();
                        }

                        sleep(time::Duration::from_secs(1));

                        match process.try_wait() {
                            Ok(Some(status)) => {
                                exit_code = status.code().unwrap_or(1);
                                println!("Process finished with exit code {}.", exit_code);
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                eprintln!("Failed to check process status: {}", e);
                                force_kill_process(&mut process, "process status check error");
                                break;
                            }
                        }

                        if test_start_at.elapsed() >= Duration::from_secs(timeout_budget_secs) {
                            println!(
                                "Considering GPU has crashed (timeout: {}s, elapsed: {}s)...",
                                timeout_budget_secs,
                                test_start_at.elapsed().as_secs()
                            );
                            force_kill_process(&mut process, "in-test timeout");
                            core_reset_vfp(gpu).unwrap_or_else(|err| {
                                eprintln!("Warning: Failed to reset GPU due to {:?}", err);
                            });
                            break;
                        }
                    }

                    if exit_code == 0 {
                        eprintln!("Process finished successfully.");
                        if in_test_check_number > 0
                            && ((thrm_or_pwr_limit_number as f64 / in_test_check_number as f64)
                                > 0.3)
                            && resolution.downgrade().is_some()
                            && !cfg.is_mem_test
                        {
                            thrm_or_pwr_limit_flag = true;
                        }
                        if thrm_or_pwr_limit_flag && let Some(lower_res) = resolution.downgrade() {
                            println!("Downgrading to {:?}", lower_res);
                            resolution = lower_res;
                            continue;
                        }
                    } else {
                        eprintln!("Process finished with exit code {}.", exit_code);
                    }

                    return (exit_code, resolution);
                }
                Err(e) => {
                    count += 1;
                    eprintln!("Failed to start process: {}, try again.", e);
                    sleep(Duration::from_secs(1));
                    if count >= cfg.timeout_loops {
                        eprintln!("Timeout reached, giving up on starting the process.");
                        return (1, resolution);
                    }
                }
            }
        }
    }
}

fn test_pressure(
    gpu: &&Gpu,
    matches: &ArgMatches,
    point: usize,
    flat_curve_flag: bool,
    vfp_set_range: usize,
    init_core_oc_value: i32,
    minimum_delta_core_freq_step: i32,
    fluctuation_coefficient: i32,
    fluctuation_mode: usize,
    test_resolution: TestResolution,
    is_mem_test: bool,
    test_exe: &str,
    test_code: String,
    timeout_loops: u64,
    test_conf_file_path: Option<&str>,
    recovery_method: bool,
    is_legacy_global_offset: bool,
) -> (i32, TestResolution) {
    let cfg = pressure_runner::TestPressureConfig {
        point,
        flat_curve_flag,
        vfp_set_range,
        init_core_oc_value,
        minimum_delta_core_freq_step,
        fluctuation_coefficient,
        fluctuation_mode,
        test_resolution,
        is_mem_test,
        test_exe,
        test_code,
        timeout_loops,
        test_conf_file_path,
        recovery_method,
        is_legacy_global_offset,
    };

    pressure_runner::run(gpu, matches, &cfg)
}

struct CommonPhaseArgs<'a> {
    matches: &'a ArgMatches,
    minimum_delta_core_freq_step: i32,
    fluctuation_coefficient: i32,
    fluctuation_mode: usize,
    test_exe: &'a str,
    delimiter: &'a str,
    test_conf_file_path: Option<&'a str>,
    recovery_method_switch: bool,
    test_duration: u64,
    endurance_coefficient: u64,
}

fn build_common_phase_args<'a>(
    matches: &'a ArgMatches,
    minimum_delta_core_freq_step: i32,
    fluctuation_coefficient: i32,
    fluctuation_mode: usize,
    test_exe: &'a str,
    delimiter: &'a str,
    test_conf_file_path: Option<&'a str>,
    recovery_method_switch: bool,
    test_duration: u64,
    endurance_coefficient: u64,
) -> CommonPhaseArgs<'a> {
    CommonPhaseArgs {
        matches,
        minimum_delta_core_freq_step,
        fluctuation_coefficient,
        fluctuation_mode,
        test_exe,
        delimiter,
        test_conf_file_path,
        recovery_method_switch,
        test_duration,
        endurance_coefficient,
    }
}

struct LegacyPhaseArgs<'a> {
    common: CommonPhaseArgs<'a>,
    point: usize,
    flat_curve_flag: bool,
    vfp_set_range: usize,
    freq_step_exp: usize,
}

fn apply_short_phase_failure_step(
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
    minimum_delta_core_freq_step: i32,
    freq_step_exp: usize,
    test_num: &mut usize,
    is_50_series: bool,
) -> i32 {
    if *test_num > 3 {
        *test_num = 3;
    }
    *core_oc_safe_limit = *init_core_oc_value;
    let decrease = minimum_delta_core_freq_step * pow(2, freq_step_exp - *test_num);
    *init_core_oc_value -= decrease;
    if is_50_series {
        *init_core_oc_value -= minimum_delta_core_freq_step;
    }
    decrease
}

fn apply_short_phase_success_step(
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: i32,
    minimum_delta_core_freq_step: i32,
    freq_step_exp: usize,
    test_num: &mut usize,
    is_50_series: bool,
) -> Option<i32> {
    if *init_core_oc_value + minimum_delta_core_freq_step >= core_oc_safe_limit {
        return None;
    }
    if is_50_series && *init_core_oc_value + 2 * minimum_delta_core_freq_step >= core_oc_safe_limit
    {
        return None;
    }

    let increase = if *init_core_oc_value
        + minimum_delta_core_freq_step * pow(2, freq_step_exp + 1 - *test_num)
        >= core_oc_safe_limit
    {
        if *init_core_oc_value + minimum_delta_core_freq_step * pow(2, freq_step_exp - *test_num)
            == core_oc_safe_limit
        {
            *test_num += 1;
        }
        minimum_delta_core_freq_step * pow(2, freq_step_exp - *test_num)
    } else {
        minimum_delta_core_freq_step * pow(2, freq_step_exp + 1 - *test_num)
    };

    *init_core_oc_value += increase;
    *test_num = test_num.saturating_sub(1);
    Some(increase)
}

fn pre_load_vf_recheck(matches: &ArgMatches, point: usize) -> Result<(), crate::error::Error> {
    println!("Waiting for pre-load volt-freq recheck");
    sleep(Duration::from_secs(1));
    voltage_frequency_check(matches.clone(), point)?;
    Ok(())
}

fn apply_long_phase_failure_step(
    init_core_oc_value: &mut i32,
    minimum_delta_core_freq_step: i32,
    is_50_series: bool,
) {
    *init_core_oc_value -= minimum_delta_core_freq_step;
    if is_50_series {
        *init_core_oc_value -= minimum_delta_core_freq_step;
    }
}

fn log_point_test_header<V: std::fmt::Display, D: std::fmt::Display>(
    l: &mut fs::File,
    test_code: usize,
    point: usize,
    voltage: V,
    delta_label: &str,
    delta_value: D,
) -> Result<(), Error> {
    write!(
        l,
        "Test #{} on point: #{}, voltage: #{}, {}: #{}. ",
        test_code, point, voltage, delta_label, delta_value
    )?;
    println!(
        "Test #{} on point: #{}, voltage: #{}, {}: #+{}. ",
        test_code, point, voltage, delta_label, delta_value
    );
    Ok(())
}

fn run_legacy_short_phase(
    l: &mut fs::File,
    gpu: &&Gpu,
    args: &LegacyPhaseArgs<'_>,
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
    test_resolution: &mut TestResolution,
    test_code: &mut usize,
    resuming_flag: &mut bool,
) -> Result<(), Error> {
    println!("Starting short test phase...");
    writeln!(l, "Starting short test phase...")?;

    let mut test_num: usize = 0;

    if *resuming_flag {
        *resuming_flag = false;
        println!(
            "Initial OC offset: {}kHz, current safe limit: {}kHz",
            *init_core_oc_value, *core_oc_safe_limit
        );
        while *init_core_oc_value
            + args.common.minimum_delta_core_freq_step * pow(2, args.freq_step_exp + 1 - test_num)
            > *core_oc_safe_limit
        {
            test_num += 1;
        }
        if args.freq_step_exp + 1 < test_num {
            println!("Skipping short test phase entirely (already converged).");
        }
    }

    loop {
        gpu.inner().set_pstates(
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(*init_core_oc_value),
            )]
            .iter()
            .cloned(),
        )?;

        test_num += 1;
        *test_code += 1;

        println!(
            "current test progress estimated: {:.2}%",
            (test_num + *test_code - 1) as f64 / (args.freq_step_exp + *test_code - 1) as f64
                * 100.0
        );
        println!("current test num: {}", test_num);

        write!(
            l,
            "Short Test #{} freq_delta: +{}kHz. ",
            *test_code, *init_core_oc_value
        )?;
        println!(
            "Short Test #{} freq_delta: +{}kHz. ",
            *test_code, *init_core_oc_value
        );

        let (test_flag, new_resolution) = test_pressure(
            gpu,
            args.common.matches,
            args.point,
            args.flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            *test_resolution,
            false,
            args.common.test_exe,
            format!("legacy{}{}", args.common.delimiter, *test_code),
            args.common.test_duration,
            args.common.test_conf_file_path,
            args.common.recovery_method_switch,
            true,
        );
        *test_resolution = new_resolution;

        writeln!(l, "Test result is code #{} .", test_flag)?;

        if test_flag != 0 {
            println!(
                "Short Test #{} FAILED at +{}kHz",
                *test_code, *init_core_oc_value
            );
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;
            let decrease = apply_short_phase_failure_step(
                init_core_oc_value,
                core_oc_safe_limit,
                args.common.minimum_delta_core_freq_step,
                args.freq_step_exp,
                &mut test_num,
                false,
            );
            println!("Decreasing target freq by {}kHz", decrease);
            continue;
        }

        println!(
            "Short Test #{} SUCCEEDED at +{}kHz",
            *test_code, *init_core_oc_value
        );
        if let Some(increase) = apply_short_phase_success_step(
            init_core_oc_value,
            *core_oc_safe_limit,
            args.common.minimum_delta_core_freq_step,
            args.freq_step_exp,
            &mut test_num,
            false,
        ) {
            println!("Increasing target freq by {}kHz", increase);
        } else {
            break;
        }

        if test_num >= args.freq_step_exp {
            break;
        }
    }

    println!(
        "Short test phase finished. Current freq_delta: +{}kHz",
        *init_core_oc_value
    );
    Ok(())
}

fn run_legacy_long_phase(
    l: &mut fs::File,
    gpu: &&Gpu,
    args: &LegacyPhaseArgs<'_>,
    init_core_oc_value: &mut i32,
    test_resolution: &mut TestResolution,
    test_code: &mut usize,
) -> Result<(), Error> {
    println!("Initiating Long Test...");
    writeln!(l, "Initiating Long Test...")?;

    loop {
        gpu.inner().set_pstates(
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(*init_core_oc_value),
            )]
            .iter()
            .cloned(),
        )?;

        *test_code += 1;
        write!(
            l,
            "Long Test #{} freq_delta: +{}kHz. ",
            *test_code, *init_core_oc_value
        )?;
        println!(
            "Long Test #{} freq_delta: +{}kHz. ",
            *test_code, *init_core_oc_value
        );

        let (long_flag, new_resolution) = test_pressure(
            gpu,
            args.common.matches,
            args.point,
            args.flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            *test_resolution,
            false,
            args.common.test_exe,
            format!("legacy{}{}", args.common.delimiter, *test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.test_conf_file_path,
            args.common.recovery_method_switch,
            true,
        );
        *test_resolution = new_resolution;

        writeln!(l, "Test result is code #{} .", long_flag)?;

        if long_flag != 0 {
            println!(
                "Long Test #{} FAILED at +{}kHz",
                *test_code, *init_core_oc_value
            );
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;
            apply_long_phase_failure_step(
                init_core_oc_value,
                args.common.minimum_delta_core_freq_step,
                false,
            );
            println!(
                "Decreasing target freq by {}kHz",
                args.common.minimum_delta_core_freq_step
            );
            continue;
        }

        println!(
            "Long Test #{} SUCCEEDED at +{}kHz",
            *test_code, *init_core_oc_value
        );
        break;
    }

    Ok(())
}

struct GpuBoostPhaseArgs<'a> {
    common: CommonPhaseArgs<'a>,
    vfp_set_range: usize,
    freq_step_exp: usize,
    is_50_series: bool,
}

enum ArchSafetyPolicyPhase {
    PrePointTest,
    PostPointTest,
}

fn apply_arch_safety_policy(
    phase: ArchSafetyPolicyPhase,
    is_50_series: bool,
    voltage_uv: u32,
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
    core_oc_safe_limit_ref: &mut i32,
    safe_elasticity_per_cycle: i32,
) {
    match phase {
        ArchSafetyPolicyPhase::PrePointTest => {
            if is_50_series && voltage_uv > 845000_u32 {
                println!("Entering High-risk-crashing region!");
                *core_oc_safe_limit_ref = 517500;
            }
        }
        ArchSafetyPolicyPhase::PostPointTest => {
            if is_50_series
                && 650000_u32 < voltage_uv
                && voltage_uv < 675000_u32
                && *init_core_oc_value > 540000
            {
                println!("leaving low voltage max-Q region...");
                *init_core_oc_value -= 3 * safe_elasticity_per_cycle;
                *core_oc_safe_limit = min(
                    *core_oc_safe_limit + safe_elasticity_per_cycle,
                    *core_oc_safe_limit_ref,
                );
            } else if is_50_series && voltage_uv > 845000_u32 {
                println!("Entering High-risk-crashing region!");
                *core_oc_safe_limit_ref = 525000;
                *init_core_oc_value -= safe_elasticity_per_cycle;
                *core_oc_safe_limit = min(
                    *core_oc_safe_limit + safe_elasticity_per_cycle,
                    *core_oc_safe_limit_ref,
                );
            } else {
                *init_core_oc_value -= safe_elasticity_per_cycle;
                *core_oc_safe_limit = min(
                    *core_oc_safe_limit + safe_elasticity_per_cycle,
                    *core_oc_safe_limit_ref,
                );
            }
        }
    }
}

fn run_gpuboostv3_short_phase<V: std::fmt::Display + Copy>(
    l: &mut fs::File,
    gpu: &Gpu,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: V,
    flat_curve_flag: bool,
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
    resuming_flag: &mut bool,
    test_resolution: &mut TestResolution,
) -> Result<usize, Error> {
    let mut test_num = 0;
    let mut test_code = 0;

    loop {
        set_vfp_curve(
            &gpu,
            point,
            args.vfp_set_range,
            flat_curve_flag,
            *init_core_oc_value,
            Some(*init_core_oc_value - args.common.minimum_delta_core_freq_step),
        )?;

        pre_load_vf_recheck(args.common.matches, point)?;

        test_num += 1;
        test_code += 1;

        if *resuming_flag {
            *resuming_flag = false;
            println!(
                "Initial OC offset:{}kHz, current safe limit:{}kHz",
                *init_core_oc_value, *core_oc_safe_limit
            );
            while *init_core_oc_value
                + args.common.minimum_delta_core_freq_step
                    * pow(2, args.freq_step_exp + 1 - test_num)
                > *core_oc_safe_limit
            {
                test_num += 1;
            }
            if args.freq_step_exp + 1 < test_num {
                println!("Skipping short test...");
                println!("Initiating Long Test...");
                writeln!(l, "Initiating Long Test...")?;
                break;
            }
        }

        println!(
            "current test progress estimated:{:.2}%",
            (test_num + test_code - 1) as f64 / (args.freq_step_exp + test_code - 1) as f64 * 100.
        );
        println!("current test num: {}", test_num);

        log_point_test_header(
            l,
            test_code,
            point,
            v,
            "freq_delta",
            KilohertzDelta(*init_core_oc_value),
        )?;

        let test_flag;
        (test_flag, *test_resolution) = test_pressure(
            &gpu,
            args.common.matches,
            point,
            flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            *test_resolution,
            false,
            args.common.test_exe,
            format!("{}{}{}", point, args.common.delimiter, test_code),
            args.common.test_duration,
            args.common.test_conf_file_path,
            args.common.recovery_method_switch,
            false,
        );
        println!("{}", test_flag);
        writeln!(l, "Test result is code #{} .", test_flag)?;

        if test_flag != 0 {
            core_reset_vfp(gpu)?;
            println!(
                "Test #{} FAILED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
                test_code,
                point,
                v,
                KilohertzDelta(*init_core_oc_value)
            );
            let decrease = apply_short_phase_failure_step(
                init_core_oc_value,
                core_oc_safe_limit,
                args.common.minimum_delta_core_freq_step,
                args.freq_step_exp,
                &mut test_num,
                args.is_50_series,
            );
            println!("Decreasing target freq by {}kHz", decrease);
            if args.is_50_series {
                println!(
                    "Additional safety: Decreasing target freq by {}kHz",
                    args.common.minimum_delta_core_freq_step
                )
            }
            continue;
        }

        println!(
            "Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            test_code,
            point,
            v,
            KilohertzDelta(*init_core_oc_value)
        );
        if let Some(increase) = apply_short_phase_success_step(
            init_core_oc_value,
            *core_oc_safe_limit,
            args.common.minimum_delta_core_freq_step,
            args.freq_step_exp,
            &mut test_num,
            args.is_50_series,
        ) {
            println!("Increasing target freq by {}kHz", increase);
        } else {
            break;
        }

        if test_num >= args.freq_step_exp {
            break;
        }
    }
    println!(
        "Short test phase finished. Current freq_delta: +{}kHz",
        *init_core_oc_value
    );
    Ok(test_code)
}

fn run_gpuboostv3_long_phase<V: std::fmt::Display + Copy>(
    l: &mut fs::File,
    gpu: &Gpu,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: V,
    flat_curve_flag: bool,
    init_core_oc_value: &mut i32,
    test_resolution: &mut TestResolution,
    test_code: &mut usize,
) -> Result<(), Error> {
    let mut long_duration_flag;
    println!("Initiating Long Test...");
    writeln!(l, "Initiating Long Test...")?;

    loop {
        pre_load_vf_recheck(args.common.matches, point)?;

        *test_code += 1;
        log_point_test_header(
            l,
            *test_code,
            point,
            v,
            "freq_delta",
            KilohertzDelta(*init_core_oc_value),
        )?;

        (long_duration_flag, *test_resolution) = test_pressure(
            &gpu,
            args.common.matches,
            point,
            flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            *test_resolution,
            false,
            args.common.test_exe,
            format!("{}{}{}", point, args.common.delimiter, *test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.test_conf_file_path,
            args.common.recovery_method_switch,
            false,
        );
        if long_duration_flag != 0 {
            core_reset_vfp(gpu)?;
            println!(
                "Long Test #{} FAILED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
                *test_code,
                point,
                v,
                KilohertzDelta(*init_core_oc_value)
            );
            writeln!(l, "Test result is code #{} .", long_duration_flag)?;
            apply_long_phase_failure_step(
                init_core_oc_value,
                args.common.minimum_delta_core_freq_step,
                args.is_50_series,
            );
            println!(
                "Decreasing target freq by {}kHz",
                args.common.minimum_delta_core_freq_step
            );
            if args.is_50_series {
                println!(
                    "Additional safety: Decreasing target freq by {}kHz",
                    args.common.minimum_delta_core_freq_step
                )
            }
            continue;
        }

        println!(
            "Long Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            *test_code,
            point,
            v,
            KilohertzDelta(*init_core_oc_value)
        );
        writeln!(l, "Test result is code #{} .", long_duration_flag)?;
        break;
    }

    Ok(())
}

struct MemOcPhaseArgs<'a> {
    common: CommonPhaseArgs<'a>,
    point: usize,
    vfp_set_range: usize,
    minimum_delta_mem_freq_step: i32,
    mem_freq_step_exp: usize,
}

fn run_mem_oc_phase<V: std::fmt::Display + Copy>(
    l: &mut fs::File,
    gpu: &Gpu,
    gpus: &Vec<&Gpu>,
    args: &MemOcPhaseArgs<'_>,
    mem_voltage: V,
    init_vmem_oc_value: &mut i32,
    mem_oc_safe_limit: &mut i32,
    test_resolution: &mut TestResolution,
) -> Result<(), Error> {
    let mut mem_test_code: usize = 0;
    let mut mem_test_num: usize = 0;

    loop {
        match handle_lock_vfp(gpus, args.common.matches, args.point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        gpu.inner().set_pstates(
            [(
                PState::P0,
                ClockDomain::Memory,
                KilohertzDelta(*init_vmem_oc_value),
            )]
            .iter()
            .cloned(),
        )?;

        mem_test_num += 1;
        mem_test_code += 1;

        pre_load_vf_recheck(args.common.matches, args.point)?;

        println!(
            "current test progress estimated:{:.2}%",
            (mem_test_num + mem_test_code - 1) as f64
                / (args.mem_freq_step_exp + mem_test_code - 1).max(1) as f64
                * 100.0
        );
        println!("current test num: {}", mem_test_num);

        log_point_test_header(
            l,
            mem_test_code,
            args.point,
            mem_voltage,
            "mem_freq_delta",
            KilohertzDelta(*init_vmem_oc_value),
        )?;

        let mem_test_flag;
        (mem_test_flag, *test_resolution) = test_pressure(
            &gpu,
            args.common.matches,
            args.point,
            true,
            args.vfp_set_range,
            0,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            *test_resolution,
            true,
            args.common.test_exe,
            format!("{}{}{}", args.point, args.common.delimiter, mem_test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.test_conf_file_path,
            args.common.recovery_method_switch,
            true,
        );

        writeln!(l, "Test result is code #{} .", mem_test_flag)?;

        if mem_test_flag != 0 {
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;
            println!(
                "Long Test #{} FAILED on point: #{}, voltage: #{}, mem_freq_delta: #+{}. ",
                mem_test_code,
                args.point,
                mem_voltage,
                KilohertzDelta(*init_vmem_oc_value)
            );

            let decrease = apply_short_phase_failure_step(
                init_vmem_oc_value,
                mem_oc_safe_limit,
                args.minimum_delta_mem_freq_step,
                args.mem_freq_step_exp,
                &mut mem_test_num,
                false,
            );
            println!("Decreasing target mem_freq by {}kHz", decrease);
            continue;
        }

        println!(
            "Test #{} SUCCEEDED on point: #{}, voltage: #{}, freq_delta: #+{}. ",
            mem_test_code,
            args.point,
            mem_voltage,
            KilohertzDelta(*init_vmem_oc_value)
        );
        if let Some(increase) = apply_short_phase_success_step(
            init_vmem_oc_value,
            *mem_oc_safe_limit,
            args.minimum_delta_mem_freq_step,
            args.mem_freq_step_exp,
            &mut mem_test_num,
            false,
        ) {
            println!("Increasing target freq by {}kHz", increase);
        } else {
            break;
        }

        if mem_test_num >= args.mem_freq_step_exp {
            break;
        }
    }

    Ok(())
}

pub fn autoscan_gpuboostv3(gpus: &Vec<&Gpu>, matches: &ArgMatches) -> Result<(), Error> {
    use crate::autoscan_config::AutoscanConfig;
    let cfg = AutoscanConfig::from_autoscan_matches(matches)?;
    let nvml = Nvml::init()
        .map_err(|e| Error::Custom(format!("NVML init failed in autoscan: {:?}", e)))?;

    let mut is_ultrafast = cfg.is_ultrafast;
    if is_ultrafast {
        println!("Ultrafast mode interpolation active...");
    }

    let test_exe = cfg.test_exe.as_str();
    let log_filename = cfg.log.as_str();
    let test_conf_file_path = stressor_3d_conf_path();
    // Ensure the directory exists
    if let Some(parent) = Path::new(log_filename).parent() {
        fs::create_dir_all(parent)?;
    }
    let mut l = fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(log_filename)?;
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
            let (lvp, uvp) = handle_test_voltage_limits(gpus, matches)?;

            for gpu in gpus {
                let minimum_voltage = get_voltage_by_point(gpu, lvp)?;
                let maximum_voltage = get_voltage_by_point(gpu, uvp)?;
                writeln!(l)?;
                writeln!(l, "minimum_voltage_point: {} @ {}", lvp, minimum_voltage)?;
                writeln!(l, "maximum_voltage_point: {} @ {}", uvp, maximum_voltage)?;
            }

            (lvp, uvp)
        }
    };

    let mut init_vmem_oc_value = 0;

    for gpu in gpus {
        gpu.inner().set_pstates(
            [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                .iter()
                .cloned(),
        )?;

        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info);

        gpu.set_vfp(
            iter::once((upper_voltage_point, KilohertzDelta(45000))),
            iter::empty(),
        )?;
        match handle_lock_vfp(gpus, matches, upper_voltage_point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        let readout_f = gpu.status()?.clone().clocks;

        let mut clocks = Vec::new();
        for (clock_name, freq) in readout_f {
            // Store the clock name and frequency in a data structure.
            clocks.push((clock_name.to_string(), freq));
        }
        // Now you can search for the Memory Clock:
        print_scan_separator();
        if let Some((_, memory_clock)) = clocks.iter().find(|(name, _)| name.contains("Memory")) {
            println!("Memory Clock: {}", memory_clock);
            init_vmem_oc_value = (memory_clock.0 / 25) as i32;
            println!("Memory OC start at: +{} MHz", init_vmem_oc_value / 1000);
        }
        print_scan_separator();

        // 从 GpuType 读取该世代的固定 OC 扫描参数
        let GpuOcParams {
            minimum_delta_core_freq_step,
            mut core_oc_safe_limit,
            mut init_core_oc_value,
            safe_elasticity_per_cycle,
            fluctuation_coefficient,
            is_50_series,
            testing_step,
        } = gpu_type.as_ref().map(|t| t.oc_params()).unwrap_or_default();

        let mut core_oc_safe_limit_ref = core_oc_safe_limit;
        let _init_core_oc_value_ref = init_core_oc_value;
        let points = gpu.status()?.vfp.ok_or(Error::VfpUnsupported)?.graphics;

        let mut point = lower_voltage_point;
        let mut resuming_flag = false;
        let mut last_succeeded_freq = init_core_oc_value;
        let mut last_failed_freq = core_oc_safe_limit_ref;
        let recovery_method_switch: bool = cfg.recovery_method.unwrap_or(is_50_series);

        match break_point_continue(log_filename, testing_step)? {
            (succeeded_freq, failed_freq, last_voltage_point, ultrafast_flag) => {
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
                    println!("  - Last voltage point: {}", voltage_point);
                    point = voltage_point; // Update if present
                    resuming_flag = true;
                }

                if let Some(ultrafast_flag) = ultrafast_flag {
                    println!("Inheriting last scanner flag...");
                    is_ultrafast = ultrafast_flag; // Update if present
                    resuming_flag = true;
                }
            }
        }

        let init_resolution =
            scan_shared::pick_initial_resolution(resuming_flag, test_conf_file_path);
        scan_shared::sync_resolution(test_conf_file_path, init_resolution);

        if is_ultrafast {
            if !ultrafast_point_extraction_flag {
                (p1, p2, p3, p4) = key_point_extractor(
                    gpus,
                    lower_voltage_point,
                    upper_voltage_point,
                    "./ws/vfp-init.csv",
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
            writeln!(l, "\n\nkey points detected:{},{},{},{}", p1, p2, p3, p4)
                .expect("extraction failed");

            println!("Scan in ultrafast mode...");
            writeln!(l, "\nScan in ultrafast mode")?;
        } else {
            println!("Scan in normal mode...");
            writeln!(l, "\nScan in normal mode")?;
        }

        init_core_oc_value = last_succeeded_freq;
        core_oc_safe_limit = last_failed_freq;
        if core_oc_safe_limit < init_core_oc_value {
            println!("log parsing error... Restoring default value");
            core_oc_safe_limit = core_oc_safe_limit_ref;
            init_core_oc_value -= safe_elasticity_per_cycle;
        };

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

        writeln!(l)?;
        println!("Waiting for default volt-freq self-check");
        voltage_frequency_check(matches.clone(), point).expect("Failed to read v-f info");
        let mut v;
        let mut default_frequency;
        let mut prev_endpoint_delta: Option<i32> = None;

        //prepare GPU OC parameter for extreme OC...
        if let Err(e) = apply_autoscan_profile(gpu, matches, 80) {
            eprintln!("apply_autoscan_profile failed: {:?}, continuing scan...", e);
        }

        let freq_step_exp = 3;
        let endurance_coefficient = 2;
        let vfp_set_range = 6;
        let mut test_duration: u64 = 10;
        if is_ultrafast {
            test_duration += test_duration / 2;
        };
        let fluctuation_mode = 3; // 1 = 0-, 2 = ±, 3 = 0+
        let mut flat_curve_flag: bool;
        let mut test_resolution = init_resolution;
        let phase_args = GpuBoostPhaseArgs {
            common: build_common_phase_args(
                matches,
                minimum_delta_core_freq_step,
                fluctuation_coefficient,
                fluctuation_mode,
                test_exe,
                delimiter.as_str(),
                test_conf_file_path,
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
            ),
            vfp_set_range,
            freq_step_exp,
            is_50_series,
        };

        // core oc scanning
        writeln!(l, "New Test Initiated at {}", local_time_hms())?;
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

            v = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .voltage;
            default_frequency = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .default_frequency;

            match handle_lock_vfp(gpus, matches, point, true) {
                Ok(_) => {
                    flat_curve_flag = false;
                }
                Err(_e) => {
                    flat_curve_flag = true;
                }
            }

            gpu.inner().set_pstates(
                [(
                    PState::P0,
                    ClockDomain::Memory,
                    KilohertzDelta(init_vmem_oc_value),
                )]
                .iter()
                .cloned(),
            )?;

            apply_arch_safety_policy(
                ArchSafetyPolicyPhase::PrePointTest,
                is_50_series,
                v.0,
                &mut init_core_oc_value,
                &mut core_oc_safe_limit,
                &mut core_oc_safe_limit_ref,
                safe_elasticity_per_cycle,
            );

            let mut test_code = run_gpuboostv3_short_phase(
                &mut l,
                gpu,
                &phase_args,
                point,
                v,
                flat_curve_flag,
                &mut init_core_oc_value,
                &mut core_oc_safe_limit,
                &mut resuming_flag,
                &mut test_resolution,
            )?;
            println!(
                "Short Test #{} finished on point: #{} , voltage: #{}, delta: #+{}. ",
                test_code,
                point,
                v,
                KilohertzDelta(init_core_oc_value)
            );
            run_gpuboostv3_long_phase(
                &mut l,
                gpu,
                &phase_args,
                point,
                v,
                flat_curve_flag,
                &mut init_core_oc_value,
                &mut test_resolution,
                &mut test_code,
            )?;
            write!(
                l,
                "\nFinished core OC on point: #{} with resolution {:?}\n",
                point, test_resolution
            )?;
            println!(
                "Core OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                v,
                KilohertzDelta(init_core_oc_value)
            );

            let p_save = nvapi_hi::VfPoint {
                voltage: v,
                frequency: default_frequency + KilohertzDelta(init_core_oc_value),
                delta: KilohertzDelta(init_core_oc_value),
                default_frequency,
            };
            let _ = export_single_point(p_save, matches);
            // interpolate when not in ultrafast mode.
            if !is_ultrafast {
                let prev_delta = prev_endpoint_delta.unwrap_or(init_core_oc_value);
                let current_delta = init_core_oc_value;
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
                    let p_save_prev = nvapi_hi::VfPoint {
                        voltage: v_prev,
                        frequency: default_frequency + KilohertzDelta(interpolated_delta),
                        delta: KilohertzDelta(interpolated_delta),
                        default_frequency,
                    };
                    let _ = export_single_point(p_save_prev, matches);
                }
            }
            prev_endpoint_delta = Some(init_core_oc_value);

            apply_arch_safety_policy(
                ArchSafetyPolicyPhase::PostPointTest,
                is_50_series,
                v.0,
                &mut init_core_oc_value,
                &mut core_oc_safe_limit,
                &mut core_oc_safe_limit_ref,
                safe_elasticity_per_cycle,
            );
            println!(
                "Reset init core oc value {}, OC safe limit to {}",
                init_core_oc_value, core_oc_safe_limit
            );
        }

        //memory oc
        let vmem_scan_switch = matches.get_flag("Vmem_scan_switch");
        if vmem_scan_switch {
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;

            let mut mem_oc_safe_limit = 0;
            let minimum_delta_mem_freq_step = 1000;
            let mem_freq_step_exp = 8;
            test_resolution = R1680x1050;

            if let Some((_, memory_clock)) = clocks.iter().find(|(name, _)| name.contains("Memory"))
            {
                println!("Memory Clock: {}", memory_clock);
                println!(
                    "Memory OC test start at: +{} MHz(+{}%)",
                    init_vmem_oc_value / 1000,
                    100 * init_vmem_oc_value / memory_clock.0 as i32
                );
                mem_oc_safe_limit = memory_clock.0 as i32 / 8;
            };

            match get_second_largest_resolution() {
                Some(resolution) => {
                    let (width, height) = resolution.dimensions();
                    println!("Second largest fitting resolution: {}x{}", width, height);
                    test_resolution = resolution;
                }
                None => println!("No fitting resolution found."),
            }
            point = upper_voltage_point;
            let mem_voltage = points
                .get(&(point))
                .ok_or(Error::Str("invalid point index"))?
                .voltage;

            let mem_phase_args = MemOcPhaseArgs {
                common: build_common_phase_args(
                    matches,
                    minimum_delta_core_freq_step,
                    fluctuation_coefficient,
                    fluctuation_mode,
                    test_exe,
                    delimiter.as_str(),
                    test_conf_file_path,
                    recovery_method_switch,
                    test_duration,
                    endurance_coefficient,
                ),
                point,
                vfp_set_range,
                minimum_delta_mem_freq_step,
                mem_freq_step_exp,
            };

            run_mem_oc_phase(
                &mut l,
                gpu,
                gpus,
                &mem_phase_args,
                mem_voltage,
                &mut init_vmem_oc_value,
                &mut mem_oc_safe_limit,
                &mut test_resolution,
            )?;
            write!(l, "\nFinished on point: #{}.\n", point)?;
            println!(
                "mem OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                mem_voltage,
                KilohertzDelta(init_vmem_oc_value)
            );
        }
        gpu.reset_cooler_levels().unwrap_or_else(|_e| {
            handle_reset_nvml_cooler_single_gpu(&nvml, gpu, "all")
                .unwrap_or_else(|e| eprintln!("Failed to reset cooler: {e}"))
        })
    }
    writeln!(l, "VFP Scan succeeded...")?;
    Ok(())
}

pub fn autoscan_legacy(gpus: &Vec<&Gpu>, matches: &ArgMatches) -> Result<(), Error> {
    use crate::autoscan_config::AutoscanConfig;
    let cfg = AutoscanConfig::from_legacy_matches(matches)?;
    let nvml = Nvml::init()
        .map_err(|e| Error::Custom(format!("NVML init failed in autoscan_legacy: {:?}", e)))?;

    let test_exe = cfg.test_exe.as_str();
    let log_filename = cfg.log.as_str();
    let test_conf_file_path = stressor_3d_conf_path();

    if let Some(parent) = Path::new(log_filename).parent() {
        fs::create_dir_all(parent)?;
    }
    let mut l = fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(log_filename)?;
    let delimiter: String = String::from("--");

    // Legacy GPU: single global offset, no V-F curve scanning
    // Use a fixed "point" value just as a placeholder for test_pressure interface
    let point: usize = 50;

    for gpu in gpus {
        // 从 GpuType 读取该世代的固定 OC 扫描参数
        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info);

        let GpuOcParams {
            minimum_delta_core_freq_step,
            mut core_oc_safe_limit,
            mut init_core_oc_value,
            safe_elasticity_per_cycle,
            fluctuation_coefficient,
            is_50_series: _, // legacy 路径不区分架构世代
            testing_step: _,
        } = gpu_type.as_ref().map(|t| t.oc_params()).unwrap_or_default();

        let core_oc_safe_limit_ref = core_oc_safe_limit;
        let _init_core_oc_value_ref = init_core_oc_value;

        // --- Breakpoint resume logic (mirrors v3) ---
        let mut resuming_flag = false;
        let mut last_succeeded_freq = init_core_oc_value;
        let mut last_failed_freq = core_oc_safe_limit_ref;

        match break_point_continue(log_filename, 1 /* single point, step=1 */)? {
            (succeeded_freq, failed_freq, last_voltage_point, _ultrafast_flag) => {
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
            }
        }

        // --- Resolution init ---
        let init_resolution =
            scan_shared::pick_initial_resolution(resuming_flag, test_conf_file_path);
        scan_shared::sync_resolution(test_conf_file_path, init_resolution);

        // Apply breakpoint-restored values
        init_core_oc_value = last_succeeded_freq;
        core_oc_safe_limit = last_failed_freq;
        if core_oc_safe_limit < init_core_oc_value {
            println!("log parsing error... Restoring default value");
            core_oc_safe_limit = core_oc_safe_limit_ref;
            init_core_oc_value -= safe_elasticity_per_cycle;
        }

        if let Err(e) = apply_autoscan_profile(gpu, matches, 80) {
            eprintln!("apply_autoscan_profile failed: {:?}, continuing scan...", e);
        }

        let recovery_method_switch: bool = cfg.recovery_method.unwrap_or(false);

        let freq_step_exp = 3;
        let endurance_coefficient = 2;
        let vfp_set_range = 0; // unused for legacy but required by test_pressure signature
        let test_duration: u64 = 10;
        let fluctuation_mode = 3;
        let flat_curve_flag = false; // not applicable for legacy

        let mut test_resolution = init_resolution;
        let mut test_code: usize = 0;

        writeln!(l, "Legacy Scan Initiated at {}", local_time_hms())?;
        print_scan_separator();
        println!("autoscan_legacy: single global core OC offset mode (Maxwell / pre-Pascal)");
        println!(
            "Initial OC offset: {}kHz, safe limit: {}kHz",
            init_core_oc_value, core_oc_safe_limit
        );
        print_scan_separator();

        let phase_args = LegacyPhaseArgs {
            common: build_common_phase_args(
                matches,
                minimum_delta_core_freq_step,
                fluctuation_coefficient,
                fluctuation_mode,
                test_exe,
                delimiter.as_str(),
                test_conf_file_path,
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
            ),
            point,
            flat_curve_flag,
            vfp_set_range,
            freq_step_exp,
        };

        for gpu in gpus {
            // Memory: keep at stock for legacy scan
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;

            run_legacy_short_phase(
                &mut l,
                gpu,
                &phase_args,
                &mut init_core_oc_value,
                &mut core_oc_safe_limit,
                &mut test_resolution,
                &mut test_code,
                &mut resuming_flag,
            )?;

            run_legacy_long_phase(
                &mut l,
                gpu,
                &phase_args,
                &mut init_core_oc_value,
                &mut test_resolution,
                &mut test_code,
            )?;

            write!(
                l,
                "\nLegacy OC scan finished. Final freq_delta: +{}kHz, resolution: {:?}\n",
                init_core_oc_value, test_resolution
            )?;
            println!(
                "Legacy OC scan finished. Final freq_delta: +{}kHz",
                init_core_oc_value
            );

            // Restore GPU to stock offset after scan
            gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            )?;
            gpu.reset_cooler_levels().unwrap_or_else(|_e| {
                handle_reset_nvml_cooler_single_gpu(&nvml, gpu, "all")
                    .unwrap_or_else(|e| eprintln!("Failed to reset cooler: {e}"))
            })
        }
    }

    writeln!(l, "Legacy VFP Scan succeeded...")?;
    Ok(())
}
