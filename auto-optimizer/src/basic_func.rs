use super::cli_types::{OutputFormat, ResetSettings};
use super::human;
use nvapi_hi::{
    Celsius, Gpu, GpuSettings, KilohertzDelta, MicrovoltsDelta, PState, Percentage,
    allowable_result,
};
use std::io;

use clap::ArgMatches;
use nvoc_core::ConvertEnum;
use nvoc_core::Error;
use nvoc_core::{VfpResetDomain, reset_all_pstate_base_voltages, reset_vfp_deltas};
use time::{OffsetDateTime, format_description::parse};

pub fn local_time_hms() -> String {
    let format = match parse("[hour]:[minute]:[second]") {
        Ok(format) => format,
        Err(_) => return String::from("??:??:??"),
    };

    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());

    now.format(&format)
        .unwrap_or_else(|_| String::from("??:??:??"))
}

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use nvoc_core::{fetch_gpu_type, get_sorted_gpus};

pub fn print_all_nvml_gpu_uuid(nvml: &Nvml) -> Result<(), Box<dyn std::error::Error>> {
    // 初始化 NVML

    // 读取 GPU 个数
    let count = nvml.device_count()?;
    println!("Detected {} GPUs via NVML", count);

    // 遍历 GPU
    for i in 0..count {
        let device = nvml.device_by_index(i)?;
        let name = device.name()?;
        let uuid = device.uuid()?; // GPU UUID

        println!("GPU {}: {} UUID={}", i, name, uuid);
    }

    Ok(())
}

pub fn handle_list(nvml: &Nvml) -> Result<(), Error> {
    // Get the list of GPUs
    print_all_nvml_gpu_uuid(nvml).unwrap();
    let gpu_list = get_sorted_gpus()?;
    for (i, gpu) in gpu_list.iter().enumerate() {
        let info = gpu.info()?;
        if let Some(ids) = info.bus.bus.pci_ids() {
            println!(
                "GPU {}: ID:0x{:04X} bus:{:08x} - {:08x} - {:08x} - {:02x}",
                i,
                gpu.id(),
                ids.device_id,
                ids.subsystem_id,
                ids.ext_device_id,
                ids.revision_id,
            );
        } // ← Print something human-readable
    }

    // 旧版接口，没法用，太可惜了
    // let gpus = custom_wrapper::enumerate_raw_gpus()?;
    // for (gpu, handle) in gpus.iter().enumerate() {
    //     println!("GPU {} raw handle = {:?}", gpu, handle);
    //     let serial = get_board_info_raw(*handle)?;
    //     println!("GPU serial:{}", serial );
    // }
    Ok(())
}

/// Print GPU info. Uses the NVAPI path when `nvapi_gpus` is non-empty;
/// falls back to NVML when NVAPI is unavailable (e.g. server GPUs on Windows).
///
/// `nvapi_gpus` and `nvml_indices` are pre-selected by the caller — GPU
/// selection is no longer performed inside this function.
pub fn handle_info(
    nvapi_gpus: &[&Gpu],
    nvml: Option<&Nvml>,
    nvml_indices: &[u32],
    oformat: OutputFormat,
    output_file: Option<&str>,
) -> Result<(), Error> {
    if !nvapi_gpus.is_empty() {
        for (i, gpu) in nvapi_gpus.iter().enumerate() {
            println!("GPU {}: ID:0x{:04X}", i, gpu.id());
        }

        match oformat {
            OutputFormat::Human => {
                let mut success = 0usize;
                for gpu in nvapi_gpus {
                    let info = match gpu.info() {
                        Ok(info) => info,
                        Err(e) => {
                            eprintln!(
                                "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                gpu.id(),
                                e
                            );
                            continue;
                        }
                    };
                    human::print_info(gpu, &info);
                    let gpu_type = fetch_gpu_type(&info)?;
                    human::print_scan_separator();
                    println!(
                        "GPU {}: {} ({})====>[{}]",
                        info.id, info.name, info.codename, gpu_type
                    );
                    human::print_scan_separator();
                    println!();
                    success += 1;
                }
                if success == 0 {
                    return Err(Error::Custom(
                        "No selected GPU returned usable NvAPI info".to_string(),
                    ));
                }
            }
            OutputFormat::Json => {
                if let Some(file_path) = output_file {
                    let mut success = 0usize;
                    for gpu in nvapi_gpus {
                        let info = match gpu.info() {
                            Ok(info) => info,
                            Err(e) => {
                                eprintln!(
                                    "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                    gpu.id(),
                                    e
                                );
                                continue;
                            }
                        };
                        let gpu_file_path = format!("{}_gpu{}.json", file_path, info.id);
                        let file = std::fs::File::create(&gpu_file_path)?;
                        serde_json::to_writer_pretty(file, &info)?;
                        human::print_scan_separator();
                        println!(
                            "GPU {} information has been saved to: {}",
                            info.id, gpu_file_path
                        );
                        human::print_scan_separator();
                        success += 1;
                    }
                    if success == 0 {
                        return Err(Error::Custom(
                            "No selected GPU returned usable NvAPI info".to_string(),
                        ));
                    }
                } else {
                    let mut gpu_info = Vec::new();
                    for gpu in nvapi_gpus {
                        match gpu.info() {
                            Ok(info) => gpu_info.push(info),
                            Err(e) => eprintln!(
                                "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                gpu.id(),
                                e
                            ),
                        }
                    }
                    if gpu_info.is_empty() {
                        return Err(Error::Custom(
                            "No selected GPU returned usable NvAPI info".to_string(),
                        ));
                    }
                    serde_json::to_writer_pretty(io::stdout(), &gpu_info)?;
                }
            }
        }
    } else if let Some(nvml) = nvml {
        // NVML fallback: used when NVAPI is unavailable (e.g. server GPUs).
        print_nvml_info(nvml, nvml_indices)?;
    } else {
        return Err(Error::Custom(
            "No GPU backend available: both NvAPI and NVML are unavailable".to_string(),
        ));
    }

    Ok(())
}

/// Print basic GPU info via NVML (fallback when NVAPI is unavailable).
///
/// `selected_ids` uses the same `pci.bus * 256` encoding as `get_sorted_gpu_ids_nvml`.
/// An empty slice means "all devices".
fn print_nvml_info(nvml: &Nvml, selected_ids: &[u32]) -> Result<(), Error> {
    let count = nvml
        .device_count()
        .map_err(|e| Error::Custom(format!("NVML device_count failed: {:?}", e)))?;

    let mut shown = 0usize;
    for i in 0..count {
        let dev = nvml
            .device_by_index(i)
            .map_err(|e| Error::Custom(format!("NVML device_by_index({}) failed: {:?}", i, e)))?;
        let pci = dev
            .pci_info()
            .map_err(|e| Error::Custom(format!("NVML pci_info({}) failed: {:?}", i, e)))?;
        let bus_id = pci.bus.saturating_mul(256);

        if !selected_ids.is_empty() && !selected_ids.contains(&bus_id) {
            continue;
        }

        let name = dev.name().unwrap_or_else(|_| "<unknown>".to_string());
        let uuid = dev.uuid().unwrap_or_else(|_| "<unknown>".to_string());
        let vbios = dev
            .vbios_version()
            .unwrap_or_else(|_| "<unknown>".to_string());

        human::print_scan_separator();
        println!("GPU {} (NVML): {}", i, name);
        println!("  PCI Bus:        0x{:02X}", pci.bus);
        println!("  PCI Device:     0x{:04X}", pci.device);
        println!("  PCI Domain:     0x{:04X}", pci.domain);
        println!("  PCI Device ID:  0x{:08X}", pci.pci_device_id);
        match pci.pci_sub_system_id {
            Some(id) => println!("  PCI SubSys ID:  0x{:08X}", id),
            None => println!("  PCI SubSys ID:  N/A"),
        }
        println!("  UUID:           {}", uuid);
        println!("  VBIOS:          {}", vbios);
        human::print_scan_separator();
        println!();
        shown += 1;
    }

    if shown == 0 {
        return Err(Error::Custom(
            "No matching NVML device found for info query".to_string(),
        ));
    }
    Ok(())
}

/// Print GPU runtime status. Uses the NVAPI path when `nvapi_gpus` is non-empty;
/// falls back to NVML when NVAPI is unavailable.
///
/// Pre-selection is performed by the caller; this function does not filter GPUs.
pub fn handle_status(
    nvapi_gpus: &[&Gpu],
    nvml: Option<&Nvml>,
    nvml_indices: &[u32],
    matches: &ArgMatches,
    oformat: OutputFormat,
) -> Result<(), Error> {
    const NANOS_IN_SECOND: f64 = 1e9;

    let monitor = matches
        .get_one::<String>("monitor")
        .map(|s| f64::from_str(s.as_str()))
        .transpose()?
        .map(|v| Duration::new(v as u64, (v.fract() * NANOS_IN_SECOND) as u32));

    loop {
        if !nvapi_gpus.is_empty() {
            match oformat {
                OutputFormat::Human => {
                    let mut shown = false;
                    for &gpu in nvapi_gpus {
                        let mut set = None;

                        fn requires_set<'a>(
                            gpu: &Gpu,
                            set: &'a mut Option<GpuSettings>,
                        ) -> Result<&'a GpuSettings, Error> {
                            if set.is_some() {
                                return Ok(set.as_ref().unwrap());
                            }
                            Ok(set.get_or_insert(gpu.settings()?))
                        }

                        let status = match gpu.status() {
                            Ok(status) => status,
                            Err(e) => {
                                eprintln!(
                                    "Warning: failed to read status for GPU ID 0x{:04X}: {:?}",
                                    gpu.id(),
                                    e
                                );
                                continue;
                            }
                        };

                        human::print_status(&status);
                        human::print_settings(gpu, requires_set(gpu, &mut set)?);
                        if let Ok(info) = gpu.info()
                            && let Some(thresholds) = nvml.and_then(|n| {
                                nvoc_core::get_nvml_temperature_thresholds(n, info.id as u32)
                            })
                        {
                            println!("NVML Temperature Thresholds:");
                            for (name, value) in thresholds {
                                match value {
                                    Some(temp) => println!("  {:<16} : {} C", name, temp),
                                    None => println!("  {:<16} : N/A", name),
                                }
                            }
                        }
                        println!();
                        shown = true;
                        break;
                    }

                    if shown {
                        sleep(Duration::from_secs_f32(0.5));
                        return Ok(());
                    }

                    return Err(Error::Custom(
                        "No selected GPU returned usable NvAPI status".to_string(),
                    ));
                }
                OutputFormat::Json => {
                    let mut status = Vec::new();
                    for &gpu in nvapi_gpus {
                        match gpu.status() {
                            Ok(s) => status.push(s),
                            Err(e) => eprintln!(
                                "Warning: failed to read status for GPU ID 0x{:04X}: {:?}",
                                gpu.id(),
                                e
                            ),
                        }
                    }
                    if status.is_empty() {
                        return Err(Error::Custom(
                            "No selected GPU returned usable NvAPI status".to_string(),
                        ));
                    }
                    if monitor.is_some() {
                        let _ = serde_json::to_writer(io::stdout(), &status);
                        println!();
                    } else {
                        let _ = serde_json::to_writer_pretty(io::stdout(), &status);
                    }
                }
            }
        } else if let Some(nvml) = nvml {
            // NVML fallback: used when NVAPI is unavailable (e.g. server GPUs).
            print_nvml_status(nvml, nvml_indices)?;
        } else {
            return Err(Error::Custom(
                "No GPU backend available: both NvAPI and NVML are unavailable".to_string(),
            ));
        }

        if let Some(monitor) = monitor {
            sleep(monitor)
        } else {
            break;
        }
    }

    Ok(())
}

/// Print GPU runtime status via NVML (fallback when NVAPI is unavailable).
fn print_nvml_status(nvml: &Nvml, selected_ids: &[u32]) -> Result<(), Error> {
    let count = nvml
        .device_count()
        .map_err(|e| Error::Custom(format!("NVML device_count failed: {:?}", e)))?;

    let mut shown = 0usize;
    for i in 0..count {
        let dev = nvml
            .device_by_index(i)
            .map_err(|e| Error::Custom(format!("NVML device_by_index({}) failed: {:?}", i, e)))?;
        let pci = dev
            .pci_info()
            .map_err(|e| Error::Custom(format!("NVML pci_info({}) failed: {:?}", i, e)))?;
        let bus_id = pci.bus.saturating_mul(256);

        if !selected_ids.is_empty() && !selected_ids.contains(&bus_id) {
            continue;
        }

        let name = dev.name().unwrap_or_else(|_| "<unknown>".to_string());
        let temp = dev.temperature(TemperatureSensor::Gpu).ok();
        let core_clock = dev.clock_info(Clock::Graphics).ok();
        let mem_clock = dev.clock_info(Clock::Memory).ok();
        let power_mw = dev.power_usage().ok();
        let fan = dev.fan_speed(0).ok();
        let util = dev.utilization_rates().ok();
        let mem_info = dev.memory_info().ok();

        human::print_scan_separator();
        println!("GPU {} (NVML): {}", i, name);
        if let Some(t) = temp {
            println!("  Temperature  : {} C", t);
        }
        if let Some(c) = core_clock {
            println!("  Core Clock   : {} MHz", c);
        }
        if let Some(m) = mem_clock {
            println!("  Mem Clock    : {} MHz", m);
        }
        if let Some(p) = power_mw {
            println!("  Power Usage  : {:.2} W", p as f32 / 1000.0);
        }
        if let Some(f) = fan {
            println!("  Fan Speed    : {}%", f);
        }
        if let Some(u) = util {
            println!("  GPU Util     : {}%  Mem Util: {}%", u.gpu, u.memory);
        }
        if let Some(m) = mem_info {
            println!(
                "  VRAM         : {} / {} MiB",
                m.used / (1024 * 1024),
                m.total / (1024 * 1024)
            );
        }
        human::print_scan_separator();
        println!();
        shown += 1;
    }

    if shown == 0 {
        return Err(Error::Custom(
            "No matching NVML device found for status query".to_string(),
        ));
    }
    Ok(())
}

pub fn handle_get(gpus: &[&Gpu], oformat: OutputFormat) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?;

    match oformat {
        OutputFormat::Human => {
            for gpu in gpus.iter() {
                if let Ok(info) = gpu.info() {
                    human::print_scan_separator();
                    println!("GPU {}: {} ({})", info.id, info.name, info.codename);
                    human::print_scan_separator();
                }
                if let Ok(set) = gpu.settings() {
                    human::print_settings(gpu, &set);
                }
                if let Ok(info) = gpu.info() {
                    let gpu_id = info.id as u32;
                    let power_limit = nvoc_core::query_nvml_power_watts(&nvml, gpu_id);
                    let temp_thresholds = nvoc_core::get_nvml_temperature_thresholds(&nvml, gpu_id);
                    let pstate_info = nvoc_core::get_nvml_pstate_info(&nvml, gpu_id);
                    let app_clocks =
                        nvoc_core::get_nvml_supported_applications_clocks(&nvml, gpu_id);
                    let min_max_fan_speed = nvoc_core::get_nvml_min_max_fan_speed(&nvml, gpu_id);
                    if power_limit.is_some()
                        || temp_thresholds.is_some()
                        || pstate_info.is_some()
                        || app_clocks.is_some()
                        || min_max_fan_speed.is_some()
                    {
                        println!("NVML Settings:");
                        if let Some((min_w, current_w, max_w)) = power_limit {
                            println!(
                                "  Power Limit        : {:.2} W (Min: {:.2} W - Max: {:.2} W)",
                                current_w, min_w, max_w
                            );
                        }
                        if let Some(thresholds) = temp_thresholds {
                            println!("  Temperature Thresholds:");
                            for (name, value) in thresholds {
                                match value {
                                    Some(temp) => println!("    {:<16} : {} C", name, temp),
                                    None => println!("    {:<16} : N/A", name),
                                }
                            }
                        }
                        if let Some((min_fan, max_fan)) = min_max_fan_speed {
                            println!("  Fan Speed Range    : {}% - {}%", min_fan, max_fan);
                        }
                        if let Some(pstates) = pstate_info {
                            println!("  Supported P-States:");
                            for (pstate, min_core, max_core, min_mem, max_mem) in pstates {
                                let pstate_str = nvoc_core::nvml_pstate_to_str(pstate);
                                println!("    {}:", pstate_str);
                                println!(
                                    "      Core Clock Range   : {} MHz - {} MHz",
                                    min_core, max_core
                                );
                                println!(
                                    "      Mem Clock Range    : {} MHz - {} MHz",
                                    min_mem, max_mem
                                );

                                let core_offset =
                                    nvoc_core::get_nvml_core_clock_vf_offset(&nvml, gpu_id, pstate);
                                let mem_offset =
                                    nvoc_core::get_nvml_mem_clock_vf_offset(&nvml, gpu_id, pstate);
                                if let Some(c) = core_offset {
                                    println!("      Core Clock Offset  : {} MHz", c);
                                }
                                if let Some(m) = mem_offset {
                                    println!("      Mem Clock Offset   : {} MHz", m);
                                }
                            }
                        } else {
                            // Fallback if pstate info is unsupported
                            let core_offset = nvoc_core::get_nvml_core_clock_vf_offset(
                                &nvml,
                                gpu_id,
                                nvml_wrapper::enum_wrappers::device::PerformanceState::Zero,
                            );
                            let mem_offset = nvoc_core::get_nvml_mem_clock_vf_offset(
                                &nvml,
                                gpu_id,
                                nvml_wrapper::enum_wrappers::device::PerformanceState::Zero,
                            );
                            if let Some(c) = core_offset {
                                println!("  Core Clock Offset (P0) : {} MHz", c);
                            }
                            if let Some(m) = mem_offset {
                                println!("  Mem Clock Offset (P0)  : {} MHz", m);
                            }
                        }
                        if let Some(clocks) = app_clocks {
                            if !clocks.is_empty() {
                                println!("  Supported Applications Clocks:");
                                for (mem_clk, mut gfx_clocks) in clocks {
                                    if gfx_clocks.is_empty() {
                                        continue;
                                    }
                                    gfx_clocks.sort_unstable();
                                    let mode_count = gfx_clocks.len();
                                    if mode_count == 1 {
                                        println!(
                                            "    Memory {:>5} MHz : {} MHz (1 mode)",
                                            mem_clk, gfx_clocks[0]
                                        );
                                    } else {
                                        let min_clk = gfx_clocks[0];
                                        let max_clk = gfx_clocks[mode_count - 1];
                                        let step = gfx_clocks[1] - gfx_clocks[0];
                                        let step_str = match step {
                                            12 => "12.5".to_string(),
                                            7 => "7.5".to_string(),
                                            _ => step.to_string(),
                                        };
                                        println!(
                                            "    Memory {:>5} MHz : {:>4} MHz ~ {:>4} MHz (Step: {} MHz, {} modes)",
                                            mem_clk, min_clk, max_clk, step_str, mode_count
                                        );
                                    }
                                }
                            } else {
                                // 简洁模式：只列出支持的显存频率，不显示具体的 GPU 时钟频率
                                let mem_clocks: Vec<_> =
                                    clocks.iter().map(|(mem_clk, _)| *mem_clk).collect();
                                if !mem_clocks.is_empty() {
                                    println!(
                                        "  Supported Applications Clocks: {} MHz",
                                        mem_clocks
                                            .iter()
                                            .map(|c| c.to_string())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        OutputFormat::Json => {
            let mut settings = Vec::new();
            for gpu in gpus {
                match gpu.settings() {
                    Ok(s) => settings.push(s),
                    Err(e) => eprintln!(
                        "Warning: failed to read settings for GPU ID 0x{:04X}: {:?}",
                        gpu.id(),
                        e
                    ),
                }
            }
            if settings.is_empty() {
                return Err(Error::Custom(
                    "No selected GPU returned usable NvAPI settings".to_string(),
                ));
            }
            let _ = serde_json::to_writer_pretty(io::stdout(), &settings);
        }
    }

    Ok(())
}

pub fn handle_reset(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let parse_settings = |key: &str| -> Result<Vec<ResetSettings>, Error> {
        matches
            .get_many::<String>(key)
            .map(|vals| {
                vals.map(|s| ResetSettings::from_str(s.as_str()))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    };

    let vfp_domain_explicit = matches
        .value_source("vfp_domain")
        .map(|s| s == clap::parser::ValueSource::CommandLine)
        .unwrap_or(false);

    let mut settings = if matches.get_many::<String>("setting").is_some()
        || matches.get_many::<String>("domain").is_some()
    {
        let mut merged = parse_settings("setting")?;
        for item in parse_settings("domain")? {
            if !merged.contains(&item) {
                merged.push(item);
            }
        }
        merged
    } else if vfp_domain_explicit {
        // If only --vfp-domain is given, interpret reset target as VFP deltas.
        vec![ResetSettings::VfpDeltas]
    } else {
        ResetSettings::possible_values_typed().to_vec()
    };

    if settings.is_empty() {
        settings = ResetSettings::possible_values_typed().to_vec();
    }

    let explicit = matches.get_many::<String>("setting").is_some()
        || matches.get_many::<String>("domain").is_some()
        || vfp_domain_explicit;

    let vfp_reset_domain = matches
        .get_one::<String>("vfp_domain")
        .map(|s| VfpResetDomain::from_str(s.as_str()))
        .transpose()?
        .unwrap_or(VfpResetDomain::All);

    fn warn_result<E: Into<nvapi_hi::Error>>(
        r: Result<(), E>,
        setting: ResetSettings,
        explicit: bool,
    ) -> Result<(), Error> {
        let reset_error = |err| Error::Custom(format!("Reset {:?} failed: {}", setting, err));
        match (
            allowable_result(r).map_err(|e| reset_error(e.into()))?,
            explicit,
        ) {
            (Err(e), true) => Err(reset_error(e)),
            _ => Ok(()),
        }
    }

    for gpu in gpus {
        let info = gpu.info()?;

        for &setting in &settings {
            match setting {
                ResetSettings::VoltageBoost => {
                    warn_result(gpu.set_voltage_boost(Percentage(0)), setting, explicit)?
                }
                ResetSettings::SensorLimits => warn_result(
                    gpu.set_sensor_limits(
                        info.sensor_limits
                            .iter()
                            .cloned()
                            .map(nvapi_hi::SensorThrottle::from_default),
                    ),
                    setting,
                    explicit,
                )?,
                ResetSettings::PowerLimits => warn_result(
                    gpu.set_power_limits(info.power_limits.iter().map(|info| info.default)),
                    setting,
                    explicit,
                )?,
                ResetSettings::CoolerLevels => {
                    warn_result(gpu.reset_cooler_levels(), setting, explicit)?
                }
                ResetSettings::VfpDeltas => {
                    warn_result(reset_vfp_deltas(gpu, vfp_reset_domain), setting, explicit)?
                }
                ResetSettings::VfpLock => warn_result(gpu.reset_vfp_lock(), setting, explicit)?,
                ResetSettings::PStateDeltas => {
                    let pstates = info.pstate_limits.iter().flat_map(|(&pstate, l)| {
                        l.iter()
                            .filter(|&(_, info)| info.frequency_delta.is_some())
                            .map(move |(&clock, _)| (pstate, clock))
                    });
                    warn_result(
                        gpu.inner().set_pstates(
                            pstates.map(|(pstate, clock)| (pstate, clock, KilohertzDelta(0))),
                        ),
                        setting,
                        explicit,
                    )?
                }
                ResetSettings::Overvolt => {
                    let gpu_type = fetch_gpu_type(&info);
                    match gpu_type {
                        Ok(ref t) if t.is_legacy_voltage() => {
                            // Maxwell / 9 系及更早：清零全部可编辑 pstate 的 Core baseVoltage delta
                            match reset_all_pstate_base_voltages(gpu) {
                                Ok(_) => {}
                                Err(e) if explicit => return Err(e),
                                Err(e) => {
                                    eprintln!("Warning: Overvolt reset failed (non-fatal): {}", e)
                                }
                            }
                        }
                        _ => {
                            // Pascal 及以后使用 VoltRails boost，Overvolt 归零由 VoltageBoost 分支负责
                            println!(
                                "Overvolt reset: not applicable for this GPU generation (use VoltageBoost reset instead)."
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn handle_set_command(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    match matches.subcommand() {
        Some(("nvapi", sub)) => handle_nvapi(gpus, sub)?,
        Some(("nvml", sub)) => handle_nvml(gpus, sub)?,
        Some(("nvml-cooler", sub)) => handle_nvml_cooler(gpus, sub)?,
        _ => {}
    }
    Ok(())
}

fn handle_nvapi(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    if let Some(&vboost) = matches.get_one::<u32>("vboost") {
        for gpu in gpus {
            gpu.set_voltage_boost(Percentage(vboost))?;
        }
    }
    if let Some(plimit) = matches.get_many::<u32>("plimit") {
        let plimit: Vec<_> = plimit.copied().map(Percentage).collect();
        for gpu in gpus {
            gpu.set_power_limits(plimit.iter().cloned())?;
        }
    }
    if let Some(tlimit) = matches.get_many::<i32>("tlimit") {
        let tlimit: Vec<_> = tlimit.copied().map(|v| Celsius(v).into()).collect();
        for gpu in gpus {
            gpu.set_sensor_limits(tlimit.iter().cloned())?;
        }
    }

    let nvapi_pstate = matches
        .get_one::<String>("pstate")
        .map(|s| PState::from_str(s.as_str()))
        .transpose()
        .map_err(|e| Error::from(format!("Invalid --pstate value: {}", e)))?
        .unwrap_or(PState::P0);

    if let Some(&delta_uv) = matches.get_one::<i32>("voltage_delta") {
        for gpu in gpus {
            nvoc_core::set_pstate_base_voltage(gpu, MicrovoltsDelta(delta_uv), nvapi_pstate)?;
        }
    }

    if let Some(&core_offset) = matches.get_one::<i32>("core_offset") {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match nvoc_core::set_pstate_clock_offset_preserve(
                gpu,
                nvapi_pstate,
                nvapi_hi::ClockDomain::Graphics,
                KilohertzDelta(core_offset),
            ) {
                Ok(_) => println!(
                    "Successfully applied NVAPI core offset {} kHz to GPU {} for PState {:?}",
                    core_offset, gpu_info.id, nvapi_pstate
                ),
                Err(e) => eprintln!(
                    "Failed to set NVAPI core offset for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if let Some(&mem_offset) = matches.get_one::<i32>("mem_offset") {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match nvoc_core::set_pstate_clock_offset_preserve(
                gpu,
                nvapi_pstate,
                nvapi_hi::ClockDomain::Memory,
                KilohertzDelta(mem_offset),
            ) {
                Ok(_) => println!(
                    "Successfully applied NVAPI mem offset {} kHz to GPU {} for PState {:?}",
                    mem_offset, gpu_info.id, nvapi_pstate
                ),
                Err(e) => eprintln!(
                    "Failed to set NVAPI mem offset for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if let Some(nvapi_pstate_lock_vals) = matches.get_many::<String>("pstate_lock") {
        let requested_pstates = nvapi_pstate_lock_vals
            .map(|s| s.as_str())
            .collect::<Vec<_>>();
        let first_pstate = nvoc_core::try_parse_nvml_pstate(requested_pstates[0])?;
        let second_pstate = if requested_pstates.len() >= 2 {
            nvoc_core::try_parse_nvml_pstate(requested_pstates[1])?
        } else {
            first_pstate
        };

        let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?;
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match nvoc_core::set_nvapi_pstate_lock(
                &nvml,
                gpu,
                gpu_info.id as u32,
                first_pstate,
                second_pstate,
            ) {
                Ok((range_label, min_lock_mhz, max_lock_mhz)) => println!(
                    "Successfully locked GPU {} to {} via NVAPI memory window {}-{} MHz",
                    gpu_info.id, range_label, min_lock_mhz, max_lock_mhz,
                ),
                Err(e) => eprintln!(
                    "Failed to lock GPU {} to NVAPI PState {}: {:?}",
                    gpu_info.id,
                    requested_pstates.join(" "),
                    e
                ),
            }
        }
    }

    if matches.get_one::<String>("locked_voltage").is_some()
        || matches.get_many::<String>("locked_core_clocks").is_some()
        || matches.get_many::<String>("locked_mem_clocks").is_some()
    {
        nvoc_core::handle_lock_vfp(gpus, matches, 0, false)?;
    }

    if matches.get_flag("reset_volt_locks") {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match gpu.reset_vfp_lock() {
                Ok(_) => println!("Successfully reset NVAPI volt lock on GPU {}", gpu_info.id),
                Err(e) => eprintln!(
                    "Failed to reset NVAPI volt lock for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if matches.get_flag("reset_core_clocks") {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match nvoc_core::reset_vfp_frequency_lock(gpu, nvapi_hi::ClockDomain::Graphics) {
                Ok(_) => println!(
                    "Successfully reset NVAPI core clocks lock on GPU {}",
                    gpu_info.id
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVAPI core clocks lock for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if matches.get_flag("reset_mem_clocks") {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match nvoc_core::reset_vfp_frequency_lock(gpu, nvapi_hi::ClockDomain::Memory) {
                Ok(_) => println!(
                    "Successfully reset NVAPI memory clocks lock on GPU {}",
                    gpu_info.id
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVAPI memory clocks lock for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if matches.get_flag("test_limit") {
        nvoc_core::handle_test_voltage_limits(gpus, matches)?;
    }

    Ok(())
}

pub fn handle_nvml_with_ids(gpu_ids: &[u32], matches: &ArgMatches) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?;
    let nvml_pstate_val = matches
        .get_one::<String>("pstate")
        .map(|s| s.as_str())
        .unwrap_or("0");
    let target_nvml_pstate = nvoc_core::parse_nvml_pstate(nvml_pstate_val);

    if let Some(&core_offset) = matches.get_one::<i32>("core_offset") {
        for &gpu_id in gpu_ids {
            match nvoc_core::set_nvml_core_clock_vf_offset(
                &nvml,
                gpu_id,
                core_offset,
                target_nvml_pstate,
            ) {
                Ok(_) => println!(
                    "Successfully applied NVML core offset {} MHz to GPU {} for PState {}",
                    core_offset, gpu_id, nvml_pstate_val
                ),
                Err(e) => eprintln!("Failed to set NVML core offset for GPU {}: {:?}", gpu_id, e),
            }
        }
    }

    if let Some(&mem_offset) = matches.get_one::<i32>("mem_offset") {
        for &gpu_id in gpu_ids {
            match nvoc_core::set_nvml_mem_clock_vf_offset(
                &nvml,
                gpu_id,
                mem_offset,
                target_nvml_pstate,
            ) {
                Ok(_) => println!(
                    "Successfully applied NVML mem offset {} MHz to GPU {} for PState {}",
                    mem_offset, gpu_id, nvml_pstate_val
                ),
                Err(e) => eprintln!("Failed to set NVML mem offset for GPU {}: {:?}", gpu_id, e),
            }
        }
    }

    if let Some(&power_w) = matches.get_one::<u32>("power_limit") {
        for &gpu_id in gpu_ids {
            match nvoc_core::set_nvml_power_limit(&nvml, gpu_id, power_w) {
                Ok(_) => println!(
                    "Successfully applied NVML power limit {} W to GPU {}",
                    power_w, gpu_id
                ),
                Err(e) => eprintln!("Failed to set NVML power limit for GPU {}: {:?}", gpu_id, e),
            }
        }
    }

    if let Some(app_clocks) = matches.get_many::<u32>("locked_app_clocks") {
        let clocks: Vec<u32> = app_clocks.copied().collect();
        if clocks.len() == 2 {
            let mem_clock = clocks[0];
            let core_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match nvoc_core::set_nvml_applications_clocks(&nvml, gpu_id, mem_clock, core_clock)
                {
                    Ok(_) => println!(
                        "Successfully locked NVML app clocks (Mem: {}, Core: {}) to GPU {}",
                        mem_clock, core_clock, gpu_id
                    ),
                    Err(e) => {
                        eprintln!("Failed to lock NVML app clocks for GPU {}: {:?}", gpu_id, e)
                    }
                }
            }
        } else {
            eprintln!(
                "Invalid arguments for --locked-app-clocks, expected 2 arguments (MEM_MHZ CORE_MHZ)"
            );
        }
    }

    if matches.get_flag("reset_app_clocks") {
        for &gpu_id in gpu_ids {
            match nvoc_core::reset_nvml_applications_clocks(&nvml, gpu_id) {
                Ok(_) => println!(
                    "Successfully reset NVML applications clocks to default on GPU {}",
                    gpu_id
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML applications clocks for GPU {}: {:?}",
                    gpu_id, e
                ),
            }
        }
    }

    if let Some(locked_core_clocks) = matches.get_many::<u32>("locked_core_clocks") {
        let clocks: Vec<u32> = locked_core_clocks.copied().collect();
        if clocks.len() == 2 {
            let min_clock = clocks[0];
            let max_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match nvoc_core::set_nvml_core_locked_clocks(&nvml, gpu_id, min_clock, max_clock) {
                    Ok(_) => println!(
                        "Successfully locked NVML core clocks (Min: {}, Max: {}) to GPU {}",
                        min_clock, max_clock, gpu_id
                    ),
                    Err(e) => eprintln!(
                        "Failed to lock NVML core clocks for GPU {}: {:?}",
                        gpu_id, e
                    ),
                }
            }
        } else {
            eprintln!(
                "Invalid arguments for --locked-core-clocks, expected 2 arguments (MIN_MHZ MAX_MHZ)"
            );
        }
    }

    if matches.get_flag("reset_core_clocks") {
        for &gpu_id in gpu_ids {
            match nvoc_core::reset_nvml_core_locked_clocks(&nvml, gpu_id) {
                Ok(_) => println!(
                    "Successfully reset NVML core locked clocks to GPU {}",
                    gpu_id
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML core locked clocks for GPU {}: {:?}",
                    gpu_id, e
                ),
            }
        }
    }

    if let Some(locked_mem_clocks) = matches.get_many::<u32>("locked_mem_clocks") {
        let clocks: Vec<u32> = locked_mem_clocks.copied().collect();
        if clocks.len() == 2 {
            let min_clock = clocks[0];
            let max_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match nvoc_core::set_nvml_mem_locked_clocks(&nvml, gpu_id, min_clock, max_clock) {
                    Ok(_) => println!(
                        "Successfully locked NVML Memory clocks (Min: {}, Max: {}) to GPU {}",
                        min_clock, max_clock, gpu_id
                    ),
                    Err(e) => eprintln!(
                        "Failed to lock NVML Memory clocks for GPU {}: {:?}",
                        gpu_id, e
                    ),
                }
            }
        } else {
            eprintln!(
                "Invalid arguments for --locked-mem-clocks, expected 2 arguments (MIN_MHZ MAX_MHZ)"
            );
        }
    }

    if let Some(nvml_pstate_lock_vals) = matches.get_many::<String>("pstate_lock") {
        let requested_pstates = nvml_pstate_lock_vals
            .map(|s| s.as_str())
            .collect::<Vec<_>>();
        let first_pstate = nvoc_core::try_parse_nvml_pstate(requested_pstates[0])?;
        let second_pstate = if requested_pstates.len() >= 2 {
            nvoc_core::try_parse_nvml_pstate(requested_pstates[1])?
        } else {
            first_pstate
        };

        for &gpu_id in gpu_ids {
            match nvoc_core::set_nvml_pstate_lock(&nvml, gpu_id, first_pstate, second_pstate) {
                Ok((range_label, min_lock_mhz, max_lock_mhz)) => println!(
                    "Successfully locked GPU {} to {} via NVML memory window {}-{} MHz",
                    gpu_id, range_label, min_lock_mhz, max_lock_mhz,
                ),
                Err(e) => eprintln!(
                    "Failed to lock GPU {} to NVML PState {}: {:?}",
                    gpu_id,
                    requested_pstates.join(" "),
                    e
                ),
            }
        }
    }

    if matches.get_flag("reset_mem_clocks") {
        for &gpu_id in gpu_ids {
            match nvoc_core::reset_nvml_mem_locked_clocks(&nvml, gpu_id) {
                Ok(_) => println!(
                    "Successfully reset NVML Memory locked clocks to GPU {}",
                    gpu_id
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML Memory locked clocks for GPU {}: {:?}",
                    gpu_id, e
                ),
            }
        }
    }

    Ok(())
}

fn handle_nvml(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let mut gpu_ids = Vec::with_capacity(gpus.len());
    for gpu in gpus {
        gpu_ids.push(gpu.info()?.id as u32);
    }
    handle_nvml_with_ids(&gpu_ids, matches)
}

pub fn handle_nvml_cooler_with_ids(gpu_ids: &[u32], matches: &ArgMatches) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?;
    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    let policy = matches
        .get_one::<String>("policy")
        .map(|s| nvoc_core::parse_nvml_fan_control_policy(s.as_str()))
        .transpose()?
        .ok_or_else(|| Error::from("Missing required argument: --policy <MODE>"))?;
    let level = matches
        .get_one::<u32>("level")
        .copied()
        .ok_or_else(|| Error::from("Missing required argument: --level <LEVEL>"))?;

    for &gpu_id in gpu_ids {
        let fan_count = nvoc_core::get_nvml_num_fans(&nvml, gpu_id).ok_or_else(|| {
            Error::Custom(format!("Failed to query NVML fan count for GPU {}", gpu_id))
        })?;

        let fan_indices: Vec<u32> = match cooler_id {
            "1" => vec![0],
            "2" => {
                if fan_count < 2 {
                    return Err(Error::Custom(format!(
                        "GPU {} reports only {} fan(s), cooler id 2 is unavailable",
                        gpu_id, fan_count
                    )));
                }
                vec![1]
            }
            _ => (0..fan_count).collect(),
        };

        for fan_idx in fan_indices {
            match nvoc_core::set_fan_speed(&nvml, gpu_id, fan_idx, policy, level) {
                Ok(_) => println!(
                    "Successfully applied NVML cooler policy {:?}, level {}% to GPU {} fan {}",
                    policy,
                    level,
                    gpu_id,
                    fan_idx + 1
                ),
                Err(e) => eprintln!(
                    "Failed to set NVML cooler for GPU {} fan {}: {:?}",
                    gpu_id,
                    fan_idx + 1,
                    e
                ),
            }
        }
    }

    Ok(())
}

pub fn handle_nvml_cooler(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let mut gpu_ids = Vec::with_capacity(gpus.len());
    for gpu in gpus {
        gpu_ids.push(gpu.info()?.id as u32);
    }
    handle_nvml_cooler_with_ids(&gpu_ids, matches)
}

pub fn handle_reset_nvml_cooler(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?;
    for gpu in gpus {
        handle_reset_nvml_cooler_single_gpu(&nvml, gpu, cooler_id)?;
    }

    Ok(())
}

pub fn handle_reset_nvml_cooler_single_gpu(
    nvml: &Nvml,
    gpu: &Gpu,
    cooler_id: &str,
) -> Result<(), Error> {
    let gpu_info = gpu.info()?;
    let gpu_id = gpu_info.id as u32;
    let fan_count = nvoc_core::get_nvml_num_fans(nvml, gpu_id).ok_or_else(|| {
        Error::Custom(format!(
            "Failed to query NVML fan count for GPU {}",
            gpu_info.id
        ))
    })?;

    let fan_indices: Vec<u32> = match cooler_id {
        "1" => vec![0],
        "2" => {
            if fan_count < 2 {
                return Err(Error::Custom(format!(
                    "GPU {} reports only {} fan(s), cooler id 2 is unavailable",
                    gpu_info.id, fan_count
                )));
            }
            vec![1]
        }
        _ => (0..fan_count).collect(),
    };

    for fan_idx in fan_indices {
        match nvoc_core::set_default_fan_speed(nvml, gpu_id, fan_idx) {
            Ok(_) => println!(
                "Successfully restored NVML default fan speed on GPU {} fan {}",
                gpu_info.id,
                fan_idx + 1
            ),
            Err(e) => eprintln!(
                "Failed to restore NVML default fan speed for GPU {} fan {}: {:?}",
                gpu_info.id,
                fan_idx + 1,
                e
            ),
        }
    }

    Ok(())
}
