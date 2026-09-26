//! `cargo xtask build`: workspace builds with the CUDA stressor generation as
//! the single knob.

use crate::args::BuildArgs;
use crate::nvapi_cache;
use crate::pathenv;
use crate::util::{self, Res};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    result?;
    if args.py_onefile {
        py_onefile(&util::repo_root())?;
        pathenv::ensure_onefile_on_user_path(&util::repo_root());
    }
    Ok(())
}

fn base(release: bool) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    if release {
        command.arg("--release");
        // Cargo.toml keeps a conservative cross-machine baseline
        // (codegen-units = 8); release builds through xtask scale it up to
        // this machine's parallelism via cargo's profile env override (higher
        // priority than the manifest, so the repo stays diff-free). A value
        // the user exported wins outright (=1 restores a max-optimization
        // build); dev profile is untouched (cargo defaults to 256 there).
        if std::env::var_os("CARGO_PROFILE_RELEASE_CODEGEN_UNITS").is_none()
            && let Ok(cores) = std::thread::available_parallelism()
        {
            command.env(
                "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
                cores.get().to_string(),
            );
        }
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
    tree.args(tree_feature_args(args.cuda));
    nvapi_cache::run_guarded(&root, &mut tree)?;

    // cuda11/none need a second invocation because feature unification would
    // merge the mutually exclusive cudarc cuda-11040/cuda-12090 units inside
    // one build. Within that invocation the optimizer and the stressor agree
    // on one feature set per generation, so their tail is a single call.
    if let Some(tail_args) = tail_feature_args(args.cuda) {
        let mut tail = base(args.release);
        tail.arg("-p").arg("nvoc-auto-optimizer");
        tail.arg("-p").arg("cli-stressor-cuda-rs");
        tail.args(tail_args);
        nvapi_cache::run_guarded(&root, &mut tail)?;
    }
    Ok(())
}

fn build_packages(args: &BuildArgs) -> Res<()> {
    let root = util::repo_root();
    for (packages, feature_args) in package_groups(&args.packages, args.cuda) {
        let mut command = base(args.release);
        for package in packages {
            command.arg("-p").arg(package);
        }
        command.args(feature_args);
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

/// Whole-tree `--exclude` list for the requested CUDA mode. cuda12 keeps every
/// member in one invocation: the stressor's direct member selection (default
/// cuda12) and the optimizer's bundled activation unify to the same
/// {cuda12, vulkan} unit, so the workspace step builds the standalone binary
/// itself. cuda11/none must drop both CUDA crates or the workspace selection's
/// default cuda12 features would poison the tail's cuda11 units.
pub fn workspace_excludes(cuda: CudaMode) -> Vec<&'static str> {
    match cuda {
        CudaMode::Cuda12 => vec![],
        CudaMode::Cuda11 | CudaMode::Off => vec!["cli-stressor-cuda-rs", "nvoc-auto-optimizer"],
    }
}

/// Extra feature arguments for the workspace invocation. The bare `vulkan`
/// name is unambiguous (only the stressor declares it), so cargo applies it
/// there alone; without it the member-selected stressor binary would build
/// without the Vulkan backend the `stressor-bundled` edge always expects.
pub fn tree_feature_args(cuda: CudaMode) -> Vec<&'static str> {
    match cuda {
        CudaMode::Cuda12 => vec!["--features", "vulkan"],
        CudaMode::Cuda11 | CudaMode::Off => vec![],
    }
}

/// Feature arguments for the second invocation that builds the optimizer and
/// the stressor together in cuda11/none modes; `None` means cuda12, whose
/// workspace step already covers both. `--no-default-features` applies to
/// both selected packages and the bare `cuda11` name is unambiguous across
/// them (only the stressor declares it), so the stressor side still receives
/// exactly its {cuda11, vulkan} set — vulkan arrives via `stressor-bundled`.
pub fn tail_feature_args(cuda: CudaMode) -> Option<Vec<&'static str>> {
    match cuda {
        CudaMode::Cuda12 => None,
        CudaMode::Cuda11 => Some(vec![
            "--no-default-features",
            "--features",
            "stressor-bundled-cuda11,cuda11",
        ]),
        CudaMode::Off => Some(vec![
            "--no-default-features",
            "--features",
            "stressor-external",
        ]),
    }
}

/// Feature arguments for one explicitly-selected package; empty means default
/// features.
fn package_feature_args(package: &str, cuda: CudaMode) -> Vec<&'static str> {
    match package {
        "cli-stressor-cuda-rs" => stressor_feature_args(cuda),
        "nvoc-auto-optimizer" => optimizer_feature_args(cuda).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Folds explicitly-selected packages into one cargo invocation per feature
/// signature (first-seen order): same-signature packages share a single
/// build's parallelism instead of each paying a separate cargo start-up and
/// full graph re-scan. When the optimizer and the stressor are selected
/// together in cuda11/none modes they share the merged tree-tail signature.
fn package_groups<'a>(
    packages: &'a [String],
    cuda: CudaMode,
) -> Vec<(Vec<&'a str>, Vec<&'static str>)> {
    let pair_signature = if packages.iter().any(|p| p == "nvoc-auto-optimizer")
        && packages.iter().any(|p| p == "cli-stressor-cuda-rs")
    {
        tail_feature_args(cuda)
    } else {
        None
    };
    let mut groups: Vec<(Vec<&'a str>, Vec<&'static str>)> = Vec::new();
    for package in packages {
        let signature = if pair_signature.is_some()
            && matches!(
                package.as_str(),
                "nvoc-auto-optimizer" | "cli-stressor-cuda-rs"
            ) {
            pair_signature
                .clone()
                .expect("pair_signature is Some whenever it is read")
        } else {
            package_feature_args(package, cuda)
        };
        if let Some(group) = groups
            .iter_mut()
            .find(|(_, existing)| *existing == signature)
        {
            group.0.push(package.as_str());
        } else {
            groups.push((vec![package.as_str()], signature));
        }
    }
    groups
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

/// One PyInstaller onefile job, mirroring release.yml exactly: a `uv sync`
/// with the component's dependency group followed by `uv run … pyinstaller`
/// against its spec. Both jobs run concurrently; interleaving their live
/// output would be unreadable, so each job writes to its own log file under
/// target/xtask/ and the tail is printed only on failure.
#[derive(Debug, Clone, Copy)]
struct PyJob {
    label: &'static str,
    dir: &'static str,
    group: &'static str,
    spec: &'static str,
    /// Artifact name from the spec (`name=`), without the platform suffix.
    exe: &'static str,
}

const PY_JOBS: [PyJob; 2] = [
    PyJob {
        label: "tui",
        dir: "tui",
        group: "dev",
        spec: "nvoc_tui.spec",
        exe: "nvoc-tui",
    },
    PyJob {
        label: "gui",
        dir: "gui",
        group: "build",
        spec: "nvoc_gui.spec",
        exe: "NVOC-GUI",
    },
];

impl PyJob {
    fn cwd(&self, root: &Path) -> PathBuf {
        root.join(self.dir)
    }

    fn log_path(&self, root: &Path) -> PathBuf {
        root.join("target")
            .join("xtask")
            .join(format!("pyinstaller-{}.log", self.label))
    }

    fn dist_exe(&self, root: &Path) -> PathBuf {
        let name = if cfg!(windows) {
            format!("{}.exe", self.exe)
        } else {
            self.exe.to_string()
        };
        self.cwd(root).join("dist").join(name)
    }

    /// Runs `uv sync` then `uv run … pyinstaller`, both with stdout+stderr
    /// redirected to the job log. Returns the dist artifact path on success.
    fn run(&self, root: &Path) -> Res<PathBuf> {
        let cwd = self.cwd(root);
        let log_path = self.log_path(root);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("py-onefile: could not create {}: {error}", parent.display())
            })?;
        }
        for (index, mut command) in [self.sync_command(&cwd), self.pyinstaller_command(&cwd)]
            .into_iter()
            .enumerate()
        {
            let shown = util::display(&command);
            // First command truncates, later commands append: the pyinstaller
            // step must not wipe the uv sync output above it — that output is
            // the only record of WHICH pynvoc wheel got frozen into the exe.
            let log = std::fs::OpenOptions::new()
                .write(true)
                .append(index > 0)
                .truncate(index == 0)
                .create(true)
                .open(&log_path)
                .map_err(|error| {
                    format!("py-onefile: could not open {}: {error}", log_path.display())
                })?;
            command.stdout(Stdio::from(log));
            // Share the same file via a fresh handle so both stdout and stderr
            // land in the log without needing a pipe drain thread.
            let stderr_handle = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .ok()
                .map(Stdio::from);
            if let Some(handle) = stderr_handle {
                command.stderr(handle);
            }
            let status = command
                .status()
                .map_err(|error| format!("failed to spawn `{shown}`: {error}"))?;
            if !status.success() {
                return Err(format!(
                    "pyinstaller [{label}] step `{shown}` exited with {status}; log: {log}",
                    label = self.label,
                    log = log_path.display()
                ));
            }
        }
        Ok(self.dist_exe(root))
    }

    fn sync_command(&self, cwd: &Path) -> Command {
        let mut command = Command::new("uv");
        command
            .args(["sync", "--locked", "--group", self.group, "--no-config"])
            .arg("--no-editable")
            // --no-editable installs pynvoc from uv's built-wheel cache, whose
            // key is the nvoc-python DIRECTORY mtime + version — edits under
            // python/ or src/ never invalidate it, so a stale wheel (old
            // wrapper without newly wrapped functions) shadows the editable
            // .pth and gets frozen into the executables. Refresh forces a
            // rebuild from the current tree every onefile build.
            .args(["--refresh-package", "pynvoc"])
            .current_dir(cwd);
        crate::util::apply_uv_index(&mut command);
        command
    }

    fn pyinstaller_command(&self, cwd: &Path) -> Command {
        let mut command = Command::new("uv");
        command
            .args(["run", "--locked", "--group", self.group, "--no-config"])
            .arg("--no-editable")
            .args(["pyinstaller", "--clean", "--noconfirm"])
            .arg(self.spec)
            .current_dir(cwd);
        command
    }
}

/// Packages the GUI and the TUI into onefile executables concurrently. The
/// first failing job aborts the step with its log tail; successful artifacts
/// are reported with their dist paths.
fn py_onefile(root: &Path) -> Res<()> {
    // Both specs bundle the pynvoc native extension, so it must be installed
    // into the uv environment before pyinstaller freezes it. Same invocation
    // as doctor::setup (and release.yml): from nvoc-python, not the root.
    util::step("pyinstaller prerequisite: pynvoc (maturin develop --release)");
    let mut maturin = Command::new("uv");
    maturin
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
        .current_dir(root.join("nvoc-python"));
    util::run(&mut maturin)?;

    util::step("pyinstaller: onefile GUI + TUI (concurrent)");
    let mut handles = Vec::new();
    for job in PY_JOBS {
        let root = root.to_path_buf();
        handles.push((job, std::thread::spawn(move || job.run(&root))));
    }
    let mut failure: Option<String> = None;
    for (job, handle) in handles {
        match handle.join() {
            Ok(Ok(dist)) => println!("  [{}] onefile artifact: {}", job.label, dist.display()),
            Ok(Err(message)) => {
                let tail = log_tail(&job.log_path(root));
                let message = if tail.is_empty() {
                    message
                } else {
                    format!("{message}\n--- {} log tail ---\n{tail}", job.label)
                };
                failure = failure.or(Some(message));
            }
            Err(_) => {
                failure = failure.or(Some(format!("pyinstaller [{}] worker panicked", job.label)));
            }
        }
    }
    match failure {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// Last few lines of a job log, for pointing at the actual pyinstaller error.
fn log_tail(path: &Path) -> String {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(30);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::CudaMode;

    #[test]
    fn py_jobs_run_in_their_component_directory() {
        let root = std::path::Path::new("/repo");
        for job in super::PY_JOBS {
            assert_eq!(job.cwd(root), root.join(job.dir));
            let cwd = job.cwd(root);
            for command in [job.sync_command(&cwd), job.pyinstaller_command(&cwd)] {
                assert_eq!(
                    command.get_current_dir(),
                    Some(job.cwd(root).as_path()),
                    "[{}] uv must run inside {}/",
                    job.label,
                    job.dir
                );
                assert!(
                    command.get_program().eq_ignore_ascii_case("uv"),
                    "[{}] must invoke uv directly",
                    job.label
                );
            }
        }
    }

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
    fn cuda12_builds_everything_in_one_invocation() {
        assert!(super::workspace_excludes(CudaMode::Cuda12).is_empty());
        assert_eq!(
            super::tree_feature_args(CudaMode::Cuda12),
            vec!["--features", "vulkan"]
        );
        assert_eq!(super::tail_feature_args(CudaMode::Cuda12), None);
    }

    #[test]
    fn cuda11_and_none_split_into_workspace_plus_merged_tail() {
        for mode in [CudaMode::Cuda11, CudaMode::Off] {
            let excludes = super::workspace_excludes(mode);
            assert!(excludes.contains(&"cli-stressor-cuda-rs"), "{mode:?}");
            assert!(excludes.contains(&"nvoc-auto-optimizer"), "{mode:?}");
            assert!(super::tree_feature_args(mode).is_empty(), "{mode:?}");
            let tail = super::tail_feature_args(mode).expect("tail invocation required");
            assert_eq!(tail.first(), Some(&"--no-default-features"), "{mode:?}");
        }
        assert_eq!(
            super::tail_feature_args(CudaMode::Cuda11),
            Some(vec![
                "--no-default-features",
                "--features",
                "stressor-bundled-cuda11,cuda11"
            ])
        );
        assert_eq!(
            super::tail_feature_args(CudaMode::Off),
            Some(vec![
                "--no-default-features",
                "--features",
                "stressor-external"
            ])
        );
    }

    #[test]
    fn cuda12_default_feature_packages_share_one_invocation() {
        let packages = ["nvoc-cli", "nvoc-auto-optimizer", "nvoc-srv"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let groups = super::package_groups(&packages, CudaMode::Cuda12);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].0,
            vec!["nvoc-cli", "nvoc-auto-optimizer", "nvoc-srv"]
        );
        assert!(groups[0].1.is_empty());
    }

    #[test]
    fn cuda11_optimizer_stressor_pair_shares_the_tail_signature() {
        let packages = ["nvoc-auto-optimizer", "cli-stressor-cuda-rs"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let groups = super::package_groups(&packages, CudaMode::Cuda11);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].1,
            super::tail_feature_args(CudaMode::Cuda11).unwrap()
        );
    }

    #[test]
    fn cuda11_lone_stressor_keeps_its_own_signature() {
        let packages = vec!["cli-stressor-cuda-rs".to_string()];
        let groups = super::package_groups(&packages, CudaMode::Cuda11);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1, super::stressor_feature_args(CudaMode::Cuda11));
    }
}
