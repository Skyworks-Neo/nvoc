use nvapi_hi::{ClockDomain, KilohertzDelta, PState};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::PerformanceState;
use nvml_wrapper::enums::device::FanControlPolicy;
use nvoc_core::{
    Error, GpuSelector, VfpResetDomain, get_nvml_core_clock_vf_offset,
    get_nvml_mem_clock_vf_offset, get_nvml_pstate_info, get_sorted_gpu_ids_nvml, get_sorted_gpus,
    parse_nvml_pstate, query_nvml_power_watts, reset_nvml_applications_clocks,
    reset_nvml_core_locked_clocks, reset_nvml_mem_locked_clocks, reset_vfp_deltas,
    reset_vfp_frequency_lock, select_gpus, set_default_fan_speed, set_fan_speed,
    set_nvml_core_clock_vf_offset, set_nvml_core_locked_clocks, set_nvml_mem_clock_vf_offset,
    set_nvml_mem_locked_clocks, set_nvml_power_limit, set_pstate_clock_offset_preserve,
};

const INVALID_GPU_ID: u32 = u32::MAX - 255;

fn require_write_opt_in() {
    assert_eq!(
        std::env::var("NVOC_CORE_GPU_WRITE_TESTS").as_deref(),
        Ok("1"),
        "set NVOC_CORE_GPU_WRITE_TESTS=1 to run conservative GPU write tests"
    );
}

fn nvml() -> Nvml {
    require_write_opt_in();
    Nvml::init().expect("NVML should initialize on the GPU test runner")
}

fn first_gpu_id_nvml(nvml: &Nvml) -> u32 {
    let ids = get_sorted_gpu_ids_nvml(nvml).expect("NVML should enumerate GPU ids");
    assert!(
        !ids.is_empty(),
        "GPU test runner should expose at least one NVML GPU"
    );
    ids[0]
}

fn first_gpu() -> nvapi_hi::Gpu {
    require_write_opt_in();
    let gpus = get_sorted_gpus().expect("NVAPI should enumerate GPUs on the GPU test runner");
    let selected = select_gpus(&gpus, &GpuSelector::from_specs(["0".to_string()]))
        .expect("first GPU should be selectable");
    assert_eq!(selected.len(), 1);
    gpus.into_iter().next().unwrap()
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

fn log_cleanup_error(label: &str, err: impl std::fmt::Display) {
    eprintln!("Warning: cleanup failed for {label}: {err}");
}

fn handle_cleanup_error(label: &str, err: impl std::fmt::Display, fail_on_permission: bool) {
    let msg = err.to_string();
    if fail_on_permission {
        assert_not_permission_denied(&msg);
    }
    log_cleanup_error(label, msg);
}

struct NvmlCleanupGuard<'a> {
    nvml: &'a Nvml,
    gpu_id: u32,
    restore_power_limit_w: Option<u32>,
    restore_clock_offsets: Vec<(PerformanceState, Option<i32>, Option<i32>)>,
    cleaned: bool,
}

impl<'a> NvmlCleanupGuard<'a> {
    fn new(nvml: &'a Nvml, gpu_id: u32) -> Self {
        Self {
            nvml,
            gpu_id,
            restore_power_limit_w: query_nvml_power_watts(nvml, gpu_id)
                .map(|(_, current_w, _)| current_w.round() as u32)
                .filter(|limit| *limit > 0),
            restore_clock_offsets: Vec::new(),
            cleaned: false,
        }
    }

    fn remember_clock_offsets(&mut self, pstate: PerformanceState) {
        self.restore_clock_offsets.push((
            pstate,
            get_nvml_core_clock_vf_offset(self.nvml, self.gpu_id, pstate),
            get_nvml_mem_clock_vf_offset(self.nvml, self.gpu_id, pstate),
        ));
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
                && let Err(err) =
                    set_nvml_core_clock_vf_offset(self.nvml, self.gpu_id, offset, pstate)
            {
                handle_cleanup_error("NVML core clock offset restore", err, fail_on_permission);
            }
            if let Some(offset) = mem_offset
                && let Err(err) =
                    set_nvml_mem_clock_vf_offset(self.nvml, self.gpu_id, offset, pstate)
            {
                handle_cleanup_error("NVML memory clock offset restore", err, fail_on_permission);
            }
        }

        for (label, result) in [
            (
                "NVML application clocks reset",
                reset_nvml_applications_clocks(self.nvml, self.gpu_id),
            ),
            (
                "NVML core locked clocks reset",
                reset_nvml_core_locked_clocks(self.nvml, self.gpu_id),
            ),
            (
                "NVML memory locked clocks reset",
                reset_nvml_mem_locked_clocks(self.nvml, self.gpu_id),
            ),
        ] {
            if let Err(err) = result {
                handle_cleanup_error(label, err, fail_on_permission);
            }
        }

        if let Some(limit_w) = self.restore_power_limit_w
            && let Err(err) = set_nvml_power_limit(self.nvml, self.gpu_id, limit_w)
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
    gpu: &'a nvapi_hi::Gpu,
    cleaned: bool,
}

impl<'a> NvapiCleanupGuard<'a> {
    fn new(gpu: &'a nvapi_hi::Gpu) -> Self {
        Self {
            gpu,
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
            if let Err(err) = reset_vfp_frequency_lock(self.gpu, domain) {
                handle_cleanup_error("NVAPI VFP frequency lock reset", err, fail_on_permission);
            }
            if let Err(err) =
                set_pstate_clock_offset_preserve(self.gpu, PState::P0, domain, KilohertzDelta(0))
            {
                handle_cleanup_error(
                    "NVAPI P0 pstate clock offset reset",
                    err,
                    fail_on_permission,
                );
            }
        }

        if let Err(err) = reset_vfp_deltas(self.gpu, VfpResetDomain::All) {
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
    let nvml = nvml();
    let result = set_fan_speed(&nvml, INVALID_GPU_ID, 0, FanControlPolicy::Manual, 101);
    let err = result.expect_err("fan levels above 100 should be rejected");
    assert!(err.to_string().contains("Invalid fan level 101"));
}

#[test]
#[ignore]
fn nvml_bad_gpu_rejects() {
    let nvml = nvml();
    let pstate = parse_nvml_pstate("P0");

    assert_invalid_gpu_id_error(set_nvml_power_limit(&nvml, INVALID_GPU_ID, 1));
    assert_invalid_gpu_id_error(set_nvml_core_clock_vf_offset(
        &nvml,
        INVALID_GPU_ID,
        0,
        pstate,
    ));
    assert_invalid_gpu_id_error(set_nvml_mem_clock_vf_offset(
        &nvml,
        INVALID_GPU_ID,
        0,
        pstate,
    ));
    assert_invalid_gpu_id_error(set_default_fan_speed(&nvml, INVALID_GPU_ID, 0));
    assert_invalid_gpu_id_error(reset_nvml_applications_clocks(&nvml, INVALID_GPU_ID));
    assert_invalid_gpu_id_error(reset_nvml_core_locked_clocks(&nvml, INVALID_GPU_ID));
    assert_invalid_gpu_id_error(reset_nvml_mem_locked_clocks(&nvml, INVALID_GPU_ID));
    assert_invalid_gpu_id_error(set_nvml_core_locked_clocks(&nvml, INVALID_GPU_ID, 1, 1));
    assert_invalid_gpu_id_error(set_nvml_mem_locked_clocks(&nvml, INVALID_GPU_ID, 1, 1));
}

#[test]
#[ignore]
fn nvml_power_current() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    let mut cleanup = NvmlCleanupGuard::new(&nvml, gpu_id);
    let Some((min_w, current_w, max_w)) = query_nvml_power_watts(&nvml, gpu_id) else {
        return;
    };
    assert!(min_w >= 0.0);
    assert!(max_w >= current_w || max_w == 0.0);

    if current_w.is_finite() && current_w > 0.0 {
        match set_nvml_power_limit(&nvml, gpu_id, current_w.round() as u32) {
            Ok(()) => {
                let (_, after_w, _) = query_nvml_power_watts(&nvml, gpu_id)
                    .expect("power should remain readable after current-value write");
                assert!((after_w - current_w).abs() <= 1.0);
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
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    let mut cleanup = NvmlCleanupGuard::new(&nvml, gpu_id);
    let Some(pstates) = get_nvml_pstate_info(&nvml, gpu_id) else {
        return;
    };

    for (pstate, _, _, _, _) in pstates.into_iter().take(1) {
        cleanup.remember_clock_offsets(pstate);

        if let Some(offset) = get_nvml_core_clock_vf_offset(&nvml, gpu_id, pstate) {
            match set_nvml_core_clock_vf_offset(&nvml, gpu_id, offset, pstate) {
                Ok(()) => assert_eq!(
                    get_nvml_core_clock_vf_offset(&nvml, gpu_id, pstate),
                    Some(offset)
                ),
                Err(err) => {
                    let msg = err.to_string();
                    assert_not_permission_denied(&msg);
                    assert!(msg.contains("NVML Set Core Clock"), "{msg}");
                }
            }
        }

        if let Some(offset) = get_nvml_mem_clock_vf_offset(&nvml, gpu_id, pstate) {
            match set_nvml_mem_clock_vf_offset(&nvml, gpu_id, offset, pstate) {
                Ok(()) => assert_eq!(
                    get_nvml_mem_clock_vf_offset(&nvml, gpu_id, pstate),
                    Some(offset)
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
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    let mut cleanup = NvmlCleanupGuard::new(&nvml, gpu_id);

    for result in [
        reset_nvml_applications_clocks(&nvml, gpu_id),
        reset_nvml_core_locked_clocks(&nvml, gpu_id),
        reset_nvml_mem_locked_clocks(&nvml, gpu_id),
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
    let gpu = first_gpu();
    let mut cleanup = NvapiCleanupGuard::new(&gpu);

    for domain in [ClockDomain::Graphics, ClockDomain::Memory] {
        match reset_vfp_frequency_lock(&gpu, domain) {
            Ok(()) => {}
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
    let gpu = first_gpu();
    let mut cleanup = NvapiCleanupGuard::new(&gpu);
    match reset_vfp_deltas(&gpu, VfpResetDomain::All) {
        Ok(()) => {}
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
    let gpu = first_gpu();
    let mut cleanup = NvapiCleanupGuard::new(&gpu);
    for (pstate, domain) in [
        (PState::P0, ClockDomain::Graphics),
        (PState::P0, ClockDomain::Memory),
    ] {
        match set_pstate_clock_offset_preserve(&gpu, pstate, domain, KilohertzDelta(0)) {
            Ok(()) => {}
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
