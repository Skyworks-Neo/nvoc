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
    /// Set the NVML acoustic target temperature (`ACOUSTIC_CURR` threshold) —
    /// the Linux-native target-temp channel. Windows rejects the NVML
    /// threshold setter family; see `SetNvmlAcousticTemp`.
    SetNvmlAcousticTemp,
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
    QueryNvapiThermalSettings,
    GetPowerMode,
    SetPowerMode,
    SetNvapiPowerLevel,
    SetNvapiOvervolt,
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
    QueryNvapiVoltRails,
    SetNvapiVoltRailOffset,
    /// Set a volt-rail to an absolute target voltage (mV) by deriving the
    /// required µV offset from the live control/status snapshot. Shares the
    /// melonVolt write path with [`OperationKind::SetNvapiVoltRailOffset`].
    SetNvapiVoltRailTarget,
    /// Query the controllable clock-domain block (private ClockClient
    /// GetControl, RM 0x2080901b) — mask + per-domain type/range/offset.
    /// The Blackwell XBar family (reverse/melonvolt/xbar.txt).
    QueryNvapiClkDomains,
    /// Measure one domain's physical clock (private ClockClient
    /// MEASURE_FREQ, RM 0x20809006) via two-sample Δcounter/Δtimestamp.
    QueryNvapiClkDomainFreq,
    /// Write a signed kHz offset into one clock-domain's control record
    /// (private ClockClient SET_CONTROL, RM 0x2080d01c). DANGEROUS GPU clock
    /// write: snapshots the full GetControl block, version-gates (magic
    /// 0x10964), patches a copy, SETs, readbacks, restores on mismatch;
    /// `temporary` restores the snapshot before returning.
    SetNvapiClkDomainOffset,
    /// Query the private ClockClient V/F-POINTS read path (GetInfo 0x8895B510
    /// → GetStatus 0x7FEE9032) — per-bank point masks + V/F curve records
    /// (units calibrated vs the public GPC VFP curve).
    QueryNvapiClkVfPoints,
    /// Write one V/F curve point via the private V/F-POINTS SetControl
    /// (ID 0xFEC00D04, mode 0 absolute / mode 1 delta). DANGEROUS:
    /// snapshots, patches, SETs, readbacks, restores on mismatch.
    SetNvapiVfpPointPrivate,
    /// Write a range of V/F curve points with the same delta via the
    /// private SetControl (single RMW cycle).
    SetNvapiVfpRangePrivate,
    /// Reset every present V/F curve point on a bank by clearing its
    /// mode-0 override via the private SetControl (single RMW cycle).
    ResetNvapiVfpPrivate,
    /// Batch-measure physical clocks for a set of domains (V3
    /// MEASURE_FREQ, one RM round-trip per sample for the whole set).
    QueryNvapiClkDomainFreqDetail,
    QueryNvapiClkDomainFreqsBatch,
    QueryNvapiPStateLevels,
    QueryNvapiPStateLockStatus,
    SetNvapiPStateNative,
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
    /// Control NVIDIA's driver-side ("OEM") OC Scanner — the family MSI's
    /// MSIOCScanner drives on drivers >= 455.00 (Start 0xBC4AEE25 /
    /// Stop 0xC28B73DE / Revert 0xCC727B22). The scan runs inside the
    /// driver; the resulting V/F offsets are applied by the driver itself.
    OemOcScanner,
    /// Force a P-State via the private SetForcePstate (0x025BFB10).
    SetForcePstate,
    /// Release a force-locked pstate via EnableDynamicPstates(enable=0).
    /// SetForcePstate has no unlock path of its own; this is the escape hatch.
    ResetForcePstate,
    /// Restart the display driver (0xB4B26B65) — legacy "apply OC" trigger.
    RestartDisplayDriver,
    /// Battery Boost 2.0 enable/disable (0xD27D0629). Mobile-only.
    SetBb2Active,
    /// Whisper Mode 2.0 enable/disable (0xD27D0629). Mobile-only.
    SetWm2Active,
    /// Whisper Mode 2.0 acoustic mode (0xD27D0629). Mobile-only.
    SetWm2Mode,
    /// Set the GPU frequency perf-cap (PerfLimitsSetStatus NDA 0x32CA4983) —
    /// clamp the perf max/min frequency to a cap value. The ref tool's
    /// `-gpuclk:<MHz>`. Distinct from P-state lock (SetNvapiPStateNative).
    SetNvapiPerfFreqCap,
    /// Read the GPU fan-curve table (ClientFanPoliciesGetControl NDA
    /// 0xE543C540, struct magic 0x200DC) — up to 4 curve slots × 3
    /// monotonic (temperature, RPM) points. Desktop-only (mobile drives
    /// fans through the EC).
    GetFanCurves,
    /// Write one fan-curve slot (ClientFanPoliciesSetControl NDA 0xC181947A,
    /// struct magic 0x200DC) via the GPUMon RMW protocol — GET snapshot,
    /// patch the target slot, SET the whole table back. Desktop-only.
    SetFanCurve,
    /// Reset one fan-curve slot to factory (FanPolicySetControl NDA
    /// 0x2B2A2A45, struct magic 0x214AC): GET the policy block, OR
    /// `1 << curve_index` into the +0x08 reset bitmask, SET. This is
    /// GPUMon's NVAPI fan reset — works where the public
    /// RestoreCoolerSettings is rejected with NOT_SUPPORTED (desktop
    /// 3060/2070).
    ResetFanCurve,
    /// Toggle fan stop / zero-RPM for a curve slot (FanArbiterSet NDA
    /// 0x44CD3014, struct magic 0x10144, enable bit0 at +0x28).
    SetFanStop,
    /// Query per-cooler info via the private FanCoolerGetInfo (NDA
    /// 0x65CE5BFC): cooler count + per-cooler index.
    QueryNvapiCoolerInfo,
    /// Set fan speed by RPM via the private FanCoolerSetControl (NDA
    /// 0xEB44E8AA): RMW the control block, patch enable+level per cooler
    /// type. RE'd from GPUMon setFanSim.
    SetFanRpm,
}

impl OperationKind {
    /// Whether this operation *writes* GPU state through NVAPI (set/reset/lock).
    ///
    /// Used to decide whether to pre-wake the dGPU via `force_gc6_exit` on
    /// mobile platforms (610+ drivers aggressively enter GC6/GCOFF at idle,
    /// causing NVAPI writes to fail with `GpuNotPowered` -220). Query/read
    /// operations are excluded so that "GPU not powered" remains a visible,
    /// truthful state for monitoring; NVML-only writes are handled separately
    /// (they don't go through the NVAPI path that GCOFF blocks).
    pub fn is_nvapi_write(self) -> bool {
        use OperationKind::*;
        matches!(
            self,
            SetPowerLimit
                | SetTemperatureLimit
                | SetNvmlAcousticTemp
                | SetClockOffset
                | SetApplicationsClocks
                | ResetApplicationsClocks
                | SetLockedClocks
                | ResetLockedClocks
                | SetFanSpeed
                | ResetFanSpeed
                | SetPstateBaseVoltage
                | SetNvapiOvervolt
                | ResetPstateBaseVoltages
                | SetPstateClockOffset
                | SetCoolerLevels
                | ResetCoolerLevels
                | SetVfpFrequencyLock
                | ResetVfpFrequencyLock
                | SetVfpVoltageLock
                | ResetVfpDeltas
                | ResetVfpLock
                | SetVfpPointDelta
                | SetVfpRangeDelta
                | SetDomainVfpDeltas
                | SetVoltageBoost
                | SetNvapiPowerLimits
                | SetNvapiSensorLimits
                | SetNvapiDynamicBoost
                | SetNvapiTgpWatt
                | ResetNvapiTgpWatt
                | SetNvapiTargetTemp
                | SetNvapiDNotifier
                | SetNvapiPStateNative
                | SetNvapiVoltRailOffset
                | SetNvapiVoltRailTarget
                | SetNvapiClkDomainOffset
                | SetFanCurve
                | ResetFanCurve
                | SetFanStop
                | SetFanRpm
                | ResetNvapiPowerLimits
                | ResetNvapiSensorLimits
                | ResetPstateClockOffsets
                | SetLegacyClocks
                | SetNvapiPstateLock
                | SetAutoBoost
                | SetAutoBoostDefault
                | SetApiRestriction
                | SetEdid
                | ClearEdid
                | OemOcScanner
                | SetForcePstate
                | ResetForcePstate
                | RestartDisplayDriver
                | SetBb2Active
                | SetWm2Active
                | SetWm2Mode
                | SetNvapiPerfFreqCap
        )
    }
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

/// One temperature→RPM point of a GPU fan curve (`ClientFanPolicies` table,
/// struct magic `0x200DC`, RE'd from GPUMon `DialogFanCurve`). Desktop-only:
/// mobile boards drive fans through the EC, not NVAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanCurvePointReadout {
    pub temp_c: u16,
    pub rpm: u32,
}

/// One fan-curve slot as reported by the driver. The 0x200DC table holds up
/// to 4 slots; `count` (the table's first byte after the magic) is the
/// authoritative number of populated curves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCurveReadout {
    pub index: u8,
    pub points: Vec<FanCurvePointReadout>,
}

/// NVIDIA App power-mode view (均衡/高性能): the App's Balanced/Max toggle via
/// the ClientPowerModes family (0xF21C2D56/0x180A9468/0x3CC8C552, RE'd
/// from NVIDIA App nvxdapix). `supported` mirrors the App's gate
/// (`max_mode_idx == 1`); false on e.g. Ada mobile (0xFFFF).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerModeStatus {
    pub supported: bool,
    /// "Balanced" | "Max" (valid when supported).
    pub active: &'static str,
    /// Raw driver fields for diagnostics: (mode_mask, max_mode_idx).
    pub mode_mask: u16,
    pub max_mode_idx: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemperatureThreshold {
    pub name: &'static str,
    pub celsius: Option<u32>,
}

/// One sensor of the legacy 3-sensor thermal view
/// (`NvAPI_GPU_GetThermalSettings`, 0xE3640A56, struct V2/0x20044 — the
/// same call AmpereOC's thermal-attenuation estimator reads per-frame:
/// filter `target == GPU` and use `current`). Reports the sensor's
/// physical range (defaultMinTemp/defaultMaxTemp) alongside the live
/// value — data the ThermChannel view doesn't carry. Distinct from
/// ThermChannel (get-status sensors: 8 channels, finer-grained) and from
/// the policy tables (get-temp-thresholds: thresholds, not readings).
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalSensorReading {
    /// Sensor target: GPU core / Memory / Board.
    pub target: nvapi_hi::nvapi::ThermalTarget,
    /// Controller (internal / ADM1032 / ...).
    pub controller: nvapi_hi::nvapi::ThermalController,
    /// Live reading.
    pub current_c: i32,
    /// Sensor physical range (defaultMinTemp..defaultMaxTemp).
    pub min_c: i32,
    pub max_c: i32,
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
/// "Unlimited" (only ever true for D1). RE'd from the ref tool `[GPUHandle::
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
/// driver's kHz for ergonomic CLI output). RE'd from the ref tool's `queryPStateInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct PStateLevelEntry {
    /// P-State number, 0..31 (e.g. 0 for P0).
    pub pstate: u8,
    /// Min core clock (MHz), if the driver exposed it.
    pub min_mhz: Option<f64>,
    /// Max core clock (MHz), if the driver exposed it.
    pub max_mhz: Option<f64>,
}

/// Native P-State level table (the the ref tool `-pstate` GET listing), read via the
/// private PerfPstatesGetInfo (`0x7B30AE0D`). Distinct from the NVML P-State
/// table — this is the driver's native P*.Max/P*.Min index used by the ref tool's
/// `-pstate:<index>` SETTER (index 0 = P0.TDP/rated, then per pstate a .Max
/// then .Min slot).
#[derive(Debug, Clone, PartialEq)]
pub struct PStateLevelsInfo {
    /// Present P-States in ascending order with min/max core clock (MHz).
    pub pstates: Vec<PStateLevelEntry>,
}

/// Native NVAPI P-State lock request (the the ref tool `-pstate:<index>` SETTER).
/// Core-level mirror of [`nvapi_hi::PStateNativeLock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvapiPStateNativeLock {
    /// Reset all P-State locks to default (the ref tool `-pstate:-1`).
    Reset,
    /// Pin the active P-State without locking a frequency.
    PstateOnly { pstate: u8 },
    /// Pin the active P-State AND lock its frequency (freq_khz = MHz × 1000).
    PstateAndFreq { pstate: u8, freq_khz: u32 },
}

/// GPU frequency perf-cap request (the ref tool `-gpuclk:<MHz>` SETTER,
/// PerfLimitsSetStatus NDA 0x32CA4983). Core-level mirror of
/// [`nvapi_hi::PerfFreqCap`]: clamps the perf max/min frequency to a cap
/// value (NOT an offset, NOT a P-state lock). `freq_khz` = MHz × 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvapiPerfFreqCap {
    /// Clear the perf frequency cap (`-gpuclk:-1`).
    Reset,
    /// Clamp perf frequency to `[min_khz, max_khz]` (MHz × 1000).
    Cap { max_khz: u32, min_khz: u32 },
}

/// Per-cooler info aggregated from the private FanCoolers family (NDA):
/// presence mask + control type/min/max + status current speed/PWM.
/// Speed fields are in the DRIVER's scale (may be the 0..65536 duty grid
/// rather than physical RPM — 2070 desktop observed); surface everything
/// so the caller can see the actual grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvapiCoolerInfoEntry {
    pub index: u32,
    /// 0=active, 1=pwm, 2=pwm-tach
    pub cooler_type: u32,
    pub min: u32,
    pub max: u32,
    pub current: u32,
    pub current_pwm_percent: u32,
}

/// Result of a set_fan_rpm call (private FanCoolerSetControl NDA 0xEB44E8AA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvapiFanRpmResult {
    pub cooler_index: u32,
    /// 0=active, 1=pwm, 2=pwm-tach
    pub cooler_type: u32,
    pub min_rpm: u32,
    pub max_rpm: u32,
    /// None = simulation disabled (returned to auto)
    pub applied_rpm: Option<u32>,
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

/// Result of the global OV SET: the applied delta plus whether the driver's
/// GET-side table reports global OV entries at all. `driver_ov_entries ==
/// false` means the SET was accepted but is observed to be silently ignored
/// on such SKUs (Ada mobile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OvervoltApplied {
    pub applied: AppliedValue<nvapi_hi::MicrovoltsDelta>,
    pub driver_ov_entries: bool,
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
