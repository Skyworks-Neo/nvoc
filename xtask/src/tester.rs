//! `cargo xtask test`: tiered test execution. The safe tier never touches a
//! GPU; the gpu-readonly tier runs only the `#[ignore]`d read-only suites that
//! gpu-ci.yml runs on hardware. GPU-write paths are deliberately unreachable.

use crate::args::Tier;
use crate::nvapi_cache;
use crate::util::{self, Res};
use std::path::Path;
use std::process::Command;

pub fn test(tier: Tier) -> Res<()> {
    match tier {
        Tier::Safe => safe(),
        Tier::GpuReadonly => gpu_readonly(),
    }
}

fn cargo_test(root: &Path, extra: &[&str]) -> Res<()> {
    let mut command = Command::new("cargo");
    command.arg("test").args(extra).current_dir(root);
    nvapi_cache::run_guarded(root, &mut command)
}

fn safe() -> Res<()> {
    util::step("tests: safe tier (no GPU access)");
    let root = util::repo_root();

    cargo_test(&root, &["-p", "nvoc-core", "--all-targets"])?;
    cargo_test(&root, &["-p", "nvoc-cli", "--all-targets"])?;
    cargo_test(&root, &["-p", "pynvoc", "--no-default-features"])?;
    cargo_test(&root, &["-p", "nvoc-auto-optimizer", "--all-targets"])?;

    util::run(&mut util::uv_run(
        &root,
        "nvoc-tui",
        &["pytest", "-q", "tui/tests/"],
    ))?;
    util::run(&mut util::uv_run(
        &root,
        "nvoc-gui",
        &["pytest", "-q", "gui/tests/"],
    ))?;
    util::run(&mut util::uv_run(
        &root,
        "pynvoc",
        &["pytest", "-q", "nvoc-python/tests/"],
    ))
}

fn gpu_readonly() -> Res<()> {
    util::step("tests: gpu-readonly tier (live hardware, read-only NVAPI/NVML)");
    println!("  this tier talks to a real GPU but never invokes write paths.");
    println!("  set NVOC_CORE_GPU_GROUND_TRUTH to enable ground-truth assertions");
    println!("  (the variable, when set, is inherited by the test processes).");
    let root = util::repo_root();

    let mut core = Command::new("cargo");
    core.args([
        "test",
        "--release",
        "-p",
        "nvoc-core",
        "--test",
        "gpu_readonly",
        "--",
        "--ignored",
        "--test-threads=1",
    ])
    .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut core)?;

    let mut optimizer = Command::new("cargo");
    optimizer
        .args([
            "test",
            "--release",
            "-p",
            "nvoc-auto-optimizer",
            "--",
            "--include-ignored",
            "gpu_readonly_",
        ])
        .current_dir(&root);
    nvapi_cache::run_guarded(&root, &mut optimizer)?;

    util::hint(
        "GPU-write suites are never run by xtask; see core/tests/gpu_write_conservative.rs \
         and docs/wiki/Safety-and-Recovery.md for the manual procedure",
    );
    Ok(())
}
