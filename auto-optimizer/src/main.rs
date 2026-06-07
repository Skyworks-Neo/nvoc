#![allow(
    unused_crate_dependencies,
    clippy::type_complexity,
    clippy::too_many_arguments
)]
mod arg_help;
mod autoscan_config;
mod cleanup;
mod oc_profile_function;
mod oc_scanner;
mod platform;
mod progressbar;
mod scan_log;
mod scan_strategy;
mod scan_support;

use anyhow::Result;
use cleanup::{AutoscanExit, cleanup_autoscan_exit};
use nvoc_core::{
    BackendSet, ConvertEnum, GpuSelector, GpuTarget, ResetVfpDeltas, VfpResetDomain,
    discover_targets, run, select_targets,
};
use oc_profile_function::{
    export_vfp_from_log, fix_result, handle_vfp_export, handle_vfp_import, sync_memory_pstate_as_p0,
};
use oc_scanner::{autoscan_gpuboostv3, autoscan_legacy};
use platform::is_elevated;
use progressbar::init_scan_cli_color;
use std::io::{self, Write};
use std::process::exit;

fn main() {
    match main_result() {
        Ok(code) => exit(code),
        Err(e) => {
            let _ = writeln!(io::stderr(), "{}", e);
            exit(1);
        }
    }
}

fn require_elevated() -> Result<(), Box<dyn std::error::Error>> {
    if is_elevated() {
        return Ok(());
    }
    #[cfg(windows)]
    return Err("This command requires Administrator privileges. \
         Please re-run nvoc from an elevated command prompt."
        .into());
    #[cfg(not(windows))]
    Err("This command requires root privileges. \
         Please re-run nvoc with sudo."
        .into())
}

fn single_target<'a>(targets: &'a [GpuTarget<'a>]) -> Result<&'a GpuTarget<'a>, nvoc_core::Error> {
    let mut targets = targets.iter();
    targets
        .next()
        .ok_or_else(|| nvoc_core::Error::from("no GPU selected"))
        .and_then(|target| match targets.next() {
            None => Ok(target),
            Some(..) => Err(nvoc_core::Error::from("multiple GPUs selected")),
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetRequirement {
    None,
    NvapiAny,
    NvapiSingle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandRequirements {
    target: TargetRequirement,
    elevation: bool,
}

fn command_requirements(command: &str) -> Option<CommandRequirements> {
    let target = match command {
        "export-vfp-log" => TargetRequirement::None,
        "export-vfp" | "import-vfp" | "sync-vfp-memory-pstate" | "fix-vfp-result" => {
            TargetRequirement::NvapiSingle
        }
        "reset-vfp" | "autoscan-vfp" | "autoscan-vfp-legacy" => TargetRequirement::NvapiAny,
        _ => return None,
    };
    let elevation = matches!(
        command,
        "reset-vfp"
            | "import-vfp"
            | "sync-vfp-memory-pstate"
            | "autoscan-vfp"
            | "autoscan-vfp-legacy"
    );

    Some(CommandRequirements { target, elevation })
}

fn main_result() -> Result<i32, Box<dyn std::error::Error>> {
    let app = arg_help::get_arguments();
    arg_help::check_single_dash_args(&app)?;
    let matches = app.get_matches();
    init_scan_cli_color(matches.get_flag("no_color"));

    let (command_name, _) = matches.subcommand().expect("subcommand required");
    let requirements = command_requirements(command_name).expect("known subcommand");

    if requirements.target == TargetRequirement::None {
        match matches.subcommand() {
            Some(("export-vfp-log", matches)) => {
                export_vfp_from_log(matches)?;
            }
            _ => unreachable!("offline command requirements did not match dispatch"),
        }
        return Ok(0);
    }

    let inventory = discover_targets(BackendSet::Both)
        .or_else(|both_err| {
            eprintln!("Warning: combined GPU discovery failed: {}", both_err);
            discover_targets(BackendSet::Nvapi)
        })
        .or_else(|nvapi_err| {
            eprintln!("Warning: NvAPI discovery failed: {}", nvapi_err);
            discover_targets(BackendSet::Nvml)
        })?;

    let selector = match matches.get_many::<String>("gpu") {
        Some(values) => GpuSelector::from_specs(values.cloned()),
        None => GpuSelector::all(),
    };

    let targets_all = inventory.targets();
    let selected_targets = select_targets(&targets_all, &selector).unwrap_or_default();
    let nvapi_selected: Vec<GpuTarget<'_>> = selected_targets
        .iter()
        .copied()
        .filter(|target| target.has_nvapi())
        .collect();

    if nvapi_selected.is_empty() {
        return Err("This command requires NvAPI, but NvAPI initialization failed".into());
    }

    if requirements.elevation {
        require_elevated()?;
    }

    match matches.subcommand() {
        Some(("reset-vfp", matches)) => {
            let domain = matches
                .get_one::<String>("vfp_domain")
                .map(|s| VfpResetDomain::from_str(s.as_str()))
                .transpose()?
                .unwrap_or(VfpResetDomain::All);
            for gpu in &nvapi_selected {
                run(gpu, ResetVfpDeltas { domain })?;
            }
        }
        Some(("export-vfp", matches)) => {
            let gpu = single_target(&nvapi_selected)?;
            handle_vfp_export(gpu, matches)?;
        }
        Some(("export-vfp-log", _)) => unreachable!("offline command was already dispatched"),
        Some(("import-vfp", matches)) => {
            let gpu = single_target(&nvapi_selected)?;
            handle_vfp_import(gpu, matches)?;
        }
        Some(("sync-vfp-memory-pstate", _matches)) => {
            let gpu = single_target(&nvapi_selected)?;
            sync_memory_pstate_as_p0(gpu)?;
        }
        Some(("fix-vfp-result", matches)) => {
            let gpu = single_target(&nvapi_selected)?;
            fix_result(gpu, matches)?
        }
        Some(("autoscan-vfp", matches)) => match autoscan_gpuboostv3(&nvapi_selected, matches) {
            Ok(()) => cleanup_autoscan_exit(&nvapi_selected, AutoscanExit::Success),
            Err(e) => {
                eprintln!("Error in autoscan: {:?}", e);
                cleanup_autoscan_exit(&nvapi_selected, AutoscanExit::Error);
                return Ok(1);
            }
        },
        Some(("autoscan-vfp-legacy", matches)) => match autoscan_legacy(&nvapi_selected, matches) {
            Ok(()) => cleanup_autoscan_exit(&nvapi_selected, AutoscanExit::Success),
            Err(e) => {
                eprintln!("Error in autoscan_legacy: {:?}", e);
                cleanup_autoscan_exit(&nvapi_selected, AutoscanExit::Error);
                return Ok(1);
            }
        },
        _ => unreachable!("unknown command"),
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirements(command: &str) -> CommandRequirements {
        command_requirements(command).expect("command requirements")
    }

    #[test]
    fn every_subcommand_has_requirements() {
        let app = arg_help::get_arguments();
        let missing: Vec<_> = app
            .get_subcommands()
            .filter_map(|subcommand| {
                let name = subcommand.get_name();
                command_requirements(name)
                    .is_none()
                    .then(|| name.to_string())
            })
            .collect();

        assert!(
            missing.is_empty(),
            "subcommands missing requirements: {missing:?}"
        );
    }

    #[test]
    fn export_vfp_log_is_offline() {
        assert_eq!(
            requirements("export-vfp-log"),
            CommandRequirements {
                target: TargetRequirement::None,
                elevation: false,
            }
        );
    }

    #[test]
    fn export_vfp_is_read_only_preflight() {
        assert_eq!(
            requirements("export-vfp"),
            CommandRequirements {
                target: TargetRequirement::NvapiSingle,
                elevation: false,
            }
        );
    }

    #[test]
    fn gpu_write_commands_require_elevation() {
        for command in [
            "reset-vfp",
            "import-vfp",
            "sync-vfp-memory-pstate",
            "autoscan-vfp",
            "autoscan-vfp-legacy",
        ] {
            assert!(
                requirements(command).elevation,
                "{command} should require elevation"
            );
        }
    }
}
