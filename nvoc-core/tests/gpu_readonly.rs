use nvapi_hi::Gpu;
use nvml_wrapper::Nvml;
use nvoc_core::legacy::{
    get_gpu_tdp_temp_limit, get_nvml_core_clock_vf_offset, get_nvml_mem_clock_vf_offset,
    get_nvml_min_max_fan_speed, get_nvml_num_fans, get_nvml_pstate_info,
    get_nvml_supported_applications_clocks, get_nvml_temperature_thresholds,
    get_sorted_gpu_ids_nvml, get_sorted_gpus, get_voltage_by_point, query_nvml_power_watts,
    select_gpu_ids, select_gpus, single_gpu, voltage_frequency_check,
};
use nvoc_core::{
    Error, GpuId, GpuSelector, fetch_gpu_type, gpu_id_from_nvml_device, nvml_pstate_to_str,
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

fn nvml() -> Nvml {
    Nvml::init().expect("NVML should initialize on the GPU CI runner")
}

fn sorted_gpus() -> Vec<Gpu> {
    let gpus = get_sorted_gpus().expect("NVAPI should enumerate GPUs on the GPU CI runner");
    assert!(
        !gpus.is_empty(),
        "GPU CI runner should expose at least one GPU"
    );
    gpus
}

fn first_gpu() -> Gpu {
    sorted_gpus().into_iter().next().unwrap()
}

fn first_gpu_id_nvml(nvml: &Nvml) -> u32 {
    let ids = get_sorted_gpu_ids_nvml(nvml).expect("NVML should enumerate GPU ids");
    assert!(
        !ids.is_empty(),
        "GPU CI runner should expose at least one NVML GPU"
    );
    ids[0]
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
    let gpus = sorted_gpus();
    let ids = gpus.iter().map(|gpu| gpu.id()).collect::<Vec<_>>();
    assert_sorted_unique(&ids);

    for gpu in &gpus {
        let info = gpu.info().expect("GPU info should be readable");
        assert_eq!(info.id, gpu.id());
        assert!(!info.name.trim().is_empty());

        if let Some(truth) = truth_for_gpu(info.id as u32) {
            if let Some(expected) = truth.get("name_contains").and_then(Value::as_str) {
                assert!(
                    info.name.contains(expected),
                    "{} should contain {expected}",
                    info.name
                );
            }
            if let Some(expected) = truth.get("gpu_type").and_then(Value::as_str) {
                let actual = fetch_gpu_type(&info).unwrap();
                assert_eq!(format!("{actual:?}"), expected);
            }
        }
    }
}

#[test]
#[ignore]
fn discovery_nvml_ids() {
    let nvml = nvml();
    let ids = get_sorted_gpu_ids_nvml(&nvml).expect("NVML ids should be readable");
    assert!(!ids.is_empty());
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
    let nvml = nvml();
    let ids = get_sorted_gpu_ids_nvml(&nvml).expect("NVML ids should be readable");
    assert!(!ids.is_empty());
    let device = nvml
        .device_by_index(0)
        .expect("first NVML device should be readable");
    assert_eq!(gpu_id_from_nvml_device(&device).unwrap().0, ids[0]);
}

#[test]
#[ignore]
fn selection_nvapi() {
    let gpus = sorted_gpus();
    let selected = select_gpus(&gpus, &GpuSelector::all()).unwrap();
    assert_eq!(selected.len(), gpus.len());
    assert!(single_gpu(&selected).is_ok() || gpus.len() > 1);

    let by_index = select_gpus(&gpus, &GpuSelector::from_specs(["0".to_string()])).unwrap();
    assert_eq!(by_index[0].id(), gpus[0].id());

    let by_id = select_gpus(&gpus, &GpuSelector::from_specs([gpus[0].id().to_string()])).unwrap();
    assert_eq!(by_id[0].id(), gpus[0].id());

    let err = match select_gpus(&gpus, &GpuSelector::from_specs(["999999".to_string()])) {
        Ok(_) => panic!("invalid GPU selector should fail"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("no GPU matches --gpu"));
    assert!(single_gpu(&[]).is_err());
}

#[test]
#[ignore]
fn selection_nvml_ids() {
    let nvml = nvml();
    let ids = get_sorted_gpu_ids_nvml(&nvml).unwrap();
    let all = select_gpu_ids(&ids, &GpuSelector::all()).unwrap();
    assert_eq!(all, ids);
    assert_eq!(
        select_gpu_ids(&ids, &GpuSelector::from_specs(["0".to_string()])).unwrap(),
        vec![ids[0]]
    );
    assert!(select_gpu_ids(&ids, &GpuSelector::from_specs(["999999".to_string()])).is_err());
}

#[test]
#[ignore]
fn nvml_power_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    let (min_w, current_w, max_w) =
        query_nvml_power_watts(&nvml, gpu_id).expect("power limits should be readable");
    assert!(min_w >= 0.0);
    assert!(current_w >= min_w || min_w == 0.0);
    assert!(max_w >= current_w || max_w == 0.0);

    if let Some(truth) = truth_for_gpu(gpu_id)
        && let Some(power) = truth.pointer("/nvml/power_watts")
    {
        assert_optional_min(power.get("min"), min_w);
        assert_optional_min(power.get("current_min"), current_w);
        assert_optional_max(power.get("current_max"), current_w);
        assert_optional_max(power.get("max"), max_w);
    }
}

#[test]
#[ignore]
fn nvml_power_bad_gpu() {
    let nvml = nvml();
    assert!(query_nvml_power_watts(&nvml, INVALID_GPU_ID).is_none());
    assert!(GpuId::from_pci_str("invalid-pci-id").is_err());
}

#[test]
#[ignore]
fn nvml_offsets_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    for (pstate, _, _, _, _) in get_nvml_pstate_info(&nvml, gpu_id).unwrap_or_default() {
        if let Some(offset) = get_nvml_core_clock_vf_offset(&nvml, gpu_id, pstate) {
            assert!(offset.abs() < 2_000);
        }
        if let Some(offset) = get_nvml_mem_clock_vf_offset(&nvml, gpu_id, pstate) {
            assert!(offset.abs() < 10_000);
        }
    }
}

#[test]
#[ignore]
fn nvml_offsets_bad_gpu() {
    let nvml = nvml();
    let pstate = nvoc_core::parse_nvml_pstate("P0").unwrap();
    assert!(get_nvml_core_clock_vf_offset(&nvml, INVALID_GPU_ID, pstate).is_none());
    assert!(get_nvml_mem_clock_vf_offset(&nvml, INVALID_GPU_ID, pstate).is_none());
}

#[test]
#[ignore]
fn nvml_temp_thresholds_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    if let Some(thresholds) = get_nvml_temperature_thresholds(&nvml, gpu_id) {
        assert_eq!(thresholds.len(), 8);
        for (_, threshold) in thresholds {
            if let Some(celsius) = threshold {
                assert!(celsius <= 130 || celsius == u32::MAX);
            }
        }
    }
}

#[test]
#[ignore]
fn nvml_temp_thresholds_bad_gpu() {
    let nvml = nvml();
    assert!(get_nvml_temperature_thresholds(&nvml, INVALID_GPU_ID).is_none());
}

#[test]
#[ignore]
fn nvml_pstates_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    if let Some(pstates) = get_nvml_pstate_info(&nvml, gpu_id) {
        assert!(!pstates.is_empty());
        for (pstate, min_core, max_core, min_mem, max_mem) in &pstates {
            assert!(min_core <= max_core);
            assert!(min_mem <= max_mem);
            assert!(nvoc_core::nvml_pstate_to_index(*pstate).is_ok());
        }

        if let Some(truth) = truth_for_gpu(gpu_id)
            && let Some(expected) = truth.pointer("/nvml/pstates").and_then(Value::as_array)
        {
            let actual = pstates
                .iter()
                .map(|(pstate, _, _, _, _)| nvml_pstate_to_str(*pstate))
                .collect::<Vec<_>>();
            for expected in expected.iter().filter_map(Value::as_str) {
                assert!(actual.contains(&expected));
            }
        }
    }
}

#[test]
#[ignore]
fn nvml_pstates_bad_gpu() {
    let nvml = nvml();
    assert!(get_nvml_pstate_info(&nvml, INVALID_GPU_ID).is_none());
}

#[test]
#[ignore]
fn nvml_app_clocks_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    if let Some(clocks) = get_nvml_supported_applications_clocks(&nvml, gpu_id) {
        for (mem_clock, graphics_clocks) in clocks {
            assert!(mem_clock > 0);
            for graphics_clock in graphics_clocks {
                assert!(graphics_clock > 0);
            }
        }
    }
}

#[test]
#[ignore]
fn nvml_app_clocks_bad_gpu() {
    let nvml = nvml();
    assert!(get_nvml_supported_applications_clocks(&nvml, INVALID_GPU_ID).is_none());
}

#[test]
#[ignore]
fn nvml_fans_ok() {
    let nvml = nvml();
    let gpu_id = first_gpu_id_nvml(&nvml);
    if let Some((min, max)) = get_nvml_min_max_fan_speed(&nvml, gpu_id) {
        assert!(min <= max);
        assert!(max <= 100);
    }

    if let Some(count) = get_nvml_num_fans(&nvml, gpu_id)
        && let Some(truth) = truth_for_gpu(gpu_id)
        && let Some(expected) = truth.pointer("/nvml/fan_count").and_then(Value::as_u64)
    {
        assert_eq!(count as u64, expected);
    }
}

#[test]
#[ignore]
fn nvml_fans_bad_gpu() {
    let nvml = nvml();
    assert!(get_nvml_min_max_fan_speed(&nvml, INVALID_GPU_ID).is_none());
    assert!(get_nvml_num_fans(&nvml, INVALID_GPU_ID).is_none());
}

#[test]
#[ignore]
fn nvapi_voltage_point_ok() {
    let gpu = first_gpu();
    let status = gpu.status().expect("GPU status should be readable");
    let Some(vfp) = status.vfp else {
        assert!(matches!(
            get_voltage_by_point(&gpu, 0),
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
    let voltage = get_voltage_by_point(&gpu, *point).expect("VFP point voltage should be readable");
    assert_eq!(voltage, expected.voltage);
    if voltage.0 != 0 {
        assert!(voltage.0 <= 2_000_000);
    }
}

#[test]
#[ignore]
fn nvapi_voltage_point_bad_point() {
    let gpu = first_gpu();
    assert!(get_voltage_by_point(&gpu, usize::MAX).is_err());
}

#[test]
#[ignore]
fn nvapi_tdp_temp_ok() {
    let gpu = first_gpu();
    let result = get_gpu_tdp_temp_limit(&[&gpu], || {});
    match result {
        Ok((min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp, curve)) => {
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
fn nvapi_tdp_temp_empty_slice() {
    assert!(get_gpu_tdp_temp_limit(&[], || {}).is_ok());
}

#[test]
#[ignore]
fn nvapi_vf_check_ok() {
    let gpu = first_gpu();
    let status = gpu.status().expect("GPU status should be readable");
    let Some(vfp) = status.vfp else {
        assert!(matches!(
            voltage_frequency_check(&[&gpu], 0, || {}),
            Err(Error::VfpUnsupported)
        ));
        return;
    };
    let point = *vfp
        .graphics
        .keys()
        .next()
        .expect("VFP table should not be empty");
    match voltage_frequency_check(&[&gpu], point, || {}) {
        Ok(_) => {}
        Err(Error::VfpUnsupported) => {}
        Err(e) => panic!("unexpected read-only voltage/frequency error: {e}"),
    }
}

#[test]
#[ignore]
fn nvapi_vf_check_bad_point() {
    let gpu = first_gpu();
    assert!(voltage_frequency_check(&[&gpu], usize::MAX, || {}).is_err());
}
