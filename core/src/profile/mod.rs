//! Overclocking profile persistence: multi-slot save/load/import/export bound
//! to one physical GPU (NVML UUID primary key + hardware fingerprint second
//! factor).
//!
//! Design notes and the reverse-engineering references this module borrows
//! from (MSI Afterburner's per-slot INI + `ProfileContents` mask; xOCD's
//! versioned JSON package + readback-verified apply lifecycle; nvoc issue
//! #360) live in the module docs of each file:
//!
//! - [`schema`] — versioned JSON documents (slot / package / backup)
//! - [`identity`] — GPU binding and the apply-time verdict matrix
//! - [`capture`] — pure snapshot → document conversion
//! - [`plan`] / [`compat`] — pre-flight verdicts (nothing applies unplanned)
//! - [`store`] — on-disk layout (one directory per GPU, atomic writes)

pub mod capture;
pub mod compat;
pub mod identity;
pub mod plan;
pub mod schema;
pub mod store;

pub use capture::{
    capture_backup, capture_baseline, capture_domains, new_profile_doc, CaptureContext,
};
pub use compat::domain_unsupported_reason;
pub use identity::{
    capture_fingerprint, check_identity, compare_fingerprints, uuid_slug, FingerprintDiff,
    IdentityCheck,
};
pub use plan::{build_plan, ApplyPlan, DomainVerdict, PlanOptions};
pub use schema::{
    empty_gpu_settings, BaselineBlock, DomainKind, FanCurvePointRecord, FanCurveReadoutRecord,
    FanSettings, LockedClocks, PackageRejection, PowerSettings, PowerWattsBounds, ProfileDoc,
    ProfileDomains, ProfilePackage, SettingsBackup, ValidationRecord, VoltageSettings,
    BACKUP_KIND, DOMAIN_APPLY_ORDER, MAX_PACKAGE_PROFILES, PACKAGE_KIND, PROFILE_KIND,
    SCHEMA_VERSION,
};
pub use store::{ProfileStore, ACTIVE_MARKER_FILE, BACKUP_LAST_FILE, STOCK_FILE};
