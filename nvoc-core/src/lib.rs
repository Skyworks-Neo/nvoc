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
pub use error::Error;
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
    SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, detect_gpu_type, fetch_gpu_type,
    find_matching_vfp_point, legacy_core_overvolt_ranges, legacy_p0_core_max_voltage_delta,
    nvml_pstate_to_index, nvml_pstate_to_str, parse_nvapi_locked_voltage_target,
    parse_nvml_fan_control_policy, parse_nvml_pstate, query_domain_vf_points_indexed,
    query_domain_vfp_indices, run, run_many, set_nvapi_cooler_settings,
    set_nvapi_domain_vfp_deltas, set_nvapi_legacy_clocks, set_nvapi_pstate_clock_offsets,
    set_nvapi_vfp_curve_delta, try_parse_nvml_pstate,
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
