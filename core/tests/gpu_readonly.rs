//! Read-only GPU integration tests.
//!
//! Two families of tests live here:
//!
//! 1. **Invariants** (`discovery_*`, `selection_*`, `nvml_*`, `nvapi_*` that assert) —
//!    verify that discovery/selection/clock/voltage/fan offsets behave correctly against a
//!    real GPU. These are `#[ignore]`d because they need hardware; run with
//!    `cargo test -p nvoc-core -- --ignored`.
//!
//! 2. **Raw probes** (`nvapi_raw_payload_probe` and anything documented under
//!    "Investigating unknown NVAPI IDs" below) — diagnostic harnesses that **bypass the
//!    `RawConversion` layer** to dump the raw byte payload an NVAPI call returns. They make
//!    no assertions about values; they exist to reverse-engineer undocumented/unknown
//!    NVAPI QueryInterface IDs. See the workflow section at the bottom of this comment.
//!
//! # Ground truth
//!
//! Assertion tests compare against an optional ground-truth file pointed to by
//! `NVOC_CORE_GPU_GROUND_TRUTH` (a JSON doc with `gpus[].id` and per-field bounds). When
//! absent, bound-checks silently no-op (see `assert_optional_min`/`assert_optional_max`).
//!
//! # Investigating unknown NVAPI IDs (the raw-probe workflow)
//!
//! When nvapi-rs lists an ID as `Unknown_XXXXXXXX` in `nvapi-rs/sys/src/nvid.rs`, the goal
//! is to decide (a) what it returns, (b) whether its bytes are *live* (change under load)
//! or a static *descriptor/blob*, and (c) whether wrapping it adds monitoring value. The
//! static answer comes from IDA (see `docs/gpuz-per-rail-investigation.md`); the dynamic
//! confirmation comes from this file's `nvapi_raw_payload_probe`.
//!
//! ## Why bypass `RawConversion`?
//!
//! The op/hi layers (`QueryGpuStatus`, `nvapi_hi::GpuStatus`) call `RawConversion::convert_raw`,
//! which is *lossy by design*: it validates padding fields and returns
//! `Err(ArgumentRangeError)` (or `allowable_result` downgrades it to `None`) when padding is
//! non-zero or an enum discriminant is out of range. That is correct for production reads,
//! but it **hides unknown bytes** — exactly the data an RE probe needs to see. The raw probe
//! calls the `sys::api::*` FFI directly with a zeroed struct and inspects every byte.
//!
//! ## The probe pattern (copy this for a new ID)
//!
//! For an ID that *is* wrapped in nvapi-rs (struct + FFI symbol exist), stamp the version
//! magic and call the raw FFI:
//! ```ignore
//! use nvapi_hi::sys::gpu::power::private as pw;
//! use nvapi_hi::sys::nvapi::{NvVersion, VersionedStruct};
//! use nvapi_hi::sys::{api, Status};
//!
//! // versioned() is ambiguous (struct impls both StructVersion and StructVersion<1>);
//! // use this macro instead to zero + stamp the v1 magic.
//! macro_rules! ver {
//!     ($ty:ty) => {{
//!         let mut s = unsafe { std::mem::zeroed::<$ty>() };
//!         *s.nvapi_version_mut() = NvVersion::with_struct::<$ty>(1);
//!         s
//!     }};
//! }
//!
//! let mut s = ver!(pw::SOME_STATUS_STRUCT);
//! let st = api::NvAPI_GPU_SomeGetStatus(handle, &mut s);
//! eprintln!("status={:?}", st);
//! if (st as i32) == (Status::Ok as i32) {
//!     // dump named fields + a raw hex view of the whole struct
//!     let bytes: &[u8] = std::slice::from_raw_parts(&s as *const _ as *const u8, std::mem::size_of_val(&s));
//!     /* walk bytes in 16-byte rows, print non-zero rows */
//! }
//! ```
//!
//! For an ID that is *not* wrapped (no struct/symbol), call it raw via
//! `nvapi_QueryInterface` with a scratch buffer, trying candidate sizes until one returns
//! `Ok` (see the GetPowerMizerInfo probe below for the full template):
//! ```ignore
//! use nvapi_hi::sys::nvapi_QueryInterface;
//! const ID: u32 = 0xXXXXXXXX;
//! #[repr(C)] struct Scratch { version: u32, data: [u32; 63] }
//! let mut s = Scratch { version: 0, data: [0; 63] };
//! for sz in [256, 64, 128] {
//!     s.version = sz | (1 << 16);                 // version magic = (v1<<16)|size
//!     s.data = [0; 63];
//!     let ptr = nvapi_QueryInterface(ID)? as *const ();
//!     let func: unsafe extern "system" fn(NvPhysicalGpuHandle, *mut Scratch) -> Status =
//!         std::mem::transmute(ptr);
//!     let st = func(handle, &mut s);
//!     if (st as i32) == (Status::Ok as i32) { /* inspect s.data */ break; }
//! }
//! ```
//!
//! ## Version magic
//!
//! NVAPI structs' first `u32` encodes `(version << 16) | struct_size`. `NvVersion::with_struct::<T>(v)`
//! computes it from the Rust type's size. For raw scratch probes where the size is unknown,
//! iterate candidate sizes (the driver accepts the call only when the magic's size matches
//! what it expects, else it returns `-9 INCOMPATIBLE_STRUCT`). IDA's handler analysis gives
//! the exact accepted magics (e.g. `65608` = v1|sz72 for the power family, `65596` = v1|sz60
//! for thermal) — prefer those over blind guessing.
//!
//! ## Deciding live-vs-descriptor (the decisive test)
//!
//! A returning `Ok` with non-zero bytes does **not** mean the value is a live sensor read.
//! To distinguish a live read from a static blob, run the probe twice under different GPU
//! load (idle vs a stressor) and compare the bytes:
//! - **Bytes change with load** → live read candidate (worth wrapping as a status field).
//! - **Bytes identical across reads AND under load** → static descriptor/capability/blob,
//!   not a status (do not wrap). See the `Unknown_7457CAB5` finding: it returns a
//!   deterministic 32-byte payload that never changes under load — a capability blob, not
//!   the per-rail watts it structurally resembled.
//!
//! ## Privilege
//!
//! Some IDs route through the privileged `\\.\NvAdminDevice` RM path and return
//! `NVAPI_INVALID_USER_PRIVILEGE` without elevation. If a probe fails that way, re-run as
//! administrator. (Note: elevation does not turn a static blob into a live read — it only
//! unlocks the call.)
//!
//! ## What this concluded for the per-rail-watts investigation
//!
//! Every power/voltage-tagged ID was probed on the dev laptop; none return live per-rail
//! watts. GPU-Z's per-rail watts come from a WinRing0 kernel driver doing direct PCI/MMIO,
//! entirely outside NVAPI. Full write-up + the IDA findings that classify each unknown ID:
//! `docs/gpuz-per-rail-investigation.md`. The per-ID RE records live as doc-comments on the
//! `Unknown_*` variants in `nvapi-rs/sys/src/nvid.rs`.

use nvapi_hi::Microvolts;
use nvml_wrapper::Nvml;
use nvoc_core::{
    BackendSet, CheckVoltageFrequency, ClockDomain, Error, GpuId, GpuSelector, GpuTarget,
    QueryClockOffset, QueryFanInfo, QueryGpuInfo, QueryGpuStatus, QueryPowerLimits, QueryPstates,
    QuerySupportedApplicationsClocks, QueryTdpTempLimits, QueryTemperatureThresholds,
    QueryVfpPointVoltage, TargetInventory, discover_targets, nvml_pstate_to_index,
    nvml_pstate_to_str, parse_nvml_pstate, run, select_targets,
};
use serde_json::Value;
use std::env;
use std::fs;

const INVALID_GPU_ID: u32 = u32::MAX - 255;

fn ground_truth() -> Option<Value> {
    let path = env::var("NVOC_CORE_GPU_GROUND_TRUTH").ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn truth_for_gpu(gpu_id: u32) -> Option<Value> {
    ground_truth()?
        .get("gpus")?
        .as_array()?
        .iter()
        .find(|gpu| gpu.get("id").and_then(Value::as_u64) == Some(gpu_id as u64))
        .cloned()
}

fn inventory() -> TargetInventory {
    discover_targets(BackendSet::Both).expect("GPU backends should initialize on the GPU CI runner")
}

fn first_target(inventory: &TargetInventory) -> GpuTarget<'_> {
    let targets = inventory.targets();
    assert!(
        !targets.is_empty(),
        "GPU CI runner should expose at least one GPU"
    );
    *targets.iter().find(|t| t.has_nvml()).unwrap_or(&targets[0])
}

fn nvml(inventory: &TargetInventory) -> &Nvml {
    let targets = inventory.targets();
    targets
        .iter()
        .find(|t| t.has_nvml())
        .expect("at least one NVML backend should be present")
        .nvml()
        .unwrap()
}

fn assert_sorted_unique<T: Ord + Copy + std::fmt::Debug>(values: &[T]) {
    for pair in values.windows(2) {
        assert!(
            pair[0] < pair[1],
            "values should be sorted and unique: {values:?}"
        );
    }
}

fn assert_optional_min(value: Option<&Value>, actual: f32) {
    if let Some(expected) = value.and_then(Value::as_f64) {
        assert!(
            actual as f64 >= expected,
            "{actual} is below expected minimum {expected}"
        );
    }
}

fn assert_optional_max(value: Option<&Value>, actual: f32) {
    if let Some(expected) = value.and_then(Value::as_f64) {
        assert!(
            actual as f64 <= expected,
            "{actual} is above expected maximum {expected}"
        );
    }
}

#[test]
#[ignore]
fn discovery_nvapi_sorted() {
    let inv = inventory();
    let targets = inv.targets();
    let ids = targets.iter().map(|t| t.id.0).collect::<Vec<_>>();
    assert_sorted_unique(&ids);

    for target in &targets {
        if !target.has_nvapi() {
            continue;
        }
        let info = run(target, QueryGpuInfo)
            .expect("GPU info should be readable")
            .output;
        assert_eq!(info.id as u32, target.id.0);
        assert!(!info.name.trim().is_empty());

        if let Some(truth) = truth_for_gpu(info.id as u32)
            && let Some(expected) = truth.get("name_contains").and_then(Value::as_str)
        {
            assert!(
                info.name.contains(expected),
                "{} should contain {expected}",
                info.name
            );
        }
    }
}

#[test]
#[ignore]
fn discovery_nvml_ids() {
    let inv = inventory();
    let targets = inv.targets();
    assert!(!targets.is_empty());
    let ids = targets.iter().map(|t| t.id.0).collect::<Vec<_>>();
    assert_sorted_unique(&ids);

    for id in ids {
        assert_eq!(id % 256, 0, "NVML ids should use NVAPI PCI bus encoding");
        assert_eq!(GpuId(id).pci_bus().saturating_mul(256), id);
        if let Some(truth) = truth_for_gpu(id)
            && let Some(bus) = truth.get("pci_bus").and_then(Value::as_u64)
        {
            assert_eq!(id / 256, bus as u32);
        }
    }
}

#[test]
#[ignore]
fn discovery_nvml_device_id_conversion() {
    let inv = inventory();
    let targets = inv.targets();
    assert!(!targets.is_empty());
    let nvml = nvml(&inv);
    let device = nvml
        .device_by_index(0)
        .expect("first NVML device should be readable");
    assert_eq!(
        nvoc_core::gpu_id_from_nvml_device(&device).unwrap().0,
        targets[0].id.0
    );
}

#[test]
#[ignore]
fn selection_nvapi() {
    let inv = inventory();
    let targets = inv.targets();
    let nvapi_targets: Vec<GpuTarget<'_>> = targets.into_iter().filter(|t| t.has_nvapi()).collect();
    let selected = select_targets(&nvapi_targets, &GpuSelector::all()).unwrap();
    assert_eq!(selected.len(), nvapi_targets.len());

    let by_index =
        select_targets(&nvapi_targets, &GpuSelector::from_specs(["0".to_string()])).unwrap();
    assert_eq!(by_index[0].id.0, nvapi_targets[0].id.0);

    let by_id = select_targets(
        &nvapi_targets,
        &GpuSelector::from_specs([nvapi_targets[0].id.0.to_string()]),
    )
    .unwrap();
    assert_eq!(by_id[0].id.0, nvapi_targets[0].id.0);

    let err = match select_targets(
        &nvapi_targets,
        &GpuSelector::from_specs(["999999".to_string()]),
    ) {
        Ok(_) => panic!("invalid GPU selector should fail"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("no GPU matches --gpu"));
    assert!(select_targets(&[], &GpuSelector::all()).is_err());
}

#[test]
#[ignore]
fn selection_nvml_ids() {
    let inv = inventory();
    let targets = inv.targets();
    let ids = targets.iter().map(|t| t.id.0).collect::<Vec<_>>();
    let all = select_targets(&targets, &GpuSelector::all()).unwrap();
    assert_eq!(all.iter().map(|t| t.id.0).collect::<Vec<_>>(), ids);
    assert_eq!(
        select_targets(&targets, &GpuSelector::from_specs(["0".to_string()]))
            .unwrap()
            .iter()
            .map(|t| t.id.0)
            .collect::<Vec<_>>(),
        vec![ids[0]]
    );
    assert!(select_targets(&targets, &GpuSelector::from_specs(["999999".to_string()])).is_err());
}

#[test]
#[ignore]
fn nvml_power_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let gpu_id = target.id.0;
    let power = run(&target, QueryPowerLimits)
        .expect("power limits should be readable")
        .output;
    assert!(power.min_watts >= 0.0);
    assert!(power.current_watts >= power.min_watts || power.min_watts == 0.0);
    assert!(power.max_watts >= power.current_watts || power.max_watts == 0.0);

    if let Some(truth) = truth_for_gpu(gpu_id)
        && let Some(power_truth) = truth.pointer("/nvml/power_watts")
    {
        assert_optional_min(power_truth.get("min"), power.min_watts);
        assert_optional_min(power_truth.get("current_min"), power.current_watts);
        assert_optional_max(power_truth.get("current_max"), power.current_watts);
        assert_optional_max(power_truth.get("max"), power.max_watts);
    }
}

#[test]
#[ignore]
fn nvml_power_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    assert!(run(&bad_target, QueryPowerLimits).is_err());
    assert!(GpuId::from_pci_str("invalid-pci-id").is_err());
}

#[test]
#[ignore]
fn nvml_offsets_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let pstates = run(&target, QueryPstates)
        .expect("pstate info should be readable")
        .output;
    for pstate in &pstates {
        if let Ok(report) = run(
            &target,
            QueryClockOffset {
                domain: ClockDomain::Graphics,
                pstate: pstate.pstate,
            },
        ) {
            assert!(report.output.mhz.abs() < 2_000);
        }
        if let Ok(report) = run(
            &target,
            QueryClockOffset {
                domain: ClockDomain::Memory,
                pstate: pstate.pstate,
            },
        ) {
            assert!(report.output.mhz.abs() < 10_000);
        }
    }
}

#[test]
#[ignore]
fn nvml_offsets_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    let pstate = parse_nvml_pstate("P0").unwrap();
    assert!(
        run(
            &bad_target,
            QueryClockOffset {
                domain: ClockDomain::Graphics,
                pstate
            }
        )
        .is_err()
    );
    assert!(
        run(
            &bad_target,
            QueryClockOffset {
                domain: ClockDomain::Memory,
                pstate
            }
        )
        .is_err()
    );
}

#[test]
#[ignore]
fn nvml_temp_thresholds_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let thresholds = run(&target, QueryTemperatureThresholds)
        .expect("temperature thresholds should be readable")
        .output;
    assert_eq!(thresholds.len(), 8);
    for threshold in &thresholds {
        if let Some(celsius) = threshold.celsius {
            assert!(celsius <= 130 || celsius == u32::MAX);
        }
    }
}

#[test]
#[ignore]
fn nvml_temp_thresholds_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    assert!(run(&bad_target, QueryTemperatureThresholds).is_err());
}

#[test]
#[ignore]
fn nvml_pstates_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let gpu_id = target.id.0;
    let pstates = run(&target, QueryPstates)
        .expect("pstate info should be readable")
        .output;
    assert!(!pstates.is_empty());
    for pstate in &pstates {
        assert!(pstate.min_core_mhz <= pstate.max_core_mhz);
        assert!(pstate.min_memory_mhz <= pstate.max_memory_mhz);
        assert!(nvml_pstate_to_index(pstate.pstate).is_ok());
    }

    if let Some(truth) = truth_for_gpu(gpu_id)
        && let Some(expected) = truth.pointer("/nvml/pstates").and_then(Value::as_array)
    {
        let actual = pstates
            .iter()
            .map(|p| nvml_pstate_to_str(p.pstate))
            .collect::<Vec<_>>();
        for expected in expected.iter().filter_map(Value::as_str) {
            assert!(actual.contains(&expected));
        }
    }
}

#[test]
#[ignore]
fn nvml_pstates_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    assert!(run(&bad_target, QueryPstates).is_err());
}

#[test]
#[ignore]
fn nvml_app_clocks_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let clocks = run(&target, QuerySupportedApplicationsClocks)
        .expect("application clocks should be readable")
        .output;
    for clock in &clocks {
        assert!(clock.memory_mhz > 0);
        for graphics_mhz in &clock.graphics_mhz {
            assert!(*graphics_mhz > 0);
        }
    }
}

#[test]
#[ignore]
fn nvml_app_clocks_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    assert!(run(&bad_target, QuerySupportedApplicationsClocks).is_err());
}

#[test]
#[ignore]
fn nvml_fans_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let gpu_id = target.id.0;
    let fan_info = run(&target, QueryFanInfo)
        .expect("fan info should be readable")
        .output;
    if let Some(min) = fan_info.min_speed
        && let Some(max) = fan_info.max_speed
    {
        assert!(min <= max);
        assert!(max <= 100);
    }

    if let Some(truth) = truth_for_gpu(gpu_id)
        && let Some(expected) = truth.pointer("/nvml/fan_count").and_then(Value::as_u64)
    {
        assert_eq!(fan_info.count as u64, expected);
    }
}

#[test]
#[ignore]
fn nvml_fans_bad_gpu() {
    let bad_target = GpuTarget::without_backends(GpuId(INVALID_GPU_ID), 0);
    assert!(run(&bad_target, QueryFanInfo).is_err());
}

#[test]
#[ignore]
fn nvapi_voltage_point_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        return;
    }
    let status = run(&target, QueryGpuStatus)
        .expect("GPU status should be readable")
        .output;
    let Some(vfp) = status.vfp else {
        assert!(matches!(
            run(&target, QueryVfpPointVoltage { point: 0 }),
            Err(Error::VfpUnsupported)
        ));
        return;
    };
    let (point, expected) = vfp
        .graphics
        .iter()
        .find(|(_, point)| (500_000..=2_000_000).contains(&point.voltage.0))
        .or_else(|| vfp.graphics.iter().next())
        .expect("VFP table should not be empty");
    let voltage: Microvolts = run(&target, QueryVfpPointVoltage { point: *point })
        .expect("VFP point voltage should be readable")
        .output;
    assert_eq!(voltage, expected.voltage);
    if voltage.0 != 0 {
        assert!(voltage.0 <= 2_000_000);
    }
}

#[test]
#[ignore]
fn nvapi_voltage_point_bad_point() {
    let inv = inventory();
    let target = first_target(&inv);
    assert!(run(&target, QueryVfpPointVoltage { point: usize::MAX }).is_err());
}

#[test]
#[ignore]
fn nvapi_tdp_temp_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let result = run(&target, QueryTdpTempLimits);
    match result {
        Ok(report) => {
            let limits = report.output;
            assert!(limits.min_tdp.0 <= limits.max_tdp.0);
            assert!(limits.default_tdp.0 >= limits.min_tdp.0 || limits.default_tdp.0 == 8191);
            assert!(limits.min_temp.0 <= limits.max_temp.0);
            assert!(limits.default_temp.0 >= limits.min_temp.0 || limits.default_temp.0 == 511);
            assert!(!limits.throttle_curve.points.is_empty());
        }
        Err(Error::FeatureUnsupportedErr | Error::VfpUnsupported) => {}
        Err(e) => panic!("unexpected read-only TDP/temp error: {e}"),
    }
}

#[test]
#[ignore]
fn nvapi_tdp_temp_no_nvapi() {
    let bad_target = GpuTarget::without_backends(GpuId(0), 0);
    assert!(run(&bad_target, QueryTdpTempLimits).is_err());
}

#[test]
#[ignore]
fn nvapi_vf_check_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        return;
    }
    let status = run(&target, QueryGpuStatus)
        .expect("GPU status should be readable")
        .output;
    let Some(vfp) = status.vfp else {
        assert!(matches!(
            run(&target, CheckVoltageFrequency { point: 0 }),
            Err(Error::VfpUnsupported)
        ));
        return;
    };
    let point = *vfp
        .graphics
        .keys()
        .next()
        .expect("VFP table should not be empty");
    match run(&target, CheckVoltageFrequency { point }) {
        Ok(report) => {
            assert!(
                report.output.matched_point.is_some(),
                "matched VFP point should be preserved"
            );
        }
        Err(Error::VfpUnsupported) => {}
        Err(e) => panic!("unexpected read-only voltage/frequency error: {e}"),
    }
}

#[test]
#[ignore]
fn nvapi_vf_check_bad_point() {
    let inv = inventory();
    let target = first_target(&inv);
    assert!(run(&target, CheckVoltageFrequency { point: usize::MAX }).is_err());
}

/// Byte-level probe of the raw driver payloads for the GPU-Z per-rail
/// investigation. Bypasses `RawConversion` (which silently drops data when
/// `Padding` fields are non-zero — the suspected "data under-used" mechanism)
/// and Debug-prints the full raw structs so we can see exactly which bytes the
/// driver fills. Run with:
///   cargo test -p nvoc-core -- --ignored --nocapture nvapi_raw_payload_probe
///
/// Compare the printed non-zero padding bytes against GPU-Z's
/// Board/Chip/MVDDC/PWR_SRC/16-Pin readings to recover field semantics.
#[test]
#[ignore]
/// Raw payload probe for undocumented/under-documented NVAPI power/voltage IDs.
///
/// This is a **diagnostic harness**, not an assertion test. It bypasses the lossy
/// `RawConversion` layer (which drops/hides unknown bytes via padding checks) and calls
/// the `sys::api::*` FFI directly with zeroed versioned structs, dumping the returned
/// bytes for human inspection. See the module docs ("Investigating unknown NVAPI IDs")
/// for the full workflow, the probe copy-template, and the live-vs-descriptor decision
/// test.
///
/// Run: `cargo test -p nvoc-core -- --ignored --nocapture nvapi_raw_payload_probe`
///
/// What each numbered block probes (see inline comments for findings):
///  1. `ClientVoltRailsGetStatus` — voltage only; checks if multi-rail volts hide in padding.
///  2. `ClientPowerTopologyGetInfo/Status` — power channel topology; Status returns -5 on
///     laptops (empty internal topology table).
///  3. `PerfPoliciesGetStatus` — 1360-byte struct, full hex dump to find live power/thermal
///     hiding in padding.
///  4. `GetVoltages` (NV_VOLT_TABLE) — Maxwell multi-domain voltage table.
///  5. `ClientPowerPoliciesGetInfo/Status` — V1 vs V2 version-magic probing; V1 layout
///     read back from a V2-typed buffer.
///  6. `PerfPoliciesGetInfo` — capability bitset (POWER_LIMIT/THERMAL/...).
///  7. `GetVoltageDomainsStatus` — Maxwell-tagged, verified on current GPU.
///  8. `GetPowerMizerInfo` (unwrapped ID `0x76bfa16b`) — raw `nvapi_QueryInterface` call
///     with a scratch buffer iterating candidate struct sizes (template for probing IDs
///     that have no Rust struct/FFI yet).
///
/// Outcome on the dev laptop: none of these return live per-rail watts. The per-rail
/// watts source is a WinRing0 PCI/MMIO kernel driver, not NVAPI — see
/// `docs/gpuz-per-rail-investigation.md`.
fn nvapi_raw_payload_probe() {
    use nvapi_hi::sys::gpu::power::private as pw;
    use nvapi_hi::sys::nvapi::{NvVersion, VersionedStruct};
    use nvapi_hi::sys::api as api;
    use nvapi_hi::sys::Status;

    // Helper: zero a versioned struct and stamp its version magic. Avoids the
    // ambiguous `StructVersion::versioned` call (each struct impls both
    // StructVersion and StructVersion<1>).
    macro_rules! ver {
        ($ty:ty) => {{
            let mut s = unsafe { std::mem::zeroed::<$ty>() };
            *s.nvapi_version_mut() = NvVersion::with_struct::<$ty>(1);
            s
        }};
    }

    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        eprintln!("nvapi_raw_payload_probe: no NVAPI backend, skipping");
        return;
    }
    // Get the first NVAPI physical GPU handle via nvapi_hi directly (the op
    // layer in core wraps RawConversion, which is exactly what we want to
    // sidestep here).
    nvapi_hi::initialize().expect("nvapi initialize");
    let gpus = nvapi_hi::Gpu::enumerate().expect("nvapi enumerate");
    if gpus.is_empty() {
        eprintln!("nvapi_raw_payload_probe: no NVAPI GPUs");
        return;
    }
    let gpu = gpus.into_iter().next().unwrap();
    let handle = *gpu.inner().handle();

    unsafe {
        // 1. NV_GPU_CLIENT_VOLT_RAILS_STATUS (76B) — we only take value_uV and
        //    *require* the two 8-u32 padding fields to be all-zero, else Err.
        //    Dump the whole thing to see if multi-rail voltages hide in padding.
        let mut volt = ver!(pw::NV_GPU_CLIENT_VOLT_RAILS_STATUS);
        let st = api::NvAPI_GPU_ClientVoltRailsGetStatus(handle, &mut volt);
        eprintln!("=== ClientVoltRailsGetStatus status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!("{:#?}", volt);
        }

        // 2. NV_GPU_CLIENT_POWER_TOPOLOGY — first query Info (which channels
        //    exist), then Status for those channels. On this laptop GPU Status
        //    returns -5 (INCOMPATIBLE_STRUCT) regardless of channels — handler's
        //    internal topology table is empty (v6[16]==0xFF). Confirm via Info.
        let mut info = ver!(pw::NV_GPU_CLIENT_POWER_TOPOLOGY_INFO);
        let st = api::NvAPI_GPU_ClientPowerTopologyGetInfo(handle, &mut info);
        eprintln!("=== ClientPowerTopologyGetInfo status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!("valid={} count={} channels={:?}", info.valid, info.count, info.channels());
        }

        let mut topo = ver!(pw::NV_GPU_CLIENT_POWER_TOPOLOGY_STATUS);
        topo.count = 2;
        topo.entries[0].channel =
            pw::NV_GPU_CLIENT_POWER_TOPOLOGY_CHANNEL_ID_TOTAL_GPU_POWER;
        topo.entries[1].channel =
            pw::NV_GPU_CLIENT_POWER_TOPOLOGY_CHANNEL_ID_NORMALIZED_TOTAL_POWER;
        let st = api::NvAPI_GPU_ClientPowerTopologyGetStatus(handle, &mut topo);
        eprintln!("=== ClientPowerTopologyGetStatus status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!("count={}", topo.count);
            for (i, e) in topo.entries.iter().enumerate().take(topo.count as usize + 1) {
                eprintln!("  entry[{i}] = {:#?}", e);
            }
        }

        // 3. NV_GPU_PERF_POLICIES_STATUS_PARAMS (0x550=1360B) — huge, lots of
        //    unexplained padding. Dump the head AND a hex view of the full
        //    payload to look for live power/thermal hiding in padding.
        let mut perf = ver!(pw::NV_GPU_PERF_POLICIES_STATUS_PARAMS);
        let st = api::NvAPI_GPU_PerfPoliciesGetStatus(handle, &mut perf);
        eprintln!("=== PerfPoliciesGetStatus status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!(
                "flags={} timer={} limits={:?} unknown={} timers={:?}",
                perf.flags, perf.timer, perf.limits, perf.unknown, perf.timers
            );
            // Raw hex of the whole 1360-byte struct to spot non-zero regions.
            let bytes: &[u8] = {
                let p = &perf as *const _ as *const u8;
                std::slice::from_raw_parts(p, std::mem::size_of_val(&perf))
            };
            eprintln!("PerfPolicies raw size={}", bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                let chunk = &bytes[i..(i + 16).min(bytes.len())];
                let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
                if chunk.iter().any(|&b| b != 0) {
                    eprintln!("  +{:04x}: {}", i, hex.join(" "));
                }
                i += 16;
            }
        }

        // 4. NV_VOLT_TABLE (0x40cc=16588B) — Maxwell multi-domain voltage table.
        let mut vt = ver!(pw::NV_VOLT_TABLE);
        let st = api::NvAPI_GPU_GetVoltages(handle, &mut vt);
        eprintln!("=== GetVoltages status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!("flags={} count={}", vt.flags, vt.count);
            for e in vt.entries() {
                eprintln!(
                    "  dom={} uV={} (first pad u32={})",
                    e.voltage_domain, e.voltage_uV, e.unknown[0]
                );
            }
        }

        // 5. NV_GPU_CLIENT_POWER_POLICIES — try V1 version magic (V2 returned
        //    -9 here). V1 returns min/def/max in MILLIWATTS (absolute watts),
        //    the prime candidate for GPU-Z's "Board Power Draw" readouts. The FFI
        //    symbol is typed V2, but the version magic selects layout — allocate
        //    a V2-sized buffer, stamp V1 magic, read back as V1 fields.
        {
            let mut pinfo = unsafe {
                std::mem::zeroed::<pw::NV_GPU_CLIENT_POWER_POLICIES_INFO>()
            };
            *pinfo.nvapi_version_mut() =
                NvVersion::with_struct::<pw::NV_GPU_CLIENT_POWER_POLICIES_INFO_V1>(1);
            let st = api::NvAPI_GPU_ClientPowerPoliciesGetInfo(handle, &mut pinfo);
            eprintln!("=== ClientPowerPoliciesGetInfo V1magic status={:?} ===", st);
            // Read the V1 layout (first 2 header bytes + V1 entries) from the
            // raw buffer regardless of which layout the driver wrote.
            let raw: &[u8] = {
                let p = &pinfo as *const _ as *const u8;
                std::slice::from_raw_parts(p, std::mem::size_of_val(&pinfo))
            };
            eprintln!(
                "  header valid={} count={} first entry u32s={:?}",
                pinfo.valid,
                pinfo.count,
                {
                    let mut v = Vec::new();
                    for i in 0..11 {
                        let off = 4 + i * 4;
                        if off + 4 <= raw.len() {
                            v.push(u32::from_le_bytes([
                                raw[off], raw[off + 1], raw[off + 2], raw[off + 3],
                            ]));
                        }
                    }
                    v
                }
            );
        }
        {
            let mut pstat = unsafe {
                std::mem::zeroed::<pw::NV_GPU_CLIENT_POWER_POLICIES_STATUS>()
            };
            *pstat.nvapi_version_mut() =
                NvVersion::with_struct::<pw::NV_GPU_CLIENT_POWER_POLICIES_STATUS_V1>(1);
            let st = api::NvAPI_GPU_ClientPowerPoliciesGetStatus(handle, &mut pstat);
            eprintln!("=== ClientPowerPoliciesGetStatus V1magic status={:?} ===", st);
            let raw: &[u8] = {
                let p = &pstat as *const _ as *const u8;
                std::slice::from_raw_parts(p, std::mem::size_of_val(&pstat))
            };
            // V1 status entry: [policy_id:u32][b:u32][power_target:u32][d:u32] = 16B
            eprintln!("  header count={}", pstat.count);
            for i in 0..4 {
                let off = 8 + i * 16;
                if off + 16 <= raw.len() {
                    let pid = u32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
                    let pt = u32::from_le_bytes(raw[off + 8..off + 12].try_into().unwrap());
                    if pid != 0 || pt != 0 {
                        eprintln!("  entry[{i}] policy={} power_target={}", pid, pt);
                    }
                }
            }
        }

        // V2 explicitly to confirm the -9.
        let mut pinfo2 = ver!(pw::NV_GPU_CLIENT_POWER_POLICIES_INFO);
        let st = api::NvAPI_GPU_ClientPowerPoliciesGetInfo(handle, &mut pinfo2);
        eprintln!("=== ClientPowerPoliciesGetInfo V2 status={:?} ===", st);

        // 6. NV_GPU_PERF_POLICIES_INFO_PARAMS — returns maxUnknown + limitSupport
        //    bitset (POWER_LIMIT/THERMAL/...). GPU-Z queries this; check for any
        //    absolute power data in the 76-byte struct.
        let mut ppinfo = ver!(pw::NV_GPU_PERF_POLICIES_INFO_PARAMS);
        let st = api::NvAPI_GPU_PerfPoliciesGetInfo(handle, &mut ppinfo);
        eprintln!("=== PerfPoliciesGetInfo status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!(
                "maxUnknown={} limitSupport={:?}",
                ppinfo.maxUnknown, ppinfo.limitSupport
            );
        }

        // 7. GetVoltageDomainsStatus (NV_VOLT_STATUS, 140B) — Maxwell-tagged but
        //    verify on this GPU.
        let mut vds = ver!(pw::NV_VOLT_STATUS);
        let st = api::NvAPI_GPU_GetVoltageDomainsStatus(handle, &mut vds);
        eprintln!("=== GetVoltageDomainsStatus status={:?} ===", st);
        if (st as i32) == (Status::Ok as i32) {
            eprintln!(
                "flags={} count={} value_uV={}",
                vds.flags, vds.count, vds.value_uV
            );
        }

        // 8. GetPowerMizerInfo (0x76bfa16b) — NOT wrapped in nvapi-rs. Probe raw
        //    via QueryInterface to see if it carries live power-state data. The
        //    struct size is unknown; try a 256-byte scratch buffer with version
        //    magic guessed as v1|sz256 = (1<<16)|256 = 65792.
        unsafe {
            use nvapi_hi::sys::nvapi_QueryInterface;
            const GET_POWERMIZER_INFO_ID: u32 = 0x76bfa16b;
            #[repr(C)]
            struct Scratch {
                version: u32,
                data: [u32; 63],
            }
            let mut scratch = Scratch { version: 0, data: [0; 63] };
            for sz in [256u32, 64, 128] {
                scratch.version = (sz) | (1 << 16);
                scratch.data = [0; 63];
                let ptr = match nvapi_QueryInterface(GET_POWERMIZER_INFO_ID) {
                    Ok(p) => p as *const (),
                    Err(_) => break,
                };
                type Fn = unsafe extern "system" fn(
                    nvapi_hi::sys::api::NvPhysicalGpuHandle,
                    *mut Scratch,
                ) -> nvapi_hi::sys::Status;
                let func: Fn = std::mem::transmute(ptr);
                let status = func(handle, &mut scratch);
                eprintln!(
                    "=== GetPowerMizerInfo sz={} status={:?} version_out=0x{:x} ===",
                    sz, status, scratch.version
                );
                if (status as i32) == (Status::Ok as i32) {
                    eprintln!("  data={:?}", &scratch.data[..16]);
                    break;
                }
            }
        }
    }
}

/// Dump the thermal-channel capability descriptor (undocumented
/// `NvAPI_GPU_ThermChannelGetInfo`, 0x0bc8163d) surfaced via `QueryGpuStatus`.
///
/// On success the status now carries authoritative "Hot Spot (authoritative)"
/// / "Memory (authoritative)" sensors when the driver exposes the priChIdx
/// LUT. On laptop GPUs the call may be stubbed (returns an error that the
/// status read tolerates) — this test only verifies it never panics and prints
/// whatever it finds for human inspection.
#[test]
#[ignore]
fn nvapi_therm_channel_info() {
    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        return;
    }
    let status = match run(&target, QueryGpuStatus) {
        Ok(report) => report.output,
        Err(e) => {
            eprintln!("QueryGpuStatus failed (thermal-channel read is best-effort): {e}");
            return;
        }
    };

    eprintln!("=== thermal sensors ({} entries) ===", status.sensors.len());
    for (desc, temp) in &status.sensors {
        eprintln!(
            "  {:<32} target={:?} mask={:?} => {:.2} C",
            desc.name.as_deref().unwrap_or("(unnamed)"),
            desc.target,
            desc.sensor_mask_number,
            temp
        );
    }

    let has_auth_hotspot = status
        .sensors
        .iter()
        .any(|(d, _)| d.name.as_deref() == Some("Hot Spot (authoritative)"));
    let has_auth_memory = status
        .sensors
        .iter()
        .any(|(d, _)| d.name.as_deref() == Some("Memory (authoritative)"));
    eprintln!(
        "=== ThermChannelGetInfo authoritative: hotspot={} memory={} ===",
        has_auth_hotspot, has_auth_memory
    );
    // No hard assertion: the authoritative path is best-effort. If the
    // driver exposes GetInfo, both authoritative entries appear; otherwise
    // only the heuristic entries do. (Verified on a laptop dGPU: both appear,
    // hotspot at channel 1, memory at channel 7.)
}

/// RAW probe of `NvAPI_GPU_ThermChannelGetInfo` (0x0bc8163d) — calls the FFI
/// directly, bypassing the hi-layer `allowable_result` degradation so we see
/// the *actual* NVAPI status code and raw struct bytes on whatever GPU this
/// runs on. This is the definitive desktop-GPU diagnostic: it tells you
/// whether GetInfo returns OK (and what the priChIdx LUT / channel records
/// contain), or which error it returns (NotSupported / NoImplementation /
/// -104 NvidiaDeviceNotFound / -9 IncompatibleStruct / ...).
///
/// Run on the other PC with:
///   cargo test -p nvoc-core --test gpu_readonly -- --ignored --nocapture nvapi_therm_channel_raw
#[test]
#[ignore]
fn nvapi_therm_channel_raw() {
    use nvapi_hi::sys::gpu::thermal::private as th;
    use nvapi_hi::sys::api as api;
    use nvapi_hi::sys::nvapi::NvVersion;
    use nvapi_hi::sys::Status;

    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        eprintln!("nvapi_therm_channel_raw: no NVAPI backend, skipping");
        return;
    }
    nvapi_hi::initialize().expect("nvapi initialize");
    let gpus = nvapi_hi::Gpu::enumerate().expect("nvapi enumerate");
    if gpus.is_empty() {
        eprintln!("nvapi_therm_channel_raw: no NVAPI GPUs");
        return;
    }
    let gpu = gpus.into_iter().next().unwrap();
    let handle = *gpu.inner().handle();

    // V2 params struct: version magic (2<<16)|sizeof = (2<<16)|2736.
    let mut info: th::NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2 =
        unsafe { std::mem::zeroed() };
    info.version = NvVersion::new(std::mem::size_of_val(&info), 2);

    let st = unsafe { api::NvAPI_GPU_ThermChannelGetInfo(handle, &mut info) };
    eprintln!(
        "=== ThermChannelGetInfo status={:?} ({}), struct_size={}, version_out=0x{:x} ===",
        st,
        st as i32,
        std::mem::size_of_val(&info),
        u32::from(info.version),
    );

    if (st as i32) != (Status::Ok as i32) {
        // Not a test failure — just a diagnostic. Print the error and stop.
        eprintln!("GetInfo did not return OK.");
        eprintln!("NotSupported/-104 => driver/GPU genuinely lacks it;");
        eprintln!("-9 (IncompatibleStruct) => struct size/layout is wrong.");
        return;
    }

    eprintln!("channel_mask = 0x{:08x} (popcount={})", info.channel_mask, info.channel_mask.count_ones());
    let type_names = ["GPU_AVG", "GPU_MAX(hotspot)", "BOARD", "MEMORY(vram)", "PWR_SUPPLY"];
    eprintln!("pri_ch_idx (primary channel per type):");
    for (ty, &idx) in info.pri_ch_idx.iter().enumerate() {
        let populated = (idx as usize) < 32 && (info.channel_mask & (1u32 << idx)) != 0;
        eprintln!(
            "  [{}] {:<18} => channel {} {}",
            ty,
            type_names.get(ty).copied().unwrap_or("?"),
            idx,
            if populated { "(valid)" } else { "(NOT in mask)" }
        );
    }
    eprintln!("per-channel records (first 16 populated):");
    let mut shown = 0;
    for i in 0..32 {
        if info.channel_mask & (1u32 << i) == 0 {
            continue;
        }
        let c = &info.channel[i];
        eprintln!(
            "  chan[{:>2}] ch_type={} ch_class={} rel_loc={} tgt_gpu={} range=[{}..{}] off_sw={} off_hw={} flags={}",
            i, c.ch_type, c.ch_class, c.rel_loc, c.tgt_gpu, c.min_temp, c.max_temp, c.offset_sw, c.offset_hw, c.flags
        );
        shown += 1;
        if shown >= 16 {
            break;
        }
    }
    if shown == 0 {
        eprintln!("  (channel_mask is 0 — driver returned OK but exposes no channels)");
    }

    // Now read the STATUS half using the RTSS ThermChannelGetStatus struct
    // (same ID 0x65fe3aad as GetThermalSensors, but the channel[32] layout).
    // Pass GetInfo's channel_mask; channel[i] is then the live temp for
    // channel i, indexed directly by priChIdx[type].
    let mut status: th::NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2 =
        unsafe { std::mem::zeroed() };
    status.version = NvVersion::new(std::mem::size_of_val(&status), 2);
    status.channel_mask = info.channel_mask;
    // Same FFI as GetThermalSensors (same QueryInterface ID), different struct.
    let st = unsafe {
        api::NvAPI_GPU_GetThermalSensors(
            handle,
            &mut status as *mut _ as *mut th::NV_GPU_THERMAL_SENSORS,
        )
    };
    eprintln!(
        "=== ThermChannelGetStatus status={:?} mask=0x{:x} ===",
        st, info.channel_mask
    );
    if (st as i32) == (Status::Ok as i32) {
        eprintln!("channel[32] (celsius*256), non-zero only:");
        for (i, &v) in status.channel.iter().enumerate() {
            if v != 0 {
                eprintln!("  chan[{:>2}] = {:>8}  => {:.2} C", i, v, v as f32 / 256.0);
            }
        }
        eprintln!("authoritative decode (channel[priChIdx[type]]):");
        for (ty, &idx) in info.pri_ch_idx.iter().enumerate() {
            if (idx as usize) >= 32 {
                continue;
            }
            let temp = status.get_temp(idx as usize);
            eprintln!(
                "  [{}] {:<18} channel[{}] = {} => {:.2} C",
                ty,
                type_names.get(ty).copied().unwrap_or("?"),
                idx,
                status.channel.get(idx as usize).copied().unwrap_or(0),
                temp.unwrap_or(0.0),
            );
        }
    }
}

