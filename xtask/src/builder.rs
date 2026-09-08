//! `cargo xtask build`: workspace builds with the CUDA stressor generation as
//! the single knob.

use crate::args::BuildArgs;
use crate::nvapi_cache;
use crate::pathenv;
use crate::util::{self, Res};
use std::process::Command;

/// Which CUDA driver generation the stressor targets. Mirrors the
/// `cuda11`/`cuda12` features of cli-stressor-cuda-rs, which are mutually
/// exclusive by compile_error!.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CudaMode {
    Cuda11,
    Cuda12,
    Off,
}

pub fn build(args: &BuildArgs) -> Res<()> {
    let result = if args.packages.is_empty() {
        build_tree(args)
    } else {
        build_packages(args)
    };
    // Convenience that only makes sense after a build that succeeded: put
    // target/release on the user PATH (idempotent, best-effort).
    if args.release {
        pathenv::ensure_release_on_user_path(&util::repo_root());
    }
    result
}

fn base(release: bool) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    if release {
        command.arg("--release");
    }
    command
}

fn build_tree(args: &BuildArgs) -> Res<()> {
    let root = util::repo_root();
    let label = match args.cuda {
        CudaMode::Cuda12 => "cuda12 (default)",
        CudaMode::Cuda11 => "cuda11 (R470-era drivers)",
        CudaMode::Off => "skipped",
    };
    util::step(&format!("building workspace (CUDA stressor: {label})"));

    let mut tree = base(args.release);
    tree.arg("--workspace");
    for exclude in workspace_excludes(args.cuda) {
        tree.args(["--exclude", exclude]);
    }
    nvapi_cache::run_guarded(&root, &mut tree)?;

    // auto-optimizer's default feature set already carries the cuda12
    // generation, so only the other two modes need an explicit override.
    if let Some(features) = optimizer_feature_args(args.cuda) {
        let mut optimizer = base(args.release);
        optimizer.arg("-p").arg("nvoc-auto-optimizer");
        optimizer.args(features);
        nvapi_cache::run_guarded(&root, &mut optimizer)?;
    }

    let mut stressor = base(args.release);
    stressor.arg("-p").arg("cli-stressor-cuda-rs");
    stressor.args(stressor_feature_args(args.cuda));
    nvapi_cache::run_guarded(&root, &mut stressor)
}

fn build_packages(args: &BuildArgs) -> Res<()> {
    let root = util::repo_root();
    for package in &args.packages {
        let mut command = base(args.release);
        command.arg("-p").arg(package);
        if package == "cli-stressor-cuda-rs" {
            command.args(stressor_feature_args(args.cuda));
        } else if package == "nvoc-auto-optimizer"
            && let Some(features) = optimizer_feature_args(args.cuda)
        {
            command.args(features);
        }
        nvapi_cache::run_guarded(&root, &mut command)?;
    }
    Ok(())
}

/// Feature arguments appended when building the CUDA stressor crate. Vulkan is
/// always on to match the `stressor-bundled` dependency edge and release.yml.
pub fn stressor_feature_args(cuda: CudaMode) -> Vec<&'static str> {
    match cuda {
        CudaMode::Cuda12 => vec!["--features", "vulkan"],
        CudaMode::Cuda11 => vec!["--no-default-features", "--features", "cuda11,vulkan"],
        CudaMode::Off => vec!["--no-default-features"],
    }
}

/// Whole-tree `--exclude` list for the requested CUDA mode.
pub fn workspace_excludes(cuda: CudaMode) -> Vec<&'static str> {
    match cuda {
        CudaMode::Cuda12 => vec!["cli-stressor-cuda-rs"],
        CudaMode::Cuda11 | CudaMode::Off => vec!["cli-stressor-cuda-rs", "nvoc-auto-optimizer"],
    }
}

/// Feature arguments for auto-optimizer when its default features do not
/// apply; `None` means the default feature set (bundled cuda12) already covers
/// the mode.
pub fn optimizer_feature_args(cuda: CudaMode) -> Option<Vec<&'static str>> {
    match cuda {
        CudaMode::Cuda12 => None,
        CudaMode::Cuda11 => Some(vec![
            "--no-default-features",
            "--features",
            "stressor-bundled-cuda11",
        ]),
        CudaMode::Off => Some(vec![
            "--no-default-features",
            "--features",
            "stressor-external",
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::CudaMode;

    #[test]
    fn stressor_features_mirror_the_dependency_edge() {
        assert_eq!(
            super::stressor_feature_args(CudaMode::Cuda12),
            vec!["--features", "vulkan"]
        );
        assert_eq!(
            super::stressor_feature_args(CudaMode::Cuda11),
            vec!["--no-default-features", "--features", "cuda11,vulkan"]
        );
        assert_eq!(
            super::stressor_feature_args(CudaMode::Off),
            vec!["--no-default-features"]
        );
    }

    #[test]
    fn only_cuda12_uses_default_optimizer_features() {
        assert_eq!(super::optimizer_feature_args(CudaMode::Cuda12), None);
        assert!(super::optimizer_feature_args(CudaMode::Cuda11).is_some());
        assert!(super::optimizer_feature_args(CudaMode::Off).is_some());
    }

    #[test]
    fn excludes_always_drop_the_stressor_crate() {
        for mode in [CudaMode::Cuda11, CudaMode::Cuda12, CudaMode::Off] {
            assert!(super::workspace_excludes(mode).contains(&"cli-stressor-cuda-rs"));
        }
        assert_eq!(
            super::workspace_excludes(CudaMode::Cuda12),
            vec!["cli-stressor-cuda-rs"]
        );
        assert_eq!(
            super::workspace_excludes(CudaMode::Off),
            vec!["cli-stressor-cuda-rs", "nvoc-auto-optimizer"]
        );
    }
}
