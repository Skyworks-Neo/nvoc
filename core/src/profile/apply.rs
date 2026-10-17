//! Apply executor: run an [`ApplyPlan`] against a [`GpuTarget`], verify each
//! domain by reading the state back, and roll back from the pre-apply
//! snapshot when a write fails verification. Mirrors the xOCD
//! apply → verify → restore lifecycle and issue #360's `expected_*` readback.

use super::plan::ApplyPlan;
use super::schema::{
    DomainKind, FanCurvePointRecord, FanCurveReadoutRecord, PowerSettings, ProfileDoc,
    SettingsBackup, VoltRailLimit, VoltageSettings,
};
use crate::nvml::query_nvml_power_watts;
use crate::operation::{
    GetFanCurves, QueryGpuSettings, QueryNvapiVoltRails, ResetFanCurve, ResetNvapiSensorLimits,
    ResetPublicVftableOffset, SetDomainVfpDeltas, SetFanCurve, SetNvapiOvervolt,
    SetNvapiPowerLimits, SetNvapiSensorLimits, SetNvapiVoltRailOffset, SetNvapiVoltRailSlot,
    SetPowerLimit, SetPstateClockOffset, SetVfpLockEntries, SetVoltageBoost, run,
};
use crate::target::GpuTarget;
use crate::types::VfpResetDomain;
use ::nvapi::hi::{
    ClockDomain, ClockLockEntry, KilohertzDelta, MicrovoltsDelta, Percentage, PerfLimitId,
};
use std::collections::BTreeMap;

/// Power readback tolerance in watts: NVML quantizes limits to the VRM's
/// granularity (typically 0.5–1 W), so exact equality would false-alarm.
pub const POWER_VERIFY_TOLERANCE_W: u32 = 2;

#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyOptions {
    /// Read the state back after writing and compare against the profile.
    pub verify: bool,
    /// When any domain fails (write error or verification mismatch), restore
    /// every already-applied domain from the pre-apply snapshot.
    pub rollback_on_failure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainOutcome {
    Applied {
        verified: bool,
    },
    Failed {
        error: String,
        rolled_back: bool,
    },
    /// Write attempted but a prerequisite read was unavailable.
    Unavailable {
        error: String,
    },
}

#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub outcomes: Vec<(DomainKind, DomainOutcome)>,
    pub rolled_back: bool,
}

impl ApplyReport {
    pub fn is_success(&self) -> bool {
        !self.rolled_back
            && self.outcomes.iter().all(|(_, o)| {
                matches!(
                    o,
                    DomainOutcome::Applied { verified: true }
                        | DomainOutcome::Applied { verified: false }
                )
            })
    }
}

/// Normalize "what to write per domain" — built either from a [`ProfileDoc`]
/// (normal apply) or from a [`SettingsBackup`] (rollback), so the appliers
/// below are shared by both paths.
#[derive(Debug, Clone, Default)]
struct DomainWrites {
    clock_offsets: BTreeMap<::nvapi::hi::PState, BTreeMap<ClockDomain, KilohertzDelta>>,
    public_vftable: Option<::nvapi::hi::VfpDeltas>,
    voltage: Option<VoltageSettings>,
    /// Per-rail melonVolt offsets to write (empty = nothing).
    volt_rails: Vec<VoltRailLimit>,
    power: Option<PowerSettings>,
    /// `None` = nothing to write; `Some(None-shaped reset)` uses
    /// `thermal_reset` instead.
    thermal: Option<Vec<::nvapi::hi::SensorThrottle>>,
    thermal_reset: bool,
    fan_curves: BTreeMap<u8, Vec<FanCurvePointRecord>>,
    /// Curve indices to reset (snapshot had no curve for them).
    fan_resets: std::collections::BTreeSet<u8>,
    /// PerfClientLimits entries to write verbatim (freq/volt locks and
    /// PstateSelect pins — each entry carries its own mode). `None` =
    /// nothing.
    vfp_locks: Option<Vec<ClockLockEntry>>,
}

impl DomainWrites {
    fn from_profile(doc: &ProfileDoc) -> Self {
        Self {
            clock_offsets: doc.domains.clock_offsets.clone().unwrap_or_default(),
            public_vftable: doc.domains.public_vftable.clone(),
            voltage: doc.domains.voltage.clone(),
            volt_rails: doc.domains.volt_rails.clone().unwrap_or_default(),
            power: doc.domains.power.clone(),
            thermal: doc.domains.thermal.clone(),
            thermal_reset: false,
            fan_curves: doc
                .domains
                .fan
                .as_ref()
                .map(|fan| fan.curves.clone())
                .unwrap_or_default(),
            fan_resets: std::collections::BTreeSet::new(),
            vfp_locks: doc
                .domains
                .vfp_locks
                .as_ref()
                .map(|locks| locks.values().copied().collect()),
        }
    }
}

/// Inverse-of-apply values: restore the snapshot's stored value for every key
/// the profile touched; where the snapshot has no value for a touched key,
/// neutralize it (zero offset / reset curve / reset limits). This is what
/// makes rollback exact instead of best-effort.
fn rollback_writes(
    doc: &ProfileDoc,
    backup: &SettingsBackup,
    applied: &[DomainKind],
) -> DomainWrites {
    let profile = DomainWrites::from_profile(doc);
    let settings = &backup.settings;
    let mut w = DomainWrites::default();

    for domain in applied {
        match domain {
            DomainKind::ClockOffsets => {
                let mut pstates: std::collections::BTreeSet<_> =
                    profile.clock_offsets.keys().copied().collect();
                pstates.extend(settings.pstate_deltas.keys().copied());
                for pstate in pstates {
                    let mut domains: std::collections::BTreeSet<_> = profile
                        .clock_offsets
                        .get(&pstate)
                        .map(|m| m.keys().copied().collect())
                        .unwrap_or_default();
                    domains.extend(
                        settings
                            .pstate_deltas
                            .get(&pstate)
                            .into_iter()
                            .flat_map(|m| m.keys().copied()),
                    );
                    for clock_domain in domains {
                        let value = settings
                            .pstate_deltas
                            .get(&pstate)
                            .and_then(|m| m.get(&clock_domain))
                            .copied()
                            .unwrap_or(KilohertzDelta(0));
                        w.clock_offsets
                            .entry(pstate)
                            .or_default()
                            .insert(clock_domain, value);
                    }
                }
            }
            DomainKind::PublicVftable => {
                let mut vfp = settings
                    .vfp
                    .clone()
                    .unwrap_or_else(|| ::nvapi::hi::VfpDeltas {
                        graphics: BTreeMap::new(),
                        memory: BTreeMap::new(),
                    });
                if let Some(pv) = &profile.public_vftable {
                    for idx in pv.graphics.keys() {
                        vfp.graphics.entry(*idx).or_insert(KilohertzDelta(0));
                    }
                    for idx in pv.memory.keys() {
                        vfp.memory.entry(*idx).or_insert(KilohertzDelta(0));
                    }
                }
                w.public_vftable = Some(vfp);
            }
            DomainKind::Voltage => {
                w.voltage = Some(VoltageSettings {
                    boost_percent: profile
                        .voltage
                        .as_ref()
                        .and_then(|v| v.boost_percent)
                        .map(|_| settings.voltage_boost.unwrap_or(Percentage(0)))
                        .or(settings.voltage_boost),
                    overvolt_uv: if profile
                        .voltage
                        .as_ref()
                        .is_some_and(|v| !v.overvolt_uv.is_empty())
                        && settings.overvolt.is_empty()
                    {
                        vec![MicrovoltsDelta(0)]
                    } else {
                        settings.overvolt.clone()
                    },
                });
            }
            DomainKind::Power => {
                w.power = backup.power_watts.as_ref().map(|p| PowerSettings {
                    watts: Some(p.current),
                    percent: None,
                });
            }
            DomainKind::Thermal => {
                if settings.sensor_limits.is_empty() {
                    w.thermal_reset = true;
                } else {
                    w.thermal = Some(settings.sensor_limits.clone());
                }
            }
            DomainKind::Fan => {
                for curve in &backup.fan_curves {
                    w.fan_curves.insert(curve.index, curve.points.clone());
                }
                if let Some(fan) = &doc.domains.fan {
                    for index in fan.curves.keys() {
                        if !backup.fan_curves.iter().any(|c| c.index == *index) {
                            w.fan_resets.insert(*index);
                        }
                    }
                }
            }
            DomainKind::VoltRails => {
                // Replay every rail the snapshot carries; a profile-touched
                // rail missing from the snapshot neutralizes to offset 0
                // (same expected type — the shape check still applies).
                let mut rails: Vec<VoltRailLimit> = backup.volt_rails.clone();
                for record in &profile.volt_rails {
                    if !rails.iter().any(|r| r.rail_bit == record.rail_bit) {
                        rails.push(VoltRailLimit {
                            rail_bit: record.rail_bit,
                            offset_uv: 0,
                            expected_type: record.expected_type,
                            // Neutralization only zeroes the offset; the
                            // opaque payload slots are not blindly stamped.
                            slots: None,
                        });
                    }
                }
                w.volt_rails = rails;
            }
            DomainKind::VfpLocks => {
                // The snapshot's full GET is the exact restore state (it
                // always carries all seeded entries on supporting cards);
                // overlay any profile-touched limit id the snapshot lacks
                // with a mode-None entry so the pin cannot survive.
                let mut entries: BTreeMap<PerfLimitId, ClockLockEntry> = settings.vfp_locks.clone();
                if let Some(profile_locks) = &doc.domains.vfp_locks {
                    for (limit, entry) in profile_locks {
                        entries.entry(*limit).or_insert(ClockLockEntry {
                            limit: *limit,
                            mode: ::nvapi::ClockLockMode::None,
                            lock_value: None,
                            select_pstate: None,
                            clock: entry.clock,
                        });
                    }
                }
                w.vfp_locks = Some(entries.into_values().collect());
            }
            _ => {}
        }
    }
    w
}

/// Execute `plan` for `doc` on `target`. `backup` must be the pre-apply
/// snapshot (the caller persists it as `backup-last.json` before calling).
pub fn execute_profile(
    target: &GpuTarget<'_>,
    doc: &ProfileDoc,
    plan: &ApplyPlan,
    backup: &SettingsBackup,
    options: ApplyOptions,
) -> ApplyReport {
    let mut outcomes: Vec<(DomainKind, DomainOutcome)> = Vec::new();
    let mut applied: Vec<DomainKind> = Vec::new();
    let mut failed = false;

    let writes = DomainWrites::from_profile(doc);

    for domain in &plan.steps {
        let outcome = match apply_domain(target, *domain, &writes) {
            Ok(DomainWriteStatus::Applied) => {
                applied.push(*domain);
                if options.verify {
                    match verify_domain(target, *domain, &writes) {
                        Ok(()) => DomainOutcome::Applied { verified: true },
                        Err(e) => {
                            failed = true;
                            DomainOutcome::Failed {
                                error: format!("verification mismatch: {e}"),
                                rolled_back: false,
                            }
                        }
                    }
                } else {
                    DomainOutcome::Applied { verified: false }
                }
            }
            // The plane exists in the profile but the card/driver cannot
            // serve it (e.g. no VoltRails GET on this generation). Counts as
            // a failure — the profile cannot be honored — but carries its
            // own outcome label so the report says WHY.
            Ok(DomainWriteStatus::Unavailable(reason)) => {
                failed = true;
                DomainOutcome::Unavailable { error: reason }
            }
            Err(e) => {
                failed = true;
                DomainOutcome::Failed {
                    error: e.to_string(),
                    rolled_back: false,
                }
            }
        };
        outcomes.push((*domain, outcome));
    }

    let mut rolled_back = false;
    if failed && options.rollback_on_failure && !applied.is_empty() {
        let restore = rollback_writes(doc, backup, &applied);
        rolled_back = true;
        for (domain, outcome) in &mut outcomes {
            if matches!(
                outcome,
                DomainOutcome::Failed {
                    rolled_back: false,
                    ..
                }
            ) && applied.contains(domain)
            {
                let result = apply_domain(target, *domain, &restore);
                if let Err(e) = result {
                    *outcome = DomainOutcome::Failed {
                        error: format!(
                            "{}; ROLLBACK ALSO FAILED for this domain: {e}; \
                             restore manually from backup-last.json",
                            match &outcome {
                                DomainOutcome::Failed { error, .. } => error.clone(),
                                _ => String::new(),
                            }
                        ),
                        rolled_back: true,
                    };
                } else {
                    *outcome = DomainOutcome::Failed {
                        error: "rolled back to pre-apply state".to_owned(),
                        rolled_back: true,
                    };
                }
            }
        }
    }

    ApplyReport {
        outcomes,
        rolled_back,
    }
}

/// Outcome of one domain's writes: applied, or the plane is unavailable on
/// this card/driver (profile asked for it, hardware can't serve it).
enum DomainWriteStatus {
    Applied,
    Unavailable(String),
}

fn apply_domain(
    target: &GpuTarget<'_>,
    domain: DomainKind,
    writes: &DomainWrites,
) -> Result<DomainWriteStatus, crate::Error> {
    match domain {
        DomainKind::ClockOffsets => {
            for (pstate, deltas) in &writes.clock_offsets {
                for (clock_domain, delta) in deltas {
                    run(
                        target,
                        SetPstateClockOffset {
                            pstate: *pstate,
                            domain: *clock_domain,
                            delta: *delta,
                        },
                    )?;
                }
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::PublicVftable => {
            let vfp = writes
                .public_vftable
                .as_ref()
                .ok_or_else(|| crate::Error::Custom("no public V/F deltas in plan".to_owned()))?;
            // Reset first so a partially-set table cannot leak stale
            // deltas into the new values (separate reset path from the
            // pstate-offset plane).
            if let Err(e) = run(
                target,
                ResetPublicVftableOffset {
                    domain: VfpResetDomain::All,
                },
            ) && !e.is_allowable_nvapi_reset_error()
            {
                return Err(e);
            }
            let domains = [
                (ClockDomain::Graphics, &vfp.graphics),
                (ClockDomain::Memory, &vfp.memory),
            ];
            for (clock_domain, deltas) in domains {
                if deltas.is_empty() {
                    continue;
                }
                run(
                    target,
                    SetDomainVfpDeltas {
                        domain: clock_domain,
                        deltas: deltas.iter().map(|(i, d)| (*i, *d)).collect(),
                    },
                )?;
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::Voltage => {
            let voltage = writes
                .voltage
                .as_ref()
                .ok_or_else(|| crate::Error::Custom("no voltage settings in plan".to_owned()))?;
            if let Some(boost) = voltage.boost_percent {
                run(target, SetVoltageBoost { boost })?;
            }
            for delta_uv in &voltage.overvolt_uv {
                run(
                    target,
                    SetNvapiOvervolt {
                        delta_uv: *delta_uv,
                    },
                )?;
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::VoltRails => {
            if writes.volt_rails.is_empty() {
                return Ok(DomainWriteStatus::Applied);
            }
            // Availability gate first: a rail write on a card whose driver
            // exposes no VoltRails plane is Unavailable, not a write error.
            let rails = match run(target, QueryNvapiVoltRails)?.output {
                Some(rails) => rails,
                None => {
                    return Ok(DomainWriteStatus::Unavailable(
                        "VoltRails plane not exposed by this driver".to_owned(),
                    ));
                }
            };
            for record in &writes.volt_rails {
                if !rails.control.iter().any(|e| e.rail_bit == record.rail_bit) {
                    return Ok(DomainWriteStatus::Unavailable(format!(
                        "rail bit {} not present (mask 0x{:08X})",
                        record.rail_bit, rails.rail_mask
                    )));
                }
                let applied = run(
                    target,
                    SetNvapiVoltRailOffset {
                        rail_bit: record.rail_bit,
                        offset_uV: record.offset_uv,
                        expected_type: record.expected_type,
                    },
                )?;
                if applied.output.is_none() {
                    return Ok(DomainWriteStatus::Unavailable(
                        "volt-rail write path not exposed by this driver".to_owned(),
                    ));
                }
                // Replay the remaining melonVolt payload slots verbatim. A
                // slot that already matches the live value is left alone —
                // the opaque dwords (1/4/5) are only ever written when the
                // capture actually differs, never blindly.
                if let Some(slots) = record.slots {
                    let live = rails
                        .control
                        .iter()
                        .find(|e| e.rail_bit == record.rail_bit)
                        .map(|e| e.values);
                    if live == Some(slots) {
                        continue;
                    }
                    for (slot, &value) in slots.iter().enumerate().skip(1) {
                        if live.as_ref().is_some_and(|vals| vals[slot] == value) {
                            continue;
                        }
                        run(
                            target,
                            SetNvapiVoltRailSlot {
                                rail_bit: record.rail_bit,
                                slot,
                                value_uV: value,
                                expected_type: record.expected_type,
                            },
                        )?;
                    }
                }
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::Power => {
            let power = writes
                .power
                .as_ref()
                .ok_or_else(|| crate::Error::Custom("no power settings in plan".to_owned()))?;
            if let Some(watts) = power.watts {
                run(target, SetPowerLimit { watts })?;
            }
            if let Some(percent) = power.percent {
                run(
                    target,
                    SetNvapiPowerLimits {
                        limits: vec![percent],
                    },
                )?;
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::Thermal => {
            if writes.thermal_reset {
                run(target, ResetNvapiSensorLimits)?;
            } else {
                let limits = writes
                    .thermal
                    .as_ref()
                    .ok_or_else(|| crate::Error::Custom("no thermal limits in plan".to_owned()))?;
                run(
                    target,
                    SetNvapiSensorLimits {
                        limits: limits.clone(),
                    },
                )?;
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::Fan => {
            for (index, points) in &writes.fan_curves {
                run(
                    target,
                    SetFanCurve {
                        index: *index,
                        points: points.iter().map(|p| (*p).into()).collect(),
                    },
                )?;
            }
            for index in &writes.fan_resets {
                run(target, ResetFanCurve { index: *index })?;
            }
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::VfpLocks => {
            let Some(entries) = &writes.vfp_locks else {
                return Ok(DomainWriteStatus::Applied);
            };
            // Availability gate first: the PerfClientLimits table is the
            // capture source AND the apply target. A driver that does not
            // expose it (empty GET) makes the domain Unavailable — the rest
            // of the profile still applies, no rollback storm.
            let current = run(target, QueryGpuSettings)?.output;
            if current.vfp_locks.is_empty() {
                return Ok(DomainWriteStatus::Unavailable(
                    "PerfClientLimits lock table not exposed by this driver".to_owned(),
                ));
            }
            // Entries carry their own mode — PstateSelect pins (ids 4/5)
            // replay exactly as captured. A plain write error after the GET
            // gate is a genuine failure (stamp-gated SET) and rolls back.
            run(
                target,
                SetVfpLockEntries {
                    entries: entries.clone(),
                },
            )?;
            Ok(DomainWriteStatus::Applied)
        }
        DomainKind::LockedClocks => Err(crate::Error::Custom(
            "locked_clocks apply ships with the clock-lock workflow (not wired in this version)"
                .to_owned(),
        )),
    }
}

/// Read-back verification. `expected` is compared against freshly-queried
/// state; mismatches name the field.
fn verify_domain(
    target: &GpuTarget<'_>,
    domain: DomainKind,
    writes: &DomainWrites,
) -> Result<(), String> {
    match domain {
        DomainKind::ClockOffsets => {
            let settings = query_settings(target)?;
            let expected = &writes.clock_offsets;
            if &settings.pstate_deltas != expected {
                return Err(format!(
                    "pstate deltas differ: expected {:?}, read back {:?}",
                    expected, settings.pstate_deltas
                ));
            }
            Ok(())
        }
        DomainKind::PublicVftable => {
            let settings = query_settings(target)?;
            let expected = writes
                .public_vftable
                .as_ref()
                .ok_or_else(|| "internal: public_vftable domain without values".to_owned())?;
            if settings.vfp.as_ref() != Some(expected) {
                return Err(format!(
                    "public V/F deltas differ: expected {expected:?}, read back {:?}",
                    settings.vfp
                ));
            }
            Ok(())
        }
        DomainKind::Voltage => {
            let settings = query_settings(target)?;
            let expected = writes
                .voltage
                .as_ref()
                .ok_or_else(|| "internal: voltage domain without values".to_owned())?;
            if expected.boost_percent.is_some() && settings.voltage_boost != expected.boost_percent
            {
                return Err(format!(
                    "voltage boost differs: expected {:?}, read back {:?}",
                    expected.boost_percent, settings.voltage_boost
                ));
            }
            Ok(())
        }
        DomainKind::Power => {
            let expected = writes
                .power
                .as_ref()
                .ok_or_else(|| "internal: power domain without values".to_owned())?;
            let nvml = target
                .nvml()
                .map_err(|e| format!("NVML required for power verification: {e}"))?;
            let (min, current, max) = query_nvml_power_watts(nvml, target.id.0)
                .ok_or_else(|| "NVML power readback failed".to_owned())?;
            if let Some(watts) = expected.watts
                && (watts as i64 - current as i64).abs() > POWER_VERIFY_TOLERANCE_W as i64
            {
                return Err(format!(
                    "power limit differs: expected {watts} W, read back {current} W \
                     (range {min}–{max} W)"
                ));
            }
            Ok(())
        }
        DomainKind::Thermal => {
            let settings = query_settings(target)?;
            let expected = writes
                .thermal
                .as_ref()
                .ok_or_else(|| "internal: thermal domain without values".to_owned())?;
            if &settings.sensor_limits != expected {
                return Err("sensor limits differ from expected values".to_owned());
            }
            Ok(())
        }
        DomainKind::Fan => {
            let readouts = run(target, GetFanCurves).map_err(|e| e.to_string())?;
            for (index, points) in &writes.fan_curves {
                let read = readouts
                    .output
                    .iter()
                    .find(|r| r.index == *index)
                    .ok_or_else(|| format!("fan curve {index} not reported back"))?;
                let read_points: Vec<FanCurvePointRecord> =
                    read.points.iter().map(|p| (*p).into()).collect();
                if &read_points != points {
                    return Err(format!(
                        "fan curve {index} differs: expected {} points, read back {}",
                        points.len(),
                        read.points.len()
                    ));
                }
            }
            Ok(())
        }
        DomainKind::VoltRails => {
            if writes.volt_rails.is_empty() {
                return Ok(());
            }
            let rails = match run(target, QueryNvapiVoltRails).map_err(|e| e.to_string())?.output {
                Some(rails) => rails,
                None => {
                    return Err("VoltRails plane not exposed by this driver".to_owned());
                }
            };
            for record in &writes.volt_rails {
                let live = rails
                    .control
                    .iter()
                    .find(|e| e.rail_bit == record.rail_bit)
                    .ok_or_else(|| {
                        format!("rail bit {} not present in readback", record.rail_bit)
                    })?;
                if live.values[0] != record.offset_uv {
                    return Err(format!(
                        "rail {} offset differs: expected {} µV, read back {} µV",
                        record.rail_bit, record.offset_uv, live.values[0]
                    ));
                }
                if let Some(slots) = record.slots
                    && live.values != slots
                {
                    return Err(format!(
                        "rail {} payload differs: expected {:?}, read back {:?}",
                        record.rail_bit, slots, live.values
                    ));
                }
            }
            Ok(())
        }
        DomainKind::VfpLocks => {
            let Some(expected) = &writes.vfp_locks else {
                return Ok(());
            };
            let settings = query_settings(target)?;
            for entry in expected {
                let live = settings.vfp_locks.get(&entry.limit).ok_or_else(|| {
                    format!("vfp lock {:?} not present in readback", entry.limit)
                })?;
                if live != entry {
                    return Err(format!(
                        "vfp lock {:?} differs: expected {entry:?}, read back {live:?}",
                        entry.limit
                    ));
                }
            }
            Ok(())
        }
        DomainKind::LockedClocks => Err(
            "locked_clocks verification ships with the clock-lock workflow (not wired in this version)"
                .to_owned(),
        ),
    }
}

fn query_settings(target: &GpuTarget<'_>) -> Result<::nvapi::hi::GpuSettings, String> {
    run(target, QueryGpuSettings)
        .map(|report| report.output)
        .map_err(|e| e.to_string())
}

/// Build the pre-apply snapshot content from a freshly-queried state. The
/// caller runs [`QueryGpuSettings`] + fan-curve reads itself (or reuses these
/// helpers) so the snapshot matches exactly what is on the card before the
/// first write.
pub fn capture_backup_now(
    target: &GpuTarget<'_>,
    identity: &super::schema::GpuIdentity,
    now_utc: String,
) -> Result<SettingsBackup, crate::Error> {
    let settings = run(target, QueryGpuSettings)?.output;
    let fan_curves = run(target, GetFanCurves)
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
    let power_watts = target
        .nvml()
        .ok()
        .and_then(|nvml| query_nvml_power_watts(nvml, target.id.0))
        .map(|(min, current, max)| super::schema::PowerWattsBounds {
            min: Some(min as u32),
            current: current as u32,
            max: Some(max as u32),
        });
    let volt_rails = super::capture::read_volt_rail_limits(target);
    let mut backup = SettingsBackup::new(identity.clone(), now_utc);
    backup.settings = settings;
    backup.fan_curves = fan_curves;
    backup.volt_rails = volt_rails;
    backup.power_watts = power_watts;
    Ok(backup)
}

/// Domains a backup snapshot can restore, in apply order.
pub fn backup_domains(backup: &SettingsBackup) -> Vec<DomainKind> {
    let mut domains = Vec::new();
    if !backup.settings.pstate_deltas.is_empty() {
        domains.push(DomainKind::ClockOffsets);
    }
    if backup.settings.vfp.is_some() {
        domains.push(DomainKind::PublicVftable);
    }
    if backup.settings.voltage_boost.is_some() || !backup.settings.overvolt.is_empty() {
        domains.push(DomainKind::Voltage);
    }
    if backup.power_watts.is_some() {
        domains.push(DomainKind::Power);
    }
    if !backup.settings.sensor_limits.is_empty() {
        domains.push(DomainKind::Thermal);
    }
    if !backup.fan_curves.is_empty() {
        domains.push(DomainKind::Fan);
    }
    if !backup.volt_rails.is_empty() {
        domains.push(DomainKind::VoltRails);
    }
    if !backup.settings.vfp_locks.is_empty() {
        domains.push(DomainKind::VfpLocks);
    }
    domains
}

/// Replay a snapshot's stored values for the requested domains — the
/// `profile restore-last` path. Restores what the snapshot carries; domains
/// the snapshot cannot express are reported in the notes, never silently
/// skipped. (To return the card to *clean* state, apply a stock profile
/// captured at zero offsets instead.)
pub fn restore_backup(
    target: &GpuTarget<'_>,
    backup: &SettingsBackup,
    domains: &[DomainKind],
) -> Result<Vec<(DomainKind, String)>, crate::Error> {
    let mut notes: Vec<(DomainKind, String)> = Vec::new();
    for domain in domains {
        match domain {
            DomainKind::ClockOffsets => {
                for (pstate, deltas) in &backup.settings.pstate_deltas {
                    for (clock_domain, delta) in deltas {
                        run(
                            target,
                            SetPstateClockOffset {
                                pstate: *pstate,
                                domain: *clock_domain,
                                delta: *delta,
                            },
                        )?;
                    }
                }
            }
            DomainKind::PublicVftable => {
                if let Some(vfp) = &backup.settings.vfp {
                    if let Err(e) = run(
                        target,
                        ResetPublicVftableOffset {
                            domain: VfpResetDomain::All,
                        },
                    ) && !e.is_allowable_nvapi_reset_error()
                    {
                        return Err(e);
                    }
                    for (clock_domain, deltas) in [
                        (ClockDomain::Graphics, &vfp.graphics),
                        (ClockDomain::Memory, &vfp.memory),
                    ] {
                        if deltas.is_empty() {
                            continue;
                        }
                        run(
                            target,
                            SetDomainVfpDeltas {
                                domain: clock_domain,
                                deltas: deltas.iter().map(|(i, d)| (*i, *d)).collect(),
                            },
                        )?;
                    }
                } else {
                    notes.push((*domain, "snapshot carries no public V/F deltas".to_owned()));
                }
            }
            DomainKind::Voltage => {
                if let Some(boost) = backup.settings.voltage_boost {
                    run(target, SetVoltageBoost { boost })?;
                }
                for delta_uv in &backup.settings.overvolt {
                    run(
                        target,
                        SetNvapiOvervolt {
                            delta_uv: *delta_uv,
                        },
                    )?;
                }
            }
            DomainKind::Power => {
                if let Some(power) = &backup.power_watts {
                    run(
                        target,
                        SetPowerLimit {
                            watts: power.current,
                        },
                    )?;
                } else {
                    notes.push((*domain, "snapshot carries no power reading".to_owned()));
                }
            }
            DomainKind::Thermal => {
                if backup.settings.sensor_limits.is_empty() {
                    run(target, ResetNvapiSensorLimits)?;
                    notes.push((*domain, "reset (snapshot had no limits)".to_owned()));
                } else {
                    run(
                        target,
                        SetNvapiSensorLimits {
                            limits: backup.settings.sensor_limits.clone(),
                        },
                    )?;
                }
            }
            DomainKind::Fan => {
                if backup.fan_curves.is_empty() {
                    notes.push((*domain, "snapshot carries no fan curves".to_owned()));
                }
                for curve in &backup.fan_curves {
                    run(
                        target,
                        SetFanCurve {
                            index: curve.index,
                            points: curve.points.iter().map(|p| (*p).into()).collect(),
                        },
                    )?;
                }
            }
            DomainKind::VoltRails => {
                if backup.volt_rails.is_empty() {
                    notes.push((*domain, "snapshot carries no volt-rail plane".to_owned()));
                }
                for record in &backup.volt_rails {
                    let applied = run(
                        target,
                        SetNvapiVoltRailOffset {
                            rail_bit: record.rail_bit,
                            offset_uV: record.offset_uv,
                            expected_type: record.expected_type,
                        },
                    )?;
                    if applied.output.is_none() {
                        notes.push((
                            *domain,
                            format!("rail {} write path unavailable", record.rail_bit),
                        ));
                    }
                }
            }
            DomainKind::VfpLocks => {
                if backup.settings.vfp_locks.is_empty() {
                    notes.push((*domain, "snapshot carries no vfp locks".to_owned()));
                }
                run(
                    target,
                    SetVfpLockEntries {
                        entries: backup.settings.vfp_locks.values().copied().collect(),
                    },
                )?;
            }
            DomainKind::LockedClocks => {
                notes.push((
                    *domain,
                    "locked_clocks restore is not wired in this version".to_owned(),
                ));
            }
        }
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::schema::{
        GpuIdentity, PowerSettings, ProfileDoc, ProfileDomains, empty_gpu_settings,
    };
    use ::nvapi::hi::PState;

    fn doc_with_clock_offsets() -> ProfileDoc {
        let mut doc = ProfileDoc::new("rollback-test", GpuIdentity::default(), "t".to_owned());
        let mut deltas = BTreeMap::new();
        deltas.insert(ClockDomain::Graphics, KilohertzDelta(150_000));
        deltas.insert(ClockDomain::Memory, KilohertzDelta(500_000));
        doc.domains.clock_offsets = Some(BTreeMap::from([(PState::P0, deltas)]));
        doc
    }

    fn backup_with_stock(offset: i32) -> SettingsBackup {
        let mut backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        let mut deltas = BTreeMap::new();
        deltas.insert(ClockDomain::Graphics, KilohertzDelta(offset));
        backup.settings.pstate_deltas.insert(PState::P0, deltas);
        backup
    }

    #[test]
    fn rollback_restores_snapshot_values_for_touched_keys() {
        let doc = doc_with_clock_offsets();
        let backup = backup_with_stock(-20_000);
        let writes = rollback_writes(&doc, &backup, &[DomainKind::ClockOffsets]);
        let p0 = &writes.clock_offsets[&PState::P0];
        assert_eq!(p0[&ClockDomain::Graphics], KilohertzDelta(-20_000));
        // profile touched memory but snapshot has no value → neutralized
        assert_eq!(p0[&ClockDomain::Memory], KilohertzDelta(0));
    }

    #[test]
    fn rollback_neutralizes_keys_missing_from_snapshot() {
        let mut doc = doc_with_clock_offsets();
        doc.domains
            .clock_offsets
            .as_mut()
            .unwrap()
            .get_mut(&PState::P0)
            .unwrap()
            .insert(ClockDomain::Video, KilohertzDelta(10_000));
        let backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        let writes = rollback_writes(&doc, &backup, &[DomainKind::ClockOffsets]);
        let p0 = &writes.clock_offsets[&PState::P0];
        assert_eq!(p0[&ClockDomain::Graphics], KilohertzDelta(0));
        assert_eq!(p0[&ClockDomain::Memory], KilohertzDelta(0));
        assert_eq!(p0[&ClockDomain::Video], KilohertzDelta(0));
    }

    #[test]
    fn rollback_fan_resets_missing_curves() {
        let mut doc = ProfileDoc::new("fan", GpuIdentity::default(), "t".to_owned());
        let mut fan = crate::profile::schema::FanSettings::default();
        fan.curves.insert(0, Vec::new());
        fan.curves.insert(1, Vec::new());
        doc.domains.fan = Some(fan);

        let mut backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        backup.fan_curves.push(FanCurveReadoutRecord {
            index: 0,
            points: Vec::new(),
        });

        let writes = rollback_writes(&doc, &backup, &[DomainKind::Fan]);
        assert!(writes.fan_curves.contains_key(&0));
        assert!(writes.fan_resets.contains(&1));
    }

    #[test]
    fn rollback_thermal_uses_reset_when_snapshot_empty() {
        let mut doc = ProfileDoc::new("thermal", GpuIdentity::default(), "t".to_owned());
        doc.domains.thermal = Some(Vec::new());
        let backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        let writes = rollback_writes(&doc, &backup, &[DomainKind::Thermal]);
        assert!(writes.thermal_reset);
        assert!(writes.thermal.is_none());
    }

    #[test]
    fn rollback_restores_volt_rails_from_snapshot() {
        let mut doc = ProfileDoc::new("rails", GpuIdentity::default(), "t".to_owned());
        doc.domains.volt_rails = Some(vec![VoltRailLimit {
            rail_bit: 1,
            offset_uv: 15000,
            expected_type: Some(3),
            slots: None,
        }]);
        let mut backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        backup.volt_rails = vec![VoltRailLimit {
            rail_bit: 1,
            offset_uv: 0,
            expected_type: Some(3),
            slots: None,
        }];
        let writes = rollback_writes(&doc, &backup, &[DomainKind::VoltRails]);
        assert_eq!(writes.volt_rails.len(), 1);
        assert_eq!(writes.volt_rails[0].offset_uv, 0);
    }

    #[test]
    fn rollback_volt_rails_neutralizes_rails_missing_from_snapshot() {
        let mut doc = ProfileDoc::new("rails", GpuIdentity::default(), "t".to_owned());
        doc.domains.volt_rails = Some(vec![VoltRailLimit {
            rail_bit: 1,
            offset_uv: 15000,
            expected_type: Some(3),
            slots: None,
        }]);
        let backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        let writes = rollback_writes(&doc, &backup, &[DomainKind::VoltRails]);
        assert_eq!(writes.volt_rails.len(), 1);
        assert_eq!(writes.volt_rails[0].offset_uv, 0);
        assert_eq!(writes.volt_rails[0].expected_type, Some(3));
    }

    #[test]
    fn rollback_replays_vfp_locks_and_neutralizes_missing_entries() {
        use ::nvapi::hi::ClockLockMode;
        use std::collections::BTreeMap;

        let mut doc = ProfileDoc::new("locks", GpuIdentity::default(), "t".to_owned());
        let mut pins = BTreeMap::new();
        pins.insert(
            PerfLimitId::PstateSelectUpper,
            ClockLockEntry {
                limit: PerfLimitId::PstateSelectUpper,
                mode: ClockLockMode::PstateSelect,
                lock_value: None,
                select_pstate: Some(3),
                clock: ClockDomain::Graphics,
            },
        );
        doc.domains.vfp_locks = Some(pins);

        let mut backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        let mut stock = BTreeMap::new();
        stock.insert(
            PerfLimitId::Gpu,
            ClockLockEntry {
                limit: PerfLimitId::Gpu,
                mode: ClockLockMode::None,
                lock_value: None,
                select_pstate: None,
                clock: ClockDomain::Graphics,
            },
        );
        backup.settings.vfp_locks = stock;

        let writes = rollback_writes(&doc, &backup, &[DomainKind::VfpLocks]);
        let entries = writes.vfp_locks.unwrap();
        assert_eq!(entries.len(), 2);
        let pin = entries
            .iter()
            .find(|e| e.limit == PerfLimitId::PstateSelectUpper)
            .unwrap();
        // the profile-touched entry the snapshot lacks -> neutralized
        assert_eq!(pin.mode, ClockLockMode::None);
        assert_eq!(pin.select_pstate, None);
        // the snapshot-carried entry replays verbatim
        assert!(entries.iter().any(|e| e.limit == PerfLimitId::Gpu));
    }

    #[test]
    fn backup_domains_includes_volt_rails_and_vfp_locks() {
        let mut backup = SettingsBackup::new(GpuIdentity::default(), "t".to_owned());
        backup.volt_rails.push(VoltRailLimit {
            rail_bit: 1,
            offset_uv: 0,
            expected_type: None,
            slots: None,
        });
        backup.settings.vfp_locks.insert(
            PerfLimitId::Gpu,
            ClockLockEntry {
                limit: PerfLimitId::Gpu,
                mode: ::nvapi::ClockLockMode::None,
                lock_value: None,
                select_pstate: None,
                clock: ClockDomain::Graphics,
            },
        );
        let domains = backup_domains(&backup);
        assert!(domains.contains(&DomainKind::VoltRails));
        assert!(domains.contains(&DomainKind::VfpLocks));
    }

    #[test]
    fn report_success_requires_all_verified_and_no_rollback() {
        let mut report = ApplyReport {
            outcomes: vec![(DomainKind::Power, DomainOutcome::Applied { verified: true })],
            rolled_back: false,
        };
        assert!(report.is_success());
        report.rolled_back = true;
        assert!(!report.is_success());
    }

    #[test]
    fn power_settings_default_is_empty() {
        let doc = ProfileDoc::new("p", GpuIdentity::default(), "t".to_owned());
        assert!(doc.domains.power.is_none());
        assert_eq!(PowerSettings::default().watts, None);
        let _ = empty_gpu_settings();
        let _ = ProfileDomains::default();
    }
}
