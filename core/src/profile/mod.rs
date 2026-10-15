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
//! - [`apply`] — execute → verify → rollback pipeline
//! - [`store`] — on-disk layout (one directory per GPU, atomic writes)
//! - [`package`] — portable single-file import/export with a rejection ladder

pub mod apply;
pub mod capture;
pub mod compat;
pub mod identity;
pub mod package;
pub mod plan;
pub mod schema;
pub mod store;

#[cfg(test)]
mod tests;

pub use apply::{
    ApplyOptions, ApplyReport, DomainOutcome, POWER_VERIFY_TOLERANCE_W, capture_backup_now,
    execute_profile, restore_backup,
};
pub use capture::{
    CaptureContext, capture_backup, capture_baseline, capture_domains, new_profile_doc,
};
pub use compat::domain_unsupported_reason;
pub use identity::{
    FingerprintDiff, IdentityCheck, capture_fingerprint, check_identity, compare_fingerprints,
    uuid_slug,
};
pub use package::{
    ImportPolicy, PackageImport, export_package, import_package, parse_package, read_package_file,
    write_package,
};
pub use plan::{ApplyPlan, DomainVerdict, PlanOptions, build_plan};
pub use schema::{
    BACKUP_KIND, BaselineBlock, DOMAIN_APPLY_ORDER, DomainKind, FanCurvePointRecord,
    FanCurveReadoutRecord, FanSettings, GpuFingerprint, GpuIdentity, LockedClocks,
    MAX_PACKAGE_PROFILES, PACKAGE_KIND, PROFILE_KIND, PackageRejection, PowerSettings,
    PowerWattsBounds, ProfileDoc, ProfileDomains, ProfilePackage, SCHEMA_VERSION, SettingsBackup,
    ValidationRecord, VoltageSettings, empty_gpu_settings,
};
pub use store::{ACTIVE_MARKER_FILE, BACKUP_LAST_FILE, ProfileStore, STOCK_FILE};
