#![allow(dead_code)] 

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

pub use crate::basic_func::{
    GpuSelector,
    get_sorted_gpus, get_sorted_gpu_ids_nvml,
    select_gpus, select_gpu_ids,
    single_gpu,
    handle_info, handle_list, handle_status, handle_get,
    handle_reset, handle_reset_nvml_cooler, handle_reset_nvml_cooler_single_gpu,
    handle_set_command, handle_nvml_with_ids, handle_nvml_cooler_with_ids,
    print_all_nvml_gpu_uuid,
    local_time_hms, get_primary_monitor_resolution, get_second_largest_resolution,
    TestResolution,
};
pub use crate::oc_get_set_function_nvapi::*;
pub use crate::oc_get_set_function_nvml::*;
pub use crate::oc_profile_function::*;
pub use crate::oc_scanner::*;
