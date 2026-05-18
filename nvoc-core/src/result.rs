use super::{Error, target::GpuId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    QueryPowerLimits,
    SetPowerLimit,
    QueryTemperatureThresholds,
    SetTemperatureLimit,
    QueryPstates,
    QuerySupportedApplicationsClocks,
    QueryClockOffset,
    SetClockOffset,
    SetApplicationsClocks,
    ResetApplicationsClocks,
    SetLockedClocks,
    ResetLockedClocks,
    QueryFanInfo,
    SetFanSpeed,
    ResetFanSpeed,
    SetPstateBaseVoltage,
    ResetPstateBaseVoltages,
    SetPstateClockOffset,
    SetCoolerLevels,
    QueryVfpPointVoltage,
    SetVfpFrequencyLock,
    ResetVfpFrequencyLock,
    SetVfpVoltageLock,
    ResetVfpDeltas,
    SetVfpPointDelta,
    SetVfpRangeDelta,
    QueryTdpTempLimits,
    ProbeVoltageLimits,
    CheckVoltageFrequency,
    SetLegacyClocks,
    SetNvapiPstateLock,
    SetNvmlPstateLock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationWarning {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationReport<T> {
    pub target: GpuId,
    pub operation: OperationKind,
    pub output: T,
    pub warnings: Vec<OperationWarning>,
}

#[derive(Debug)]
pub struct BatchReport<T> {
    pub operation: OperationKind,
    pub outcomes: Vec<TargetOutcome<T>>,
}

#[derive(Debug)]
pub enum TargetOutcome<T> {
    Ok(OperationReport<T>),
    Err { target: GpuId, error: Error },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerLimits {
    pub min_watts: f32,
    pub current_watts: f32,
    pub max_watts: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemperatureThreshold {
    pub name: &'static str,
    pub celsius: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockOffset {
    pub mhz: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PstateClockRange {
    pub pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
    pub min_core_mhz: u32,
    pub max_core_mhz: u32,
    pub min_memory_mhz: u32,
    pub max_memory_mhz: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedApplicationClocks {
    pub memory_mhz: u32,
    pub graphics_mhz: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanInfo {
    pub count: u32,
    pub min_speed: Option<u32>,
    pub max_speed: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedValue<T> {
    pub requested: T,
    pub applied: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoltageLimits {
    pub lower_point: usize,
    pub upper_point: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TdpTempLimits {
    pub min_tdp: nvapi_hi::Percentage,
    pub default_tdp: nvapi_hi::Percentage,
    pub max_tdp: nvapi_hi::Percentage,
    pub min_temp: nvapi_hi::Celsius,
    pub default_temp: nvapi_hi::Celsius,
    pub max_temp: nvapi_hi::Celsius,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoltageFrequencyCheck {
    pub precise: bool,
    pub matched_point: Option<usize>,
}
