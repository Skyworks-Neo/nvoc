//! `cargo xtask setup`: environment doctor plus fresh-clone bootstrap.
//!
//! Every check prints `[ok]` / `[warn]` / `[FAIL]` with an actionable hint, and
//! mutating steps honor `--dry-run`. Blocking failures (git, submodule, Rust,
//! MSVC, uv, Python envs) fail the run; informational probes (CUDA runtime,
//! tkinter) never do.

use crate::args::SetupArgs;
use crate::util::{self, Res};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn setup(args: &SetupArgs) -> Res<()> {
    let root = util::repo_root();
    println!("==> nvoc development environment setup");
    if args.dry_run {
        println!("  (dry run: mutating commands are printed, not executed)");
    }

    let mut problems = 0usize;
    problems += check_git();
    problems += check_submodule(args, &root);
    problems += check_rust();
    problems += check_msvc();

    if check_uv(args) {
        problems += bootstrap_python(args, &root);
        if !args.dry_run {
            problems += sanity_imports(&root);
        }
    } else {
        println!("  [..]   skipping Python bootstrap (uv unavailable)");
        problems += 1;
    }

    probe_cuda_runtime();
    println!("  [info] PyPI index: the root pyproject.toml pins the Tsinghua (tuna) mirror;");
    println!("         override with UV_DEFAULT_INDEX if that mirror is slow from your network.");

    if problems == 0 {
        println!("\nsetup complete: environment ready.");
        Ok(())
    } else {
        Err(format!(
            "{problems} blocking problem(s) remain; see the [FAIL] lines above"
        ))
    }
}

fn check_git() -> usize {
    if util::have("git") {
        println!("  [ok]   git");
        0
    } else {
        println!("  [FAIL] git not found");
        util::hint("install from https://git-scm.com (or `winget install Git.Git`)");
        1
    }
}

/// The nvapi-rs submodule is a path dependency of every NVAPI-backed crate, so
/// its absence blocks everything.
fn check_submodule(args: &SetupArgs, root: &Path) -> usize {
    if root.join("nvapi-rs").join("Cargo.toml").is_file() {
        println!("  [ok]   nvapi-rs submodule present");
        return 0;
    }
    println!("  [..]   initializing nvapi-rs submodule");
    let mut command = Command::new("git");
    command
        .args(["submodule", "update", "--init"])
        .current_dir(root);
    match util::run_dry(&mut command, args.dry_run) {
        Ok(()) => {
            println!("  [ok]   nvapi-rs submodule initialized");
            0
        }
        Err(error) => {
            println!("  [FAIL] could not initialize the nvapi-rs submodule ({error})");
            util::hint("forks need a submodule URL override (see CONTRIBUTING.md):");
            util::hint("git config submodule.nvapi-rs.url git@github.com:<your-org>/nvapi-rs.git");
            1
        }
    }
}

fn check_rust() -> usize {
    if !util::have("rustc") {
        println!("  [FAIL] rustc not found");
        util::hint("install rustup from https://rustup.rs; rust-toolchain.toml pins 1.95.0");
        return 1;
    }
    let mut command = Command::new("rustc");
    command.arg("-vV");
    match util::capture(&mut command) {
        Ok(output) => {
            let release = find_field(&output, "release").unwrap_or("unknown");
            println!("  [ok]   rustc {release}");
            if release != "1.95.0" {
                util::hint(
                    "rust-toolchain.toml pins 1.95.0; rustup fetches it on the next cargo invocation",
                );
            }
            0
        }
        Err(error) => {
            println!("  [FAIL] `rustc -vV` failed ({error})");
            1
        }
    }
}

#[cfg(windows)]
fn check_msvc() -> usize {
    let program_files_x86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    let vswhere = PathBuf::from(program_files_x86)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    if !vswhere.is_file() {
        println!("  [FAIL] MSVC Build Tools not detected (vswhere missing)");
        util::hint(
            "winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override \"--quiet --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100\"",
        );
        return 1;
    }
    let mut command = Command::new(&vswhere);
    command.args([
        "-latest",
        "-products",
        "*",
        "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property",
        "installationPath",
    ]);
    match util::capture(&mut command) {
        Ok(installation) if !installation.is_empty() => {
            println!("  [ok]   MSVC C++ toolset ({installation})");
            0
        }
        _ => {
            println!("  [FAIL] Visual Studio found but the C++ toolset (MSVC x86/x64) is missing");
            util::hint(
                "add the \"Desktop development with C++\" workload, or the minimal components:",
            );
            util::hint(
                "winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override \"--quiet --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100\"",
            );
            1
        }
    }
}

#[cfg(not(windows))]
fn check_msvc() -> usize {
    if util::have("cc") {
        println!("  [ok]   C compiler (cc)");
    } else {
        util::warn(
            "no `cc` on PATH; building pynvoc needs a C toolchain (e.g. apt install build-essential)",
        );
    }
    0
}

/// Returns true when uv is available afterwards.
fn check_uv(args: &SetupArgs) -> bool {
    if util::have("uv") {
        let mut command = Command::new("uv");
        command.arg("--version");
        let version = util::capture(&mut command).unwrap_or_default();
        println!("  [ok]   uv ({version})");
        return true;
    }
    if args.install_missing {
        println!("  [..]   installing uv");
        let mut command = install_uv_command();
        let installed = util::run_dry(&mut command, args.dry_run).is_ok();
        if args.dry_run {
            return false;
        }
        if installed && util::have("uv") {
            println!("  [ok]   uv installed");
            return true;
        }
        println!("  [FAIL] uv was installed but is not on PATH yet");
        util::hint("open a new shell so PATH changes take effect, then re-run setup");
        return false;
    }
    println!("  [FAIL] uv not found");
    util::hint("winget install astral-sh.uv  (or re-run setup with --install-missing)");
    util::hint("Linux/macOS: curl -LsSf https://astral.sh/uv/install.sh | sh");
    false
}

fn install_uv_command() -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("winget");
        command.args([
            "install",
            "--id",
            "astral-sh.uv",
            "-e",
            "--accept-source-agreements",
            "--accept-package-agreements",
        ]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("curl -LsSf https://astral.sh/uv/install.sh | sh");
        command
    }
}

/// Syncs every Python workspace member the way ci.yml does, then rebuilds the
/// pynvoc extension in develop mode. uv sync already installs a backend-built
/// wheel, so a failing `maturin develop` downgrade to a warning: the import
/// sanity check below is the real gate.
fn bootstrap_python(args: &SetupArgs, root: &Path) -> usize {
    let mut problems = 0usize;
    for (package, label) in [
        ("nvoc-gui", "sync gui environment"),
        ("nvoc-tui", "sync tui environment"),
        ("pynvoc", "sync pynvoc environment"),
    ] {
        println!("  [..]   {label}");
        let mut command = Command::new("uv");
        command
            .args([
                "sync",
                "--locked",
                "--package",
                package,
                "--group",
                "dev",
                "--no-config",
            ])
            .current_dir(root);
        match util::run_dry(&mut command, args.dry_run) {
            Ok(()) => println!("  [ok]   {label}"),
            Err(error) => {
                println!("  [FAIL] {label}");
                println!("         {error}");
                problems += 1;
            }
        }
    }

    println!("  [..]   building pynvoc native extension (maturin develop --release)");
    let mut command = Command::new("uv");
    command
        .args([
            "run",
            "--locked",
            "--package",
            "pynvoc",
            "--group",
            "dev",
            "--no-config",
            "maturin",
            "develop",
            "--release",
        ])
        // maturin resolves pyproject.toml from the working directory; invoked
        // from the repo root it reads the uv-workspace manifest (no
        // [build-system]) and aborts, leaving whatever stale wheel uv sync
        // had cached in place — which then fails the import check below.
        .current_dir(root.join("nvoc-python"));
    match util::run_dry(&mut command, args.dry_run) {
        Ok(()) => println!("  [ok]   pynvoc native extension built"),
        Err(error) => {
            util::warn(&format!(
                "maturin develop --release failed ({error}); the wheel installed by uv sync may still work"
            ));
        }
    }
    problems
}

fn sanity_imports(root: &Path) -> usize {
    let mut problems = 0usize;

    println!("  [..]   checking tkinter in the gui environment");
    let mut tkinter = util::uv_run(root, "nvoc-gui", &["python", "-c", "import tkinter"]);
    if util::run(&mut tkinter).is_err() {
        util::warn(
            "tkinter unavailable; the GUI needs it (Linux: python3-tk, Windows: reinstall Python with tcl/tk)",
        );
    }

    println!("  [..]   checking pynvoc native module in the tui environment");
    let mut native = util::uv_run(root, "nvoc-tui", &["python", "-c", "import pynvoc._native"]);
    if util::run(&mut native).is_err() {
        println!("  [FAIL] pynvoc._native is not importable from the tui environment");
        util::hint("check the maturin step above, then re-run setup");
        problems += 1;
    }
    problems
}

/// Warn-only: the CUDA stressor needs the runtime DLLs/SOs to RUN, never to
/// build (cudarc dlopens everything at runtime).
fn probe_cuda_runtime() {
    let (found, missing) = probe_cuda_targets();
    if found.is_empty() {
        util::warn(
            "CUDA runtime libraries not found; only needed to RUN the CUDA stressor (building requires no CUDA toolkit)",
        );
    } else if missing.is_empty() {
        println!("  [ok]   CUDA runtime: {}", found.join(", "));
    } else {
        println!("  [ok]   CUDA runtime found: {}", found.join(", "));
        util::warn(&format!(
            "missing: {}; the stressor loads all of them at runtime",
            missing.join(", ")
        ));
    }
}

#[cfg(windows)]
fn probe_cuda_targets() -> (Vec<&'static str>, Vec<&'static str>) {
    const PREFIXES: &[&str] = &["nvrtc64", "cublas64", "cublasLt64", "cudart64"];
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for &prefix in PREFIXES {
        if dirs.iter().any(|dir| dir_contains_dll(dir, prefix)) {
            found.push(prefix);
        } else {
            missing.push(prefix);
        }
    }
    (found, missing)
}

#[cfg(windows)]
fn dir_contains_dll(dir: &Path, prefix: &str) -> bool {
    dir.read_dir()
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                name.starts_with(prefix) && name.ends_with(".dll")
            })
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn probe_cuda_targets() -> (Vec<&'static str>, Vec<&'static str>) {
    const LIBRARIES: &[&str] = &["libnvrtc.so", "libcublas.so", "libcudart.so"];
    let mut found = Vec::new();
    let mut missing = Vec::new();
    match util::capture(Command::new("ldconfig").arg("-p")) {
        Ok(loader_cache) => {
            for &library in LIBRARIES {
                if loader_cache.contains(library) {
                    found.push(library);
                } else {
                    missing.push(library);
                }
            }
        }
        Err(_) => missing.extend_from_slice(LIBRARIES),
    }
    (found, missing)
}

fn find_field<'a>(version_output: &'a str, field: &str) -> Option<&'a str> {
    let prefix = format!("{field}: ");
    version_output
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
}
