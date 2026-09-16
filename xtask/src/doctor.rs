//! `cargo xtask setup`: environment doctor plus fresh-clone bootstrap.
//!
//! Every check prints `[ok]` / `[warn]` / `[FAIL]` with an actionable hint, and
//! mutating steps honor `--dry-run`. Blocking failures (git, submodule, Rust,
//! the native toolchain, uv, Python envs) fail the run; informational probes
//! (CUDA runtime, tkinter) never do. The native-toolchain check branches on
//! the resolved rustc's host triple: `-msvc` requires Visual Studio Build
//! Tools, while gnu-like faces (msys2's clang64 `windows-gnullvm`) link via
//! the toolchain's own lld and skip the MSVC gate.

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
    let rust = check_rust();
    problems += rust.problems;
    problems += check_native_toolchain(rust.flavor);

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
    println!("  [info] PyPI index: uv runs default to the Tsinghua (tuna) mirror");
    println!("         (UV_DEFAULT_INDEX already overrides this if it is set).");

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
/// its absence blocks everything. A missing checkout is safe to bootstrap
/// automatically here; a diverged one may hold intentional nvapi-rs work, so
/// that case is only reported (see [`ensure_submodule_synced`]).
fn check_submodule(args: &SetupArgs, root: &Path) -> usize {
    if !root.join("nvapi-rs").join("Cargo.toml").is_file() {
        println!("  [..]   initializing nvapi-rs submodule");
        let mut command = Command::new("git");
        command
            .args(["submodule", "update", "--init", "nvapi-rs"])
            .current_dir(root);
        return match util::run_dry(&mut command, args.dry_run) {
            Ok(()) => {
                println!("  [ok]   nvapi-rs submodule initialized");
                0
            }
            Err(error) => {
                println!("  [FAIL] could not initialize the nvapi-rs submodule ({error})");
                util::hint("forks need a submodule URL override (see CONTRIBUTING.md):");
                util::hint(
                    "git config submodule.nvapi-rs.url git@github.com:<your-org>/nvapi-rs.git",
                );
                1
            }
        };
    }
    println!("  [..]   checking nvapi-rs submodule against the commit recorded in HEAD");
    match ensure_submodule_synced(root) {
        Ok(()) => {
            println!("  [ok]   nvapi-rs submodule at the recorded commit");
            0
        }
        Err(message) => {
            println!("  [FAIL] {message}");
            1
        }
    }
}

/// Fails unless the nvapi-rs submodule checkout matches the commit recorded
/// by HEAD. A stale checkout (very common on machines that pull the main
/// repository without ever running `git submodule update`) otherwise surfaces
/// as cryptic E0425/E0599 errors in nvoc-core, because nvapi-rs gains new API
/// surfaces between recorded commits.
pub fn ensure_submodule_synced(root: &Path) -> Res<()> {
    let mut command = Command::new("git");
    command
        .args(["submodule", "status", "--", "nvapi-rs"])
        .current_dir(root);
    let status = util::capture(&mut command)
        .map_err(|error| format!("could not query the nvapi-rs submodule state: {error}"))?;
    let Some(line) = status.lines().next() else {
        return Err("git reported no nvapi-rs submodule although it is in .gitmodules".to_string());
    };
    match submodule_problem(line) {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
}

/// Maps the first column of a `git submodule status` line to a problem
/// description: `-` never initialized, `+` checked out aside from the
/// recorded commit (older machine behind, or an intentional pin), `U`
/// conflicted.
fn submodule_problem(line: &str) -> Option<String> {
    match line.chars().next().unwrap_or(' ') {
        ' ' => None,
        '-' => Some(
            "nvapi-rs submodule is not initialized; run: git submodule update --init nvapi-rs"
                .to_string(),
        ),
        '+' => Some(
            "nvapi-rs submodule is not at the commit recorded by HEAD; \
             if it is simply stale, run: git submodule update --init nvapi-rs \
             (if you intentionally pinned another nvapi-rs revision, keep it \
             and bump the gitlink in the main repository instead)"
                .to_string(),
        ),
        'U' => Some("nvapi-rs submodule has merge conflicts; resolve them first".to_string()),
        _ => None,
    }
}

/// Which link environment the resolved rustc is built for, decided from the
/// `host:` line of `rustc -vV`. The msvc face needs Visual Studio components;
/// the gnu-like faces (windows-gnu, and msys2's windows-gnullvm) link through
/// the toolchain's own lld and need none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolchainFlavor {
    Msvc,
    GnuLike,
}

fn flavor_for_host(host: &str) -> ToolchainFlavor {
    if host.ends_with("-msvc") {
        ToolchainFlavor::Msvc
    } else if host.contains("gnu") {
        ToolchainFlavor::GnuLike
    } else {
        // Unknown triple: keep the historical MSVC requirement.
        ToolchainFlavor::Msvc
    }
}

struct RustStatus {
    problems: usize,
    flavor: ToolchainFlavor,
}

fn check_rust() -> RustStatus {
    if !util::have("rustc") {
        println!("  [FAIL] rustc not found");
        util::hint("install rustup from https://rustup.rs; rust-toolchain.toml pins 1.95.0");
        return RustStatus {
            problems: 1,
            flavor: ToolchainFlavor::Msvc,
        };
    }
    let mut command = Command::new("rustc");
    command.arg("-vV");
    match util::capture(&mut command) {
        Ok(output) => {
            let release = find_field(&output, "release").unwrap_or("unknown");
            let host = find_field(&output, "host").unwrap_or("");
            let flavor = flavor_for_host(host);
            match flavor {
                ToolchainFlavor::Msvc => println!("  [ok]   rustc {release} ({host})"),
                ToolchainFlavor::GnuLike => {
                    println!("  [ok]   rustc {release} ({host}, gnu-like face)");
                }
            }
            if release != "1.95.0" && flavor == ToolchainFlavor::Msvc {
                util::hint(
                    "rust-toolchain.toml pins 1.95.0; rustup fetches it on the next cargo invocation",
                );
            } else if release != "1.95.0" && flavor == ToolchainFlavor::GnuLike {
                // A non-rustup toolchain (e.g. msys2's) ignores
                // rust-toolchain.toml entirely, so the pin cannot auto-apply.
                util::hint(
                    "rust-toolchain.toml pins 1.95.0 for the msvc face; this non-rustup toolchain \
                     ignores that file and follows its package manager (e.g. `pacman -Syu`)",
                );
            }
            RustStatus {
                problems: 0,
                flavor,
            }
        }
        Err(error) => {
            println!("  [FAIL] `rustc -vV` failed ({error})");
            RustStatus {
                problems: 1,
                flavor: ToolchainFlavor::Msvc,
            }
        }
    }
}

#[cfg(windows)]
fn check_native_toolchain(flavor: ToolchainFlavor) -> usize {
    match flavor {
        ToolchainFlavor::Msvc => check_msvc(),
        ToolchainFlavor::GnuLike => {
            // gnu-like faces ship their own linker (rust-lld with the
            // toolchain's own mingw-w64 sysroot on the llvm face, the bundled
            // mingw driver on the gcc face); no Visual Studio components
            // involved. The C compiler differs per face: clang on clang64,
            // gcc on ucrt64.
            let c_compiler = ["clang", "gcc"].into_iter().find(|&name| util::have(name));
            match c_compiler {
                Some(compiler) => {
                    let version =
                        util::capture(Command::new(compiler).arg("--version")).unwrap_or_default();
                    let first = version.lines().next().unwrap_or(compiler);
                    println!("  [ok]   gnu-like native toolchain ({first})");
                }
                None => util::warn(
                    "neither clang nor gcc on PATH; rust still links via its bundled linker, \
                     but C sources would have no compiler",
                ),
            }
            util::hint(
                "gnu-like artifacts need their toolchain's bin on PATH to run \
                 (e.g. C:\\msys64\\clang64\\bin or C:\\msys64\\ucrt64\\bin)",
            );
            0
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
fn check_native_toolchain(_flavor: ToolchainFlavor) -> usize {
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
    #[cfg(windows)]
    if let Some(found) = uv_off_path_install() {
        println!("  [FAIL] uv is installed off-PATH at {}", found.display());
        util::hint(
            "msys2 shells launched with a minimal PATH hide it: start them via \
             msys2_shell.cmd -clang64 -use-full-path,",
        );
        util::hint("or add uv.exe's directory to PATH, then re-run setup");
        return false;
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

/// Well-known uv install locations that can sit off-PATH — the msys2
/// minimal-PATH shell is the case in point: uv is installed and working, the
/// shell just never sees it. Pure so tests can inject the environment.
#[cfg(windows)]
fn uv_off_path_candidates(
    local_app_data: Option<&str>,
    user_profile: Option<&str>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = local_app_data {
        candidates.push(
            Path::new(local)
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("uv.exe"),
        );
    }
    if let Some(profile) = user_profile {
        candidates.push(Path::new(profile).join(".local").join("bin").join("uv.exe"));
    }
    candidates
}

/// The first well-known location where uv is actually installed, if any.
#[cfg(windows)]
fn uv_off_path_install() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok();
    let profile = std::env::var("USERPROFILE").ok();
    uv_off_path_candidates(local.as_deref(), profile.as_deref())
        .into_iter()
        .find(|candidate| candidate.is_file())
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
        util::apply_uv_index(&mut command);
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
    util::apply_uv_index(&mut command);
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

#[cfg(test)]
mod tests {
    use super::{ToolchainFlavor, flavor_for_host};

    #[test]
    fn submodule_status_flags_map_to_problems() {
        assert!(super::submodule_problem(" 73ff53ab nvapi-rs").is_none());
        let stale = super::submodule_problem("+73ff53ab nvapi-rs").unwrap();
        assert!(stale.contains("git submodule update --init nvapi-rs"));
        let missing = super::submodule_problem("-73ff53ab nvapi-rs").unwrap();
        assert!(missing.contains("not initialized"));
        assert!(
            super::submodule_problem("U73ff53ab nvapi-rs")
                .unwrap()
                .contains("merge conflicts")
        );
    }

    #[test]
    fn host_triples_map_to_toolchain_flavors() {
        assert_eq!(
            flavor_for_host("x86_64-pc-windows-msvc"),
            ToolchainFlavor::Msvc
        );
        assert_eq!(
            flavor_for_host("x86_64-pc-windows-gnullvm"),
            ToolchainFlavor::GnuLike
        );
        assert_eq!(
            flavor_for_host("aarch64-pc-windows-gnullvm"),
            ToolchainFlavor::GnuLike
        );
        assert_eq!(
            flavor_for_host("x86_64-pc-windows-gnu"),
            ToolchainFlavor::GnuLike
        );
        // Unknown triples keep the historical MSVC requirement.
        assert_eq!(flavor_for_host("something-else"), ToolchainFlavor::Msvc);
    }

    #[cfg(windows)]
    #[test]
    fn uv_candidates_cover_winget_links_and_local_bin() {
        use super::uv_off_path_candidates;
        let candidates =
            uv_off_path_candidates(Some(r"C:\Users\dev\AppData\Local"), Some(r"C:\Users\dev"));
        assert_eq!(
            candidates,
            vec![
                std::path::PathBuf::from(r"C:\Users\dev\AppData\Local")
                    .join(r"Microsoft\WinGet\Links\uv.exe"),
                std::path::PathBuf::from(r"C:\Users\dev").join(r".local\bin\uv.exe"),
            ]
        );
        assert!(uv_off_path_candidates(None, None).is_empty());
    }
}
