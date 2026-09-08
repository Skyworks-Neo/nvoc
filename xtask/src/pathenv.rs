//! Makes `target/release` reachable from future shells after a successful
//! release build, mirroring how `uv` wires itself into the user environment:
//! the per-user PATH registry value on Windows, or a marked PATH line in the
//! rc file of `$SHELL` (bash/zsh/fish) elsewhere.
//!
//! Idempotent — an rc/registry entry that already contains the directory is
//! left untouched, so every release build just verifies and moves on — and
//! best-effort: failures warn but never fail the build that already succeeded.

use crate::util::{self, Res};
use std::path::Path;

/// Called after a successful `xtask build --release`.
pub fn ensure_release_on_user_path(root: &Path) {
    if let Err(message) = add(root) {
        util::warn(&format!(
            "could not put target/release on the user PATH: {message}"
        ));
        util::hint(
            "add <repo>/target/release to your PATH manually if you want the \
             release binaries available in every shell",
        );
    }
}

/// True when `dir` already appears in the *live* PATH of this process — the
/// merged machine+user (or session) view. An entry configured at system level
/// or in a parent shell must not be duplicated into the per-user scope.
fn live_path_covers(dir: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|live| entry_present(&live.to_string_lossy(), dir))
}

#[cfg(windows)]
fn add(root: &Path) -> Res<()> {
    use std::process::Command;
    let dir = root.join("target").join("release");
    let dir = dir.to_string_lossy().replace('/', "\\").to_string();

    if live_path_covers(&dir) {
        return Ok(());
    }
    let query = Command::new("reg")
        .args(["query", r"HKCU\Environment", "/v", "Path"])
        .output()
        .map_err(|error| format!("could not run `reg query`: {error}"))?;
    let Some((kind, raw)) = parse_reg_path_value(&String::from_utf8_lossy(&query.stdout)) else {
        return write_user_path(&dir, "REG_SZ").and_then(|()| broadcast_environment_change());
    };
    if entry_present(&raw, &dir) {
        return Ok(());
    }
    println!("  [..]   adding {dir} to the user PATH");
    write_user_path(&format!("{raw};{dir}"), kind).and_then(|()| broadcast_environment_change())?;
    println!("  [ok]   user PATH updated (new terminals will see it)");
    Ok(())
}

/// Extracts the kind and raw data of the `Path` value from `reg query`
/// output, keeping `%VAR%` references unexpanded.
#[cfg(windows)]
fn parse_reg_path_value(query: &str) -> Option<(&'static str, String)> {
    const KINDS: [&str; 2] = ["REG_EXPAND_SZ", "REG_SZ"];
    for line in query.lines() {
        for kind in KINDS {
            if let Some(position) = line.find(kind) {
                let data = line[position + kind.len()..].trim();
                if !data.is_empty() {
                    return Some((kind, data.to_string()));
                }
            }
        }
    }
    None
}

/// Appends `data` as the user `Path` value, preserving the original registry
/// kind so existing `%VAR%` references keep expanding.
#[cfg(windows)]
fn write_user_path(data: &str, kind: &str) -> Res<()> {
    let mut command = std::process::Command::new("reg");
    command
        .args([
            r"add",
            r"HKCU\Environment",
            "/v",
            "Path",
            "/t",
            kind,
            "/d",
            data,
            "/f",
        ])
        .output()
        .map_err(|error| format!("could not run `reg add`: {error}"))
        .and_then(|output| {
            output.status.success().then_some(()).ok_or_else(|| {
                format!(
                    "`reg add` failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
            })
        })
}

/// Notifies running shells/explorer so new terminals pick the PATH up without
/// a re-login (`reg add` itself does not broadcast).
#[cfg(windows)]
fn broadcast_environment_change() -> Res<()> {
    let script = r#"
Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
$result = [UIntPtr]::Zero
[Win32.NativeMethods]::SendMessageTimeout([IntPtr]0xFFFF, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
"#;
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-Command", script]);
    command
        .output()
        .map(|output| {
            if output.status.success() {
                Ok(())
            } else {
                Err("the PATH was written but the change broadcast failed".to_string())
            }
        })
        .map_err(|error| format!("could not run `powershell`: {error}"))?
}

/// True when one PATH entry already names `dir` — case-insensitive on
/// Windows, exact elsewhere, trailing-slash tolerant on both.
fn entry_present(raw: &str, dir: &str) -> bool {
    let separator = if cfg!(windows) { ';' } else { ':' };
    raw.split(separator).any(|entry| {
        let entry = entry.trim().trim_end_matches(['\\', '/']);
        let dir = dir.trim_end_matches(['\\', '/']);
        if cfg!(windows) {
            entry.eq_ignore_ascii_case(dir)
        } else {
            entry == dir
        }
    })
}

#[cfg(unix)]
fn add(root: &Path) -> Res<()> {
    use std::path::PathBuf;
    let dir = root.join("target").join("release");
    let dir = dir.to_string_lossy().into_owned();
    if live_path_covers(&dir) {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Err("HOME is not set".to_string());
    };
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell = shell.rsplit('/').next().unwrap_or_default();
    let (rc, line) = rc_for_shell(&home, shell, &dir)?;
    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    if existing.contains(&dir) {
        return Ok(());
    }
    let mut appended = existing;
    if !appended.is_empty() && !appended.ends_with('\n') {
        appended.push('\n');
    }
    appended.push_str("# nvoc xtask: release binaries\n");
    appended.push_str(&line);
    appended.push('\n');
    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    std::fs::write(&rc, appended)
        .map_err(|error| format!("could not write {}: {error}", rc.display()))?;
    println!(
        "  [ok]   added {dir} to {} (new shells will see it)",
        rc.display()
    );
    Ok(())
}

/// The rc file and PATH line for `$SHELL`, in the shape `uv` uses.
#[cfg(unix)]
fn rc_for_shell(home: &Path, shell: &str, dir: &str) -> Res<(std::path::PathBuf, String)> {
    match shell {
        "fish" => Ok((
            home.join(".config/fish/config.fish"),
            format!("fish_add_path {dir}"),
        )),
        "bash" | "sh" => Ok((home.join(".bashrc"), format!("export PATH=\"{dir}:$PATH\""))),
        "zsh" => Ok((home.join(".zshrc"), format!("export PATH=\"{dir}:$PATH\""))),
        other => Err(format!(
            "unsupported $SHELL `{other}` (supported: bash, zsh, fish)"
        )),
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #[test]
    fn reg_output_is_parsed_with_kind_and_raw_data() {
        let query = "\
HKEY_CURRENT_USER\\Environment
    Path    REG_EXPAND_SZ    C:\\bin;%USERPROFILE%\\bin
";
        let (kind, data) = super::parse_reg_path_value(query).expect("value line found");
        assert_eq!(kind, "REG_EXPAND_SZ");
        assert_eq!(data, r"C:\bin;%USERPROFILE%\bin");
        assert!(super::parse_reg_path_value("HKEY_CURRENT_USER\\Environment\n").is_none());
    }

    #[test]
    fn path_entries_match_case_and_slash_insensitively() {
        let raw = r"C:\Bin;D:\other\";
        assert!(super::entry_present(raw, r"c:\bin"));
        assert!(super::entry_present(raw, r"D:\other"));
        assert!(!super::entry_present(raw, r"E:\nowhere"));
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use std::path::Path;

    #[test]
    fn shells_map_to_their_rc_and_path_line() {
        let home = Path::new("/home/dev");
        let (rc, line) = super::rc_for_shell(home, "fish", "/x/target/release").unwrap();
        assert_eq!(rc, home.join(".config/fish/config.fish"));
        assert_eq!(line, "fish_add_path /x/target/release");
        let (rc, line) = super::rc_for_shell(home, "zsh", "/x/target/release").unwrap();
        assert_eq!(rc, home.join(".zshrc"));
        assert_eq!(line, "export PATH=\"/x/target/release:$PATH\"");
        assert!(super::rc_for_shell(home, "tcsh", "/x").is_err());
    }
}
