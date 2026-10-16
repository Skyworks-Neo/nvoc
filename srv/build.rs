//! Build script: embed the git commit hash the srv binaries were built from,
//! so a deployed service can identify itself (startup log line and GET
//! /version on the loopback control plane). The workspace version needs no
//! build script — `env!("CARGO_PKG_VERSION")` is available in normal code.

use std::process::Command;

fn main() {
    let hash = git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=NVOC_BUILD_GIT_HASH={hash}");

    // Track the package itself (source changes imply the binary may differ) and
    // HEAD (branch switches / detached checkouts with an unchanged tree).
    // Caveat: a commit that touches nothing under srv/ leaves the hash stale —
    // which is honest provenance, since the srv binary did not change.
    println!("cargo:rerun-if-changed=.");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    // Packaged refs (e.g. after `git fetch`) move commits without touching
    // HEAD; this escape hatch forces a re-resolve.
    println!("cargo:rerun-if-env-changed=NVOC_FORCE_BUILD_REHASH");
}

/// Short commit hash of the enclosing repository; `None` when git is absent,
/// the directory is not a repository, or git fails for any reason. Build must
/// never fail over metadata.
fn git_hash() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").ok()?)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() { None } else { Some(hash) }
}
