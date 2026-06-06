#![allow(
    unused_crate_dependencies,
    clippy::type_complexity,
    clippy::too_many_arguments
)]
mod arg_help;
mod autoscan_config;
mod oc_profile_function;
mod oc_scanner;
mod platform;
mod progressbar;
mod scan_strategy;
mod scan_support;

use anyhow::Result;
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

fn main_result() -> Result<i32, Box<dyn std::error::Error>> {
    let app = arg_help::get_arguments();
    arg_help::check_single_dash_args(&app)?;
    let matches = app.get_matches();
    init_scan_cli_color(matches.get_flag("no_color"));

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

    require_elevated()?;

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
        Some(("export-vfp-log", matches)) => {
            export_vfp_from_log(matches)?;
        }
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
        Some(("autoscan-vfp", matches)) => {
            if let Err(e) = autoscan_gpuboostv3(&nvapi_selected, matches) {
                eprintln!("Error in autoscan: {:?}", e);
            }
        }
        Some(("autoscan-vfp-legacy", matches)) => {
            if let Err(e) = autoscan_legacy(&nvapi_selected, matches) {
                eprintln!("Error in autoscan_legacy: {:?}", e);
            }
        }
        _ => unreachable!("unknown command"),
    }

    Ok(0)
}
