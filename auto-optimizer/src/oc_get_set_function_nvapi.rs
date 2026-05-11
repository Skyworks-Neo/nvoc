use crate::basic_func::{select_gpus, GpuSelector};
use crate::conv::ConvertEnum;
use crate::error::Error;
use crate::human::print_scan_separator;
use crate::nvidia_gpu_type::{fetch_gpu_type, GpuType};
use crate::types::NvapiLockedVoltageTarget;
use crate::types::VfpResetDomain;
use clap::ArgMatches;
use num_traits::abs;
use nvapi_hi::nvapi::{CelsiusShifted, ClockFrequencyType};
use nvapi_hi::{
    Celsius, ClockDomain, ClockLockEntry, ClockLockValue, CoolerPolicy, CoolerSettings,
    FanCoolerId, Gpu, Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta, PState, Percentage,
    PerfLimitId, PffCurve, PffPoint, VfpPoint,
};
use std::collections::BTreeMap;
use std::iter;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

/// 通过 NvAPI_GPU_SetPstates20 的 baseVoltages 字段写入指定 pstate 的核心电压 delta。
/// 适用于 900 系（Maxwell）及更早不支持 ClientVoltRailsSetControl 的 GPU。
/// delta_uv 单位为 μV，正值加压，负值降压，范围由 GPU 自身 voltDelta_uV.{min,max} 决定。
/// target_pstate 指定目标 pstate，默认应传入 PState::P0。
pub fn set_pstate_base_voltage(
    gpu: &Gpu,
    delta_uv: MicrovoltsDelta,
    target_pstate: PState,
) -> Result<(), Error> {
    use nvapi_hi::sys::gpu::pstate as sys_pstate;

    // 1. 先读取当前 pstate 信息，确认目标 pstate 的 baseVoltages 可写并取得允许范围
    let pstates = gpu
        .inner()
        .pstates()
        .map_err(|e| Error::from(format!("Failed to read pstates: {:?}", e)))?;

    let target_ps = pstates
        .pstates
        .iter()
        .find(|p| p.id == target_pstate)
        .ok_or_else(|| Error::from(format!("{:?} pstate not found", target_pstate)))?;

    let base_volt = target_ps
        .base_voltages
        .iter()
        .find(|v| v.voltage_domain == nvapi_hi::nvapi::VoltageDomain::Core)
        .ok_or_else(|| {
            Error::from(format!(
            "{:?} Core baseVoltage entry not found — GPU may not support pstate voltage control",
            target_pstate
        ))
        })?;

    if !base_volt.editable {
        return Err(Error::from(format!(
            "{:?} Core baseVoltage is not editable on this GPU",
            target_pstate
        )));
    }

    let range = base_volt.voltage_delta.range;
    if delta_uv.0 < range.min.0 || delta_uv.0 > range.max.0 {
        return Err(Error::from(format!(
            "voltage delta {}μV out of allowed range [{}μV, {}μV]",
            delta_uv.0, range.min.0, range.max.0
        )));
    }

    // 2. 构造最小化的 NV_GPU_PERF_PSTATES20_INFO，只填目标 pstate 的 baseVoltages，不修改时钟
    let mut info = sys_pstate::NV_GPU_PERF_PSTATES20_INFO::default();
    info.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);
    info.numPstates = 1;
    info.numClocks = 0; // 不修改时钟
    info.numBaseVoltages = 1; // 一个核心电压条目

    {
        let pe = &mut info.pstates[0];
        pe.pstateId = target_pstate.raw();
        pe.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);

        let ve = &mut pe.baseVoltages[0];
        ve.domainId = nvapi::VoltageDomain::Core.raw();
        ve.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);
        ve.volt_uV = base_volt.voltage.0;
        ve.voltDelta_uV.value = delta_uv.0;
        ve.voltDelta_uV.min = range.min.0;
        ve.voltDelta_uV.max = range.max.0;
    }

    // 3. 调用私有 undocumented API，返回值 0 = NVAPI_OK
    let status =
        unsafe { sys_pstate::private::NvAPI_GPU_SetPstates20(*gpu.inner().handle(), &info) };

    if status != 0 {
        return Err(Error::from(format!(
            "NvAPI_GPU_SetPstates20 failed with status code {}",
            status
        )));
    }

    println!(
        "{:?} Core voltage delta set to {:+} μV ({:+.3} mV)",
        target_pstate,
        delta_uv.0,
        delta_uv.0 as f64 / 1000.0
    );
    Ok(())
}

/// 将所有 pstate 的 Core baseVoltage delta 清零（适用于 Maxwell / 9 系及更早）。
/// 遍历驱动报告的全部 pstate，对每个含有可编辑 Core baseVoltage 的条目发起单独写入，
/// 单个 pstate 失败时打印警告并继续，不中断其他 pstate 的清零。
pub fn reset_all_pstate_base_voltages(gpu: &Gpu) -> Result<(), Error> {
    use nvapi_hi::sys::gpu::pstate as sys_pstate;

    let pstates = gpu
        .inner()
        .pstates()
        .map_err(|e| Error::from(format!("Failed to read pstates: {:?}", e)))?;

    let mut any_written = false;

    for ps in &pstates.pstates {
        // 找到 Core 域的 baseVoltage 条目
        let base_volt = match ps
            .base_voltages
            .iter()
            .find(|v| v.voltage_domain == nvapi_hi::nvapi::VoltageDomain::Core)
        {
            Some(v) => v,
            None => continue, // 该 pstate 无 Core 电压条目，跳过
        };

        if !base_volt.editable {
            continue; // 不可编辑，跳过
        }

        let range = base_volt.voltage_delta.range;

        // 构造单 pstate 写入结构
        let mut info = sys_pstate::NV_GPU_PERF_PSTATES20_INFO::default();
        info.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);
        info.numPstates = 1;
        info.numClocks = 0;
        info.numBaseVoltages = 1;

        {
            let pe = &mut info.pstates[0];
            pe.pstateId = ps.id.raw();
            pe.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);

            let ve = &mut pe.baseVoltages[0];
            ve.domainId = nvapi::VoltageDomain::Core.raw();
            ve.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);
            ve.volt_uV = base_volt.voltage.0;
            ve.voltDelta_uV.value = 0; // 清零
            ve.voltDelta_uV.min = range.min.0;
            ve.voltDelta_uV.max = range.max.0;
        }

        let status =
            unsafe { sys_pstate::private::NvAPI_GPU_SetPstates20(*gpu.inner().handle(), &info) };

        if status != 0 {
            eprintln!(
                "Warning: NvAPI_GPU_SetPstates20 failed for {:?} with status {}, skipping.",
                ps.id, status
            );
        } else {
            println!("{:?} Core voltage delta cleared (set to 0 μV).", ps.id);
            any_written = true;
        }
    }

    if any_written {
        println!("All editable pstate Core voltage deltas have been reset.");
    } else {
        println!("No editable pstate Core voltage entries found — nothing to reset.");
    }

    Ok(())
}

// In oc_set_function

pub fn handle_cooler_command(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
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

    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    let settings = CoolerSettings {
        policy: mode,
        level: Some(Percentage(level)),
    };

    for gpu in gpus {
        match cooler_id {
            "1" => {
                gpu.set_cooler_levels([(FanCoolerId::Cooler1, settings)].into_iter())?;
            }
            "2" => {
                gpu.set_cooler_levels([(FanCoolerId::Cooler2, settings)].into_iter())?;
            }
            _ => {
                // "all" or anything else: set both coolers
                gpu.set_cooler_levels(
                    [
                        (FanCoolerId::Cooler1, settings),
                        (FanCoolerId::Cooler2, settings),
                    ]
                    .into_iter(),
                )?;
            }
        }
    }
    Ok(())
}

pub fn get_voltage_by_point(gpu: &Gpu, point: usize) -> Result<Microvolts, Error> {
    let status = gpu.status()?;
    let v = status
        .vfp
        .ok_or(Error::VfpUnsupported)?
        .graphics
        .get(&(point))
        .ok_or(Error::Str("invalid point index"))?
        .voltage;

    Ok(v)
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

    if let Some(lower) = lower_mhz {
        if lower > upper_mhz {
            return Err(Error::from(
                "--clock expects upper bound first and lower bound second",
            ));
        }
    }

    let domain = match matches
        .get_one::<String>("domain")
        .map(|s| s.as_str())
        .unwrap_or("Graphics")
    {
        "Graphics" => ClockDomain::Graphics,
        "Memory" => ClockDomain::Memory,
        other => ClockDomain::from_str(other)
            .map_err(|e| Error::from(format!("Invalid --domain value '{}': {}", other, e)))?,
    };

    Ok((
        domain,
        Kilohertz(upper_mhz.saturating_mul(1000)),
        lower_mhz.map(|v| Kilohertz(v.saturating_mul(1000))),
    ))
}

fn set_vfp_frequency_lock(
    gpu: &Gpu,
    domain: ClockDomain,
    upper: Kilohertz,
    lower: Option<Kilohertz>,
) -> Result<(), Error> {
    match lower {
        None => {
            gpu.set_vfp_lock(domain, Some(upper))?;
        }
        Some(lower) => {
            let (upper_limit, lower_limit) = match domain {
                ClockDomain::Graphics => (PerfLimitId::Gpu, PerfLimitId::GpuUnknown),
                ClockDomain::Memory => (PerfLimitId::Memory, PerfLimitId::MemoryUnknown),
                _ => {
                    return Err(Error::from(
                        "--domain must be Graphics or Memory in --clock mode",
                    ));
                }
            };

            gpu.inner()
                .set_vfp_locks([
                    ClockLockEntry {
                        limit: upper_limit,
                        clock: domain,
                        lock_value: Some(ClockLockValue::Frequency(upper)),
                    },
                    ClockLockEntry {
                        limit: lower_limit,
                        clock: domain,
                        lock_value: Some(ClockLockValue::Frequency(lower)),
                    },
                ])
                .map_err(Error::from)?;
        }
    }
    Ok(())
}

pub fn reset_vfp_frequency_lock(gpu: &Gpu, domain: ClockDomain) -> Result<(), Error> {
    let (upper_limit, lower_limit) = match domain {
        ClockDomain::Graphics => (PerfLimitId::Gpu, PerfLimitId::GpuUnknown),
        ClockDomain::Memory => (PerfLimitId::Memory, PerfLimitId::MemoryUnknown),
        _ => {
            return Err(Error::from(
                "--domain must be Graphics or Memory in --clock mode",
            ));
        }
    };

    gpu.inner()
        .set_vfp_locks([
            ClockLockEntry {
                limit: upper_limit,
                clock: domain,
                lock_value: None,
            },
            ClockLockEntry {
                limit: lower_limit,
                clock: domain,
                lock_value: None,
            },
        ])
        .map_err(Error::from)?;

    Ok(())
}

fn parse_lock_voltage(
    gpu: &Gpu,
    matches: &ArgMatches,
    default_point: usize,
) -> Result<Microvolts, Error> {
    let raw_target = matches
        .get_one::<String>("point")
        .map(|s| s.as_str())
        .unwrap_or("");

    if matches
        .try_get_one::<bool>("voltage")
        .is_ok_and(|v| v.copied().unwrap_or(false))
    {
        let input_voltage = raw_target.parse::<u32>()?;
        let voltage_uv = if input_voltage >= 10_000 {
            input_voltage
        } else {
            input_voltage * 1000
        };
        Ok(Microvolts(voltage_uv))
    } else {
        let point = raw_target.parse::<usize>().unwrap_or(default_point);
        get_voltage_by_point(gpu, point)
    }
}

fn parse_nvapi_locked_voltage_target(raw: &str) -> Result<NvapiLockedVoltageTarget, Error> {
    let input = raw.trim();
    let lower = input.to_ascii_lowercase();

    if let Some(v) = lower.strip_suffix("mv") {
        let mv = u32::from_str(v.trim()).map_err(|_| {
            Error::from("Invalid --nvapi-locked-voltage value: expected POINT or <N>mV/<N>uV")
        })?;
        return Ok(NvapiLockedVoltageTarget::Voltage(Microvolts(
            mv.saturating_mul(1000),
        )));
    }

    if let Some(v) = lower.strip_suffix("uv") {
        let uv = u32::from_str(v.trim()).map_err(|_| {
            Error::from("Invalid --nvapi-locked-voltage value: expected POINT or <N>mV/<N>uV")
        })?;
        return Ok(NvapiLockedVoltageTarget::Voltage(Microvolts(uv)));
    }

    let point = usize::from_str(input).map_err(|_| {
        Error::from("Invalid --nvapi-locked-voltage value: expected POINT or <N>mV/<N>uV")
    })?;
    Ok(NvapiLockedVoltageTarget::Point(point))
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

const NVAPI_PSTATE_LOCK_MARGIN_MHZ: u32 = 50;

fn nvapi_ranges_overlap(a_min: u32, a_max: u32, b_min: u32, b_max: u32) -> bool {
    a_min <= b_max && b_min <= a_max
}

pub fn set_nvapi_pstate_lock(
    nvml: &nvml_wrapper::Nvml,
    gpu: &Gpu,
    gpu_id: u32,
    first_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
    second_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Result<(String, u32, u32), Error> {
    let pstates =
        crate::oc_get_set_function_nvml::get_nvml_pstate_info(nvml, gpu_id).ok_or_else(|| {
            Error::Custom(format!(
                "Failed to query NVML P-State information for GPU {}",
                gpu_id
            ))
        })?;

    let first_index = crate::conv::nvml_pstate_to_index(first_pstate)?;
    let second_index = crate::conv::nvml_pstate_to_index(second_pstate)?;
    let (high_perf_pstate, low_perf_pstate, min_index, max_index) = if first_index <= second_index {
        (first_pstate, second_pstate, first_index, second_index)
    } else {
        (second_pstate, first_pstate, second_index, first_index)
    };

    let range_label = if min_index == max_index {
        crate::conv::nvml_pstate_to_str(high_perf_pstate).to_string()
    } else {
        format!(
            "{}-{}",
            crate::conv::nvml_pstate_to_str(high_perf_pstate),
            crate::conv::nvml_pstate_to_str(low_perf_pstate)
        )
    };

    let supported_pstates = pstates
        .iter()
        .map(|(reported_pstate, _, _, _, _)| {
            crate::conv::nvml_pstate_to_str(*reported_pstate)
                .trim_start_matches('P')
                .to_string()
        })
        .collect::<Vec<_>>();

    let high_perf_entry = pstates
        .iter()
        .find(|(reported_pstate, _, _, _, _)| *reported_pstate == high_perf_pstate)
        .ok_or_else(|| {
            Error::Custom(format!(
                "{} is not reported by NVML for GPU {}. Supported NVML P-States: {}",
                crate::conv::nvml_pstate_to_str(high_perf_pstate),
                gpu_id,
                supported_pstates.join(",")
            ))
        })?;
    let low_perf_entry = pstates
        .iter()
        .find(|(reported_pstate, _, _, _, _)| *reported_pstate == low_perf_pstate)
        .ok_or_else(|| {
            Error::Custom(format!(
                "{} is not reported by NVML for GPU {}. Supported NVML P-States: {}",
                crate::conv::nvml_pstate_to_str(low_perf_pstate),
                gpu_id,
                supported_pstates.join(",")
            ))
        })?;

    let min_target_mem_clock_mhz = low_perf_entry.3;
    let max_target_mem_clock_mhz = high_perf_entry.4;
    let min_lock_mhz = min_target_mem_clock_mhz.saturating_sub(NVAPI_PSTATE_LOCK_MARGIN_MHZ);
    let max_lock_mhz = max_target_mem_clock_mhz.saturating_add(NVAPI_PSTATE_LOCK_MARGIN_MHZ);

    let overlapping_pstates = pstates
        .iter()
        .filter(|(_, _, _, min_mem_mhz, max_mem_mhz)| {
            nvapi_ranges_overlap(*min_mem_mhz, *max_mem_mhz, min_lock_mhz, max_lock_mhz)
        })
        .map(|(reported_pstate, _, _, _, _)| {
            (
                crate::conv::nvml_pstate_to_index(*reported_pstate),
                crate::conv::nvml_pstate_to_str(*reported_pstate),
            )
        })
        .collect::<Vec<_>>();

    let outside_requested_range = overlapping_pstates
        .iter()
        .filter_map(|(reported_index, reported_label)| {
            reported_index.as_ref().ok().and_then(|reported_index| {
                if *reported_index < min_index || *reported_index > max_index {
                    Some(*reported_label)
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();

    if !outside_requested_range.is_empty() {
        return Err(Error::Custom(format!(
            "{} would map to memory lock window {}-{} MHz, but that also overlaps NVML P-States outside the requested range: {}. Use --locked-mem-clocks for a manual NVAPI range instead.",
            range_label,
            min_lock_mhz,
            max_lock_mhz,
            outside_requested_range.join(", "),
        )));
    }

    set_vfp_frequency_lock(
        gpu,
        ClockDomain::Memory,
        Kilohertz(max_lock_mhz.saturating_mul(1000)),
        Some(Kilohertz(min_lock_mhz.saturating_mul(1000))),
    )?;

    Ok((range_label, min_lock_mhz, max_lock_mhz))
}

pub fn handle_lock_vfp(
    gpus: &[&Gpu],
    matches: &ArgMatches,
    default_point: usize,
    feedback_flag: bool,
) -> Result<(), Error> {
    // NVAPI set 直传参数：统一在 handle_lock_vfp 内处理电压锁定与核心/显存锁频。
    let mut handled_nvapi_lock = false;

    if let Some(locked_voltage_raw) = matches.get_one::<String>("locked_voltage") {
        handled_nvapi_lock = true;
        let target = parse_nvapi_locked_voltage_target(locked_voltage_raw.as_str())?;

        for gpu in gpus {
            let gpu_info = gpu.info()?;
            let voltage = match target {
                NvapiLockedVoltageTarget::Point(point) => match get_voltage_by_point(gpu, point) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "Failed to resolve VFP point {} for GPU {}: {:?}",
                            point, gpu_info.id, e
                        );
                        continue;
                    }
                },
                NvapiLockedVoltageTarget::Voltage(v) => v,
            };

            match gpu.set_vfp_lock_voltage(Some(voltage)) {
                Ok(_) => match target {
                    NvapiLockedVoltageTarget::Point(point) => println!(
                        "Successfully locked GPU {} on VFP point {} ({} mV)",
                        gpu_info.id,
                        point,
                        voltage.0 / 1000
                    ),
                    NvapiLockedVoltageTarget::Voltage(_) => println!(
                        "Successfully applied NVAPI locked voltage {} mV to GPU {}",
                        voltage.0 / 1000,
                        gpu_info.id
                    ),
                },
                Err(e) => eprintln!(
                    "Failed to set NVAPI locked voltage for GPU {}: {:?}",
                    gpu_info.id, e
                ),
            }
        }
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_core_clocks")?
    {
        handled_nvapi_lock = true;
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match set_vfp_frequency_lock(
                gpu,
                ClockDomain::Graphics,
                Kilohertz(max_clock.saturating_mul(1000)),
                Some(Kilohertz(min_clock.saturating_mul(1000))),
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
    }

    if let Some((min_clock, max_clock)) =
        parse_nvapi_locked_clock_range(matches, "locked_mem_clocks")?
    {
        handled_nvapi_lock = true;
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match set_vfp_frequency_lock(
                gpu,
                ClockDomain::Memory,
                Kilohertz(max_clock.saturating_mul(1000)),
                Some(Kilohertz(min_clock.saturating_mul(1000))),
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
    }

    if handled_nvapi_lock {
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
            set_vfp_frequency_lock(gpu, domain, upper, lower)?;
            if feedback_flag {
                match lower {
                    Some(lower) => println!(
                        "Target: {:?} lock upper={} MHz, lower={} MHz",
                        domain,
                        upper.0 / 1000,
                        lower.0 / 1000
                    ),
                    None => println!("Target: {:?} lock = {} MHz", domain, upper.0 / 1000),
                }
            }
        }
        return Ok(());
    }

    for gpu in gpus {
        let v = parse_lock_voltage(gpu, matches, default_point)?;

        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info);
        let lock_params = gpu_type
            .as_ref()
            .map(|t| t.voltage_lock_params())
            .unwrap_or_default();
        let skew_rate_enabled = if lock_params.skew_rate_enabled { 1 } else { 0 };
        let crit_volt_margin = lock_params.crit_volt_margin;

        gpu.set_vfp_lock_voltage(Some(v))?;
        if !feedback_flag {
            sleep(Duration::from_millis(500));
            return Ok(());
        }

        let mut core_v = gpu.inner().core_voltage()?;
        println!(
            "Target: {} mV, Current: v = {} mV, Margin: {} mV",
            v.0 / 1000,
            core_v.0 / 1000,
            (v.0 as i32 - core_v.0 as i32) / 1000
        );

        let mut count = 0;
        let mut flag = 0;
        let mut skew_rate = 45;

        while (v.0 as i32 - core_v.0 as i32) / 1000 > crit_volt_margin
            || (core_v.0 as i32 - v.0 as i32) / 1000 > crit_volt_margin
        {
            if count >= 4 {
                flag = 1;
                println!("voltage lock failed!");
                break;
            }

            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    (abs(v.0 as i32 - core_v.0 as i32) / skew_rate) as u64,
                ));
            }
            gpu.set_vfp_lock_voltage(Some(v))?;
            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    (abs(v.0 as i32 - core_v.0 as i32) / skew_rate) as u64,
                ));
            }
            core_v = gpu.inner().core_voltage()?;
            println!(
                "Attempt {} FAILED==Target: {} mV, Current: v = {} mV, Margin: {} mV",
                count,
                v.0 / 1000,
                core_v.0 / 1000,
                (v.0 as i32 - core_v.0 as i32) / 1000
            );
            println!(
                "Current assumed skewrate: {}mV/s, TAU SET TO {}ms",
                skew_rate,
                (abs(v.0 as i32 - core_v.0 as i32) / skew_rate) as u64
            );
            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    (abs(v.0 as i32 - core_v.0 as i32) / skew_rate) as u64,
                ));
            }
            count += 1;
            skew_rate -= 4;
        }

        if count < 5 {
            println!("SUCCESS",);
        }
        if flag == 1 {
            return Err(Error::Str("Failed to lock voltage"));
        }
    }
    Ok(())
}

pub fn reset_vfp_deltas(gpu: &Gpu, domain: VfpResetDomain) -> nvapi_hi::Result<()> {
    let info = gpu.info()?;
    let gpu_type = fetch_gpu_type(&info);

    // 9 系及更早（Maxwell 及之前）不支持 VFP 曲线，只能通过 set_pstates 单点清零
    let is_legacy = matches!(
        gpu_type,
        Ok(GpuType::Mobile9Series)
            | Ok(GpuType::Desktop9Series)
            | Ok(GpuType::ComputationVolta)
            | Ok(GpuType::Unknown)
            | Err(_)
    );

    let reset_graphics = matches!(domain, VfpResetDomain::All | VfpResetDomain::Core);
    let reset_memory = matches!(domain, VfpResetDomain::All | VfpResetDomain::Memory);

    if is_legacy {
        let mut any_ok = false;
        if reset_graphics {
            if let Err(e) = gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            ) {
                // TDR 恢复期间调用可能失败，warn 后继续，不中断扫描流程
                eprintln!(
                    "Warning: legacy pstate Graphics reset failed (GPU may be recovering from TDR): {:?}",
                    e
                );
            } else {
                any_ok = true;
            }
        }
        if reset_memory {
            if let Err(e) = gpu.inner().set_pstates(
                [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                    .iter()
                    .cloned(),
            ) {
                eprintln!(
                    "Warning: legacy pstate Memory reset failed (GPU may be recovering from TDR): {:?}",
                    e
                );
            } else {
                any_ok = true;
            }
        }
        if any_ok {
            println!("vfp reset ({:?}) done (legacy pstate mode)", domain);
            return Ok(());
        }
    }

    // 快速路径：通过 set_pstates 分别清零 Graphics 和 Memory 的 P0 偏置
    // 等价于命令行 `set OC -p P0`，速度远快于逐点 VFP 循环
    // 两者独立容错，TDR 恢复期间单项失败不阻断另一项的重置
    let graphics_ok = if reset_graphics {
        let r = gpu.inner().set_pstates(
            [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                .iter()
                .cloned(),
        );
        if let Err(e) = &r {
            eprintln!("Warning: fast pstate Graphics reset failed: {:?}", e);
        }
        r.is_ok()
    } else {
        false
    };

    let memory_ok = if reset_memory {
        let r = gpu.inner().set_pstates(
            [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                .iter()
                .cloned(),
        );
        if let Err(e) = &r {
            eprintln!("Warning: fast pstate Memory reset failed: {:?}", e);
        }
        r.is_ok()
    } else {
        false
    };

    if graphics_ok || memory_ok {
        println!("vfp reset ({:?}) done (fast pstate path)", domain);
        return Ok(());
    }

    // 保底路径：逐点清零 VFP 曲线
    eprintln!("Warning: fast pstate reset failed, falling back to point-by-point VFP reset...");
    let point_range: usize = gpu_type
        .as_ref()
        .map(|t| t.vfp_point_range())
        .unwrap_or(126);

    for point in 0..=point_range {
        match domain {
            VfpResetDomain::All => {
                gpu.set_vfp(
                    iter::once((point, KilohertzDelta(0))),
                    iter::once((point, KilohertzDelta(0))),
                )?;
            }
            VfpResetDomain::Core => {
                gpu.set_vfp(iter::once((point, KilohertzDelta(0))), iter::empty())?;
            }
            VfpResetDomain::Memory => {
                gpu.set_vfp(iter::empty(), iter::once((point, KilohertzDelta(0))))?;
            }
        }
    }
    println!(
        "vfp reset ({:?}) done (fallback point-by-point path)",
        domain
    );
    Ok(())
}

pub fn core_reset_vfp(gpu: &Gpu) -> nvapi_hi::Result<()> {
    reset_vfp_deltas(gpu, VfpResetDomain::All)
}

// oc_profile_function.rs

pub fn single_point_adj(gpus: &Vec<&Gpu>, matches: &ArgMatches) -> Result<(), Error> {
    let point_start = *matches.get_one::<u32>("point_start").unwrap() as usize;
    let delta_ini = *matches.get_one::<i32>("delta").unwrap();

    for gpu in gpus {
        gpu.set_vfp(
            iter::once((point_start, KilohertzDelta(delta_ini))),
            iter::empty(),
        )?;
    }

    Ok(())
}

/// Handle `set vfp pointwiseoc <RANGE> <DELTA>` command.
/// RANGE format: "start-end" (e.g. "39-76", both ends inclusive).
/// DELTA unit: kHz (e.g. +150000 or -50000).
pub fn handle_pointwiseoc(gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let range_str = matches
        .get_one::<String>("range")
        .ok_or_else(|| Error::from("Missing required argument: RANGE"))?;

    // Parse "start-end"; support leading '+' on delta side won't be here, but the range
    // itself may look like "39-76" – split on '-' keeping in mind it can only appear once
    // as a separator between two non-negative integers.
    let (start, end) = {
        let parts: Vec<&str> = range_str.splitn(2, '-').collect();
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
        if start > end {
            return Err(Error::from(format!(
                "Range start ({}) must be <= end ({}).",
                start, end
            )));
        }
        (start, end)
    };

    let delta = *matches
        .get_one::<i32>("delta")
        .ok_or_else(|| Error::from("Missing required argument: DELTA"))?;

    println!(
        "pointwiseoc: applying delta {} kHz to VFP points {}..={} (inclusive)",
        delta, start, end
    );

    for gpu in gpus {
        set_vfp_range(&gpu, start..=end, delta)?;
    }

    Ok(())
}

pub fn handle_test_voltage_limits(
    gpus: &[&Gpu],
    matches: &ArgMatches,
) -> Result<(usize, usize), Error> {
    let mut upper_init_point: usize = 70;
    let mut lower_init_point: usize = 60;
    let mut vfp_strict_inc_flag = 0;
    let mut margin_threshold_check = 0;

    for gpu in gpus {
        core_reset_vfp(gpu)?;

        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info);

        if info.name.contains("Laptop") || info.name.contains("Device") {
            println!("TDP/Temp/VDDQ control not available on MOBILE chips! Skipping...");
        } else {
            match gpu.set_voltage_boost(Percentage(100)) {
                Ok(_) => {
                    println!("Successfully set VDDQ boost to +100% (max allowed V_core in fact).")
                }
                Err(e) => eprintln!("Failed to set VDDQ boost: {:?}", e),
            }
        }

        match gpu_type {
            Ok(ref t) => {
                let vlp = t.voltage_limit_params();
                upper_init_point = vlp.upper_init_point;
                lower_init_point = vlp.lower_init_point;
                if vlp.vfp_strict_inc_flag {
                    vfp_strict_inc_flag = 1;
                }
                if vlp.margin_threshold_check {
                    margin_threshold_check = 1;
                }

                // 9 系及 Volta/Unknown 的特殊打印（无 VFP 支持）
                match t {
                    GpuType::Mobile9Series => {
                        println!("Mobile 9 Series GPU detected.");
                        drop(Error::VfpUnsupported);
                    }
                    GpuType::Desktop9Series => {
                        println!("Desktop 9 Series GPU detected.");
                        drop(Error::VfpUnsupported);
                    }
                    GpuType::ComputationVolta => {
                        println!("Computation Volta GPU detected.");
                    }
                    GpuType::Unknown => {
                        println!("Unknown GPU type detected.");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let mut current_test_point = upper_init_point;
    let mut voltage: Option<Microvolts> = None;
    let mut upper_voltage_old = Microvolts(0);
    let mut lower_voltage_old = Microvolts(0);
    let mut flat_curve_modifier_flag = vfp_strict_inc_flag;
    let mut revert_scan_flag = 0;
    let mut upper_target_point: usize = 0;

    // Iterate over a range of test voltage values
    let lower_target_point: usize = loop {
        if revert_scan_flag == 0 {
            println!("\nTesting upper voltage limit...");
        } else {
            println!("\nTesting lower voltage limit...");
        }

        for gpu in gpus {
            let gpu_status = gpu.status()?;
            voltage = Some(
                gpu_status
                    .vfp
                    .ok_or(Error::VfpUnsupported)?
                    .graphics
                    .get(&(current_test_point))
                    .ok_or(Error::Str("invalid point index"))?
                    .voltage,
            );
        }

        let voltage = voltage.ok_or(Error::VfpUnsupported)?;

        match handle_lock_vfp(gpus, matches, current_test_point, true) {
            Ok(_) => {
                println!("Successfully set voltage to {} mV", voltage.0 as i32 / 1000);
                if revert_scan_flag == 0 {
                    current_test_point += 1;
                    upper_voltage_old = voltage;
                } else {
                    current_test_point -= 1;
                    lower_voltage_old = voltage;
                }
                if vfp_strict_inc_flag == 1 {
                    flat_curve_modifier_flag = 1;
                }
            }
            Err(e) => {
                println!(
                    "Failed to set voltage to {} mV. Error: {:?}",
                    voltage.0 as i32 / 1000,
                    e
                );

                if vfp_strict_inc_flag == 1 && flat_curve_modifier_flag == 1 {
                    println!("May stuck on flat curve, modifying...");
                    for gpu in gpus {
                        if revert_scan_flag == 0 {
                            gpu.set_vfp(
                                iter::once((current_test_point - 1, KilohertzDelta(-45000))),
                                iter::empty(),
                            )?;
                        } else {
                            gpu.set_vfp(
                                iter::once((current_test_point, KilohertzDelta(45000))),
                                iter::empty(),
                            )?;
                        }
                    }
                    flat_curve_modifier_flag = 0;
                    continue;
                } else {
                    println!("Exceeding flat modifier limit, considered reaching TRUE limit!");
                }

                if revert_scan_flag == 0 {
                    upper_target_point = current_test_point - 1;
                    print_scan_separator();
                    println!(
                        "Upper voltage limit seems to be:{}mV @ point {}",
                        upper_voltage_old.0 as i32 / 1000,
                        upper_target_point - margin_threshold_check
                    );
                    println!("Reverting Scanner direction...");
                    print_scan_separator();
                    revert_scan_flag = 1;
                    flat_curve_modifier_flag = 1;
                    for gpu in gpus {
                        core_reset_vfp(gpu)?;
                    }
                    current_test_point = lower_init_point;
                } else {
                    let lower_target_point = current_test_point + 1;
                    print_scan_separator();
                    println!(
                        "Upper voltage limit seems to be:{}mV @ point {}",
                        upper_voltage_old.0 as i32 / 1000,
                        upper_target_point - margin_threshold_check
                    );
                    println!(
                        "Lower voltage limit seems to be:{}mV @ point {}",
                        lower_voltage_old.0 as i32 / 1000,
                        lower_target_point + margin_threshold_check
                    );
                    print_scan_separator();
                    break lower_target_point;
                }
            }
        }
    };
    for gpu in gpus {
        core_reset_vfp(gpu)?;
    }
    Ok((
        lower_target_point + margin_threshold_check,
        upper_target_point - margin_threshold_check,
    ))
}

pub fn get_gpu_tdp_temp_limit(
    arg_matches: ArgMatches,
) -> Result<
    (
        Percentage,
        Percentage,
        Percentage,
        Celsius,
        Celsius,
        Celsius,
        PffCurve,
    ),
    Error,
> {
    let mut min_tdp = 16383.0_f32;
    let mut max_tdp = 32767.0_f32;
    let mut default_tdp = 65535.0_f32;

    let mut min_tdp_percentage = Percentage(2047);
    let mut max_tdp_percentage = Percentage(4095);
    let mut default_tdp_percentage = Percentage(8191);

    let mut min_temp_lim = Celsius(127);
    let mut max_temp_lim = Celsius(255);
    let mut default_temp_lim = Celsius(511);

    // Nvidia encodes temperature as << 8 for some reason sometimes.
    let pff_current_point = vec![
        PffPoint {
            x: CelsiusShifted(100 << 8),
            y: Kilohertz(3300000),
        },
        PffPoint {
            x: CelsiusShifted(110 << 8),
            y: Kilohertz(3300000),
        },
        PffPoint {
            x: CelsiusShifted(120 << 8),
            y: Kilohertz(3300000),
        },
    ];

    let mut current_pff_curve = PffCurve {
        points: pff_current_point,
    };

    let selector = GpuSelector::from_clap(arg_matches.get_many::<String>("gpu"));
    let gpu_list = crate::basic_func::get_sorted_gpus()?;
    let gpus = select_gpus(&gpu_list, &selector)?;

    let nvml = nvml_wrapper::Nvml::init().ok();
    for gpu in gpus.iter() {
        let info = gpu.info()?;

        // 使用 NVAPI GPU ID 直接查询（公式：GPU_ID = PCI_Bus × 256）
        if let Some((min_w, current_w, max_w)) = nvml.as_ref().and_then(|n| {
            crate::oc_get_set_function_nvml::query_nvml_power_watts(n, info.id as u32)
        }) {
            min_tdp = min_w;
            default_tdp = current_w;
            max_tdp = max_w;
        } else {
            eprintln!(
                "Warning: Failed to query NVML power limits for GPU id {}, using placeholder values",
                info.id
            );
        }

        //power limit readout
        for limit in info.power_limits.iter() {
            max_tdp_percentage = limit.range.max;
            min_tdp_percentage = limit.range.min;
            default_tdp_percentage = limit.default;
            print_scan_separator();
            println!("Power Limit: {} ({} default)", limit.range, limit.default);
            println!(
                "Min TDP: {:.2}W ({}), Default TDP: {:.2}W ({}), Max TDP: {:.2}W ({})",
                min_tdp,
                min_tdp_percentage,
                default_tdp,
                default_tdp_percentage,
                max_tdp,
                max_tdp_percentage
            );
            print_scan_separator();
        }

        //temp limit readout
        for (_sensor, limit) in info.sensors.iter().zip(
            info.sensor_limits
                .iter()
                .map(Some)
                .chain(iter::repeat(None)),
        ) {
            if let Some(limit) = limit {
                println!("Thermal Limit {} ({} default)", limit.range, limit.default);
                min_temp_lim = limit.range.min;
                max_temp_lim = limit.range.max;
                default_temp_lim = limit.default;
                if let Some(pff) = &limit.throttle_curve {
                    current_pff_curve = pff.clone();
                    println!("Thermal Throttle {}", pff);
                }
            }
        }
    }

    Ok((
        min_tdp_percentage,
        default_tdp_percentage,
        max_tdp_percentage,
        min_temp_lim,
        default_temp_lim,
        max_temp_lim,
        current_pff_curve,
    ))
}

pub fn find_matching_vfp_point(
    vfp_table: &BTreeMap<usize, VfpPoint>,
    sensor_v: Microvolts,
) -> Option<(&usize, &VfpPoint)> {
    vfp_table
        .iter()
        .min_by_key(|(_, point)| (point.voltage.0 as i64 - sensor_v.0 as i64).abs())
    // Find closest voltage
}

pub fn voltage_frequency_check(arg_matches: ArgMatches, point: usize) -> Result<bool, Error> {
    let selector = GpuSelector::from_clap(arg_matches.get_many::<String>("gpu"));
    let gpu_list = crate::basic_func::get_sorted_gpus()?;
    let gpus = select_gpus(&gpu_list, &selector)?;
    let mut precise_flag = false;

    for gpu in gpus {
        let status = gpu.status()?;
        let readout_v = status.voltage.ok_or_else(|| Error::Custom("GPU did not report voltage in status; check if the GPU supports voltage monitoring".into()))?;
        let readout_f = status.clone().clocks;
        print_scan_separator();
        println!("readout volt: {:?}, freq: {:?}", readout_v, readout_f);

        let current_point = status.clone().vfp.ok_or(Error::VfpUnsupported)?.graphics;

        let default_v = current_point
            .get(&(point))
            .ok_or(Error::Str("invalid point index"))?
            .voltage;
        let default_f = current_point
            .get(&(point))
            .ok_or(Error::Str("invalid point index"))?
            .default_frequency;
        let current_f = current_point
            .get(&(point))
            .ok_or(Error::Str("invalid point index"))?
            .frequency;

        println!(
            "Chking Pnt: {} (volt: {}) with default freq: {}, target freq: {}",
            point, default_v, default_f, current_f
        );

        let sensor_v = gpu.inner().core_voltage()?;
        let sensor_f = gpu.inner().clock_frequencies(ClockFrequencyType::Current)?;

        println!("current volt: {}, freq:{:?}", sensor_v, sensor_f);

        if let Some((index, vfp_point)) = find_matching_vfp_point(&current_point, sensor_v) {
            println!(
                "Working VfPoint Inferred:{}, Volt = {:?}, Freq = {:?}",
                index, vfp_point.voltage, vfp_point.frequency
            );
            precise_flag = index.abs_diff(point) < 5;
        } else {
            eprintln!("No matching VfpPoint found");
            precise_flag = false;
        }
        print_scan_separator();
    }
    Ok(precise_flag)
}

// ---------------------------------------------------------------------------
// VFP batch-setting helpers used by the CLI autoscan path.
// ---------------------------------------------------------------------------

/// Set the same frequency delta on a VFP point range.
/// Errors are logged and the scan continues so the caller can recover from GPU resets.
pub fn set_vfp_range_warn(gpu: &&Gpu, range: std::ops::RangeInclusive<usize>, delta_khz: i32) {
    for offset in range {
        match gpu.set_vfp(
            iter::once((offset, KilohertzDelta(delta_khz))),
            iter::empty(),
        ) {
            Ok(_) => {}
            Err(e) => eprintln!(
                "Warning: {}, set_vfp offset={} Error. GPU crashed...",
                e, offset
            ),
        }
    }
}

/// Set the same frequency delta on a VFP point range, propagating errors.
pub fn set_vfp_range(
    gpu: &&Gpu,
    range: std::ops::RangeInclusive<usize>,
    delta_khz: i32,
) -> Result<(), Error> {
    for offset in range {
        gpu.set_vfp(
            iter::once((offset, KilohertzDelta(delta_khz))),
            iter::empty(),
        )?;
    }
    Ok(())
}

/// Set the main VFP scan range, and optionally a lower range for flat-curve mode.
pub fn set_vfp_curve_warn(
    gpu: &&Gpu,
    point: usize,
    vfp_set_range: usize,
    flat_curve_flag: bool,
    main_delta: i32,
    lower_delta: Option<i32>,
) {
    if !flat_curve_flag {
        set_vfp_range_warn(
            gpu,
            (point - vfp_set_range)..=(point + vfp_set_range),
            main_delta,
        );
    } else {
        set_vfp_range_warn(gpu, point..=(point + vfp_set_range), main_delta);
        if let Some(ld) = lower_delta {
            set_vfp_range_warn(gpu, (point - vfp_set_range)..=(point - 1), ld);
        }
    }
}

/// Set the main VFP scan range, and optionally a lower range for flat-curve mode.
pub fn set_vfp_curve(
    gpu: &&Gpu,
    point: usize,
    vfp_set_range: usize,
    flat_curve_flag: bool,
    main_delta: i32,
    lower_delta: Option<i32>,
) -> Result<(), Error> {
    if !flat_curve_flag {
        set_vfp_range(
            gpu,
            (point - vfp_set_range)..=(point + vfp_set_range),
            main_delta,
        )?;
    } else {
        set_vfp_range(gpu, point..=(point + vfp_set_range), main_delta)?;
        if let Some(ld) = lower_delta {
            set_vfp_range(gpu, (point - vfp_set_range)..=(point - 1), ld)?;
        }
    }
    Ok(())
}

pub fn set_legacy_clocks_nvapi(gpu: &Gpu, core_mhz: u32, mem_mhz: u32) -> Result<(), Error> {
    use nvapi_hi::sys::nvapi_QueryInterface;
    use std::mem;

    const NVAPI_GPU_SET_CLOCKS_ID: u32 = 0x6f151055;

    #[repr(C)]
    struct NvClocksInfo {
        version: u32,
        clocks: [u32; 32],
    }

    let version = (size_of::<NvClocksInfo>() as u32) | (2 << 16);
    let mut info = NvClocksInfo {
        version,
        clocks: [0; 32],
    };

    info.clocks[8] = mem_mhz.saturating_mul(1000);
    info.clocks[30] = core_mhz.saturating_mul(2000);

    unsafe {
        let ptr_res = nvapi_QueryInterface(NVAPI_GPU_SET_CLOCKS_ID);
        let ptr = match ptr_res {
            Ok(p) => p as *const (),
            Err(_) => {
                return Err(Error::Custom(format!(
                    "legacy interface not found at ID (0x{:x}), NVIDIA has ended its support",
                    NVAPI_GPU_SET_CLOCKS_ID
                )));
            }
        };

        #[allow(improper_ctypes_definitions)]
        type SetClocksFn = unsafe extern "system" fn(
            h_physical_gpu: nvapi_hi::sys::api::NvPhysicalGpuHandle,
            p_clks: *mut NvClocksInfo,
        ) -> nvapi_hi::sys::Status;

        let func: SetClocksFn = mem::transmute(ptr);
        let status = func(*gpu.inner().handle(), &mut info);

        if status != nvapi_hi::sys::Status::Ok {
            return Err(Error::Custom(format!(
                "Failed to call legacy interface NvAPI_GPU_SetClocks, error code: {:?}",
                status
            )));
        }
    }

    Ok(())
}
