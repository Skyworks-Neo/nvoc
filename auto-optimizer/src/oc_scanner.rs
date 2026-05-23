use super::basic_func::local_time_hms;
use super::basic_func::{
    handle_lock_vfp, handle_reset_nvml_cooler_single_gpu, handle_test_voltage_limits,
    voltage_frequency_check,
};
use super::human::print_scan_separator;
use super::oc_profile_function::{
    apply_autoscan_profile, break_point_continue, check_voltage_points, export_single_point,
    key_point_extractor,
};
use clap::ArgMatches;
use colored::{Colorize, control};
use num_traits::pow;
use nvoc_core::{
    ClockDomain, Error, GpuOcParams, GpuOperation, GpuTarget, KilohertzDelta,
    NvapiLockedVoltageTarget, PState, QueryGpuInfo, QueryGpuStatus, QueryVfpPointVoltage,
    ResetCoolerLevels, ResetVfpDeltas, SetVfpPointDelta, SetVfpVoltageLock, VfPoint,
    VfpResetDomain, fetch_gpu_type, run, set_nvapi_pstate_clock_offsets, set_nvapi_vfp_curve_delta,
};
use std::cmp::min;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::time::SystemTime;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

mod scan_cli_color {
    use super::{AtomicBool, Colorize, Ordering, control};

    static COLOR_ENABLED: AtomicBool = AtomicBool::new(true);

    pub(super) fn init(no_color_flag: bool) {
        let disable = no_color_flag || std::env::var_os("NO_COLOR").is_some();
        COLOR_ENABLED.store(!disable, Ordering::Relaxed);
        if disable {
            control::set_override(false);
        }
    }

    fn enabled() -> bool {
        COLOR_ENABLED.load(Ordering::Relaxed)
    }

    fn is_numeric_like(token: &str) -> bool {
        let mut has_digit = false;
        for c in token.chars() {
            if c.is_ascii_digit() {
                has_digit = true;
                continue;
            }
            if matches!(c, '+' | '-' | '.' | '#' | ':' | '/' | ',') {
                continue;
            }
            return false;
        }
        has_digit
    }

    fn split_affixes(token: &str) -> (&str, &str, &str) {
        let start = token
            .char_indices()
            .find(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .map(|(i, _)| i)
            .unwrap_or(token.len());
        let end = token
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '%' | '+' | '°'))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if start >= end {
            return (token, "", "");
        }
        (&token[..start], &token[start..end], &token[end..])
    }

    fn style_plain(core: &str, is_stderr: bool) -> String {
        if is_stderr {
            core.bright_white().to_string()
        } else {
            core.normal().to_string()
        }
    }

    fn style_number(core: &str, is_stderr: bool) -> String {
        if is_stderr {
            core.bright_cyan().bold().to_string()
        } else {
            core.bright_blue().bold().to_string()
        }
    }

    fn style_keyword(core: &str, is_stderr: bool) -> String {
        let lower = core.to_ascii_lowercase();
        if lower.contains("failed") || lower.contains("error") || lower.contains("crash") {
            return core.red().bold().to_string();
        }
        if lower.contains("warning") {
            return core.yellow().bold().to_string();
        }
        if lower.contains("skipped") || lower.contains("skip") {
            return core.bright_yellow().bold().to_string();
        }
        if lower.contains("succeed") || lower == "passed" {
            return core.green().bold().to_string();
        }
        if lower.contains("scanner") || lower.contains("point") || lower.contains("gpu") {
            return core.bright_blue().bold().to_string();
        }
        if lower.contains("voltage") || lower.contains("uv") || lower.contains("mv") {
            return core.bright_magenta().bold().to_string();
        }
        if lower.contains("freq")
            || lower.contains("clock")
            || lower.contains("khz")
            || lower.contains("mhz")
            || lower.contains("ghz")
        {
            return core.bright_cyan().bold().to_string();
        }
        if lower.contains("percent") || lower.contains("progress") || lower.ends_with('%') {
            return core.bright_yellow().bold().to_string();
        }
        style_plain(core, is_stderr)
    }

    fn color_core_token(core: &str, is_stderr: bool) -> String {
        let lower = core.to_ascii_lowercase();
        if is_numeric_like(core) {
            return style_number(core, is_stderr);
        }

        if lower.ends_with("khz") || lower.ends_with("mhz") || lower.ends_with("ghz") {
            return core.bright_cyan().bold().to_string();
        }
        if lower.ends_with("uv") || lower.ends_with("mv") || lower.ends_with('v') {
            return core.bright_magenta().bold().to_string();
        }
        if lower.ends_with('%') {
            return core.bright_yellow().bold().to_string();
        }
        if lower.ends_with("ms") || lower.ends_with('s') {
            return core.bright_green().bold().to_string();
        }

        style_keyword(core, is_stderr)
    }

    pub(super) fn stylize(message: &str, is_stderr: bool) -> String {
        if !enabled() {
            return message.to_string();
        }

        message
            .split(' ')
            .map(|token| {
                if token.is_empty() {
                    return String::new();
                }
                let (prefix, core, suffix) = split_affixes(token);
                if core.is_empty() {
                    return token.to_string();
                }
                format!("{}{}{}", prefix, color_core_token(core, is_stderr), suffix)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub(super) fn init_scan_cli_color(no_color_flag: bool) {
    scan_cli_color::init(no_color_flag);
}

macro_rules! println {
    () => {
        std::println!()
    };
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::println!("{}", scan_cli_color::stylize(&msg, false));
    }};
}

macro_rules! eprintln {
    () => {
        std::eprintln!()
    };
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::eprintln!("{}", scan_cli_color::stylize(&msg, true));
    }};
}

fn run_output<O: GpuOperation>(gpu: &GpuTarget<'_>, op: O) -> Result<O::Output, Error> {
    run(gpu, op).map(|report| report.output)
}

mod pressure_runner {
    use super::*;

    fn set_vfp_range_warn(
        gpu: &GpuTarget<'_>,
        range: std::ops::RangeInclusive<usize>,
        delta_khz: i32,
    ) {
        const MAX_CONSECUTIVE_FAILURES: usize = 3;
        let mut consecutive_failures = 0;

        for offset in range {
            match run_output(
                gpu,
                SetVfpPointDelta {
                    point: offset,
                    delta: KilohertzDelta(delta_khz),
                },
            ) {
                Ok(_) => {
                    consecutive_failures = 0;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    eprintln!(
                        "Warning: {}, set_vfp offset={} Error. GPU crashed...",
                        e, offset
                    );
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        eprintln!(
                            "Too many consecutive VFP errors ({}). Skipping remaining offsets in range.",
                            consecutive_failures
                        );
                        return;
                    }
                }
            }
        }
    }

    fn set_vfp_curve_warn(
        gpu: &GpuTarget<'_>,
        point: usize,
        vfp_set_range: usize,
        flat_curve_flag: bool,
        main_delta: i32,
        lower_delta: Option<i32>,
    ) {
        if !flat_curve_flag {
            set_vfp_range_warn(
                gpu,
                (point - vfp_set_range)..=(point + vfp_set_range),
                main_delta,
            );
        } else {
            set_vfp_range_warn(gpu, point..=(point + vfp_set_range), main_delta);
            if let Some(ld) = lower_delta {
                set_vfp_range_warn(gpu, (point - vfp_set_range)..=(point - 1), ld);
            }
        }
    }

    fn retry_nvapi_with_backoff<F, E>(
        mut op: F,
        label: &str,
        on_err: E,
    ) -> Result<(), Error>
    where
        F: FnMut() -> Result<(), Error>,
        E: Fn(&Error),
    {
        const BACKOFF_SECS: [u64; 5] = [5, 10, 20, 40, 80];

        for (attempt, &wait_secs) in BACKOFF_SECS.iter().enumerate() {
            if attempt > 0 {
                eprintln!(
                    "Retrying {} in {}s (attempt {}/{})...",
                    label,
                    wait_secs,
                    attempt + 1,
                    BACKOFF_SECS.len()
                );
            }
            sleep(Duration::from_secs(wait_secs));

            match op() {
                Ok(()) => {
                    if attempt > 0 {
                        eprintln!("{} succeeded on attempt {}.", label, attempt + 1);
                    }
                    return Ok(());
                }
                Err(e) if attempt + 1 < BACKOFF_SECS.len() => {
                    eprintln!(
                        "{} failed (attempt {}): {:?}",
                        label,
                        attempt + 1,
                        e
                    );
                    on_err(&e);
                }
                Err(e) => {
                    eprintln!(
                        "{} failed after {} attempts: {:?}",
                        label,
                        BACKOFF_SECS.len(),
                        e
                    );
                    return Err(e);
                }
            }
        }

        unreachable!()
    }

    pub(super) struct TestPressureConfig<'a> {
        pub(super) point: usize,
        pub(super) flat_curve_flag: bool,
        pub(super) vfp_set_range: usize,
        pub(super) init_core_oc_value: i32,
        pub(super) minimum_delta_core_freq_step: i32,
        pub(super) fluctuation_coefficient: i32,
        pub(super) fluctuation_mode: usize,
        pub(super) test_exe: &'a str,
        pub(super) test_code: String,
        pub(super) timeout_loops: u64,
        pub(super) recovery_method: bool,
        pub(super) is_legacy_global_offset: bool,
        /// Stressor CUDA device ordinal (sets CUDA_VISIBLE_DEVICES when non-None).
        pub(super) cuda_device: Option<u32>,
        /// Extra arguments appended verbatim to the stressor command.
        pub(super) stressor_extra_args: &'a [String],
    }

    fn test_initialization(gpu: &GpuTarget<'_>, cfg: &TestPressureConfig<'_>) {
        if cfg.is_legacy_global_offset {
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(
                    PState::P0,
                    ClockDomain::Graphics,
                    KilohertzDelta(cfg.init_core_oc_value),
                )],
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
        gpu: &GpuTarget<'_>,
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
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(
                    PState::P0,
                    ClockDomain::Graphics,
                    KilohertzDelta(cfg.init_core_oc_value + fluctuation_freq),
                )],
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

    #[cfg(windows)]
    fn count_windows_gpu_error_events_by_id(
        start: SystemTime,
        end: SystemTime,
    ) -> Option<Vec<(u32, usize)>> {
        use std::time::UNIX_EPOCH;

        let start_ms = start.duration_since(UNIX_EPOCH).ok()?.as_millis();
        let end_ms = end.duration_since(UNIX_EPOCH).ok()?.as_millis();

        let script = format!(
            "$start=[DateTimeOffset]::FromUnixTimeMilliseconds({start_ms}).LocalDateTime; \
             $end=[DateTimeOffset]::FromUnixTimeMilliseconds({end_ms}).LocalDateTime; \
             $ids=@(153,13,4101,10110,10111); \
             $logs=@('System','Microsoft-Windows-DriverFrameworks-UserMode/Operational'); \
             foreach($id in $ids){{ \
                 $idCount=0; \
                 foreach($log in $logs){{ \
                     try {{ $idCount += @(Get-WinEvent -FilterHashtable @{{LogName=$log;Id=$id;StartTime=$start;EndTime=$end}} -ErrorAction Stop).Count }} catch {{}} \
                 }}; \
                 Write-Output ($id.ToString() + '=' + $idCount.ToString()) \
             }}"
        );

        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;

        if !output.status.success() {
            eprintln!(
                "Warning: Failed to query Windows Event Log: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }

        let output_text = String::from_utf8_lossy(&output.stdout);
        let mut counts = Vec::new();
        for line in output_text.lines() {
            let mut parts = line.trim().split('=');
            let id = parts.next()?.parse::<u32>().ok()?;
            let count = parts.next()?.parse::<usize>().ok()?;
            counts.push((id, count));
        }
        Some(counts)
    }

    pub(super) fn run(
        gpu: &GpuTarget<'_>,
        _matches: &ArgMatches,
        cfg: &TestPressureConfig<'_>,
    ) -> i32 {
        let app_path = String::from(cfg.test_exe);
        // Build argv as a structured Vec so paths or codes containing whitespace
        // are not silently re-tokenized into multiple arguments.
        let mut args: Vec<String> = vec![cfg.test_code.clone(), cfg.timeout_loops.to_string()];
        let timeout_budget_secs = cfg.timeout_loops * 8;
        println!("Timeout: {}s", timeout_budget_secs);
        if cfg.recovery_method {
            args.push("--aggressive-recovery".to_string());
        }

        let mut count = 0;
        loop {
            let mut cmd = Command::new(app_path.clone());
            cmd.args(&args);
            if !cfg.stressor_extra_args.is_empty() {
                cmd.args(cfg.stressor_extra_args);
            }
            if let Some(dev) = cfg.cuda_device {
                // PCI_BUS_ID makes CUDA ordinals match NVAPI/NVML ordering,
                // so --gpu N and CUDA_VISIBLE_DEVICES=N refer to the same device.
                cmd.env("CUDA_DEVICE_ORDER", "PCI_BUS_ID");
                cmd.env("CUDA_VISIBLE_DEVICES", dev.to_string());
            }
            match cmd.spawn() {
                Ok(mut process) => {
                    let mut exit_code = 1;

                    #[cfg(windows)]
                    let event_window_start = SystemTime::now();

                    let test_start_at = Instant::now();
                    let mut last_fluctuation = Instant::now();
                    let mut in_test_check_number = 0;
                    let mut fluctuation_h_l_flag = false;
                    let mut thrm_or_pwr_limit_number = 0;
                    let _ = retry_nvapi_with_backoff(
                        || {
                            run_output(
                                gpu,
                                ResetVfpDeltas {
                                    domain: VfpResetDomain::All,
                                },
                            )
                            .map(|_| ())
                        },
                        "ResetVfpDeltas",
                        |e| {
                            eprintln!("Warning: Failed to reset GPU due to {:?}", e);
                        },
                    );
                    sleep(Duration::from_secs(1));
                    test_initialization(gpu, cfg);
                    sleep(Duration::from_secs(1));

                    loop {
                        if last_fluctuation.elapsed() >= Duration::from_millis(1500) {
                            in_test_check_number += 1;
                            println!("inducing freq fluctuation...");

                            fluctuation_h_l_flag =
                                apply_fluctuation(gpu, cfg, fluctuation_h_l_flag);

                            println!("Processing in-test v-f check #{}...", in_test_check_number);

                            if !cfg.is_legacy_global_offset {
                                match voltage_frequency_check(
                                    std::slice::from_ref(gpu),
                                    cfg.point,
                                    print_scan_separator,
                                ) {
                                    Ok(checks) if checks.iter().all(|check| check.precise) => {}
                                    Ok(checks) => {
                                        for check in checks {
                                            println!(
                                                "GPU {} V/F check at point {} precise={}",
                                                check.gpu_id, cfg.point, check.precise
                                            );
                                        }
                                        println!("Considering trig'd thrm/pwr capping!!!");
                                        thrm_or_pwr_limit_number += 1;
                                    }
                                    Err(e) => {
                                        eprintln!("Warning: Failed to read v-f info: {}", e);
                                        force_kill_process(&mut process, "v-f check read failure");
                                        break;
                                    }
                                }

                                match run_output(gpu, QueryVfpPointVoltage { point: cfg.point }) {
                                    Ok(v) => {
                                        println!("Voltage at point {}: {}", cfg.point, v);
                                        run_output(
                                            gpu,
                                            SetVfpVoltageLock {
                                                voltage_target: NvapiLockedVoltageTarget::Voltage(
                                                    v,
                                                ),
                                                feedback: false,
                                            },
                                        )
                                        .unwrap_or_else(
                                            |err| {
                                                eprintln!(
                                                    "Warning: Failed to set voltage due to {:?}",
                                                    err
                                                );
                                            },
                                        );
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

                        sleep(Duration::from_secs(1));

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
                            let _ = retry_nvapi_with_backoff(
                                || {
                                    run_output(
                                        gpu,
                                        ResetVfpDeltas {
                                            domain: VfpResetDomain::All,
                                        },
                                    )
                                    .map(|_| ())
                                },
                                "ResetVfpDeltas (timeout recovery)",
                                |e| {
                                    eprintln!("Warning: Failed to reset GPU due to {:?}", e);
                                },
                            );
                            break;
                        }
                    }

                    #[cfg(windows)]
                    let windows_event_counts = {
                        let event_window_end = SystemTime::now();
                        count_windows_gpu_error_events_by_id(event_window_start, event_window_end)
                    };

                    if exit_code == 0 {
                        eprintln!("Process finished successfully.");
                        let throttle_ratio = if in_test_check_number > 0 {
                            thrm_or_pwr_limit_number as f64 / in_test_check_number as f64
                        } else {
                            0.0
                        };
                        if throttle_ratio > 0.3 {
                            eprintln!(
                                "Warning: Thermal/power throttling detected ({:.0}%).",
                                throttle_ratio * 100.0
                            );
                        }
                    } else {
                        eprintln!("Process finished with exit code {}.", exit_code);
                    }

                    #[cfg(windows)]
                    {
                        match windows_event_counts {
                            Some(id_counts) => {
                                let total_hits: usize = id_counts.iter().map(|(_, c)| *c).sum();
                                if total_hits > 0 {
                                    eprintln!(
                                        "Detected {} Windows GPU/driver-related event(s) during pressure test:",
                                        total_hits
                                    );
                                    for (id, count) in id_counts {
                                        if count > 0 {
                                            eprintln!("  Event ID {}: {}", id, count);
                                        }
                                    }
                                    if exit_code == 0 {
                                        eprintln!(
                                            "Marking this run as failed due to Windows event log hits."
                                        );
                                        exit_code = 1;
                                    }
                                }
                            }
                            None => {
                                eprintln!(
                                    "Warning: Failed to query Windows Event Log for this run."
                                );
                            }
                        }
                    }

                    // If a run failed (non-zero exit), re-apply the autoscan profile to
                    // restore the locked volt/freq state before the next test. This helps
                    // ensure subsequent runs start from the expected operating point after
                    // driver resets (TDR) or other disruptive events.
                    if exit_code != 0 {
                        eprintln!(
                            "Test returned non-zero ({}). Re-applying autoscan profile before next run...",
                            exit_code
                        );
                        let _ = retry_nvapi_with_backoff(
                            || apply_autoscan_profile(gpu, _matches, 80),
                            "apply_autoscan_profile",
                            |e| {
                                eprintln!(
                                    "apply_autoscan_profile attempt failed: {:?}",
                                    e
                                );
                            },
                        );
                        // Small sleep to allow the profile to settle.
                        sleep(Duration::from_millis(500));
                    }

                    return exit_code;
                }
                Err(e) => {
                    count += 1;
                    eprintln!("Failed to start process: {}, try again.", e);
                    sleep(Duration::from_secs(1));
                    if count >= cfg.timeout_loops {
                        eprintln!("Timeout reached, giving up on starting the process.");
                        return 1;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn test_pressure(
    gpu: &GpuTarget<'_>,
    matches: &ArgMatches,
    point: usize,
    flat_curve_flag: bool,
    vfp_set_range: usize,
    init_core_oc_value: i32,
    minimum_delta_core_freq_step: i32,
    fluctuation_coefficient: i32,
    fluctuation_mode: usize,
    test_exe: &str,
    test_code: String,
    timeout_loops: u64,
    recovery_method: bool,
    is_legacy_global_offset: bool,
    cuda_device: Option<u32>,
    stressor_extra_args: &[String],
) -> i32 {
    let cfg = pressure_runner::TestPressureConfig {
        point,
        flat_curve_flag,
        vfp_set_range,
        init_core_oc_value,
        minimum_delta_core_freq_step,
        fluctuation_coefficient,
        fluctuation_mode,
        test_exe,
        test_code,
        timeout_loops,
        recovery_method,
        is_legacy_global_offset,
        cuda_device,
        stressor_extra_args,
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
    recovery_method_switch: bool,
    test_duration: u64,
    endurance_coefficient: u64,
    cuda_device: Option<u32>,
    stressor_extra_args: &'a [String],
}

#[allow(clippy::too_many_arguments)]
fn build_common_phase_args<'a>(
    matches: &'a ArgMatches,
    minimum_delta_core_freq_step: i32,
    fluctuation_coefficient: i32,
    fluctuation_mode: usize,
    test_exe: &'a str,
    delimiter: &'a str,
    recovery_method_switch: bool,
    test_duration: u64,
    endurance_coefficient: u64,
    cuda_device: Option<u32>,
    stressor_extra_args: &'a [String],
) -> CommonPhaseArgs<'a> {
    CommonPhaseArgs {
        matches,
        minimum_delta_core_freq_step,
        fluctuation_coefficient,
        fluctuation_mode,
        test_exe,
        delimiter,
        recovery_method_switch,
        test_duration,
        endurance_coefficient,
        cuda_device,
        stressor_extra_args,
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

fn pre_load_vf_recheck(gpu: &GpuTarget<'_>, point: usize) -> bool {
    println!("Waiting for pre-load volt-freq recheck");
    sleep(Duration::from_secs(1));

    // voltage_frequency_check 可能仍返回 Result，我们这里捕获错误并当作失败处理
    let checks =
        match voltage_frequency_check(std::slice::from_ref(gpu), point, print_scan_separator) {
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
        .map(|check| format!("GPU {} precise={}", check.gpu_id, check.precise))
        .collect::<Vec<_>>()
        .join(", ");

    eprintln!("V/F check failed at point {point}: {summary}");
    false // 检查失败
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
    gpu: &GpuTarget<'_>,
    args: &LegacyPhaseArgs<'_>,
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
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
        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(*init_core_oc_value),
            )],
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

        let test_flag = test_pressure(
            gpu,
            args.common.matches,
            args.point,
            args.flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            args.common.test_exe,
            format!("legacy{}{}", args.common.delimiter, *test_code),
            args.common.test_duration,
            args.common.recovery_method_switch,
            true,
            args.common.cuda_device,
            args.common.stressor_extra_args,
        );
        writeln!(l, "Test result is code #{} .", test_flag)?;

        if test_flag != 0 {
            println!(
                "Short Test #{} FAILED at +{}kHz",
                *test_code, *init_core_oc_value
            );
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
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
    gpu: &GpuTarget<'_>,
    args: &LegacyPhaseArgs<'_>,
    init_core_oc_value: &mut i32,
    test_code: &mut usize,
) -> Result<(), Error> {
    println!("Initiating Long Test...");
    writeln!(l, "Initiating Long Test...")?;

    loop {
        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Graphics,
                KilohertzDelta(*init_core_oc_value),
            )],
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

        let long_flag = test_pressure(
            gpu,
            args.common.matches,
            args.point,
            args.flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            args.common.test_exe,
            format!("legacy{}{}", args.common.delimiter, *test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.recovery_method_switch,
            true,
            args.common.cuda_device,
            args.common.stressor_extra_args,
        );
        writeln!(l, "Test result is code #{} .", long_flag)?;

        if long_flag != 0 {
            println!(
                "Long Test #{} FAILED at +{}kHz",
                *test_code, *init_core_oc_value
            );
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
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

#[allow(clippy::too_many_arguments)]
fn run_gpuboostv3_short_phase<V: std::fmt::Display + Copy>(
    l: &mut fs::File,
    gpu: &GpuTarget<'_>,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: V,
    flat_curve_flag: bool,
    init_core_oc_value: &mut i32,
    core_oc_safe_limit: &mut i32,
    resuming_flag: &mut bool,
) -> Result<usize, Error> {
    let mut test_num = 0;
    let mut test_code = 0;

    loop {
        set_nvapi_vfp_curve_delta(
            gpu,
            point,
            args.vfp_set_range,
            flat_curve_flag,
            *init_core_oc_value,
            Some(*init_core_oc_value - args.common.minimum_delta_core_freq_step),
        )?;

        let mut attempt = 0;
        let max_attempts = 10;

        while attempt < max_attempts {
            attempt += 1;

            set_nvapi_vfp_curve_delta(
                gpu,
                point,
                args.vfp_set_range,
                flat_curve_flag,
                *init_core_oc_value,
                Some(*init_core_oc_value - args.common.minimum_delta_core_freq_step),
            )?;

            if pre_load_vf_recheck(gpu, point) {
                println!("V/F recheck passed on attempt {attempt}");
                break;
            } else {
                eprintln!("Retrying set_nvapi_vfp_curve_delta... (attempt {attempt})");
                sleep(Duration::from_millis(500));
            }

            if attempt == max_attempts {
                return Err(Error::Custom(format!(
                    "V/F recheck failed after {max_attempts} attempts"
                )));
            }
        }

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

        let test_flag = test_pressure(
            gpu,
            args.common.matches,
            point,
            flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            args.common.test_exe,
            format!("{}{}{}", point, args.common.delimiter, test_code),
            args.common.test_duration,
            args.common.recovery_method_switch,
            false,
            args.common.cuda_device,
            args.common.stressor_extra_args,
        );
        println!("{}", test_flag);
        writeln!(l, "Test result is code #{} .", test_flag)?;

        if test_flag != 0 {
            run_output(
                gpu,
                ResetVfpDeltas {
                    domain: VfpResetDomain::All,
                },
            )?;
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

#[allow(clippy::too_many_arguments)]
fn run_gpuboostv3_long_phase<V: std::fmt::Display + Copy>(
    l: &mut fs::File,
    gpu: &GpuTarget<'_>,
    args: &GpuBoostPhaseArgs<'_>,
    point: usize,
    v: V,
    flat_curve_flag: bool,
    init_core_oc_value: &mut i32,
    test_code: &mut usize,
) -> Result<(), Error> {
    let mut long_duration_flag;
    println!("Initiating Long Test...");
    writeln!(l, "Initiating Long Test...")?;

    loop {
        let mut attempt = 0;
        let max_attempts = 5;

        while attempt < max_attempts {
            attempt += 1;

            set_nvapi_vfp_curve_delta(
                gpu,
                point,
                args.vfp_set_range,
                flat_curve_flag,
                *init_core_oc_value,
                Some(*init_core_oc_value - args.common.minimum_delta_core_freq_step),
            )?;

            if pre_load_vf_recheck(gpu, point) {
                println!("V/F recheck passed on attempt {attempt}");
                break;
            } else {
                eprintln!("Retrying set_nvapi_vfp_curve_delta... (attempt {attempt})");
                sleep(Duration::from_millis(500));
            }

            if attempt == max_attempts {
                return Err(Error::Custom(format!(
                    "V/F recheck failed after {max_attempts} attempts"
                )));
            }
        }

        *test_code += 1;
        log_point_test_header(
            l,
            *test_code,
            point,
            v,
            "freq_delta",
            KilohertzDelta(*init_core_oc_value),
        )?;

        long_duration_flag = test_pressure(
            gpu,
            args.common.matches,
            point,
            flat_curve_flag,
            args.vfp_set_range,
            *init_core_oc_value,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            args.common.test_exe,
            format!("{}{}{}", point, args.common.delimiter, *test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.recovery_method_switch,
            false,
            args.common.cuda_device,
            args.common.stressor_extra_args,
        );
        if long_duration_flag != 0 {
            run_output(
                gpu,
                ResetVfpDeltas {
                    domain: VfpResetDomain::All,
                },
            )?;
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
    gpu: &GpuTarget<'_>,
    gpus: &Vec<GpuTarget<'_>>,
    args: &MemOcPhaseArgs<'_>,
    mem_voltage: V,
    init_vmem_oc_value: &mut i32,
    mem_oc_safe_limit: &mut i32,
) -> Result<(), Error> {
    let mut mem_test_code: usize = 0;
    let mut mem_test_num: usize = 0;

    loop {
        match handle_lock_vfp(gpus, args.common.matches, args.point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        set_nvapi_pstate_clock_offsets(
            gpu,
            [(
                PState::P0,
                ClockDomain::Memory,
                KilohertzDelta(*init_vmem_oc_value),
            )],
        )?;

        mem_test_num += 1;
        mem_test_code += 1;

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

        let mem_test_flag = test_pressure(
            gpu,
            args.common.matches,
            args.point,
            false,
            args.vfp_set_range,
            0,
            args.common.minimum_delta_core_freq_step,
            args.common.fluctuation_coefficient,
            args.common.fluctuation_mode,
            args.common.test_exe,
            format!("{}{}{}", args.point, args.common.delimiter, mem_test_code),
            args.common.endurance_coefficient * args.common.test_duration,
            args.common.recovery_method_switch,
            true,
            args.common.cuda_device,
            args.common.stressor_extra_args,
        );

        writeln!(l, "Test result is code #{} .", mem_test_flag)?;

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

pub fn autoscan_gpuboostv3(gpus: &Vec<GpuTarget<'_>>, matches: &ArgMatches) -> Result<(), Error> {
    use super::autoscan_config::AutoscanConfig;
    let cfg = AutoscanConfig::from_autoscan_matches(matches)?;
    let mut is_ultrafast = cfg.is_ultrafast;
    if is_ultrafast {
        println!("Ultrafast mode interpolation active...");
    }

    let test_exe = cfg.test_exe.as_str();
    let log_filename = cfg.log.as_str();
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
            let voltage_limits = handle_test_voltage_limits(gpus, matches, print_scan_separator)?;
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
                writeln!(l)?;
                writeln!(
                    l,
                    "GPU {} minimum_voltage_point: {} @ {}",
                    limits.gpu_id, limits.lower_point, minimum_voltage
                )?;
                writeln!(
                    l,
                    "GPU {} maximum_voltage_point: {} @ {}",
                    limits.gpu_id, limits.upper_point, maximum_voltage
                )?;
            }
            writeln!(l, "common_voltage_point_range: {}-{}", lvp, uvp)?;

            (lvp, uvp)
        }
    };

    let mut init_vmem_oc_value = 0;

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
        match handle_lock_vfp(gpus, matches, upper_voltage_point, false) {
            Ok(_) => println!("Voltage locked successfully."),
            Err(e) => eprintln!("Error: Failed to lock voltage - {:?}", e),
        }

        let readout_f = run_output(gpu, QueryGpuStatus)?.clone().clocks;

        let mut clocks = Vec::new();
        for (clock_name, freq) in readout_f {
            // Store the clock name and frequency in a data structure.
            clocks.push((clock_name.to_string(), freq));
        }
        // Now you can search for the Memory Clock:
        print_scan_separator();
        if let Some((_, memory_clock)) = clocks.iter().find(|(name, _)| name.contains("Memory")) {
            println!(
                "{}: {}",
                nvoc_core::color::stylize_title("Memory Clock"),
                nvoc_core::color::stylize(&format!("{}", memory_clock), false)
            );
            init_vmem_oc_value = (memory_clock.0 / 25) as i32;
            println!(
                "{} {}",
                nvoc_core::color::stylize_title("Memory OC start at"),
                nvoc_core::color::stylize(&format!("+{} MHz", init_vmem_oc_value / 1000), false)
            );
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
        let points = run_output(gpu, QueryGpuStatus)?
            .vfp
            .ok_or(Error::VfpUnsupported)?
            .graphics;

        let mut point = lower_voltage_point;
        let mut resuming_flag = false;
        let mut last_succeeded_freq = init_core_oc_value;
        let mut last_failed_freq = core_oc_safe_limit_ref;
        let recovery_method_switch: bool = cfg.recovery_method.unwrap_or(is_50_series);

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
        let checks =
            voltage_frequency_check(std::slice::from_ref(gpu), point, print_scan_separator)
                .expect("Failed to read v-f info");
        if !checks.iter().all(|check| check.precise) {
            let summary = checks
                .iter()
                .map(|check| format!("GPU {} precise={}", check.gpu_id, check.precise))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("Warning: default V/F self-check failed at point {point}: {summary}");
        }
        let mut v;
        let mut default_frequency;
        let mut prev_endpoint_delta: Option<i32> = None;

        //prepare GPU OC parameter for extreme OC...
        if let Err(e) = apply_autoscan_profile(gpu, matches, 80) {
            eprintln!("apply_autoscan_profile failed: {:?}, continuing scan...", e);
        }

        let freq_step_exp = 3;
        let endurance_coefficient = 2;
        let vfp_set_range = 3;
        let mut test_duration: u64 = 10;
        if is_ultrafast {
            test_duration += test_duration / 2;
        };
        let fluctuation_mode = 3; // 1 = 0-, 2 = ±, 3 = 0+
        let mut flat_curve_flag: bool;
        let phase_args = GpuBoostPhaseArgs {
            common: build_common_phase_args(
                matches,
                minimum_delta_core_freq_step,
                fluctuation_coefficient,
                fluctuation_mode,
                test_exe,
                delimiter.as_str(),
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
                cfg.cuda_device,
                &cfg.stressor_extra_args,
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

            set_nvapi_pstate_clock_offsets(
                gpu,
                [(
                    PState::P0,
                    ClockDomain::Memory,
                    KilohertzDelta(init_vmem_oc_value),
                )],
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
                &mut test_code,
            )?;
            write!(l, "\nFinished core OC on point: #{}\n", point)?;
            println!(
                "Core OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                v,
                KilohertzDelta(init_core_oc_value)
            );

            let p_save = VfPoint {
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
                    let p_save_prev = VfPoint {
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
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))],
            )?;

            let mut mem_oc_safe_limit = 0;
            let minimum_delta_mem_freq_step = 1000;
            let mem_freq_step_exp = 8;
            if let Some((_, memory_clock)) = clocks.iter().find(|(name, _)| name.contains("Memory"))
            {
                println!(
                    "{}: {}",
                    nvoc_core::color::stylize_title("Memory Clock"),
                    nvoc_core::color::stylize(&format!("{}", memory_clock), false)
                );
                println!(
                    "{} {}",
                    nvoc_core::color::stylize_title("Memory OC test start at"),
                    nvoc_core::color::stylize(
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
                common: build_common_phase_args(
                    matches,
                    minimum_delta_core_freq_step,
                    fluctuation_coefficient,
                    fluctuation_mode,
                    test_exe,
                    delimiter.as_str(),
                    recovery_method_switch,
                    test_duration,
                    endurance_coefficient,
                    cfg.cuda_device,
                    &cfg.stressor_extra_args,
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
            )?;
            write!(l, "\nFinished on point: #{}.\n", point)?;
            println!(
                "mem OC finished on point: #{}, voltage: #{}, delta: #+{}. ",
                point,
                mem_voltage,
                KilohertzDelta(init_vmem_oc_value)
            );
        }
        run_output(gpu, ResetCoolerLevels).unwrap_or_else(|_e| {
            handle_reset_nvml_cooler_single_gpu(gpu, "all")
                .unwrap_or_else(|e| eprintln!("Failed to reset cooler: {e}"))
        })
    }
    writeln!(l, "VFP Scan succeeded...")?;
    Ok(())
}

pub fn autoscan_legacy(gpus: &Vec<GpuTarget<'_>>, matches: &ArgMatches) -> Result<(), Error> {
    use super::autoscan_config::AutoscanConfig;
    let cfg = AutoscanConfig::from_legacy_matches(matches)?;
    let test_exe = cfg.test_exe.as_str();
    let log_filename = cfg.log.as_str();

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
        let info = run_output(gpu, QueryGpuInfo)?;
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
                recovery_method_switch,
                test_duration,
                endurance_coefficient,
                cfg.cuda_device,
                &cfg.stressor_extra_args,
            ),
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
                &mut init_core_oc_value,
                &mut core_oc_safe_limit,
                &mut test_code,
                &mut resuming_flag,
            )?;

            run_legacy_long_phase(
                &mut l,
                gpu,
                &phase_args,
                &mut init_core_oc_value,
                &mut test_code,
            )?;

            write!(
                l,
                "\nLegacy OC scan finished. Final freq_delta: +{}kHz\n",
                init_core_oc_value
            )?;
            println!(
                "Legacy OC scan finished. Final freq_delta: +{}kHz",
                init_core_oc_value
            );

            // Restore GPU to stock offset after scan
            set_nvapi_pstate_clock_offsets(
                gpu,
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))],
            )?;
            run_output(gpu, ResetCoolerLevels).unwrap_or_else(|_e| {
                handle_reset_nvml_cooler_single_gpu(gpu, "all")
                    .unwrap_or_else(|e| eprintln!("Failed to reset cooler: {e}"))
            })
        }
    }

    writeln!(l, "Legacy VFP Scan succeeded...")?;
    Ok(())
}
