//! Guards the cargo build cache against stale nvapi metadata.
//!
//! nvapi-rs is a path dependency whose sources evolve with the RE work, and
//! cargo fingerprints it by mtime. An old artifact can resurface with a fresh
//! mtime (the `deps/` copies are hardlinks into `target/debug/incremental`),
//! at which point cargo keeps serving metadata compiled from an older
//! submodule revision and nvoc-core fails with E0425/E0432/E0599 "not found
//! in `::nvapi`" errors — the signature of a wrong nvapi *version* that is
//! actually a cache problem (observed 2026-09-08: release artifacts healthy,
//! one clippy feature-unit poisoned while the submodule sat exactly at the
//! commit recorded by HEAD).
//!
//! Two defenses:
//! - [`ensure_fresh`]: a content fingerprint (submodule HEAD + working-tree
//!   diff) stamped under `target/xtask`; any change force-cleans the nvapi
//!   packages before cargo runs.
//! - [`run_guarded`]: when a compile-bearing step still fails with the
//!   signature, the nvapi cache is cleaned once and the step retried.

use crate::util::{self, Res};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every nvapi-family package cargo may hold feature-unified duplicates of.
const PACKAGES: [&str; 2] = ["nvapi", "nvapi-sys"];

fn stamp_path(root: &Path) -> PathBuf {
    root.join("target").join("xtask").join("nvapi.stamp")
}

/// Content fingerprint of the nvapi-rs submodule: the checked-out commit plus
/// its working-tree diff, so uncommitted RE work also invalidates the stamp —
/// the dimension the recorded-gitlink gate in `doctor` cannot see.
fn fingerprint(root: &Path) -> Res<String> {
    let submodule = root.join("nvapi-rs");
    let mut head = Command::new("git");
    head.args(["rev-parse", "HEAD"]).current_dir(&submodule);
    let head = util::capture(&mut head)
        .map_err(|error| format!("could not read nvapi-rs HEAD: {error}"))?;
    let mut status = Command::new("git");
    status
        .args(["status", "--porcelain"])
        .current_dir(&submodule);
    let status = util::capture(&mut status)
        .map_err(|error| format!("could not read the nvapi-rs working tree: {error}"))?;
    Ok(format!("{head}\n{status}"))
}

/// `cargo clean -p` for every nvapi-family package. Feature-unified duplicate
/// units of one package fall together, which is exactly the poisoned-unit
/// shape the stamp cannot always predict.
fn clean(root: &Path) -> Res<()> {
    let mut command = Command::new("cargo");
    command.arg("clean").current_dir(root);
    for package in PACKAGES {
        command.args(["-p", package]);
    }
    util::run(&mut command)
}

fn write_stamp(root: &Path, print: &str) -> Res<()> {
    let stamp = stamp_path(root);
    let parent = stamp
        .parent()
        .expect("the stamp always has a target/xtask parent");
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", stamp.display()))?;
    std::fs::write(&stamp, print)
        .map_err(|error| format!("could not write {}: {error}", stamp.display()))
}

/// Cleans the nvapi cargo cache whenever the submodule content changed since
/// the last xtask-driven run; a fast no-op while the stamp matches.
pub fn ensure_fresh(root: &Path) -> Res<()> {
    let print = fingerprint(root)?;
    if std::fs::read_to_string(stamp_path(root)).is_ok_and(|recorded| recorded == print) {
        return Ok(());
    }
    println!(
        "  [..]   nvapi-rs content changed since the last xtask run; refreshing its cargo cache"
    );
    clean(root)?;
    write_stamp(root, &print)?;
    println!("  [ok]   nvapi build cache refreshed");
    Ok(())
}

/// Runs a compile-bearing cargo step, cleaning the nvapi cache and retrying
/// once when the failure carries the stale-metadata signature; every other
/// failure passes through unchanged. Not for interactive targets (`xtask run`
/// launches user-facing binaries whose stderr must stay directly inherited).
pub fn run_guarded(root: &Path, command: &mut Command) -> Res<()> {
    let mut log = String::new();
    if let Err(message) = util::run_capture_stderr(command, &mut log) {
        if !is_stale_signature(&log) {
            return Err(message);
        }
        util::warn("name-resolution errors against `nvapi` — the stale-cache signature");
        util::hint("cleaning the nvapi build cache and retrying once");
        clean(root)?;
        write_stamp(root, &fingerprint(root)?)?;
        util::run_capture_stderr(command, &mut log)?;
    }
    Ok(())
}

/// True for rustc name-resolution failures that also mention the nvapi crate.
/// The stamp guard cannot catch a poisoned unit whose fingerprint still
/// matches, so this signature is the backstop.
fn is_stale_signature(stderr: &str) -> bool {
    let unresolved = ["error[E0425]", "error[E0432]", "error[E0599]"]
        .iter()
        .any(|code| stderr.contains(code));
    unresolved && stderr.contains("nvapi")
}

#[cfg(test)]
mod tests {
    #[test]
    fn signature_needs_both_a_resolution_error_and_nvapi() {
        assert!(super::is_stale_signature(
            "error[E0599]: no method named `volt_devices` found for reference `&nvapi::hi::Gpu`"
        ));
        assert!(super::is_stale_signature(
            "error[E0425]: cannot find function `clear_status_error` in crate `::nvapi`"
        ));
        assert!(super::is_stale_signature(
            "error[E0432]: unresolved import `nvapi::hi::ClkVfRawRecord`"
        ));
    }

    #[test]
    fn other_failures_do_not_match() {
        assert!(!super::is_stale_signature(
            "error[E0599]: no method named `foo` found for struct `Bar`"
        ));
        assert!(!super::is_stale_signature(
            "error: could not compile `nvoc-core` due to 11 previous errors"
        ));
        assert!(!super::is_stale_signature("warning: unused variable"));
    }
}
