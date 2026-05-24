use super::cli_types::{OutputFormat, ResetSettings};
use super::human;
use nvoc_core::{
    Celsius, ClockDomain, CoolerPolicy, GpuSettings, GpuTarget, GpuTdpTempLimits, Kilohertz,
    KilohertzDelta, Microvolts, MicrovoltsDelta, PState, Percentage,
};
use std::io;

use clap::ArgMatches;
use nvoc_core::ConvertEnum;
use nvoc_core::Error;
use nvoc_core::VfpResetDomain;
use nvoc_core::{
    CheckVoltageFrequency, CoolerTarget, GpuOperation, ProbeVoltageLimits, QueryGpuInfo,
    QueryGpuSettings, QueryGpuStatus, QueryTdpTempLimits, QueryVfpPointVoltage, ResetCoolerLevels,
    ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetPstateBaseVoltages, ResetVfpDeltas,
    ResetVfpFrequencyLock, ResetVfpLock, SetCoolerLevels, SetNvapiPowerLimits, SetNvapiPstateLock,
    SetNvapiSensorLimits, SetPstateBaseVoltage, SetPstateClockOffset, SetVfpFrequencyLock,
    SetVfpPointDelta, SetVfpVoltageLock, SetVoltageBoost, VfpLockRequest,
    set_nvapi_pstate_clock_offsets,
};
use nvoc_core::{
    QueryClockOffset, QueryFanInfo, QueryPowerLimits, QueryPstates,
    QuerySupportedApplicationsClocks, QueryTemperatureThresholds, ResetApplicationsClocks,
    ResetFanSpeed, ResetLockedClocks, SetApplicationsClocks, SetClockOffset, SetFanSpeed,
    SetLockedClocks, SetNvmlPstateLock, SetPowerLimit, run,
};
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

use nvoc_core::fetch_gpu_type;

fn run_output<O: GpuOperation>(gpu: &GpuTarget<'_>, op: O) -> Result<O::Output, Error> {
    run(gpu, op).map(|report| report.output)
}

#[derive(Clone, Debug)]
pub struct GpuVoltageLimits {
    pub gpu_id: u32,
    pub lower_point: usize,
    pub upper_point: usize,
}

#[derive(Clone, Debug)]
pub struct GpuVoltageFrequencyCheck {
    pub gpu_id: u32,
    pub precise: bool,
}

fn apply_vfp_lock(
    gpu: &GpuTarget<'_>,
    request: VfpLockRequest,
    feedback: bool,
) -> Result<(), Error> {
    match request {
        VfpLockRequest::VoltagePoint(point) => run_output(
            gpu,
            SetVfpVoltageLock {
                voltage_target: nvoc_core::NvapiLockedVoltageTarget::Point(point),
                feedback,
            },
        ),
        VfpLockRequest::Voltage(voltage) => run_output(
            gpu,
            SetVfpVoltageLock {
                voltage_target: nvoc_core::NvapiLockedVoltageTarget::Voltage(voltage),
                feedback,
            },
        ),
        VfpLockRequest::Frequency {
            domain,
            upper,
            lower,
        } => run_output(
            gpu,
            SetVfpFrequencyLock {
                domain,
                upper,
                lower,
            },
        ),
    }
}

fn collect_long_flags(cmd: &clap::Command, out: &mut Vec<String>) {
    for arg in cmd.get_arguments() {
        if let Some(long) = arg.get_long() {
            out.push(long.to_string());
        }
    }
    for sub in cmd.get_subcommands() {
        collect_long_flags(sub, out);
    }
}

pub fn check_single_dash_args_from<I, S>(
    cmd: &clap::Command,
    args: I,
) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut known_longs: Vec<String> = Vec::new();
    collect_long_flags(cmd, &mut known_longs);

    for arg in args {
        let arg = arg.as_ref();
        if !arg.starts_with('-') || arg.starts_with("--") || arg == "-" {
            continue;
        }

        let body = arg.trim_start_matches('-');
        let flag_name = body.split('=').next().unwrap_or(body);

        if known_longs.iter().any(|l| l == flag_name) {
            return Err(format!("invalid option {:?} -- did you mean --{}?", arg, body).into());
        }
    }
    Ok(())
}

pub fn check_single_dash_args(cmd: &clap::Command) -> Result<(), Box<dyn std::error::Error>> {
    check_single_dash_args_from(cmd, std::env::args().skip(1))
}

fn json_error(err: serde_json::Error) -> Error {
    Error::Custom(format!("JSON Error: {}", err))
}

fn parse_clock_domain(raw: Option<&String>) -> Result<ClockDomain, Error> {
    match raw.map(|s| s.as_str()).unwrap_or("Graphics") {
        "Graphics" => Ok(ClockDomain::Graphics),
        "Memory" => Ok(ClockDomain::Memory),
        other => ClockDomain::from_str(other)
            .map_err(|e| Error::from(format!("Invalid --domain value '{}': {}", other, e))),
    }
}

fn parse_lock_frequency(
    matches: &ArgMatches,
) -> Result<(ClockDomain, Kilohertz, Option<Kilohertz>), Error> {
    let raw_targets = matches
        .get_many::<String>("clock")
        .ok_or_else(|| Error::from("Missing --clock <UPPER_MHZ> [LOWER_MHZ] value"))?
        .map(|s| s.as_str())
        .collect::<Vec<_>>();

    let upper_mhz = raw_targets[0]
        .parse::<u32>()
        .map_err(|_| Error::from("In --clock mode, UPPER_MHZ must be an integer MHz value"))?;

    let lower_mhz =
        if raw_targets.len() >= 2 {
            Some(raw_targets[1].parse::<u32>().map_err(|_| {
                Error::from("In --clock mode, LOWER_MHZ must be an integer MHz value")
            })?)
        } else {
            None
        };

    if let Some(lower) = lower_mhz
        && lower > upper_mhz
    {
        return Err(Error::from(
            "--clock expects upper bound first and lower bound second",
        ));
    }

    Ok((
        parse_clock_domain(matches.get_one::<String>("domain"))?,
        Kilohertz(upper_mhz.saturating_mul(1000)),
        lower_mhz.map(|v| Kilohertz(v.saturating_mul(1000))),
    ))
}

fn parse_lock_voltage(
    gpu: &GpuTarget<'_>,
    matches: &ArgMatches,
    default_point: usize,
) -> Result<VfpLockRequest, Error> {
    let raw_target = matches
        .get_one::<String>("point")
        .map(|s| s.as_str())
        .unwrap_or("");

    if matches
        .try_get_one::<bool>("voltage")
        .is_ok_and(|v| v.copied().unwrap_or(false))
    {
        const MIN_LOCK_UV: u32 = 500_000;
        const MAX_LOCK_UV: u32 = 2_000_000;

        let input_voltage = raw_target.parse::<u32>()?;
        let voltage_uv = if input_voltage >= 10_000 {
            input_voltage
        } else {
            input_voltage.saturating_mul(1000)
        };

        if !(MIN_LOCK_UV..=MAX_LOCK_UV).contains(&voltage_uv) {
            return Err(Error::from(format!(
                "--voltage {} µV is outside the supported range {}–{} µV (0.5–2.0 V)",
                voltage_uv, MIN_LOCK_UV, MAX_LOCK_UV
            )));
        }

        Ok(VfpLockRequest::Voltage(Microvolts(voltage_uv)))
    } else {
        let point = raw_target.parse::<usize>().unwrap_or(default_point);
        run_output(gpu, QueryVfpPointVoltage { point })?;
        Ok(VfpLockRequest::VoltagePoint(point))
    }
}

fn parse_nvapi_locked_clock_range(
    matches: &ArgMatches,
    key: &str,
) -> Result<Option<(u32, u32)>, Error> {
    let Some(raw) = matches.get_many::<String>(key) else {
        return Ok(None);
    };

    let (invalid_msg, count_msg, order_msg) = if key == "locked_core_clocks" {
        (
            "Invalid --locked-core-clocks value: expected integer MHz",
            "Invalid arguments for --nvapi-locked-core-clocks, expected 2 values (MIN_MHZ MAX_MHZ)",
            "--nvapi-locked-core-clocks expects MIN_MHZ <= MAX_MHZ",
        )
    } else {
        (
            "Invalid --locked-mem-clocks value: expected integer MHz",
            "Invalid arguments for --nvapi-locked-mem-clocks, expected 2 values (MIN_MHZ MAX_MHZ)",
            "--nvapi-locked-mem-clocks expects MIN_MHZ <= MAX_MHZ",
        )
    };

    let clocks = raw
        .map(|s| u32::from_str(s.as_str()).map_err(|_| Error::from(invalid_msg)))
        .collect::<Result<Vec<_>, _>>()?;

    if clocks.len() != 2 {
        return Err(Error::from(count_msg));
    }

    let min_clock = clocks[0];
    let max_clock = clocks[1];
    if min_clock > max_clock {
        return Err(Error::from(order_msg));
    }

    Ok(Some((min_clock, max_clock)))
}

pub fn handle_lock_vfp(
    gpus: &[GpuTarget<'_>],
    matches: &ArgMatches,
    default_point: usize,
    feedback_flag: bool,
) -> Result<(), Error> {
    if let Some(locked_voltage_raw) = matches.get_one::<String>("locked_voltage") {
        let target = nvoc_core::parse_nvapi_locked_voltage_target(locked_voltage_raw.as_str())?;
        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            let request = match target {
                nvoc_core::NvapiLockedVoltageTarget::Point(point) => {
                    VfpLockRequest::VoltagePoint(point)
                }
                nvoc_core::NvapiLockedVoltageTarget::Voltage(v) => VfpLockRequest::Voltage(v),
            };
            match apply_vfp_lock(gpu, request, false) {
                Ok(_) => match target {
                    nvoc_core::NvapiLockedVoltageTarget::Point(point) => {
                        let voltage = run_output(gpu, QueryVfpPointVoltage { point })?;
                        println!(
                            "Successfully locked GPU {} on VFP point {} ({} mV)",
                            gpu_info.id,
                            point,
                            voltage.0 / 1000
                        );
                    }
                    nvoc_core::NvapiLockedVoltageTarget::Voltage(v) => println!(
                        "Successfully applied NVAPI locked voltage {} mV to GPU {}",
                        v.0 / 1000,
                        gpu_info.id
                    ),
                },
                Err(e) => eprintln!(
                    "Failed to set NVAPI locked voltage for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
        return Ok(());
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_core_clocks")?
    {
        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                SetVfpFrequencyLock {
                    domain: ClockDomain::Graphics,
                    upper: Kilohertz(max_clock.saturating_mul(1000)),
                    lower: Some(Kilohertz(min_clock.saturating_mul(1000))),
                },
            ) {
                Ok(_) => println!(
                    "Successfully locked NVAPI core clocks (Min: {}, Max: {}) to GPU {}",
                    min_clock, max_clock, gpu_info.id
                ),
                Err(e) => eprintln!(
                    "Failed to lock NVAPI core clocks for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
        return Ok(());
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_mem_clocks")?
    {
        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                SetVfpFrequencyLock {
                    domain: ClockDomain::Memory,
                    upper: Kilohertz(max_clock.saturating_mul(1000)),
                    lower: Some(Kilohertz(min_clock.saturating_mul(1000))),
                },
            ) {
                Ok(_) => println!(
                    "Successfully locked NVAPI memory clocks (Min: {}, Max: {}) to GPU {}",
                    min_clock, max_clock, gpu_info.id
                ),
                Err(e) => eprintln!(
                    "Failed to lock NVAPI memory clocks for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
        return Ok(());
    }

    if matches.get_one::<String>("clock").is_some() {
        if matches
            .try_get_one::<bool>("voltage")
            .is_ok_and(|v| v.copied().unwrap_or(false))
        {
            return Err(Error::from("Cannot use --clock and --voltage together"));
        }

        let (domain, upper, lower) = parse_lock_frequency(matches)?;
        for gpu in gpus {
            apply_vfp_lock(
                gpu,
                VfpLockRequest::Frequency {
                    domain,
                    upper,
                    lower,
                },
                feedback_flag,
            )?;
        }
        return Ok(());
    }

    let request = parse_lock_voltage(
        gpus.first().ok_or_else(|| Error::from("no GPU selected"))?,
        matches,
        default_point,
    )?;
    for gpu in gpus {
        apply_vfp_lock(gpu, request, feedback_flag)?;
    }
    Ok(())
}

pub fn handle_test_voltage_limits(
    gpus: &[GpuTarget<'_>],
    _matches: &ArgMatches,
    mut print_separator: impl FnMut(),
) -> Result<Vec<GpuVoltageLimits>, Error> {
    if gpus.is_empty() {
        return Err(Error::from("no GPU selected"));
    }

    print_separator();
    gpus.iter()
        .map(|gpu| {
            let limits = run_output(gpu, ProbeVoltageLimits)?;
            Ok(GpuVoltageLimits {
                gpu_id: gpu.id.0,
                lower_point: limits.lower_point,
                upper_point: limits.upper_point,
            })
        })
        .collect()
}

pub fn voltage_frequency_check(
    gpus: &[GpuTarget<'_>],
    point: usize,
    mut print_separator: impl FnMut(),
) -> Result<Vec<GpuVoltageFrequencyCheck>, Error> {
    if gpus.is_empty() {
        return Err(Error::from("no GPU selected"));
    }

    print_separator();
    gpus.iter()
        .map(|gpu| {
            run_output(gpu, CheckVoltageFrequency { point }).map(|check| GpuVoltageFrequencyCheck {
                gpu_id: gpu.id.0,
                precise: check.precise,
            })
        })
        .collect()
}

pub fn get_gpu_tdp_temp_limit(
    matches: &ArgMatches,
    mut print_separator: impl FnMut(),
) -> Result<GpuTdpTempLimits, Error> {
    let selector = match matches.get_many::<String>("gpu") {
        Some(values) => nvoc_core::GpuSelector::from_specs(values.cloned()),
        None => nvoc_core::GpuSelector::all(),
    };
    let inventory = nvoc_core::discover_targets(nvoc_core::BackendSet::Nvapi)?;
    let all_targets = inventory.targets();
    let gpus = nvoc_core::select_targets(&all_targets, &selector)?;
    print_separator();
    let gpu = gpus.first().ok_or_else(|| Error::from("no GPU selected"))?;
    run_output(gpu, QueryTdpTempLimits)
}

pub fn handle_cooler_command(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let policy_raw = matches
        .get_one::<String>("policy")
        .ok_or_else(|| Error::from("Missing required argument: --policy <MODE>"))?;
    let mode = match policy_raw.to_ascii_lowercase().as_str() {
        "continuous" => CoolerPolicy::from_str("continuous")?,
        "manual" => CoolerPolicy::from_str("manual")?,
        "auto" => CoolerPolicy::from_str("continuous")?,
        _ => CoolerPolicy::from_str(policy_raw.as_str())?,
    };
    let level = matches
        .get_one::<u32>("level")
        .copied()
        .ok_or_else(|| Error::from("Missing required argument: --level <LEVEL>"))?;
    let target = match matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all")
    {
        "1" => CoolerTarget::Cooler1,
        "2" => CoolerTarget::Cooler2,
        _ => CoolerTarget::All,
    };

    for gpu in gpus {
        run_output(
            gpu,
            SetCoolerLevels {
                policy: mode,
                level,
                cooler_target: target,
            },
        )?;
    }
    Ok(())
}

pub fn single_point_adj(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let point_start = *matches.get_one::<u32>("point_start").unwrap() as usize;
    let delta_ini = *matches.get_one::<i32>("delta").unwrap();
    for gpu in gpus {
        run_output(
            gpu,
            SetVfpPointDelta {
                point: point_start,
                delta: KilohertzDelta(delta_ini),
            },
        )?;
    }
    Ok(())
}

pub fn handle_pointwiseoc(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let range_str = matches
        .get_one::<String>("range")
        .ok_or_else(|| Error::from("Missing required argument: RANGE"))?;
    let parts = range_str.splitn(2, '-').collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(Error::from(format!(
            "Invalid RANGE format '{}'. Expected 'start-end', e.g. '39-76'.",
            range_str
        )));
    }

    let start = parts[0].trim().parse::<usize>().map_err(|_| {
        Error::from(format!(
            "Invalid range start '{}': must be a non-negative integer",
            parts[0]
        ))
    })?;
    let end = parts[1].trim().parse::<usize>().map_err(|_| {
        Error::from(format!(
            "Invalid range end '{}': must be a non-negative integer",
            parts[1]
        ))
    })?;
    let delta = *matches
        .get_one::<i32>("delta")
        .ok_or_else(|| Error::from("Missing required argument: DELTA"))?;

    if start > end {
        return Err(Error::from(format!(
            "Range start ({}) must be <= end ({}).",
            start, end
        )));
    }
    println!(
        "pointwiseoc: applying delta {} kHz to VFP points {}..={} (inclusive)",
        delta, start, end
    );
    for gpu in gpus {
        for point in start..=end {
            run_output(
                gpu,
                SetVfpPointDelta {
                    point,
                    delta: KilohertzDelta(delta),
                },
            )?;
        }
    }
    Ok(())
}

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
    let inventory = nvoc_core::discover_targets(nvoc_core::BackendSet::Nvapi)?;
    let gpu_list = inventory.targets();
    for (i, gpu) in gpu_list.iter().enumerate() {
        let info = run_output(gpu, QueryGpuInfo)?;
        if let Some(ids) = info.bus.bus.pci_ids() {
            println!(
                "GPU {}: ID:0x{:04X} bus:{:08x} - {:08x} - {:08x} - {:02x}",
                i, gpu.id.0, ids.device_id, ids.subsystem_id, ids.ext_device_id, ids.revision_id,
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
    nvapi_gpus: &[GpuTarget<'_>],
    nvml: Option<&Nvml>,
    nvml_indices: &[u32],
    oformat: OutputFormat,
    output_file: Option<&str>,
) -> Result<(), Error> {
    if !nvapi_gpus.is_empty() {
        for (i, gpu) in nvapi_gpus.iter().enumerate() {
            println!("GPU {}: ID:0x{:04X}", i, gpu.id.0);
        }

        match oformat {
            OutputFormat::Human => {
                let mut success = 0usize;
                for gpu in nvapi_gpus {
                    let info = match run_output(gpu, QueryGpuInfo) {
                        Ok(info) => info,
                        Err(e) => {
                            eprintln!(
                                "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                gpu.id.0, e
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
                        let info = match run_output(gpu, QueryGpuInfo) {
                            Ok(info) => info,
                            Err(e) => {
                                eprintln!(
                                    "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                    gpu.id.0, e
                                );
                                continue;
                            }
                        };
                        let gpu_file_path = format!("{}_gpu{}.json", file_path, info.id);
                        let file = std::fs::File::create(&gpu_file_path)?;
                        serde_json::to_writer_pretty(file, &info).map_err(json_error)?;
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
                        match run_output(gpu, QueryGpuInfo) {
                            Ok(info) => gpu_info.push(info),
                            Err(e) => eprintln!(
                                "Warning: failed to read info for GPU ID 0x{:04X}: {:?}",
                                gpu.id.0, e
                            ),
                        }
                    }
                    if gpu_info.is_empty() {
                        return Err(Error::Custom(
                            "No selected GPU returned usable NvAPI info".to_string(),
                        ));
                    }
                    serde_json::to_writer_pretty(io::stdout(), &gpu_info).map_err(json_error)?;
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
    nvapi_gpus: &[GpuTarget<'_>],
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
                    for gpu in nvapi_gpus {
                        let mut set = None;

                        fn requires_set<'a>(
                            gpu: &GpuTarget<'_>,
                            set: &'a mut Option<GpuSettings>,
                        ) -> Result<&'a GpuSettings, Error> {
                            if set.is_some() {
                                return Ok(set.as_ref().unwrap());
                            }
                            Ok(set.get_or_insert(run_output(gpu, QueryGpuSettings)?))
                        }

                        let status = match run_output(gpu, QueryGpuStatus) {
                            Ok(status) => status,
                            Err(e) => {
                                eprintln!(
                                    "Warning: failed to read status for GPU ID 0x{:04X}: {:?}",
                                    gpu.id.0, e
                                );
                                continue;
                            }
                        };

                        human::print_status(&status);
                        human::print_settings(gpu, requires_set(gpu, &mut set)?);
                        if let Ok(thresholds) = run(gpu, QueryTemperatureThresholds) {
                            println!("NVML Temperature Thresholds:");
                            for threshold in thresholds.output {
                                match threshold.celsius {
                                    Some(temp) => println!("  {:<16} : {} C", threshold.name, temp),
                                    None => println!("  {:<16} : N/A", threshold.name),
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
                    for gpu in nvapi_gpus {
                        match run_output(gpu, QueryGpuStatus) {
                            Ok(s) => status.push(s),
                            Err(e) => eprintln!(
                                "Warning: failed to read status for GPU ID 0x{:04X}: {:?}",
                                gpu.id.0, e
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

pub fn handle_get(gpus: &[GpuTarget<'_>], oformat: OutputFormat) -> Result<(), Error> {
    match oformat {
        OutputFormat::Human => {
            for gpu in gpus.iter() {
                if let Ok(info) = run_output(gpu, QueryGpuInfo) {
                    human::print_scan_separator();
                    println!("GPU {}: {} ({})", info.id, info.name, info.codename);
                    human::print_scan_separator();
                }
                if let Ok(set) = run_output(gpu, QueryGpuSettings) {
                    human::print_settings(gpu, &set);
                }
                if run_output(gpu, QueryGpuInfo).is_ok() {
                    let power_limit = run(gpu, QueryPowerLimits).ok().map(|r| r.output);
                    let temp_thresholds =
                        run(gpu, QueryTemperatureThresholds).ok().map(|r| r.output);
                    let pstate_info = run(gpu, QueryPstates).ok().map(|r| r.output);
                    let app_clocks = run(gpu, QuerySupportedApplicationsClocks)
                        .ok()
                        .map(|r| r.output);
                    let fan_info = run(gpu, QueryFanInfo).ok().map(|r| r.output);
                    if power_limit.is_some()
                        || temp_thresholds.is_some()
                        || pstate_info.is_some()
                        || app_clocks.is_some()
                        || fan_info.is_some()
                    {
                        println!("NVML Settings:");
                        if let Some(power) = power_limit {
                            println!(
                                "  Power Limit        : {:.2} W (Min: {:.2} W - Max: {:.2} W)",
                                power.current_watts, power.min_watts, power.max_watts
                            );
                        }
                        if let Some(thresholds) = temp_thresholds {
                            println!("  Temperature Thresholds:");
                            for threshold in thresholds {
                                match threshold.celsius {
                                    Some(temp) => {
                                        println!("    {:<16} : {} C", threshold.name, temp)
                                    }
                                    None => println!("    {:<16} : N/A", threshold.name),
                                }
                            }
                        }
                        if let Some(fan) = fan_info
                            && let (Some(min_fan), Some(max_fan)) = (fan.min_speed, fan.max_speed)
                        {
                            println!("  Fan Speed Range    : {}% - {}%", min_fan, max_fan);
                        }
                        if let Some(pstates) = pstate_info {
                            println!("  Supported P-States:");
                            for pstate_range in pstates {
                                let pstate_str = nvoc_core::nvml_pstate_to_str(pstate_range.pstate);
                                println!("    {}:", pstate_str);
                                println!(
                                    "      Core Clock Range   : {} MHz - {} MHz",
                                    pstate_range.min_core_mhz, pstate_range.max_core_mhz
                                );
                                println!(
                                    "      Mem Clock Range    : {} MHz - {} MHz",
                                    pstate_range.min_memory_mhz, pstate_range.max_memory_mhz
                                );

                                if let Ok(core_offset) = run(
                                    gpu,
                                    QueryClockOffset {
                                        domain: ClockDomain::Graphics,
                                        pstate: pstate_range.pstate,
                                    },
                                ) {
                                    println!(
                                        "      Core Clock Offset  : {} MHz",
                                        core_offset.output.mhz
                                    );
                                }
                                if let Ok(mem_offset) = run(
                                    gpu,
                                    QueryClockOffset {
                                        domain: ClockDomain::Memory,
                                        pstate: pstate_range.pstate,
                                    },
                                ) {
                                    println!(
                                        "      Mem Clock Offset   : {} MHz",
                                        mem_offset.output.mhz
                                    );
                                }
                            }
                        } else {
                            // Fallback if pstate info is unsupported
                            let pstate =
                                nvml_wrapper::enum_wrappers::device::PerformanceState::Zero;
                            if let Ok(core_offset) = run(
                                gpu,
                                QueryClockOffset {
                                    domain: ClockDomain::Graphics,
                                    pstate,
                                },
                            ) {
                                println!(
                                    "  Core Clock Offset (P0) : {} MHz",
                                    core_offset.output.mhz
                                );
                            }
                            if let Ok(mem_offset) = run(
                                gpu,
                                QueryClockOffset {
                                    domain: ClockDomain::Memory,
                                    pstate,
                                },
                            ) {
                                println!(
                                    "  Mem Clock Offset (P0)  : {} MHz",
                                    mem_offset.output.mhz
                                );
                            }
                        }
                        if let Some(clocks) = app_clocks {
                            if !clocks.is_empty() {
                                println!("  Supported Applications Clocks:");
                                for app_clock in clocks {
                                    let mem_clk = app_clock.memory_mhz;
                                    let mut gfx_clocks = app_clock.graphics_mhz;
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
                                    clocks.iter().map(|clock| clock.memory_mhz).collect();
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
                match run_output(gpu, QueryGpuSettings) {
                    Ok(s) => settings.push(s),
                    Err(e) => eprintln!(
                        "Warning: failed to read settings for GPU ID 0x{:04X}: {:?}",
                        gpu.id.0, e
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

pub fn handle_reset(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
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

    fn warn_result(
        r: Result<(), Error>,
        setting: ResetSettings,
        explicit: bool,
    ) -> Result<(), Error> {
        match r {
            Ok(()) => Ok(()),
            Err(err) if err.is_allowable_nvapi_reset_error() && !explicit => Ok(()),
            Err(err) => Err(Error::Custom(format!(
                "Reset {:?} failed: {}",
                setting, err
            ))),
        }
    }

    for gpu in gpus {
        let info = run_output(gpu, QueryGpuInfo)?;

        for &setting in &settings {
            match setting {
                ResetSettings::VoltageBoost => warn_result(
                    run_output(
                        gpu,
                        SetVoltageBoost {
                            boost: Percentage(0),
                        },
                    )
                    .map(|_| ()),
                    setting,
                    explicit,
                )?,
                ResetSettings::SensorLimits => {
                    warn_result(run_output(gpu, ResetNvapiSensorLimits), setting, explicit)?
                }
                ResetSettings::PowerLimits => {
                    warn_result(run_output(gpu, ResetNvapiPowerLimits), setting, explicit)?
                }
                ResetSettings::CoolerLevels => {
                    warn_result(run_output(gpu, ResetCoolerLevels), setting, explicit)?
                }
                ResetSettings::VfpDeltas => warn_result(
                    run_output(
                        gpu,
                        ResetVfpDeltas {
                            domain: vfp_reset_domain,
                        },
                    ),
                    setting,
                    explicit,
                )?,
                ResetSettings::VfpLock => {
                    warn_result(run_output(gpu, ResetVfpLock), setting, explicit)?
                }
                ResetSettings::PStateDeltas => {
                    let pstates = info.pstate_limits.iter().flat_map(|(&pstate, l)| {
                        l.iter()
                            .filter(|&(_, info)| info.frequency_delta.is_some())
                            .map(move |(&clock, _)| (pstate, clock))
                    });
                    warn_result(
                        set_nvapi_pstate_clock_offsets(
                            gpu,
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
                            match run_output(gpu, ResetPstateBaseVoltages) {
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

pub fn handle_set_command(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    match matches.subcommand() {
        Some(("nvapi", sub)) => handle_nvapi(gpus, sub)?,
        Some(("nvml", sub)) => handle_nvml(gpus, sub)?,
        Some(("nvml-cooler", sub)) => handle_nvml_cooler(gpus, sub)?,
        _ => {}
    }
    Ok(())
}

fn handle_nvapi(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    if let Some(&vboost) = matches.get_one::<u32>("vboost") {
        for gpu in gpus {
            run_output(
                gpu,
                SetVoltageBoost {
                    boost: Percentage(vboost),
                },
            )?;
        }
    }
    if let Some(plimit) = matches.get_many::<u32>("plimit") {
        let plimit: Vec<_> = plimit.copied().map(Percentage).collect();
        for gpu in gpus {
            run_output(
                gpu,
                SetNvapiPowerLimits {
                    limits: plimit.clone(),
                },
            )?;
        }
    }
    if let Some(tlimit) = matches.get_many::<i32>("tlimit") {
        let tlimit: Vec<_> = tlimit.copied().map(|v| Celsius(v).into()).collect();
        for gpu in gpus {
            run_output(
                gpu,
                SetNvapiSensorLimits {
                    limits: tlimit.clone(),
                },
            )?;
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
            run_output(
                gpu,
                SetPstateBaseVoltage {
                    pstate: nvapi_pstate,
                    delta_uv: MicrovoltsDelta(delta_uv),
                },
            )?;
        }
    }

    if let Some(&core_offset) = matches.get_one::<i32>("core_offset") {
        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                SetPstateClockOffset {
                    pstate: nvapi_pstate,
                    domain: ClockDomain::Graphics,
                    delta: KilohertzDelta(core_offset),
                },
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
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                SetPstateClockOffset {
                    pstate: nvapi_pstate,
                    domain: ClockDomain::Memory,
                    delta: KilohertzDelta(mem_offset),
                },
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

        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                SetNvapiPstateLock {
                    first_pstate,
                    second_pstate,
                },
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
        handle_lock_vfp(gpus, matches, 0, false)?;
    }

    if matches.get_flag("reset_volt_locks") {
        for gpu in gpus {
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(gpu, ResetVfpLock) {
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
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                ResetVfpFrequencyLock {
                    domain: ClockDomain::Graphics,
                },
            ) {
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
            let gpu_info = run_output(gpu, QueryGpuInfo)?;
            match run_output(
                gpu,
                ResetVfpFrequencyLock {
                    domain: ClockDomain::Memory,
                },
            ) {
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
        handle_test_voltage_limits(gpus, matches, human::print_scan_separator)?;
    }

    Ok(())
}

pub fn handle_nvml(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let nvml_pstate_val = matches
        .get_one::<String>("pstate")
        .map(|s| s.as_str())
        .unwrap_or("0");
    let target_nvml_pstate = nvoc_core::parse_nvml_pstate(nvml_pstate_val)?;

    if let Some(&core_offset) = matches.get_one::<i32>("core_offset") {
        for gpu in gpus {
            match run(
                gpu,
                SetClockOffset {
                    domain: ClockDomain::Graphics,
                    pstate: target_nvml_pstate,
                    mhz: core_offset,
                },
            ) {
                Ok(_) => println!(
                    "Successfully applied NVML core offset {} MHz to GPU {} for PState {}",
                    core_offset, gpu.id.0, nvml_pstate_val
                ),
                Err(e) => eprintln!(
                    "Failed to set NVML core offset for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    if let Some(&mem_offset) = matches.get_one::<i32>("mem_offset") {
        for gpu in gpus {
            match run(
                gpu,
                SetClockOffset {
                    domain: ClockDomain::Memory,
                    pstate: target_nvml_pstate,
                    mhz: mem_offset,
                },
            ) {
                Ok(_) => println!(
                    "Successfully applied NVML mem offset {} MHz to GPU {} for PState {}",
                    mem_offset, gpu.id.0, nvml_pstate_val
                ),
                Err(e) => eprintln!(
                    "Failed to set NVML mem offset for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    if let Some(&power_w) = matches.get_one::<u32>("power_limit") {
        for gpu in gpus {
            match run(gpu, SetPowerLimit { watts: power_w }) {
                Ok(_) => println!(
                    "Successfully applied NVML power limit {} W to GPU {}",
                    power_w, gpu.id.0
                ),
                Err(e) => eprintln!(
                    "Failed to set NVML power limit for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    if let Some(app_clocks) = matches.get_many::<u32>("locked_app_clocks") {
        let clocks: Vec<u32> = app_clocks.copied().collect();
        if clocks.len() == 2 {
            for gpu in gpus {
                match run(
                    gpu,
                    SetApplicationsClocks {
                        memory_mhz: clocks[0],
                        graphics_mhz: clocks[1],
                    },
                ) {
                    Ok(_) => println!(
                        "Successfully locked NVML app clocks (Mem: {}, Core: {}) to GPU {}",
                        clocks[0], clocks[1], gpu.id.0
                    ),
                    Err(e) => {
                        eprintln!(
                            "Failed to lock NVML app clocks for GPU {}: {:?}",
                            gpu.id.0, e
                        )
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
        for gpu in gpus {
            match run(gpu, ResetApplicationsClocks) {
                Ok(_) => println!(
                    "Successfully reset NVML applications clocks to default on GPU {}",
                    gpu.id.0
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML applications clocks for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    if let Some(locked_core_clocks) = matches.get_many::<u32>("locked_core_clocks") {
        let clocks: Vec<u32> = locked_core_clocks.copied().collect();
        if clocks.len() == 2 {
            for gpu in gpus {
                match run(
                    gpu,
                    SetLockedClocks {
                        domain: ClockDomain::Graphics,
                        min_mhz: clocks[0],
                        max_mhz: clocks[1],
                    },
                ) {
                    Ok(_) => println!(
                        "Successfully locked NVML core clocks (Min: {}, Max: {}) to GPU {}",
                        clocks[0], clocks[1], gpu.id.0
                    ),
                    Err(e) => eprintln!(
                        "Failed to lock NVML core clocks for GPU {}: {:?}",
                        gpu.id.0, e
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
        for gpu in gpus {
            match run(
                gpu,
                ResetLockedClocks {
                    domain: ClockDomain::Graphics,
                },
            ) {
                Ok(_) => println!(
                    "Successfully reset NVML core locked clocks to GPU {}",
                    gpu.id.0
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML core locked clocks for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    if let Some(locked_mem_clocks) = matches.get_many::<u32>("locked_mem_clocks") {
        let clocks: Vec<u32> = locked_mem_clocks.copied().collect();
        if clocks.len() == 2 {
            for gpu in gpus {
                match run(
                    gpu,
                    SetLockedClocks {
                        domain: ClockDomain::Memory,
                        min_mhz: clocks[0],
                        max_mhz: clocks[1],
                    },
                ) {
                    Ok(_) => println!(
                        "Successfully locked NVML Memory clocks (Min: {}, Max: {}) to GPU {}",
                        clocks[0], clocks[1], gpu.id.0
                    ),
                    Err(e) => eprintln!(
                        "Failed to lock NVML Memory clocks for GPU {}: {:?}",
                        gpu.id.0, e
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

        for gpu in gpus {
            match run(
                gpu,
                SetNvmlPstateLock {
                    first_pstate,
                    second_pstate,
                },
            ) {
                Ok(report) => {
                    let (range_label, min_lock_mhz, max_lock_mhz) = report.output;
                    println!(
                        "Successfully locked GPU {} to {} via NVML memory window {}-{} MHz",
                        gpu.id.0, range_label, min_lock_mhz, max_lock_mhz,
                    );
                }
                Err(e) => eprintln!(
                    "Failed to lock GPU {} to NVML PState {}: {:?}",
                    gpu.id.0,
                    requested_pstates.join(" "),
                    e
                ),
            }
        }
    }

    if matches.get_flag("reset_mem_clocks") {
        for gpu in gpus {
            match run(
                gpu,
                ResetLockedClocks {
                    domain: ClockDomain::Memory,
                },
            ) {
                Ok(_) => println!(
                    "Successfully reset NVML Memory locked clocks to GPU {}",
                    gpu.id.0
                ),
                Err(e) => eprintln!(
                    "Failed to reset NVML Memory locked clocks for GPU {}: {:?}",
                    gpu.id.0, e
                ),
            }
        }
    }

    Ok(())
}

pub fn handle_nvml_cooler(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
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

    for gpu in gpus {
        let fan_info = run(gpu, QueryFanInfo)?.output;
        let fan_count = fan_info.count;
        let fan_indices: Vec<u32> = match cooler_id {
            "1" => vec![0],
            "2" => {
                if fan_count < 2 {
                    return Err(Error::Custom(format!(
                        "GPU {} reports only {} fan(s), cooler id 2 is unavailable",
                        gpu.id.0, fan_count
                    )));
                }
                vec![1]
            }
            _ => (0..fan_count).collect(),
        };

        for fan_idx in fan_indices {
            match run(
                gpu,
                SetFanSpeed {
                    fan_index: fan_idx,
                    policy,
                    level,
                },
            ) {
                Ok(_) => println!(
                    "Successfully applied NVML cooler policy {:?}, level {}% to GPU {} fan {}",
                    policy,
                    level,
                    gpu.id.0,
                    fan_idx + 1
                ),
                Err(e) => eprintln!(
                    "Failed to set NVML cooler for GPU {} fan {}: {:?}",
                    gpu.id.0,
                    fan_idx + 1,
                    e
                ),
            }
        }
    }

    Ok(())
}

pub fn handle_reset_nvml_cooler(gpus: &[GpuTarget<'_>], matches: &ArgMatches) -> Result<(), Error> {
    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    for gpu in gpus {
        handle_reset_nvml_cooler_single_gpu(gpu, cooler_id)?;
    }

    Ok(())
}

pub fn handle_reset_nvml_cooler_single_gpu(
    gpu: &GpuTarget<'_>,
    cooler_id: &str,
) -> Result<(), Error> {
    let fan_count = run(gpu, QueryFanInfo)?.output.count;

    let fan_indices: Vec<u32> = match cooler_id {
        "1" => vec![0],
        "2" => {
            if fan_count < 2 {
                return Err(Error::Custom(format!(
                    "GPU {} reports only {} fan(s), cooler id 2 is unavailable",
                    gpu.id.0, fan_count
                )));
            }
            vec![1]
        }
        _ => (0..fan_count).collect(),
    };

    for fan_idx in fan_indices {
        match run(gpu, ResetFanSpeed { fan_index: fan_idx }) {
            Ok(_) => println!(
                "Successfully restored NVML default fan speed on GPU {} fan {}",
                gpu.id.0,
                fan_idx + 1
            ),
            Err(e) => eprintln!(
                "Failed to restore NVML default fan speed for GPU {} fan {}: {:?}",
                gpu.id.0,
                fan_idx + 1,
                e
            ),
        }
    }

    Ok(())
}
