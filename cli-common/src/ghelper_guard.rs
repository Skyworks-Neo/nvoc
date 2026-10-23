//! Neutralize G-Helper's persisted NVIDIA GPU OC before a standalone
//! nvoc-cli run touches the GPU.
//!
//! Rust twin of `tui/nvoc_tui/ghelper_guard.py` (mirrored into
//! `gui/src/ghelper_guard.py`), which guards the TUI/GUI startup paths; this
//! module covers CLI runs no Python app wraps. Keep the key set and
//! semantics in sync — the golden sample config in this crate's tests mirrors
//! the one in `tui/tests/test_ghelper_guard.py`.
//!
//! G-Helper (https://github.com/seerge/g-helper) keeps every GPU-side setting
//! in `%APPDATA%\GHelper\config.json` as one flat `<key>_<mode>` entry per
//! ASUS performance mode (0=Balanced, 1=Turbo, 2=Silent, plus custom modes up
//! to 19), and re-applies them at its own startup, performance-mode switch,
//! AC/DC transition and resume: core/memory offsets go out through NVAPI
//! `SetPerformanceStates20` (P0 delta) — the same clock plane NVOC drives —
//! the clock lock through `nvidia-smi -lgc`, power/temp/boost through
//! ACPI/NVML. Two tools writing one clock plane means last writer wins and
//! the user's NVOC profile silently decays.
//!
//! [`strip_oc_keys`] backs the config up to `config.json.bak.nvoc` (never
//! overwritten once it exists) and deletes every GPU OC key; a config that
//! fails to parse is left completely untouched — G-Helper has its own
//! recovery pass for broken configs, and a truncated file would destroy that
//! chance. [`run_guard`] reports the active mode (which mode the next event
//! would replay and what it carries), and after a successful strip freezes
//! the config read-only: G-Helper's writer (File.Replace/Move) fails on a
//! READONLY file and swallows the error, so the keys stay out even while
//! G-Helper runs. It also appends a standing tip (code-verified against the
//! G-Helper source): stock modes write no limits themselves, so the caps are
//! ASUS mode power budgets (ACPI) and stale driver-side `nvidia-smi -lgc`
//! locks — a G-Helper custom mode plus clearing stale locks is the
//! workaround. Unlike the Python twins (which always log one status line),
//! the CLI caller prints only actionable reports, keeping stdout/stderr clean
//! for the common no-G-Helper case. Key order inside the rewritten config is
//! normalized (serde maps), which G-Helper's lenient reader treats the same.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub const CONFIG_DIR_NAME: &str = "GHelper";
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const BACKUP_SUFFIX: &str = ".bak.nvoc";
pub const LOG_PREFIX: &str = "[GHelper]";

/// G-Helper's Modes.cs registers exactly these settings as per-mode GPU-side
/// writers: gpu_core/gpu_memory (NVAPI P0 delta offsets), gpu_clock_limit
/// (nvidia-smi -lgc lock), and gpu_power/gpu_temp/gpu_boost (ACPI/NVML).
/// `fan_profile_gpu` is deliberately absent: it drives the EC fan controller,
/// which NVOC does not touch (laptop fans are EC-owned; G-Helper has no GPU
/// fan surface on desktops). Non-mode keys (`gpu_mode`, `max_gpu_core`, ...)
/// are UI state, not writers, and are left alone.
pub const GPU_OC_KEYS: &[&str] = &[
    "gpu_core",
    "gpu_memory",
    "gpu_clock_limit",
    "gpu_power",
    "gpu_temp",
    "gpu_boost",
];

/// Modes.cs: performance_mode is 0/1/2 plus custom modes 3..maxModes-1 (20).
pub const MAX_PERF_MODE: u32 = 19;

const LOG_VALUE_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardStatus {
    Removed,
    Clean,
    NoConfig,
    Error,
    Unsupported,
}

#[derive(Debug)]
pub struct GuardReport {
    pub status: GuardStatus,
    pub lines: Vec<String>,
    pub removed: Map<String, Value>,
    pub backup_path: Option<PathBuf>,
    pub ghelper_running: Option<bool>,
}

impl GuardReport {
    fn new(status: GuardStatus) -> Self {
        Self {
            status,
            lines: Vec::new(),
            removed: Map::new(),
            backup_path: None,
            ghelper_running: None,
        }
    }
}

/// `%APPDATA%\GHelper\config.json`, or None when APPDATA is unset.
pub fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| {
        PathBuf::from(appdata)
            .join(CONFIG_DIR_NAME)
            .join(CONFIG_FILE_NAME)
    })
}

/// Split a `"<base>_<mode>"` key the same way the Python twin's regex does:
/// lowercase/underscore base, a one- or two-digit mode suffix, nothing else.
fn split_mode_key(key: &str) -> Option<(&str, u32)> {
    let (base, mode) = key.rsplit_once('_')?;
    if base.is_empty() || mode.is_empty() || mode.len() > 2 {
        return None;
    }
    if !mode.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((base, mode.parse().ok()?))
}

/// Pick the per-mode GPU OC keys (`<key>_<mode>`, mode <= 19) from a loaded
/// config object. Values pass through untouched — G-Helper stores ints and
/// strings alike and re-parses them on read.
pub fn find_oc_keys(data: &Map<String, Value>) -> Map<String, Value> {
    let mut found = Map::new();
    for (key, value) in data {
        if let Some((base, mode)) = split_mode_key(key)
            && mode <= MAX_PERF_MODE
            && GPU_OC_KEYS.contains(&base)
        {
            found.insert(key.clone(), value.clone());
        }
    }
    found
}

/// One line naming the mode G-Helper's next event would replay, and the GPU
/// OC keys that mode currently carries — the guard reports it so the user
/// knows which mode the interference actually applies to. Mirrors the Python
/// twin's `active_mode_summary`.
pub fn active_mode_summary(data: &Map<String, Value>) -> String {
    let mode = match data.get("performance_mode") {
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok())),
        None => None,
    };
    let Some(mode) = mode.filter(|m| *m >= 0) else {
        return "active mode: unknown (no usable performance_mode key)".to_string();
    };
    let name = data
        .get(format!("mode_name_{mode}").as_str())
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match mode {
            0 => "Balanced".to_string(),
            1 => "Turbo".to_string(),
            2 => "Silent".to_string(),
            _ => "Custom".to_string(),
        });
    let keys: Vec<(String, Value)> = find_oc_keys(data)
        .into_iter()
        .filter(|(key, _)| key.ends_with(&format!("_{mode}")))
        .collect();
    if keys.is_empty() {
        return format!("active mode: '{name}' ({mode}) — 0 GPU OC key(s) (replay is a no-op)");
    }
    let listing = keys
        .iter()
        .map(|(key, value)| format!("{key}={}", serde_json::to_string(value).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "active mode: '{name}' ({mode}) — {} GPU OC key(s) live here: {listing}",
        keys.len()
    )
}

/// Back up `config_path`, then rewrite it without its GPU OC keys.
///
/// The backup is a byte-for-byte copy created only when a modification is
/// about to happen; an existing backup is never overwritten.
pub fn strip_oc_keys(config_path: &Path, backup_path: Option<&Path>) -> GuardReport {
    let backup_path = match backup_path {
        Some(path) => path.to_path_buf(),
        None => match config_path.file_name() {
            Some(name) => {
                let mut name = name.to_os_string();
                name.push(BACKUP_SUFFIX);
                config_path.with_file_name(name)
            }
            None => return error_report("unusable config path", Vec::new(), None),
        },
    };

    if !config_path.is_file() {
        return GuardReport::new(GuardStatus::NoConfig);
    }

    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(err) => return parse_error_report(&err),
    };
    let mut data = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            return parse_error_report(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "config root is not a JSON object",
            ));
        }
        Err(err) => return parse_error_report(&err),
    };

    let removed = find_oc_keys(&data);
    // Snapshot the pre-strip view: the active-mode line names the mode the
    // next event would replay and what it carried.
    let active = active_mode_summary(&data);
    if removed.is_empty() {
        let mut report = GuardReport::new(GuardStatus::Clean);
        report.lines = vec![
            format!("{LOG_PREFIX} G-Helper config carries no GPU OC keys — nothing stripped"),
            format!("{LOG_PREFIX} {active}"),
        ];
        return report;
    }
    for key in removed.keys() {
        data.remove(key);
    }

    // Sorted like the Python twin's echo line, independent of the map's
    // own iteration order.
    let mut keys: Vec<&String> = removed.keys().collect();
    keys.sort();
    let mut entries = keys
        .into_iter()
        .map(|key| {
            let value = serde_json::to_string(&removed[key]).unwrap_or_default();
            format!("{key}={value}")
        })
        .collect::<Vec<_>>();
    let extra = entries.len().saturating_sub(LOG_VALUE_LIMIT);
    entries.truncate(LOG_VALUE_LIMIT);
    if extra > 0 {
        entries.push(format!("... (+{extra} more, see backup)"));
    }
    let mut lines = vec![
        format!(
            "{LOG_PREFIX} removed {} GPU OC key(s) from {}:",
            removed.len(),
            config_path.display()
        ),
        format!("{LOG_PREFIX}   {}", entries.join(", ")),
        format!("{LOG_PREFIX} {active}"),
    ];

    if backup_path.exists() {
        lines.push(format!(
            "{LOG_PREFIX} backup kept (exists): {}",
            backup_path.display()
        ));
    } else if let Err(err) = std::fs::copy(config_path, &backup_path) {
        // Without a backup the strip is destructive with no undo — abort.
        lines.push(format!(
            "{LOG_PREFIX} backup failed ({err}) — file left untouched"
        ));
        return GuardReport {
            status: GuardStatus::Error,
            lines,
            removed,
            backup_path: Some(backup_path),
            ghelper_running: None,
        };
    } else {
        lines.push(format!(
            "{LOG_PREFIX} backup written: {}",
            backup_path.display()
        ));
    }

    let tmp_path = {
        let mut name = config_path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".nvoc-tmp");
        config_path.with_file_name(name)
    };
    let serialized = match serde_json::to_string_pretty(&Value::Object(data)) {
        Ok(serialized) => serialized,
        Err(err) => {
            lines.push(format!(
                "{LOG_PREFIX} strip failed ({err}) — config unchanged"
            ));
            return GuardReport {
                status: GuardStatus::Error,
                lines,
                removed,
                backup_path: Some(backup_path),
                ghelper_running: None,
            };
        }
    };
    // A user-frozen (READONLY) config: clear the attribute just long enough
    // to write; run_guard re-freezes on success. Windows-only in practice —
    // run_guard never reaches here on Unix, where the READONLY attribute
    // does not exist (the allow covers the PermissionsExt semantics lint).
    let mut unfroze = false;
    let write_result = {
        let result = (|| -> std::io::Result<()> {
            if let Ok(meta) = std::fs::metadata(config_path) {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                if perms.readonly() {
                    perms.set_readonly(false);
                    std::fs::set_permissions(config_path, perms)?;
                    unfroze = true;
                }
            }
            std::fs::write(&tmp_path, serialized)?;
            std::fs::rename(&tmp_path, config_path)
        })();
        if result.is_err()
            && unfroze
            && let Ok(meta) = std::fs::metadata(config_path)
        {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(config_path, perms);
        }
        result
    };
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        lines.push(format!(
            "{LOG_PREFIX} strip failed ({err}) — config may be unchanged"
        ));
        return GuardReport {
            status: GuardStatus::Error,
            lines,
            removed,
            backup_path: Some(backup_path),
            ghelper_running: None,
        };
    }

    GuardReport {
        status: GuardStatus::Removed,
        lines,
        removed,
        backup_path: Some(backup_path),
        ghelper_running: None,
    }
}

fn parse_error_report(err: impl std::fmt::Display) -> GuardReport {
    GuardReport {
        status: GuardStatus::Error,
        lines: vec![format!(
            "{LOG_PREFIX} config parse failed ({err}) — file left untouched"
        )],
        removed: Map::new(),
        backup_path: None,
        ghelper_running: None,
    }
}

fn error_report(message: &str, lines: Vec<String>, backup_path: Option<PathBuf>) -> GuardReport {
    let mut lines = lines;
    lines.push(format!("{LOG_PREFIX} {message}"));
    GuardReport {
        status: GuardStatus::Error,
        lines,
        removed: Map::new(),
        backup_path,
        ghelper_running: None,
    }
}

/// Whether GHelper.exe is running (Windows); None when detection fails.
///
/// Reads the Toolhelp32 snapshot directly — no extra dependency beyond the
/// window-sys sub-namespace, no child process in the startup path.
#[cfg(windows)]
pub fn ghelper_running() -> Option<bool> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: plain Win32 snapshot walk; the entry struct is zeroed to the
    // API contract (dwSize first), and the handle is closed on every exit.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        // HANDLE is a pointer in windows-sys; INVALID_HANDLE_VALUE is all
        // bits set — compare bit patterns so the exact const form does not
        // matter across windows-sys versions.
        if (snapshot as usize) == usize::MAX {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("ghelper.exe")
            {
                found = true;
                break;
            }
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        Some(found)
    }
}

#[cfg(not(windows))]
pub fn ghelper_running() -> Option<bool> {
    None
}

/// One guard pass for a CLI invocation: backup + strip + live-process flag.
///
/// `running` comes from [`ghelper_running`] so callers (and tests) control
/// when the process snapshot is taken. Windows-only: anywhere else it
/// reports [`GuardStatus::Unsupported`] without touching anything. Never
/// panics on user-supplied config state.
pub fn run_guard(config_path: Option<&Path>, running: Option<bool>) -> GuardReport {
    if !cfg!(windows) {
        return GuardReport::new(GuardStatus::Unsupported);
    }

    let Some(path) = config_path
        .map(Path::to_path_buf)
        .or_else(default_config_path)
    else {
        return error_report(
            "APPDATA is not set — cannot locate G-Helper config",
            Vec::new(),
            None,
        );
    };

    let mut report = strip_oc_keys(&path, None);
    report.ghelper_running = running;

    if report.status == GuardStatus::NoConfig {
        report.lines = vec![format!(
            "{LOG_PREFIX} no G-Helper config at {} — nothing to do",
            path.display()
        )];
    }

    // Freeze the stripped config: G-Helper's debounced writer (WriteAtomic:
    // File.Replace / File.Move) fails on a READONLY file and it swallows the
    // error, so the stripped keys stay out even while G-Helper runs.
    // Empirically harmless to G-Helper itself (everything keeps working
    // in-session; only settings persistence stops until the attribute is
    // cleared). Only the Removed path freezes — a user who cleared the
    // attribute to edit settings again is not re-frozen for no reason.
    if report.status == GuardStatus::Removed {
        let frozen = match std::fs::metadata(&path) {
            Ok(meta) => {
                let mut perms = meta.permissions();
                let was_readonly = perms.readonly();
                perms.set_readonly(true);
                std::fs::set_permissions(&path, perms).map(|()| was_readonly)
            }
            Err(err) => Err(err),
        };
        match frozen {
            Ok(false) => report.lines.push(format!(
                "{LOG_PREFIX} config frozen read-only — G-Helper cannot write the stripped \
                 keys back (clear the read-only attribute to edit G-Helper settings again)"
            )),
            Ok(true) => {}
            Err(err) => report.lines.push(format!(
                "{LOG_PREFIX} could not freeze config read-only ({err})"
            )),
        }
    }

    if running == Some(true) {
        report.lines.push(format!(
            "{LOG_PREFIX} WARNING: GHelper.exe is running — it holds its config in memory, \
             re-applies stored OC on its own events and can rewrite the file. Exit G-Helper \
             (tray icon > Exit) before relying on nvoc-cli writes."
        ));
    }
    if matches!(
        report.status,
        GuardStatus::Removed | GuardStatus::Clean | GuardStatus::Error
    ) {
        report.lines.extend(mode_tip_lines());
    }
    report
}

/// Code-verified mechanism: stock modes write NO limits themselves (no
/// per-mode key -> early return / range-guard miss). What caps frequencies:
/// ASUS mode-switch power budgets (ACPI/firmware) and any clock lock
/// G-Helper ever set via `nvidia-smi -lgc`, which persists in the driver
/// until `-rgc` — its slider max is the const 3000 "Default" sentinel.
fn mode_tip_lines() -> Vec<String> {
    vec![
        format!(
            "{LOG_PREFIX} TIP: G-Helper writes no limits on stock modes itself — caps come \
             from ASUS mode power budgets (ACPI) and any clock lock G-Helper ever set, which \
             persists in the driver (nvidia-smi -lgc until -rgc)."
        ),
        format!(
            "{LOG_PREFIX} TIP: prefer a custom G-Helper mode, clear stale clock locks \
            (slider to far right = Default, or `nvidia-smi -rgc`), then let NVOC own \
            the GPU clocks."
        ),
    ]
}
