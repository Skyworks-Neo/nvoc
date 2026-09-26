//! `cargo xtask check`: the local mirror of the ci.yml lint gates, plus the
//! `ci --fmt` auto-fix pass that force-applies formatting and machine
//! applicable lint suggestions before the gate re-runs.

use crate::nvapi_cache;
use crate::util::{self, Res};
use std::process::Command;

/// Auto-apply every compliance fix the toolchain can make on its own:
/// rustfmt, clippy's machine-applicable suggestions, ruff format and ruff's
/// fixable lint violations. Verification is deliberately NOT part of this
/// pass — the caller re-runs [`check`] afterwards, which still fails on
/// anything the fixers could not resolve.
pub fn fix_all() -> Res<()> {
    let root = util::repo_root();

    util::step("auto-fix: rustfmt (cargo fmt --all)");
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all"]).current_dir(&root);
    util::run(&mut fmt)?;

    // clippy --fix cannot run under -D warnings (the gate would abort before
    // applying suggestions); the plain run only emits fixable suggestions.
    // --allow-dirty is required because development trees are rarely clean.
    util::step("auto-fix: clippy --fix (workspace, CUDA stressor excluded)");
    let mut clippy = Command::new("cargo");
    clippy
        .args([
            "clippy",
            "--workspace",
            "--exclude",
            "cli-stressor-cuda-rs",
            "--all-targets",
            "--fix",
            "--allow-dirty",
        ])
        .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut clippy)?;

    util::step("auto-fix: clippy --fix (cli-stressor-cuda-rs, no default features)");
    let mut stressor = Command::new("cargo");
    stressor
        .args([
            "clippy",
            "-p",
            "cli-stressor-cuda-rs",
            "--all-targets",
            "--no-default-features",
            "--fix",
            "--allow-dirty",
        ])
        .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut stressor)?;

    let excludes = ruff_exclude_args(&root);
    util::step("auto-fix: ruff format (.)");
    let mut format = util::uv_run(&root, "nvoc-tui", &["ruff", "format", "."]);
    format.args(&excludes);
    util::run(&mut format)?;

    util::step("auto-fix: ruff check --fix (.)");
    let mut lint = util::uv_run(&root, "nvoc-tui", &["ruff", "check", ".", "--fix"]);
    lint.args(&excludes);
    util::run(&mut lint)
}

pub fn check() -> Res<()> {
    let root = util::repo_root();

    util::step("rustfmt (cargo fmt --all -- --check)");
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all", "--", "--check"])
        .current_dir(&root);
    util::run(&mut fmt)?;

    util::step("clippy: workspace (CUDA stressor excluded)");
    let mut clippy = Command::new("cargo");
    clippy
        .args([
            "clippy",
            "--workspace",
            "--exclude",
            "cli-stressor-cuda-rs",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut clippy)?;

    cross_clippy_mirror(&root)?;

    util::step("clippy: cli-stressor-cuda-rs (no default features)");
    let mut stressor = Command::new("cargo");
    stressor
        .args([
            "clippy",
            "-p",
            "cli-stressor-cuda-rs",
            "--all-targets",
            "--no-default-features",
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut stressor)?;

    util::step("cross-check: cli-stressor-cuda-rs (aarch64-linux, where c_char = u8)");
    cross_check_cuda_stressor(&root)?;

    util::step("ruff format (.)");
    let excludes = ruff_exclude_args(&root);
    if !excludes.is_empty() {
        let skipped: Vec<&String> = excludes.iter().skip(1).step_by(2).collect();
        println!(
            "  local-only paths invisible to CI are excluded: {}",
            skipped
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut format = util::uv_run(&root, "nvoc-tui", &["ruff", "format", ".", "--check"]);
    format.args(&excludes);
    util::run(&mut format)?;

    util::step("ruff check (.)");
    let mut lint = util::uv_run(&root, "nvoc-tui", &["ruff", "check", "."]);
    lint.args(&excludes);
    util::run(&mut lint)
}

/// Re-runs the CI clippy shape against the platform this host is NOT, so
/// `#[cfg(...)]`-gated code compiles in the gate instead of silently
/// vanishing: platform-skewed imports and dead items pass the host clippy
/// yet fail `-D warnings` on the other platform's CI job (xtask's own
/// doctor.rs unused `PathBuf`, core's dead non-Windows stub — both caught
/// here). Windows hosts mirror the ubuntu job (`x86_64-unknown-linux-gnu`);
/// Linux hosts mirror the Windows jobs (`x86_64-pc-windows-msvc` — a
/// workspace-wide superset of ci.yml's auto-optimizer/srv Windows runs);
/// hosts with no CI platform (macOS) mirror nothing. Static checks only —
/// cross-compiled test binaries cannot execute locally, so behavioral
/// platform gaps still belong to CI. The target's rust-std is installed on
/// first use.
fn cross_clippy_mirror(root: &std::path::Path) -> Res<()> {
    let target = match std::env::consts::OS {
        "windows" => "x86_64-unknown-linux-gnu",
        "linux" => "x86_64-pc-windows-msvc",
        _ => return Ok(()),
    };

    util::step(&format!("clippy: cross-target mirror ({target})"));
    let installed = util::capture(Command::new("rustup").args(["target", "list", "--installed"]))?
        .lines()
        .any(|line| line.trim() == target);
    if !installed {
        let mut add = Command::new("rustup");
        add.args(["target", "add", target]);
        util::run(&mut add)?;
    }

    let mut clippy = Command::new("cargo");
    clippy
        .args([
            "clippy",
            "--workspace",
            "--exclude",
            "cli-stressor-cuda-rs",
            "--all-targets",
            "--target",
            target,
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(root);
    nvapi_cache::run_guarded(root, &mut clippy)
}

/// Cross-checks the CUDA stressor against aarch64-linux, the one release
/// target where `core::ffi::c_char` is `u8` rather than `i8`. release.yml's
/// linux-arm64 cell is the only place the crate meets that target, so a
/// c_char-skewed buffer (e.g. `[i8]` passed to cudarc's `*mut c_char` APIs)
/// passes every host gate and only explodes when a release tag builds
/// (E0308, the v0.2.0-alpha.2 arm64 cell). Check-only — no cross linker and
/// no CUDA toolkit are needed; the target's rust-std is installed on first
/// use, mirroring [`cross_clippy_mirror`].
fn cross_check_cuda_stressor(root: &std::path::Path) -> Res<()> {
    const TARGET: &str = "aarch64-unknown-linux-gnu";

    let installed = util::capture(Command::new("rustup").args(["target", "list", "--installed"]))?
        .lines()
        .any(|line| line.trim() == TARGET);
    if !installed {
        let mut add = Command::new("rustup");
        add.args(["target", "add", TARGET]);
        util::run(&mut add)?;
    }

    let mut check = Command::new("cargo");
    check
        .args([
            "check",
            "-p",
            "cli-stressor-cuda-rs",
            "--features",
            "cuda12,vulkan",
            "--target",
            TARGET,
        ])
        .current_dir(root);
    nvapi_cache::run_guarded(root, &mut check)
}

/// `--exclude` arguments for the ruff steps covering top-level paths git does
/// not track at all. ci.yml runs on a fresh checkout, so purely local working
/// trees (reverse-engineering scripts, wiki clones, …) must not fail the local
/// gate. Directories that also contain tracked files stay in scope: a new
/// untracked source file there is fair game for the gate.
fn ruff_exclude_args(root: &std::path::Path) -> Vec<String> {
    let Some(untracked) =
        git_top_paths(root, &["ls-files", "--others", "--exclude-standard", "-z"])
    else {
        return Vec::new();
    };
    let tracked = git_top_paths(root, &["ls-files", "-z"]).unwrap_or_default();
    exclude_tops(&untracked, &tracked)
        .iter()
        .flat_map(|top| ["--exclude".to_string(), top.clone()])
        .collect()
}

/// Distinct first path components reported by `git ls-files`.
fn git_top_paths(root: &std::path::Path, args: &[&str]) -> Option<Vec<String>> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root);
    let listing = util::capture(&mut command).ok()?;
    let mut tops: Vec<String> = listing
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| first_component(entry).to_string())
        .collect();
    tops.sort();
    tops.dedup();
    Some(tops)
}

fn exclude_tops(untracked: &[String], tracked: &[String]) -> Vec<String> {
    let mut tops: Vec<String> = untracked
        .iter()
        .map(|path| first_component(path).to_string())
        .filter(|top| {
            !tracked
                .iter()
                .any(|tracked_path| first_component(tracked_path) == top.as_str())
        })
        .collect();
    tops.sort();
    tops.dedup();
    tops
}

fn first_component(path: &str) -> &str {
    match path.find(['/', '\\']) {
        Some(index) => &path[..index],
        None => path,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_fully_local_top_level_paths_are_excluded() {
        let excludes = super::exclude_tops(
            &[
                "reverse/a.py".to_string(),
                "nvoc.wiki/x.md".to_string(),
                "tui/new_file.py".to_string(),
            ],
            &["tui/lib.py".to_string(), "README.md".to_string()],
        );
        assert_eq!(
            excludes,
            vec!["nvoc.wiki".to_string(), "reverse".to_string()]
        );
    }

    #[test]
    fn path_first_component_handles_both_separators() {
        assert_eq!(super::first_component("reverse/a.py"), "reverse");
        assert_eq!(super::first_component("reverse\\a.py"), "reverse");
        assert_eq!(super::first_component("README.md"), "README.md");
    }
}
