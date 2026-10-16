//! Cross-module integration tests for the profile module: full
//! save → list → load → package-export → package-import → plan round trips
//! over a temp store. Hardware-dependent apply paths live behind
//! `core/tests/profile_gpu.rs` (`#[ignore]`, gpu-write tier).

use super::schema::{
    DomainKind, GpuIdentity, PowerSettings, ProfileDoc, ProfilePackage, SettingsBackup,
};
use super::store::ProfileStore;
use crate::gpu_type::GpuType;
use std::collections::BTreeMap;

fn unique_store(tag: &str) -> ProfileStore {
    ProfileStore::new(std::env::temp_dir().join(format!(
        "nvoc-profile-integration-tests/{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )))
}

fn identity() -> GpuIdentity {
    GpuIdentity {
        uuid: Some("GPU-integration-test".to_owned()),
        fingerprint: super::schema::GpuFingerprint {
            vbios_version: Some("94.06".to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn captured_doc(name: &str, watts: u32) -> ProfileDoc {
    let mut doc = ProfileDoc::new(name, identity(), "2026-10-15T00:00:00Z".to_owned());
    doc.domains.power = Some(PowerSettings {
        watts: Some(watts),
        percent: None,
    });
    doc
}

#[test]
fn save_list_package_import_roundtrip() {
    let store = unique_store("roundtrip");
    let gpu = identity();

    store
        .save_profile(&gpu, &captured_doc("silent", 170))
        .unwrap();
    store
        .save_profile(&gpu, &captured_doc("benchmark", 220))
        .unwrap();

    let listed = store.list_profiles(&gpu).unwrap();
    assert_eq!(listed.len(), 2);
    let names: Vec<&str> = listed
        .iter()
        .map(|r| r.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(names, vec!["benchmark", "silent"]);

    // export both slots into one package
    let docs: Vec<ProfileDoc> = listed.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
    let package = super::package::export_package(docs, None, None, "t".to_owned()).unwrap();
    let bytes = serde_json::to_vec_pretty(&package).unwrap();

    // import into a fresh store for the same identity
    let other_store = unique_store("roundtrip-import");
    let parsed = super::package::parse_package(&bytes).unwrap();
    let import =
        super::package::import_package(&other_store, &parsed, super::package::ImportPolicy::Skip)
            .unwrap();
    assert_eq!(import.imported.len(), 2);
    assert_eq!(
        other_store
            .load_profile(&gpu, "benchmark")
            .unwrap()
            .domains
            .power
            .unwrap()
            .watts,
        Some(220)
    );
}

#[test]
fn plan_refuses_foreign_gpu_and_allows_own() {
    let store = unique_store("plan");
    let gpu = identity();
    store.save_profile(&gpu, &captured_doc("p", 170)).unwrap();
    let doc = store.load_profile(&gpu, "p").unwrap();

    let own = identity();
    let plan = super::plan::build_plan(
        &doc,
        &own,
        GpuType::Desktop40Series,
        &super::plan::PlanOptions::default(),
    );
    assert!(plan.is_runnable());
    assert_eq!(plan.steps, vec![DomainKind::Power]);

    let mut foreign = identity();
    foreign.uuid = Some("GPU-other-card".to_owned());
    let plan = super::plan::build_plan(
        &doc,
        &foreign,
        GpuType::Desktop40Series,
        &super::plan::PlanOptions::default(),
    );
    assert!(!plan.is_runnable());
    assert!(plan.refusal().unwrap().contains("GPU-other-card"));
}

#[test]
fn backup_snapshot_validates_and_survives_json() {
    let mut backup = SettingsBackup::new(identity(), "t".to_owned());
    backup.settings = super::schema::empty_gpu_settings();
    backup.settings.pstate_deltas.insert(
        ::nvapi::hi::PState::P0,
        BTreeMap::from([(
            ::nvapi::hi::ClockDomain::Graphics,
            ::nvapi::hi::KilohertzDelta(0),
        )]),
    );
    backup.validate().unwrap();

    let json = serde_json::to_string(&backup).unwrap();
    let back: SettingsBackup = serde_json::from_str(&json).unwrap();
    assert_eq!(back, backup);
}

#[test]
fn captured_doc_from_capture_module_matches_schema() {
    let settings = super::schema::empty_gpu_settings();
    let context = super::capture::CaptureContext {
        identity: identity(),
        power: None,
        fan_curves: Vec::new(),
        volt_rails: Vec::new(),
    };
    let domains =
        super::capture::capture_domains(&settings, &context, &DomainKind::DEFAULT_CAPTURE);
    let doc = super::capture::new_profile_doc(
        "from-capture",
        None,
        &context,
        domains,
        Some(super::capture::capture_baseline(&settings, None)),
        "t".to_owned(),
    );
    // all-empty settings → nothing captured → invalid to store, and that is
    // the documented contract (empty capture would wipe planes on apply)
    assert!(doc.validate().is_err());
}

#[test]
fn package_kind_strings_are_stable() {
    let package = ProfilePackage::new("t".to_owned());
    let json = serde_json::to_string(&package).unwrap();
    assert!(json.contains("\"kind\":\"nvoc-profile-package\""));
    assert!(json.contains("\"schema_version\":1"));
}
