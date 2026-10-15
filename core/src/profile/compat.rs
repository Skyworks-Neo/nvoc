//! Generation compatibility gates. A profile captured on one card may contain
//! domains the applying card physically cannot express (pre-Pascal cards
//! have no editable V/F table; legacy-voltage generations take a different
//! voltage path). Verdicts are explicit and reported — a domain is never
//! silently skipped.

use super::schema::DomainKind;
use crate::gpu_type::GpuType;

/// Why a domain cannot be applied to this GPU generation. `None` = supported.
pub fn domain_unsupported_reason(domain: DomainKind, gpu_type: GpuType) -> Option<String> {
    match domain {
        // Maxwell (900 series) and earlier expose no editable per-point V/F
        // curve at all — the same gate the GUI's VF ladder uses
        // (`GpuType::is_legacy_voltage`). 10–20 series have a *legacy-shaped*
        // table (`is_legacy_vfp`), which the setters already handle, so they
        // stay applicable.
        DomainKind::PublicVftable => {
            if gpu_type.is_legacy_voltage() {
                Some(format!(
                    "no editable V/F curve on {gpu_type:?} (pre-Pascal generation)"
                ))
            } else {
                None
            }
        }
        // Legacy-voltage generations take the SetPstates20 baseVoltage path,
        // not the NVML boost / NVAPI overvolt pair stored in the domain.
        DomainKind::Voltage => {
            if gpu_type.is_legacy_voltage() {
                Some(format!(
                    "voltage domain uses the legacy rail path on {gpu_type:?}, which profiles v1 does not write"
                ))
            } else {
                None
            }
        }
        DomainKind::VfpLocks => {
            Some("vfp_locks apply ships with the VFP lock workflow (not wired in v1)".to_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_type::GpuType;

    #[test]
    fn vftable_refused_on_pre_pascal_only() {
        assert!(
            domain_unsupported_reason(DomainKind::PublicVftable, GpuType::DesktopKepler).is_some()
        );
        // 10–20 series have a legacy-shaped but editable table.
        assert!(
            domain_unsupported_reason(DomainKind::PublicVftable, GpuType::Desktop10Series)
                .is_none()
        );
        assert!(
            domain_unsupported_reason(DomainKind::PublicVftable, GpuType::Desktop30Series)
                .is_none()
        );
    }

    #[test]
    fn voltage_refused_on_legacy_voltage() {
        assert!(domain_unsupported_reason(DomainKind::Voltage, GpuType::DesktopFermi).is_some());
        assert!(domain_unsupported_reason(DomainKind::Voltage, GpuType::Desktop10Series).is_none());
    }

    #[test]
    fn plain_domains_always_supported() {
        for domain in [
            DomainKind::ClockOffsets,
            DomainKind::Power,
            DomainKind::Thermal,
            DomainKind::Fan,
        ] {
            assert!(
                domain_unsupported_reason(domain, GpuType::DesktopKepler).is_none(),
                "{domain:?} should have no gate"
            );
        }
    }
}
