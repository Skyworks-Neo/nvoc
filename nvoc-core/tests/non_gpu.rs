use nvapi_hi::{ClockDomain, CoolerPolicy, Kilohertz, Microvolts, PState, VfpPoint};
use nvml_wrapper::enum_wrappers::device::PerformanceState;
use nvml_wrapper::enums::device::FanControlPolicy;
use nvoc_core::{
    BackendSet, ConvertEnum, GpuId, GpuOperation, GpuSelector, GpuTarget, GpuType, OperationKind,
    PciAddress, QueryPowerLimits, VfpResetDomain, detect_gpu_type, find_matching_vfp_point,
    nvml_pstate_to_index, nvml_pstate_to_str, parse_nvml_fan_control_policy, parse_nvml_pstate,
    run, select_targets, try_parse_nvml_pstate,
};
use std::collections::BTreeMap;
use std::str::FromStr;

#[test]
fn pstate_parse_forms() {
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
fn pstate_format_roundtrip() {
    for index in 0..=15 {
        let raw = format!("P{index}");
        let pstate = parse_nvml_pstate(&raw).unwrap();

        assert_eq!(nvml_pstate_to_index(pstate).unwrap(), index);
        assert_eq!(nvml_pstate_to_str(pstate), raw);
    }

    assert!(nvml_pstate_to_index(PerformanceState::Unknown).is_err());
    assert_eq!(nvml_pstate_to_str(PerformanceState::Unknown), "Unknown");
}

#[test]
fn vfp_reset_domain_cli_values() {
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
fn convert_enum_values() {
    assert_eq!(PState::from_str("P0").unwrap(), PState::P0);
    assert_eq!(PState::P15.to_str(), "P15");
    assert!(PState::from_str("P16").is_err());

    assert_eq!(
        ClockDomain::from_str("graphics").unwrap(),
        ClockDomain::Graphics
    );
    assert_eq!(ClockDomain::Memory.to_str(), "memory");
    assert!(ClockDomain::from_str("core").is_err());

    assert_eq!(
        CoolerPolicy::from_str("manual").unwrap(),
        CoolerPolicy::Manual
    );
    assert_eq!(CoolerPolicy::TemperatureContinuous.to_str(), "continuous");
    assert!(CoolerPolicy::from_str("automatic").is_err());
}

#[test]
fn fan_policy_aliases() {
    assert_eq!(
        parse_nvml_fan_control_policy("continuous").unwrap(),
        FanControlPolicy::TemperatureContinousSw
    );
    assert_eq!(
        parse_nvml_fan_control_policy("auto").unwrap(),
        FanControlPolicy::TemperatureContinousSw
    );
    assert_eq!(
        parse_nvml_fan_control_policy("manual").unwrap(),
        FanControlPolicy::Manual
    );

    let err = parse_nvml_fan_control_policy("default")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Invalid NVML fan policy"));
}

#[test]
fn gpu_id_selection_ok() {
    let targets: Vec<GpuTarget<'_>> = [0x100u32, 0x300, 0x900]
        .into_iter()
        .enumerate()
        .map(|(i, id)| GpuTarget {
            id: GpuId(id),
            index: i,
            nvapi: None,
            nvml: None,
        })
        .collect();

    let selected = select_targets(&targets, &GpuSelector::all()).unwrap();
    assert_eq!(
        selected.iter().map(|t| t.id.0).collect::<Vec<_>>(),
        vec![0x100, 0x300, 0x900]
    );

    let selected = select_targets(
        &targets,
        &GpuSelector::from_specs(["0".to_string(), "0x300".to_string()]),
    )
    .unwrap();
    assert_eq!(
        selected.iter().map(|t| t.id.0).collect::<Vec<_>>(),
        vec![0x100, 0x300]
    );

    let selected = select_targets(&targets, &GpuSelector::from_specs(["768".to_string()])).unwrap();
    assert_eq!(
        selected.iter().map(|t| t.id.0).collect::<Vec<_>>(),
        vec![0x300]
    );

    let err = select_targets(&targets, &GpuSelector::from_specs(["pu=0".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("did you mean --gpu=0?"));

    assert!(select_targets(&[], &GpuSelector::all()).is_err());
}

#[test]
fn gpu_id_selection_rejects_bad_specs() {
    let targets: Vec<GpuTarget<'_>> = [0x100u32, 0x300]
        .into_iter()
        .enumerate()
        .map(|(i, id)| GpuTarget {
            id: GpuId(id),
            index: i,
            nvapi: None,
            nvml: None,
        })
        .collect();

    let err = select_targets(&targets, &GpuSelector::from_specs(["x".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected a decimal or hex"));

    let err = select_targets(&targets, &GpuSelector::from_specs(["2".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("no GPU matches --gpu 2"));

    let err = select_targets(&targets, &GpuSelector::from_specs(["0x999".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("no GPU matches --gpu 2457"));
}

#[test]
fn gpu_type_detection() {
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
fn gpu_type_params() {
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

#[test]
fn gpu_type_display() {
    assert_eq!(
        GpuType::Desktop40Series.to_string(),
        "40 series desktop detected"
    );
    assert_eq!(GpuType::Unknown.to_string(), "Unknown");
}

#[test]
fn vfp_point_nearest_voltage() {
    let table = BTreeMap::from([
        (
            7,
            VfpPoint {
                default_frequency: Kilohertz(1_800_000),
                frequency: Kilohertz(1_800_000),
                voltage: Microvolts(850_000),
            },
        ),
        (
            8,
            VfpPoint {
                default_frequency: Kilohertz(1_860_000),
                frequency: Kilohertz(1_860_000),
                voltage: Microvolts(900_000),
            },
        ),
    ]);

    let (index, point) = find_matching_vfp_point(&table, Microvolts(880_000)).unwrap();
    assert_eq!(*index, 8);
    assert_eq!(point.voltage, Microvolts(900_000));
    assert!(find_matching_vfp_point(&BTreeMap::new(), Microvolts(880_000)).is_none());
}

#[test]
fn structured_operation_requires_matching_backend() {
    let target = GpuTarget {
        id: GpuId(0x100),
        index: 0,
        nvapi: None,
        nvml: None,
    };

    let err = run(&target, QueryPowerLimits).unwrap_err().to_string();
    assert!(err.contains("has no NVML backend"));
    assert_eq!(QueryPowerLimits.kind(), OperationKind::QueryPowerLimits);
    assert_eq!(BackendSet::Both, BackendSet::Both);
}

#[test]
fn pci_address_parses_standard_and_nvapi_forms() {
    let standard = PciAddress::from_str("0000:01:00.0").unwrap();
    assert_eq!(standard.domain, 0);
    assert_eq!(standard.bus, 1);
    assert_eq!(standard.device, 0);
    assert_eq!(standard.function, 0);
    assert_eq!(standard.to_string(), "0000:01:00.0");
    assert_eq!(GpuId::from_pci_address(standard), GpuId(0x100));

    let nvapi = PciAddress::from_str("PCIe x16 (1:0 routed to IRQ 0)").unwrap();
    assert_eq!(nvapi.bus, 1);
    assert_eq!(nvapi.device, 0);
    assert_eq!(
        GpuId::from_pci_str("PCIe x16 (1:0 routed to IRQ 0)").unwrap(),
        GpuId(0x100)
    );
}

#[test]
fn gpu_id_round_trips_pci_bus() {
    let id = GpuId::from_pci_bus(3);
    assert_eq!(id, GpuId(0x300));
    assert_eq!(id.pci_bus(), 3);
    assert!(GpuId::from_pci_str("invalid-pci-id").is_err());
}
