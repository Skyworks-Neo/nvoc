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

pub use self::nvapi::{
    CoolerTarget, GpuTdpTempLimits, VfpLockRequest, nvapi_overvolt_reported, set_nvapi_overvolt,
};
pub use ::nvapi::sys::gpu::power::undocumented::Wm2AcousticMode;
pub use ::nvapi::{P0VoltageBounds, VoltRails};
pub use conv::ConvertEnum;
pub use error::Error;
pub use gpu::GpuSelector;
pub use gpu_type::{
    ArchOcPrior, GpuOcParams, GpuType, GpuVoltageLimitParams, GpuVoltageLockParams, OcPriorPoint,
};
pub use operation::{
    CheckVoltageFrequency, ClearEdid, DisableNvapiThermalSim, GetFanCurves, GetPowerMode,
    GpuOperation, OemOcScanner, OemOcScannerAction, ProbeVoltageLimits, QueryApiRestriction,
    QueryAutoBoost, QueryClockOffset, QueryDisplays, QueryDomainVfpIndices, QueryDomainVfpPoints,
    QueryEdid, QueryFanInfo, QueryGpuInfo, QueryGpuSettings, QueryGpuStatus,
    QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta, QueryNvapiClkDomainFreq,
    QueryNvapiClkDomainFreqDetail, QueryNvapiClkDomainFreqDirect, QueryNvapiClkDomainFreqsBatch,
    QueryNvapiClkDomains, QueryNvapiClkVfControl, QueryNvapiClkVfPoints, QueryNvapiCoolerInfo,
    QueryNvapiCoreVoltageControl, QueryNvapiDNotifier, QueryNvapiOcScannerIncomplete,
    QueryNvapiPStateLevels, QueryNvapiPStateLockStatus, QueryNvapiPmgrVoltageArbiter,
    QueryNvapiRatedTdp, QueryNvapiTargetTempPolicies, QueryNvapiTargetTempPolicyIndex,
    QueryNvapiTgpWattRange, QueryNvapiThermalSettings, QueryNvapiThermalSim, QueryNvapiVoltRails,
    QueryPowerLimits, QueryPstateBaseVoltage, QueryPstates, QuerySupportedApplicationsClocks,
    QueryTdpTempLimits, QueryTemperatureThresholds, QueryThrottleReasons, QueryVfpPointVoltage,
    QueryViolationStatus, QueryVoltageBoost, ResetAutoboostStatus, ResetCoolerLevels,
    ResetFanCurve, ResetFanSpeed, ResetForcePstate, ResetFreqLock, ResetLegacyApplicationFreqLock,
    ResetLegacyGpcRailOvervoltLimit, ResetNvapiPowerLimits, ResetNvapiSensorLimits,
    ResetNvapiTgpWatt, ResetNvapiVfpPrivate, ResetPstateGlobalFreqOffset,
    ResetPublicVftableGpcLock, ResetPublicVftableOffset, ResetVfpFrequencyLock,
    RestartDisplayDriver, SetApplicationsClocks, SetAutoboostStatus, SetAutoboostSupport,
    SetBb2Active, SetClockOffset, SetCoolerLevels, SetDomainVfpDeltas, SetEdid, SetFanCurve,
    SetFanRpm, SetFanSpeed, SetFanStop, SetForcePstate, SetGpcVoltLock, SetLegacyClocks,
    SetLockedClocks, SetNvapiBackgroundOcScanner, SetNvapiClkDomainOffset,
    SetNvapiCoreVoltageControl, SetNvapiDNotifier, SetNvapiDynamicBoost, SetNvapiOvervolt,
    SetNvapiPStateNative, SetNvapiPerfFreqCap, SetNvapiPerfLevelLock, SetNvapiPmgrVoltageArbiter,
    SetNvapiPowerLimits, SetNvapiPstateLock, SetNvapiSensorLimits, SetNvapiTargetTemp,
    SetNvapiTgpWatt, SetNvapiThermalSim, SetNvapiVfpPointPrivate, SetNvapiVfpRangePerPointPrivate,
    SetNvapiVfpRangePrivate, SetNvapiVoltRailOffset, SetNvapiVoltRailTarget, SetNvmlAcousticTemp,
    SetNvmlPstateLock, SetPowerLimit, SetPowerMode, SetPstateBaseVoltage, SetPstateClockOffset,
    SetPublicVftablePointOffset, SetPublicVftableRangeOffset, SetTemperatureLimit,
    SetVfpFrequencyLock, SetVoltageBoost, SetWm2Active, SetWm2Mode, TgpWattRangeInfo,
    detect_gpu_type, fetch_gpu_type, find_matching_vfp_point, legacy_core_overvolt_ranges,
    legacy_p0_core_max_voltage_delta, nvml_pstate_to_index, nvml_pstate_to_str,
    parse_nvapi_locked_voltage_target, parse_nvml_fan_control_policy, parse_nvml_pstate,
    query_domain_vf_points_indexed, query_domain_vfp_indices, run, run_many,
    set_nvapi_cooler_settings, set_nvapi_domain_vfp_deltas, set_nvapi_legacy_clocks,
    set_nvapi_pstate_clock_offsets, set_nvapi_vfp_curve_delta, sync_memory_pstate_as_p0,
    try_parse_nvml_pstate,
};
pub use result::{
    ApiRestrictionState, AppliedValue, AutoBoostState, BatchReport, ClockOffset, DNotifierInfo,
    DNotifierLevel, DisplayInfo, EdidData, FanCurvePointReadout, FanCurveReadout, FanInfo,
    NvapiCoolerInfoEntry, NvapiFanRpmResult, NvapiPStateNativeLock, NvapiPerfFreqCap,
    OperationKind, OperationReport, OperationWarning, PStateLevelEntry, PStateLevelsInfo,
    PowerLimits, PowerModeStatus, PstateBaseVoltage, PstateClockRange, SupportedApplicationClocks,
    TargetOutcome, TargetTempPolicy, TdpTempLimits, TemperatureThreshold, ThermalSensorReading,
    ThrottleReason, ViolationEntry, ViolationStatusReport, VoltageBoostState,
    VoltageFrequencyCheck, VoltageLimits,
};
pub use target::{
    BackendSet, GpuId, GpuTarget, PciAddress, TargetInventory, discover_targets,
    gpu_id_from_nvml_device, pci_address_from_nvml_device, select_targets,
};
pub use types::{NvapiLockedVoltageTarget, VfpResetDomain};

pub use ::nvapi::hi::{
    Celsius, ClkVfControlPointPrivate, ClkVfControlPrivate, ClkVfDomainClass, ClkVfDomainHint,
    ClkVfPointPrivate, ClkVfPointsPrivate, ClkVfSegmentKind, ClockDomain, CoolerControl,
    CoolerPolicy, CoolerSettings, DisplayId, FanCoolerId, GpuInfo, GpuSettings, GpuStatus,
    Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta, PState, Percentage, SensorThrottle,
    VfPoint, VfPointType, VoltageDomain,
};
pub use ::nvapi::{clk_vf_delta_for_target, clk_vf_effect_for_delta};
