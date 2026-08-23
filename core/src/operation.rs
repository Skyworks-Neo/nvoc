use super::error::Error;
use super::nvapi as low_nvapi;
use super::nvml as low_nvml;
use super::result::{
    ApiRestrictionState, AppliedValue, AutoBoostState, BatchReport, ClockOffset, DNotifierInfo,
    DNotifierLevel, DisplayInfo, EdidData, FanInfo, NvapiPStateNativeLock, OperationKind,
    OperationReport, OvervoltApplied, PStateLevelEntry, PStateLevelsInfo, PstateBaseVoltage,
    PstateClockRange,
    SupportedApplicationClocks, TargetOutcome, TargetTempPolicy, TdpTempLimits,
    TemperatureThreshold, ThrottleReason, ViolationEntry, ViolationStatusReport, VoltageBoostState,
    VoltageFrequencyCheck,
};
use super::target::GpuTarget;
use super::types::{NvapiLockedVoltageTarget, VfpResetDomain};
use nvapi_hi::{
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
    if operation.is_nvapi_write() {
        if let Ok(gpu) = target.nvapi() {
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
    }
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
    type Output = nvapi_hi::GpuInfo;

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
    type Output = nvapi_hi::GpuSettings;

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
    type Output = nvapi_hi::GpuStatus;

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
pub struct ResetApplicationsClocks;

impl GpuOperation for ResetApplicationsClocks {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetApplicationsClocks
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
pub struct ResetLockedClocks {
    pub domain: ClockDomain,
}

impl GpuOperation for ResetLockedClocks {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetLockedClocks
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
        Ok(FanInfo {
            count,
            min_speed,
            max_speed,
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
pub struct ResetPstateBaseVoltages;

impl GpuOperation for ResetPstateBaseVoltages {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetPstateBaseVoltages
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
    type Output = nvapi_hi::Microvolts;

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
pub struct SetVfpVoltageLock {
    pub voltage_target: NvapiLockedVoltageTarget,
    pub feedback: bool,
}

impl GpuOperation for SetVfpVoltageLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetVfpVoltageLock
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
pub struct ResetVfpDeltas {
    pub domain: VfpResetDomain,
}

impl GpuOperation for ResetVfpDeltas {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetVfpDeltas
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::reset_vfp_deltas(target.nvapi()?, self.domain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResetVfpLock;

impl GpuOperation for ResetVfpLock {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetVfpLock
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        target.nvapi()?.reset_vfp_lock().map_err(Error::from)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetVfpPointDelta {
    pub point: usize,
    pub delta: KilohertzDelta,
}

impl GpuOperation for SetVfpPointDelta {
    type Output = AppliedValue<KilohertzDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetVfpPointDelta
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
pub struct SetVfpRangeDelta {
    pub start: usize,
    pub end: usize,
    pub delta: KilohertzDelta,
}

impl GpuOperation for SetVfpRangeDelta {
    type Output = AppliedValue<KilohertzDelta>;

    fn kind(&self) -> OperationKind {
        OperationKind::SetVfpRangeDelta
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::set_pointwise_vfp_delta(&[target.nvapi()?], self.start, self.end, self.delta.0)?;
        Ok(AppliedValue {
            requested: self.delta,
            applied: self.delta,
        })
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
    type Output = Option<nvapi_hi::nvapi::VoltRails>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiVoltRails
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target.nvapi()?.volt_rails().map_err(Error::from)?)
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
        // here means "no type-1 entry / not yet updated" — re-run
        // get-volt-rails to confirm.
        #[allow(non_snake_case)]
        let effective_wall_uV = gpu
            .volt_rails()
            .map_err(Error::from)?
            .and_then(|r| {
                r.status
                    .iter()
                    .find(|e| e.rail_bit == self.rail_bit && e.entry_type == 1)
                    // status payload index 4 = effective wall (clamped to
                    // min(target, vbios_wall, vrm_max_wall)); see
                    // nvapi-rs sys::gpu::power::private::status_values
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
    /// it may be below the requested offset's implied wall. 0 = no type-1
    /// status entry / driver hasn't refreshed yet (re-run get-volt-rails).
    pub effective_wall_uV: i32,
}

/// Set a volt-rail to an ABSOLUTE target voltage by deriving the required µV
/// offset from the live control/status snapshot. Convenience wrapper around
/// the melonVolt offset SET (see `reverse/melonvolt/ANALYSIS.md`) for
/// GUI/TUI sliders that think in absolute volts, not offsets.
///
/// Derivation (the offset is relative to the factory/default wall):
///   - `control` entry `.values[0]` = the offset currently applied (µV)
///   - `status` type-1 entry `.values[1]` = the target wall the driver holds
///     (µV) — the wall *including* the current offset, before the
///     VRM/vBIOS clamp
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
        // The target wall the driver currently holds (status type-1 entry,
        // payload index 1). This is the wall *including* the current offset,
        // before the VRM/vBIOS clamp — see sys status_values doc.
        let target_wall_uV = rails
            .status
            .iter()
            .find(|e| e.rail_bit == self.rail_bit && e.entry_type == 1)
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
        // here means "no type-1 entry / not yet updated" — re-run
        // get-volt-rails to confirm.
        #[allow(non_snake_case)]
        let effective_wall_uV = gpu
            .volt_rails()
            .map_err(Error::from)?
            .and_then(|r| {
                r.status
                    .iter()
                    .find(|e| e.rail_bit == self.rail_bit && e.entry_type == 1)
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
    /// it may be below the requested target. 0 = no type-1 status entry /
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
    type Output = Option<nvapi_hi::nvapi::ClockDomainControl>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomains
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target.nvapi()?.clk_domains_control().map_err(Error::from)?)
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
    type Output = Option<nvapi_hi::nvapi::ClockDomainFreqDetail>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreqDetail
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .clk_domain_freq_detail(self.domain_bit)
            .map_err(Error::from)?)
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
        Ok(target
            .nvapi()?
            .set_vfp_point_private(self.bank, self.idx, self.freq_mode, self.value)
            .map_err(Error::from)?)
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
        Ok(target
            .nvapi()?
            .set_vfp_range_private(self.bank, self.start, self.end, self.delta_mhz)
            .map_err(Error::from)?)
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
        Ok(target
            .nvapi()?
            .set_vfp_range_per_point_private(self.bank, self.start, self.end, &self.deltas)
            .map_err(Error::from)?)
    }
}

/// Batch-measure physical clocks for a set of domains via the V3
/// MEASURE_FREQ (RM 0x20809006, magic 0x30038) — one RM round-trip per
/// sample for the whole set, with per-domain V1/V2 fallback.
#[derive(Clone, Debug)]
pub struct QueryNvapiClkDomainFreqsBatch {
    /// sequential domain indices (GPC=0, XBAR=1, SYS=2, MCLK=4, …)
    pub domains: Vec<u32>,
}

impl GpuOperation for QueryNvapiClkDomainFreqsBatch {
    type Output = Option<Vec<nvapi_hi::nvapi::ClockDomainFreq>>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreqsBatch
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .clk_domain_freqs_batch(&self.domains)
            .map_err(Error::from)?)
    }
}

/// Query the private ClockClient V/F-POINTS read path (GetInfo 0x8895B510 →
/// GetStatus 0x7FEE9032, RM 0x20809061/0x20809062) — the article's per-domain
/// V/F curve family. Returns `None` where the driver doesn't expose the
/// private interface. Units live-calibrated vs the public GPC VFP curve
/// (see `nvapi::ClkVfPointPrivate`).
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiClkVfPoints;

impl GpuOperation for QueryNvapiClkVfPoints {
    type Output = Option<nvapi_hi::nvapi::ClkVfPointsPrivate>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkVfPoints
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target.nvapi()?.clk_vf_points_private().map_err(Error::from)?)
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
    type Output = Option<nvapi_hi::nvapi::ClockDomainFreq>;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryNvapiClkDomainFreq
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        Ok(target
            .nvapi()?
            .clk_domain_freq(self.domain_bit)
            .map_err(Error::from)?)
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
    /// which of the record's 8 value dwords to write (0-7; slot 0 is the
    /// article's signed frequency offset, the rest are driver-opaque
    /// range/voltage terms — A/B with MEASURE_FREQ to identify)
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
#[derive(Clone, Copy, Debug)]
pub struct QueryNvapiPStateLevels {
    /// Clock-domain index (0=GPC/core default).
    pub domain: usize,
}

impl Default for QueryNvapiPStateLevels {
    fn default() -> Self {
        Self { domain: 0 }
    }
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
            NvapiPStateNativeLock::Reset => nvapi_hi::PStateNativeLock::Reset,
            NvapiPStateNativeLock::PstateOnly { pstate } => {
                nvapi_hi::PStateNativeLock::PstateOnly { pstate }
            }
            NvapiPStateNativeLock::PstateAndFreq { pstate, freq_khz } => {
                nvapi_hi::PStateNativeLock::PstateAndFreq { pstate, freq_khz }
            }
        };
        target.nvapi()?.set_pstate_native(lock).map_err(Error::from)
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
pub struct ResetPstateClockOffsets {
    pub offsets: Vec<(PState, ClockDomain)>,
}

impl GpuOperation for ResetPstateClockOffsets {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::ResetPstateClockOffsets
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
/// window, rejects windows that would overlap P-States outside the requested
/// range, then applies the window with NVAPI.
///
/// The output is `(range_label, min_lock_mhz, max_lock_mhz)`.
#[derive(Clone, Copy, Debug)]
pub struct SetNvapiPstateLock {
    pub first_pstate: PerformanceState,
    pub second_pstate: PerformanceState,
}

impl GpuOperation for SetNvapiPstateLock {
    type Output = (String, u32, u32);

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
/// window, rejects windows that would overlap P-States outside the requested
/// range, then applies the window with NVML memory locked clocks.
///
/// The output is `(range_label, min_lock_mhz, max_lock_mhz)`.
#[derive(Clone, Copy, Debug)]
pub struct SetNvmlPstateLock {
    pub first_pstate: PerformanceState,
    pub second_pstate: PerformanceState,
}

impl GpuOperation for SetNvmlPstateLock {
    type Output = (String, u32, u32);

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
pub struct SetAutoBoost {
    pub enabled: bool,
}

impl GpuOperation for SetAutoBoost {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetAutoBoost
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvml::set_nvml_auto_boost(target.nvml()?, target.id.0, self.enabled)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SetAutoBoostDefault {
    pub enabled: bool,
}

impl GpuOperation for SetAutoBoostDefault {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetAutoBoostDefault
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
pub struct SetApiRestriction {
    pub api_type: Api,
    pub restricted: bool,
}

impl GpuOperation for SetApiRestriction {
    type Output = ();

    fn kind(&self) -> OperationKind {
        OperationKind::SetApiRestriction
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

pub fn detect_gpu_type(gpu_name: &str) -> super::gpu_type::GpuType {
    super::gpu_type::detect_gpu_type(gpu_name)
}

pub fn fetch_gpu_type(info: &nvapi_hi::GpuInfo) -> Result<super::gpu_type::GpuType, Error> {
    super::gpu_type::fetch_gpu_type(info)
}

pub fn find_matching_vfp_point(
    vfp_table: &std::collections::BTreeMap<usize, nvapi_hi::VfpPoint>,
    sensor_v: nvapi_hi::Microvolts,
) -> Option<(&usize, &nvapi_hi::VfpPoint)> {
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
            SetVfpRangeDelta {
                start,
                end: point + vfp_set_range,
                delta: KilohertzDelta(main_delta),
            },
        )?;
    } else {
        run(
            target,
            SetVfpRangeDelta {
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
                SetVfpRangeDelta {
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
    I: IntoIterator<Item = (nvapi_hi::FanCoolerId, nvapi_hi::CoolerSettings)>,
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
