use crate::error::Error;
use nvml_wrapper::Nvml;
use nvml_wrapper::enums::device::FanControlPolicy;

// ---------------------------------------------------------------------------
// Private helper: find an NVML device by NVAPI-style GPU ID (PCI bus * 256)
// ---------------------------------------------------------------------------

fn find_nvml_device<'n>(nvml: &'n Nvml, gpu_id: u32) -> Option<nvml_wrapper::Device<'n>> {
    let pci_bus = gpu_id / 256;
    let count = nvml.device_count().ok()?;
    for i in 0..count {
        if let Ok(dev) = nvml.device_by_index(i)
            && let Ok(pci) = dev.pci_info()
                && pci.bus == pci_bus {
                    return Some(dev);
                }
    }
    None
}

fn find_nvml_device_err<'n>(
    nvml: &'n Nvml,
    gpu_id: u32,
) -> Result<nvml_wrapper::Device<'n>, Error> {
    find_nvml_device(nvml, gpu_id)
        .ok_or_else(|| Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

// ---------------------------------------------------------------------------
// Power queries
// ---------------------------------------------------------------------------

/// 通过 NVML 查询指定 GPU 的功率限制（单位：瓦）。
///
/// **注意：** 优先使用 `query_nvml_power_watts(nvml, gpu_id)` —— 它更简洁且直接。
/// 此函数保留作为备用实现（适用于需要从字符串解析 PCI Bus ID 的场景）。
///
/// # 参数
/// - `pci_bus_id_str`: PCI Bus ID 字符串（格式如 "0000:01:00.0" 或 NVAPI 的 "PCIe x16 (1:0...)"）
///
/// # 返回
/// - `Some((min_W, current_W, max_W))`: 成功时返回三个瓦数值
/// - `None`: NVML 初始化失败或设备不可用时返回
#[allow(dead_code)]
pub fn query_nvml_power_watts_by_pci(pci_bus_id_str: &str) -> Option<(f32, f32, f32)> {
    let nvml = Nvml::init().ok()?;

    // 从 NVAPI 格式提取 PCI Bus 编号
    // 例如 "PCIe x16 (1:0 routed to IRQ 0)" -> Bus = 1
    let nvapi_bus_num = if let Some(start) = pci_bus_id_str.find('(') {
        if let Some(end) = pci_bus_id_str[start..].find(':') {
            pci_bus_id_str[start + 1..start + end]
                .trim()
                .parse::<u32>()
                .ok()
        } else {
            None
        }
    } else {
        // 尝试解析标准格式 "0000:01:00.0"
        pci_bus_id_str
            .split(':')
            .nth(1)
            .and_then(|s| s.parse::<u32>().ok())
    };

    // 尝试直接通过 PCI Bus ID 获取设备（可能失败）
    let device = nvml
        .device_by_pci_bus_id(pci_bus_id_str)
        .or_else(|_| {
            // 降级方案：枚举所有 NVML 设备，匹配 PCI Bus 编号
            let device_count = nvml.device_count()?;

            for i in 0..device_count {
                if let Ok(dev) = nvml.device_by_index(i)
                    && let Ok(pci_info) = dev.pci_info() {
                        // 匹配策略：比较 PCI Bus 编号
                        if let Some(target_bus) = nvapi_bus_num
                            && pci_info.bus == target_bus {
                                return Ok(dev);
                            }

                        // 备用：宽松字符串匹配
                        let nvml_pci_str = format!(
                            "{:04x}:{:02x}:{:02x}.0",
                            pci_info.domain, pci_info.bus, pci_info.device
                        );
                        let nvapi_stripped = pci_bus_id_str.trim_start_matches("0000:");
                        let nvml_stripped = nvml_pci_str.trim_start_matches("0000:");
                        if nvapi_stripped.eq_ignore_ascii_case(nvml_stripped) {
                            return Ok(dev);
                        }
                    }
            }

            Err(nvml_wrapper::error::NvmlError::NotFound)
        })
        .ok()?;

    let current_mw = device.power_management_limit().ok()?;
    let constraints = device.power_management_limit_constraints();

    let (min_mw, max_mw) = match constraints {
        Ok(c) => (c.min_limit, c.max_limit),
        Err(_) => (0, 0),
    };

    let min_w = min_mw as f32 / 1000.0;
    let max_w = max_mw as f32 / 1000.0;
    let current_w = current_mw as f32 / 1000.0;

    Some((min_w, current_w, max_w))
}

/// Query power limits for a GPU via NVML.
///
/// `gpu_id` uses the NVAPI encoding: `PCI_Bus_Number × 256`.
pub fn query_nvml_power_watts(nvml: &Nvml, gpu_id: u32) -> Option<(f32, f32, f32)> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let current_mw = device.power_management_limit().ok()?;
    let constraints = device.power_management_limit_constraints();
    let (min_mw, max_mw) = match constraints {
        Ok(c) => (c.min_limit, c.max_limit),
        Err(_) => (0, 0),
    };
    Some((
        min_mw as f32 / 1000.0,
        current_mw as f32 / 1000.0,
        max_mw as f32 / 1000.0,
    ))
}

pub fn set_nvml_power_limit(nvml: &Nvml, gpu_id: u32, limit_w: u32) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_power_management_limit(limit_w.saturating_mul(1000))
        .map_err(|e| Error::Custom(format!("NVML Set Power Limit Error: {:?}", e)))
}

// ---------------------------------------------------------------------------
// Clock offset get/set
// ---------------------------------------------------------------------------

pub fn get_nvml_core_clock_vf_offset(
    nvml: &Nvml,
    gpu_id: u32,
    pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Option<i32> {
    let device = find_nvml_device(nvml, gpu_id)?;
    device
        .clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Graphics, pstate)
        .ok()
        .map(|o| o.clock_offset_mhz)
}

pub fn set_nvml_core_clock_vf_offset(
    nvml: &Nvml,
    gpu_id: u32,
    offset: i32,
    pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_clock_offset(
            nvml_wrapper::enum_wrappers::device::Clock::Graphics,
            pstate,
            offset,
        )
        .map_err(|e| Error::Custom(format!("NVML Set Core Clock VF Offset Error: {:?}", e)))
}

pub fn get_nvml_mem_clock_vf_offset(
    nvml: &Nvml,
    gpu_id: u32,
    pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Option<i32> {
    let device = find_nvml_device(nvml, gpu_id)?;
    // NVML reports memory clock offset as double the actual frequency (GDDR historical reason).
    device
        .clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Memory, pstate)
        .ok()
        .map(|o| o.clock_offset_mhz / 2)
}

pub fn set_nvml_mem_clock_vf_offset(
    nvml: &Nvml,
    gpu_id: u32,
    offset: i32,
    pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    // NVML expects memory clock offset as double the actual target (GDDR historical reason).
    device
        .set_clock_offset(
            nvml_wrapper::enum_wrappers::device::Clock::Memory,
            pstate,
            offset.saturating_mul(2),
        )
        .map_err(|e| Error::Custom(format!("NVML Set Mem Clock Offset Error: {:?}", e)))
}

// ---------------------------------------------------------------------------
// Temperature thresholds
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn set_nvml_temperature_threshold(
    nvml: &Nvml,
    gpu_id: u32,
    threshold: nvml_wrapper::enum_wrappers::device::TemperatureThreshold,
    limit_c: i32,
) -> Result<(), Error> {
    let device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_temperature_threshold(threshold, limit_c)
        .map_err(|e| Error::Custom(format!("NVML Set Temperature Threshold Error: {:?}", e)))
}

#[allow(dead_code)]
pub fn set_nvml_temperature_limit(nvml: &Nvml, gpu_id: u32, limit_c: i32) -> Result<(), Error> {
    set_nvml_temperature_threshold(
        nvml,
        gpu_id,
        nvml_wrapper::enum_wrappers::device::TemperatureThreshold::GpuMax,
        limit_c,
    )
}

pub fn get_nvml_temperature_thresholds(
    nvml: &Nvml,
    gpu_id: u32,
) -> Option<Vec<(&'static str, Option<u32>)>> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let thresholds = [
        (
            "Shutdown",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::Shutdown,
        ),
        (
            "Slowdown",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::Slowdown,
        ),
        (
            "MemoryMax",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::MemoryMax,
        ),
        (
            "GpuMax",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::GpuMax,
        ),
        (
            "AcousticMin",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::AcousticMin,
        ),
        (
            "AcousticCurr",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::AcousticCurr,
        ),
        (
            "AcousticMax",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::AcousticMax,
        ),
        (
            "GpsCurr",
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::GpsCurr,
        ),
    ];
    Some(
        thresholds
            .iter()
            .map(|(name, threshold)| (*name, device.temperature_threshold(*threshold).ok()))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// P-State info and clock ranges
// ---------------------------------------------------------------------------

pub fn get_nvml_pstate_info(
    nvml: &Nvml,
    gpu_id: u32,
) -> Option<
    Vec<(
        nvml_wrapper::enum_wrappers::device::PerformanceState,
        u32,
        u32,
        u32,
        u32,
    )>,
> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let pstates = device.supported_performance_states().ok()?;
    let mut res = Vec::new();
    for p in pstates {
        let core_clock = device
            .min_max_clock_of_pstate(nvml_wrapper::enum_wrappers::device::Clock::Graphics, p)
            .unwrap_or((0, 0));
        let mem_clock = device
            .min_max_clock_of_pstate(nvml_wrapper::enum_wrappers::device::Clock::Memory, p)
            .unwrap_or((0, 0));
        res.push((p, core_clock.0, core_clock.1, mem_clock.0, mem_clock.1));
    }
    Some(res)
}

/// Returns supported memory clocks and, for each, the supported graphics clocks.
pub fn get_nvml_supported_applications_clocks(
    nvml: &Nvml,
    gpu_id: u32,
) -> Option<Vec<(u32, Vec<u32>)>> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let mut supported = Vec::new();
    if let Ok(mem_clocks) = device.supported_memory_clocks() {
        for mc in mem_clocks {
            if let Ok(gfx_clocks) = device.supported_graphics_clocks(mc) {
                supported.push((mc, gfx_clocks));
            } else {
                supported.push((mc, vec![]));
            }
        }
    }
    Some(supported)
}

// ---------------------------------------------------------------------------
// Fan speed queries
// ---------------------------------------------------------------------------

pub fn get_nvml_min_max_fan_speed(nvml: &Nvml, gpu_id: u32) -> Option<(u32, u32)> {
    let device = find_nvml_device(nvml, gpu_id)?;
    device.min_max_fan_speed().ok()
}

pub fn get_nvml_num_fans(nvml: &Nvml, gpu_id: u32) -> Option<u32> {
    let device = find_nvml_device(nvml, gpu_id)?;
    device.num_fans().ok()
}

// ---------------------------------------------------------------------------
// Fan control
// ---------------------------------------------------------------------------

pub fn parse_nvml_fan_control_policy(policy_raw: &str) -> Result<FanControlPolicy, Error> {
    match policy_raw.to_ascii_lowercase().as_str() {
        "continuous" | "auto" => Ok(FanControlPolicy::TemperatureContinousSw),
        "manual" => Ok(FanControlPolicy::Manual),
        _ => Err(Error::Custom(format!(
            "Invalid NVML fan policy '{}'. Expected continuous/manual/auto",
            policy_raw
        ))),
    }
}

pub fn set_fan_speed(
    nvml: &Nvml,
    gpu_id: u32,
    fan_idx: u32,
    policy: FanControlPolicy,
    level: u32,
) -> Result<(), Error> {
    if level > 100 {
        return Err(Error::Custom(format!(
            "Invalid fan level {}: expected 0..100",
            level
        )));
    }
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_fan_control_policy(fan_idx, policy)
        .map_err(|e| Error::Custom(format!("NVML Set Fan Control Policy Error: {:?}", e)))?;
    device
        .set_fan_speed(fan_idx, level)
        .map_err(|e| Error::Custom(format!("NVML Set Fan Speed Error: {:?}", e)))
}

pub fn set_default_fan_speed(nvml: &Nvml, gpu_id: u32, fan_idx: u32) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_default_fan_speed(fan_idx)
        .map_err(|e| Error::Custom(format!("NVML Set Default Fan Speed Error: {:?}", e)))
}

// ---------------------------------------------------------------------------
// P-State lock (via memory clock window)
// ---------------------------------------------------------------------------

const NVML_PSTATE_LOCK_MARGIN_MHZ: u32 = 50;

fn nvml_ranges_overlap(a_min: u32, a_max: u32, b_min: u32, b_max: u32) -> bool {
    a_min <= b_max && b_min <= a_max
}

pub fn set_nvml_pstate_lock(
    nvml: &Nvml,
    gpu_id: u32,
    first_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
    second_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Result<(String, u32, u32), Error> {
    let pstates = get_nvml_pstate_info(nvml, gpu_id).ok_or_else(|| {
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
    let min_lock_mhz = min_target_mem_clock_mhz.saturating_sub(NVML_PSTATE_LOCK_MARGIN_MHZ);
    let max_lock_mhz = max_target_mem_clock_mhz.saturating_add(NVML_PSTATE_LOCK_MARGIN_MHZ);

    let overlapping_pstates = pstates
        .iter()
        .filter(|(_, _, _, min_mem_mhz, max_mem_mhz)| {
            nvml_ranges_overlap(*min_mem_mhz, *max_mem_mhz, min_lock_mhz, max_lock_mhz)
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
            "{} would map to memory lock window {}-{} MHz, but that also overlaps NVML P-States outside the requested range: {}. Use --nvml-locked-mem-clocks for a manual range instead.",
            range_label,
            min_lock_mhz,
            max_lock_mhz,
            outside_requested_range.join(", "),
        )));
    }

    set_nvml_mem_locked_clocks(nvml, gpu_id, min_lock_mhz, max_lock_mhz)?;
    Ok((range_label, min_lock_mhz, max_lock_mhz))
}

// ---------------------------------------------------------------------------
// Application clocks and locked clocks
// ---------------------------------------------------------------------------

pub fn set_nvml_applications_clocks(
    nvml: &Nvml,
    gpu_id: u32,
    mem_clock_mhz: u32,
    graphics_clock_mhz: u32,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_applications_clocks(mem_clock_mhz, graphics_clock_mhz)
        .map_err(|e| Error::Custom(format!("NVML Set Applications Clocks Error: {:?}", e)))
}

pub fn reset_nvml_applications_clocks(nvml: &Nvml, gpu_id: u32) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .reset_applications_clocks()
        .map_err(|e| Error::Custom(format!("NVML Reset Applications Clocks Error: {:?}", e)))
}

pub fn set_nvml_core_locked_clocks(
    nvml: &Nvml,
    gpu_id: u32,
    min_clock_mhz: u32,
    max_clock_mhz: u32,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_gpu_locked_clocks(
            nvml_wrapper::enums::device::GpuLockedClocksSetting::Numeric {
                min_clock_mhz,
                max_clock_mhz,
            },
        )
        .map_err(|e| Error::Custom(format!("NVML Set GPU Locked Clocks Error: {:?}", e)))
}

pub fn reset_nvml_core_locked_clocks(nvml: &Nvml, gpu_id: u32) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .reset_gpu_locked_clocks()
        .map_err(|e| Error::Custom(format!("NVML Reset GPU Locked Clocks Error: {:?}", e)))
}

pub fn set_nvml_mem_locked_clocks(
    nvml: &Nvml,
    gpu_id: u32,
    min_clock_mhz: u32,
    max_clock_mhz: u32,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_mem_locked_clocks(min_clock_mhz, max_clock_mhz)
        .map_err(|e| Error::Custom(format!("NVML Set Memory Locked Clocks Error: {:?}", e)))
}

pub fn reset_nvml_mem_locked_clocks(nvml: &Nvml, gpu_id: u32) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .reset_mem_locked_clocks()
        .map_err(|e| Error::Custom(format!("NVML Reset Memory Locked Clocks Error: {:?}", e)))
}
