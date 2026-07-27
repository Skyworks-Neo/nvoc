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
            "  ch_type={:<3} target={:?} channel={:?} off_sw={:?} off_hw={:?} scaling={:?} => {:.2} C",
            desc.channel_type.unwrap_or(999),
            desc.target,
            desc.channel_num,
            desc.offset_sw,
            desc.offset_hw,
            desc.scaling,
            temp
        );
    }

    // Core = channel_type GPU_AVG (0); Hot Spot = GPU_MAX (1).
    let has_core = status
        .sensors
        .first()
        .is_some_and(|(d, _)| d.channel_type == Some(0));
    let has_hotspot = status
        .sensors
        .iter()
        .any(|(d, _)| d.channel_type == Some(1));
    eprintln!(
        "=== unified RTSS path: core_at_index0={} hotspot={} ===",
        has_core, has_hotspot
    );
    // Core MUST be sensors[0] — positional consumers take sensors.first() as
    // the core temperature. Hot Spot presence is best-effort (older GPUs may
    // only expose Core).
    assert!(has_core, "Core must be the first thermal sensor");
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
            "  chan[{:>2}] ch_type={} ch_class={} rel_loc={} tgt_gpu={} scaling={} range=[{}..{}] off_sw={} off_hw={} sim={} flags={} dev=[{},{}]",
            i,
            c.ch_type,
            c.ch_class,
            c.rel_loc,
            c.tgt_gpu,
            c.scaling,
            c.min_temp,
            c.max_temp,
            c.offset_sw,
            c.offset_hw,
            c.is_temp_sim_supported,
            c.flags,
            c.therm_dev_idx(),
            c.therm_dev_prov_idx()
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
    // (ID 0x65fe3aad, channel[32] layout). Pass GetInfo's channel_mask;
    // channel[i] is then the live temp for channel i, indexed directly by
    // priChIdx[type].
    let mut status: th::NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2 =
        unsafe { std::mem::zeroed() };
    status.version = NvVersion::new(std::mem::size_of_val(&status), 2);
    status.channel_mask = info.channel_mask;
    let st = unsafe { api::NvAPI_GPU_ThermChannelGetStatus(handle, &mut status) };
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

/// Raw dump of `NvAPI_GPU_ClientVoltRailsGetStatus` (0x465f9bcf, 76-byte V1).
///
/// Three-way cross-check (nvapi-rs / LibreHardwareMonitor / dev-laptop probe)
/// agrees the layout is 76B with the live core-voltage µV at offset 0x28. The
/// open question is offset **0x2C** onwards: LHM names 0x2C
/// `CoreMicrovoltsHigh` (the high 32 bits of a 64-bit µV value); the dev
/// laptop sees it all-zero, so it can't decide. RTSS iterates a `rails[]`
/// array form that doesn't fit 76B — its `MAX_ENTRIES`/entry layout are
/// unknown here.
///
/// This probe dumps the full 76 bytes as hex AND as a u32 word table with
/// offset annotations, so a **desktop GPU** run can settle whether 0x2C+ ever
/// carries a non-zero high-half (LHM's 64-bit µV) or a multi-rail pattern
/// (RTSS's array form). The laptop run is just a sanity baseline.
///
/// Run: `cargo test -p nvoc-core --test gpu_readonly -- --ignored --nocapture nvapi_volt_rails_raw`
#[test]
#[ignore]
fn nvapi_volt_rails_raw() {
    use nvapi_hi::sys::gpu::power::private as pw;
    use nvapi_hi::sys::api as api;
    use nvapi_hi::sys::nvapi::{NvVersion, VersionedStruct};
    use nvapi_hi::sys::Status;

    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        eprintln!("nvapi_volt_rails_raw: no NVAPI backend, skipping");
        return;
    }
    nvapi_hi::initialize().expect("nvapi initialize");
    let gpus = nvapi_hi::Gpu::enumerate().expect("nvapi enumerate");
    if gpus.is_empty() {
        eprintln!("nvapi_volt_rails_raw: no NVAPI GPUs");
        return;
    }
    let gpu = gpus.into_iter().next().unwrap();
    let handle = *gpu.inner().handle();

    // 76-byte V1 struct, version magic (1<<16)|76 = 65612.
    let mut volt: pw::NV_GPU_CLIENT_VOLT_RAILS_STATUS = unsafe { std::mem::zeroed() };
    *volt.nvapi_version_mut() = NvVersion::with_struct::<pw::NV_GPU_CLIENT_VOLT_RAILS_STATUS>(1);

    let st = unsafe { api::NvAPI_GPU_ClientVoltRailsGetStatus(handle, &mut volt) };
    eprintln!(
        "=== ClientVoltRailsGetStatus status={:?} ({}), struct_size={} ===",
        st,
        st as i32,
        std::mem::size_of_val(&volt),
    );
    if (st as i32) != (Status::Ok as i32) {
        eprintln!("Not OK — diagnostic only, not a failure.");
        return;
    }

    // Reinterpret the 76 bytes as a raw byte/u32 table to see every word,
    // bypassing the Rust struct's named fields (which hide 0x2C+ as padding).
    let len = std::mem::size_of_val(&volt);
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(&volt as *const _ as *const u8, len)
    };
    eprintln!("raw {} bytes (hex):", len);
    for chunk in bytes.chunks(16) {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
        eprintln!("  {:04x}: {}", chunk.as_ptr() as usize - bytes.as_ptr() as usize, hex.join(" "));
    }
    eprintln!("u32 word table (little-endian), with named-offset annotations:");
    let words: &[u32] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u32, len / 4) };
    for (i, &w) in words.iter().enumerate() {
        let off = i * 4;
        let note = match off {
            0x00 => "version magic",
            0x04 => "flags",
            0x28 => "value_uV / CoreMicrovolts (LIVE core voltage, low 32 bits)",
            0x2C => "unknown[0] / CoreMicrovoltsHigh (LHM: high 32 bits of 64-bit µV)",
            _ if (0x08..0x28).contains(&off) => "zero[] padding",
            _ if (0x30..0x4C).contains(&off) => "unknown[] padding",
            _ => "",
        };
        eprintln!("  +0x{:02X} [{:>2}] = 0x{:08X} ({}) {}", off, i, w, w, note);
    }
    // Decode the live value both ways for comparison.
    let lo = words[0x28 / 4];
    let hi = words[0x2C / 4];
    eprintln!(
        "decoded: value_uV(0x28)={} ({} mV); 64-bit µV=(hi<<32)|lo={} ({} mV)",
        lo,
        lo / 1000,
        ((hi as u64) << 32) | lo as u64,
        (((hi as u64) << 32) | lo as u64) / 1000,
    );
    eprintln!(
        "desktop verdict: if 0x2C non-zero -> LHM 64-bit µV confirmed; if 0x30..0x4C shows a repeating non-zero pattern -> RTSS rails[] array form"
    );
}

/// Raw probe of `NvAPI_GPU_PowerMonitorGetInfo` (0xC12EB19E) AND GetStatus
/// (0xF40238EF). Both are routed by `nvapi_QueryInterface` to handlers in the
/// deployed `nvapi64_impl.dll`, then both funnel into the SAME RM escape
/// 0x06FF0016 (the private per-channel power-monitor RM control) via
/// sub_1803894A0/sub_180389320. The handler decodes the caller's first-DWORD
/// version magic as `(version<<16)|sizeof(struct)` and accepts ONLY a fixed set
/// per IID; anything else → -9 INCOMPATIBLE_STRUCT_VERSION.
///
/// **CORRECTED accepted-magic sets (RE'd from the handler comparisons, 2026-07-27):**
/// - GetInfo (0xC12EB19E), handler @0x180257660 — accepts:
///     65940  = (1<<16)|396
///     68264  = (1<<16)|2728
///     199848 = (3<<16)|3208
///     268456 = (4<<16)|6088
///     377896 = (5<<16)|50216   ← the big v5 layout (50 KiB)
/// - GetStatus (0xF40238EF), handler @0x180258170 — accepts:
///     65928  = (1<<16)|392
///     66972  = (1<<16)|1436
///     69408  = (1<<16)|3872
///     74968  = (1<<16)|9432
///     336752 = (5<<16)|9072    ← the big v5 layout
///
/// **PRIOR BUG (why every probe returned -9):** the old probe fed the GetStatus
/// magics (65928/66972/69408/74968/336752) to GetInfo — a completely different
/// accepted set — so GetInfo's size gate rejected all of them. The GetInfo and
/// GetStatus accepted sets share NO members. This corrected probe tries each
/// IID against its OWN accepted set.
///
/// The GetInfo handler populates a per-channel table: `v23[84]` is the channel
/// type (1/3 scalar, 4 bitset, 5 multi-field, 7 signed+state) and `v25[1]` the
/// value — exactly the per-rail power/current data GPU-Z shows via WinRing0.
/// If a desktop GPU ACCEPTS one of these magics, the per-channel power table is
/// reachable via pure NVAPI and worth wrapping. On laptop/locked GPUs the RM
/// escape itself returns non-zero and the handler propagates that (RM-level
/// gate, not a struct-version gate).
///
/// Run: `cargo test -p nvoc-core --test gpu_readonly -- --ignored --nocapture nvapi_power_monitor_raw`
#[test]
#[ignore]
fn nvapi_power_monitor_raw() {
    use nvapi_hi::sys::nvapi_QueryInterface;
    use nvapi_hi::sys::Status;

    let inv = inventory();
    let target = first_target(&inv);
    if !target.has_nvapi() {
        eprintln!("nvapi_power_monitor_raw: no NVAPI backend, skipping");
        return;
    }
    nvapi_hi::initialize().expect("nvapi initialize");
    let gpus = nvapi_hi::Gpu::enumerate().expect("nvapi enumerate");
    if gpus.is_empty() {
        eprintln!("nvapi_power_monitor_raw: no NVAPI GPUs");
        return;
    }
    let gpu = gpus.into_iter().next().unwrap();
    let handle = *gpu.inner().handle();

    // The two PowerMonitor IIDs and their CORRECT accepted-magic sets, RE'd from
    // the handler size-gates in nvapi64_impl.dll (see the doc comment above).
    // Each IID only accepts its OWN set; cross-feeding (the prior bug) always -9s.
    const POWER_MONITOR_GET_INFO_ID: u32 = 0xC12EB19E;
    const POWER_MONITOR_GET_STATUS_ID: u32 = 0xF40238EF;
    // (label, iid, accepted magics)
    let probes: &[(&str, u32, &[u32])] = &[
        // GetInfo accepted set: v1|396, v1|2728, v3|3208, v4|6088, v5|50216.
        (
            "GetInfo",
            POWER_MONITOR_GET_INFO_ID,
            &[65940, 68264, 199848, 268456, 377896],
        ),
        // GetStatus accepted set: v1|392, v1|1436, v1|3872, v1|9432, v5|9072.
        (
            "GetStatus",
            POWER_MONITOR_GET_STATUS_ID,
            &[65928, 66972, 69408, 74968, 336752],
        ),
    ];
    // Scratch big enough for the LARGEST layout we'll try (GetInfo v5 = 50216B) + slack.
    const SCRATCH_U32: usize = 50216 / 4 + 64;
    #[repr(C)]
    struct Scratch {
        version: u32,
        data: [u32; SCRATCH_U32 - 1],
    }

    type Fn = unsafe extern "system" fn(
        nvapi_hi::sys::api::NvPhysicalGpuHandle,
        *mut Scratch,
    ) -> Status;

    // Run each IID against its OWN accepted-magic set. For GetStatus, take the
    // first accepted magic (one live layout is enough). For GetInfo, try ALL
    // magics — the larger layouts (v3|3208, v4|6088, v5|50216) may carry the
    // per-channel DESCRIPTOR/scaling tables (channel_type / pwr_rail /
    // pwr_corr_slope / pwr_offset_mw) that v1|404 lacks, which are needed to
    // convert raw GetStatus values to W/A. Keep every accepted GetInfo layout.
    let mut accepted_per_iid: Vec<(&str, u32, u32, Scratch)> = Vec::new(); // (label, version, size, scratch)
    for (label, iid, magics) in probes {
        let ptr = match nvapi_QueryInterface(*iid) {
            Ok(p) => p as *const (),
            Err(e) => {
                eprintln!("nvapi_power_monitor_raw: QueryInterface {:#x} ({}) not found: {:?}", iid, label, e);
                continue;
            }
        };
        let func: Fn = unsafe { std::mem::transmute(ptr) };
        let try_all = *label == "GetInfo"; // GetInfo: probe every magic for descriptor data
        eprintln!("=== {} ({:#X}): trying its accepted-magic set ===", label, iid);
        for &magic in *magics {
            let mut scratch = Scratch { version: 0, data: [0; SCRATCH_U32 - 1] };
            scratch.version = magic;
            let status = unsafe { func(handle, &mut scratch) };
            let ok = (status as i32) == (Status::Ok as i32);
            // Count nonzero words as a signal of how much the layout returned.
            let sz = (magic & 0xFFFF) as usize;
            let nz_words = scratch.data[..sz / 4]
                .iter()
                .filter(|w| **w != 0)
                .count();
            eprintln!(
                "  [{}] magic=0x{:X} (v{}|sz{}) status={:?} ({}){} nonzero_words={}",
                label, magic, magic >> 16, magic & 0xFFFF, status, status as i32,
                if ok { "  <<< ACCEPTED" } else { "" }, nz_words,
            );
            if ok {
                accepted_per_iid.push((label, magic >> 16, magic & 0xFFFF, scratch));
                if !try_all {
                    break; // GetStatus: first accepted magic is enough
                }
                // GetInfo: keep trying larger magics for descriptor data
            }
        }
    }

    if accepted_per_iid.is_empty() {
        eprintln!(
            "no accepted magic for either IID — even with the corrected sets. This means the\n\
             RM escape 0x06FF0016 itself is gated off on this GPU/driver (RM-level rejection,\n\
             not a struct-version mismatch). Per-rail power is not reachable via NVAPI here;\n\
             it needs the WinRing0 PCI/MMIO kernel path (see docs/gpuz-per-rail-investigation.md)."
        );
        return;
    }

    for (label, ver, sz, scratch) in &accepted_per_iid {
        eprintln!("=== {} ACCEPTED version={} size={} ===", label, ver, sz);
        let words: &[u32] = unsafe {
            std::slice::from_raw_parts(scratch as *const _ as *const u32, SCRATCH_U32)
        };
        eprintln!("  first 16 u32: {:?}", &words[..16]);
        eprintln!(
            "  +0x00 version=0x{:X}  +0x04 bSupported?={}  +0x08 samplingPeriodMs?={}  +0x0C sampleCount?={}  +0x10 channelMask?=0x{:X}  +0x14 chRelMask?=0x{:X}  +0x18 totalGpuPowerChannelMask?=0x{:X}  +0x1C totalGpuChannelIdx=0x{:X}",
            words[0], words[1], words[2], words[3], words[4], words[5], words[6], words[7],
        );
        // Dump the ENTIRE accepted struct (not just 256B) so per-channel
        // offsets in the v1|392 GetStatus layout are fully visible.
        let dump_len = (*sz as usize).min(SCRATCH_U32 * 4);
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(scratch as *const _ as *const u8, dump_len)
        };
        eprintln!("  full {} bytes (hex):", dump_len);
        for chunk in bytes.chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            eprintln!(
                "    {:04x}: {}",
                chunk.as_ptr() as usize - bytes.as_ptr() as usize,
                hex.join(" ")
            );
        }
        // List every nonzero 32-bit word offset to spotlight the per-channel
        // value slots (the meat of the GetStatus layout) for layout RE.
        let nz: Vec<(usize, u32)> = words
            .iter()
            .take(*sz as usize / 4)
            .enumerate()
            .filter(|(_, w)| **w != 0)
            .map(|(i, w)| (i * 4, *w))
            .collect();
        eprintln!("  nonzero u32 offsets: {:?}", nz);
        // For GetInfo, decode the power-channel bitmask. In this v1|404 layout
        // the channel mask lives at byte offset +0x40 (word 16), NOT +0x10 — the
        // RTSS-derived V2 doc puts it at +0x10, but the deployed v1 handler writes
        // it later in the struct. Scan words 4..32 for the first plausible mask
        // (the populated channel set) so we report it regardless of exact slot.
        if *label == "GetInfo" {
            let mask = (4..32.min(words.len()))
                .map(|i| words[i])
                .find(|w| *w != 0 && *w != 1)
                .unwrap_or(0);
            if mask != 0 {
                let chans: Vec<u32> = (0..32).filter(|i| mask & (1u32 << i) != 0).collect();
                eprintln!(
                    "  GetInfo channelMask=0x{:X} -> {} channels: {:?}",
                    mask,
                    chans.len(),
                    chans
                );
            }
        }
    }

    // For GetStatus, sample several times and report which byte offsets vary —
    // those are the LIVE per-channel value slots (units TBD). Static offsets
    // are descriptors/headers. This confirms the data is realtime, not a blob.
    if let Some((_, ver, sz, _)) = accepted_per_iid.iter().find(|(l, _, _, _)| *l == "GetStatus") {
        let ptr = match nvapi_QueryInterface(POWER_MONITOR_GET_STATUS_ID) {
            Ok(p) => p as *const (),
            Err(_) => return,
        };
        let func: Fn = unsafe { std::mem::transmute(ptr) };
        let magic = (*ver << 16) | *sz;

        // EXPERIMENT: GetStatus takes channel_mask as INPUT (caller selects which
        // channels to read). With only the version magic set, the driver fills
        // just channel 0 (total). Try setting the input mask to the FULL GetInfo
        // channel set (0x80C142B) at +0x04 and see whether the per-rail channels
        // (MVDDC/Chip/PWR_SRC/16-pin) populate at their per-channel slots.
        eprintln!(
            "=== GetStatus per-channel experiment: input channel_mask = full GetInfo set 0x80C142B ==="
        );
        {
            let mut s = Scratch { version: 0, data: [0; SCRATCH_U32 - 1] };
            s.version = magic;
            s.data[0] = 0x80C142B; // word[1] = +0x04 = input channel_mask (all 9)
            let status = unsafe { func(handle, &mut s) };
            let words: &[u32] =
                unsafe { std::slice::from_raw_parts(&s as *const _ as *const u32, *sz as usize / 4) };
            let nz: Vec<(usize, u32)> = words
                .iter()
                .enumerate()
                .filter(|(_, w)| **w != 0)
                .map(|(i, w)| (i * 4, *w))
                .collect();
            eprintln!("  status={:?} ({})  nonzero u32 offsets with full input mask: {:?}", status, status as i32, nz);
            // The populated offsets are irregular (0x44,0x98,0xE0,0x14C,...),
            // suggesting records are packed contiguously per active bit. Dump the
            // region from +0x40 onward in 16-byte rows so the record structure is
            // visible. Cross-reference values against GPU-Z rails under load.
            eprintln!("  GetStatus body from +0x40 (16-byte rows) — match values to GPU-Z rails:");
            let body: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    (&s as *const _ as *const u8).add(0x40),
                    (*sz as usize).saturating_sub(0x40),
                )
            };
            for (i, chunk) in body.chunks(16).enumerate() {
                let any = chunk.iter().any(|b| *b != 0);
                if any {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
                    eprintln!("    +{:04X}: {}", 0x40 + i * 16, hex.join(" "));
                }
            }
        }

        eprintln!(
            "=== GetStatus liveness sweep (8 samples, {} bytes each) — varying offsets are live sensor values ===",
            sz
        );
        // Also correlate the live channel value(s) against NVML power_draw (mW)
        // to deduce units. The most prominent live offset observed is +0x44.
        let nvml = nvml_wrapper::Nvml::init().ok();
        let nvml_dev = nvml.as_ref().and_then(|n| n.device_by_index(0).ok());
        let mut prev: Option<Vec<u8>> = None;
        let n_words = *sz as usize / 4;
        for sample in 0..8 {
            let mut s = Scratch { version: 0, data: [0; SCRATCH_U32 - 1] };
            s.version = magic;
            let _ = unsafe { func(handle, &mut s) }; // status may flicker; ignore
            let bytes: Vec<u8> = unsafe {
                std::slice::from_raw_parts(&s as *const _ as *const u8, *sz as usize).to_vec()
            };
            let words: &[u32] =
                unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u32, n_words) };
            let nz: Vec<(usize, u32)> = words
                .iter()
                .enumerate()
                .filter(|(_, w)| **w != 0)
                .map(|(i, w)| (i * 4, *w))
                .collect();
            let nvml_mw = nvml_dev
                .as_ref()
                .and_then(|d| d.power_usage().ok())
                .map(|mw| mw as u32)
                .unwrap_or(0);
            // Candidate live value at +0x44 (word index 17); show ratio to NVML mW.
            let ch44 = words.get(17).copied().unwrap_or(0);
            let ratio = if nvml_mw > 0 { ch44 as f32 / nvml_mw as f32 } else { 0.0 };
            let mut diff: Vec<(usize, u32)> = Vec::new();
            if let Some(p) = &prev {
                diff = bytes
                    .iter()
                    .zip(p.iter())
                    .enumerate()
                    .filter(|(_, (ab, pb))| ab != pb)
                    .map(|(i, _)| (i, words[i / 4]))
                    .collect();
            }
            eprintln!(
                "  sample {}: nonzero={:?} changed={:?} | NVML={}mW  +0x44={}  ratio(+0x44/NVML)={:.3}",
                sample, nz, diff, nvml_mw, ch44, ratio
            );
            prev = Some(bytes);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

