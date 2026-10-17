//! Profile persistence schema (`schema_version = 1`).
//!
//! A [`ProfileDoc`] is a versioned, self-describing JSON document that binds a
//! set of captured overclocking settings ("domains") to one physical GPU. Two
//! invariants drive every field decision:
//!
//! 1. **Absent domain = untouched hardware.** Each domain in
//!    [`ProfileDomains`] is `Option`; a domain that was not captured is never
//!    written on apply (the MSI Afterburner `ProfileContents` mask idea,
//!    expressed as omitted fields instead of a bitmask).
//! 2. **Units are schema-fixed** (kHz / µV / W / %). Frontends convert for
//!    display only; the file never carries per-field unit soup.
//!
//! Value types are reused verbatim from `nvapi`'s serde-enabled `hi` layer
//! ([`GpuSettings`], [`VfpDeltas`], [`KilohertzDelta`], …) so a captured
//! profile round-trips bit-exactly against what [`QueryGpuSettings`] reads and
//! the write ops accept — no translation layer to drift.

use ::nvapi::hi::{
    ClockDomain, ClockLockEntry, CoolerSettings, FanCoolerId, GpuSettings, KilohertzDelta,
    MicrovoltsDelta, PState, Percentage, PerfLimitId, SensorThrottle, VfpDeltas,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Serde fan-curve point (schema-owned): `core::result::FanCurvePointReadout`
/// predates the profile module and carries no serde derives, so persistence
/// uses this mirror and the apply boundary converts.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FanCurvePointRecord {
    pub temp_c: u16,
    pub rpm: u32,
}

impl From<crate::result::FanCurvePointReadout> for FanCurvePointRecord {
    fn from(p: crate::result::FanCurvePointReadout) -> Self {
        Self {
            temp_c: p.temp_c,
            rpm: p.rpm,
        }
    }
}

impl From<FanCurvePointRecord> for crate::result::FanCurvePointReadout {
    fn from(p: FanCurvePointRecord) -> Self {
        Self {
            temp_c: p.temp_c,
            rpm: p.rpm,
        }
    }
}

/// The only schema version this module reads and writes. Matches the strict
/// `scan_log.rs` precedent: an unexpected version is a hard error, not a
/// best-effort parse.
pub const SCHEMA_VERSION: u32 = 1;

/// Upper bound on profiles per package (borrowed from xOCD's
/// "too many profiles" rejection ladder).
pub const MAX_PACKAGE_PROFILES: usize = 64;

/// `kind` discriminator of a single-slot profile file.
pub const PROFILE_KIND: &str = "nvoc-profile";

/// `kind` discriminator of a multi-slot portable package file.
pub const PACKAGE_KIND: &str = "nvoc-profile-package";

/// `kind` discriminator of a settings snapshot file (`stock.json` /
/// `backup-last.json`).
pub const BACKUP_KIND: &str = "nvoc-settings-backup";

/// One adjustable settings family. Order in [`DOMAIN_APPLY_ORDER`] is the
/// fixed apply sequence; `--domains` on `profile save` filters captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DomainKind {
    /// Per-PState per-domain clock deltas (the `pstate_deltas` plane).
    ClockOffsets,
    /// Public per-point V/F table deltas, graphics + memory (the `vfp` plane).
    PublicVftable,
    /// Voltage boost percent + core overvolt deltas.
    Voltage,
    /// Per-rail µV offsets on the private VoltRails plane (melonVolt path,
    /// keyed by rail bit). Written after `Voltage` (same plane family).
    VoltRails,
    /// Power limit (watts via NVML, or percent via NVAPI).
    Power,
    /// Thermal / sensor throttle limits.
    Thermal,
    /// Fan curves (per cooler index).
    Fan,
    /// Min/max clock locks. High-risk; never captured by default.
    LockedClocks,
    /// Per-perf-limit client locks (freq/voltage locks and PstateSelect
    /// pins, keyed by `PerfLimitId`). High-risk; never captured by default.
    VfpLocks,
}

impl DomainKind {
    pub const ALL: [DomainKind; 9] = [
        DomainKind::ClockOffsets,
        DomainKind::PublicVftable,
        DomainKind::Voltage,
        DomainKind::VoltRails,
        DomainKind::Power,
        DomainKind::Thermal,
        DomainKind::Fan,
        DomainKind::LockedClocks,
        DomainKind::VfpLocks,
    ];

    /// Domains `profile save` captures unless `--domains` says otherwise.
    /// The `vfp_locks` family IS the get-lock-status table (PerfClientLimits
    /// GET): saving it is read-only, and its apply replays the exact table
    /// (mode-None entries unlock), so it rides along by default. The two
    /// `locked_clocks`/authoring families stay out: that plane has no
    /// capture source yet.
    pub const DEFAULT_CAPTURE: [DomainKind; 8] = [
        DomainKind::ClockOffsets,
        DomainKind::PublicVftable,
        DomainKind::Voltage,
        DomainKind::VoltRails,
        DomainKind::Power,
        DomainKind::Thermal,
        DomainKind::Fan,
        DomainKind::VfpLocks,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DomainKind::ClockOffsets => "clock_offsets",
            DomainKind::PublicVftable => "public_vftable",
            DomainKind::Voltage => "voltage",
            DomainKind::VoltRails => "volt_rails",
            DomainKind::Power => "power",
            DomainKind::Thermal => "thermal",
            DomainKind::Fan => "fan",
            DomainKind::LockedClocks => "locked_clocks",
            DomainKind::VfpLocks => "vfp_locks",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        DomainKind::ALL
            .into_iter()
            .find(|d| d.as_str() == raw.trim().to_ascii_lowercase())
    }
}

impl std::fmt::Display for DomainKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Fixed apply execution order. Resets happen inside the domain appliers that
/// need them (e.g. `PublicVftable` resets the public table before replaying
/// deltas); the order exists so global offsets land before per-point curves
/// and lock-style writes land last.
pub const DOMAIN_APPLY_ORDER: [DomainKind; 9] = DomainKind::ALL;

/// Identity binding stored in every profile. `uuid` is the primary key (NVML
/// `GPU-…`); the fingerprint is the apply-time second factor. The PCI address
/// is recorded for humans only — it must never participate in matching
/// (Afterburner keys on bus position, which breaks when the card is reseated).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    pub fingerprint: GpuFingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pci_address_at_capture: Option<String>,
}

/// Hardware fingerprint compared field-by-field at apply time. `None` fields
/// mean "could not be read at capture"; comparison only fails on
/// both-present-and-different so degraded readbacks never hard-lock a profile
/// to its capture machine.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuFingerprint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsystem_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vbios_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pstate_roster: Vec<PState>,
    /// Recorded, never compared: a driver upgrade is expected and must not
    /// invalidate the binding (it only downgrades the verdict to a warning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
}

/// The captured settings domains. Every field omitted = "apply must not touch
/// this plane".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileDomains {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_offsets: Option<BTreeMap<PState, BTreeMap<ClockDomain, KilohertzDelta>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_vftable: Option<VfpDeltas>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<VoltageSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volt_rails: Option<Vec<VoltRailLimit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<Vec<SensorThrottle>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan: Option<FanSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_clocks: Option<BTreeMap<ClockDomain, LockedClocks>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vfp_locks: Option<BTreeMap<PerfLimitId, ClockLockEntry>>,
}

impl ProfileDomains {
    /// Which domains are present (`Some`) in this document, in
    /// [`DOMAIN_APPLY_ORDER`].
    pub fn present(&self) -> Vec<DomainKind> {
        DOMAIN_APPLY_ORDER
            .into_iter()
            .filter(|d| self.contains(*d))
            .collect()
    }

    pub fn contains(&self, domain: DomainKind) -> bool {
        match domain {
            DomainKind::ClockOffsets => self.clock_offsets.is_some(),
            DomainKind::PublicVftable => self.public_vftable.is_some(),
            DomainKind::Voltage => self.voltage.is_some(),
            DomainKind::VoltRails => self.volt_rails.is_some(),
            DomainKind::Power => self.power.is_some(),
            DomainKind::Thermal => self.thermal.is_some(),
            DomainKind::Fan => self.fan.is_some(),
            DomainKind::LockedClocks => self.locked_clocks.is_some(),
            DomainKind::VfpLocks => self.vfp_locks.is_some(),
        }
    }
}

/// One volt-rail limit record (private VoltRails / melonVolt plane), keyed
/// by rail bit. `offset_uv` is the ABSOLUTE µV offset from stock (0 =
/// stock), matching the driver's offset semantics. `expected_type` carries
/// the control entry's type discriminator at capture (e.g. 3 on 5090
/// MSVDD) — re-checked at apply unless `None`, refusing the write when the
/// entry shape changed (type semantics differ per platform).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoltRailLimit {
    pub rail_bit: u32,
    pub offset_uv: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<u32>,
    /// Full melonVolt control payload captured verbatim: the 6 dwords
    /// (`values[0..6]`), slot 0 being the µV offset mirrored in
    /// `offset_uv`, slot 2 the VRM max-wall offset, slot 3 the VMIN /
    /// min-hold offset (both mV-in/uV-out, P100/582.41-pinned), slots
    /// 1/4/5 retained-but-quiet firmware-opaque. `None` on documents
    /// captured before slot persistence — apply then touches slot 0 only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<[i32; 6]>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct VoltageSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_percent: Option<Percentage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overvolt_uv: Vec<MicrovoltsDelta>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerSettings {
    /// Absolute watt limit (NVML path). Preferred over `percent` when both
    /// are present because watts survive range-width differences across
    /// VBIOS flashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<Percentage>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct FanSettings {
    /// Per cooler-index fan curve points.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub curves: BTreeMap<u8, Vec<FanCurvePointRecord>>,
    /// Static cooler policies captured for reference. v1 does not apply these
    /// (`SetCoolerLevels` interacts with live cooler arbitration); the field
    /// exists so a capture is not silently lossy.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cooler_settings: BTreeMap<FanCoolerId, CoolerSettings>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockedClocks {
    pub min_mhz: u32,
    pub max_mhz: u32,
}

/// Reference values captured alongside the profile: the stock state at
/// capture time, used by `diff`, drift detection and future `--rebase`
/// rebinding. Replaces issue #360's SHA-256 `curve_sha256` with the lossless
/// original values (a hash can only detect drift, not repair it).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BaselineBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_vfp: Option<VfpDeltas>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stock_pstate_deltas: BTreeMap<PState, BTreeMap<ClockDomain, KilohertzDelta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_min_watts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_max_watts: Option<u32>,
}

/// Optional user-recorded validation evidence (e.g. "30 min Superposition
/// stable").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ValidationRecord {
    pub checked_utc: String,
    pub kind: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A single named profile bound to one GPU.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileDoc {
    pub schema_version: u32,
    /// Always [`PROFILE_KIND`]; rejected otherwise on load.
    pub kind: String,
    pub name: String,
    pub created_utc: String,
    pub updated_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub gpu: GpuIdentity,
    #[serde(default)]
    pub domains: ProfileDomains,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<ValidationRecord>,
}

impl ProfileDoc {
    /// Build a doc with `schema_version`/`kind`/timestamps filled in.
    pub fn new(name: impl Into<String>, identity: GpuIdentity, now_utc: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind: PROFILE_KIND.to_owned(),
            name: name.into(),
            created_utc: now_utc.clone(),
            updated_utc: now_utc,
            note: None,
            gpu: identity,
            domains: ProfileDomains::default(),
            baseline: None,
            validation: None,
        }
    }

    /// Structural validation applied on every load/import. Returns human
    /// readable reasons; the caller decides whether to aggregate.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported profile schema_version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if self.kind != PROFILE_KIND {
            return Err(format!(
                "not a {PROFILE_KIND} document (kind={:?})",
                self.kind
            ));
        }
        if self.name.trim().is_empty() {
            return Err("profile name is empty".to_owned());
        }
        if self.domains.present().is_empty() {
            return Err("profile captures no settings domains".to_owned());
        }
        Ok(())
    }
}

/// `GpuSettings` has no `Default` (nvapi-rs keeps the type strict); build the
/// all-empty snapshot used by `SettingsBackup::new` before a real capture
/// overwrites it.
pub fn empty_gpu_settings() -> GpuSettings {
    GpuSettings {
        voltage_boost: None,
        sensor_limits: Vec::new(),
        power_limits: Vec::new(),
        coolers: BTreeMap::new(),
        vfp: None,
        pstate_deltas: BTreeMap::new(),
        overvolt: Vec::new(),
        vfp_locks: BTreeMap::new(),
        pstates: Vec::new(),
    }
}

/// NVML power-limit bounds snapshot (min/current/max, watts).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerWattsBounds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u32>,
    pub current: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

/// A full `GpuSettings` snapshot bound to a GPU — the payload of
/// `stock.json` and `backup-last.json`, and the rollback source for
/// `profile apply`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsBackup {
    pub schema_version: u32,
    pub kind: String,
    pub captured_utc: String,
    pub gpu: GpuIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub settings: GpuSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fan_curves: Vec<FanCurveReadoutRecord>,
    /// Full VoltRails control-plane snapshot (all present rails) — the
    /// rollback source for the `volt_rails` domain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volt_rails: Vec<VoltRailLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_watts: Option<PowerWattsBounds>,
}

impl SettingsBackup {
    pub fn new(identity: GpuIdentity, now_utc: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind: BACKUP_KIND.to_owned(),
            captured_utc: now_utc,
            gpu: identity,
            note: None,
            settings: empty_gpu_settings(),
            fan_curves: Vec::new(),
            volt_rails: Vec::new(),
            power_watts: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported backup schema_version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if self.kind != BACKUP_KIND {
            return Err(format!(
                "not a {BACKUP_KIND} document (kind={:?})",
                self.kind
            ));
        }
        Ok(())
    }
}

/// One fan curve as read back by `GetFanCurves` (index + points), stored in
/// [`SettingsBackup`] so a rollback can restore curve *shape*, which
/// `GpuSettings.coolers` does not carry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FanCurveReadoutRecord {
    pub index: u8,
    pub points: Vec<FanCurvePointRecord>,
}

/// Multi-slot portable package: the import/export interchange format
/// (`*.nvocprofile.json`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfilePackage {
    pub schema_version: u32,
    pub kind: String,
    pub created_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    pub profiles: Vec<ProfileDoc>,
}

impl ProfilePackage {
    pub fn new(now_utc: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind: PACKAGE_KIND.to_owned(),
            created_utc: now_utc,
            source_host: None,
            purpose: None,
            profiles: Vec::new(),
        }
    }

    /// Rejection ladder applied on import, in order: version → kind → empty →
    /// too many → per-slot structural errors (aggregated, never silently
    /// dropped).
    pub fn validate(&self) -> Result<(), PackageRejection> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(PackageRejection::unsupported_version(self.schema_version));
        }
        if self.kind != PACKAGE_KIND {
            return Err(PackageRejection::bad_kind(&self.kind));
        }
        if self.profiles.is_empty() {
            return Err(PackageRejection::empty());
        }
        if self.profiles.len() > MAX_PACKAGE_PROFILES {
            return Err(PackageRejection::too_many(self.profiles.len()));
        }
        let mut slot_errors = Vec::new();
        for (index, doc) in self.profiles.iter().enumerate() {
            if let Err(reason) = doc.validate() {
                slot_errors.push((index, doc.name.clone(), reason));
            }
        }
        if !slot_errors.is_empty() {
            return Err(PackageRejection::invalid_slots(slot_errors));
        }
        Ok(())
    }
}

/// Structured package rejection with a stable machine-readable `code` and the
/// per-slot error list for partial-failure reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRejection {
    pub code: &'static str,
    pub message: String,
    pub slot_errors: Vec<(usize, String, String)>,
}

impl PackageRejection {
    fn unsupported_version(found: u32) -> Self {
        Self {
            code: "unsupported-version",
            message: format!(
                "unsupported package schema_version {found} (expected {SCHEMA_VERSION})"
            ),
            slot_errors: Vec::new(),
        }
    }

    fn bad_kind(kind: &str) -> Self {
        Self {
            code: "bad-kind",
            message: format!("not a {PACKAGE_KIND} document (kind={kind:?})"),
            slot_errors: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            code: "empty",
            message: "the profile package contains no profiles".to_owned(),
            slot_errors: Vec::new(),
        }
    }

    fn too_many(count: usize) -> Self {
        Self {
            code: "too-many",
            message: format!("the package contains {count} profiles (max {MAX_PACKAGE_PROFILES})"),
            slot_errors: Vec::new(),
        }
    }

    fn invalid_slots(slot_errors: Vec<(usize, String, String)>) -> Self {
        Self {
            code: "invalid-slots",
            message: format!(
                "{} profile(s) in the package are invalid",
                slot_errors.len()
            ),
            slot_errors,
        }
    }
}

impl std::fmt::Display for PackageRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)?;
        for (index, name, reason) in &self.slot_errors {
            write!(f, "\n  - slot[{index}] {name:?}: {reason}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> ProfileDoc {
        let mut doc = ProfileDoc::new(
            "test",
            GpuIdentity::default(),
            "2026-10-15T00:00:00Z".to_owned(),
        );
        doc.domains.power = Some(PowerSettings {
            watts: Some(170),
            percent: None,
        });
        doc
    }

    #[test]
    fn profile_doc_roundtrips() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        let back: ProfileDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
        assert!(json.contains("nvoc-profile"));
    }

    #[test]
    fn profile_doc_rejects_unknown_fields() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        let mut json = json.strip_suffix('}').unwrap().to_owned();
        json.push_str(", \"bogus_field\": 1}");
        assert!(serde_json::from_str::<ProfileDoc>(&json).is_err());
    }

    #[test]
    fn profile_doc_rejects_wrong_schema_version() {
        let mut doc = sample_doc();
        doc.schema_version = 999;
        assert!(doc.validate().is_err());
        let json = serde_json::to_string(&doc).unwrap();
        let err = serde_json::from_str::<ProfileDoc>(&json).unwrap();
        assert!(err.validate().is_err());
    }

    #[test]
    fn absent_domains_are_not_serialized() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        assert!(!json.contains("clock_offsets"));
        assert!(!json.contains("vfp_locks"));
        assert!(json.contains("power"));
    }

    #[test]
    fn volt_rail_record_without_slots_is_valid_v1() {
        // Documents captured before slot persistence carry no `slots` key;
        // they must keep loading (apply then touches slot 0 only).
        let json = serde_json::json!({
            "rail_bit": 1,
            "offset_uv": 15000,
            "expected_type": 3
        });
        let record: VoltRailLimit = serde_json::from_value(json).unwrap();
        assert_eq!(record.offset_uv, 15000);
        assert_eq!(record.slots, None);
    }

    #[test]
    fn empty_profile_fails_validation() {
        let doc = ProfileDoc::new("x", GpuIdentity::default(), "t".to_owned());
        assert!(doc.validate().is_err());
    }

    #[test]
    fn package_rejection_ladder() {
        let mut pkg = ProfilePackage::new("t".to_owned());
        assert_eq!(pkg.validate().unwrap_err().code, "empty");

        pkg.schema_version = 42;
        assert_eq!(pkg.validate().unwrap_err().code, "unsupported-version");
        pkg.schema_version = SCHEMA_VERSION;

        pkg.kind = "other".to_owned();
        assert_eq!(pkg.validate().unwrap_err().code, "bad-kind");
        pkg.kind = PACKAGE_KIND.to_owned();

        pkg.profiles.push(sample_doc());
        assert!(pkg.validate().is_ok());

        pkg.profiles.push(ProfileDoc::new(
            "empty",
            GpuIdentity::default(),
            "t".to_owned(),
        ));
        let err = pkg.validate().unwrap_err();
        assert_eq!(err.code, "invalid-slots");
        assert_eq!(err.slot_errors.len(), 1);
    }

    #[test]
    fn package_rejects_too_many() {
        let mut pkg = ProfilePackage::new("t".to_owned());
        for i in 0..(MAX_PACKAGE_PROFILES + 1) {
            let mut doc = sample_doc();
            doc.name = format!("p{i}");
            pkg.profiles.push(doc);
        }
        assert_eq!(pkg.validate().unwrap_err().code, "too-many");
    }

    #[test]
    fn domain_kind_parse_roundtrip() {
        for d in DomainKind::ALL {
            assert_eq!(DomainKind::parse(d.as_str()), Some(d));
        }
        assert_eq!(
            DomainKind::parse("CLOCK_OFFSETS"),
            Some(DomainKind::ClockOffsets)
        );
        assert_eq!(DomainKind::parse("nope"), None);
    }
}
