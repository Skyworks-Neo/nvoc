use super::Wm2AcousticMode;
use super::error::Error;
use super::nvapi as low_nvapi;
use super::nvml as low_nvml;
use super::result::{
    ApiRestrictionState, AppliedValue, AutoBoostState, BatchReport, ClockOffset, DNotifierInfo,
    DNotifierLevel, DisplayInfo, EdidData, FanCurvePointReadout, FanCurveReadout, FanInfo,
    NvapiCoolerInfoEntry, NvapiFanPolicyEntry, NvapiFanPolicyInfo, NvapiFanRpmResult,
    NvapiPStateNativeLock, NvapiPerfFreqCap, OperationKind, OperationReport, OvervoltApplied,
    PStateLevelEntry, PStateLevelsInfo, PowerCeilingInfo, PowerModeStatus, PstateBaseVoltage,
    PstateClockRange, SupportedApplicationClocks, TargetOutcome, TargetTempPolicy, TdpTempLimits,
    TemperatureThreshold, ThermalSensorReading, ThrottleReason, ViolationEntry,
    ViolationStatusReport, VoltageBoostState, VoltageFrequencyCheck,
};
use super::target::GpuTarget;
use super::types::{NvapiLockedVoltageTarget, VfpResetDomain};
use ::nvapi::hi::{
    ClockDomain, CoolerPolicy, Kilohertz, KilohertzDelta, MicrovoltsDelta, PState, Percentage,
    SensorThrottle, VfPoint,
};
use nvml_wrapper::enum_wrappers::device::{Api, PerformanceState};

fn nvapi_clock_domain_to_nvml(
    domain: ClockDomain,
) -> Option<nvml_wrapper::enum_wrappers::device::Clock> {
    use nvml_wrapper::enum_wrappers::device::Clock;
    match domain {
        ClockDomain::Graphics => Some(Clock::Graphics),
        ClockDomain::Memory => Some(Clock::Memory),
        ClockDomain::Processor => Some(Clock::SM),
        ClockDomain::Video => Some(Clock::Video),
        _ => None,
    }
}
use nvml_wrapper::enums::device::FanControlPolicy;

pub trait GpuOperation {
    type Output;

    fn kind(&self) -> OperationKind;
    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error>;
}

pub fn run<O: GpuOperation>(
    target: &GpuTarget<'_>,
    op: O,
) -> Result<OperationReport<O::Output>, Error> {
    let operation = op.kind();
    // Pre-wake the dGPU before NVAPI write ops. 610+ mobile drivers aggressively
    // enter GC6/GCOFF at idle (5-20s), making NVAPI writes fail with
    // GpuNotPowered (-220). force_gc6_exit independently wakes a GCOFF'd dGPU
    // (empirically verified — plain GET/SET do NOT wake it).
    //
    // Two-stage gate (GpuType::needs_gc6_wake is the single source of truth,
    // also used by the auto-optimizer's explicit native wakes):
    //  1. is_mobile() (by GPU model) filters out positively-identified desktop
    //     GPUs — those skip the wake entirely (zero escape cost).
    //  2. For mobile OR Unknown OR info()-unreadable (likely already GCOFF)
    //     GPUs, call force_gc6_exit. Its own return value is the native
    //     fallback: desktop/unknown-that's-really-desktop returns
    //     NoImplementation (-104, ignored); mobile returns OK and wakes.
    if operation.is_nvapi_write()
        && let Ok(gpu) = target.nvapi()
    {
        let need_wake = match gpu.info() {
            Ok(info) => match fetch_gpu_type(&info) {
                Ok(t) => t.needs_gc6_wake(),
                Err(_) => true, // can't classify -> conservative wake
            },
            Err(_) => true, // info() failed (likely GCOFF) -> wake
        };
        if need_wake {
            let _ = gpu.force_gc6_exit(); // best-effort; -104 etc. ignored
        }
    }
    // Scope the last-error ledger to the operation itself. The wake-gate
    // classification above probes ~30 info() surfaces with soft-fail, and
    // every tolerated failure is still RECORDED in the ledger (TCC cards:
    // GetConnectedDisplayIds -6; WDDM laptops: the GetCoolerSettings
    // fallback NotSupported, ...). Left in place, a LOCAL refusal inside
    // the operation — e.g. the ClkDomains V2 record-type gate, which
    // issues no failing NVAPI call at all — would surface one of those
    // stale probe errors as the "Last NVAPI error" under supported:false,
    // blaming the write on a display/fan probe that has nothing to do
    // with it.
    ::nvapi::clear_status_error();
    let output = op.run(target)?;
    Ok(OperationReport {
        target: target.id,
        operation,
        output,
        warnings: Vec::new(),
    })
}

pub fn run_many<O: GpuOperation + Clone>(
    targets: &[GpuTarget<'_>],
    op: O,
) -> Result<BatchReport<O::Output>, Error> {
    let operation = op.kind();
    let outcomes = targets
        .iter()
        .map(|target| match run(target, op.clone()) {
            Ok(report) => TargetOutcome::Ok(report),
            Err(error) => TargetOutcome::Err {
                target: target.id,
                error,
            },
        })
        .collect();
    Ok(BatchReport {
        operation,
        outcomes,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct QueryGpuInfo;

impl GpuOperation for QueryGpuInfo {
    type Output = ::nvapi::hi::GpuInfo;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryGpuInfo
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let mut info = target.nvapi()?.info().map_err(Error::from)?;
        if info.uuid.is_none()
            && let Ok(nvml) = target.nvml()
        {
            info.uuid = low_nvml::query_nvml_uuid(nvml, target.id.0);
        }
        Ok(info)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryGpuSettings;

impl GpuOperation for QueryGpuSettings {
    type Output = ::nvapi::hi::GpuSettings;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryGpuSettings
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.settings().map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryGpuStatus;

impl GpuOperation for QueryGpuStatus {
    type Output = ::nvapi::hi::GpuStatus;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryGpuStatus
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.status().map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryPowerLimits;

impl GpuOperation for QueryPowerLimits {
    type Output = super::result::PowerLimits;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryPowerLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let (min_watts, current_watts, max_watts) =
            low_nvml::query_nvml_power_watts(target.nvml()?, target.id.0).ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML power limits for GPU {}",
                    target.id.0
                ))
            })?;
        Ok(super::result::PowerLimits {
            min_watts,
            current_watts,
            max_watts,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetPowerLimit {
    pub watts: u32,
}

impl GpuOperation for SetPowerLimit {
    type Output = AppliedValue<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPowerLimit
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_power_limit(target.nvml()?, target.id.0, self.watts)?;
        Ok(AppliedValue {
            requested: self.watts,
            applied: self.watts,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryTemperatureThresholds;

/// Legacy 3-sensor thermal view via `NvAPI_GPU_GetThermalSettings`
/// (0xE3640A56, struct V2/0x20044, sensorIndex=ALL). Reports the GPU core /
/// Memory / Board sensors with their physical ranges — the same source
/// AmpereOC's thermal-attenuation estimator reads (target==GPU → current).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiThermalSettings;

impl GpuOperation for QueryNvapiThermalSettings {
    type Output = Vec<ThermalSensorReading>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiThermalSettings
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let sensors = target
            .nvapi()?
            .inner()
            .thermal_settings(None)
            .map_err(Error::from)?;
        Ok(sensors
            .into_iter()
            .map(|s| ThermalSensorReading {
                target: s.target,
                controller: s.controller,
                current_c: s.current_temperature.0,
                min_c: s.default_temperature_range.min.0,
                max_c: s.default_temperature_range.max.0,
            })
            .collect())
    }
}

/// NVIDIA App power-mode (均衡/高性能) read via the ClientPowerModes family —
/// the App's Balanced/Max toggle. Reports the support gate
/// (`max_mode_idx == 1`) alongside the active mode.
#[derive(Clone, Copy, Debug)]
pub struct GetPowerMode;

impl GpuOperation for GetPowerMode {
    type Output = PowerModeStatus;

    fn kind(&self) -> OperationKind {
        OperationKind::GetPowerMode
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let (mode_mask, max_mode_idx) =
            gpu.inner().power_modes_capability().map_err(Error::from)?;
        let supported = max_mode_idx == 1;
        let active = if supported {
            gpu.inner().power_mode().map_err(Error::from)?
        } else {
            "N/A"
        };
        Ok(PowerModeStatus {
            supported,
            active,
            mode_mask,
            max_mode_idx,
        })
    }
}

/// NVIDIA App power-mode SET (`Max` = 高性能, `false` = 均衡 Balanced).
#[derive(Clone, Copy, Debug)]
pub struct SetPowerMode {
    pub max: bool,
}

impl GpuOperation for SetPowerMode {
    type Output = AppliedValue<bool>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPowerMode
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        // Reject on unsupported GPUs with the App's own gate so the user
        // gets a clear message instead of a driver -9/-1.
        let (_, max_mode_idx) = gpu.inner().power_modes_capability().map_err(Error::from)?;
        if max_mode_idx != 1 {
            return Err(Error::Custom(format!(
                "power mode (Balanced/Max) not supported on this GPU (max_mode_idx={max_mode_idx:#x})"
            )));
        }
        gpu.inner().set_power_mode(self.max).map_err(Error::from)?;
        Ok(AppliedValue {
            requested: self.max,
            applied: self.max,
        })
    }
}

/// Read the GPU fan-curve table (`ClientFanPoliciesGetControl` NDA
/// 0xE543C540, struct magic 0x200DC). RE'd from ref tool's pollFanCurve —
/// one snapshot holds up to 4 curve slots × 3 (temp, RPM) points. Curves
/// are typically settable/readable on desktops only; mobile boards drive
/// their fans through the EC.
#[derive(Clone, Copy, Debug)]
pub struct GetFanCurves;

impl GpuOperation for GetFanCurves {
    type Output = Vec<FanCurveReadout>;

    fn kind(&self) -> OperationKind {
        OperationKind::GetFanCurves
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .fan_curves()
            .map_err(Error::from)
            .map(|curves| {
                curves
                    .into_iter()
                    .map(|c| FanCurveReadout {
                        index: c.index,
                        points: c
                            .points
                            .into_iter()
                            .map(|p| FanCurvePointReadout {
                                temp_c: p.temp_c,
                                rpm: p.rpm,
                            })
                            .collect(),
                    })
                    .collect()
            })
    }
}

/// Write one fan-curve slot via the ref tool RMW protocol (GET snapshot →
/// patch the target slot's 3 (temp, RPM) points → SET the whole table back).
/// Driver enforces strict monotonicity across all lanes.
#[derive(Clone, Debug)]
pub struct SetFanCurve {
    pub index: u8,
    pub points: Vec<FanCurvePointReadout>,
}

impl GpuOperation for SetFanCurve {
    type Output = AppliedValue<Vec<FanCurvePointReadout>>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetFanCurve
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let curve = ::nvapi::hi::FanCurve {
            index: self.index,
            points: self
                .points
                .iter()
                .map(|p| ::nvapi::hi::FanCurvePoint {
                    temp_c: p.temp_c,
                    rpm: p.rpm,
                })
                .collect(),
        };
        target
            .nvapi()?
            .inner()
            .set_fan_curve(&curve)
            .map_err(Error::from)?;
        Ok(AppliedValue {
            requested: self.points.clone(),
            applied: self.points.clone(),
        })
    }
}

/// Reset one fan-curve slot to factory (ref tool 2's `GPUHandle::resetFanCurve`:
/// FanPolicySetControl NDA 0x2B2A2A45, struct magic 0x214AC — GET the policy
/// block, OR `1 << index` into the +0x08 reset bitmask, SET). This is
/// ref tool 2's NVAPI fan reset; unlike the public RestoreCoolerSettings it works
/// on GPUs whose user-mode cooler table isn't exposed (desktop 3060/2070
/// reject RestoreCoolerSettings with NOT_SUPPORTED).
#[derive(Clone, Copy, Debug)]
pub struct ResetFanCurve {
    pub index: u8,
}

impl GpuOperation for ResetFanCurve {
    type Output = AppliedValue<u8>;

    fn kind(&self) -> OperationKind {
        OperationKind::ResetFanCurve
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .reset_fan_curve(self.index as u32)
            .map_err(Error::from)?;
        Ok(AppliedValue {
            requested: self.index,
            applied: self.index,
        })
    }
}

/// NVAPI fan reset that actually undoes a pinned level on modern cards.
///
/// Live A/B (1650 Super + A4000, 2026-09-03, probe-fan-reset): the NDA
/// ClientFanCoolers control block carries a per-cooler level-override flag
/// (`NV_GPU_CLIENT_FAN_COOLER_CONTROL_V1.flags` bit0) next to the pinned
/// level. Applying "continuous + level %" works BECAUSE
/// `CoolerSettings::to_raw` sets bit0 with the level — and the ONLY reset
/// that takes effect is writing the same block back with bit0 CLEARED
/// (`level: None`): the A4000 returned to its auto curve (tach drifted
/// 2515→1535 rpm) and the 1650 Super returned to its stock zero-RPM idle
/// curve. What does NOT work there: the 0x214AC policy-block reset bitmask
/// (`ResetFanCurve` — accepted, leaves the pin untouched), the public
/// `RestoreCoolerSettings` and `RestoreCoolerPolicyTable` (both
/// NOT_SUPPORTED).
///
/// Chain: control-block rewrite (bit0=0) on every present cooler, then the
/// 0x214AC policy-block reset as a harmless best-effort cleanup of
/// curve-slot state. Callers keep the public RestoreCoolerSettings fallback
/// for legacy drivers (R391 rejects the NDA family outright — GT730).
#[derive(Clone, Copy, Debug)]
pub struct ResetNvapiFanControl;

impl GpuOperation for ResetNvapiFanControl {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetNvapiFanControl
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        // Present coolers from the private family (both live cards report
        // exactly one); fall back to Cooler1 when the family is silent.
        // Only Cooler1/Cooler2 exist in the enum today.
        let cooler_ids: Vec<::nvapi::FanCoolerId> = match gpu.inner().cooler_info_private() {
            Ok(infos) if !infos.is_empty() => infos
                .iter()
                .enumerate()
                .filter_map(|(i, _)| match i {
                    0 => Some(::nvapi::FanCoolerId::Cooler1),
                    1 => Some(::nvapi::FanCoolerId::Cooler2),
                    _ => None,
                })
                .collect(),
            _ => vec![::nvapi::FanCoolerId::Cooler1],
        };
        // level None → to_raw writes level 0 with the override bit CLEARED.
        gpu.inner()
            .set_cooler(cooler_ids.into_iter().map(|id| {
                (
                    id,
                    ::nvapi::CoolerSettings {
                        policy: ::nvapi::CoolerPolicy::TemperatureContinuous,
                        level: None,
                    },
                )
            }))
            .map_err(Error::from)?;
        // Best-effort: also clear curve-slot state in the 0x214AC block
        // (no-op on stock, accepted rc=0 everywhere observed).
        let _ = gpu.inner().reset_fan_curve(0);
        Ok(())
    }
}

/// Toggle fan stop / zero-RPM for a curve slot (FanArbiterSet NDA 0x44CD3014,
/// struct magic 0x10144, enable bit0 at +0x28). RE'd from ref tool
/// setFanCurve's tail call.
#[derive(Clone, Copy, Debug)]
pub struct SetFanStop {
    pub curve_index: u8,
    pub enable: bool,
}

impl GpuOperation for SetFanStop {
    type Output = AppliedValue<bool>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetFanStop
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .set_fan_stop(self.curve_index as u32, self.enable)
            .map_err(Error::from)?;
        Ok(AppliedValue {
            requested: self.enable,
            applied: self.enable,
        })
    }
}

/// Query per-cooler info via the private FanCoolerGetInfo (NDA 0x65CE5BFC).
/// Returns one entry per cooler with its index. RE'd from ref tool's setFanSim —
/// the private path, richer than public GetCoolerSettings.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiCoolerInfo;

impl GpuOperation for QueryNvapiCoolerInfo {
    type Output = Vec<NvapiCoolerInfoEntry>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiCoolerInfo
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let infos = target
            .nvapi()?
            .inner()
            .cooler_info_private()
            .map_err(Error::from)?;
        Ok(infos
            .into_iter()
            .map(|c| NvapiCoolerInfoEntry {
                index: c.index,
                cooler_type: c.cooler_type,
                min: c.min,
                max: c.max,
                current: c.current,
                current_pwm_percent: c.current_pwm_percent,
            })
            .collect())
    }
}

/// Query fan-policy capabilities via the private ClientFanPoliciesGetInfo
/// (NDA 0x52B76D12). Modern drivers answer the V2 block (raw); R391-era
/// drivers answer the legacy V1 block (decoded: policy list + active marker
/// + two capability flag bits per policy — no curve points in either).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiFanPolicyInfo;

impl GpuOperation for QueryNvapiFanPolicyInfo {
    type Output = Option<NvapiFanPolicyInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiFanPolicyInfo
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let info = target.nvapi()?.fan_policy_info()?;
        Ok(info.map(|i| NvapiFanPolicyInfo {
            layout: if i.stamp == 0x1003C { "v1" } else { "v2" },
            raw: i.raw,
            entries: i
                .entries
                .into_iter()
                .map(|e| NvapiFanPolicyEntry {
                    dword0: e.dword0,
                    active: e.active,
                    flags: e.flags,
                })
                .collect(),
        }))
    }
}

/// Set fan speed by RPM via the private FanCoolerSetControl (NDA 0xEB44E8AA).
/// RE'd from ref tool's setFanSim: GET control snapshot → patch the target
/// cooler's enable+level per its type → SET back. `rpm=None` disables
/// simulation (returns to auto/driver control).
#[derive(Clone, Copy, Debug)]
pub struct SetFanRpm {
    /// `None` targets every cooler present in the info mask.
    pub cooler_index: Option<u32>,
    pub rpm: Option<u32>,
}

impl GpuOperation for SetFanRpm {
    type Output = Vec<NvapiFanRpmResult>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetFanRpm
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let rs = target
            .nvapi()?
            .inner()
            .set_fan_rpm(self.cooler_index, self.rpm)
            .map_err(Error::from)?;
        Ok(rs
            .into_iter()
            .map(|r| NvapiFanRpmResult {
                cooler_index: r.cooler_index,
                cooler_type: r.cooler_type,
                min_rpm: r.min_rpm,
                max_rpm: r.max_rpm,
                applied_rpm: r.applied_rpm,
            })
            .collect())
    }
}

/// Admin-free pstate lock via `NvAPI_GPU_SetPerfLevel` (0x75dd3e6a, escape
/// 0x7000040). 2026-08-26 correction — NOT the NVCP power-mode dropdown:
/// `level` is an INDEX into the GPU's actual available P-State list (see
/// `QueryNvapiPstateNative`/get-pstate-native), not a fixed enum — on the
/// 4060 Laptop the measured mapping is 0=P8, 1=P5, 2=P4, 3=P3, 4=P0, but
/// other GPUs expose a different P-State set. Re-locking re-targets (last
/// call wins). No release argument exists (RM accepts only valid indices)
/// and the lock survives every other known release API — only a driver
/// reload/reboot clears it.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPerfLevelLock {
    pub level: u32, // index into the GPU's real P-State list (driver-validated)
}

impl GpuOperation for SetNvapiPerfLevelLock {
    type Output = AppliedValue<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPerfLevelLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .set_pstate_lock(self.level)
            .map_err(Error::from)?;
        Ok(AppliedValue {
            requested: self.level,
            applied: self.level,
        })
    }
}

impl GpuOperation for QueryTemperatureThresholds {
    type Output = Vec<TemperatureThreshold>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryTemperatureThresholds
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::get_nvml_temperature_thresholds(target.nvml()?, target.id.0)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML temperature thresholds for GPU {}",
                    target.id.0
                ))
            })
            .map(|items| {
                items
                    .into_iter()
                    .map(|(name, celsius)| TemperatureThreshold { name, celsius })
                    .collect()
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryThrottleReasons;

impl GpuOperation for QueryThrottleReasons {
    type Output = Vec<ThrottleReason>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryThrottleReasons
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::get_nvml_throttle_reasons(target.nvml()?, target.id.0)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML throttle reasons for GPU {}",
                    target.id.0
                ))
            })
            .map(|items| {
                items
                    .into_iter()
                    .map(|(name, active)| ThrottleReason {
                        name: name.to_string(),
                        active,
                    })
                    .collect()
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryViolationStatus;

fn violation_status_report(
    items: Vec<(&'static str, low_nvml::ViolationStatus)>,
) -> Option<ViolationStatusReport> {
    let reference_time_us = items.iter().find_map(|(_, status)| {
        (status.reference_time_us != 0).then_some(status.reference_time_us)
    })?;

    Some(ViolationStatusReport {
        entries: items
            .into_iter()
            .map(|(name, status)| ViolationEntry {
                name: name.to_string(),
                violation_time_ns: status.violation_time_ns,
            })
            .collect(),
        reference_time_us,
    })
}

impl GpuOperation for QueryViolationStatus {
    type Output = Option<ViolationStatusReport>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryViolationStatus
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let nvml = target.nvml()?;
        Ok(
            low_nvml::get_nvml_violation_status(nvml, target.id.0)
                .and_then(violation_status_report),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetTemperatureLimit {
    pub celsius: i32,
}

impl GpuOperation for SetTemperatureLimit {
    type Output = AppliedValue<i32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetTemperatureLimit
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_temperature_limit(target.nvml()?, target.id.0, self.celsius)?;
        Ok(AppliedValue {
            requested: self.celsius,
            applied: self.celsius,
        })
    }
}

/// Set the NVML acoustic target temperature (`ACOUSTIC_CURR` threshold) — the
/// Linux-native target-temp channel (same one nvidia_oc / MSI Afterburner
/// "target temperature" use). Windows rejects the NVML threshold setter
/// outright; use the NVAPI wall ([`SetNvapiTargetTemp`]) there.
#[derive(Clone, Copy, Debug)]
pub struct SetNvmlAcousticTemp {
    pub celsius: i32,
}

impl GpuOperation for SetNvmlAcousticTemp {
    type Output = AppliedValue<i32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvmlAcousticTemp
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_acoustic_temperature(target.nvml()?, target.id.0, self.celsius)?;
        Ok(AppliedValue {
            requested: self.celsius,
            applied: self.celsius,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryPstates;

impl GpuOperation for QueryPstates {
    type Output = Vec<PstateClockRange>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryPstates
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::get_nvml_pstate_info(target.nvml()?, target.id.0)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML P-State information for GPU {}",
                    target.id.0
                ))
            })
            .map(|items| {
                items
                    .into_iter()
                    .map(
                        |(pstate, min_core_mhz, max_core_mhz, min_memory_mhz, max_memory_mhz)| {
                            PstateClockRange {
                                pstate,
                                min_core_mhz,
                                max_core_mhz,
                                min_memory_mhz,
                                max_memory_mhz,
                            }
                        },
                    )
                    .collect()
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QuerySupportedApplicationsClocks;

impl GpuOperation for QuerySupportedApplicationsClocks {
    type Output = Vec<SupportedApplicationClocks>;

    fn kind(&self) -> OperationKind {
        OperationKind::QuerySupportedApplicationsClocks
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::get_nvml_supported_applications_clocks(target.nvml()?, target.id.0)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML application clocks for GPU {}",
                    target.id.0
                ))
            })
            .map(|items| {
                items
                    .into_iter()
                    .map(|(memory_mhz, graphics_mhz)| SupportedApplicationClocks {
                        memory_mhz,
                        graphics_mhz,
                    })
                    .collect()
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryClockOffset {
    pub domain: ClockDomain,
    pub pstate: PerformanceState,
}

impl GpuOperation for QueryClockOffset {
    type Output = ClockOffset;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryClockOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let nvml = target.nvml()?;
        let clock = nvapi_clock_domain_to_nvml(self.domain).ok_or_else(|| {
            Error::Custom(format!(
                "NVML clock offset does not support domain {:?}",
                self.domain
            ))
        })?;
        let mhz = low_nvml::get_nvml_clock_offset(nvml, target.id.0, clock, self.pstate)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "failed to query NVML clock offset for GPU {}",
                    target.id.0
                ))
            })?;
        Ok(ClockOffset { mhz })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetClockOffset {
    pub domain: ClockDomain,
    pub pstate: PerformanceState,
    pub mhz: i32,
}

impl GpuOperation for SetClockOffset {
    type Output = AppliedValue<i32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetClockOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let nvml = target.nvml()?;
        let clock = nvapi_clock_domain_to_nvml(self.domain).ok_or_else(|| {
            Error::Custom(format!(
                "NVML clock offset does not support domain {:?}",
                self.domain
            ))
        })?;
        low_nvml::set_nvml_clock_offset(nvml, target.id.0, clock, self.pstate, self.mhz)?;
        Ok(AppliedValue {
            requested: self.mhz,
            applied: self.mhz,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetApplicationsClocks {
    pub memory_mhz: u32,
    pub graphics_mhz: u32,
}

impl GpuOperation for SetApplicationsClocks {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetApplicationsClocks
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_applications_clocks(
            target.nvml()?,
            target.id.0,
            self.memory_mhz,
            self.graphics_mhz,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetLegacyApplicationFreqLock;

impl GpuOperation for ResetLegacyApplicationFreqLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetLegacyApplicationFreqLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::reset_nvml_applications_clocks(target.nvml()?, target.id.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetLockedClocks {
    pub domain: ClockDomain,
    pub min_mhz: u32,
    pub max_mhz: u32,
}

impl GpuOperation for SetLockedClocks {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetLockedClocks
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        match self.domain {
            ClockDomain::Graphics => low_nvml::set_nvml_core_locked_clocks(
                target.nvml()?,
                target.id.0,
                self.min_mhz,
                self.max_mhz,
            ),
            ClockDomain::Memory => low_nvml::set_nvml_mem_locked_clocks(
                target.nvml()?,
                target.id.0,
                self.min_mhz,
                self.max_mhz,
            ),
            _ => Err(Error::from(
                "NVML locked clock domain must be Graphics or Memory",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetFreqLock {
    pub domain: ClockDomain,
}

impl GpuOperation for ResetFreqLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetFreqLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        match self.domain {
            ClockDomain::Graphics => {
                low_nvml::reset_nvml_core_locked_clocks(target.nvml()?, target.id.0)
            }
            ClockDomain::Memory => {
                low_nvml::reset_nvml_mem_locked_clocks(target.nvml()?, target.id.0)
            }
            _ => Err(Error::from(
                "NVML locked clock domain must be Graphics or Memory",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryFanInfo;

impl GpuOperation for QueryFanInfo {
    type Output = FanInfo;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryFanInfo
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let nvml = target.nvml()?;
        let count = low_nvml::get_nvml_num_fans(nvml, target.id.0).ok_or_else(|| {
            Error::Custom(format!("failed to query fan count for GPU {}", target.id.0))
        })?;
        let (min_speed, max_speed) = match low_nvml::get_nvml_min_max_fan_speed(nvml, target.id.0) {
            Some((min, max)) => (Some(min), Some(max)),
            None => (None, None),
        };
        // Best-effort: legacy NVML only answers the v1 symbol (min/max are
        // v2-only there), so current is often the single live value.
        let current_speed = low_nvml::get_nvml_fan_speed_current(nvml, target.id.0);
        Ok(FanInfo {
            count,
            min_speed,
            max_speed,
            current_speed,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetFanSpeed {
    pub fan_index: u32,
    pub policy: FanControlPolicy,
    pub level: u32,
}

impl GpuOperation for SetFanSpeed {
    type Output = AppliedValue<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetFanSpeed
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_fan_speed(
            target.nvml()?,
            target.id.0,
            self.fan_index,
            self.policy,
            self.level,
        )?;
        Ok(AppliedValue {
            requested: self.level,
            applied: self.level,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetFanSpeed {
    pub fan_index: u32,
}

impl GpuOperation for ResetFanSpeed {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetFanSpeed
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_default_fan_speed(target.nvml()?, target.id.0, self.fan_index)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryPstateBaseVoltage {
    pub pstate: PState,
}

impl GpuOperation for QueryPstateBaseVoltage {
    type Output = PstateBaseVoltage;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryPstateBaseVoltage
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::query_pstate_base_voltage(target.nvapi()?, self.pstate)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetNvapiOvervolt {
    pub delta_uv: MicrovoltsDelta,
}

impl GpuOperation for SetNvapiOvervolt {
    type Output = OvervoltApplied;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiOvervolt
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let ov_reported = low_nvapi::set_nvapi_overvolt(target.nvapi()?, self.delta_uv)?;
        Ok(OvervoltApplied {
            applied: AppliedValue {
                requested: self.delta_uv,
                applied: self.delta_uv,
            },
            driver_ov_entries: ov_reported,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetPstateBaseVoltage {
    pub pstate: PState,
    pub delta_uv: MicrovoltsDelta,
}

impl GpuOperation for SetPstateBaseVoltage {
    type Output = AppliedValue<MicrovoltsDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPstateBaseVoltage
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_pstate_base_voltage(target.nvapi()?, self.delta_uv, self.pstate)?;
        Ok(AppliedValue {
            requested: self.delta_uv,
            applied: self.delta_uv,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetLegacyGpcRailOvervoltLimit;

impl GpuOperation for ResetLegacyGpcRailOvervoltLimit {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetLegacyGpcRailOvervoltLimit
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::reset_all_pstate_base_voltages(target.nvapi()?)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetPstateClockOffset {
    pub pstate: PState,
    pub domain: ClockDomain,
    pub delta: KilohertzDelta,
}

impl GpuOperation for SetPstateClockOffset {
    type Output = AppliedValue<KilohertzDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPstateClockOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_pstate_clock_offset_preserve(
            target.nvapi()?,
            self.pstate,
            self.domain,
            self.delta,
        )?;
        Ok(AppliedValue {
            requested: self.delta,
            applied: self.delta,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetCoolerLevels {
    pub policy: CoolerPolicy,
    pub level: u32,
    pub cooler_target: low_nvapi::CoolerTarget,
}

impl GpuOperation for SetCoolerLevels {
    type Output = AppliedValue<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetCoolerLevels
    }

    fn run(&self, gpu: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_cooler_levels(&[gpu.nvapi()?], self.policy, self.level, self.cooler_target)?;
        Ok(AppliedValue {
            requested: self.level,
            applied: self.level,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryVfpPointVoltage {
    pub point: usize,
}

impl GpuOperation for QueryVfpPointVoltage {
    type Output = ::nvapi::hi::Microvolts;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVfpPointVoltage
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::get_voltage_by_point(target.nvapi()?, self.point)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetVfpFrequencyLock {
    pub domain: ClockDomain,
    pub upper: Kilohertz,
    pub lower: Option<Kilohertz>,
}

impl GpuOperation for SetVfpFrequencyLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetVfpFrequencyLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_vfp_frequency_lock(target.nvapi()?, self.domain, self.upper, self.lower)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetVfpFrequencyLock {
    pub domain: ClockDomain,
}

impl GpuOperation for ResetVfpFrequencyLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetVfpFrequencyLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::reset_vfp_frequency_lock(target.nvapi()?, self.domain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetGpcVoltLock {
    pub voltage_target: NvapiLockedVoltageTarget,
    pub feedback: bool,
}

impl GpuOperation for SetGpcVoltLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetGpcVoltLock
    }

    fn run(&self, gpu: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let request = match self.voltage_target {
            NvapiLockedVoltageTarget::Point(point) => {
                low_nvapi::VfpLockRequest::VoltagePoint(point)
            }
            NvapiLockedVoltageTarget::Voltage(voltage) => {
                low_nvapi::VfpLockRequest::Voltage(voltage)
            }
        };
        low_nvapi::lock_vfp(&[gpu.nvapi()?], request, self.feedback)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetPublicVftableOffset {
    pub domain: VfpResetDomain,
}

impl GpuOperation for ResetPublicVftableOffset {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetPublicVftableOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::reset_vfp_deltas(target.nvapi()?, self.domain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetPublicVftableGpcLock;

impl GpuOperation for ResetPublicVftableGpcLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetPublicVftableGpcLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.reset_vfp_lock().map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetPublicVftablePointOffset {
    pub point: usize,
    pub delta: KilohertzDelta,
}

impl GpuOperation for SetPublicVftablePointOffset {
    type Output = AppliedValue<KilohertzDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPublicVftablePointOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::adjust_single_vfp_point(&[target.nvapi()?], self.point, self.delta.0)?;
        Ok(AppliedValue {
            requested: self.delta,
            applied: self.delta,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetPublicVftableRangeOffset {
    pub start: usize,
    pub end: usize,
    pub delta: KilohertzDelta,
}

impl GpuOperation for SetPublicVftableRangeOffset {
    type Output = AppliedValue<KilohertzDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetPublicVftableRangeOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_pointwise_vfp_delta(&[target.nvapi()?], self.start, self.end, self.delta.0)?;
        Ok(AppliedValue {
            requested: self.delta,
            applied: self.delta,
        })
    }
}

/// Which driver-side ("OEM"/NVIDIA) OC Scanner action to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OemOcScannerAction {
    Start,
    Stop,
    Revert,
    /// Query last-run status (does not write per-point results; returns Ok
    /// if idle/has-result, or a status error if busy/not-supported).
    Status,
}

/// Control NVIDIA's driver-side OC Scanner (drivers >= 455.00). The scan
/// runs inside the driver and applies the resulting V/F offsets itself;
/// Start is fire-and-forget, Revert restores the pre-scan curve.
#[derive(Clone, Copy, Debug)]
pub struct OemOcScanner {
    pub action: OemOcScannerAction,
}

impl GpuOperation for OemOcScanner {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::OemOcScanner
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let res = match self.action {
            OemOcScannerAction::Start => gpu.oem_oc_scanner_start(),
            OemOcScannerAction::Stop => gpu.oem_oc_scanner_stop(),
            OemOcScannerAction::Revert => gpu.oem_oc_scanner_revert(),
            OemOcScannerAction::Status => gpu.oem_oc_scanner_status(),
        };
        res.map_err(Error::from)
    }
}

/// Force a P-State via the private SetForcePstate (NDA 0x025BFB10).
/// `set_type`: 2 = force until released (nvapioc convention), 0 = release.
#[derive(Clone, Copy, Debug)]
pub struct SetForcePstate {
    pub pstate: u32,
    pub set_type: u32,
}

impl GpuOperation for SetForcePstate {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetForcePstate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_force_pstate(self.pstate, self.set_type)
            .map_err(Error::from)
    }
}

/// Restart the display driver (NDA 0xB4B26B65) — legacy "apply OC" trigger.
#[derive(Clone, Copy, Debug)]
pub struct RestartDisplayDriver;

impl GpuOperation for RestartDisplayDriver {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::RestartDisplayDriver
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .restart_display_driver()
            .map_err(Error::from)
    }
}

/// Release a force-locked pstate via EnableDynamicPstates(enable=0).
/// SetForcePstate (0x025BFB10) has no unlock path — all set_type values
/// force-lock. This is the escape hatch when a pstate gets stuck locked.
#[derive(Clone, Copy, Debug)]
pub struct ResetForcePstate;

impl GpuOperation for ResetForcePstate {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetForcePstate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        // Release a force-locked pstate. IDA-verified: SetForcePstate has no
        // dedicated unlock path, but pstate=16 is the "all pstates" sentinel
        // that sets bitmask=0 — the same value GetForcePstate returns (16)
        // when no force is active. So SetForcePstate(pstate=16, set_type=0)
        // is the most likely release: it sends bitmask=0 + mode=0 via the
        // same RM escape 0x7000056. EnableDynamicPstates(enable=0) was also
        // tested live and does NOT release (different escape 0x70000BB).
        target.nvapi()?.set_force_pstate(16, 0).map_err(Error::from)
    }
}

/// Battery Boost 2.0 enable/disable (0xD27D0629).
/// Mobile-only feature.
#[derive(Clone, Copy, Debug)]
pub struct SetBb2Active {
    pub enable: bool,
}

impl GpuOperation for SetBb2Active {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetBb2Active
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_bb2_active(self.enable)
            .map_err(Error::from)
    }
}

/// Whisper Mode 2.0 enable/disable (NDA 0xD27D0629).
/// Mobile-only feature.
#[derive(Clone, Copy, Debug)]
pub struct SetWm2Active {
    pub enable: bool,
}

impl GpuOperation for SetWm2Active {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetWm2Active
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_wm2_active(self.enable)
            .map_err(Error::from)
    }
}

/// Whisper Mode 2.0 acoustic mode (NDA 0xD27D0629).
/// 0=Quieter, 1=Quiet, 2=Balanced.
#[derive(Clone, Copy, Debug)]
pub struct SetWm2Mode {
    pub mode: Wm2AcousticMode,
}

impl GpuOperation for SetWm2Mode {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetWm2Mode
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.set_wm2_mode(self.mode).map_err(Error::from)
    }
}

#[derive(Clone, Debug)]
pub struct SetDomainVfpDeltas {
    pub domain: ClockDomain,
    pub deltas: Vec<(usize, KilohertzDelta)>,
}

impl GpuOperation for SetDomainVfpDeltas {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetDomainVfpDeltas
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_nvapi_domain_vfp_deltas(target.nvapi()?, self.domain, &self.deltas)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryDomainVfpPoints {
    pub domain: ClockDomain,
    pub infer_missing_default: bool,
    pub indexed: bool,
}

impl GpuOperation for QueryDomainVfpPoints {
    type Output = Vec<(usize, VfPoint)>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryDomainVfpPoints
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let mut points = low_nvapi::query_domain_vf_points_indexed(
            target.nvapi()?,
            self.domain,
            self.infer_missing_default,
        )?;
        if !self.indexed {
            points = points
                .into_iter()
                .enumerate()
                .map(|(i, (_, point))| (i, point))
                .collect();
        }
        Ok(points)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryDomainVfpIndices {
    pub domain: ClockDomain,
}

impl GpuOperation for QueryDomainVfpIndices {
    type Output = Vec<usize>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryDomainVfpIndices
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::query_domain_vfp_indices(target.nvapi()?, self.domain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryLegacyCoreOvervoltRanges;

impl GpuOperation for QueryLegacyCoreOvervoltRanges {
    type Output = Vec<(PState, MicrovoltsDelta, MicrovoltsDelta, MicrovoltsDelta)>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryLegacyCoreOvervoltRanges
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::legacy_core_overvolt_ranges(target.nvapi()?)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryLegacyP0CoreMaxVoltageDelta;

impl GpuOperation for QueryLegacyP0CoreMaxVoltageDelta {
    type Output = Option<MicrovoltsDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryLegacyP0CoreMaxVoltageDelta
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::legacy_p0_core_max_voltage_delta(target.nvapi()?)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryVoltageBoost;

impl GpuOperation for QueryVoltageBoost {
    type Output = VoltageBoostState;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVoltageBoost
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(VoltageBoostState {
            voltage_boost: target.nvapi()?.settings()?.voltage_boost,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetVoltageBoost {
    pub boost: Percentage,
}

impl GpuOperation for SetVoltageBoost {
    type Output = AppliedValue<Percentage>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetVoltageBoost
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.set_voltage_boost(self.boost)?;
        Ok(AppliedValue {
            requested: self.boost,
            applied: self.boost,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SetNvapiPowerLimits {
    pub limits: Vec<Percentage>,
}

impl GpuOperation for SetNvapiPowerLimits {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPowerLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_power_limits(self.limits.iter().copied())
            .map_err(Error::from)
    }
}

/// Set the PPAB / Dynamic-Boost controller enable state (notebook dGPU↔CPU
/// power coordination). NDA-private nvapi ID 0x1504FC3D; raw boolean setter.
#[derive(Clone, Debug)]
pub struct SetNvapiDynamicBoost {
    pub active: bool,
}

impl GpuOperation for SetNvapiDynamicBoost {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiDynamicBoost
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_dynamic_boost(self.active)
            .map_err(Error::from)
    }
}

/// Set the GPU TGP in watts (notebook watts-form TGP slider; the range that
/// appears under the PPAB/Dynamic-Boost enable). NDA-private nvapi triplet:
/// GET 0x8B3E7343 → patch → SET 0xBFF09E59. `policy_index` selects the entry
/// (use [`QueryNvapiTgpWattRange`]); if None, defaults to index 2 like the ref tool.
#[derive(Clone, Debug)]
pub struct SetNvapiTgpWatt {
    pub watts: u32,
    pub policy_index: Option<usize>,
}

impl GpuOperation for SetNvapiTgpWatt {
    type Output = u32;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiTgpWatt
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let idx = self.policy_index.unwrap_or(2);
        target
            .nvapi()?
            .set_tgp_watt(self.watts, idx)
            .map_err(Error::from)
    }
}

/// Reset the GPU TGP to its rated/default value (the TGP slider's "Reset").
#[derive(Clone, Debug, Default)]
pub struct ResetNvapiTgpWatt {
    pub policy_index: Option<usize>,
}

impl GpuOperation for ResetNvapiTgpWatt {
    type Output = Option<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::ResetNvapiTgpWatt
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let idx = self.policy_index.unwrap_or(2);
        target.nvapi()?.reset_tgp_watt(idx).map_err(Error::from)
    }
}

/// Query the TGP-watts range (min/default/max mW + active policy index) from
/// the private ClientPowerPoliciesGetInfo variant (NDA 0x67F31384).
#[derive(Clone, Debug, Default)]
pub struct QueryNvapiTgpWattRange;

/// TGP-watts range result (all values in **watts**, derived from the NDA
/// milliwatt struct for ergonomic CLI output).
#[derive(Clone, Debug)]
pub struct TgpWattRangeInfo {
    pub policy_index: usize,
    pub min_watt: Option<f64>,
    pub default_watt: Option<f64>,
    pub max_watt: Option<f64>,
}

impl GpuOperation for QueryNvapiTgpWattRange {
    type Output = Option<TgpWattRangeInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiTgpWattRange
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .tgp_watt_range()
            .map_err(Error::from)?
            .map(|r| TgpWattRangeInfo {
                policy_index: r.policy_index,
                min_watt: r.min_mw.map(|mw| mw as f64 / 1000.0),
                default_watt: r.default_mw.map(|mw| mw as f64 / 1000.0),
                max_watt: r.max_mw.map(|mw| mw as f64 / 1000.0),
            }))
    }
}

/// Query the D-Notifier (D0-notify) current level + the D1..D5 power-cap table
/// via the private ClientPowerPoliciesGetInfo (NDA 0x67F31384) — the same call
/// `QueryNvapiTgpWattRange` uses; the D-Notifier fields live in the struct's
/// tail. Returns `None` where the driver doesn't expose the private interface.
/// Power values are converted mW → watts for ergonomic CLI output.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiDNotifier;

impl GpuOperation for QueryNvapiDNotifier {
    type Output = Option<DNotifierInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiDNotifier
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .dnotify_info()
            .map_err(Error::from)?
            .map(|r| DNotifierInfo {
                active: r.active.as_ref().map(|l| l.level),
                levels: r
                    .levels
                    .iter()
                    .map(|l| DNotifierLevel {
                        level: l.level,
                        watts: l.power_mw.map(|mw| mw as f64 / 1000.0),
                    })
                    .collect(),
            }))
    }
}

/// Query the actually-effective power wall (nvidia-smi's PPAB
/// `GPU Ceiling Power Limit` trio) by composing the three private reads:
/// the TGP range (VBIOS default + active policy index), the standalone
/// `ClientTgpWattGetStatus` (the requested TGP — the slider's live
/// position), and the active D-Notifier level's cap. The effective ceiling
/// is the MIN of the requested TGP and the D-Notifier cap (live-verified
/// against nvidia-smi on RTX 4060 Laptop: D2 active → 55W ceiling, D1
/// active → full 100W requested). This is the "you set 100W — here is what
/// actually applies" value the GUI/TUI power slider anchors to.
/// Returns `None` where the driver doesn't expose the private interface.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiPowerCeiling;

impl GpuOperation for QueryNvapiPowerCeiling {
    type Output = Option<PowerCeilingInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiPowerCeiling
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let range = gpu.tgp_watt_range().map_err(Error::from)?;
        let status = gpu.tgp_watt_status().map_err(Error::from)?;
        let dnotify = gpu.dnotify_info().map_err(Error::from)?;
        // The private family is all-or-nothing: no range ⇒ no ceiling surface.
        let (policy_index, default_watt) = match range {
            Some(r) => (r.policy_index, r.default_mw.map(|mw| mw as f64 / 1000.0)),
            None => return Ok(None),
        };
        // tgp_watt_status resolves its own policy index; trust it when it
        // disagrees (it re-read the same private GetInfo).
        let (requested_watt, dnotify_watt) = {
            let requested = status
                .and_then(|s| s.current_mw)
                .map(|mw| mw as f64 / 1000.0);
            // The active D level's cap; D1 (Unlimited) / N/A ⇒ None (no cap).
            let active = dnotify.and_then(|d| d.active).and_then(|l| l.power_mw);
            (requested, active.map(|mw| mw as f64 / 1000.0))
        };
        let ceiling_watt = [requested_watt, dnotify_watt]
            .into_iter()
            .flatten()
            .reduce(f64::min);
        Ok(Some(PowerCeilingInfo {
            policy_index,
            default_watt,
            requested_watt,
            dnotify_watt,
            ceiling_watt,
        }))
    }
}

/// Read-only snapshot of the private VoltRails family (the "melonVolt path"):
/// rail mask + per-rail control-offset entries + live per-rail voltages, via
/// the private-but-publicly-resolvable 0x2C73AFDC (rail builder) /
/// 0xA3070DB0 (control GET) / 0x5D0634EE (live status) — see
/// `reverse/melonvolt/ANALYSIS.md` for the full RE chain. The µV-offset SET
/// sibling (0x87C55C8A) is wrapped as [`SetNvapiVoltRailOffset`] and the
/// absolute-target convenience as [`SetNvapiVoltRailTarget`].
/// Returns `None` where the driver doesn't expose the private interface.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiVoltRails;

impl GpuOperation for QueryNvapiVoltRails {
    type Output = Option<::nvapi::VoltRails>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiVoltRails
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.volt_rails().map_err(Error::from)
    }
}

/// Set one rail's µV offset via the private VoltRails control object (the
/// melonVolt write path: GET snapshot → locate entry → type guard →
/// patch → SET → readback verify, see `reverse/melonvolt/ANALYSIS.md`).
/// Payload index 0 is the offset on RTX-5090 MSVDD (rail bit 1, type 3);
/// type semantics on other GPUs are platform-specific — set `expected_type`
/// to guard them.
///
/// No magnitude limit is enforced: the offset is passed through verbatim and
/// the driver clamps the effective wall (status index 4) to
/// `min(target, vbios_wall, vrm_max_wall)` on its own. An offset past the
/// ceiling is not wasted in a dangerous sense — it just cannot raise the
/// wall further. The post-SET readback reports the effective wall so the
/// user sees the clamp.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // uV suffix matches the nvapi-rs field naming
pub struct SetNvapiVoltRailOffset {
    /// rail bit within the mask (RTX 5090 MSVDD = 1)
    pub rail_bit: u32,
    /// target offset in µV (absolute, not a delta; 0 = stock). Passed through
    /// verbatim — the driver clamps the effective wall itself.
    pub offset_uV: i32,
    /// refuse to write unless the entry's current type equals this
    /// (melonVolt requires 3 on 5090 MSVDD); `None` = no type check
    pub expected_type: Option<u32>,
}

impl GpuOperation for SetNvapiVoltRailOffset {
    type Output = Option<NvapiVoltRailOffsetApplied>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiVoltRailOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let rails = gpu.volt_rails().map_err(Error::from)?;
        let Some(rails) = rails else {
            return Ok(None);
        };
        let entry = rails
            .control
            .iter()
            .find(|e| e.rail_bit == self.rail_bit)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "rail bit {} not present (mask 0x{:08X})",
                    self.rail_bit, rails.rail_mask
                ))
            })?;
        if let Some(expected) = self.expected_type
            && entry.entry_type != expected
        {
            return Err(Error::Custom(format!(
                "rail {} entry type {} != expected {expected} — refusing to write \
                 (type semantics differ per platform; override with a different \
                 --expect-type if you know better)",
                self.rail_bit, entry.entry_type
            )));
        }
        // No magnitude limit: the offset is passed through verbatim. The
        // driver clamps the effective wall (status index 4) to
        // min(target, vbios_wall, vrm_max_wall) on its own — an offset past
        // the ceiling cannot raise the wall further but is not dangerous, so
        // we do not reject it. The post-SET readback reports the effective
        // wall so the user sees the clamp.
        let previous = entry.values[0];
        let retained = gpu
            .set_volt_rail_value(self.rail_bit, self.offset_uV)
            .map_err(Error::from)?
            .ok_or_else(|| Error::Custom("volt-rails family vanished between reads".into()))?;
        // Read back the status entry for this rail to surface the effective
        // wall the driver actually put in force (clamped to VRM/vBIOS max).
        // The driver may not have refreshed status immediately after SET; a 0
        // here means "no status entry / not yet updated" — re-run
        // get-volt-rails to confirm.
        #[allow(non_snake_case)]
        let effective_wall_uV = gpu
            .volt_rails()
            .map_err(Error::from)?
            .and_then(|r| {
                r.status
                    .iter()
                    .find(|e| e.rail_bit == self.rail_bit)
                    // status payload index 4 = effective wall (clamped to
                    // min(target, vbios_wall, vrm_max_wall)); see
                    // nvapi-rs sys::gpu::power::undocumented::status_values.
                    // Entry type is a per-rail protocol tag (GB10/50-series
                    // Xbar status = 3), not a layout marker — match by
                    // rail_bit only.
                    .map(|e| e.values[4])
            })
            .unwrap_or(0);
        Ok(Some(NvapiVoltRailOffsetApplied {
            rail_bit: self.rail_bit,
            previous_uV: previous,
            applied_uV: retained,
            effective_wall_uV,
        }))
    }
}

/// Result of a successful volt-rail offset write.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // uV suffix matches the nvapi-rs field naming
pub struct NvapiVoltRailOffsetApplied {
    pub rail_bit: u32,
    pub previous_uV: i32,
    pub applied_uV: i32,
    /// effective wall after SET, read back from the status entry's index 4.
    /// The driver clamps this to `min(target, vbios_wall, vrm_max_wall)`, so
    /// it may be below the requested offset's implied wall. 0 = no status
    /// entry / driver hasn't refreshed yet (re-run get-volt-rails).
    pub effective_wall_uV: i32,
}

/// Set a volt-rail to an ABSOLUTE target voltage by deriving the required µV
/// offset from the live control/status snapshot. Convenience wrapper around
/// the melonVolt offset SET (see `reverse/melonvolt/ANALYSIS.md`) for
/// GUI/TUI sliders that think in absolute volts, not offsets.
///
/// Derivation (the offset is relative to the factory/default wall):
///   - `control` entry `.values[0]` = the offset currently applied (µV)
///   - `status` entry `.values[1]` = the target wall the driver holds (µV) —
///     the wall *including* the current offset, before the VRM/vBIOS clamp.
///     Status entries are matched by `rail_bit` only: the entry type is a
///     per-rail protocol tag (GB10/50-series Xbar status = 3) with the same
///     six-value layout as a type-1 core entry
///   - `base_wall = target_wall − current_offset` recovers the factory wall
///   - `offset = target_uV − base_wall` is what gets written
///
/// Because the initial offset is unknown to a caller that thinks in
/// absolute volts, this read-compute-write happens inside one operation so
/// the snapshot is consistent. The driver still clamps the effective wall
/// (status index 4) to `min(target, vbios_wall, vrm_max_wall)` on its own —
/// a target past the ceiling is not dangerous, it just caps out there.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // uV suffix matches the nvapi-rs field naming
pub struct SetNvapiVoltRailTarget {
    /// rail bit within the mask (RTX 5090 MSVDD = 1)
    pub rail_bit: u32,
    /// absolute target voltage in µV (e.g. 1150000 = 1.15V). Passed through
    /// after offset derivation — the driver clamps the effective wall itself.
    pub target_uV: i32,
    /// refuse to write unless the control entry's current type equals this
    /// (melonVolt requires 3 on 5090 MSVDD); `None` = no type check
    pub expected_type: Option<u32>,
}

impl GpuOperation for SetNvapiVoltRailTarget {
    type Output = Option<NvapiVoltRailTargetApplied>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiVoltRailTarget
    }

    #[allow(non_snake_case)] // uV-suffixed locals match the nvapi-rs field naming
    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let rails = gpu.volt_rails().map_err(Error::from)?;
        let Some(rails) = rails else {
            return Ok(None);
        };
        let ctrl = rails
            .control
            .iter()
            .find(|e| e.rail_bit == self.rail_bit)
            .ok_or_else(|| {
                Error::Custom(format!(
                    "rail bit {} not present (mask 0x{:08X})",
                    self.rail_bit, rails.rail_mask
                ))
            })?;
        if let Some(expected) = self.expected_type
            && ctrl.entry_type != expected
        {
            return Err(Error::Custom(format!(
                "rail {} entry type {} != expected {expected} — refusing to write \
                 (type semantics differ per platform; override with a different \
                 --expect-type if you know better)",
                self.rail_bit, ctrl.entry_type
            )));
        }
        // The offset the driver currently holds for this rail (control entry
        // payload index 0).
        let previous_offset_uV = ctrl.values[0];
        // The target wall the driver currently holds (status entry for this
        // rail, payload index 1). This is the wall *including* the current
        // offset, before the VRM/vBIOS clamp — see sys status_values doc.
        // Matched by rail_bit only: the entry type is a per-rail protocol
        // tag (GB10/50-series Xbar status = 3), not a layout marker.
        let target_wall_uV = rails
            .status
            .iter()
            .find(|e| e.rail_bit == self.rail_bit)
            .and_then(|e| e.values.get(1).copied())
            .unwrap_or(0);
        if target_wall_uV == 0 {
            // The dGPU is likely asleep/idle and hasn't populated status, so
            // the base wall can't be recovered and the derived offset would
            // be garbage. The run() pre-wake should normally keep it awake,
            // but a status value of 0 means the driver hasn't reported one.
            return Err(Error::Custom(format!(
                "rail {} status target wall is 0 — the dGPU may be idle/asleep; \
                 apply a load and retry (cannot derive base wall from an empty \
                 status)",
                self.rail_bit
            )));
        }
        // Recover the factory/default wall by removing the current offset
        // from the target wall (target_wall = base + offset).
        let base_wall_uV = target_wall_uV - previous_offset_uV;
        let offset_uV = self.target_uV - base_wall_uV;
        let applied_uV = gpu
            .set_volt_rail_value(self.rail_bit, offset_uV)
            .map_err(Error::from)?
            .ok_or_else(|| Error::Custom("volt-rails family vanished between reads".into()))?;
        // Read back the status entry for this rail to surface the effective
        // wall the driver actually put in force (clamped to VRM/vBIOS max).
        // The driver may not have refreshed status immediately after SET; a 0
        // here means "no status entry / not yet updated" — re-run
        // get-volt-rails to confirm.
        #[allow(non_snake_case)]
        let effective_wall_uV = gpu
            .volt_rails()
            .map_err(Error::from)?
            .and_then(|r| {
                r.status
                    .iter()
                    .find(|e| e.rail_bit == self.rail_bit)
                    .and_then(|e| e.values.get(4).copied())
            })
            .unwrap_or(0);
        Ok(Some(NvapiVoltRailTargetApplied {
            rail_bit: self.rail_bit,
            target_uV: self.target_uV,
            base_wall_uV,
            offset_uV,
            previous_offset_uV,
            applied_uV,
            effective_wall_uV,
        }))
    }
}

/// Result of a successful absolute-target volt-rail write.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // uV suffix matches the nvapi-rs field naming
pub struct NvapiVoltRailTargetApplied {
    pub rail_bit: u32,
    /// absolute target requested (µV)
    pub target_uV: i32,
    /// derived factory/default wall = target_wall − previous offset (µV)
    pub base_wall_uV: i32,
    /// derived offset actually written (µV) = target − base_wall
    pub offset_uV: i32,
    /// offset that was in effect before the write (µV)
    pub previous_offset_uV: i32,
    /// offset the driver retained (== offset_uV unless clamped)
    pub applied_uV: i32,
    /// effective wall after SET, read back from the status entry's index 4.
    /// The driver clamps this to `min(target, vbios_wall, vrm_max_wall)`, so
    /// it may be below the requested target. 0 = no status entry /
    /// driver hasn't refreshed yet (re-run get-volt-rails).
    pub effective_wall_uV: i32,
}

/// Query the controllable clock-domain block (private ClockClient
/// GetControl, RM 0x2080901b). The Blackwell XBar family
/// (reverse/melonvolt/xbar.txt). Returns `None` where the driver doesn't
/// expose the private interface.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkDomains;

impl GpuOperation for QueryNvapiClkDomains {
    type Output = Option<::nvapi::ClockDomainControl>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomains
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.clk_domains_control().map_err(Error::from)
    }
}

/// Detailed single-domain measure (private MEASURE_FREQ) — frequency plus
/// the second sample's raw {counter, timestamp, extra} and the accepted
/// protocol form (V1 0x10020 / V2 0x20020).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkDomainFreqDetail {
    pub domain_bit: u32,
}

impl GpuOperation for QueryNvapiClkDomainFreqDetail {
    type Output = Option<::nvapi::ClockDomainFreqDetail>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreqDetail
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .clk_domain_freq_detail(self.domain_bit)
            .map_err(Error::from)
    }
}

/// Write one V/F curve point via the private ClockClient V/F-POINTS
/// SetControl (ID 0xFEC00D04). DANGEROUS: snapshots the full control block,
/// patches one record (mode 0 freq-offset / mode 1 reverse-volt), SETs, readbacks,
/// restores on mismatch. `bank` 0 = V/F curve points, 1 = pstate-class;
/// `idx` 0..2048.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiVfpPointPrivate {
    pub bank: usize,
    pub idx: usize,
    pub freq_mode: bool,
    pub value: u32,
}

impl GpuOperation for SetNvapiVfpPointPrivate {
    type Output = Option<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiVfpPointPrivate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_vfp_point_private(self.bank, self.idx, self.freq_mode, self.value)
            .map_err(Error::from)
    }
}

/// Write a RANGE of V/F curve points with the same delta via the
/// private V/F-POINTS SetControl (ID 0xFEC00D04). Single RMW cycle.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiVfpRangePrivate {
    pub bank: usize,
    pub start: usize,
    pub end: usize,
    pub delta_mhz: i16,
}

impl GpuOperation for SetNvapiVfpRangePrivate {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiVfpRangePrivate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_vfp_range_private(self.bank, self.start, self.end, self.delta_mhz)
            .map_err(Error::from)
    }
}

/// Per-point variant of [`SetNvapiVfpRangePrivate`]: writes a DIFFERENT raw
/// mode-1 value to each point in `[start, end]` in a single RMW cycle.
/// `deltas.len()` must equal `end - start + 1`. Used by the CLI
/// `--raw-converted` path which translates one MHz target through each
/// point's own g(def) prior.
#[derive(Clone, Debug)]
pub struct SetNvapiVfpRangePerPointPrivate {
    pub bank: usize,
    pub start: usize,
    pub end: usize,
    pub deltas: Vec<i16>,
}

impl GpuOperation for SetNvapiVfpRangePerPointPrivate {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiVfpRangePrivate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_vfp_range_per_point_private(self.bank, self.start, self.end, &self.deltas)
            .map_err(Error::from)
    }
}

/// Reset every present V/F curve point on `bank` to default by clearing its
/// mode-0 (absolute kHz) override via the private V/F-POINTS SetControl
/// (ID 0xFEC00D04) in a single RMW cycle. Unlike `ResetPublicVftableOffset` /
/// `CoreResetVfp` (which route through the pstate20 or public Client
/// VfPoints families and cannot reach private mode-0 state), this writes
/// the same private SetControl that `SetNvapiVfpPointPrivate` uses.
/// Returns `Some(count)` of points written, or `None` where the family
/// is absent.
#[derive(Clone, Copy, Debug)]
pub struct ResetNvapiVfpPrivate {
    pub bank: usize,
    /// clear only points currently in this mode (0 = absolute kHz,
    /// 1 = raw delta); None clears both
    pub only_mode: Option<u8>,
}

impl GpuOperation for ResetNvapiVfpPrivate {
    type Output = Option<usize>;

    fn kind(&self) -> OperationKind {
        OperationKind::ResetNvapiVfpPrivate
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .reset_vfp_private(self.bank, self.only_mode.map(u32::from))
            .map_err(Error::from)
    }
}

// ---------------------------------------------------------------------------
// OC-gap wraps (2026-08-26 audit follow-up) — RE spec: docs/oc-gaps-re-spec.md
// ---------------------------------------------------------------------------

// NOTE (2026-08-28): QueryNvapiPowerMizer withdrawn. GetPowerMizerInfo
// (0x76BFA16B) is NOT a readback — elevated SET experiment (mode=6, both
// sources, rc=0) leaves the GET at its boot-time constant 7, and neither the
// NVCP power dropdown nor AC/DC transitions move it. The GET reports a
// constant; SetPowerMizerInfo (0x50016C78) has no runtime effect. Full
// evidence: docs/reverse-engineering/nvapi/power-mizer-corevolt-pmgr-semantics.md
// (probe: build/probe_pmizer.ps1).
// NOTE (2026-08-26): QueryNvapiDynamicBoost withdrawn. 0xC80068A1 reads the
// PCF controller table's platform status bytes (rec[+60]/rec[+61]), NOT the
// PPAB enable written by 0x1504FC3D — live-probed both bytes = 2 with PPAB
// enforcing (see nvapi-rs examples/probe_pcf_dynamic_boost.rs). The nvapi-rs
// layer keeps the wrap; re-expose only when a true readback is identified.

/// Core-voltage control-object GET (0xA91F88EB, escape 0x07000045) — half
/// of the RMW pair with [`SetNvapiCoreVoltageControl`]. Distinct SET path
/// from the VoltVoltRails µV-offset family.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiCoreVoltageControl;

impl GpuOperation for QueryNvapiCoreVoltageControl {
    type Output = Option<u32>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiCoreVoltageControl
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.core_voltage_control().map_err(Error::from)
    }
}

/// Core-voltage control SET (0xDC2BD4A6, escape 0x07000044). A third
/// voltage write path (distinct from VoltVoltRails offset and
/// ClientVoltRails percent). Elevation-gated (-104 without admin).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiCoreVoltageControl {
    pub value: u32,
}

impl GpuOperation for SetNvapiCoreVoltageControl {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiCoreVoltageControl
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_core_voltage_control(self.value)
            .map_err(Error::from)
    }
}

/// PMGR voltage-request arbiter GET (0x717648FD, escape 0x0700019F, v2
/// struct 0x20030). Calls the FFI directly instead of the hi-layer wrapper:
/// the wrapper collapses NVAPI_NOT_SUPPORTED (-104) and
/// NVAPI_NO_IMPLEMENTATION (-3) into the same `None`, hiding *why* the
/// surface is absent — live-probed, consumer SKUs return -104 because the
/// kernel-side method is not registered there at all (see
/// docs/reverse-engineering/nvapi/power-mizer-corevolt-pmgr-semantics.md).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiPmgrVoltageArbiter;

/// Probe outcome: the 11 raw arbiter dwords, or the raw NVAPI status code
/// that rejected the call.
#[derive(Clone, Copy, Debug)]
pub enum PmgrArbiterProbe {
    Values([u32; 11]),
    Unsupported { status_code: i32 },
}

/// Compact name for a raw NVAPI status code ("NotSupported", "Error", …),
/// `UNKNOWN` outside the known enum range.
pub fn nvapi_status_name(code: i32) -> String {
    match ::nvapi::sys::Status::from_raw(code) {
        Ok(status) => format!("{status:?}"),
        Err(_) => "UNKNOWN".to_string(),
    }
}

impl GpuOperation for QueryNvapiPmgrVoltageArbiter {
    type Output = PmgrArbiterProbe;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiPmgrVoltageArbiter
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        use ::nvapi::sys::gpu::power::undocumented::NV_PMGR_VOLTAGE_ARBITER_VALUES;
        use ::nvapi::sys::nvapi::NvVersion;
        use ::nvapi::sys::nvapi::VersionedStructField;

        let handle = *target.nvapi()?.inner().handle();
        let mut values = unsafe { std::mem::zeroed::<NV_PMGR_VOLTAGE_ARBITER_VALUES>() };
        *values.nvapi_version_mut() = NvVersion::with_version(0x20030);
        let status = unsafe {
            ::nvapi::sys::api::NvAPI_GPU_GetPMGRVoltageRequestArbiterValues(handle, &mut values)
        };
        if status == 0 {
            Ok(PmgrArbiterProbe::Values(values.values))
        } else {
            Ok(PmgrArbiterProbe::Unsupported {
                status_code: status,
            })
        }
    }
}

/// PMGR voltage-request arbiter SET (0x9C4BB8D0). Elevation-gated. Prefer
/// GET → patch → SET (raw dwords, semantics not yet calibrated).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPmgrVoltageArbiter {
    pub values: [u32; 11],
}

impl GpuOperation for SetNvapiPmgrVoltageArbiter {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPmgrVoltageArbiter
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_pmgr_voltage_arbiter(&self.values)
            .map_err(Error::from)
    }
}

/// Rated-TDP readback trio (0xED2BEA09 / 0x87BD35EF / 0xFCBDF642). Returns
/// `(control_mode, info_capabilities, status_raw[10])`.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiRatedTdp;

impl GpuOperation for QueryNvapiRatedTdp {
    type Output = Option<(u32, u8, [u32; 10])>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiRatedTdp
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.rated_tdp_readback().map_err(Error::from)
    }
}

/// Background OC-scanner enable/disable (0x06DC7CE8, 72B struct 0x10048
/// with the validated 9-byte feature GUID).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiBackgroundOcScanner {
    pub enable: bool,
}

impl GpuOperation for SetNvapiBackgroundOcScanner {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiBackgroundOcScanner
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .oem_oc_scanner_set_background(self.enable)
            .map_err(Error::from)
    }
}

/// Query the last INCOMPLETE OC-scanner run's partial results
/// (0xBE371D0A). `Ok(Some(()))` = call accepted (status-code semantics,
/// like `OemOcScannerStatus`).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiOcScannerIncomplete;

impl GpuOperation for QueryNvapiOcScannerIncomplete {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiOcScannerIncomplete
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .oem_oc_scanner_incomplete_results()
            .map_err(Error::from)
    }
}

/// Temperature-simulation GET (`NvAPI_GPU_GetThermalSimulationMode`) —
/// readback `(enable, temperature_celsius)` of the thermal-sim trio.
/// Gated by the driver's Secured-Overrides "Temp faking allowed" flag.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiThermalSim;

impl GpuOperation for QueryNvapiThermalSim {
    type Output = Option<(bool, i32)>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiThermalSim
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.temp_sim().map_err(Error::from)
    }
}

/// Temperature-simulation SET (Extended → basic fallback): fake the GPU
/// temperature the driver sees. DANGEROUS research tool; requires the
/// Secured-Overrides gate.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiThermalSim {
    pub temperature_c: i32,
}

impl GpuOperation for SetNvapiThermalSim {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiThermalSim
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_temp_sim(self.temperature_c)
            .map_err(Error::from)
    }
}

/// Temperature-simulation disable (restore the real sensor reading).
#[derive(Clone, Copy, Debug)]
pub struct DisableNvapiThermalSim;

impl GpuOperation for DisableNvapiThermalSim {
    type Output = Option<()>;

    fn kind(&self) -> OperationKind {
        OperationKind::DisableNvapiThermalSim
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.disable_temp_sim().map_err(Error::from)
    }
}

// NOTE (2026-08-26): the PerfVfeEqu/PerfVfeVar ×4 query operations were
// deliberately withdrawn from core/CLI/Python — the surface is not yet
// calibrated enough to expose to users (equ-control records are
// variable-length and remain raw). The nvapi-rs layer keeps the full
// wrap + tests; re-expose here only after per-type field decoding lands.

/// Batch-measure physical clocks for a set of domains via the V3
/// MEASURE_FREQ (RM 0x20809006, magic 0x30038) — one RM round-trip per
/// sample for the whole set, with per-domain V1/V2 fallback.
#[derive(Clone, Debug)]
pub struct QueryNvapiClkDomainFreqsBatch {
    /// sequential domain indices (GPC=0, XBAR=1, SYS=2, MCLK=4, …)
    pub domains: Vec<u32>,
}

impl GpuOperation for QueryNvapiClkDomainFreqsBatch {
    type Output = Option<Vec<::nvapi::ClockDomainFreq>>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreqsBatch
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .clk_domain_freqs_batch(&self.domains)
            .map_err(Error::from)
    }
}

/// Query the private ClockClient V/F-POINTS read path (GetInfo 0x8895B510 →
/// GetStatus 0x7FEE9032, RM 0x20809061/0x20809062) — the article's per-domain
/// V/F curve family. Returns `None` where the driver doesn't expose the
/// private interface. Units live-calibrated vs the public GPC VFP curve
/// (see `::nvapi::ClkVfPointPrivate`).
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryNvapiClkVfPoints {
    /// Attach the raw 488B GetStatus records (diagnostic slot-map dumps —
    /// ~64KB per 132-point table). The normal read leaves them empty.
    pub include_raw: bool,
}

impl From<bool> for QueryNvapiClkVfPoints {
    fn from(include_raw: bool) -> Self {
        Self { include_raw }
    }
}

impl GpuOperation for QueryNvapiClkVfPoints {
    type Output = Option<::nvapi::ClkVfPointsPrivate>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkVfPoints
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        let out = if self.include_raw {
            gpu.clk_vf_points_private_raw()
        } else {
            gpu.clk_vf_points_private()
        };
        out.map_err(Error::from)
    }
}

/// Read the private V/F-POINTS CONTROL override table (GetControl
/// 0xDA025C3E, masks seeded from GetInfo): per-point mode (0 = absolute
/// kHz offset / 1 = raw delta) + value — the direct readback of raw
/// control values written by `SetNvapiVfpPointPrivate` & friends. Returns
/// `None` where the driver doesn't expose the private interface.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkVfControl;

impl GpuOperation for QueryNvapiClkVfControl {
    type Output = Option<::nvapi::ClkVfControlPrivate>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkVfControl
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .clk_vf_control_private()
            .map_err(Error::from)
    }
}

/// Read the full VBIOS image via `NvAPI_GPU_GetVbiosImage` (0xFC13EE11,
/// escape 0x0700004F). On legacy drivers (391.35) this escape succeeds where
/// the VFP-curve escape 0x0700004A is kernel-unimplemented, making this the
/// viable path to the V/F curve (BIT VoltageTable) on old GPUs.
#[derive(Clone, Copy, Debug)]
pub struct QueryVbiosImage;

impl GpuOperation for QueryVbiosImage {
    type Output = Vec<u8>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVbiosImage
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.vbios_image().map_err(Error::from)
    }
}

/// Read the VBIOS version string (e.g. "70.08.0F.00.05") via
/// `NvAPI_GPU_GetVbiosVersionString` — the brief companion to
/// [`QueryVbiosImage`].
#[derive(Clone, Copy, Debug)]
pub struct QueryVbiosVersion;

impl GpuOperation for QueryVbiosVersion {
    type Output = String;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVbiosVersion
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.vbios_version_string().map_err(Error::from)
    }
}

/// Read the VBIOS security configuration word via
/// `NvAPI_GPU_GetVbiosSecurityInfo` (0x8d3ac6b9, struct stamp 0x1000C).
/// Raw flags dword — P100 server/TCC reads 0x0203; bit semantics
/// driver-opaque (compare across SKUs before assigning meaning).
#[derive(Clone, Copy, Debug)]
pub struct QueryVbiosSecurityInfo;

impl GpuOperation for QueryVbiosSecurityInfo {
    type Output = u32;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVbiosSecurityInfo
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.vbios_security_flags().map_err(Error::from)
    }
}

/// Read the human-readable VBIOS status via
/// `NvAPI_GPU_GetVbiosStatusString` (0x8011c22c). State-dependent text —
/// don't parse; compare across cards/states.
#[derive(Clone, Copy, Debug)]
pub struct QueryVbiosStatusString;

impl GpuOperation for QueryVbiosStatusString {
    type Output = String;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryVbiosStatusString
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.vbios_status_string().map_err(Error::from)
    }
}

/// Measure one clock-domain's physical clock (private ClockClient
/// MEASURE_FREQ, RM 0x20809006) via two-sample Δcounter/Δtimestamp.
/// `domain_bit` is the sequential domain index (GPC=0, XBAR=1, SYS=2, MCLK=4).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkDomainFreq {
    pub domain_bit: u32,
}

impl GpuOperation for QueryNvapiClkDomainFreq {
    type Output = Option<::nvapi::ClockDomainFreq>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreq
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .clk_domain_freq(self.domain_bit)
            .map_err(Error::from)
    }
}

/// Direct physical clock for one domain — the green-curve MEASURE path
/// (ID 0x527FC458). One call returns `freq_khz` (no two-sample Δt + sleep).
/// `domain_bit`: GPC=0, XBAR=1, SYS=2, MCLK=4, HOST=5. `freq_khz == 0` when
/// the driver refuses / the domain isn't measurable through this interface.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkDomainFreqDirect {
    pub domain_bit: u32,
}

impl GpuOperation for QueryNvapiClkDomainFreqDirect {
    type Output = Option<::nvapi::ClockDomainFreqDirect>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreqDirect
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        // hi wrapper already folds NotSupported/NoImplementation → Ok(None).
        target
            .nvapi()?
            .clk_domain_freq_direct(self.domain_bit)
            .map_err(Error::from)
    }
}

/// Write a signed kHz offset into one clock-domain's control record (private
/// ClockClient SET_CONTROL, RM 0x2080d01c). DANGEROUS GPU clock write: the
/// operation snapshots the full GetControl block, version-gates (magic
/// 0x10964), patches a copy, SETs, readbacks, and restores on mismatch. If
/// `temporary`, the snapshot is restored before returning (the article's
/// reversible experiment recipe, xbar.txt:62-72).
///
/// No magnitude limit is enforced — the caller owns offset/range policy (the
/// article bounds XBAR ±60000 kHz on GB202). The driver may reject or clamp
/// the offset; the post-SET readback surfaces what was actually retained.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // kHz suffix matches the nvapi-rs field naming
pub struct SetNvapiClkDomainOffset {
    /// domain index / mask bit (XBAR=1)
    pub domain_bit: u32,
    /// signed kHz offset to write (0 = stock)
    pub offset_kHz: i32,
    /// which of the record's 8 value dwords to write (0-7). Identified
    /// semantics: slot 0 = the article's signed frequency offset (kHz);
    /// slot 1 = the V/F-curve horizontal voltage shift in µV (live
    /// V100/GV100 2026-09-01 — slides the whole curve along the voltage
    /// axis; a GPC slot1 shift breaks get-public-vftable readback). The
    /// rest are driver-opaque range/voltage terms
    pub slot: u32,
    /// if true, restore the pre-write snapshot before returning (safe
    /// experiment mode); if false, persist the offset
    pub temporary: bool,
}

impl GpuOperation for SetNvapiClkDomainOffset {
    type Output = Option<NvapiClkDomainOffsetApplied>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiClkDomainOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        // guard BEFORE the previous-value lookup below indexes the record's
        // 8 value dwords — the medium layer checks too, but only after this
        let slot = self.slot as usize;
        if slot >= 8 {
            return Err(Error::Custom(
                "invalid slot: the clock-domain record has 8 value dwords (0-7)".to_string(),
            ));
        }
        let gpu = target.nvapi()?;
        // Capture the previous offset for the result, if the family is present.
        #[allow(non_snake_case)] // kHz suffix matches the nvapi-rs field naming
        let previous_kHz = gpu
            .clk_domains_control()
            .ok()
            .flatten()
            .and_then(|c| {
                c.entries
                    .iter()
                    .find(|e| e.bit == self.domain_bit)
                    .map(|e| e.values_kHz[slot])
            })
            .unwrap_or(0);
        let applied = gpu
            .set_clk_domain_offset(self.domain_bit, self.offset_kHz, self.slot, self.temporary)
            .map_err(Error::from)?;
        Ok(applied.map(|entry| NvapiClkDomainOffsetApplied {
            bit: entry.bit,
            entry_type: entry.entry_type,
            slot: self.slot,
            previous_kHz,
            applied_kHz: entry.values_kHz[slot],
            values_kHz: entry.values_kHz,
            temporary_restored: self.temporary,
        }))
    }
}

/// Result of a successful clock-domain offset write.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)] // kHz suffix matches the nvapi-rs field naming
pub struct NvapiClkDomainOffsetApplied {
    /// domain index / mask bit
    pub bit: u32,
    /// record type byte (0x0A on offset-capable domains)
    pub entry_type: u8,
    /// value-dword slot that was written (0-7)
    pub slot: u32,
    /// slot value in effect before the write (kHz, semantics per slot)
    pub previous_kHz: i32,
    /// slot value the driver retained (== requested unless rejected/clamped)
    pub applied_kHz: i32,
    /// the record's full 8 value dwords after the write (driver-opaque slots)
    pub values_kHz: [i32; 8],
    /// whether the pre-write snapshot was restored (temporary mode)
    pub temporary_restored: bool,
}

/// Set the ECC memory configuration (public `NvAPI_GPU_SetECCConfiguration`
/// 0x1CF639D9): `enable` turns ECC on/off, `immediately` applies the change
/// now instead of deferring it to the next reboot. The configuration is
/// stored in non-volatile memory either way — this is NOT a readback-style
/// SET; the post-write state comes from `GetECCConfigurationInfo`
/// (0x77A796F3), returned as the operation output when readable.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiEccConfiguration {
    /// desired ECC enable state
    pub enable: bool,
    /// apply immediately (NV_ECC_CONFIGURATION_IMMEDIATE) instead of
    /// persisting for the next reboot (DEFERRED)
    pub immediately: bool,
}

/// Result of a successful ECC configuration write: the NV-stored state
/// read back after the SET.
#[derive(Clone, Copy, Debug)]
pub struct NvapiEccConfigurationApplied {
    /// ECC enabled in the persistent configuration
    pub enabled: bool,
    /// factory default ECC configuration (static)
    pub enabled_by_default: bool,
}

impl GpuOperation for SetNvapiEccConfiguration {
    type Output = Option<NvapiEccConfigurationApplied>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiEccConfiguration
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        gpu.inner()
            .ecc_configure(self.enable, self.immediately)
            .map_err(Error::from)?;
        // readback from the NV-stored configuration; a GET failure after a
        // successful SET is surfaced as None, not an error
        let readback = gpu
            .inner()
            .ecc_configuration()
            .ok()
            .map(
                |(enabled, enabled_by_default)| NvapiEccConfigurationApplied {
                    enabled,
                    enabled_by_default,
                },
            );
        Ok(readback)
    }
}

/// Set the D-Notifier (D0-notify) limit to a D level (1..5). Maps the CLI
/// level to the signed driver code (-1=D1/Unlimited, 0..3=D2..D5) exactly as
/// the ref tool's `[GPUHandle::setDNotifyLimit]` switch does, then calls the raw
/// two-arg setter (NDA 0x48E0847D).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiDNotifier {
    /// D level, 1..5.
    pub level: u8,
}

impl SetNvapiDNotifier {
    /// Map D level (1..5) to the signed driver D-index code the setter takes.
    /// Returns `Err` for out-of-range levels.
    fn driver_index(level: u8) -> Result<i32, Error> {
        match level {
            1 => Ok(-1),
            2 => Ok(0),
            3 => Ok(1),
            4 => Ok(2),
            5 => Ok(3),
            _ => Err(Error::Custom(format!(
                "D-Notifier level must be 1..5 (D1-D5), got {level}"
            ))),
        }
    }
}

impl GpuOperation for SetNvapiDNotifier {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiDNotifier
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let didx = Self::driver_index(self.level)?;
        target.nvapi()?.set_dnotify_limit(didx).map_err(Error::from)
    }
}

/// Query the native P-State level table (the the ref tool `-pstate` GET listing) via
/// the private PerfPstatesGetInfo (NDA 0x7B30AE0D): present P-States with their
/// min/max clock for the given clock-domain (0=GPC/core by default; the ref tool
/// resolves the GPC index via 0x57B5A5DF). Returns `None` where the driver
/// doesn't expose the private interface. Clocks are converted kHz → MHz.
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryNvapiPStateLevels {
    /// Clock-domain index (0=GPC/core default).
    pub domain: usize,
}

impl GpuOperation for QueryNvapiPStateLevels {
    type Output = Option<PStateLevelsInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiPStateLevels
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .pstate_levels_domain(self.domain)
            .map_err(Error::from)?
            .map(|r| PStateLevelsInfo {
                pstates: r
                    .pstates
                    .iter()
                    .map(|p| PStateLevelEntry {
                        pstate: p.pstate,
                        min_mhz: p.min_khz.map(|khz| khz as f64 / 1000.0),
                        max_mhz: p.max_khz.map(|khz| khz as f64 / 1000.0),
                    })
                    .collect(),
            }))
    }
}

/// Query the set of P-State numbers currently locked (via
/// PerfClientLimitsSetStatus 0x39442CFB), from the private
/// ClientPStateLimitStatus (NDA 0x9962C97C). Returns `None` where the driver
/// doesn't expose the private interface; an empty `Vec` means nothing locked.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiPStateLockStatus;

impl GpuOperation for QueryNvapiPStateLockStatus {
    type Output = Option<Vec<u8>>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiPStateLockStatus
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.pstate_lock_status().map_err(Error::from)
    }
}

/// Set the native NVAPI P-State lock (the the ref tool `-pstate:<index>` SETTER) via
/// PerfClientLimitsSetStatus (NDA 0x39442CFB). See [`NvapiPStateNativeLock`] for
/// the lock shapes (reset / pstate-only / pstate+frequency).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPStateNative {
    pub lock: NvapiPStateNativeLock,
}

impl GpuOperation for SetNvapiPStateNative {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPStateNative
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let lock = match self.lock {
            NvapiPStateNativeLock::Reset => ::nvapi::hi::PStateNativeLock::Reset,
            NvapiPStateNativeLock::PstateOnly { pstate } => {
                ::nvapi::hi::PStateNativeLock::PstateOnly { pstate }
            }
            NvapiPStateNativeLock::PstateAndFreq { pstate, freq_khz } => {
                ::nvapi::hi::PStateNativeLock::PstateAndFreq { pstate, freq_khz }
            }
        };
        target.nvapi()?.set_pstate_native(lock).map_err(Error::from)
    }
}

/// Set the GPU frequency perf-cap (the ref tool `-gpuclk:<MHz>` SETTER,
/// PerfLimitsSetStatus NDA 0x32CA4983). Clamps the perf max/min frequency to
/// a cap value — NOT an offset, NOT a P-state lock (see [`SetNvapiPStateNative`]).
/// `freq_khz` is MHz × 1000; `Reset` clears the cap (`-gpuclk:-1`).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPerfFreqCap {
    pub cap: NvapiPerfFreqCap,
}

impl GpuOperation for SetNvapiPerfFreqCap {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPerfFreqCap
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let cap = match self.cap {
            NvapiPerfFreqCap::Reset => ::nvapi::hi::PerfFreqCap::Reset,
            NvapiPerfFreqCap::Cap { max_khz, min_khz } => {
                ::nvapi::hi::PerfFreqCap::Cap { max_khz, min_khz }
            }
        };
        target.nvapi()?.set_perf_freq_cap(cap).map_err(Error::from)
    }
}

/// Toggle the overclocked-pstate unlock (EnableOverclockedPstates NDA
/// 0xB23B70EE, escape 0x070000BA). enable=true opens the extended/OC pstate
/// range — run BEFORE a SetPstates20 delta write so the delta can exceed the
/// stock VBIOS clamp (P100 pstate-delta-plane experiment entry).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiOverclockedPstates {
    pub enable: bool,
}

impl GpuOperation for SetNvapiOverclockedPstates {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiOverclockedPstates
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .enable_overclocked_pstates(self.enable)
            .map_err(Error::from)
    }
}

/// Read-only raw dump of the private pstates-2.0 delta table
/// (GetPstates20Private 0xC5DDF56E) — the frequency-ceiling "plane A"
/// storage. Returns header fields + the raw buffer.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiPstates20Private {
    /// Version stamp: 81044 (base) or 146840 (extended tail).
    pub stamp: u32,
}

/// One clock slot of a private-pstates pstate block (raw table words; no
/// unit is asserted — the native delta unit is percent-of-domainMax per the
/// public-path marshalling but is unverified per SKU).
pub struct Pstates20PrivateClock {
    pub domain_id: u32,
    pub fmt: u32,
    pub enabled: bool,
    pub delta_raw: i32,
}

/// One pstate block of the private pstates table.
pub struct Pstates20PrivatePstate {
    pub pstate_id: u32,
    pub enabled: bool,
    pub clocks: Vec<Pstates20PrivateClock>,
}

/// Parsed header of the private pstates table (user layout, little-endian).
pub struct Pstates20PrivateDump {
    pub stamp: u32,
    pub caps_editable: bool,
    pub flags_raw: u32,
    pub num_pstates: u32,
    pub num_clocks: u32,
    pub num_voltages: u32,
    pub pstates: Vec<Pstates20PrivatePstate>,
    pub raw_len: usize,
}

fn parse_pstates20_private(buf: &[u8]) -> Pstates20PrivateDump {
    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_ne_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
    let stamp = u32_at(buf, 0);
    let flags_raw = u32_at(buf, 4);
    let num_pstates = u32_at(buf, 8);
    let num_clocks = u32_at(buf, 12);
    let num_voltages = u32_at(buf, 16);
    let mut pstates = Vec::new();
    for i in 0..num_pstates.min(32) {
        let base = 20 + 968 * i as usize;
        if base + 968 > buf.len() {
            break;
        }
        let pstate_id = u32_at(buf, base);
        let enabled = u32_at(buf, base + 4) & 1 == 1;
        let mut clocks = Vec::new();
        for j in 0..num_clocks.min(22) {
            let slot = base + 8 + 44 * j as usize;
            if slot + 44 > buf.len() {
                break;
            }
            clocks.push(Pstates20PrivateClock {
                domain_id: u32_at(buf, slot),
                fmt: u32_at(buf, slot + 4),
                enabled: u32_at(buf, slot + 8) & 1 == 1,
                delta_raw: u32_at(buf, slot + 12) as i32,
            });
        }
        pstates.push(Pstates20PrivatePstate {
            pstate_id,
            enabled,
            clocks,
        });
    }
    Pstates20PrivateDump {
        stamp,
        caps_editable: flags_raw & 1 == 1,
        flags_raw,
        num_pstates,
        num_clocks,
        num_voltages,
        pstates,
        raw_len: buf.len(),
    }
}

impl GpuOperation for QueryNvapiPstates20Private {
    type Output = Pstates20PrivateDump;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiPstates20Private
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let buf = target
            .nvapi()?
            .pstates20_private_raw(self.stamp)
            .map_err(Error::from)?;
        Ok(parse_pstates20_private(&buf))
    }
}

/// RMW write of one delta in the private pstates-2.0 table
/// (SetPstates20Private 0x4C0B519A): GET → locate the (pstate, domain) clock
/// slot → patch the delta dword → SET → GET verify. `delta` is in the
/// table's native percent-of-domainMax units. `domain_raw`/`pstate_id` use
/// the raw ids found by [`QueryNvapiPstates20Private`] (0xFFFF wildcard
/// matches the first slot with that id).
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPstates20PrivateDelta {
    pub pstate_id: u32,
    pub domain_raw: u32,
    pub delta: i32,
    /// Extra bits ORed into the flags word at byte@+4 (bit1 = the RM apply
    /// flag the public path sets from NV_GPU_PERF_PSTATES20_INFO bit1).
    pub flags: u32,
}

impl GpuOperation for SetNvapiPstates20PrivateDelta {
    type Output = i32;

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPstates20PrivateDelta
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let gpu = target.nvapi()?;
        const STAMP: u32 = 81044;
        let mut buf = gpu.pstates20_private_raw(STAMP).map_err(Error::from)?;
        if self.flags != 0 {
            buf[4] |= (self.flags & 0xFF) as u8;
        }

        fn u32_at(b: &[u8], off: usize) -> u32 {
            u32::from_ne_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
        }
        fn set_u32(b: &mut [u8], off: usize, v: u32) {
            b[off..off + 4].copy_from_slice(&v.to_ne_bytes());
        }

        let num_pstates = u32_at(&buf, 8).min(32);
        let num_clocks = u32_at(&buf, 12).min(22);
        let mut hit = None;
        let mut slot_disabled = false;
        'outer: for i in 0..num_pstates {
            let base = 20 + 968 * i as usize;
            if base + 968 > buf.len() {
                break;
            }
            let pid = u32_at(&buf, base);
            if pid != self.pstate_id {
                continue;
            }
            for j in 0..num_clocks {
                let slot = base + 8 + 44 * j as usize;
                if u32_at(&buf, slot) == self.domain_raw {
                    slot_disabled = u32_at(&buf, slot + 8) & 1 == 0;
                    hit = Some(slot + 12);
                    break 'outer;
                }
            }
        }
        if slot_disabled {
            return Err(Error::from(format!(
                "the pstate {} domain {} slot is DISABLED in the private table \
                 (see get-private-legacy-pstates20-freq-domain-info) — the kernel rejects writes to \
                 disabled slots with NVAPI_ERROR",
                self.pstate_id, self.domain_raw
            )));
        }
        let off = hit.ok_or_else(|| {
            Error::from(format!(
                "no clock slot with pstate_id={} domain_raw={} in the private pstates table",
                self.pstate_id, self.domain_raw
            ))
        })?;
        set_u32(&mut buf, off, self.delta as u32);
        gpu.set_pstates20_private_raw(&buf).map_err(Error::from)?;

        // Verify via fresh GET.
        let verify = gpu.pstates20_private_raw(STAMP).map_err(Error::from)?;
        let got = u32_at(&verify, off) as i32;
        if got != self.delta {
            return Err(Error::from(format!(
                "driver did not retain the delta (wrote {}, read back {})",
                self.delta, got
            )));
        }
        Ok(got)
    }
}

/// Query every NVAPI target-temperature (温度墙) policy slot the driver exposes
/// (private ClientThermalTarget GET-prime 0xC4554575). Returns one
/// [`TargetTempPolicy`] per slot; empty on GPUs/driver paths that don't expose
/// the table. Drives the `--nvapi` branch of `get-temp-thresholds` and
/// lets callers discover which `policy_index` is the "GPU Target Temperature"
/// wall (idx 2 on RTX 4060 Laptop) instead of hardcoding it.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiTargetTempPolicies;

impl GpuOperation for QueryNvapiTargetTempPolicies {
    type Output = Vec<TargetTempPolicy>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiTargetTempPolicies
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .target_temperature_policies_with_info()
            .map_err(Error::from)?
            .into_iter()
            .map(|entry| TargetTempPolicy {
                policy_index: entry.policy_index,
                celsius: entry.current,
                min: entry.min,
                default: entry.default,
                max: entry.max,
            })
            .collect())
    }
}

/// The auto-discovered target-temp policy index (private GetInfo 0x2F69F8E5):
/// GPS index if the VBIOS exposes one, else the acoustics fallback (desktop =
/// NVML AcousticCurr), else None. Lets the CLI tag the wall slot without
/// hardcoding idx 2 or touching the crate-private `target.nvapi()`.
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiTargetTempPolicyIndex;

impl GpuOperation for QueryNvapiTargetTempPolicyIndex {
    type Output = Option<usize>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiTargetTempPolicyIndex
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .target_temp_policy_index()
            .map_err(Error::from)
    }
}

/// Set one NVAPI target-temperature (温度墙) policy slot (private RMW:
/// GET-prime 0xC4554575 + SET 0xE097144F). `policy_index` defaults to the
/// auto-discovered slot (private GetInfo: GPS idx, else acoustics fallback) —
/// pass one explicitly to override or probe writability of other slots.
#[derive(Clone, Debug, Default)]
pub struct SetNvapiTargetTemp {
    pub celsius: f32,
    pub policy_index: Option<usize>,
}

impl GpuOperation for SetNvapiTargetTemp {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiTargetTemp
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        // Default policy_index is auto-discovered via the private GetInfo
        // (GPS idx, else acoustics fallback) rather than hardcoded. Falls back
        // to 2 only if discovery itself fails (legacy path).
        let idx = match self.policy_index {
            Some(i) => i,
            None => target
                .nvapi()?
                .target_temp_policy_index()
                .map_err(Error::from)?
                .unwrap_or(2),
        };
        target
            .nvapi()?
            .set_target_temperature(self.celsius, idx)
            .map_err(Error::from)
    }
}

#[derive(Clone, Debug)]
pub struct SetNvapiSensorLimits {
    pub limits: Vec<SensorThrottle>,
}

impl GpuOperation for SetNvapiSensorLimits {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiSensorLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .set_sensor_limits(self.limits.iter().cloned())
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetNvapiPowerLimits;

impl GpuOperation for ResetNvapiPowerLimits {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetNvapiPowerLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let info = target.nvapi()?.info()?;
        target
            .nvapi()?
            .set_power_limits(info.power_limits.iter().map(|info| info.default))
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetNvapiSensorLimits;

impl GpuOperation for ResetNvapiSensorLimits {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetNvapiSensorLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let info = target.nvapi()?.info()?;
        target
            .nvapi()?
            .set_sensor_limits(
                info.sensor_limits
                    .iter()
                    .cloned()
                    .map(SensorThrottle::from_default),
            )
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetCoolerLevels;

impl GpuOperation for ResetCoolerLevels {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetCoolerLevels
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.reset_cooler_levels().map_err(Error::from)
    }
}

#[derive(Clone, Debug)]
pub struct ResetPstateGlobalFreqOffset {
    pub offsets: Vec<(PState, ClockDomain)>,
}

impl GpuOperation for ResetPstateGlobalFreqOffset {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetPstateGlobalFreqOffset
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .set_pstates(
                self.offsets
                    .iter()
                    .map(|&(pstate, clock)| (pstate, clock, KilohertzDelta(0))),
            )
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryTdpTempLimits;

impl GpuOperation for QueryTdpTempLimits {
    type Output = TdpTempLimits;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryTdpTempLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let (min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp, throttle_curve) =
            low_nvapi::get_gpu_tdp_temp_limit(&[target.nvapi()?], || {})?;
        Ok(TdpTempLimits {
            min_tdp,
            default_tdp,
            max_tdp,
            min_temp,
            default_temp,
            max_temp,
            throttle_curve,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProbeVoltageLimits;

impl GpuOperation for ProbeVoltageLimits {
    type Output = super::result::VoltageLimits;

    fn kind(&self) -> OperationKind {
        OperationKind::ProbeVoltageLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let (lower_point, upper_point) =
            low_nvapi::handle_test_voltage_limits(&[target.nvapi()?], || {})?;
        Ok(super::result::VoltageLimits {
            lower_point,
            upper_point,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CheckVoltageFrequency {
    pub point: usize,
}

impl GpuOperation for CheckVoltageFrequency {
    type Output = VoltageFrequencyCheck;

    fn kind(&self) -> OperationKind {
        OperationKind::CheckVoltageFrequency
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let (precise, matched_point) =
            low_nvapi::voltage_frequency_check(&[target.nvapi()?], self.point, || {})?;
        Ok(VoltageFrequencyCheck {
            precise,
            matched_point,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryDisplays {
    pub all: bool,
}

impl GpuOperation for QueryDisplays {
    type Output = Vec<DisplayInfo>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryDisplays
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::query_displays(target.nvapi()?, self.all)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryEdid {
    pub display_id: u32,
}

impl GpuOperation for QueryEdid {
    type Output = EdidData;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryEdid
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let bytes = target
            .nvapi()?
            .inner()
            .get_edid(self.display_id)
            .map_err(Error::from)?;
        Ok(EdidData {
            display_id: self.display_id,
            bytes,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SetEdid {
    pub display_id: u32,
    pub bytes: Vec<u8>,
}

impl GpuOperation for SetEdid {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetEdid
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .set_edid(self.display_id, &self.bytes)
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ClearEdid {
    pub display_id: u32,
}

impl GpuOperation for ClearEdid {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ClearEdid
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target
            .nvapi()?
            .inner()
            .clear_edid(self.display_id)
            .map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetLegacyClocks {
    pub core_mhz: u32,
    pub memory_mhz: u32,
}

impl GpuOperation for SetLegacyClocks {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetLegacyClocks
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_legacy_clocks_nvapi(target.nvapi()?, self.core_mhz, self.memory_mhz)
    }
}

/// Lock one NVML P-State or a contiguous P-State range through NVAPI.
///
/// This is a logical P-State operation in the structured API. Internally it
/// queries NVML P-State memory clock ranges, derives a memory VFP frequency
/// window, warns (but proceeds) when the window also overlaps P-States
/// outside the requested range — identical memory clocks across P-States
/// (e.g. a VBIOS edit pinning P2 to P0's clocks) make the ranges inseparable
/// by construction — then applies the window with NVAPI.
///
/// The output is `(range_label, min_lock_mhz, max_lock_mhz, warning)`.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPstateLock {
    pub first_pstate: PerformanceState,
    pub second_pstate: PerformanceState,
}

impl GpuOperation for SetNvapiPstateLock {
    type Output = (String, u32, u32, Option<String>);

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvapiPstateLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_nvapi_pstate_lock(
            target.nvml()?,
            target.nvapi()?,
            target.id.0,
            self.first_pstate,
            self.second_pstate,
        )
    }
}

/// Lock one NVML P-State or a contiguous P-State range through NVML.
///
/// This is a logical P-State operation in the structured API. Internally it
/// queries NVML P-State memory clock ranges, derives a memory locked-clock
/// window, warns (but proceeds) when the window also overlaps P-States
/// outside the requested range (same policy as the NVAPI variant — see
/// [`SetNvapiPstateLock`]), then applies the window with NVML memory locked
/// clocks.
///
/// The output is `(range_label, min_lock_mhz, max_lock_mhz, warning)`.
#[derive(Clone, Copy, Debug)]
pub struct SetNvmlPstateLock {
    pub first_pstate: PerformanceState,
    pub second_pstate: PerformanceState,
}

impl GpuOperation for SetNvmlPstateLock {
    type Output = (String, u32, u32, Option<String>);

    fn kind(&self) -> OperationKind {
        OperationKind::SetNvmlPstateLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_pstate_lock(
            target.nvml()?,
            target.id.0,
            self.first_pstate,
            self.second_pstate,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryAutoBoost;

impl GpuOperation for QueryAutoBoost {
    type Output = AutoBoostState;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryAutoBoost
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let (enabled, default_enabled) =
            low_nvml::query_nvml_auto_boost(target.nvml()?, target.id.0)?;
        Ok(AutoBoostState {
            enabled,
            default_enabled,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetAutoboostStatus {
    pub enabled: bool,
}

impl GpuOperation for SetAutoboostStatus {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetAutoboostStatus
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_auto_boost(target.nvml()?, target.id.0, self.enabled)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetAutoboostStatus {
    pub enabled: bool,
}

impl GpuOperation for ResetAutoboostStatus {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetAutoboostStatus
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_auto_boost_default(target.nvml()?, target.id.0, self.enabled)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueryApiRestriction {
    pub api_type: Api,
}

impl GpuOperation for QueryApiRestriction {
    type Output = ApiRestrictionState;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryApiRestriction
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        let restricted =
            low_nvml::query_nvml_api_restriction(target.nvml()?, target.id.0, self.api_type)?;
        Ok(ApiRestrictionState {
            api_type: self.api_type,
            restricted,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetAutoboostSupport {
    pub api_type: Api,
    pub restricted: bool,
}

impl GpuOperation for SetAutoboostSupport {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetAutoboostSupport
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_api_restriction(
            target.nvml()?,
            target.id.0,
            self.api_type,
            self.restricted,
        )
    }
}

pub fn parse_nvapi_locked_voltage_target(raw: &str) -> Result<NvapiLockedVoltageTarget, Error> {
    low_nvapi::parse_nvapi_locked_voltage_target(raw)
}

pub fn parse_nvml_fan_control_policy(policy_raw: &str) -> Result<FanControlPolicy, Error> {
    low_nvml::parse_nvml_fan_control_policy(policy_raw)
}

pub fn try_parse_nvml_pstate(raw: &str) -> Result<PerformanceState, Error> {
    super::conv::try_parse_nvml_pstate(raw)
}

pub fn nvml_pstate_to_str(pstate: PerformanceState) -> &'static str {
    super::conv::nvml_pstate_to_str(pstate)
}

pub fn nvml_pstate_to_index(pstate: PerformanceState) -> Result<u8, Error> {
    super::conv::nvml_pstate_to_index(pstate)
}

pub fn parse_nvml_pstate(raw: &str) -> Result<PerformanceState, Error> {
    try_parse_nvml_pstate(raw)
}

pub fn detect_gpu_type(gpu_name: &str, codename: &str) -> super::gpu_type::GpuType {
    super::gpu_type::detect_gpu_type(gpu_name, codename)
}

pub fn fetch_gpu_type(info: &::nvapi::hi::GpuInfo) -> Result<super::gpu_type::GpuType, Error> {
    super::gpu_type::fetch_gpu_type(info)
}

pub fn find_matching_vfp_point(
    vfp_table: &std::collections::BTreeMap<usize, ::nvapi::hi::VfpPoint>,
    sensor_v: ::nvapi::hi::Microvolts,
) -> Option<(&usize, &::nvapi::hi::VfpPoint)> {
    low_nvapi::find_matching_vfp_point(vfp_table, sensor_v)
}

pub fn oc_params(gpu_type: super::gpu_type::GpuType) -> super::gpu_type::GpuOcParams {
    gpu_type.oc_params()
}

pub fn percentage(value: u32) -> Percentage {
    Percentage(value)
}

pub fn set_nvapi_vfp_curve_delta(
    target: &GpuTarget<'_>,
    point: usize,
    vfp_set_range: usize,
    flat_curve: bool,
    main_delta: i32,
    lower_delta: Option<i32>,
) -> Result<(), Error> {
    if !flat_curve {
        let start = point.checked_sub(vfp_set_range).ok_or_else(|| {
            Error::Custom(format!(
                "invalid VFP range: point ({point}) is smaller than range ({vfp_set_range})"
            ))
        })?;
        run(
            target,
            SetPublicVftableRangeOffset {
                start,
                end: point + vfp_set_range,
                delta: KilohertzDelta(main_delta),
            },
        )?;
    } else {
        run(
            target,
            SetPublicVftableRangeOffset {
                start: point,
                end: point + vfp_set_range,
                delta: KilohertzDelta(main_delta),
            },
        )?;
        if let Some(ld) = lower_delta {
            let start = point.checked_sub(vfp_set_range).ok_or_else(|| {
                Error::Custom(format!(
                    "invalid VFP range: point ({point}) is smaller than range ({vfp_set_range})"
                ))
            })?;
            let end = point.checked_sub(1).ok_or_else(|| {
                Error::Custom("invalid VFP range: point must be greater than 0".to_string())
            })?;
            run(
                target,
                SetPublicVftableRangeOffset {
                    start,
                    end,
                    delta: KilohertzDelta(ld),
                },
            )?;
        }
    }
    Ok(())
}

pub fn set_nvapi_domain_vfp_deltas(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
    deltas: &[(usize, KilohertzDelta)],
) -> Result<(), Error> {
    run(
        target,
        SetDomainVfpDeltas {
            domain,
            deltas: deltas.to_vec(),
        },
    )
    .map(|report| report.output)
}

pub fn query_domain_vf_points_indexed(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
    infer_missing_default: bool,
) -> Result<Vec<(usize, VfPoint)>, Error> {
    run(
        target,
        QueryDomainVfpPoints {
            domain,
            infer_missing_default,
            indexed: true,
        },
    )
    .map(|report| report.output)
}

pub fn query_domain_vfp_indices(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
) -> Result<Vec<usize>, Error> {
    run(target, QueryDomainVfpIndices { domain }).map(|report| report.output)
}

pub fn legacy_core_overvolt_ranges(
    target: &GpuTarget<'_>,
) -> Result<Vec<(PState, MicrovoltsDelta, MicrovoltsDelta, MicrovoltsDelta)>, Error> {
    run(target, QueryLegacyCoreOvervoltRanges).map(|report| report.output)
}

pub fn legacy_p0_core_max_voltage_delta(
    target: &GpuTarget<'_>,
) -> Result<Option<MicrovoltsDelta>, Error> {
    run(target, QueryLegacyP0CoreMaxVoltageDelta).map(|report| report.output)
}

pub fn set_nvapi_pstate_clock_offsets<I>(target: &GpuTarget<'_>, offsets: I) -> Result<(), Error>
where
    I: IntoIterator<Item = (PState, ClockDomain, KilohertzDelta)>,
{
    target
        .nvapi()?
        .inner()
        .set_pstates(offsets)
        .map_err(Error::from)
}

pub fn set_nvapi_cooler_settings<I>(target: &GpuTarget<'_>, settings: I) -> Result<(), Error>
where
    I: IntoIterator<Item = (::nvapi::hi::FanCoolerId, ::nvapi::hi::CoolerSettings)>,
{
    target
        .nvapi()?
        .set_cooler_levels(settings)
        .map_err(Error::from)
}

pub fn sync_memory_pstate_as_p0(target: &GpuTarget<'_>) -> Result<(), Error> {
    let info = run(target, QueryGpuInfo)?.output;
    let gpu_type = fetch_gpu_type(&info).unwrap_or(super::gpu_type::GpuType::Unknown);
    let memory_points =
        query_domain_vf_points_indexed(target, ClockDomain::Memory, gpu_type.is_legacy_vfp())?;

    if memory_points.len() < 2 {
        return Err(Error::Custom(
            "memory VFP table has fewer than two points; cannot sync second stage to P0".into(),
        ));
    }

    let (p0_index, p0_point) = memory_points
        .last()
        .cloned()
        .ok_or_else(|| Error::Custom("memory VFP table is empty".into()))?;
    let (sync_index, sync_point) = memory_points[memory_points.len() - 2].clone();

    let new_delta =
        sync_point.delta.0 as i64 + (p0_point.frequency.0 as i64 - sync_point.frequency.0 as i64);
    let new_delta = i32::try_from(new_delta).map_err(|_| {
        Error::Custom(format!(
            "derived memory delta {} is out of i32 range for VFP point {}",
            new_delta, sync_index
        ))
    })?;

    set_nvapi_domain_vfp_deltas(
        target,
        ClockDomain::Memory,
        &[(sync_index, KilohertzDelta(new_delta))],
    )?;

    println!(
        "Synced memory VFP point {} to P0 point {}: current={} kHz, old_delta={} kHz, target={} kHz, new_delta={} kHz",
        sync_index,
        p0_index,
        sync_point.frequency.0,
        sync_point.delta.0,
        p0_point.frequency.0,
        new_delta
    );

    Ok(())
}

pub fn set_nvapi_legacy_clocks(
    target: &GpuTarget<'_>,
    core_mhz: u32,
    memory_mhz: u32,
) -> Result<(), Error> {
    run(
        target,
        SetLegacyClocks {
            core_mhz,
            memory_mhz,
        },
    )
    .map(|report| report.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violation_report_uses_first_nonzero_reference_time() {
        let report = violation_status_report(vec![
            (
                "Pwr",
                low_nvml::ViolationStatus {
                    violation_time_ns: 0,
                    reference_time_us: 0,
                },
            ),
            (
                "Thrm",
                low_nvml::ViolationStatus {
                    violation_time_ns: 42,
                    reference_time_us: 1_234_567,
                },
            ),
        ])
        .expect("a later successful policy should produce a report");

        assert_eq!(report.reference_time_us, 1_234_567);
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[1].violation_time_ns, 42);
    }

    #[test]
    fn violation_report_is_none_when_all_policies_are_unavailable() {
        let report = violation_status_report(vec![
            (
                "Pwr",
                low_nvml::ViolationStatus {
                    violation_time_ns: 0,
                    reference_time_us: 0,
                },
            ),
            (
                "Thrm",
                low_nvml::ViolationStatus {
                    violation_time_ns: 0,
                    reference_time_us: 0,
                },
            ),
        ]);

        assert!(report.is_none());
    }
}
