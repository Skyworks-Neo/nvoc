use super::{Error, target::GpuId};

/// Logical operation requested through the structured GPU operation API.
///
/// This identifies the high-level operation exposed to callers in
/// [`OperationReport`] and [`BatchReport`]. It does not always name the lowest
/// level driver primitive used to implement the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    QueryGpuInfo,
    QueryGpuSettings,
    QueryGpuStatus,
    QueryPowerLimits,
    SetPowerLimit,
    QueryTemperatureThresholds,
    SetTemperatureLimit,
    QueryPstates,
    QuerySupportedApplicationsClocks,
    QueryClockOffset,
    QueryPstateBaseVoltage,
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
    ResetVfpLock,
    SetVfpPointDelta,
    SetVfpRangeDelta,
    SetDomainVfpDeltas,
    QueryDomainVfpPoints,
    QueryDomainVfpIndices,
    QueryLegacyCoreOvervoltRanges,
    QueryLegacyP0CoreMaxVoltageDelta,
    QueryVoltageBoost,
    SetVoltageBoost,
    SetNvapiPowerLimits,
    SetNvapiSensorLimits,
    SetNvapiDynamicBoost,
    SetNvapiTgpWatt,
    ResetNvapiTgpWatt,
    QueryNvapiTgpWattRange,
    QueryNvapiTargetTempPolicies,
    QueryNvapiTargetTempPolicyIndex,
    SetNvapiTargetTemp,
    QueryNvapiDNotifier,
    SetNvapiDNotifier,
    QueryNvapiPStateLevels,
    QueryNvapiPStateLockStatus,
    ResetNvapiPowerLimits,
    ResetNvapiSensorLimits,
    ResetCoolerLevels,
    ResetPstateClockOffsets,
    QueryTdpTempLimits,
    ProbeVoltageLimits,
    CheckVoltageFrequency,
    SetLegacyClocks,
    /// Lock one NVML P-State or a contiguous range through the NVAPI path.
    ///
    /// The implementation derives a memory VFP frequency window from the
    /// requested P-State memory clock ranges and applies that window with
    /// NVAPI. This remains distinct from a caller directly requesting
    /// [`OperationKind::SetVfpFrequencyLock`].
    SetNvapiPstateLock,
    /// Lock one NVML P-State or a contiguous range through the NVML path.
    ///
    /// The implementation derives a memory clock window from the requested
    /// P-State memory clock ranges and applies it with NVML locked clocks. This
    /// remains distinct from a caller directly requesting
    /// [`OperationKind::SetLockedClocks`].
    SetNvmlPstateLock,
    SetAutoBoost,
    SetAutoBoostDefault,
    QueryAutoBoost,
    SetApiRestriction,
    QueryApiRestriction,
    QueryDisplays,
    QueryEdid,
    SetEdid,
    ClearEdid,
    QueryThrottleReasons,
    QueryViolationStatus,
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

/// One entry of the NVAPI target-temperature (温度墙) policy table, read via the
/// private ClientThermalTarget GET-prime (0xC4554575). `policy_index` is the
/// slot in the driver's policy table; on RTX 4060 Laptop index 2 is the "GPU
/// Target Temperature" wall (matches nvidia-smi and NVML's GpsCurr channel).
/// Used by `get-temp-thresholds --nvapi` for per-GPU index discovery.
///
/// `current` is the live value; `min`/`default`/`max` are the VBIOS range from
/// the private ClientThermalPolicies GetInfo (0x2F69F8E5), None when the driver
/// didn't expose that slot's range.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetTempPolicy {
    pub policy_index: usize,
    /// Live current target temp (celsius).
    pub celsius: f32,
    /// VBIOS minimum (writable floor), if known.
    pub min: Option<f32>,
    /// VBIOS rated/default, if known.
    pub default: Option<f32>,
    /// VBIOS maximum (writable ceiling), if known.
    pub max: Option<f32>,
}

/// One D-Notifier (D0-notify / "extern power state") level: the D level number
/// (1..5) and the power cap it imposes when active. `None` power means
/// "Unlimited" (only ever true for D1). RE'd from GPUMon `[GPUHandle::
/// pollDNotifyLimit]` ("D{n}({power}mW)" string); mW values cross-checked live
/// on RTX 4060 Laptop (D2=55W, D3=45W, D4=33W, D5=10W, D1=Unlimited).
#[derive(Debug, Clone, PartialEq)]
pub struct DNotifierLevel {
    /// D level number, 1..5 (D1..D5). Render as `format!("D{}", level)`.
    pub level: u8,
    /// Power cap in **watts** when this level is active; `None` = Unlimited (D1).
    pub watts: Option<f64>,
}

/// D-Notifier current state read via the private ClientPowerPoliciesGetInfo
/// (0x67F31384): the active D level plus the full D1..D5 power-cap table.
/// Note the D-Notifier cap and the TGP-watts wall share the same power-policy
/// table, so the effective power limit is the SMALLER of the two — that is why
/// setting D-Notifier too low silently clamps the TGP wall.
#[derive(Debug, Clone, PartialEq)]
pub struct DNotifierInfo {
    /// The currently-active D level (None when driver reports N/A).
    pub active: Option<u8>,
    /// The D1..D5 power-cap table (always 5 entries, in D1→D5 order).
    pub levels: Vec<DNotifierLevel>,
}

/// One P-State entry from the native PerfPstatesGetInfo table (`0x7B30AE0D`):
/// the pstate number and its min/max core clock in **MHz** (converted from the
/// driver's kHz for ergonomic CLI output). RE'd from GPUMon's `queryPStateInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct PStateLevelEntry {
    /// P-State number, 0..31 (e.g. 0 for P0).
    pub pstate: u8,
    /// Min core clock (MHz), if the driver exposed it.
    pub min_mhz: Option<f64>,
    /// Max core clock (MHz), if the driver exposed it.
    pub max_mhz: Option<f64>,
}

/// Native P-State level table (the GPUMon `-pstate` GET listing), read via the
/// private PerfPstatesGetInfo (`0x7B30AE0D`). Distinct from the NVML P-State
/// table — this is the driver's native P*.Max/P*.Min index used by GPUMon's
/// `-pstate:<index>` SETTER (index 0 = P0.TDP/rated, then per pstate a .Max
/// then .Min slot).
#[derive(Debug, Clone, PartialEq)]
pub struct PStateLevelsInfo {
    /// Present P-States in ascending order with min/max core clock (MHz).
    pub pstates: Vec<PStateLevelEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockOffset {
    pub mhz: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PstateBaseVoltage {
    pub pstate: nvapi_hi::PState,
    pub voltage_domain: nvapi_hi::VoltageDomain,
    pub editable: bool,
    pub voltage: nvapi_hi::Microvolts,
    pub delta: nvapi_hi::MicrovoltsDelta,
    pub min_delta: nvapi_hi::MicrovoltsDelta,
    pub max_delta: nvapi_hi::MicrovoltsDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoltageBoostState {
    pub voltage_boost: Option<nvapi_hi::Percentage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoBoostState {
    pub enabled: bool,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiRestrictionState {
    pub api_type: nvml_wrapper::enum_wrappers::device::Api,
    pub restricted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdidData {
    pub display_id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub display_id: u32,
    pub connector: String,
    pub flags_bits: u32,
    pub connected: bool,
    pub physically_connected: bool,
    pub active: bool,
    pub os_visible: bool,
    pub dynamic: bool,
    pub mst_root: bool,
    pub wireless: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdpTempLimits {
    pub min_tdp: nvapi_hi::Percentage,
    pub default_tdp: nvapi_hi::Percentage,
    pub max_tdp: nvapi_hi::Percentage,
    pub min_temp: nvapi_hi::Celsius,
    pub default_temp: nvapi_hi::Celsius,
    pub max_temp: nvapi_hi::Celsius,
    pub throttle_curve: nvapi_hi::PffCurve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoltageFrequencyCheck {
    pub precise: bool,
    pub matched_point: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleReason {
    pub name: String,
    pub active: bool,
}

/// One NVML performance-policy violation entry.
///
/// `violation_time_ns` is the cumulative time the GPU has spent in this
/// policy's violation state, measured by the driver hardware. `name` is the
/// short human label (e.g. `"Pwr"`, `"Idle"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationEntry {
    pub name: String,
    pub violation_time_ns: u64,
}

/// Aggregated NVML violation status across all performance policies.
///
/// `reference_time_us` is the driver's reference timestamp (a Unix epoch
/// microsecond stamp) marking when the cumulative violation counters started;
/// callers format it as the "Since" wall-clock time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationStatusReport {
    pub entries: Vec<ViolationEntry>,
    pub reference_time_us: u64,
}
