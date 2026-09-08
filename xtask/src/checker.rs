//! `cargo xtask check`: the local mirror of the ci.yml lint gates.

use crate::util::{self, Res};
use std::process::Command;

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
    util::run(&mut clippy)?;

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
    util::run(&mut stressor)?;

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
