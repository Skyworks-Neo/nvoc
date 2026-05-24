use super::error::Error;
use super::nvapi as low_nvapi;
use super::nvml as low_nvml;
use super::result::{
    AppliedValue, BatchReport, ClockOffset, FanInfo, OperationKind, OperationReport,
    PstateClockRange, SupportedApplicationClocks, TargetOutcome, TemperatureThreshold,
    VoltageFrequencyCheck,
};
use super::target::GpuTarget;
use super::types::{NvapiLockedVoltageTarget, VfpResetDomain};
use nvapi_hi::{
    ClockDomain, CoolerPolicy, Kilohertz, KilohertzDelta, MicrovoltsDelta, PState, Percentage,
    SensorThrottle, VfPoint,
};
use nvml_wrapper::enum_wrappers::device::PerformanceState;
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
        target.nvapi()?.info().map_err(Error::from)
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
        let mhz = match self.domain {
            ClockDomain::Graphics => {
                low_nvml::get_nvml_core_clock_vf_offset(nvml, target.id.0, self.pstate)
            }
            ClockDomain::Memory => {
                low_nvml::get_nvml_mem_clock_vf_offset(nvml, target.id.0, self.pstate)
            }
            _ => {
                return Err(Error::from(
                    "NVML clock offset domain must be Graphics or Memory",
                ));
            }
        }
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
        match self.domain {
            ClockDomain::Graphics => {
                low_nvml::set_nvml_core_clock_vf_offset(nvml, target.id.0, self.mhz, self.pstate)?
            }
            ClockDomain::Memory => {
                low_nvml::set_nvml_mem_clock_vf_offset(nvml, target.id.0, self.mhz, self.pstate)?
            }
            _ => {
                return Err(Error::from(
                    "NVML clock offset domain must be Graphics or Memory",
                ));
            }
        }
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
                    .map(nvapi_hi::SensorThrottle::from_default),
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
    type Output = low_nvapi::GpuTdpTempLimits;

    fn kind(&self) -> OperationKind {
        OperationKind::QueryTdpTempLimits
    }

    fn run(&self, target: &GpuTarget<'_>) -> Result<Self::Output, Error> {
        low_nvapi::get_gpu_tdp_temp_limit(&[target.nvapi()?], || {})
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
        let precise = low_nvapi::voltage_frequency_check(&[target.nvapi()?], self.point, || {})?;
        Ok(VoltageFrequencyCheck {
            precise,
            matched_point: None,
        })
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
