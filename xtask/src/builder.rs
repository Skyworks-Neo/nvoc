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
    }
    Ok(())
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
        for mut command in [self.sync_command(&cwd), self.pyinstaller_command(&cwd)] {
            let shown = util::display(&command);
            let log = std::fs::File::create(&log_path).map_err(|error| {
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
            .current_dir(cwd);
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
