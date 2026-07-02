mod conv;
mod error;
mod gpu;
mod gpu_type;
mod nvapi;
pub mod nvml;
pub mod operation;
pub mod result;
pub mod target;
mod types;

pub use conv::ConvertEnum;
pub use error::Error;
pub use gpu::GpuSelector;
pub use gpu_type::{
    ArchOcPrior, GpuOcParams, GpuType, GpuVoltageLimitParams, GpuVoltageLockParams, OcPriorPoint,
};
pub use nvapi::{CoolerTarget, GpuTdpTempLimits, ThermalSensors, VfpLockRequest};
pub use operation::{
    CheckVoltageFrequency, ClearEdid, GpuOperation, ProbeVoltageLimits, QueryApiRestriction,
    QueryAutoBoost, QueryClockOffset, QueryDisplays, QueryDomainVfpIndices, QueryDomainVfpPoints,
    QueryEdid, QueryFanInfo, QueryGpuInfo, QueryGpuSettings, QueryGpuStatus,
    QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta, QueryPowerLimits,
    QueryPstateBaseVoltage, QueryPstates, QuerySupportedApplicationsClocks, QueryTdpTempLimits,
    QueryTemperatureThresholds, QueryThrottleReasons, QueryVfpPointVoltage, QueryViolationStatus,
    QueryVoltageBoost,
    ResetApplicationsClocks, ResetCoolerLevels, ResetFanSpeed, ResetLockedClocks,
    ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetPstateBaseVoltages,
    ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock, ResetVfpLock,
    SetApiRestriction, SetApplicationsClocks, SetAutoBoost, SetAutoBoostDefault, SetClockOffset,
    SetCoolerLevels, SetDomainVfpDeltas, SetEdid, SetFanSpeed, SetLegacyClocks, SetLockedClocks,
    SetNvapiPowerLimits, SetNvapiPstateLock, SetNvapiSensorLimits, SetNvmlPstateLock,
    SetPowerLimit, SetPstateBaseVoltage, SetPstateClockOffset, SetTemperatureLimit,
    SetVfpFrequencyLock, SetVfpPointDelta, SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost,
    detect_gpu_type, fetch_gpu_type, find_matching_vfp_point, legacy_core_overvolt_ranges,
    legacy_p0_core_max_voltage_delta, nvml_pstate_to_index, nvml_pstate_to_str,
    parse_nvapi_locked_voltage_target, parse_nvml_fan_control_policy, parse_nvml_pstate,
    probe_thermal_sensors_mask, query_domain_vf_points_indexed, query_domain_vfp_indices,
    read_thermal_sensors, run, run_many, set_nvapi_cooler_settings, set_nvapi_domain_vfp_deltas,
    set_nvapi_legacy_clocks, set_nvapi_pstate_clock_offsets, set_nvapi_vfp_curve_delta,
    sync_memory_pstate_as_p0, try_parse_nvml_pstate,
};
pub use result::{
    ApiRestrictionState, AppliedValue, AutoBoostState, BatchReport, ClockOffset, DisplayInfo,
    EdidData, FanInfo, OperationKind, OperationReport, OperationWarning, PowerLimits,
    PstateBaseVoltage, PstateClockRange, SupportedApplicationClocks, TargetOutcome, TdpTempLimits,
    TemperatureThreshold, ThrottleReason, ViolationEntry, ViolationStatusReport, VoltageBoostState,
    VoltageFrequencyCheck, VoltageLimits,
};
pub use target::{
    BackendSet, GpuId, GpuTarget, PciAddress, TargetInventory, discover_targets,
    gpu_id_from_nvml_device, pci_address_from_nvml_device, select_targets,
};
pub use types::{NvapiLockedVoltageTarget, VfpResetDomain};

pub use nvapi_hi::{
    Celsius, ClockDomain, CoolerControl, CoolerPolicy, CoolerSettings, DisplayId, FanCoolerId,
    GpuInfo, GpuSettings, GpuStatus, Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta,
    PState, Percentage, SensorThrottle, VfPoint, VfPointType, VoltageDomain,
};
