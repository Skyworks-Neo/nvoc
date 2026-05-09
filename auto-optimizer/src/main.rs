#![allow(unused_crate_dependencies)]
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

fn main() {
    match main_result() {
        Ok(code) => exit(code),
        Err(e) => {
            let _ = writeln!(io::stderr(), "{}", e);
            exit(1);
        }
    }
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


    let gpu = matches.get_many::<String>("gpu");
    let oformat = matches
        .get_one::<String>("oformat")
        .map(|s| OutputFormat::from_str(s.as_str()))
        .unwrap()?;

    match matches.subcommand() {
        Some(("info", _matches)) => {
            let output_file = _matches.get_one::<String>("output").map(|s| s.as_str());
            if let Err(e) = handle_info(gpu, oformat, output_file) {
                eprintln!("Error: {:?}", e);
            }
        }
        Some(("list", _matches)) => {
            match &nvml_init_result {
                Ok(nvml) => {
                    if let Err(e) = handle_list(nvml) {
                        eprintln!("Error: {:?}", e);
                    }
                }
                Err(e) => {
                    eprintln!("Error: list requires NVML, but NVML init failed: {}", e);
                }
            }
        }
        Some(("status", matches)) => {
            if let Err(e) = handle_status(&get_sorted_gpus()?, gpu, matches, oformat) {
                eprintln!("Error: {:?}", e);
            }
        }
        Some(("get", _matches)) => {
            if let Err(e) = handle_get(&get_sorted_gpus()?, gpu, oformat) {
                eprintln!("Error getting info: {:?}", e);
            }
        }
        Some(("reset", matches)) => {
            match matches.subcommand() {
                Some(("nvml-cooler", sub_matches)) => {
                    if let Err(e) = handle_reset_nvml_cooler(&get_sorted_gpus()?, gpu, sub_matches) {
                        eprintln!("Error: {:?}", e);
                    }
                }
                _ => {
                    if let Err(e) = handle_reset(&get_sorted_gpus()?, gpu, matches) {
                        eprintln!("Error: {:?}", e);
                    }
                }
            }
        }
        Some(("set", matches)) => {
            match matches.subcommand() {
                Some(("nvml", sub_matches)) => {
                    match &nvml_init_result {
                        Ok(nvml) => {
                            let gpu_ids = get_sorted_gpu_ids_nvml(nvml)?;
                            let selected_ids = select_gpu_ids(&gpu_ids, gpu)?;
                            handle_nvml_with_ids(&selected_ids, sub_matches)?;
                        }
                        Err(e) => {
                            return Err(format!("NVML backend unavailable: {}", e).into());
                        }
                    }
                }
                Some(("nvml-cooler", sub_matches)) => {
                    match &nvml_init_result {
                        Ok(nvml) => {
                            let gpu_ids = get_sorted_gpu_ids_nvml(nvml)?;
                            let selected_ids = select_gpu_ids(&gpu_ids, gpu)?;
                            handle_nvml_cooler_with_ids(&selected_ids, sub_matches)?;
                        }
                        Err(e) => {
                            return Err(format!("NVML backend unavailable: {}", e).into());
                        }
                    }
                }
                _ => {
                    if nvapi_init_result.is_err() {
                        return Err("This subcommand requires NvAPI, but NvAPI initialization failed".into());
                    }

                    let gpus = get_sorted_gpus()?;
                    let gpus = select_gpus(&gpus, gpu)?;

                    handle_set_command(&gpus, matches)?;

                    match matches.subcommand() {
                        Some(("nvapi", _)) => (), // Handled by handle_set_command
                        Some(("nvapi-cooler", matches)) => {
                            handle_cooler_command(&gpus, matches)?;
                        }
                        Some(("legacy-clock", matches)) => {
                            let core_mhz = matches.get_one::<String>("core").unwrap().parse::<u32>()
                                .map_err(|_| "Invalid integer for core frequency")?;
                            let mem_mhz = matches.get_one::<String>("memory").unwrap().parse::<u32>()
                                .map_err(|_| "Invalid integer for memory frequency")?;

                            // Reject values outside a generous but finite window.
                            // The internal scale factors are ×2000 (core) and ×1000 (mem);
                            // anything above 5 000 MHz would produce a u32 overflow even
                            // with saturating_mul, and no real Fermi/Kepler GPU runs there.
                            const MIN_LEGACY_MHZ: u32 = 100;
                            const MAX_LEGACY_MHZ: u32 = 5_000;
                            if !(MIN_LEGACY_MHZ..=MAX_LEGACY_MHZ).contains(&core_mhz) {
                                return Err(format!(
                                    "--core {} MHz is outside the supported legacy range {}–{} MHz",
                                    core_mhz, MIN_LEGACY_MHZ, MAX_LEGACY_MHZ
                                ).into());
                            }
                            if !(MIN_LEGACY_MHZ..=MAX_LEGACY_MHZ).contains(&mem_mhz) {
                                return Err(format!(
                                    "--memory {} MHz is outside the supported legacy range {}–{} MHz",
                                    mem_mhz, MIN_LEGACY_MHZ, MAX_LEGACY_MHZ
                                ).into());
                            }

                            for gpu in &gpus {
                                match set_legacy_clocks_nvapi(gpu, core_mhz, mem_mhz) {
                                    Ok(_) => println!("Legacy clock applied to GPU: Core = {} MHz, Mem = {} MHz", core_mhz, mem_mhz),
                                    Err(e) => eprintln!("Failed to apply legacy clock: {:?}", e),
                                }
                            }
                        }
                    Some(("vfp", matches)) => {
                        match matches.subcommand() {
                            Some(("export", matches)) => {
                                let gpu = single_gpu(&gpus)?;
                                handle_vfp_export(gpu, matches)?; // Call the export function
                            }
                            Some(("export_log", matches)) => {
                                export_vfp_from_log(matches)?; // Call the export function
                            }
                            Some(("import", matches)) => {
                                let gpu = single_gpu(&gpus)?;
                                handle_vfp_import(gpu, matches)?; // Call the import function
                            }
                            Some(("single_point_adj", matches)) => {
                                single_point_adj(&gpus, matches)? // Call the adjustment function
                            }
                            Some(("pointwiseoc", matches)) => {
                                handle_pointwiseoc(&gpus, matches)?
                            }
                            Some(("fix_result", matches)) => {
                                let gpu = single_gpu(&gpus)?;
                                fix_result(gpu, matches)? // Call the polishment function
                            }
                            Some(("autoscan", matches)) => {
                                if let Err(e) = autoscan_gpuboostv3(&gpus, matches) {
                                    eprintln!("Error in autoscan: {:?}", e);
                                }
                            }
                            Some(("autoscan_legacy", matches)) => {
                                if let Err(e) = autoscan_legacy(&gpus, matches) {
                                    eprintln!("Error in autoscan_legacy: {:?}", e);
                                }
                            }
                            _ => unreachable!("unknown command"),
                        }
                    }
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
