//! Portable single-file package (`*.nvocprofile.json`): the import/export
//! interchange format. The live store is a directory tree; the package is
//! what crosses machines. Rejection ladder on import mirrors xOCD: unknown
//! kind → unsupported version → empty → too many → aggregated per-slot
//! errors.

use super::schema::{PACKAGE_KIND, PackageRejection, ProfileDoc, ProfilePackage};
use crate::Error;
use std::path::Path;

/// Assemble a package from slot documents.
pub fn export_package(
    profiles: Vec<ProfileDoc>,
    source_host: Option<String>,
    purpose: Option<String>,
    now_utc: String,
) -> Result<ProfilePackage, Error> {
    let mut package = ProfilePackage::new(now_utc);
    package.source_host = source_host;
    package.purpose = purpose;
    package.profiles = profiles;
    package
        .validate()
        .map_err(|e| Error::Custom(e.to_string()))?;
    Ok(package)
}

/// Parse and validate package bytes. All rejections are structured; slot
/// errors are aggregated so one bad profile does not hide the others.
pub fn parse_package(bytes: &[u8]) -> Result<ProfilePackage, PackageRejection> {
    let package: ProfilePackage = match serde_json::from_slice(bytes) {
        Ok(package) => package,
        Err(e) => {
            // Distinguish the "not this format at all" case from a parse
            // failure inside an otherwise-plausible document.
            let is_wrong_kind = bytes
                .iter()
                .position(|&b| b == b'{')
                .and_then(|start| serde_json::from_slice::<serde_json::Value>(&bytes[start..]).ok())
                .and_then(|value| {
                    value
                        .get("kind")
                        .and_then(|k| k.as_str())
                        .map(str::to_owned)
                })
                .is_some_and(|kind| kind != PACKAGE_KIND);
            return Err(PackageRejection {
                code: if is_wrong_kind {
                    "bad-kind"
                } else {
                    "malformed"
                },
                message: if is_wrong_kind {
                    format!("not a {PACKAGE_KIND} document")
                } else {
                    format!("the profile package is empty or invalid: {e}")
                },
                slot_errors: Vec::new(),
            });
        }
    };
    package.validate()?;
    Ok(package)
}

/// Read a package from disk.
pub fn read_package_file(path: &Path) -> Result<ProfilePackage, Error> {
    let bytes = std::fs::read(path)?;
    parse_package(&bytes).map_err(|e| Error::Custom(e.to_string()))
}

/// Write a package to disk (atomic replace when the target exists).
pub fn write_package(path: &Path, package: &ProfilePackage) -> Result<(), Error> {
    super::store::write_atomic(path, &serde_json::to_vec_pretty(package)?)?;
    Ok(())
}

/// Result of importing a package into the store: what was written, skipped
/// (same name, identical content) and refused (name conflict with different
/// content without an overwrite policy).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageImport {
    pub imported: Vec<String>,
    pub skipped_identical: Vec<String>,
    pub conflicts: Vec<String>,
}

impl PackageImport {
    pub fn is_empty(&self) -> bool {
        self.imported.is_empty() && self.skipped_identical.is_empty() && self.conflicts.is_empty()
    }
}

/// Conflict policy for [`import_package`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportPolicy {
    /// Name conflict → record in `PackageImport::conflicts`, write nothing.
    #[default]
    Skip,
    /// Name conflict → replace the stored slot.
    Overwrite,
    /// Name conflict → store under a suffixed name (`name-imported`,
    /// `name-imported-2`, …).
    Rename,
}

/// Import a validated package's profiles into the store, bound to their own
/// captured GPU identities (an import never re-binds a profile to a different
/// card — apply-time checks still guard the hardware).
pub fn import_package(
    store: &super::store::ProfileStore,
    package: &ProfilePackage,
    policy: ImportPolicy,
) -> Result<PackageImport, Error> {
    let mut result = PackageImport::default();
    for doc in &package.profiles {
        doc.validate()
            .map_err(|e| Error::Custom(format!("profile {:?} is invalid: {e}", doc.name)))?;
        let identity = &doc.gpu;
        if store.exists(identity, &doc.name) {
            let existing = store.load_profile(identity, &doc.name)?;
            if existing == *doc {
                result.skipped_identical.push(doc.name.clone());
                continue;
            }
            match policy {
                ImportPolicy::Skip => {
                    result.conflicts.push(doc.name.clone());
                    continue;
                }
                ImportPolicy::Overwrite => {
                    store.save_profile(identity, doc)?;
                    result.imported.push(doc.name.clone());
                }
                ImportPolicy::Rename => {
                    let mut renamed = doc.clone();
                    renamed.name = unique_imported_name(store, identity, &doc.name);
                    renamed.updated_utc = super::store::now_utc()?;
                    store.save_profile(identity, &renamed)?;
                    result.imported.push(renamed.name.clone());
                }
            }
        } else {
            store.save_profile(identity, doc)?;
            result.imported.push(doc.name.clone());
        }
    }
    Ok(result)
}

fn unique_imported_name(
    store: &super::store::ProfileStore,
    identity: &super::schema::GpuIdentity,
    base: &str,
) -> String {
    let first = format!("{base}-imported");
    if !store.exists(identity, &first) {
        return first;
    }
    for suffix in 2.. {
        let candidate = format!("{base}-imported-{suffix}");
        if !store.exists(identity, &candidate) {
            return candidate;
        }
    }
    unreachable!("suffix loop always returns")
}

#[cfg(test)]
mod tests {
    use super::super::schema::{GpuIdentity, PowerSettings, ProfileDoc};
    use super::*;
    use crate::profile::schema::SCHEMA_VERSION;

    fn doc_named(name: &str, watts: u32) -> ProfileDoc {
        let identity = GpuIdentity {
            uuid: Some("GPU-import-test".to_owned()),
            ..Default::default()
        };
        let mut doc = ProfileDoc::new(name, identity, "t".to_owned());
        doc.domains.power = Some(PowerSettings {
            watts: Some(watts),
            percent: None,
        });
        doc
    }

    fn package_with(docs: Vec<ProfileDoc>) -> ProfilePackage {
        let mut package = ProfilePackage::new("t".to_owned());
        package.profiles = docs;
        package
    }

    #[test]
    fn export_and_parse_roundtrip() {
        let package = export_package(
            vec![doc_named("a", 170)],
            Some("host".to_owned()),
            None,
            "t".to_owned(),
        )
        .unwrap();
        let bytes = serde_json::to_vec(&package).unwrap();
        let parsed = parse_package(&bytes).unwrap();
        assert_eq!(parsed, package);
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn malformed_bytes_rejected_with_code() {
        let rejection = parse_package(b"not json at all").unwrap_err();
        assert_eq!(rejection.code, "malformed");
        assert!(rejection.message.contains("empty or invalid"));
    }

    #[test]
    fn wrong_kind_detected() {
        let json = br#"{"kind": "something-else"}"#;
        let rejection = parse_package(json).unwrap_err();
        assert_eq!(rejection.code, "bad-kind");
    }

    #[test]
    fn import_skip_policy_reports_conflicts() {
        let store = super::super::store::ProfileStore::new(std::env::temp_dir().join(format!(
                "nvoc-package-tests/{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )));
        let package = package_with(vec![doc_named("p", 170), doc_named("q", 180)]);
        let first = import_package(&store, &package, ImportPolicy::Skip).unwrap();
        assert_eq!(first.imported.len(), 2);

        let changed = package_with(vec![doc_named("p", 200)]);
        let second = import_package(&store, &changed, ImportPolicy::Skip).unwrap();
        assert!(second.imported.is_empty());
        assert_eq!(second.conflicts, vec!["p".to_owned()]);

        let third = import_package(&store, &changed, ImportPolicy::Overwrite).unwrap();
        assert_eq!(third.imported, vec!["p".to_owned()]);
        let loaded = store.load_profile(&package.profiles[0].gpu, "p").unwrap();
        assert_eq!(loaded.domains.power.unwrap().watts, Some(200));
    }

    #[test]
    fn import_rename_policy_uniquifies() {
        let store = super::super::store::ProfileStore::new(std::env::temp_dir().join(format!(
                "nvoc-package-tests/{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )));
        let identity = doc_named("p", 170).gpu;
        let original = package_with(vec![doc_named("p", 170)]);
        import_package(&store, &original, ImportPolicy::Skip).unwrap();

        let changed = package_with(vec![doc_named("p", 220)]);
        let result = import_package(&store, &changed, ImportPolicy::Rename).unwrap();
        assert_eq!(result.imported, vec!["p-imported".to_owned()]);
        assert!(store.exists(&identity, "p-imported"));

        let again = import_package(&store, &changed, ImportPolicy::Rename).unwrap();
        assert_eq!(again.imported, vec!["p-imported-2".to_owned()]);
    }
}
