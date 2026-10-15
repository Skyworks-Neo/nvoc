//! GPU identity capture and the apply-time binding check.
//!
//! Binding model (decision D1): NVML UUID is the primary key; the hardware
//! fingerprint (PCI device/subsystem IDs, VBIOS version, model name, P-State
//! roster) is the apply-time second factor. The driver version is recorded
//! but never compared — a driver upgrade must not orphan a profile, it only
//! downgrades the verdict to a warning.

use super::schema::{GpuFingerprint, GpuIdentity};
use ::nvapi::hi::GpuInfo;
use std::fmt;

/// Result of comparing a profile's stored identity against the live GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityCheck {
    /// UUID and fingerprint agree (fingerprint fields absent on either side
    /// are skipped — "could not read" must not hard-fail a binding).
    Match,
    /// Same as [`IdentityCheck::Match`] but the live GPU exposed no UUID (or
    /// the profile was captured without one), so the match rests on the
    /// fingerprint alone.
    MatchDegraded,
    /// Identity agrees but the recorded driver version differs from the live
    /// one. Proceed, but read-back verification is mandatory.
    MatchDriverDrift {
        expected: Option<String>,
        actual: Option<String>,
    },
    /// The profile carries a UUID but this GPU has none: nothing to bind to.
    RefuseNoUuid { expected: String },
    /// Different physical card.
    RefuseUuidMismatch { expected: String, actual: String },
    /// Same UUID (or UUID-less match path) but the hardware no longer looks
    /// like the capture: VBIOS flash, different subsystem ID, etc.
    RefuseFingerprint { diffs: Vec<FingerprintDiff> },
}

impl IdentityCheck {
    pub fn is_allowed(&self) -> bool {
        matches!(
            self,
            IdentityCheck::Match
                | IdentityCheck::MatchDegraded
                | IdentityCheck::MatchDriverDrift { .. }
        )
    }

    /// True when the caller must run read-back verification even if the user
    /// did not ask for it (driver drift).
    pub fn requires_verify(&self) -> bool {
        matches!(self, IdentityCheck::MatchDriverDrift { .. })
    }
}

impl fmt::Display for IdentityCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityCheck::Match => write!(f, "identity match"),
            IdentityCheck::MatchDegraded => {
                write!(f, "identity match (degraded: no UUID, fingerprint only)")
            }
            IdentityCheck::MatchDriverDrift { expected, actual } => write!(
                f,
                "identity match with driver drift (captured {expected:?}, live {actual:?})"
            ),
            IdentityCheck::RefuseNoUuid { expected } => write!(
                f,
                "refusing: profile is bound to UUID {expected:?} but this GPU exposes no UUID"
            ),
            IdentityCheck::RefuseUuidMismatch { expected, actual } => write!(
                f,
                "refusing: profile is bound to UUID {expected:?}, this GPU is {actual:?}"
            ),
            IdentityCheck::RefuseFingerprint { diffs } => {
                write!(f, "refusing: hardware fingerprint drifted from capture")?;
                for diff in diffs {
                    write!(f, "\n  - {diff}")?;
                }
                Ok(())
            }
        }
    }
}

/// One fingerprint field that differs between capture and live GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintDiff {
    pub field: &'static str,
    pub expected: String,
    pub actual: String,
}

impl fmt::Display for FingerprintDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: captured {}, live {}",
            self.field, self.expected, self.actual
        )
    }
}

fn diff_option<T: PartialEq + std::fmt::Debug>(
    field: &'static str,
    expected: &Option<T>,
    actual: &Option<T>,
    diffs: &mut Vec<FingerprintDiff>,
) {
    if let (Some(e), Some(a)) = (expected, actual)
        && e != a
    {
        diffs.push(FingerprintDiff {
            field,
            expected: format!("{e:?}"),
            actual: format!("{a:?}"),
        });
    }
}

/// Field-by-field fingerprint comparison. Hard diffs only where both sides
/// could read the field.
pub fn compare_fingerprints(
    expected: &GpuFingerprint,
    actual: &GpuFingerprint,
) -> Vec<FingerprintDiff> {
    let mut diffs = Vec::new();
    diff_option(
        "device_id",
        &expected.device_id,
        &actual.device_id,
        &mut diffs,
    );
    diff_option(
        "subsystem_id",
        &expected.subsystem_id,
        &actual.subsystem_id,
        &mut diffs,
    );
    diff_option(
        "vbios_version",
        &expected.vbios_version,
        &actual.vbios_version,
        &mut diffs,
    );
    diff_option(
        "model_name",
        &expected.model_name,
        &actual.model_name,
        &mut diffs,
    );
    if !expected.pstate_roster.is_empty()
        && !actual.pstate_roster.is_empty()
        && expected.pstate_roster != actual.pstate_roster
    {
        diffs.push(FingerprintDiff {
            field: "pstate_roster",
            expected: format!("{:?}", expected.pstate_roster),
            actual: format!("{:?}", actual.pstate_roster),
        });
    }
    diffs
}

/// The apply-time binding decision. `expected` is the profile's stored
/// identity; `actual` is the live GPU's.
pub fn check_identity(expected: &GpuIdentity, actual: &GpuIdentity) -> IdentityCheck {
    match (&expected.uuid, &actual.uuid) {
        (Some(e), Some(a)) if e != a => {
            return IdentityCheck::RefuseUuidMismatch {
                expected: e.clone(),
                actual: a.clone(),
            };
        }
        (Some(e), None) => {
            return IdentityCheck::RefuseNoUuid {
                expected: e.clone(),
            };
        }
        _ => {}
    }

    let diffs = compare_fingerprints(&expected.fingerprint, &actual.fingerprint);
    if !diffs.is_empty() {
        return IdentityCheck::RefuseFingerprint { diffs };
    }

    let degraded = expected.uuid.is_none() || actual.uuid.is_none();
    let driver_drift = matches!(
        (
            &expected.fingerprint.driver_version,
            &actual.fingerprint.driver_version
        ),
        (Some(e), Some(a)) if e != a
    );

    if driver_drift {
        IdentityCheck::MatchDriverDrift {
            expected: expected.fingerprint.driver_version.clone(),
            actual: actual.fingerprint.driver_version.clone(),
        }
    } else if degraded {
        IdentityCheck::MatchDegraded
    } else {
        IdentityCheck::Match
    }
}

/// Directory-safe slug for a UUID: keep `A-Za-z0-9-_.`, fold everything else
/// to `-`, collapse runs, trim. Falls back to `gpu-unknown` when empty.
pub fn uuid_slug(uuid: &str) -> String {
    let mut out = String::with_capacity(uuid.len());
    let mut last_dash = false;
    for ch in uuid.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_');
        if keep {
            out.push(ch);
            last_dash = ch == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "gpu-unknown".to_owned()
    } else {
        trimmed
    }
}

/// File-name-safe slug for a profile name (same rules, plus lowercase).
pub fn name_slug(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for ch in lower.chars() {
        let keep = ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_');
        if keep {
            out.push(ch);
            last_dash = ch == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "profile".to_owned()
    } else {
        trimmed
    }
}

/// Build a [`GpuFingerprint`] from a live `GpuInfo` plus out-of-band reads
/// (UUID / driver version come from NVML, PCI address from the target).
pub fn capture_fingerprint(
    info: &GpuInfo,
    uuid: Option<String>,
    driver_version: Option<String>,
    pci_address: Option<String>,
) -> GpuIdentity {
    let pci_ids = info.bus.bus.pci_ids();
    GpuIdentity {
        uuid,
        fingerprint: GpuFingerprint {
            device_id: pci_ids.map(|ids| ids.device_id),
            subsystem_id: pci_ids.map(|ids| ids.subsystem_id),
            vbios_version: Some(info.bios_version.clone()).filter(|s| !s.is_empty()),
            model_name: Some(info.name.clone()).filter(|s| !s.is_empty()),
            pstate_roster: info.pstate_limits.keys().copied().collect(),
            driver_version,
        },
        pci_address_at_capture: pci_address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(fields: &[(&'static str, Option<&str>)]) -> GpuFingerprint {
        let mut f = GpuFingerprint::default();
        for (field, value) in fields {
            match *field {
                "device_id" => f.device_id = value.map(|v| v.parse().unwrap()),
                "subsystem_id" => f.subsystem_id = value.map(|v| v.parse().unwrap()),
                "vbios" => f.vbios_version = value.map(str::to_owned),
                "model" => f.model_name = value.map(str::to_owned),
                "driver" => f.driver_version = value.map(str::to_owned),
                _ => panic!("unknown field {field}"),
            }
        }
        f
    }

    fn ident(uuid: Option<&str>, fingerprint: GpuFingerprint) -> GpuIdentity {
        GpuIdentity {
            uuid: uuid.map(str::to_owned),
            fingerprint,
            pci_address_at_capture: None,
        }
    }

    #[test]
    fn uuid_and_fingerprint_match() {
        let expected = ident(Some("GPU-A"), fp(&[("device_id", Some("132"))]));
        let actual = ident(Some("GPU-A"), fp(&[("device_id", Some("132"))]));
        assert_eq!(check_identity(&expected, &actual), IdentityCheck::Match);
    }

    #[test]
    fn uuid_mismatch_refuses() {
        let expected = ident(Some("GPU-A"), GpuFingerprint::default());
        let actual = ident(Some("GPU-B"), GpuFingerprint::default());
        assert_eq!(
            check_identity(&expected, &actual),
            IdentityCheck::RefuseUuidMismatch {
                expected: "GPU-A".to_owned(),
                actual: "GPU-B".to_owned(),
            }
        );
    }

    #[test]
    fn live_gpu_without_uuid_refuses_when_profile_has_one() {
        let expected = ident(Some("GPU-A"), GpuFingerprint::default());
        let actual = ident(None, GpuFingerprint::default());
        assert_eq!(
            check_identity(&expected, &actual),
            IdentityCheck::RefuseNoUuid {
                expected: "GPU-A".to_owned(),
            }
        );
    }

    #[test]
    fn fingerprint_mismatch_refuses_with_diffs() {
        let expected = ident(
            Some("GPU-A"),
            fp(&[("device_id", Some("132")), ("vbios", Some("94.01"))]),
        );
        let actual = ident(
            Some("GPU-A"),
            fp(&[("device_id", Some("132")), ("vbios", Some("96.02"))]),
        );
        let verdict = check_identity(&expected, &actual);
        assert!(
            matches!(verdict, IdentityCheck::RefuseFingerprint { ref diffs } if diffs.len() == 1)
        );
        assert!(!verdict.is_allowed());
    }

    #[test]
    fn unreadable_fingerprint_fields_do_not_block() {
        let expected = ident(Some("GPU-A"), fp(&[("vbios", Some("94.01"))]));
        let actual = ident(Some("GPU-A"), fp(&[]));
        assert_eq!(check_identity(&expected, &actual), IdentityCheck::Match);
    }

    #[test]
    fn driver_drift_warns_but_allows() {
        let expected = ident(Some("GPU-A"), fp(&[("driver", Some("582.41"))]));
        let actual = ident(Some("GPU-A"), fp(&[("driver", Some("610.47"))]));
        let verdict = check_identity(&expected, &actual);
        assert!(verdict.is_allowed());
        assert!(verdict.requires_verify());
    }

    #[test]
    fn uuidless_profiles_match_on_fingerprint() {
        let expected = ident(None, fp(&[("device_id", Some("132"))]));
        let actual = ident(Some("GPU-Z"), fp(&[("device_id", Some("132"))]));
        assert_eq!(
            check_identity(&expected, &actual),
            IdentityCheck::MatchDegraded
        );
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(
            uuid_slug("GPU-3f2c9a1e-1234-5678"),
            "GPU-3f2c9a1e-1234-5678"
        );
        assert_eq!(uuid_slug("weird//..id"), "weird-id");
        assert_eq!(uuid_slug(""), "gpu-unknown");
        assert_eq!(name_slug("Silent Gaming!"), "silent-gaming");
        assert_eq!(name_slug("  "), "profile");
    }
}
