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

    // GPU filter extracted once from CLI — no longer passed as clap ValuesRef
    let gpu_filter: Option<Vec<String>> = matches
        .get_many::<String>("gpu")
        .map(|v| v.cloned().collect());

    let oformat = matches
        .get_one::<String>("oformat")
        .map(|s| OutputFormat::from_str(s.as_str()))
        .unwrap()?;

    let nvml = nvml_init_result.as_ref().ok();

    match matches.subcommand() {
        Some(("info", sub_matches)) => {
            let output_file = sub_matches.get_one::<String>("output").map(|s| s.as_str());

            if nvapi_init_result.is_ok() {
                // NVAPI path (full info)
                let gpu_list = get_sorted_gpus()?;
                let selected = select_gpus(&gpu_list, gpu_filter.as_deref())?;
                if let Err(e) = handle_info(nvml, &selected, oformat, output_file) {
                    eprintln!("Error: {:?}", e);
                }
            } else if let Some(n) = nvml {
                // NVML-only fallback
                eprintln!("Note: NvAPI unavailable, using NVML-only info (limited detail).");
                let gpu_ids = get_sorted_gpu_ids_nvml(n)?;
                let selected_ids = select_gpu_ids(&gpu_ids, gpu_filter.as_deref())?;
                if let Err(e) = handle_info_nvml_only(n, &selected_ids, oformat) {
                    eprintln!("Error: {:?}", e);
                }
            }
        }
        Some(("list", _matches)) => {
            match nvml {
                Some(n) => {
                    if let Err(e) = handle_list(n) {
                        eprintln!("Error: {:?}", e);
                    }
                }
                None => {
                    eprintln!("Error: list requires NVML, but NVML init failed");
                }
            }
        }
        Some(("status", sub_matches)) => {
            if nvapi_init_result.is_ok() {
                let gpu_list = get_sorted_gpus()?;
                let selected = select_gpus(&gpu_list, gpu_filter.as_deref())?;
                if let Err(e) = handle_status(nvml, &selected, sub_matches, oformat) {
                    eprintln!("Error: {:?}", e);
                }
            } else if let Some(n) = nvml {
                eprintln!("Note: NvAPI unavailable, using NVML-only status.");
                let gpu_ids = get_sorted_gpu_ids_nvml(n)?;
                let selected_ids = select_gpu_ids(&gpu_ids, gpu_filter.as_deref())?;
                if let Err(e) = handle_status_nvml_only(n, &selected_ids, sub_matches, oformat) {
                    eprintln!("Error: {:?}", e);
                }
            } else {
                eprintln!("Error: status requires NvAPI or NVML, neither is available.");
            }
        }
        Some(("get", _matches)) => {
            if nvapi_init_result.is_ok() {
                let gpu_list = get_sorted_gpus()?;
                let selected = select_gpus(&gpu_list, gpu_filter.as_deref())?;
                if let Err(e) = handle_get(nvml, &selected, oformat) {
                    eprintln!("Error getting info: {:?}", e);
                }
            } else {
                eprintln!("Error: get requires NvAPI (for GPU settings), but NvAPI initialization failed.");
            }
        }
        Some(("reset", sub_matches)) => {
            if nvapi_init_result.is_err() {
                return Err("reset requires NvAPI, but NvAPI initialization failed".into());
            }
            let gpu_list = get_sorted_gpus()?;
            match sub_matches.subcommand() {
                Some(("nvml-cooler", cooler_matches)) => {
                    let selected = select_gpus(&gpu_list, gpu_filter.as_deref())?;
                    match nvml {
                        Some(n) => {
                            if let Err(e) = handle_reset_nvml_cooler(n, &selected, cooler_matches) {
                                eprintln!("Error: {:?}", e);
                            }
                        }
                        None => {
                            eprintln!("Error: nvml-cooler reset requires NVML, but NVML init failed");
                        }
                    }
                }
                _ => {
                    let selected = select_gpus(&gpu_list, gpu_filter.as_deref())?;
                    if let Err(e) = handle_reset(&selected, sub_matches) {
                        eprintln!("Error: {:?}", e);
                    }
                }
            }
        }
        Some(("set", sub_matches)) => {
            match sub_matches.subcommand() {
                Some(("nvml", nvml_matches)) => {
                    match nvml {
                        Some(n) => {
                            let gpu_ids = get_sorted_gpu_ids_nvml(n)?;
                            let selected_ids = select_gpu_ids(&gpu_ids, gpu_filter.as_deref())?;
                            handle_nvml_with_ids(n, &selected_ids, nvml_matches)?;
                        }
                        None => {
                            return Err("NVML backend unavailable".into());
                        }
                    }
                }
                Some(("nvml-cooler", nvml_matches)) => {
                    match nvml {
                        Some(n) => {
                            let gpu_ids = get_sorted_gpu_ids_nvml(n)?;
                            let selected_ids = select_gpu_ids(&gpu_ids, gpu_filter.as_deref())?;
                            handle_nvml_cooler_with_ids(n, &selected_ids, nvml_matches)?;
                        }
                        None => {
                            return Err("NVML backend unavailable".into());
                        }
                    }
                }
                _ => {
                    if nvapi_init_result.is_err() {
                        return Err("This subcommand requires NvAPI, but NvAPI initialization failed".into());
                    }
                    let Some(n) = nvml else {
                        return Err("This subcommand requires NVML for P-State info, but NVML initialization failed".into());
                    };

                    let gpus = get_sorted_gpus()?;
                    let gpus = select_gpus(&gpus, gpu_filter.as_deref())?;

                    handle_set_command(n, &gpus, sub_matches)?;

                    match sub_matches.subcommand() {
                        Some(("nvapi", _)) => (), // Handled by handle_set_command
                        Some(("nvapi-cooler", matches)) => {
                            handle_cooler_command(&gpus, matches)?;
                        }
                        Some(("legacy-clock", matches)) => {
                            let core_mhz = matches.get_one::<String>("core").unwrap().parse::<u32>()
                                .map_err(|_| "Invalid integer for core frequency")?;
                            let mem_mhz = matches.get_one::<String>("memory").unwrap().parse::<u32>()
                                .map_err(|_| "Invalid integer for memory frequency")?;
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
                                handle_vfp_export(gpu, matches)?;
                            }
                            Some(("export_log", matches)) => {
                                export_vfp_from_log(matches)?;
                            }
                            Some(("import", matches)) => {
                                let gpu = single_gpu(&gpus)?;
                                handle_vfp_import(gpu, matches)?;
                            }
                            Some(("single_point_adj", matches)) => {
                                single_point_adj(&gpus, matches)?
                            }
                            Some(("pointwiseoc", matches)) => {
                                handle_pointwiseoc(&gpus, matches)?
                            }
                            Some(("fix_result", matches)) => {
                                let gpu = single_gpu(&gpus)?;
                                fix_result(gpu, matches)?
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
