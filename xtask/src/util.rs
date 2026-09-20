//! Process helpers shared by every subcommand.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub type Res<T> = Result<T, String>;

/// Repository root, anchored to the xtask crate location so every subcommand
/// behaves identically no matter which workspace directory `cargo xtask` was
/// invoked from.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask always lives inside the workspace")
        .to_path_buf()
}

pub fn step(title: &str) {
    println!("==> {title}");
}

pub fn warn(msg: &str) {
    println!("  [warn] {msg}");
}

pub fn hint(msg: &str) {
    println!("         hint: {msg}");
}

pub fn display(command: &Command) -> String {
    let mut line = command.get_program().to_string_lossy().into_owned();
    for arg in command.get_args() {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    line
}

/// Runs a command with inherited stdio; fails when the child exits nonzero.
pub fn run(command: &mut Command) -> Res<()> {
    let shown = display(command);
    println!("  $ {shown}");
    let status = command
        .status()
        .map_err(|error| format!("failed to spawn `{shown}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{shown}` exited with {status}"))
    }
}

/// Like [`run`], but pipes the child's stderr through this process line by
/// line while collecting it into `stderr_log` (stdout stays inherited). The
/// log lets callers diagnose cargo failures after the fact; rustc diagnostics
/// all go to stderr, so nothing diagnostic is lost.
pub fn run_capture_stderr(command: &mut Command, stderr_log: &mut String) -> Res<()> {
    use std::io::{BufRead, BufReader};
    let shown = display(command);
    println!("  $ {shown}");
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn `{shown}`: {error}"))?;
    if let Some(stderr) = child.stderr.take() {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    eprint!("{line}");
                    stderr_log.push_str(&line);
                }
                Err(error) => {
                    // The exit status stays the source of truth; keep the
                    // lines collected so far and stop draining.
                    eprintln!("  [warn] could not read child stderr: {error}");
                    break;
                }
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for `{shown}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{shown}` exited with {status}"))
    }
}

/// Like [`run`], but only prints the command when `dry_run` is set.
pub fn run_dry(command: &mut Command, dry_run: bool) -> Res<()> {
    if dry_run {
        println!("  [dry-run] $ {}", display(command));
        Ok(())
    } else {
        run(command)
    }
}

/// Runs a command capturing stdout (stderr inherited) and returns trimmed stdout.
pub fn capture(command: &mut Command) -> Res<String> {
    let shown = display(command);
    let output: Output = command
        .output()
        .map_err(|error| format!("failed to spawn `{shown}`: {error}"))?;
    if !output.status.success() {
        return Err(format!("`{shown}` exited with {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when the program can be spawned and reports success on `--version`.
pub fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Builds `uv run --locked --package <package> --group dev --no-config …`,
/// the same invocation shape ci.yml uses for every Python step.
pub fn uv_run(root: &Path, package: &str, extra: &[&str]) -> Command {
    let mut command = Command::new("uv");
    command.args([
        "run",
        "--locked",
        "--package",
        package,
        "--group",
        "dev",
        "--no-config",
    ]);
    command.args(extra);
    command.current_dir(root);
    command
}
