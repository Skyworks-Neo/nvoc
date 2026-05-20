mod conv;
mod error;
mod gpu;
mod gpu_type;
mod nvapi;
mod nvml;
pub mod operation;
pub mod result;
pub mod target;
mod types;

pub use conv::ConvertEnum;
pub use error::{Error, is_allowable_nvapi_reset_error};
pub use gpu::GpuSelector;
pub use gpu_type::{GpuOcParams, GpuType, GpuVoltageLimitParams, GpuVoltageLockParams};
pub use nvapi::{CoolerTarget, GpuTdpTempLimits, VfpLockRequest};
pub use operation::{
    CheckVoltageFrequency, GpuOperation, ProbeVoltageLimits, QueryClockOffset,
    QueryDomainVfpIndices, QueryDomainVfpPoints, QueryFanInfo, QueryGpuInfo, QueryGpuSettings,
    QueryGpuStatus, QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta,
    QueryPowerLimits, QueryPstates, QuerySupportedApplicationsClocks, QueryTdpTempLimits,
    QueryTemperatureThresholds, QueryVfpPointVoltage, ResetApplicationsClocks, ResetCoolerLevels,
    ResetFanSpeed, ResetLockedClocks, ResetNvapiPowerLimits, ResetNvapiSensorLimits,
    ResetPstateBaseVoltages, ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock,
    ResetVfpLock, SetApplicationsClocks, SetClockOffset, SetCoolerLevels, SetDomainVfpDeltas,
    SetFanSpeed, SetLegacyClocks, SetLockedClocks, SetNvapiPowerLimits, SetNvapiPstateLock,
    SetNvapiSensorLimits, SetNvmlPstateLock, SetPowerLimit, SetPstateBaseVoltage,
    SetPstateClockOffset, SetTemperatureLimit, SetVfpFrequencyLock, SetVfpPointDelta,
    SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, check_nvapi_voltage_frequency,
    detect_gpu_type, fetch_gpu_type, find_matching_vfp_point, legacy_core_overvolt_ranges,
    legacy_p0_core_max_voltage_delta, nvml_pstate_to_index, nvml_pstate_to_str,
    parse_nvapi_locked_voltage_target, parse_nvml_fan_control_policy, parse_nvml_pstate,
    probe_nvapi_voltage_limits, query_domain_vf_points_indexed, query_domain_vfp_indices,
    query_gpu_info, query_gpu_settings, query_gpu_status, query_nvapi_tdp_temp_limits,
    query_nvapi_vfp_point_voltage, reset_all_nvapi_pstate_base_voltages,
    reset_all_nvapi_vfp_deltas, reset_nvapi_cooler_levels, reset_nvapi_vfp_deltas,
    reset_nvapi_vfp_frequency_lock, reset_nvapi_vfp_lock, run, run_many, set_nvapi_cooler_levels,
    set_nvapi_cooler_settings, set_nvapi_domain_vfp_deltas, set_nvapi_gpu_pstate_lock,
    set_nvapi_legacy_clocks, set_nvapi_power_limits, set_nvapi_power_limits_to_default,
    set_nvapi_pstate_base_voltage, set_nvapi_pstate_clock_offset_preserve,
    set_nvapi_pstate_clock_offsets, set_nvapi_sensor_limits, set_nvapi_sensor_limits_to_default,
    set_nvapi_vfp_curve_delta, set_nvapi_vfp_frequency_lock, set_nvapi_vfp_lock,
    set_nvapi_vfp_point_delta, set_nvapi_vfp_voltage_lock, set_nvapi_voltage_boost,
    try_parse_nvml_pstate,
};
pub use result::{
    AppliedValue, BatchReport, ClockOffset, FanInfo, OperationKind, OperationReport,
    OperationWarning, PowerLimits, PstateClockRange, SupportedApplicationClocks, TargetOutcome,
    TdpTempLimits, TemperatureThreshold, VoltageFrequencyCheck, VoltageLimits,
};
pub use target::{
    BackendSet, GpuId, GpuTarget, PciAddress, TargetInventory, discover_targets,
    gpu_id_from_nvapi_gpu, gpu_id_from_nvml_device, pci_address_from_nvml_device, select_targets,
};
pub use types::{NvapiLockedVoltageTarget, VfpResetDomain};

pub use nvapi_hi::{
    Celsius, ClockDomain, CoolerControl, CoolerPolicy, CoolerSettings, FanCoolerId, GpuInfo,
    GpuSettings, GpuStatus, Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta, PState,
    Percentage, SensorThrottle, VfPoint,
};
