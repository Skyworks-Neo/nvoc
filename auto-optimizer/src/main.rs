#![allow(
    unused_crate_dependencies,
    clippy::type_complexity,
    clippy::too_many_arguments
)]
mod arg_help;
mod autoscan_config;
mod basic_func;
mod cli_types;
mod human;
mod oc_profile_function;
mod oc_scanner;
mod platform;

use anyhow::Result;
use basic_func::{
    check_single_dash_args, handle_cooler_command, handle_get, handle_info, handle_list,
    handle_nvml, handle_nvml_cooler, handle_pointwiseoc, handle_reset, handle_reset_nvml_cooler,
    handle_set_command, handle_status, single_point_adj,
};
use cli_types::OutputFormat;
use nvoc_core::{
    BackendSet, ConvertEnum, GpuSelector, GpuTarget, discover_targets, select_targets,
    set_nvapi_legacy_clocks,
};
use oc_profile_function::{
    export_vfp_from_log, fix_result, handle_vfp_export, handle_vfp_import, sync_memory_pstate_as_p0,
};
use oc_scanner::{autoscan_gpuboostv3, autoscan_legacy};
use platform::is_elevated;
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

/// Gate write-class subcommands behind the required OS privilege level.
/// Exits with a clear message rather than letting NVAPI/NVML fail opaquely.
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
    check_single_dash_args(&app)?;
    let matches = app.get_matches();
    let exit_code = 0;

    let inventory = discover_targets(BackendSet::Both)
        .or_else(|both_err| {
            eprintln!("Warning: combined GPU discovery failed: {}", both_err);
            discover_targets(BackendSet::Nvapi)
        })
        .or_else(|nvapi_err| {
            eprintln!("Warning: NvAPI discovery failed: {}", nvapi_err);
            discover_targets(BackendSet::Nvml)
        })?;

    let oformat = matches
        .get_one::<String>("oformat")
        .map(|s| OutputFormat::from_str(s.as_str()))
        .unwrap()?;

    // Build GPU selector from the --gpu argument (CLI-agnostic after this point).
    let selector = match matches.get_many::<String>("gpu") {
        Some(values) => GpuSelector::from_specs(values.cloned()),
        None => GpuSelector::all(),
    };

    let targets_all = inventory.targets();
    let selected_targets = select_targets(&targets_all, &selector).unwrap_or_default();
    let nvapi_selected: Vec<GpuTarget<'_>> = selected_targets
        .iter()
        .copied()
        .filter(|target| target.nvapi.is_some())
        .collect();
    let nvml_selected: Vec<u32> = selected_targets
        .iter()
        .filter(|target| target.nvml.is_some())
        .map(|target| target.id.0)
        .collect();
    let nvml_ref = selected_targets.iter().find_map(|target| target.nvml);

    match matches.subcommand() {
        Some(("info", _matches)) => {
            let output_file = _matches.get_one::<String>("output").map(|s| s.as_str());
            if let Err(e) = handle_info(
                &nvapi_selected,
                nvml_ref,
                &nvml_selected,
                oformat,
                output_file,
            ) {
                eprintln!("Error: {:?}", e);
            }
        }
        Some(("list", _matches)) => match nvml_ref {
            Some(nvml) => {
                if let Err(e) = handle_list(nvml) {
                    eprintln!("Error: {:?}", e);
                }
            }
            None => {
                eprintln!("Error: list requires NVML, but NVML init failed");
            }
        },
        Some(("status", matches)) => {
            if let Err(e) =
                handle_status(&nvapi_selected, nvml_ref, &nvml_selected, matches, oformat)
            {
                eprintln!("Error: {:?}", e);
            }
        }
        Some(("get", _matches)) => {
            if let Err(e) = handle_get(&nvapi_selected, oformat) {
                eprintln!("Error getting info: {:?}", e);
            }
        }
        Some(("reset", matches)) => {
            require_elevated()?;
            match matches.subcommand() {
                Some(("nvml-cooler", sub_matches)) => {
                    if let Err(e) = handle_reset_nvml_cooler(&nvapi_selected, sub_matches) {
                        eprintln!("Error: {:?}", e);
                    }
                }
                _ => {
                    if let Err(e) = handle_reset(&nvapi_selected, matches) {
                        eprintln!("Error: {:?}", e);
                    }
                }
            }
        }
        Some(("set", matches)) => {
            require_elevated()?;
            match matches.subcommand() {
                Some(("nvml", sub_matches)) => match nvml_ref {
                    Some(_) => {
                        handle_nvml(&selected_targets, sub_matches)?;
                    }
                    None => {
                        return Err("NVML backend unavailable".into());
                    }
                },
                Some(("nvml-cooler", sub_matches)) => match nvml_ref {
                    Some(_) => {
                        handle_nvml_cooler(&selected_targets, sub_matches)?;
                    }
                    None => {
                        return Err("NVML backend unavailable".into());
                    }
                },
                _ => {
                    if nvapi_selected.is_empty() {
                        return Err(
                            "This subcommand requires NvAPI, but NvAPI initialization failed"
                                .into(),
                        );
                    }

                    handle_set_command(&nvapi_selected, matches)?;

                    match matches.subcommand() {
                        Some(("nvapi", _)) => (), // Handled by handle_set_command
                        Some(("nvapi-cooler", matches)) => {
                            handle_cooler_command(&nvapi_selected, matches)?;
                        }
                        Some(("legacy-clock", matches)) => {
                            let core_mhz = *matches.get_one::<u32>("core").unwrap();
                            let mem_mhz = *matches.get_one::<u32>("memory").unwrap();
                            for gpu in &nvapi_selected {
                                match set_nvapi_legacy_clocks(gpu, core_mhz, mem_mhz) {
                                    Ok(_) => println!(
                                        "Legacy clock applied to GPU: Core = {} MHz, Mem = {} MHz",
                                        core_mhz, mem_mhz
                                    ),
                                    Err(e) => eprintln!("Failed to apply legacy clock: {:?}", e),
                                }
                            }
                        }
                        Some(("vfp", matches)) => match matches.subcommand() {
                            Some(("export", matches)) => {
                                let gpu = single_target(&nvapi_selected)?;
                                handle_vfp_export(gpu, matches)?;
                            }
                            Some(("export_log", matches)) => {
                                export_vfp_from_log(matches)?;
                            }
                            Some(("import", matches)) => {
                                let gpu = single_target(&nvapi_selected)?;
                                handle_vfp_import(gpu, matches)?;
                            }
                            Some(("sync_mem_pstate_as_p0", _matches)) => {
                                let gpu = single_target(&nvapi_selected)?;
                                sync_memory_pstate_as_p0(gpu)?;
                            }
                            Some(("single_point_adj", matches)) => {
                                single_point_adj(&nvapi_selected, matches)?
                            }
                            Some(("pointwiseoc", matches)) => {
                                handle_pointwiseoc(&nvapi_selected, matches)?
                            }
                            Some(("fix_result", matches)) => {
                                let gpu = single_target(&nvapi_selected)?;
                                fix_result(gpu, matches)?
                            }
                            Some(("autoscan", matches)) => {
                                if let Err(e) = autoscan_gpuboostv3(&nvapi_selected, matches) {
                                    eprintln!("Error in autoscan: {:?}", e);
                                }
                            }
                            Some(("autoscan_legacy", matches)) => {
                                if let Err(e) = autoscan_legacy(&nvapi_selected, matches) {
                                    eprintln!("Error in autoscan_legacy: {:?}", e);
                                }
                            }
                            _ => unreachable!("unknown command"),
                        },
                        None => (),
                        _ => unreachable!("unknown command"),
                    }
                }
            }
        }
        _ => unreachable!("unknown command"),
    }
    Ok(exit_code)
}
