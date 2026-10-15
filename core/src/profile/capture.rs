//! Capture: build a [`ProfileDoc`] (or [`SettingsBackup`]) from already-read
//! hardware state. Pure functions over snapshots — no hardware calls here, so
//! callers (CLI, pynvoc, tests) control *how* the snapshots are obtained and
//! captures are unit-testable without a GPU.

use super::schema::{
    BaselineBlock, DomainKind, FanSettings, GpuIdentity, PowerSettings, PowerWattsBounds,
    ProfileDoc, ProfileDomains, SettingsBackup, VoltageSettings, empty_gpu_settings,
};
use super::schema::{FanCurveReadoutRecord, ValidationRecord};
use ::nvapi::hi::{CoolerSettings, FanCoolerId, GpuSettings, SensorThrottle};
use std::collections::BTreeMap;

/// Out-of-band reads that do not live in [`GpuSettings`], gathered by the
/// caller (NVML UUID / driver version / power bounds, target PCI address).
#[derive(Debug, Clone, Default)]
pub struct CaptureContext {
    pub identity: GpuIdentity,
    pub power: Option<PowerWattsBounds>,
    /// Fan curves read via `GetFanCurves` (used only for [`SettingsBackup`]).
    pub fan_curves: Vec<FanCurveReadoutRecord>,
}

/// Capture the requested domains out of a `GpuSettings` snapshot. Domains the
/// snapshot cannot express (the driver reported nothing for that plane) are
/// captured as *absent*, never as empty values — an empty capture would wipe
/// the plane on apply.
pub fn capture_domains(settings: &GpuSettings, domains: &[DomainKind]) -> ProfileDomains {
    let mut out = ProfileDomains::default();
    for domain in domains {
        match domain {
            DomainKind::ClockOffsets => {
                if !settings.pstate_deltas.is_empty() {
                    out.clock_offsets = Some(settings.pstate_deltas.clone());
                }
            }
            DomainKind::PublicVftable => {
                if let Some(vfp) = &settings.vfp {
                    out.public_vftable = Some(vfp.clone());
                }
            }
            DomainKind::Voltage => {
                let overvolt = settings.overvolt.clone();
                let boost = settings.voltage_boost;
                if boost.is_some() || !overvolt.is_empty() {
                    out.voltage = Some(VoltageSettings {
                        boost_percent: boost,
                        overvolt_uv: overvolt,
                    });
                }
            }
            DomainKind::Power => {
                if let Some(percent) = settings.power_limits.first() {
                    out.power = Some(PowerSettings {
                        watts: None,
                        percent: Some(*percent),
                    });
                }
            }
            DomainKind::Thermal => {
                if !settings.sensor_limits.is_empty() {
                    out.thermal = Some(settings.sensor_limits.clone());
                }
            }
            DomainKind::Fan => {
                let coolers: BTreeMap<FanCoolerId, CoolerSettings> = settings.coolers.clone();
                if !coolers.is_empty() {
                    out.fan = Some(FanSettings {
                        curves: BTreeMap::new(),
                        cooler_settings: coolers,
                    });
                }
            }
            DomainKind::LockedClocks => {
                // `GpuSettings` carries no locked-clock state; locks are
                // opt-in, authored explicitly (or via a future capture op).
            }
            DomainKind::VfpLocks => {
                if !settings.vfp_locks.is_empty() {
                    out.vfp_locks = Some(settings.vfp_locks.clone());
                }
            }
        }
    }
    out
}

/// Baseline block: the *stock* reference values at capture time. Callers
/// should run this against a zeroed state (`capture-stock` asks the user to
/// be at stock); whatever is passed in is stored losslessly.
pub fn capture_baseline(settings: &GpuSettings, power: Option<&PowerWattsBounds>) -> BaselineBlock {
    BaselineBlock {
        stock_vfp: settings.vfp.clone(),
        stock_pstate_deltas: settings.pstate_deltas.clone(),
        power_min_watts: power.and_then(|p| p.min),
        power_max_watts: power.and_then(|p| p.max),
    }
}

/// Assemble the final profile document.
pub fn new_profile_doc(
    name: impl Into<String>,
    note: Option<String>,
    context: &CaptureContext,
    domains: ProfileDomains,
    baseline: Option<BaselineBlock>,
    now_utc: String,
) -> ProfileDoc {
    let mut doc = ProfileDoc::new(name, context.identity.clone(), now_utc);
    doc.note = note;
    doc.domains = domains;
    doc.baseline = baseline;
    doc
}

/// Snapshot the live settings (`stock.json` / `backup-last.json` payload).
pub fn capture_backup(
    context: &CaptureContext,
    settings: &GpuSettings,
    now_utc: String,
) -> SettingsBackup {
    let mut backup = SettingsBackup::new(context.identity.clone(), now_utc);
    backup.settings = settings.clone();
    backup.fan_curves = context.fan_curves.clone();
    backup.power_watts = context.power.clone();
    backup
}

/// Attach user-recorded validation evidence to a profile.
pub fn with_validation(mut doc: ProfileDoc, record: ValidationRecord) -> ProfileDoc {
    doc.validation = Some(record);
    doc
}

/// Best-effort out-of-band reads for a target: power bounds via NVML and
/// fan curves via NVAPI. Absent backends simply leave the fields empty —
/// the degradation is visible in the context, never an error.
pub fn capture_context(
    target: &crate::target::GpuTarget<'_>,
    identity: GpuIdentity,
) -> CaptureContext {
    let power = target
        .nvml()
        .ok()
        .and_then(|nvml| crate::nvml::query_nvml_power_watts(nvml, target.id.0))
        .map(|(min, current, max)| PowerWattsBounds {
            min: Some(min as u32),
            current: current as u32,
            max: Some(max as u32),
        });
    let fan_curves = crate::operation::run(target, crate::operation::GetFanCurves)
        .map(|report| {
            report
                .output
                .iter()
                .map(|curve| FanCurveReadoutRecord {
                    index: curve.index,
                    points: curve.points.iter().map(|p| (*p).into()).collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    CaptureContext {
        identity,
        power,
        fan_curves,
    }
}

/// Convenience for tests and callers that start from "all empty".
pub fn empty_settings() -> GpuSettings {
    empty_gpu_settings()
}

/// Re-exported so callers do not need the nvapi path for the thermal alias.
pub type ThermalLimits = Vec<SensorThrottle>;

#[cfg(test)]
mod tests {
    use super::super::schema::empty_gpu_settings;
    use super::*;

    fn settings_with_all_planes() -> GpuSettings {
        let mut settings = empty_gpu_settings();
        settings
            .pstate_deltas
            .insert(::nvapi::hi::PState::P0, BTreeMap::new());
        settings.overvolt.push(::nvapi::hi::MicrovoltsDelta(10000));
        settings.voltage_boost = Some(::nvapi::hi::Percentage(10));
        settings
    }

    #[test]
    fn capture_only_requested_domains() {
        let settings = settings_with_all_planes();
        let domains = capture_domains(&settings, &[DomainKind::Voltage]);
        assert!(domains.voltage.is_some());
        assert!(domains.clock_offsets.is_none());
        assert!(domains.public_vftable.is_none());
    }

    #[test]
    fn capture_never_emits_empty_domains() {
        let settings = empty_gpu_settings();
        let domains = capture_domains(
            &settings,
            &[
                DomainKind::ClockOffsets,
                DomainKind::Power,
                DomainKind::Thermal,
            ],
        );
        assert!(domains.clock_offsets.is_none());
        assert!(domains.power.is_none());
        assert!(domains.thermal.is_none());
        // nothing present at all
        assert!(domains.present().is_empty());
    }

    #[test]
    fn locked_clocks_never_captured_from_settings() {
        let settings = empty_gpu_settings();
        let domains = capture_domains(&settings, &[DomainKind::LockedClocks]);
        assert!(domains.locked_clocks.is_none());
    }

    #[test]
    fn baseline_carries_power_bounds() {
        let power = PowerWattsBounds {
            min: Some(100),
            current: 170,
            max: Some(220),
        };
        let baseline = capture_baseline(&empty_gpu_settings(), Some(&power));
        assert_eq!(baseline.power_min_watts, Some(100));
        assert_eq!(baseline.power_max_watts, Some(220));
    }

    #[test]
    fn capture_backup_roundtrips() {
        let mut context = CaptureContext::default();
        context.identity.uuid = Some("GPU-x".to_owned());
        context.power = Some(PowerWattsBounds {
            min: None,
            current: 170,
            max: None,
        });
        let backup = capture_backup(&context, &empty_gpu_settings(), "t".to_owned());
        assert!(backup.validate().is_ok());
        let json = serde_json::to_string(&backup).unwrap();
        let back: SettingsBackup = serde_json::from_str(&json).unwrap();
        assert_eq!(back, backup);
    }
}
