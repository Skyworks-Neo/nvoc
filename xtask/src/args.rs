//! Command-line surface of the xtask orchestrator.
//!
//! Hand-rolled parsing on purpose: the orchestrator stays dependency-free so
//! `clippy --workspace -D warnings` and cold builds of the workspace never pay
//! for it.

use crate::builder::CudaMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Safe,
    GpuReadonly,
}

impl Tier {
    fn parse(value: &str) -> Result<Tier, String> {
        match value {
            "safe" => Ok(Tier::Safe),
            "gpu-readonly" => Ok(Tier::GpuReadonly),
            other => Err(format!(
                "--tier expects `safe` or `gpu-readonly`, got `{other}`\n\n{}",
                HELP
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTarget {
    Gui,
    Tui,
    Cli,
    Stressor,
}

impl RunTarget {
    fn parse(value: &str) -> Result<RunTarget, String> {
        match value {
            "gui" => Ok(RunTarget::Gui),
            "tui" => Ok(RunTarget::Tui),
            "cli" => Ok(RunTarget::Cli),
            "stressor" => Ok(RunTarget::Stressor),
            other => Err(format!(
                "run target must be one of gui | tui | cli | stressor, got `{other}`\n\n{}",
                HELP
            )),
        }
    }
}

#[derive(Debug)]
pub struct SetupArgs {
    pub install_missing: bool,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct BuildArgs {
    pub release: bool,
    pub packages: Vec<String>,
    pub cuda: CudaMode,
    /// After the cargo build, run PyInstaller onefile packaging for the GUI
    /// and the TUI concurrently (mirrors the release.yml invocations).
    pub py_onefile: bool,
}

#[derive(Debug)]
pub struct CiArgs {
    /// Auto-apply formatting and machine-applicable lint fixes before
    /// re-running the full gate.
    pub fmt: bool,
}

#[derive(Debug)]
pub enum Command {
    Help,
    Setup(SetupArgs),
    Build(BuildArgs),
    Check,
    Test(Tier),
    Run {
        target: RunTarget,
        passthrough: Vec<String>,
    },
    Ci(CiArgs),
}

pub const HELP: &str = "\
nvoc development orchestrator

USAGE:
    cargo xtask <COMMAND> [OPTIONS] [-- <passthrough args>]

COMMANDS:
    setup    Bootstrap a fresh clone: submodule, uv envs, pynvoc build, doctor report
    build    Build the workspace (CUDA stressor generation selectable)
    check    rustfmt + clippy + ruff, mirroring the ci.yml gates
    test     Run the test suite (tiered; GPU-write tests are never run)
    run      Launch a component: gui | tui | cli | stressor
    ci       check + test, the local mirror of the ci.yml non-GPU gate
             (`ci --fmt` force-applies fmt/lint fixes before the gate)

BUILD OPTIONS:
    --release            Build in release mode
    -p, --package <name> Build only the named crates (repeatable)
    --cuda <12|11|none>  CUDA stressor generation: 12 (default), 11 for
                         R470-era drivers, none to skip the stressor
    --py-onefile         Also package the GUI and TUI into onefile
                         executables with PyInstaller (both run concurrently;
                         same invocations as release.yml)

SETUP OPTIONS:
    --install-missing    Install missing tools (uv) instead of only reporting
    --dry-run            Print the commands setup would run

TEST OPTIONS:
    --tier <safe|gpu-readonly>
                         safe (default) is GPU-free; gpu-readonly runs the
                         ignored read-only NVAPI/NVML tests on live hardware.
                         GPU-write tests are never run by xtask; see
                         core/tests/gpu_write_conservative.rs.

CI OPTIONS:
    --fmt                Force-apply compliance first: cargo fmt, clippy --fix,
                         ruff format, ruff check --fix; the full gate then
                         re-runs, so unfixable violations still fail.

EXAMPLES:
    cargo xtask setup
    cargo xtask build --release
    cargo xtask build --cuda 11
    cargo xtask run cli -- get-info
    cargo xtask ci
";

pub fn parse(tokens: &[String]) -> Result<Command, String> {
    let Some((head, rest)) = tokens.split_first() else {
        return Ok(Command::Help);
    };
    match head.as_str() {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "setup" => parse_setup(rest),
        "build" => parse_build(rest),
        "check" => no_extra(rest, Command::Check),
        "test" => parse_test(rest),
        "run" => parse_run(rest),
        "ci" => parse_ci(rest),
        other => Err(format!("unknown command `{other}`\n\n{HELP}")),
    }
}

fn parse_ci(rest: &[String]) -> Result<Command, String> {
    let mut args = CiArgs { fmt: false };
    for token in rest {
        match token.as_str() {
            "--fmt" => args.fmt = true,
            other => return Err(format!("unknown ci option `{other}`\n\n{HELP}")),
        }
    }
    Ok(Command::Ci(args))
}

fn no_extra(rest: &[String], command: Command) -> Result<Command, String> {
    if rest.is_empty() {
        Ok(command)
    } else {
        Err(format!("this command takes no options\n\n{HELP}"))
    }
}

fn parse_setup(rest: &[String]) -> Result<Command, String> {
    let mut args = SetupArgs {
        install_missing: false,
        dry_run: false,
    };
    for token in rest {
        match token.as_str() {
            "--install-missing" => args.install_missing = true,
            "--dry-run" => args.dry_run = true,
            other => return Err(format!("unknown setup option `{other}`\n\n{HELP}")),
        }
    }
    Ok(Command::Setup(args))
}

fn parse_build(rest: &[String]) -> Result<Command, String> {
    let mut args = BuildArgs {
        release: false,
        packages: Vec::new(),
        cuda: CudaMode::Cuda12,
        py_onefile: false,
    };
    let mut iter = rest.iter();
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--release" => args.release = true,
            "--py-onefile" => args.py_onefile = true,
            "--package" | "-p" => {
                let name = iter
                    .next()
                    .ok_or_else(|| format!("--package needs a crate name\n\n{HELP}"))?;
                args.packages.push(name.clone());
            }
            "--cuda" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("--cuda needs a value (12 | 11 | none)\n\n{HELP}"))?;
                args.cuda = parse_cuda(value)?;
            }
            other => return Err(format!("unknown build option `{other}`\n\n{HELP}")),
        }
    }
    Ok(Command::Build(args))
}

fn parse_cuda(value: &str) -> Result<CudaMode, String> {
    match value {
        "12" => Ok(CudaMode::Cuda12),
        "11" => Ok(CudaMode::Cuda11),
        "none" => Ok(CudaMode::Off),
        other => Err(format!(
            "--cuda expects `12`, `11`, or `none`, got `{other}`\n\n{HELP}"
        )),
    }
}

fn parse_test(rest: &[String]) -> Result<Command, String> {
    let mut tier = Tier::Safe;
    let mut iter = rest.iter();
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--tier" => {
                let value = iter.next().ok_or_else(|| {
                    format!("--tier needs a value (safe | gpu-readonly)\n\n{HELP}")
                })?;
                tier = Tier::parse(value)?;
            }
            other => return Err(format!("unknown test option `{other}`\n\n{HELP}")),
        }
    }
    Ok(Command::Test(tier))
}

fn parse_run(rest: &[String]) -> Result<Command, String> {
    let Some((target, tail)) = rest.split_first() else {
        return Err(format!(
            "run needs a target: gui | tui | cli | stressor\n\n{HELP}"
        ));
    };
    let target = RunTarget::parse(target)?;
    let mut passthrough = Vec::new();
    let mut after_separator = false;
    for token in tail {
        if after_separator {
            passthrough.push(token.clone());
        } else if token == "--" {
            after_separator = true;
        } else {
            return Err(format!(
                "unexpected token `{token}` (component arguments must follow `--`)\n\n{HELP}"
            ));
        }
    }
    Ok(Command::Run {
        target,
        passthrough,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn empty_input_prints_help() {
        assert!(matches!(parse(&[]), Ok(Command::Help)));
        assert!(matches!(parse(&tokens(&["help"])), Ok(Command::Help)));
    }

    #[test]
    fn build_defaults_to_release_off_and_cuda12() {
        let cmd = parse(&tokens(&["build"])).unwrap();
        let Command::Build(args) = cmd else {
            panic!("expected build");
        };
        assert!(!args.release);
        assert!(args.packages.is_empty());
        assert_eq!(args.cuda, CudaMode::Cuda12);
    }

    #[test]
    fn build_accepts_cuda_generations() {
        for (value, expected) in [("12", CudaMode::Cuda12), ("11", CudaMode::Cuda11)] {
            let cmd = parse(&tokens(&["build", "--cuda", value])).unwrap();
            let Command::Build(args) = cmd else {
                panic!("expected build");
            };
            assert_eq!(args.cuda, expected);
        }
        let cmd = parse(&tokens(&["build", "--cuda", "none"])).unwrap();
        let Command::Build(args) = cmd else {
            panic!("expected build");
        };
        assert_eq!(args.cuda, CudaMode::Off);
    }

    #[test]
    fn build_accepts_py_onefile_flag() {
        let cmd = parse(&tokens(&["build", "--release", "--py-onefile"])).unwrap();
        let Command::Build(args) = cmd else {
            panic!("expected build");
        };
        assert!(args.release);
        assert!(args.py_onefile);
    }

    #[test]
    fn build_rejects_unknown_cuda_generation() {
        assert!(parse(&tokens(&["build", "--cuda", "10"])).is_err());
    }

    #[test]
    fn build_package_short_and_long_forms() {
        let cmd = parse(&tokens(&["build", "-p", "nvoc-cli"])).unwrap();
        let Command::Build(args) = cmd else {
            panic!("expected build");
        };
        assert_eq!(args.packages, vec!["nvoc-cli"]);
    }

    #[test]
    fn test_tier_defaults_to_safe() {
        let cmd = parse(&tokens(&["test"])).unwrap();
        assert!(matches!(cmd, Command::Test(Tier::Safe)));
        let cmd = parse(&tokens(&["test", "--tier", "gpu-readonly"])).unwrap();
        assert!(matches!(cmd, Command::Test(Tier::GpuReadonly)));
    }

    #[test]
    fn run_collects_passthrough_after_separator() {
        let cmd = parse(&tokens(&["run", "cli", "--", "get-info", "--json"])).unwrap();
        let Command::Run {
            target,
            passthrough,
        } = cmd
        else {
            panic!("expected run");
        };
        assert_eq!(target, RunTarget::Cli);
        assert_eq!(
            passthrough,
            vec!["get-info".to_string(), "--json".to_string()]
        );
    }

    #[test]
    fn run_rejects_args_before_separator() {
        assert!(parse(&tokens(&["run", "cli", "get-info"])).is_err());
        assert!(parse(&tokens(&["run"])).is_err());
    }

    #[test]
    fn unknown_command_fails() {
        assert!(parse(&tokens(&["nonsense"])).is_err());
    }

    #[test]
    fn ci_defaults_to_gate_only_and_accepts_fmt() {
        let cmd = parse(&tokens(&["ci"])).unwrap();
        let Command::Ci(args) = cmd else {
            panic!("expected ci");
        };
        assert!(!args.fmt);
        let cmd = parse(&tokens(&["ci", "--fmt"])).unwrap();
        let Command::Ci(args) = cmd else {
            panic!("expected ci");
        };
        assert!(args.fmt);
        assert!(parse(&tokens(&["ci", "--nonsense"])).is_err());
    }
}
