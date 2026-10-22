//! Capture: build a [`ProfileDoc`] (or [`SettingsBackup`]) from already-read
//! hardware state. Pure functions over snapshots — no hardware calls here, so
//! callers (CLI, pynvoc, tests) control *how* the snapshots are obtained and
//! captures are unit-testable without a GPU.

use super::schema::{
    BaselineBlock, ClkDomainOffsetRecord, DomainKind, FanSettings, GpuIdentity, PowerSettings,
    PowerWattsBounds, PrivateVfPointOffsetRecord, ProfileDoc, ProfileDomains, SettingsBackup,
    TopRelsRatioRecord, VoltRailLimit, VoltageSettings, empty_gpu_settings,
};
use super::schema::{FanCurveReadoutRecord, ValidationRecord};
use ::nvapi::hi::{CoolerSettings, FanCoolerId, GpuSettings, SensorThrottle};
use std::collections::BTreeMap;

/// Out-of-band reads that do not live in [`GpuSettings`], gathered by the
/// caller (NVML UUID / driver version / power bounds / VoltRails plane,
/// target PCI address).
#[derive(Debug, Clone, Default)]
pub struct CaptureContext {
    pub identity: GpuIdentity,
    pub power: Option<PowerWattsBounds>,
    /// Fan curves read via `GetFanCurves` (used only for [`SettingsBackup`]).
    pub fan_curves: Vec<FanCurveReadoutRecord>,
    /// VoltRails control-plane snapshot (all present rails; empty when the
    /// driver exposes no plane — the degradation is visible, never silent).
    pub volt_rails: Vec<VoltRailLimit>,
    /// TopRels GPC:XBAR ratio records (live control value per populated
    /// relation; empty when the plane is absent — 50-series only).
    pub toprels_ratios: Vec<TopRelsRatioRecord>,
    /// Private ClkDomains control records (all entries, full 8-slot
    /// arrays; empty when the family is absent).
    pub clk_domain_records: Vec<ClkDomainOffsetRecord>,
    /// Non-zero applied per-point frequency offsets on the private V/F
    /// table (mode-0 form; empty when the family is absent).
    pub private_vf_offsets: Vec<PrivateVfPointOffsetRecord>,
}

/// Capture the requested domains out of a `GpuSettings` snapshot plus the
/// out-of-band [`CaptureContext`] planes. Domains the sources cannot express
/// (the driver reported nothing for that plane) are captured as *absent*,
/// never as empty values — an empty capture would wipe the plane on apply.
pub fn capture_domains(
    settings: &GpuSettings,
    context: &CaptureContext,
    domains: &[DomainKind],
) -> ProfileDomains {
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
            DomainKind::VoltRails => {
                if !context.volt_rails.is_empty() {
                    out.volt_rails = Some(context.volt_rails.clone());
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
            DomainKind::TopRelsRatios => {
                if !context.toprels_ratios.is_empty() {
                    out.toprels_ratios = Some(context.toprels_ratios.clone());
                }
            }
            DomainKind::ClkDomainOffsets => {
                if !context.clk_domain_records.is_empty() {
                    out.clk_domain_offsets = Some(context.clk_domain_records.clone());
                }
            }
            DomainKind::PrivateVfOffsets => {
                if !context.private_vf_offsets.is_empty() {
                    out.private_vf_offsets = Some(context.private_vf_offsets.clone());
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
    backup.volt_rails = context.volt_rails.clone();
    backup.toprels_ratios = context.toprels_ratios.clone();
    backup.clk_domain_records = context.clk_domain_records.clone();
    backup.private_vf_offsets = context.private_vf_offsets.clone();
    backup.power_watts = context.power.clone();
    backup
}

/// Attach user-recorded validation evidence to a profile.
pub fn with_validation(mut doc: ProfileDoc, record: ValidationRecord) -> ProfileDoc {
    doc.validation = Some(record);
    doc
}

/// Read the VoltRails control plane as profile records (all present rails).
/// Empty when the driver exposes no plane — the degradation is visible in
/// the caller's context, never an error.
pub(crate) fn read_volt_rail_limits(target: &crate::target::GpuTarget<'_>) -> Vec<VoltRailLimit> {
    crate::operation::run(target, crate::operation::QueryNvapiVoltRails)
        .ok()
        .and_then(|report| report.output)
        .map(|rails| volt_rail_limits(&rails))
        .unwrap_or_default()
}

/// Convert a VoltRails snapshot into profile records. The full 6-dword
/// control payload (melonVolt slots) is carried verbatim and applied on
/// restore via the slot write path.
/// Read the TopRels relation table with live control ratios as profile
/// records. Empty when the plane is absent (non-50-series: the control GET
/// answers -103/-1) — visible degradation, never an error.
/// Read the private ClkDomains control records (all entries, full
/// 8-slot arrays). Empty when the family is absent — visible
/// degradation, never an error.
pub(crate) fn read_clk_domain_records(
    target: &crate::target::GpuTarget<'_>,
) -> Vec<ClkDomainOffsetRecord> {
    crate::operation::run(target, crate::operation::QueryNvapiClkDomains)
        .ok()
        .and_then(|report| report.output)
        .map(|control| {
            control
                .entries
                .iter()
                .map(|entry| ClkDomainOffsetRecord {
                    bit: entry.bit,
                    values: entry.values_kHz,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read the non-zero applied per-point frequency offsets from the
/// private V/F table (mode-0 form: current minus default, in kHz).
/// Empty when the family is absent.
pub(crate) fn read_private_vf_offsets(
    target: &crate::target::GpuTarget<'_>,
) -> Vec<PrivateVfPointOffsetRecord> {
    crate::operation::run(
        target,
        crate::operation::QueryNvapiClkVfPoints { include_raw: false },
    )
    .ok()
    .and_then(|report| report.output)
    .map(|points| {
        points
            .points
            .iter()
            .filter_map(|point| {
                let offset_khz =
                    ((point.freq_current_mhz as i64 - point.freq_default_mhz as i64) * 1000) as i32;
                (offset_khz != 0).then_some(PrivateVfPointOffsetRecord {
                    bank: point.bank,
                    index: point.index,
                    offset_khz,
                })
            })
            .collect()
    })
    .unwrap_or_default()
}
pub(crate) fn read_toprels_ratios(
    target: &crate::target::GpuTarget<'_>,
) -> Vec<TopRelsRatioRecord> {
    crate::operation::run(target, crate::operation::QueryTopRelsRelations)
        .ok()
        .map(|report| {
            report
                .output
                .iter()
                .filter_map(|(index, src, dst, _dir, _payload, live)| {
                    let ratio_raw = (*live).filter(|raw| *raw > 0)?;
                    Some(TopRelsRatioRecord {
                        index: *index,
                        src: *src,
                        dst: *dst,
                        ratio_raw,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn volt_rail_limits(rails: &::nvapi::VoltRails) -> Vec<VoltRailLimit> {
    rails
        .control
        .iter()
        .map(|entry| VoltRailLimit {
            rail_bit: entry.rail_bit,
            offset_uv: entry.values[0],
            expected_type: Some(entry.entry_type),
            slots: Some(entry.values),
        })
        .collect()
}

/// Best-effort out-of-band reads for a target: power bounds via NVML, fan
/// curves and the VoltRails plane via NVAPI. Absent backends simply leave
/// the fields empty — the degradation is visible in the context, never an
/// error.
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
    let volt_rails = read_volt_rail_limits(target);
    let toprels_ratios = read_toprels_ratios(target);
    let clk_domain_records = read_clk_domain_records(target);
    let private_vf_offsets = read_private_vf_offsets(target);
    CaptureContext {
        identity,
        power,
        fan_curves,
        volt_rails,
        toprels_ratios,
        clk_domain_records,
        private_vf_offsets,
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
        let domains = capture_domains(
            &settings,
            &CaptureContext::default(),
            &[DomainKind::Voltage],
        );
        assert!(domains.voltage.is_some());
        assert!(domains.clock_offsets.is_none());
        assert!(domains.public_vftable.is_none());
    }

    #[test]
    fn capture_never_emits_empty_domains() {
        let settings = empty_gpu_settings();
        let domains = capture_domains(
            &settings,
            &CaptureContext::default(),
            &[
                DomainKind::ClockOffsets,
                DomainKind::VoltRails,
                DomainKind::Power,
                DomainKind::Thermal,
            ],
        );
        assert!(domains.clock_offsets.is_none());
        assert!(domains.volt_rails.is_none());
        assert!(domains.power.is_none());
        assert!(domains.thermal.is_none());
        // nothing present at all
        assert!(domains.present().is_empty());
    }

    #[test]
    fn volt_rails_captured_from_context() {
        let settings = empty_gpu_settings();
        let mut context = CaptureContext::default();
        context.volt_rails.push(VoltRailLimit {
            rail_bit: 1,
            offset_uv: 15000,
            expected_type: Some(3),
            slots: None,
        });
        let domains = capture_domains(&settings, &context, &[DomainKind::VoltRails]);
        let rails = domains.volt_rails.unwrap();
        assert_eq!(rails.len(), 1);
        assert_eq!(rails[0].offset_uv, 15000);
        assert_eq!(rails[0].expected_type, Some(3));
    }

    #[test]
    fn volt_rail_capture_carries_full_slot_payload() {
        // 5090-MSVDD-shaped control entry: slot 0 = µV offset, 2 = VRM max
        // wall, 3 = VMIN min-hold, 1/4/5 firmware-opaque.
        let values = [15000i32, 7, -25000, -30000, 0, 12345];
        let rails = ::nvapi::VoltRails {
            rail_mask: 0x2,
            control: vec![::nvapi::VoltRailEntry {
                rail_bit: 1,
                entry_type: 3,
                values,
                extra_values: None,
            }],
            status: Vec::new(),
            rail_descriptors: Vec::new(),
        };
        let limits = volt_rail_limits(&rails);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].offset_uv, 15000);
        assert_eq!(limits[0].expected_type, Some(3));
        assert_eq!(limits[0].slots, Some(values));
    }

    #[test]
    fn clk_domain_and_private_vf_domains_roundtrip_through_capture() {
        let settings = empty_gpu_settings();
        let mut context = CaptureContext::default();
        context.clk_domain_records.push(ClkDomainOffsetRecord {
            bit: 1,
            values: [12000, 0, 0, 0, 0, 0, 0, 0],
        });
        context.private_vf_offsets.push(PrivateVfPointOffsetRecord {
            bank: 0,
            index: 5,
            offset_khz: -15000,
        });
        let domains = capture_domains(
            &settings,
            &context,
            &[DomainKind::ClkDomainOffsets, DomainKind::PrivateVfOffsets],
        );
        assert_eq!(domains.clk_domain_offsets.as_ref().unwrap()[0].bit, 1);
        assert_eq!(
            domains.private_vf_offsets.as_ref().unwrap()[0].offset_khz,
            -15000
        );
        // both domains ride along by default
        assert!(DomainKind::DEFAULT_CAPTURE.contains(&DomainKind::ClkDomainOffsets));
        assert!(DomainKind::DEFAULT_CAPTURE.contains(&DomainKind::PrivateVfOffsets));
        assert_eq!(
            DomainKind::parse("clk_domain_offsets"),
            Some(DomainKind::ClkDomainOffsets)
        );
        assert_eq!(
            DomainKind::parse("private_vf_offsets"),
            Some(DomainKind::PrivateVfOffsets)
        );
    }
    #[test]
    fn vfp_locks_in_default_capture() {
        // The get-lock-status table (PerfClientLimits GET) rides along by
        // default: capture is a read, and apply replays the exact table.
        assert!(DomainKind::DEFAULT_CAPTURE.contains(&DomainKind::VfpLocks));
    }

    #[test]
    fn toprels_ratios_domain_roundtrip_and_default() {
        assert!(DomainKind::DEFAULT_CAPTURE.contains(&DomainKind::TopRelsRatios));
        assert_eq!(
            DomainKind::parse("toprels_ratios"),
            Some(DomainKind::TopRelsRatios)
        );
        let settings = empty_gpu_settings();
        let mut context = CaptureContext::default();
        context.toprels_ratios.push(TopRelsRatioRecord {
            index: 0,
            src: 0,
            dst: 1,
            ratio_raw: 0xE660,
        });
        let domains = capture_domains(&settings, &context, &[DomainKind::TopRelsRatios]);
        let records = domains.toprels_ratios.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].ratio_raw, 0xE660);
        // absent plane -> absent domain (apply must not touch it)
        let empty = capture_domains(
            &settings,
            &CaptureContext::default(),
            &[DomainKind::TopRelsRatios],
        );
        assert!(empty.present().is_empty());
    }

    #[test]
    fn locked_clocks_never_captured_from_settings() {
        let settings = empty_gpu_settings();
        let domains = capture_domains(
            &settings,
            &CaptureContext::default(),
            &[DomainKind::LockedClocks],
        );
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
        context.volt_rails.push(VoltRailLimit {
            rail_bit: 1,
            offset_uv: 0,
            expected_type: Some(3),
            slots: None,
        });
        let backup = capture_backup(&context, &empty_gpu_settings(), "t".to_owned());
        assert!(backup.validate().is_ok());
        let json = serde_json::to_string(&backup).unwrap();
        let back: SettingsBackup = serde_json::from_str(&json).unwrap();
        assert_eq!(back, backup);
        assert_eq!(back.volt_rails.len(), 1);
    }
}
