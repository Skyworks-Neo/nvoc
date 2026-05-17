use nvml_wrapper::enum_wrappers::device::PerformanceState;
use nvoc_core::{
    ConvertEnum, GpuSelector, GpuType, VfpResetDomain, detect_gpu_type, nvml_pstate_to_index,
    nvml_pstate_to_str, parse_nvml_pstate, select_gpu_ids, try_parse_nvml_pstate,
};

#[test]
fn nvml_pstate_parsing_accepts_common_forms() {
    assert_eq!(try_parse_nvml_pstate("P0").unwrap(), PerformanceState::Zero);
    assert_eq!(
        try_parse_nvml_pstate("p15").unwrap(),
        PerformanceState::Fifteen
    );
    assert_eq!(
        try_parse_nvml_pstate(" 10 ").unwrap(),
        PerformanceState::Ten
    );

    let err = try_parse_nvml_pstate("P16").unwrap_err().to_string();
    assert!(err.contains("Invalid NVML PState P16"));
}

#[test]
fn nvml_pstate_formatting_round_trips_known_states() {
    for index in 0..=15 {
        let raw = format!("P{index}");
        let pstate = parse_nvml_pstate(&raw);

        assert_eq!(nvml_pstate_to_index(pstate).unwrap(), index);
        assert_eq!(nvml_pstate_to_str(pstate), raw);
    }

    assert!(nvml_pstate_to_index(PerformanceState::Unknown).is_err());
    assert_eq!(nvml_pstate_to_str(PerformanceState::Unknown), "Unknown");
}

#[test]
fn vfp_reset_domain_convert_enum_matches_cli_values() {
    assert_eq!(
        VfpResetDomain::from_str("all").unwrap(),
        VfpResetDomain::All
    );
    assert_eq!(
        VfpResetDomain::from_str("core").unwrap(),
        VfpResetDomain::Core
    );
    assert_eq!(
        VfpResetDomain::from_str("memory").unwrap(),
        VfpResetDomain::Memory
    );
    assert_eq!(VfpResetDomain::Core.to_str(), "core");
    assert_eq!(
        VfpResetDomain::possible_values(),
        &["all", "core", "memory"]
    );
    assert!(VfpResetDomain::from_str("graphics").is_err());
}

#[test]
fn gpu_id_selection_supports_indices_and_nvapi_bus_ids() {
    let gpu_ids = [0x100, 0x300, 0x900];

    assert_eq!(
        select_gpu_ids(&gpu_ids, &GpuSelector::all()).unwrap(),
        gpu_ids
    );
    assert_eq!(
        select_gpu_ids(
            &gpu_ids,
            &GpuSelector::from_specs(["0".to_string(), "0x300".to_string()])
        )
        .unwrap(),
        vec![0x100, 0x300]
    );
    assert_eq!(
        select_gpu_ids(&gpu_ids, &GpuSelector::from_specs(["768".to_string()])).unwrap(),
        vec![0x300]
    );

    let err = select_gpu_ids(&gpu_ids, &GpuSelector::from_specs(["pu=0".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("did you mean --gpu=0?"));

    assert!(select_gpu_ids(&[], &GpuSelector::all()).is_err());
}

#[test]
fn gpu_type_detection_classifies_consumer_and_datacenter_generations() {
    let cases = [
        (
            "NVIDIA GeForce RTX 5090 Laptop GPU GB203",
            GpuType::Mobile50Series,
        ),
        ("NVIDIA GeForce RTX 5090 GB202", GpuType::Desktop50Series),
        ("NVIDIA GeForce RTX 4090 AD102", GpuType::Desktop40Series),
        (
            "NVIDIA GeForce RTX 4080 Laptop GPU AD104",
            GpuType::Mobile40Series,
        ),
        ("NVIDIA RTX A6000 GA102", GpuType::WorkstationAmpere),
        ("NVIDIA L40 AD102", GpuType::ServerLovelace),
        ("NVIDIA H100 GH100", GpuType::ServerHopper),
        ("NVIDIA Tesla V100 GV100", GpuType::ServerVolta),
        ("NVIDIA GeForce GTX 1080 GP104", GpuType::Desktop10Series),
        (
            "NVIDIA GeForce GTX 980M Laptop GPU GM204",
            GpuType::Mobile9Series,
        ),
    ];

    for (name, expected) in cases {
        assert_eq!(detect_gpu_type(name), expected, "{name}");
    }

    assert_eq!(detect_gpu_type("NVIDIA Experimental GPU"), GpuType::Unknown);
}

#[test]
fn gpu_type_parameter_helpers_cover_special_cases() {
    let mobile_50 = GpuType::Mobile50Series;
    assert!(mobile_50.oc_params().is_50_series);
    assert_eq!(mobile_50.oc_params().testing_step, 5);
    assert!(mobile_50.voltage_limit_params().margin_threshold_check);
    assert!(mobile_50.voltage_lock_params().skew_rate_enabled);
    assert!(mobile_50.is_maxq());

    let desktop_10 = GpuType::Desktop10Series;
    assert!(desktop_10.is_legacy_vfp());
    assert!(!desktop_10.is_legacy_voltage());
    assert_eq!(desktop_10.vfp_point_range(), 79);

    let unknown = GpuType::Unknown;
    assert!(unknown.is_legacy_voltage());
    assert_eq!(unknown.minimum_freq_step_khz(), 15000);
    assert_eq!(unknown.vfp_point_range(), 126);
}
