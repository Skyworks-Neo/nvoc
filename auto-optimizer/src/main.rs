#![allow(
    unused_crate_dependencies,
    clippy::type_complexity,
    clippy::too_many_arguments
)]
mod arg_help;
mod autoscan_config;
mod basic_func;
mod conv;
mod error;
mod human;
mod nvidia_gpu_type;
mod oc_get_set_function_nvapi;
mod oc_get_set_function_nvml;
mod oc_profile_function;
mod oc_scanner;
mod platform;
mod types;

use anyhow::Result;
use nvml_wrapper::Nvml;
use std::io::{self, Write};
use std::process::exit;

use self::conv::ConvertEnum;
use self::types::*;
use crate::basic_func::*;
use crate::error::check_single_dash_args;
use crate::oc_get_set_function_nvapi::*;
use crate::oc_profile_function::*;
use crate::oc_scanner::*;
use crate::platform::is_elevated;

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

fn main_result() -> Result<i32, Box<dyn std::error::Error>> {
    let app = arg_help::get_arguments();
    check_single_dash_args(&app)?;
    let matches = app.get_matches();
    let exit_code = 0;

    let nvml_init_result = Nvml::init();
    let nvapi_init_result = nvapi_hi::initialize();

    if let Err(e) = &nvml_init_result {
        eprintln!("Warning: NVML init failed: {}", e);
    }
    if let Err(e) = &nvapi_init_result {
        eprintln!("Warning: NvAPI init failed: {}", e);
    }
    if nvml_init_result.is_err() && nvapi_init_result.is_err() {
        return Err("Both NVML and NvAPI initialization failed".into());
    }

    let oformat = matches
        .get_one::<String>("oformat")
        .map(|s| OutputFormat::from_str(s.as_str()))
        .unwrap()?;

    // Build GPU selector from the --gpu argument (CLI-agnostic after this point).
    let selector = GpuSelector::from_clap(matches.get_many::<String>("gpu"));

    // Enumerate both backends once, then resolve the selection upfront.
    // Handlers receive already-selected handles and do not filter themselves.
    let nvapi_all: Option<Vec<nvapi_hi::Gpu>> = if nvapi_init_result.is_ok() {
        get_sorted_gpus().ok()
    } else {
        None
    };

    let nvml_ref = nvml_init_result.as_ref().ok();

    let nvml_ids_all: Vec<u32> = nvml_ref
        .and_then(|nvml| get_sorted_gpu_ids_nvml(nvml).ok())
        .unwrap_or_default();

    // Pre-select for the NVAPI path (empty when NVAPI is unavailable).
    let nvapi_selected: Vec<&nvapi_hi::Gpu> = nvapi_all
        .as_deref()
        .and_then(|all| select_gpus(all, &selector).ok())
        .unwrap_or_default();

    // Pre-select for the NVML path (empty when NVML is unavailable).
    let nvml_selected: Vec<u32> = select_gpu_ids(&nvml_ids_all, &selector).unwrap_or_default();

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
                        handle_nvml_with_ids(&nvml_selected, sub_matches)?;
                    }
                    None => {
                        return Err("NVML backend unavailable".into());
                    }
                },
                Some(("nvml-cooler", sub_matches)) => match nvml_ref {
                    Some(_) => {
                        handle_nvml_cooler_with_ids(&nvml_selected, sub_matches)?;
                    }
                    None => {
                        return Err("NVML backend unavailable".into());
                    }
                },
                _ => {
                    if nvapi_init_result.is_err() {
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
                                match set_legacy_clocks_nvapi(gpu, core_mhz, mem_mhz) {
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
                                let gpu = single_gpu(&nvapi_selected)?;
                                handle_vfp_export(gpu, matches)?;
                            }
                            Some(("export_log", matches)) => {
                                export_vfp_from_log(matches)?;
                            }
                            Some(("import", matches)) => {
                                let gpu = single_gpu(&nvapi_selected)?;
                                handle_vfp_import(gpu, matches)?;
                            }
                            Some(("single_point_adj", matches)) => {
                                single_point_adj(&nvapi_selected, matches)?
                            }
                            Some(("pointwiseoc", matches)) => {
                                handle_pointwiseoc(&nvapi_selected, matches)?
                            }
                            Some(("fix_result", matches)) => {
                                let gpu = single_gpu(&nvapi_selected)?;
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
