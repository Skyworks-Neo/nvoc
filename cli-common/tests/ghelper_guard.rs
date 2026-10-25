//! Golden-sample tests for the G-Helper config guard, mirroring the Python
//! twins (`tui/tests/test_ghelper_guard.py` == `gui/tests/…`): same sample
//! config, same strip/backup/failure-path semantics, so the two
//! implementations cannot drift silently.

use std::fs;
use std::path::{Path, PathBuf};

use nvoc_cli_common::ghelper_guard::{
    GuardStatus, active_mode_summary, find_oc_keys, run_guard, strip_oc_keys,
};
use serde_json::{Value, json};

// Mirrors a real G-Helper config: GPU-side writers are stored per performance
// mode ("<key>_<mode>"), everything else is UI state or another device plane.
fn sample_config() -> Value {
    json!({
        "performance_mode": 1,
        "mode_name_3": "Custom",
        "gpu_core_0": -50,
        "gpu_core_1": 200,
        "gpu_memory_1": "1500",
        "gpu_power_1": 115,
        "gpu_temp_2": 87,
        "gpu_boost_1": 15,
        "gpu_clock_limit_1": 2100,
        "fan_profile_gpu_1": "1E-3F-44-48",
        "gpu_mode": 1,
        "max_gpu_core": 400,
        "gpu_core_20": 999,
        "gpu_core": 5,
        "cpu_uv_1": "-80.5",
        "language": "zh-CN",
    })
}

fn sample_oc_keys() -> Value {
    json!({
        "gpu_core_0": -50,
        "gpu_core_1": 200,
        "gpu_memory_1": "1500",
        "gpu_power_1": 115,
        "gpu_temp_2": 87,
        "gpu_boost_1": 15,
        "gpu_clock_limit_1": 2100,
    })
}

fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nvoc-ghelper-guard-test-{}-{tag}",
        std::process::id()
    ))
}

fn write_config(path: &Path, payload: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string(payload).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn find_oc_keys_matches_per_mode_gpu_writers_only() {
    let sample = sample_config();
    let found = find_oc_keys(sample.as_object().unwrap());
    assert_eq!(
        found,
        *sample_oc_keys().as_object().unwrap(),
        "exactly the per-mode GPU writers must match; fan/MUX/slider-bound keys must not"
    );
}

#[test]
fn strip_removes_oc_keys_keeps_rest_and_writes_backup() {
    let root = temp_root("strip");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    let backup = root.join("GHelper").join("config.json.bak.nvoc");
    write_config(&config, &sample_config());

    let report = strip_oc_keys(&config, None);

    assert_eq!(report.status, GuardStatus::Removed);
    assert_eq!(report.backup_path.as_deref(), Some(backup.as_path()));
    assert_eq!(report.removed, *sample_oc_keys().as_object().unwrap());
    assert!(backup.is_file());
    assert_eq!(read_json(&backup), sample_config());

    let remaining = read_json(&config);
    for key in sample_oc_keys().as_object().unwrap().keys() {
        assert!(!remaining.as_object().unwrap().contains_key(key));
    }
    let mut expected = sample_config();
    for key in sample_oc_keys().as_object().unwrap().keys() {
        expected.as_object_mut().unwrap().remove(key).unwrap();
    }
    assert_eq!(remaining, expected);
    // Fans are EC-owned (nvoc never writes fan curves), so the GPU fan
    // profile must survive the strip by design.
    assert_eq!(remaining["fan_profile_gpu_1"], "1E-3F-44-48");

    let joined = report.lines.join("\n");
    assert!(joined.contains("gpu_core_1=200"));
    assert!(joined.contains("gpu_memory_1=\"1500\""));
    // Active-mode check: SAMPLE's current mode is 1 (Turbo) carrying 5 keys.
    assert!(joined.contains("active mode: 'Turbo' (1)"));
    assert!(joined.contains("5 GPU OC key(s) live here"));
    for line in &report.lines {
        assert!(line.starts_with("[GHelper]"));
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn active_mode_summary_names_stock_modes_and_unknown() {
    let stock = json!({"performance_mode": 2, "gpu_temp_2": 87, "gpu_core_1": 10});
    let summary = active_mode_summary(stock.as_object().unwrap());
    assert!(summary.contains("'Silent' (2)"), "{summary}");
    assert!(summary.contains("gpu_temp_2=87"), "{summary}");
    assert!(!summary.contains("gpu_core_1"), "{summary}");

    let custom = json!({"performance_mode": 5, "mode_name_5": "Ultimate Perf"});
    let summary = active_mode_summary(custom.as_object().unwrap());
    assert!(summary.contains("'Ultimate Perf' (5)"), "{summary}");
    assert!(summary.contains("0 GPU OC key(s)"), "{summary}");

    let bare = json!({"gpu_core_1": 5});
    assert!(active_mode_summary(bare.as_object().unwrap()).contains("unknown"));
    let negative = json!({"performance_mode": -1});
    assert!(active_mode_summary(negative.as_object().unwrap()).contains("unknown"));
}

#[test]
fn strip_never_overwrites_existing_backup() {
    let root = temp_root("backup-kept");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    let backup = root.join("GHelper").join("config.json.bak.nvoc");
    write_config(&config, &sample_config());
    write_config(&backup, &json!({"sentinel": 1}));

    let report = strip_oc_keys(&config, None);

    assert_eq!(report.status, GuardStatus::Removed);
    assert_eq!(read_json(&backup), json!({"sentinel": 1}));
    assert!(report.lines.iter().any(|line| line.contains("backup kept")));
    // With the first backup frozen, the echoed values are the only record of
    // what was stripped this round.
    assert!(report.lines.join("\n").contains("gpu_core_1=200"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn strip_leaves_broken_config_untouched() {
    let root = temp_root("broken");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let broken = r#"{"performance_mode": 1,,"#;
    fs::write(&config, broken).unwrap();

    let report = strip_oc_keys(&config, None);

    assert_eq!(report.status, GuardStatus::Error);
    assert!(report.backup_path.is_none());
    assert!(!root.join("GHelper").join("config.json.bak.nvoc").exists());
    assert_eq!(fs::read_to_string(&config).unwrap(), broken);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn strip_without_oc_keys_is_clean() {
    let root = temp_root("clean");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    let mut payload = sample_config();
    for key in sample_oc_keys().as_object().unwrap().keys() {
        payload.as_object_mut().unwrap().remove(key).unwrap();
    }
    write_config(&config, &payload);

    let report = strip_oc_keys(&config, None);

    assert_eq!(report.status, GuardStatus::Clean);
    assert!(report.removed.is_empty());
    assert!(report.backup_path.is_none());
    assert!(!root.join("GHelper").join("config.json.bak.nvoc").exists());
    assert_eq!(read_json(&config), payload);
    // Active-mode line: mode 1 (Turbo) is clean once the sample's mode-1 keys
    // are removed from the payload.
    let joined = report.lines.join(
        "
",
    );
    assert!(joined.contains("active mode: 'Turbo' (1)"), "{joined}");
    assert!(joined.contains("0 GPU OC key(s)"), "{joined}");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn strip_handles_user_frozen_config() {
    let root = temp_root("frozen");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    write_config(&config, &sample_config());
    let mut perms = fs::metadata(&config).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&config, perms).unwrap();

    let report = strip_oc_keys(&config, None);

    assert_eq!(report.status, GuardStatus::Removed);
    let remaining = read_json(&config);
    for key in sample_oc_keys().as_object().unwrap().keys() {
        assert!(!remaining.as_object().unwrap().contains_key(key));
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn strip_missing_config_reports_no_config() {
    let root = temp_root("missing");
    let _ = fs::remove_dir_all(&root);

    let report = strip_oc_keys(&root.join("GHelper").join("config.json"), None);

    assert_eq!(report.status, GuardStatus::NoConfig);
}

#[test]
fn run_guard_strips_explicit_path_and_flags_running_process() {
    let root = temp_root("run-guard");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    write_config(&config, &sample_config());

    let report = run_guard(Some(&config), Some(true));

    if cfg!(windows) {
        assert_eq!(report.status, GuardStatus::Removed);
        assert_eq!(report.ghelper_running, Some(true));
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("WARNING") && line.contains("GHelper.exe is running"))
        );
        let remaining = read_json(&config);
        for key in sample_oc_keys().as_object().unwrap().keys() {
            assert!(!remaining.as_object().unwrap().contains_key(key));
        }
        assert!(root.join("GHelper").join("config.json.bak.nvoc").is_file());
        // The stripped config is frozen so G-Helper cannot write keys back.
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("frozen read-only"))
        );
        assert!(fs::metadata(&config).unwrap().permissions().readonly());
        // Standing tip: stale -lgc locks and ASUS mode power budgets survive the
        // strip (stock modes write no limits themselves).
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("TIP") && line.contains("nvidia-smi -lgc"))
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("custom G-Helper mode"))
        );
    } else {
        assert_eq!(report.status, GuardStatus::Unsupported);
        assert!(report.lines.is_empty());
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn run_guard_without_running_process_has_no_warning() {
    let root = temp_root("run-guard-quiet");
    let _ = fs::remove_dir_all(&root);
    let config = root.join("GHelper").join("config.json");
    write_config(&config, &sample_config());

    let report = run_guard(Some(&config), Some(false));

    if cfg!(windows) {
        assert_eq!(report.status, GuardStatus::Removed);
        assert_eq!(report.ghelper_running, Some(false));
        assert!(!report.lines.iter().any(|line| line.contains("WARNING")));
    }

    let _ = fs::remove_dir_all(&root);
}
