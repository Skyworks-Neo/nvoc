//! Apply planner: turn a [`ProfileDoc`] plus live GPU state into a verified
//! execution plan *before* any hardware write happens. `profile apply
//! --dry-run` prints exactly this; the executor in `apply.rs` refuses to run
//! anything that is not in a plan.

use super::compat::domain_unsupported_reason;
use super::identity::IdentityCheck;
use super::schema::{DOMAIN_APPLY_ORDER, DomainKind, ProfileDoc};
use crate::gpu_type::GpuType;

/// Caller options that shape the plan (never the schema).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanOptions {
    /// Rebind the profile to this GPU despite a fingerprint drift (VBIOS
    /// flash / subsystem change). Deltas are kept as-is; the caller is
    /// responsible for refreshing the baseline afterwards.
    pub rebase: bool,
}

/// Per-domain verdict. `Refuse` aborts the whole apply (the operator asked to
/// apply a profile that cannot be honored); `NotApplicable` skips one domain
/// with a printed reason while the rest proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainVerdict {
    Apply,
    /// The card cannot express this domain (generation gate).
    NotApplicable {
        reason: String,
    },
    /// Wired in a later workflow; reported, never silent.
    Unimplemented {
        reason: String,
    },
}

/// The verified plan.
#[derive(Debug, Clone)]
pub struct ApplyPlan {
    pub identity: IdentityCheck,
    /// Domains that will be written, in [`DOMAIN_APPLY_ORDER`].
    pub steps: Vec<DomainKind>,
    /// Domain → generation-gate skip (with reason).
    pub not_applicable: Vec<(DomainKind, String)>,
    /// Domain → not wired in this version (with reason).
    pub unimplemented: Vec<(DomainKind, String)>,
    /// Non-fatal notes (driver drift, rebase, mutual-existence warnings).
    pub warnings: Vec<String>,
}

impl ApplyPlan {
    /// True when every verdict allows execution. Refusals live in
    /// [`ApplyPlan::refusal`].
    pub fn is_runnable(&self) -> bool {
        self.refusal().is_none()
    }

    /// The identity refusal reason, if the plan is blocked.
    pub fn refusal(&self) -> Option<String> {
        match &self.identity {
            IdentityCheck::RefuseNoUuid { .. }
            | IdentityCheck::RefuseUuidMismatch { .. }
            | IdentityCheck::RefuseFingerprint { .. } => Some(self.identity.to_string()),
            _ => None,
        }
    }

    /// Human-readable plan rendering (shared by CLI human and `--dry-run`).
    pub fn describe(&self, profile_name: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("apply plan for profile {profile_name:?}\n"));
        out.push_str(&format!("  identity: {}\n", self.identity));
        for warning in &self.warnings {
            out.push_str(&format!("  warning: {warning}\n"));
        }
        for domain in &self.steps {
            out.push_str(&format!("  apply:    {domain}\n"));
        }
        for (domain, reason) in &self.not_applicable {
            out.push_str(&format!("  skip:     {domain} — {reason}\n"));
        }
        for (domain, reason) in &self.unimplemented {
            out.push_str(&format!("  deferred: {domain} — {reason}\n"));
        }
        out
    }
}

/// Build the plan. Pure: everything is decided from the profile document, the
/// live identity and the GPU generation — no hardware calls.
pub fn build_plan(
    doc: &ProfileDoc,
    live_identity: &super::schema::GpuIdentity,
    gpu_type: GpuType,
    options: &PlanOptions,
) -> ApplyPlan {
    let mut identity = super::identity::check_identity(&doc.gpu, live_identity);
    let mut warnings = Vec::new();

    if let IdentityCheck::RefuseFingerprint { .. } = identity
        && options.rebase
    {
        warnings.push(
            "--rebase: rebinding profile to this GPU despite fingerprint drift; \
             refresh the baseline (profile capture-stock) after apply"
                .to_owned(),
        );
        identity = IdentityCheck::MatchDegraded;
    }

    if identity.requires_verify() {
        warnings.push(
            "driver version changed since capture; read-back verification is mandatory".to_owned(),
        );
    }

    // Mutual-existence note: global offsets and per-point curves are separate
    // driver planes with separate reset paths — both may be applied, in the
    // fixed order, but the operator should know they coexist.
    if doc.domains.clock_offsets.is_some() && doc.domains.public_vftable.is_some() {
        warnings.push(
            "profile contains both global clock offsets and per-point V/F deltas; \
             they target different planes and are applied offsets-first"
                .to_owned(),
        );
    }

    let mut steps = Vec::new();
    let mut not_applicable = Vec::new();
    let mut unimplemented = Vec::new();

    for domain in DOMAIN_APPLY_ORDER {
        if !doc.domains.contains(domain) {
            continue;
        }
        if let Some(reason) = domain_unsupported_reason(domain, gpu_type) {
            not_applicable.push((domain, reason));
            continue;
        }
        // LockedClocks is schema-present but the apply wiring lands with the
        // lock workflow (locks are opt-in, never default-captured).
        if domain == DomainKind::LockedClocks {
            unimplemented.push((
                domain,
                "locked_clocks apply ships with the clock-lock workflow (not wired in v1)"
                    .to_owned(),
            ));
            continue;
        }
        steps.push(domain);
    }

    ApplyPlan {
        identity,
        steps,
        not_applicable,
        unimplemented,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_type::GpuType;
    use crate::profile::schema::{GpuIdentity, PowerSettings, ProfileDoc, ProfileDomains};
    use std::collections::BTreeMap;

    fn empty_vfp() -> ::nvapi::hi::VfpDeltas {
        ::nvapi::hi::VfpDeltas {
            graphics: BTreeMap::new(),
            memory: BTreeMap::new(),
        }
    }

    fn doc_with(domains: impl FnOnce(&mut ProfileDomains)) -> ProfileDoc {
        let mut doc = ProfileDoc::new(
            "plan-test",
            GpuIdentity {
                uuid: Some("GPU-A".to_owned()),
                ..Default::default()
            },
            "t".to_owned(),
        );
        domains(&mut doc.domains);
        doc
    }

    fn live() -> GpuIdentity {
        GpuIdentity {
            uuid: Some("GPU-A".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn plain_plan_applies_in_fixed_order() {
        let doc = doc_with(|d| {
            d.fan = Some(Default::default());
            d.power = Some(PowerSettings::default());
            d.clock_offsets = Some(Default::default());
        });
        let plan = build_plan(
            &doc,
            &live(),
            GpuType::Desktop40Series,
            &PlanOptions::default(),
        );
        assert!(plan.is_runnable());
        assert_eq!(
            plan.steps,
            vec![DomainKind::ClockOffsets, DomainKind::Power, DomainKind::Fan]
        );
        assert!(plan.not_applicable.is_empty());
        // offsets + vftable coexistence warning fires only when both present
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn coexistence_warning_fires_for_both_planes() {
        let doc = doc_with(|d| {
            d.clock_offsets = Some(Default::default());
            d.public_vftable = Some(empty_vfp());
        });
        let plan = build_plan(
            &doc,
            &live(),
            GpuType::Desktop40Series,
            &PlanOptions::default(),
        );
        assert!(plan.warnings.iter().any(|w| w.contains("different planes")));
    }

    #[test]
    fn generation_gate_marks_not_applicable() {
        let doc = doc_with(|d| {
            d.public_vftable = Some(empty_vfp());
            d.power = Some(PowerSettings::default());
        });
        let plan = build_plan(
            &doc,
            &live(),
            GpuType::DesktopKepler,
            &PlanOptions::default(),
        );
        assert!(plan.is_runnable());
        assert_eq!(plan.steps, vec![DomainKind::Power]);
        assert_eq!(plan.not_applicable.len(), 1);
        assert!(plan.not_applicable[0].1.contains("pre-Pascal"));
    }

    #[test]
    fn vfp_locks_apply_last_in_order() {
        let doc = doc_with(|d| {
            d.vfp_locks = Some(Default::default());
            d.power = Some(PowerSettings::default());
        });
        let plan = build_plan(
            &doc,
            &live(),
            GpuType::Desktop40Series,
            &PlanOptions::default(),
        );
        assert_eq!(plan.steps, vec![DomainKind::Power, DomainKind::VfpLocks]);
        assert!(plan.unimplemented.is_empty());
    }

    #[test]
    fn uuid_mismatch_blocks_and_rebase_does_not_help() {
        let doc = doc_with(|d| {
            d.power = Some(PowerSettings::default());
        });
        let plan = build_plan(
            &doc,
            &GpuIdentity::default(),
            GpuType::Desktop40Series,
            &PlanOptions { rebase: true },
        );
        // profile has UUID "GPU-A", live has none → RefuseNoUuid, rebase must
        // NOT override a missing card.
        assert!(!plan.is_runnable());
    }

    #[test]
    fn rebase_overrides_fingerprint_refusal() {
        let mut doc = doc_with(|d| {
            d.power = Some(PowerSettings::default());
        });
        doc.gpu.fingerprint.vbios_version = Some("old".to_owned());
        let mut live_identity = live();
        live_identity.fingerprint.vbios_version = Some("new".to_owned());

        let plan = build_plan(
            &doc,
            &live_identity,
            GpuType::Desktop40Series,
            &PlanOptions { rebase: false },
        );
        assert!(!plan.is_runnable());

        let plan = build_plan(
            &doc,
            &live_identity,
            GpuType::Desktop40Series,
            &PlanOptions { rebase: true },
        );
        assert!(plan.is_runnable());
        assert!(plan.warnings.iter().any(|w| w.contains("--rebase")));
    }

    #[test]
    fn describe_lists_every_verdict() {
        let doc = doc_with(|d| {
            d.power = Some(PowerSettings::default());
            d.public_vftable = Some(empty_vfp());
        });
        let plan = build_plan(
            &doc,
            &live(),
            GpuType::DesktopKepler,
            &PlanOptions::default(),
        );
        let text = plan.describe("plan-test");
        assert!(text.contains("apply:    power"));
        assert!(text.contains("skip:     public_vftable"));
        assert!(text.contains("identity: identity match"));
    }
}
