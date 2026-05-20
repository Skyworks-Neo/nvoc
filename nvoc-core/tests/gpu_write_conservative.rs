use nvapi_hi::{ClockDomain, KilohertzDelta, PState};
use nvml_wrapper::enum_wrappers::device::PerformanceState;
use nvml_wrapper::enums::device::FanControlPolicy;
use nvoc_core::{
    BackendSet, Error, GpuId, GpuTarget, QueryClockOffset, QueryPowerLimits, QueryPstates,
    ResetApplicationsClocks, ResetFanSpeed, ResetLockedClocks, ResetVfpDeltas,
    ResetVfpFrequencyLock, SetClockOffset, SetFanSpeed, SetLockedClocks, SetPowerLimit,
    SetPstateClockOffset, TargetInventory, VfpResetDomain, discover_targets, parse_nvml_pstate,
    run,
};

const INVALID_GPU_ID: u32 = u32::MAX - 255;

fn require_write_opt_in() {
    assert_eq!(
        std::env::var("NVOC_CORE_GPU_WRITE_TESTS").as_deref(),
        Ok("1"),
        "set NVOC_CORE_GPU_WRITE_TESTS=1 to run conservative GPU write tests"
    );
}

fn inventory() -> TargetInventory {
    require_write_opt_in();
    discover_targets(BackendSet::Both)
        .expect("GPU backends should initialize on the GPU test runner")
}

fn first_target_with_nvml(inventory: &TargetInventory) -> GpuTarget<'_> {
    let targets = inventory.targets();
    let target = targets
        .iter()
        .find(|t| t.nvml.is_some())
        .expect("GPU test runner should expose at least one NVML GPU");
    *target
}

fn first_target_with_nvapi(inventory: &TargetInventory) -> GpuTarget<'_> {
    let targets = inventory.targets();
    let target = targets
        .iter()
        .find(|t| t.nvapi.is_some())
        .expect("GPU test runner should expose at least one NVAPI GPU");
    *target
}

fn assert_invalid_gpu_id_error(result: Result<(), Error>) {
    let err = result.expect_err("invalid GPU id should fail before writing");
    assert!(err.to_string().contains("not found"), "{err}");
}

fn is_permission_denied(msg: &str) -> bool {
    msg.contains("No Permission")
        || msg.contains("Insufficient Permissions")
        || msg.contains("Permission Denied")
        || msg.contains("NVAPI_INVALID_USER_PRIVILEGE")
        || msg.contains("InvalidUserPrivilege")
}

fn assert_not_permission_denied(msg: &str) {
    assert!(!is_permission_denied(msg), "permission denied: {msg}");
}

fn log_cleanup_error(label: &str, err: Error) {
    eprintln!("Warning: cleanup failed for {label}: {err}");
}

fn handle_cleanup_error(label: &str, err: Error, fail_on_permission: bool) {
    let msg = err.to_string();
    if fail_on_permission {
        assert_not_permission_denied(&msg);
    }
    log_cleanup_error(label, err);
}

struct NvmlCleanupGuard<'a> {
    target: GpuTarget<'a>,
    restore_power_limit_w: Option<u32>,
    restore_clock_offsets: Vec<(PerformanceState, Option<i32>, Option<i32>)>,
    cleaned: bool,
}

impl<'a> NvmlCleanupGuard<'a> {
    fn new(target: GpuTarget<'a>) -> Self {
        let power = run(&target, QueryPowerLimits)
            .ok()
            .map(|p| p.output.current_watts.round() as u32)
            .filter(|limit| *limit > 0);
        Self {
            target,
            restore_power_limit_w: power,
            restore_clock_offsets: Vec::new(),
            cleaned: false,
        }
    }

    fn remember_clock_offsets(&mut self, pstate: PerformanceState) {
        let core = run(
            &self.target,
            QueryClockOffset {
                domain: ClockDomain::Graphics,
                pstate,
            },
        )
        .ok()
        .map(|o| o.output.mhz);
        let mem = run(
            &self.target,
            QueryClockOffset {
                domain: ClockDomain::Memory,
                pstate,
            },
        )
        .ok()
        .map(|o| o.output.mhz);
        self.restore_clock_offsets.push((pstate, core, mem));
    }

    fn reset_after_write(&mut self) {
        self.cleanup(true);
    }

    fn cleanup(&mut self, fail_on_permission: bool) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;

        for (pstate, core_offset, mem_offset) in self.restore_clock_offsets.iter().copied() {
            if let Some(offset) = core_offset
                && let Err(err) = run(
                    &self.target,
                    SetClockOffset {
                        domain: ClockDomain::Graphics,
                        pstate,
                        mhz: offset,
                    },
                )
            {
                handle_cleanup_error("NVML core clock offset restore", err, fail_on_permission);
            }
            if let Some(offset) = mem_offset
                && let Err(err) = run(
                    &self.target,
                    SetClockOffset {
                        domain: ClockDomain::Memory,
                        pstate,
                        mhz: offset,
                    },
                )
            {
                handle_cleanup_error("NVML memory clock offset restore", err, fail_on_permission);
            }
        }

        for (label, result) in [
            (
                "NVML application clocks reset",
                run(&self.target, ResetApplicationsClocks).map(|_| ()),
            ),
            (
                "NVML core locked clocks reset",
                run(
                    &self.target,
                    ResetLockedClocks {
                        domain: ClockDomain::Graphics,
                    },
                )
                .map(|_| ()),
            ),
            (
                "NVML memory locked clocks reset",
                run(
                    &self.target,
                    ResetLockedClocks {
                        domain: ClockDomain::Memory,
                    },
                )
                .map(|_| ()),
            ),
        ] {
            if let Err(err) = result {
                handle_cleanup_error(label, err, fail_on_permission);
            }
        }

        if let Some(limit_w) = self.restore_power_limit_w
            && let Err(err) = run(&self.target, SetPowerLimit { watts: limit_w })
        {
            handle_cleanup_error("NVML power limit restore", err, fail_on_permission);
        }
    }
}

impl Drop for NvmlCleanupGuard<'_> {
    fn drop(&mut self) {
        self.cleanup(false);
    }
}

struct NvapiCleanupGuard<'a> {
    target: GpuTarget<'a>,
    cleaned: bool,
}

impl<'a> NvapiCleanupGuard<'a> {
    fn new(target: GpuTarget<'a>) -> Self {
        Self {
            target,
            cleaned: false,
        }
    }

    fn reset_after_write(&mut self) {
        self.cleanup(true);
    }

    fn cleanup(&mut self, fail_on_permission: bool) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;

        for domain in [ClockDomain::Graphics, ClockDomain::Memory] {
            if let Err(err) = run(&self.target, ResetVfpFrequencyLock { domain }) {
                handle_cleanup_error("NVAPI VFP frequency lock reset", err, fail_on_permission);
            }
            if let Err(err) = run(
                &self.target,
                SetPstateClockOffset {
                    pstate: PState::P0,
                    domain,
                    delta: KilohertzDelta(0),
                },
            ) {
                handle_cleanup_error(
                    "NVAPI P0 pstate clock offset reset",
                    err,
                    fail_on_permission,
                );
            }
        }

        if let Err(err) = run(
            &self.target,
            ResetVfpDeltas {
                domain: VfpResetDomain::All,
            },
        ) {
            handle_cleanup_error("NVAPI VFP delta reset", err, fail_on_permission);
        }
    }
}

impl Drop for NvapiCleanupGuard<'_> {
    fn drop(&mut self) {
        self.cleanup(false);
    }
}

#[test]
#[ignore]
fn nvml_fan_level_rejects() {
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
    let result = run(
        &bad_target,
        SetFanSpeed {
            fan_index: 0,
            policy: FanControlPolicy::Manual,
            level: 101,
        },
    );
    let err = result.expect_err("fan levels above 100 should be rejected");
    assert!(err.to_string().contains("Invalid fan level 101"));
}

#[test]
#[ignore]
fn nvml_bad_gpu_rejects() {
    let pstate = parse_nvml_pstate("P0").unwrap();
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };

    assert_invalid_gpu_id_error(run(&bad_target, SetPowerLimit { watts: 1 }).map(|_| ()));
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            SetClockOffset {
                domain: ClockDomain::Graphics,
                pstate,
                mhz: 0,
            },
        )
        .map(|_| ()),
    );
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            SetClockOffset {
                domain: ClockDomain::Memory,
                pstate,
                mhz: 0,
            },
        )
        .map(|_| ()),
    );
    assert_invalid_gpu_id_error(run(&bad_target, ResetFanSpeed { fan_index: 0 }).map(|_| ()));
    assert_invalid_gpu_id_error(run(&bad_target, ResetApplicationsClocks).map(|_| ()));
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            ResetLockedClocks {
                domain: ClockDomain::Graphics,
            },
        )
        .map(|_| ()),
    );
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            ResetLockedClocks {
                domain: ClockDomain::Memory,
            },
        )
        .map(|_| ()),
    );
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            SetLockedClocks {
                domain: ClockDomain::Graphics,
                min_mhz: 1,
                max_mhz: 1,
            },
        )
        .map(|_| ()),
    );
    assert_invalid_gpu_id_error(
        run(
            &bad_target,
            SetLockedClocks {
                domain: ClockDomain::Memory,
                min_mhz: 1,
                max_mhz: 1,
            },
        )
        .map(|_| ()),
    );
}

#[test]
#[ignore]
fn nvml_power_current() {
    let inv = inventory();
    let target = first_target_with_nvml(&inv);
    let mut cleanup = NvmlCleanupGuard::new(target);
    let power = run(&target, QueryPowerLimits)
        .expect("power limits should be readable")
        .output;
    let min_w = power.min_watts;
    let current_w = power.current_watts;
    let max_w = power.max_watts;
    assert!(min_w >= 0.0);
    assert!(max_w >= current_w || max_w == 0.0);

    if current_w.is_finite() && current_w > 0.0 {
        match run(
            &target,
            SetPowerLimit {
                watts: current_w.round() as u32,
            },
        ) {
            Ok(_) => {
                let after = run(&target, QueryPowerLimits)
                    .expect("power should remain readable after current-value write")
                    .output;
                assert!((after.current_watts - current_w).abs() <= 1.0);
            }
            Err(err) => {
                let msg = err.to_string();
                assert_not_permission_denied(&msg);
                assert!(
                    msg.contains("Not Supported") || msg.contains("NVML Set Power Limit Error"),
                    "{msg}"
                );
            }
        }
    }

    cleanup.reset_after_write();
}

#[test]
#[ignore]
fn nvml_offsets_current() {
    let inv = inventory();
    let target = first_target_with_nvml(&inv);
    let mut cleanup = NvmlCleanupGuard::new(target);
    let pstates = run(&target, QueryPstates)
        .expect("pstate info should be readable")
        .output;

    for pstate in pstates.into_iter().take(1) {
        cleanup.remember_clock_offsets(pstate.pstate);

        if let Ok(offset_report) = run(
            &target,
            QueryClockOffset {
                domain: ClockDomain::Graphics,
                pstate: pstate.pstate,
            },
        ) {
            let offset = offset_report.output.mhz;
            match run(
                &target,
                SetClockOffset {
                    domain: ClockDomain::Graphics,
                    pstate: pstate.pstate,
                    mhz: offset,
                },
            ) {
                Ok(_) => assert_eq!(
                    run(
                        &target,
                        QueryClockOffset {
                            domain: ClockDomain::Graphics,
                            pstate: pstate.pstate
                        }
                    )
                    .unwrap()
                    .output
                    .mhz,
                    offset
                ),
                Err(err) => {
                    let msg = err.to_string();
                    assert_not_permission_denied(&msg);
                    assert!(msg.contains("NVML Set Core Clock"), "{msg}");
                }
            }
        }

        if let Ok(offset_report) = run(
            &target,
            QueryClockOffset {
                domain: ClockDomain::Memory,
                pstate: pstate.pstate,
            },
        ) {
            let offset = offset_report.output.mhz;
            match run(
                &target,
                SetClockOffset {
                    domain: ClockDomain::Memory,
                    pstate: pstate.pstate,
                    mhz: offset,
                },
            ) {
                Ok(_) => assert_eq!(
                    run(
                        &target,
                        QueryClockOffset {
                            domain: ClockDomain::Memory,
                            pstate: pstate.pstate
                        }
                    )
                    .unwrap()
                    .output
                    .mhz,
                    offset
                ),
                Err(err) => {
                    let msg = err.to_string();
                    assert_not_permission_denied(&msg);
                    assert!(msg.contains("NVML Set Mem Clock"), "{msg}");
                }
            }
        }
    }

    cleanup.reset_after_write();
}

#[test]
#[ignore]
fn nvml_resets() {
    let inv = inventory();
    let target = first_target_with_nvml(&inv);
    let mut cleanup = NvmlCleanupGuard::new(target);

    for result in [
        run(&target, ResetApplicationsClocks).map(|_| ()),
        run(
            &target,
            ResetLockedClocks {
                domain: ClockDomain::Graphics,
            },
        )
        .map(|_| ()),
        run(
            &target,
            ResetLockedClocks {
                domain: ClockDomain::Memory,
            },
        )
        .map(|_| ()),
    ] {
        if let Err(err) = result {
            let msg = err.to_string();
            assert_not_permission_denied(&msg);
            assert!(
                msg.contains("Not Supported") || msg.contains("NVML Reset"),
                "{msg}"
            );
        }
    }

    cleanup.reset_after_write();
}

#[test]
#[ignore]
fn nvapi_lock_resets() {
    let inv = inventory();
    let target = first_target_with_nvapi(&inv);
    let mut cleanup = NvapiCleanupGuard::new(target);

    for domain in [ClockDomain::Graphics, ClockDomain::Memory] {
        match run(&target, ResetVfpFrequencyLock { domain }) {
            Ok(_) => {}
            Err(Error::VfpUnsupported) | Err(Error::FeatureUnsupportedErr) => {}
            Err(err) if matches!(err, Error::Nvapi(_)) => {
                let msg = err.to_string();
                assert_not_permission_denied(&msg);
            }
            Err(err) => panic!("unexpected NVAPI VFP lock reset error: {err}"),
        }
    }

    cleanup.reset_after_write();
}

#[test]
#[ignore]
fn nvapi_vfp_delta_reset() {
    let inv = inventory();
    let target = first_target_with_nvapi(&inv);
    let mut cleanup = NvapiCleanupGuard::new(target);
    match run(
        &target,
        ResetVfpDeltas {
            domain: VfpResetDomain::All,
        },
    ) {
        Ok(_) => {}
        Err(err) => {
            let msg = err.to_string();
            assert_not_permission_denied(&msg);
            assert!(
                msg.contains("VFP unsupported")
                    || msg.contains("Feature unsupported")
                    || msg.contains("NVAPI error"),
                "{msg}"
            );
        }
    }

    cleanup.reset_after_write();
}

#[test]
#[ignore]
fn nvapi_pstate_zero_delta() {
    let inv = inventory();
    let target = first_target_with_nvapi(&inv);
    let mut cleanup = NvapiCleanupGuard::new(target);
    for (pstate, domain) in [
        (PState::P0, ClockDomain::Graphics),
        (PState::P0, ClockDomain::Memory),
    ] {
        match run(
            &target,
            SetPstateClockOffset {
                pstate,
                domain,
                delta: KilohertzDelta(0),
            },
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                assert_not_permission_denied(&msg);
                assert!(
                    msg.contains("not found")
                        || msg.contains("not editable")
                        || msg.contains("no editable clock entries")
                        || msg.contains("NVAPI error"),
                    "{msg}"
                );
            }
        }
    }

    cleanup.reset_after_write();
}
