//! On-disk profile store: one directory per GPU identity, one JSON file per
//! slot, plus the well-known `stock.json` / `backup-last.json` / `active`
//! markers. Directory listing is the source of truth (no separate index to
//! corrupt); every write is atomic (temp file + rename in the same
//! directory).

use super::identity::{name_slug, uuid_slug};
use super::schema::{PROFILE_KIND, SettingsBackup};
use super::schema::{ProfileDoc, SCHEMA_VERSION};
use crate::Error;
use std::fs;
use std::path::{Path, PathBuf};

pub const STOCK_FILE: &str = "stock.json";
pub const BACKUP_LAST_FILE: &str = "backup-last.json";
pub const ACTIVE_MARKER_FILE: &str = "active";

/// Root of the profile store. Resolve with [`ProfileStore::default_root`].
#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    /// Machine-level default: `%PROGRAMDATA%\nvoc\profiles` on Windows,
    /// `$XDG_CONFIG_HOME/nvoc/profiles` (or `/etc/nvoc/profiles`) on Linux —
    /// the same tier as the srv config path, shared by CLI/srv and readable
    /// by every frontend. Overridable per invocation (`--profile-root`).
    pub fn default_root() -> Result<PathBuf, Error> {
        if cfg!(windows) {
            let program_data = std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:".to_owned());
            Ok(Path::new(&program_data).join("nvoc").join("profiles"))
        } else {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
                && !xdg.is_empty()
            {
                return Ok(Path::new(&xdg).join("nvoc").join("profiles"));
            }
            Ok(Path::new("/etc/nvoc/profiles").to_owned())
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Per-GPU directory. Keyed by UUID slug when the identity has one,
    /// otherwise by a `fingerprint-` prefix so UUID-less environments still
    /// get a stable home.
    pub fn gpu_dir(&self, identity: &super::schema::GpuIdentity) -> PathBuf {
        let dir_name = match &identity.uuid {
            Some(uuid) => uuid_slug(uuid),
            None => format!("fingerprint-{}", uuid_slug(&identity_key(identity))),
        };
        self.root.join(dir_name)
    }

    fn slot_path(&self, identity: &super::schema::GpuIdentity, doc: &ProfileDoc) -> PathBuf {
        self.gpu_dir(identity)
            .join(format!("{}.json", name_slug(&doc.name)))
    }

    /// Atomically write `doc` as a slot under the identity's directory.
    /// Refuses to clobber a *different* profile that slug-collides with an
    /// existing file.
    pub fn save_profile(
        &self,
        identity: &super::schema::GpuIdentity,
        doc: &ProfileDoc,
    ) -> Result<PathBuf, Error> {
        doc.validate()
            .map_err(|e| Error::Custom(format!("profile {:?} is invalid: {e}", doc.name)))?;
        let dir = self.gpu_dir(identity);
        fs::create_dir_all(&dir)?;
        let path = self.slot_path(identity, doc);
        if path.exists() {
            let existing = self.read_doc(&path)?;
            if existing.name != doc.name {
                return Err(Error::Custom(format!(
                    "profile name {:?} collides with existing file {} owned by profile {:?}",
                    doc.name,
                    path.display(),
                    existing.name
                )));
            }
        }
        write_atomic(&path, &serde_json::to_vec_pretty(doc)?)?;
        Ok(path)
    }

    pub fn load_profile(
        &self,
        identity: &super::schema::GpuIdentity,
        name: &str,
    ) -> Result<ProfileDoc, Error> {
        let dir = self.gpu_dir(identity);
        let path = dir.join(format!("{}.json", name_slug(name)));
        let doc = self.read_doc(&path)?;
        doc.validate()
            .map_err(|e| Error::Custom(format!("profile at {} is invalid: {e}", path.display())))?;
        Ok(doc)
    }

    /// List all slots for the identity, sorted by name. Damaged files are
    /// reported as errors alongside the good ones instead of aborting the
    /// listing.
    pub fn list_profiles(
        &self,
        identity: &super::schema::GpuIdentity,
    ) -> Result<Vec<Result<ProfileDoc, String>>, Error> {
        let dir = self.gpu_dir(identity);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match self.read_doc(&path) {
                Ok(doc) if doc.kind == PROFILE_KIND && doc.schema_version == SCHEMA_VERSION => {
                    out.push(Ok(doc));
                }
                Ok(doc) => out.push(Err(format!(
                    "{}: not a {PROFILE_KIND} v{SCHEMA_VERSION} document (kind={:?}, v{})",
                    path.display(),
                    doc.kind,
                    doc.schema_version
                ))),
                Err(e) => out.push(Err(format!("{}: {e}", path.display()))),
            }
        }
        out.sort_by_key(|item| match item {
            Ok(doc) => doc.name.to_ascii_lowercase(),
            Err(msg) => msg.to_ascii_lowercase(),
        });
        Ok(out)
    }

    pub fn delete_profile(
        &self,
        identity: &super::schema::GpuIdentity,
        name: &str,
    ) -> Result<(), Error> {
        let doc = self.load_profile(identity, name)?;
        let path = self.slot_path(identity, &doc);
        fs::remove_file(&path)?;
        if self.active_name(identity)?.as_deref() == Some(doc.name.as_str()) {
            self.set_active(identity, None)?;
        }
        Ok(())
    }

    pub fn rename_profile(
        &self,
        identity: &super::schema::GpuIdentity,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), Error> {
        let mut doc = self.load_profile(identity, old_name)?;
        if name_slug(new_name) != name_slug(&doc.name) && self.exists(identity, new_name) {
            return Err(Error::Custom(format!(
                "profile {new_name:?} already exists"
            )));
        }
        let old_path = self.slot_path(identity, &doc);
        let was_active = self.active_name(identity)?.as_deref() == Some(doc.name.as_str());
        doc.name = new_name.to_owned();
        doc.updated_utc = now_utc()?;
        let new_path = self.slot_path(identity, &doc);
        write_atomic(&new_path, &serde_json::to_vec_pretty(&doc)?)?;
        if old_path != new_path {
            fs::remove_file(&old_path)?;
        }
        if was_active {
            self.set_active(identity, Some(new_name))?;
        }
        Ok(())
    }

    pub fn exists(&self, identity: &super::schema::GpuIdentity, name: &str) -> bool {
        self.gpu_dir(identity)
            .join(format!("{}.json", name_slug(name)))
            .exists()
    }

    /// Save the factory/current-state snapshot (`stock.json`).
    pub fn save_stock(
        &self,
        identity: &super::schema::GpuIdentity,
        backup: &SettingsBackup,
    ) -> Result<PathBuf, Error> {
        backup
            .validate()
            .map_err(|e| Error::Custom(format!("stock snapshot invalid: {e}")))?;
        let dir = self.gpu_dir(identity);
        fs::create_dir_all(&dir)?;
        let path = dir.join(STOCK_FILE);
        write_atomic(&path, &serde_json::to_vec_pretty(backup)?)?;
        Ok(path)
    }

    pub fn load_stock(
        &self,
        identity: &super::schema::GpuIdentity,
    ) -> Result<Option<SettingsBackup>, Error> {
        self.load_backup_file(identity, STOCK_FILE)
    }

    /// Save the pre-apply snapshot (`backup-last.json`).
    pub fn save_backup_last(
        &self,
        identity: &super::schema::GpuIdentity,
        backup: &SettingsBackup,
    ) -> Result<PathBuf, Error> {
        backup
            .validate()
            .map_err(|e| Error::Custom(format!("backup snapshot invalid: {e}")))?;
        let dir = self.gpu_dir(identity);
        fs::create_dir_all(&dir)?;
        let path = dir.join(BACKUP_LAST_FILE);
        write_atomic(&path, &serde_json::to_vec_pretty(backup)?)?;
        Ok(path)
    }

    pub fn load_backup_last(
        &self,
        identity: &super::schema::GpuIdentity,
    ) -> Result<Option<SettingsBackup>, Error> {
        self.load_backup_file(identity, BACKUP_LAST_FILE)
    }

    fn load_backup_file(
        &self,
        identity: &super::schema::GpuIdentity,
        file: &str,
    ) -> Result<Option<SettingsBackup>, Error> {
        let path = self.gpu_dir(identity).join(file);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let backup: SettingsBackup = serde_json::from_slice(&bytes).map_err(|e| {
            Error::Custom(format!("{} is not a valid snapshot: {e}", path.display()))
        })?;
        backup
            .validate()
            .map_err(|e| Error::Custom(format!("{} is invalid: {e}", path.display())))?;
        Ok(Some(backup))
    }

    /// Mark `name` as the active slot (`None` clears the marker).
    pub fn set_active(
        &self,
        identity: &super::schema::GpuIdentity,
        name: Option<&str>,
    ) -> Result<(), Error> {
        let dir = self.gpu_dir(identity);
        fs::create_dir_all(&dir)?;
        let path = dir.join(ACTIVE_MARKER_FILE);
        match name {
            Some(name) => write_atomic(&path, name.as_bytes())?,
            None => {
                if path.exists() {
                    fs::remove_file(&path)?;
                }
            }
        }
        Ok(())
    }

    pub fn active_name(
        &self,
        identity: &super::schema::GpuIdentity,
    ) -> Result<Option<String>, Error> {
        let path = self.gpu_dir(identity).join(ACTIVE_MARKER_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_owned()))
    }

    fn read_doc(&self, path: &Path) -> Result<ProfileDoc, Error> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Custom(format!("{} is not a valid profile: {e}", path.display())))
    }
}

/// Stable string for a UUID-less identity (fingerprint fields that exist).
fn identity_key(identity: &super::schema::GpuIdentity) -> String {
    let fp = &identity.fingerprint;
    [
        fp.device_id.map(|v| format!("{v:x}")),
        fp.subsystem_id.map(|v| format!("{v:x}")),
        fp.vbios_version.clone(),
        fp.model_name.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("|")
}

/// Atomic file write: write `<name>.tmp-<pid>-<nanos>` then rename over the
/// target. A crash mid-write leaves the previous file intact.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Custom(format!("{} has no parent directory", path.display())))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{nanos}",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_owned()),
        std::process::id()
    ));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// RFC3339 UTC timestamp for `created_utc`/`updated_utc` fields.
pub(crate) fn now_utc() -> Result<String, Error> {
    let now = time::OffsetDateTime::now_utc();
    let formatted = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| Error::Custom(format!("timestamp format failed: {e}")))?;
    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::super::schema::{GpuIdentity, PowerSettings, ProfileDoc, ProfileDomains};
    use super::*;

    fn doc_named(name: &str) -> ProfileDoc {
        let mut doc = ProfileDoc::new(
            name,
            GpuIdentity::default(),
            "2026-10-15T00:00:00Z".to_owned(),
        );
        doc.domains.power = Some(PowerSettings {
            watts: Some(170),
            percent: None,
        });
        doc
    }

    fn ident() -> GpuIdentity {
        GpuIdentity {
            uuid: Some("GPU-test-uuid".to_owned()),
            ..Default::default()
        }
    }

    fn tmp_store(tag: &str) -> ProfileStore {
        let dir = std::env::temp_dir().join(format!(
            "nvoc-profile-store-tests/{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ProfileStore::new(dir)
    }

    #[test]
    fn save_load_roundtrip() {
        let store = tmp_store("roundtrip");
        let identity = ident();
        store
            .save_profile(&identity, &doc_named("Silent Mode"))
            .unwrap();
        let loaded = store.load_profile(&identity, "silent-mode").unwrap();
        assert_eq!(loaded.name, "Silent Mode");

        let listed = store.list_profiles(&identity).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_ok());
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let store = tmp_store("empty");
        assert!(store.list_profiles(&ident()).unwrap().is_empty());
    }

    #[test]
    fn damaged_slot_is_reported_not_fatal() {
        let store = tmp_store("damaged");
        let identity = ident();
        store.save_profile(&identity, &doc_named("good")).unwrap();
        let dir = store.gpu_dir(&identity);
        std::fs::write(dir.join("zz-broken.json"), b"{ not json").unwrap();

        let listed = store.list_profiles(&identity).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(listed.iter().filter(|r| r.is_err()).count(), 1);
    }

    #[test]
    fn delete_and_active_marker() {
        let store = tmp_store("delete");
        let identity = ident();
        store.save_profile(&identity, &doc_named("one")).unwrap();
        store.save_profile(&identity, &doc_named("two")).unwrap();
        store.set_active(&identity, Some("one")).unwrap();
        assert_eq!(
            store.active_name(&identity).unwrap(),
            Some("one".to_owned())
        );

        store.delete_profile(&identity, "one").unwrap();
        assert_eq!(store.active_name(&identity).unwrap(), None);
        assert!(!store.exists(&identity, "one"));
    }

    #[test]
    fn rename_moves_file_and_active_marker() {
        let store = tmp_store("rename");
        let identity = ident();
        store.save_profile(&identity, &doc_named("old")).unwrap();
        store.set_active(&identity, Some("old")).unwrap();

        store.rename_profile(&identity, "old", "new").unwrap();
        assert!(store.exists(&identity, "new"));
        assert!(!store.exists(&identity, "old"));
        assert_eq!(
            store.active_name(&identity).unwrap(),
            Some("new".to_owned())
        );
    }

    #[test]
    fn stock_and_backup_last_files() {
        let store = tmp_store("backups");
        let identity = ident();
        assert!(store.load_stock(&identity).unwrap().is_none());

        let mut backup = SettingsBackup::new(identity.clone(), "t".to_owned());
        backup.settings = super::super::schema::empty_gpu_settings();
        store.save_stock(&identity, &backup).unwrap();
        assert!(store.load_stock(&identity).unwrap().is_some());

        store.save_backup_last(&identity, &backup).unwrap();
        assert!(store.load_backup_last(&identity).unwrap().is_some());
    }

    #[test]
    fn default_root_exists_on_both_platforms() {
        let _ = ProfileStore::default_root().unwrap();
    }

    #[test]
    fn domains_present_uses_apply_order() {
        let domains = ProfileDomains {
            fan: Some(Default::default()),
            clock_offsets: Some(Default::default()),
            ..Default::default()
        };
        let doc = ProfileDoc {
            schema_version: SCHEMA_VERSION,
            kind: PROFILE_KIND.to_owned(),
            name: "x".to_owned(),
            created_utc: "t".to_owned(),
            updated_utc: "t".to_owned(),
            note: None,
            gpu: GpuIdentity::default(),
            domains,
            baseline: None,
            validation: None,
        };
        assert_eq!(
            doc.domains.present(),
            vec![
                super::super::schema::DomainKind::ClockOffsets,
                super::super::schema::DomainKind::Fan
            ]
        );
    }
}
