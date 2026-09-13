//! Logging setup: rotating file sink via `log2`; `tee` echoes to the console
//! in foreground mode. The Windows service additionally redirects
//! stdout/stderr into the same file (no console attached) — see
//! [`crate::service`].

use log::LevelFilter;
use std::path::{Path, PathBuf};

pub fn log_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("nvoc")
            .join("logs")
    }
    #[cfg(not(windows))]
    {
        // /var/log/nvoc when writable (systemd service runs as root);
        // fall back to /tmp for unprivileged foreground runs.
        let preferred = PathBuf::from("/var/log/nvoc");
        if std::fs::create_dir_all(&preferred).is_ok() {
            preferred
        } else {
            std::env::temp_dir().join("nvoc")
        }
    }
}

/// Initialize the file sink; returns the log path. `tee` mirrors entries to
/// stdout for foreground runs.
pub fn init(tee: bool) -> Result<PathBuf, String> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create log dir {}: {e}", dir.display()))?;
    let path = dir.join("nvoc-srv.log");
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("non-UTF8 log path {}", path.display()))?
        .to_string();
    let mut builder = log2::open(&path_str)
        .size(100 * 1024 * 1024)
        .rotate(2)
        .level(LevelFilter::Info);
    if tee {
        builder = builder.tee(true);
    }
    builder.start();
    Ok(path)
}

/// Redirect stdout/stderr into the log file. The returned handle must stay
/// alive for the process lifetime (leaked intentionally — the redirect is
/// global state for a service process).
pub fn redirect_stdio_to(log_path: &Path) {
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(file) => {
            let stdout = gag::Redirect::stdout(file.try_clone().expect("dup log fd"))
                .expect("redirect stdout");
            let stderr = gag::Redirect::stderr(file).expect("redirect stderr");
            std::mem::forget((stdout, stderr));
        }
        Err(e) => {
            eprintln!("cannot open {} for stdio redirect: {e}", log_path.display());
        }
    }
}
