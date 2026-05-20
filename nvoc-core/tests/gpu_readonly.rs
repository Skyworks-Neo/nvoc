use nvapi_hi::Microvolts;
use nvml_wrapper::Nvml;
use nvoc_core::{
    BackendSet, CheckVoltageFrequency, ClockDomain, Error, GpuId, GpuSelector, GpuTarget,
    QueryClockOffset, QueryFanInfo, QueryPowerLimits, QueryPstates,
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
    *targets
        .iter()
        .find(|t| t.nvml.is_some())
        .unwrap_or(&targets[0])
}

fn nvml(inventory: &TargetInventory) -> &Nvml {
    inventory
        .targets()
        .iter()
        .find(|t| t.nvml.is_some())
        .expect("at least one NVML backend should be present")
        .nvml
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
        let Some(gpu) = target.nvapi else { continue };
        let info = gpu.info().expect("GPU info should be readable");
        assert_eq!(info.id, gpu.id());
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
    let nvapi_targets: Vec<GpuTarget<'_>> =
        targets.into_iter().filter(|t| t.nvapi.is_some()).collect();
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
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
    let bad_target = GpuTarget {
        id: GpuId(INVALID_GPU_ID),
        index: 0,
        nvapi: None,
        nvml: None,
    };
    assert!(run(&bad_target, QueryFanInfo).is_err());
}

#[test]
#[ignore]
fn nvapi_voltage_point_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let Some(gpu) = target.nvapi else { return };
    let status = gpu.status().expect("GPU status should be readable");
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
            let (min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp, curve) =
                report.output;
            assert!(min_tdp.0 <= max_tdp.0);
            assert!(default_tdp.0 >= min_tdp.0 || default_tdp.0 == 8191);
            assert!(min_temp.0 <= max_temp.0);
            assert!(default_temp.0 >= min_temp.0 || default_temp.0 == 511);
            assert!(!curve.points.is_empty());
        }
        Err(Error::FeatureUnsupportedErr | Error::VfpUnsupported) => {}
        Err(e) => panic!("unexpected read-only TDP/temp error: {e}"),
    }
}

#[test]
#[ignore]
fn nvapi_tdp_temp_no_nvapi() {
    let bad_target = GpuTarget {
        id: GpuId(0),
        index: 0,
        nvapi: None,
        nvml: None,
    };
    assert!(run(&bad_target, QueryTdpTempLimits).is_err());
}

#[test]
#[ignore]
fn nvapi_vf_check_ok() {
    let inv = inventory();
    let target = first_target(&inv);
    let Some(gpu) = target.nvapi else { return };
    let status = gpu.status().expect("GPU status should be readable");
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
        Ok(_) => {}
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
