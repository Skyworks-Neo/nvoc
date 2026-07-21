use super::runtime::{retry_operation_with_backoff, run_output};
use crate::oc_profile_function::apply_autoscan_profile;
use crate::progressbar::{ScanProgress, forward_child_output, progress_print};
use crate::scan_strategy::FluctuationStrategy;
use crate::scan_support::voltage_frequency_check;
use crate::stressor_process::{bundled_command, external_command, is_bundled, resolve_profile};
use clap::ArgMatches;
use nvoc_core::{
    ClockDomain, GpuTarget, KilohertzDelta, NvapiLockedVoltageTarget, PState, QueryGpuStatus,
    QueryVfpPointVoltage, ResetVfpDeltas, SetVfpPointDelta, SetVfpVoltageLock, VfpResetDomain,
    set_nvapi_pstate_clock_offsets,
};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

pub(super) struct PressureTestConfig<'a> {
    // This config has enough context to both set the target GPU state and run
    // the external stressor process without reaching back into phase structs.
    pub(super) point: usize,
    pub(super) flat_curve_flag: bool,
    pub(super) vfp_set_range: usize,
    pub(super) init_core_oc_value: i32,
    pub(super) minimum_delta_core_freq_step: i32,
    pub(super) fluctuation: FluctuationStrategy,
    pub(super) test_exe: &'a str,
    pub(super) minload_exe: &'a str,
    pub(super) test_code: String,
    pub(super) timeout_loops: u64,
    pub(super) is_legacy_global_offset: bool,
    pub(super) test_duration_secs: u64,
    pub(super) progress: Option<&'a ScanProgress>,
    /// Stressor CUDA device ordinal (sets CUDA_VISIBLE_DEVICES when non-None).
    pub(super) cuda_device: Option<u32>,
    /// Extra arguments appended verbatim to the stressor command.
    pub(super) stressor_extra_args: &'a [String],
    pub(super) stressor_profile: &'a str,
    pub(super) stressor_config: Option<&'a str>,
    /// GpuId.0 value of the GPU under test (used for event-log GPU filtering).
    #[cfg(windows)]
    pub(super) target_gpu_id: u32,
}

fn set_vfp_range_warn(gpu: &GpuTarget<'_>, range: std::ops::RangeInclusive<usize>, delta_khz: i32) {
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

fn vfp_curve_range(point: usize, vfp_set_range: usize) -> std::ops::RangeInclusive<usize> {
    point.saturating_sub(vfp_set_range)..=point.saturating_add(vfp_set_range)
}

fn lower_vfp_curve_range(
    point: usize,
    vfp_set_range: usize,
) -> Option<std::ops::RangeInclusive<usize>> {
    let lower = point.saturating_sub(vfp_set_range);
    let upper = point.checked_sub(1)?;
    if lower > upper {
        return None;
    }
    Some(lower..=upper)
}

fn set_vfp_curve_warn(
    gpu: &GpuTarget<'_>,
    point: usize,
    vfp_set_range: usize,
    flat_curve_flag: bool,
    main_delta: i32,
    lower_delta: Option<i32>,
) {
    // A flat curve can only apply the main delta above the target point; the
    // lower side uses the previous bin to avoid over-tightening low points.
    if !flat_curve_flag {
        set_vfp_range_warn(gpu, vfp_curve_range(point, vfp_set_range), main_delta);
    } else {
        set_vfp_range_warn(gpu, point..=point.saturating_add(vfp_set_range), main_delta);
        if let Some(ld) = lower_delta
            && let Some(range) = lower_vfp_curve_range(point, vfp_set_range)
        {
            set_vfp_range_warn(gpu, range, ld);
        }
    }
}

fn test_initialization(gpu: &GpuTarget<'_>, cfg: &PressureTestConfig<'_>) {
    if cfg.is_legacy_global_offset {
        // Legacy mode has no programmable VFP curve, so initialize by setting a
        // single P0 graphics offset.
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
    cfg: &PressureTestConfig<'_>,
    fluctuation_h_l_flag: bool,
    elapsed_since_test_start: Duration,
) -> (i32, bool) {
    let (fluctuation_freq, new_h_l_flag) = cfg.fluctuation.next_delta(
        cfg.init_core_oc_value,
        cfg.minimum_delta_core_freq_step,
        elapsed_since_test_start,
        fluctuation_h_l_flag,
    );

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

    (fluctuation_freq, new_h_l_flag)
}

fn force_kill_process(process: &mut Child, reason: &str) {
    // Kill the full child process tree on Windows in case the stressor loaded
    // helper processes of its own.
    #[cfg(windows)]
    {
        let pid = process.id();
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

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
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct WindowsGpuEvent {
    event_id: u32,
    /// Raw GPUID from event message (matches `GpuId.0` which = pci_bus * 256).
    /// `None` for system-wide events (e.g. `\Device\Video3`) that carry no GPUID.
    gpu_bus_id: Option<u32>,
    /// True when the event message contains Graphics FECS Exception.
    is_fecs: bool,
    /// True when the event message contains Restarting TDR or Reset TDR.
    is_tdr: bool,
}

#[cfg(windows)]
fn query_windows_gpu_events(start: SystemTime, end: SystemTime) -> Option<Vec<WindowsGpuEvent>> {
    use std::time::UNIX_EPOCH;

    let start_ms = start.duration_since(UNIX_EPOCH).ok()?.as_millis();
    let end_ms = end.duration_since(UNIX_EPOCH).ok()?.as_millis();

    let script_path = "./test/windows_gpu_event_query.ps1";

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script_path,
            "-StartMs",
            &start_ms.to_string(),
            "-EndMs",
            &end_ms.to_string(),
        ])
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
    let mut events = Vec::new();
    for line in output_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(5, '|');
        let event_id = parts.next()?.parse::<u32>().ok()?;
        let gpu_bus_str = parts.next()?;
        let gpu_bus_id = if gpu_bus_str.is_empty() {
            None
        } else {
            gpu_bus_str.parse::<u32>().ok()
        };
        let is_fecs = parts.next() == Some("1");
        let is_tdr = parts.next() == Some("1");
        events.push(WindowsGpuEvent {
            event_id,
            gpu_bus_id,
            is_fecs,
            is_tdr,
        });
    }
    Some(events)
}

#[cfg(not(windows))]
fn count_linux_gpu_xid_events_by_time(
    start: SystemTime,
    end: SystemTime,
) -> Option<Vec<(u32, usize)>> {
    use std::time::UNIX_EPOCH;

    let start_epoch = start.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let end_epoch = end.duration_since(UNIX_EPOCH).ok()?.as_secs();

    let output = Command::new("dmesg")
        .args([
            "--since",
            &format!("@{start_epoch}"),
            "--until",
            &format!("@{end_epoch}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;

    if !output.status.success() {
        eprintln!(
            "Warning: Failed to query dmesg (time range {} → {}): {}",
            start_epoch,
            end_epoch,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let output_text = String::from_utf8_lossy(&output.stdout);
    let mut counts: Vec<(u32, usize)> = Vec::new();

    for line in output_text.lines() {
        if !line.contains("NVRM: Xid") {
            continue;
        }
        let xid: u32 = if let Some(pos) = line.find("): ") {
            let after = &line[pos + 3..];
            let num_end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            match after[..num_end].parse() {
                Ok(n) => n,
                Err(_) => continue,
            }
        } else {
            continue;
        };

        if let Some(existing) = counts.iter_mut().find(|(id, _)| *id == xid) {
            existing.1 += 1;
        } else {
            counts.push((xid, 1));
        }
    }

    Some(counts)
}

#[cfg(windows)]
fn pnp_recover_gpu(gpu: &GpuTarget<'_>) -> bool {
    let pci_bus = gpu.id.pci_bus();

    let script_path = "./test/windows_oc_pnp_recover.ps1";
    eprintln!(
        "pnp_recover: Triggering PnP disable/enable cycle for GPU at PCI bus {} (GpuId: {})...",
        pci_bus, gpu.id.0
    );

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script_path,
            "-TargetPciBus",
            &pci_bus.to_string(),
        ])
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                eprintln!("pnp_recover: PnP cycle completed successfully.");
                eprintln!("stdout: {}", String::from_utf8_lossy(&out.stdout).trim());
                // Wait for GPU to re-appear in NVML
                sleep(Duration::from_secs(10));
                true
            } else {
                eprintln!(
                    "pnp_recover: PnP cycle failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                false
            }
        }
        Err(e) => {
            eprintln!("pnp_recover: Failed to launch recovery script: {e}");
            false
        }
    }
}

pub(super) fn run_pressure_test(
    gpu: &GpuTarget<'_>,
    matches: &ArgMatches,
    cfg: &PressureTestConfig<'_>,
) -> i32 {
    let app_path = String::from(cfg.test_exe);
    let stressor_profile = if cfg.stressor_config.is_none() {
        match resolve_profile(gpu, cfg.stressor_profile) {
            Ok(profile) => Some(profile),
            Err(e) => {
                eprintln!("Failed to resolve stressor profile: {e}");
                return 1;
            }
        }
    } else {
        None
    };
    let timeout_budget_secs = cfg.timeout_loops * 15;
    progress_print(cfg.progress, format!("Timeout: {}s", timeout_budget_secs));

    let mut count = 0;
    loop {
        // Rebuild Command every retry so env vars, pipes, and extra args are
        // fresh for each stressor attempt.
        let mut cmd = if is_bundled(&app_path) {
            match bundled_command(
                stressor_profile.as_deref(),
                cfg.stressor_config,
                (cfg.timeout_loops * 5) as f64,
                cfg.cuda_device,
                cfg.stressor_extra_args,
            ) {
                Ok(command) => command,
                Err(e) => {
                    eprintln!("Failed to prepare bundled stressor process: {e}");
                    return 1;
                }
            }
        } else {
            external_command(
                &app_path,
                stressor_profile.as_deref(),
                cfg.stressor_config,
                (cfg.timeout_loops * 5) as f64,
                cfg.cuda_device,
                cfg.stressor_extra_args,
            )
        };
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(mut process) => {
                let mut exit_code = 1;
                let mut output_threads: Vec<JoinHandle<()>> = Vec::new();
                let test_progress = cfg.progress.map(|progress| {
                    progress.begin_test(cfg.test_code.clone(), cfg.test_duration_secs)
                });

                if let Some(stdout) = process.stdout.take() {
                    output_threads.push(forward_child_output(
                        stdout,
                        cfg.progress.map(|progress| progress.total_bar()),
                        false,
                    ));
                }
                if let Some(stderr) = process.stderr.take() {
                    output_threads.push(forward_child_output(
                        stderr,
                        cfg.progress.map(|progress| progress.total_bar()),
                        true,
                    ));
                }

                #[cfg(windows)]
                let event_baseline = query_windows_gpu_events(
                    SystemTime::now() - Duration::from_secs(30),
                    SystemTime::now(),
                );
                #[cfg(windows)]
                let event_window_start = SystemTime::now();
                #[cfg(windows)]
                let mut last_event_poll = Instant::now();
                #[cfg(windows)]
                let poll_interval = Duration::from_secs(3);
                #[cfg(not(windows))]
                let linux_xid_window_start = SystemTime::now();

                let test_start_at = Instant::now();
                let mut last_fluctuation = Instant::now();
                let mut in_test_check_number = 0;
                let mut fluctuation_h_l_flag = false;
                let mut thrm_or_pwr_limit_number = 0;
                let _ = retry_operation_with_backoff(
                    || {
                        run_output(
                            gpu,
                            ResetVfpDeltas {
                                domain: VfpResetDomain::Core,
                            },
                        )
                        .map(|_| ())
                    },
                    "ResetVfpDeltas",
                    5,
                    5,
                    cfg.minload_exe,
                    cfg.cuda_device,
                );
                test_initialization(gpu, cfg);

                loop {
                    if last_fluctuation.elapsed() >= Duration::from_millis(1) {
                        // Frequency fluctuation is intentionally driven while
                        // the stressor is running to expose marginal V/F points.
                        in_test_check_number += 1;
                        let (freq_delta, new_flag) = apply_fluctuation(
                            gpu,
                            cfg,
                            fluctuation_h_l_flag,
                            test_start_at.elapsed(),
                        );
                        fluctuation_h_l_flag = new_flag;
                        let fluctuation_freq = freq_delta;
                        let state_label = if fluctuation_h_l_flag { "LOW" } else { "HIGH" };
                        // update a single status line (replaces noisy per-check printing)
                        if let Some(progress) = cfg.progress {
                            progress.set_status(format!(
                                "inducing freq fluctuation; state: {}. in-test v-f check #{}",
                                state_label, in_test_check_number
                            ));
                        } else {
                            progress_print(
                                None,
                                format!(
                                    "inducing freq fluctuation; state: {}. in-test v-f check #{}",
                                    state_label, in_test_check_number
                                ),
                            );
                        }

                        if !cfg.is_legacy_global_offset {
                            match voltage_frequency_check(std::slice::from_ref(gpu), cfg.point) {
                                Ok(checks) if checks.iter().all(|check| check.precise) => {}
                                Ok(checks) => {
                                    // summarize checks into a single status line instead of printing per-GPU
                                    let summary = checks
                                        .iter()
                                        .map(|c| {
                                            format!(
                                                "{}:precise={},matched_point={:?}",
                                                c.gpu_id, c.precise, c.matched_point
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    thrm_or_pwr_limit_number += 1;
                                    if let Some(progress) = cfg.progress {
                                        progress.set_status(format!(
                                            "V/F summary [{}] (possible thrm/pwr capping)",
                                            summary
                                        ));
                                    } else {
                                        progress_print(
                                            None,
                                            format!(
                                                "V/F summary [{}] (possible thrm/pwr capping)",
                                                summary
                                            ),
                                        );
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Warning: Failed to read v-f info: {}", e);
                                    force_kill_process(&mut process, "v-f check read failure");
                                    break;
                                }
                            }

                            match run_output(gpu, QueryVfpPointVoltage { point: cfg.point }) {
                                Ok(v) => {
                                    // fetch default/current frequencies for the point if available
                                    // and determine the actual VFP point key reported by the GPU.
                                    let mut default_freq_khz: Option<i32> = None;
                                    let mut current_freq_khz: Option<i32> = None;
                                    let mut actual_point: usize = cfg.point;
                                    if let Ok(status) = run_output(gpu, QueryGpuStatus)
                                        && let Some(vfp) = status.vfp
                                    {
                                        // use shared helper to find the closest VFP point by voltage
                                        if let Some((idx, pt)) =
                                            nvoc_core::find_matching_vfp_point(&vfp.graphics, v)
                                        {
                                            actual_point = *idx;
                                            default_freq_khz = Some(pt.default_frequency.0 as i32);
                                            current_freq_khz = Some(pt.frequency.0 as i32);
                                        }
                                    }

                                    // recompute the currently-applied fluctuation delta
                                    let main_delta = cfg.init_core_oc_value + fluctuation_freq;

                                    let to_mhz = |freq_khz: Option<i32>| {
                                        freq_khz
                                            .map(|khz| format!("{:.1}", khz as f64 / 1000.0))
                                            .unwrap_or_else(|| "N/A".to_string())
                                    };

                                    let state_msg = format!(
                                        "State:{} Pt:{} V:{} default:{}MHz current:{}MHz delta:{:+.1}MHz thrm:{}/{}",
                                        state_label,
                                        actual_point,
                                        v,
                                        to_mhz(default_freq_khz),
                                        to_mhz(current_freq_khz),
                                        main_delta as f64 / 1000.0,
                                        thrm_or_pwr_limit_number,
                                        in_test_check_number
                                    );

                                    if let Some(progress) = cfg.progress {
                                        progress.set_status(state_msg);
                                    } else {
                                        progress_print(None, state_msg);
                                    }

                                    // ensure voltage lock is applied as before
                                    run_output(
                                        gpu,
                                        SetVfpVoltageLock {
                                            voltage_target: NvapiLockedVoltageTarget::Voltage(v),
                                            feedback: false,
                                        },
                                    )
                                    .unwrap_or_else(|err| {
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

                    sleep(Duration::from_millis(500));

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

                    #[cfg(windows)]
                    if last_event_poll.elapsed() >= poll_interval {
                        last_event_poll = Instant::now();
                        let now = SystemTime::now();
                        if let Some(current_events) =
                            query_windows_gpu_events(event_window_start, now)
                        {
                            let target_id = cfg.target_gpu_id;
                            let matches_target = |evt: &&WindowsGpuEvent| {
                                evt.gpu_bus_id.is_none_or(|id| id == target_id)
                            };
                            let current_target_count =
                                current_events.iter().filter(|e| matches_target(e)).count();
                            if current_target_count > 0 {
                                let fecs_count = current_events
                                    .iter()
                                    .filter(|e| matches_target(e) && e.is_fecs)
                                    .count();
                                let tdr_count = current_events
                                    .iter()
                                    .filter(|e| matches_target(e) && e.is_tdr)
                                    .count();
                                eprintln!(
                                    "Detected {} GPU event(s) for target GPU during test (FECS: {}, TDR: {}). Killing stressor.",
                                    current_target_count, fecs_count, tdr_count
                                );
                                force_kill_process(&mut process, "GPU event detected during test");
                                exit_code = 1;

                                // Only trigger PnP recovery on critical thresholds
                                if fecs_count > 3 || tdr_count > 6 {
                                    eprintln!(
                                        "Event count exceeds critical threshold — triggering PnP recovery."
                                    );
                                    pnp_recover_gpu(gpu);
                                }
                                break;
                            }
                        }
                    }

                    if test_start_at.elapsed() >= Duration::from_secs(timeout_budget_secs) {
                        progress_print(
                            cfg.progress,
                            format!(
                                "Considering GPU has crashed (timeout: {}s, elapsed: {}s)...",
                                timeout_budget_secs,
                                test_start_at.elapsed().as_secs()
                            ),
                        );
                        force_kill_process(&mut process, "in-test timeout");
                        let _ = retry_operation_with_backoff(
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
                            5,
                            5,
                            cfg.minload_exe,
                            cfg.cuda_device,
                        );
                        break;
                    }
                }

                drop(test_progress);
                for handle in output_threads {
                    let _ = handle.join();
                }

                #[cfg(windows)]
                let windows_event_counts = {
                    let event_window_end = SystemTime::now();
                    query_windows_gpu_events(event_window_start, event_window_end)
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
                        Some(detailed_events) => {
                            let target_id = cfg.target_gpu_id;
                            let matches_target = |evt: &&WindowsGpuEvent| {
                                evt.gpu_bus_id.is_none_or(|id| id == target_id)
                            };

                            let fecs_count = detailed_events
                                .iter()
                                .filter(|e| matches_target(e) && e.is_fecs)
                                .count();
                            let tdr_count = detailed_events
                                .iter()
                                .filter(|e| matches_target(e) && e.is_tdr)
                                .count();

                            if fecs_count > 3 {
                                eprintln!(
                                    "Detected {} FECS exception(s) for target GPU (>3 threshold) during pressure test.",
                                    fecs_count
                                );
                                exit_code = 1;
                            }

                            if tdr_count > 6 {
                                eprintln!(
                                    "Detected {} TDR events for target GPU (>6 threshold) during pressure test.",
                                    tdr_count
                                );
                                exit_code = 1;
                            }

                            // For non-FECS / non-TDR events: differential check against baseline
                            let other_target = detailed_events
                                .iter()
                                .filter(|e| matches_target(e) && !e.is_fecs && !e.is_tdr)
                                .count();
                            let baseline_other = event_baseline
                                .as_ref()
                                .map(|bl| {
                                    bl.iter()
                                        .filter(|e| matches_target(e) && !e.is_fecs && !e.is_tdr)
                                        .count()
                                })
                                .unwrap_or(0);
                            let new_other = other_target.saturating_sub(baseline_other);
                            if new_other > 0 && exit_code == 0 {
                                eprintln!(
                                    "Detected {} new non-critical Windows event(s) for target GPU.",
                                    new_other
                                );
                                exit_code = 1;
                            }

                            // Log summary
                            let total_relevant =
                                detailed_events.iter().filter(|e| matches_target(e)).count();
                            if total_relevant > 0 {
                                eprintln!(
                                    "Event summary for target GPU: {} total, {} FECS, {} TDR, {} other",
                                    total_relevant,
                                    fecs_count,
                                    tdr_count,
                                    total_relevant - fecs_count - tdr_count
                                );
                            }
                        }
                        None => {
                            eprintln!("Warning: Failed to query Windows Event Log for this run.");
                        }
                    }
                }

                #[cfg(not(windows))]
                if let Some(xid_counts) =
                    count_linux_gpu_xid_events_by_time(linux_xid_window_start, SystemTime::now())
                    && !xid_counts.is_empty()
                {
                    let summary = xid_counts
                        .iter()
                        .map(|(xid, count)| format!("Xid {} x{}", xid, count))
                        .collect::<Vec<_>>()
                        .join(", ");
                    eprintln!("Detected NVIDIA Xid event(s) during pressure test: {summary}");
                    exit_code = 1;
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
                    let _ = retry_operation_with_backoff(
                        || apply_autoscan_profile(gpu, matches, 80),
                        "apply_autoscan_profile",
                        5,
                        5,
                        cfg.minload_exe,
                        cfg.cuda_device,
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfp_curve_range_saturates_low_points() {
        assert_eq!(vfp_curve_range(2, 5), 0..=7);
        assert_eq!(vfp_curve_range(0, 5), 0..=5);
    }

    #[test]
    fn lower_vfp_curve_range_skips_empty_low_side() {
        assert_eq!(lower_vfp_curve_range(2, 5), Some(0..=1));
        assert_eq!(lower_vfp_curve_range(5, 0), None);
        assert_eq!(lower_vfp_curve_range(0, 5), None);
    }
}
