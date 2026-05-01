use crate::error::Error;
use nvml_wrapper::Nvml;
use nvml_wrapper::enums::device::FanControlPolicy;

// ---------------------------------------------------------------------------
// NVML 功率查询辅助函数（供 human.rs 和 get_gpu_tdp_temp_limit 复用）
// ---------------------------------------------------------------------------

/// 通过 NVML 查询指定 GPU 的功率限制（单位：瓦）。
///
/// **注意：** 优先使用 `query_nvml_power_watts(gpu_id)` —— 它更简洁且直接。
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
            pci_bus_id_str[start+1..start+end].trim().parse::<u32>().ok()
        } else {
            None
        }
    } else {
        // 尝试解析标准格式 "0000:01:00.0"
        pci_bus_id_str.split(':').nth(1).and_then(|s| s.parse::<u32>().ok())
    };

    // 尝试直接通过 PCI Bus ID 获取设备（可能失败）
    let device = nvml.device_by_pci_bus_id(pci_bus_id_str).or_else(|_| {
        // 降级方案：枚举所有 NVML 设备，匹配 PCI Bus 编号
        let device_count = nvml.device_count()?;

        for i in 0..device_count {
            if let Ok(dev) = nvml.device_by_index(i) {
                if let Ok(pci_info) = dev.pci_info() {
                    // 匹配策略：比较 PCI Bus 编号
                    if let Some(target_bus) = nvapi_bus_num {
                        if pci_info.bus == target_bus {
                            return Ok(dev);
                        }
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
        }

        Err(nvml_wrapper::error::NvmlError::NotFound)
    }).ok()?;

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

/// 通过 NVAPI GPU ID 查询功率限制（直接计算 PCI Bus 号）。
///
/// # NVAPI GPU ID 编码规则
///
/// NVAPI 使用以下公式编码 GPU ID：
/// ```text
/// GPU_ID = PCI_Bus_Number × 256
/// ```
///
/// **示例：**
/// - GPU ID `256` (0x0100) → PCI Bus `0x01` (Bus 1)
/// - GPU ID `35072` (0x8900) → PCI Bus `0x89` (Bus 137)
///
/// **逆向公式：**
/// ```text
/// PCI_Bus_Number = GPU_ID ÷ 256
/// ```
///
/// 这个函数通过上述公式从 GPU ID 提取 PCI Bus 号，然后枚举 NVML 设备进行匹配。
///
/// # 参数
/// - `gpu_id`: NVAPI GPU ID（从 `GpuInfo.id` 获取）
///
/// # 返回
/// - `Some((min_W, current_W, max_W))`: 成功时返回三个瓦数值
/// - `None`: NVML 初始化失败或设备不可用时返回
pub fn query_nvml_power_watts(gpu_id: u32) -> Option<(f32, f32, f32)> {
    let nvml = Nvml::init().ok()?;

    // NVAPI GPU ID 编码规则：GPU_ID = PCI_Bus × 256
    let pci_bus_num = gpu_id / 256;

    // 枚举所有 NVML 设备，匹配 PCI Bus 编号
    let device_count = nvml.device_count().ok()?;
    let mut device = None;

    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    device = Some(dev);
                    break;
                }
            }
        }
    }

    let device = device?;

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

pub fn get_nvml_core_clock_vf_offset(gpu_id: u32, pstate: nvml_wrapper::enum_wrappers::device::PerformanceState) -> Option<i32> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Graphics, pstate).ok().map(|o| o.clock_offset_mhz);
                }
            }
        }
    }
    None
}

pub fn set_nvml_core_clock_vf_offset(gpu_id: u32, offset: i32, pstate: nvml_wrapper::enum_wrappers::device::PerformanceState) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.set_clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Graphics, pstate, offset)
                        .map_err(|e| Error::Custom(format!("NVML Set Core Clock VF Offset Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

pub fn get_nvml_mem_clock_vf_offset(gpu_id: u32, pstate: nvml_wrapper::enum_wrappers::device::PerformanceState) -> Option<i32> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    // Note: NVML reports memory clock offset as double the actual frequency (GDDR historical reason).
                    // We divide by 2 here so the getter returns the actual effective memory offset.
                    return dev.clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Memory, pstate).ok().map(|o| o.clock_offset_mhz / 2);
                }
            }
        }
    }
    None
}

pub fn set_nvml_mem_clock_vf_offset(gpu_id: u32, offset: i32, pstate: nvml_wrapper::enum_wrappers::device::PerformanceState) -> Result<(), Error> {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(e) => return Err(Error::Custom(format!("NVML Init Error: {:?}", e))),
    };
    let pci_bus_num = gpu_id / 256;
    let device_count = match nvml.device_count() {
        Ok(c) => c,
        Err(e) => return Err(Error::Custom(format!("NVML Device Count Error: {:?}", e))),
    };
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    // NVML expects memory clock offset as double the actual target (GDDR historical reason).
                    match dev.set_clock_offset(nvml_wrapper::enum_wrappers::device::Clock::Memory, pstate, (offset * 2) as i32) {
                        Ok(_) => return Ok(()),
                        Err(e) => return Err(Error::Custom(format!("NVML Set Mem Clock Offset Error: {:?}", e))),
                    }
                }
            }
        }
    }
    Err(Error::Custom("NVML Device not found".to_string()))
}


pub fn set_nvml_power_limit(gpu_id: u32, limit_w: u32) -> Result<(), Error> {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(e) => return Err(Error::Custom(format!("NVML Init Error: {:?}", e))),
    };
    let pci_bus_num = gpu_id / 256;
    let device_count = match nvml.device_count() {
        Ok(c) => c,
        Err(e) => return Err(Error::Custom(format!("NVML Device Count Error: {:?}", e))),
    };
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    let limit_mw = limit_w * 1000;
                    match dev.set_power_management_limit(limit_mw) {
                        Ok(_) => return Ok(()),
                        Err(e) => return Err(Error::Custom(format!("NVML Set Power Limit Error: {:?}", e))),
                    }
                }
            }
        }
    }
    Err(Error::Custom("NVML Device not found".to_string()))
}

#[allow(dead_code)]
pub fn set_nvml_temperature_threshold(
    gpu_id: u32,
    threshold: nvml_wrapper::enum_wrappers::device::TemperatureThreshold,
    limit_c: i32,
) -> Result<(), Error> {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(e) => return Err(Error::Custom(format!("NVML Init Error: {:?}", e))),
    };
    let pci_bus_num = gpu_id / 256;
    let device_count = match nvml.device_count() {
        Ok(c) => c,
        Err(e) => return Err(Error::Custom(format!("NVML Device Count Error: {:?}", e))),
    };
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev
                        .set_temperature_threshold(threshold, limit_c)
                        .map_err(|e| Error::Custom(format!("NVML Set Temperature Threshold Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom("NVML Device not found".to_string()))
}

#[allow(dead_code)]
pub fn set_nvml_temperature_limit(gpu_id: u32, limit_c: i32) -> Result<(), Error> {
    set_nvml_temperature_threshold(
        gpu_id,
        nvml_wrapper::enum_wrappers::device::TemperatureThreshold::GpuMax,
        limit_c,
    )
}

pub fn get_nvml_temperature_thresholds(gpu_id: u32) -> Option<Vec<(&'static str, Option<u32>)>> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
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
                    return Some(
                        thresholds
                            .iter()
                            .map(|(name, threshold)| (*name, dev.temperature_threshold(*threshold).ok()))
                            .collect(),
                    );
                }
            }
        }
    }
    None
}

pub fn get_nvml_pstate_info(gpu_id: u32) -> Option<Vec<(nvml_wrapper::enum_wrappers::device::PerformanceState, u32, u32, u32, u32)>> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    if let Ok(pstates) = dev.supported_performance_states() {
                        let mut res = Vec::new();
                        for p in pstates {
                            let core_clock = dev.min_max_clock_of_pstate(nvml_wrapper::enum_wrappers::device::Clock::Graphics, p).unwrap_or((0, 0));
                            let mem_clock = dev.min_max_clock_of_pstate(nvml_wrapper::enum_wrappers::device::Clock::Memory, p).unwrap_or((0, 0));
                            res.push((p, core_clock.0, core_clock.1, mem_clock.0, mem_clock.1));
                        }
                        return Some(res);
                    }
                }
            }
        }
    }
    None
}

/// 获取支持的 memory clocks 以及每个 memory clock 对应的 graphics clocks 列表
pub fn get_nvml_supported_applications_clocks(gpu_id: u32) -> Option<Vec<(u32, Vec<u32>)>> {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(_) => return None,
    };
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    let mut supported = Vec::new();
                    if let Ok(mem_clocks) = dev.supported_memory_clocks() {
                        for mc in mem_clocks {
                            if let Ok(gfx_clocks) = dev.supported_graphics_clocks(mc) {
                                supported.push((mc, gfx_clocks));
                            } else {
                                supported.push((mc, vec![]));
                            }
                        }
                    }
                    return Some(supported);
                }
            }
        }
    }
    None
}

pub fn get_nvml_min_max_fan_speed(gpu_id: u32) -> Option<(u32, u32)> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.min_max_fan_speed().ok();
                }
            }
        }
    }
    None
}

pub fn get_nvml_num_fans(gpu_id: u32) -> Option<u32> {
    let nvml = Nvml::init().ok()?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().ok()?;
    for i in 0..device_count {
        if let Ok(dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.num_fans().ok();
                }
            }
        }
    }
    None
}

pub fn parse_nvml_fan_control_policy(
    policy_raw: &str,
) -> Result<FanControlPolicy, Error> {
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

    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count =
        nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;

    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    dev.set_fan_control_policy(fan_idx, policy).map_err(|e| {
                        Error::Custom(format!("NVML Set Fan Control Policy Error: {:?}", e))
                    })?;
                    return dev
                        .set_fan_speed(fan_idx, level)
                        .map_err(|e| Error::Custom(format!("NVML Set Fan Speed Error: {:?}", e)));
                }
            }
        }
    }

    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

pub fn set_default_fan_speed(gpu_id: u32, fan_idx: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count =
        nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;

    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.set_default_fan_speed(fan_idx).map_err(|e| {
                        Error::Custom(format!("NVML Set Default Fan Speed Error: {:?}", e))
                    });
                }
            }
        }
    }

    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

const NVML_PSTATE_LOCK_MARGIN_MHZ: u32 = 50;

fn nvml_ranges_overlap(a_min: u32, a_max: u32, b_min: u32, b_max: u32) -> bool {
    a_min <= b_max && b_min <= a_max
}

pub fn set_nvml_pstate_lock(
    gpu_id: u32,
    first_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
    second_pstate: nvml_wrapper::enum_wrappers::device::PerformanceState,
) -> Result<(String, u32, u32), Error> {
    let pstates = get_nvml_pstate_info(gpu_id).ok_or_else(|| {
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

    set_nvml_mem_locked_clocks(gpu_id, min_lock_mhz, max_lock_mhz)?;
    Ok((range_label, min_lock_mhz, max_lock_mhz))
}

/// 设定 GPU 在运行应用程序时锁定在指定的显存与核心频率，避免波动。
pub fn set_nvml_applications_clocks(gpu_id: u32, mem_clock_mhz: u32, graphics_clock_mhz: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.set_applications_clocks(mem_clock_mhz, graphics_clock_mhz)
                        .map_err(|e| Error::Custom(format!("NVML Set Applications Clocks Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

/// 锁定GPU核心频率在指定的最小值和最大值之间。
pub fn set_nvml_core_locked_clocks(gpu_id: u32, min_clock_mhz: u32, max_clock_mhz: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.set_gpu_locked_clocks(nvml_wrapper::enums::device::GpuLockedClocksSetting::Numeric { min_clock_mhz, max_clock_mhz })
                        .map_err(|e| Error::Custom(format!("NVML Set GPU Locked Clocks Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

/// 解除GPU核心频率锁定。
pub fn reset_nvml_core_locked_clocks(gpu_id: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.reset_gpu_locked_clocks()
                        .map_err(|e| Error::Custom(format!("NVML Reset GPU Locked Clocks Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

/// 锁定GPU显存频率在指定的最小值和最大值之间。
pub fn set_nvml_mem_locked_clocks(gpu_id: u32, min_clock_mhz: u32, max_clock_mhz: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.set_mem_locked_clocks(min_clock_mhz, max_clock_mhz)
                        .map_err(|e| Error::Custom(format!("NVML Set Memory Locked Clocks Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

/// 解除GPU显存频率锁定。
pub fn reset_nvml_mem_locked_clocks(gpu_id: u32) -> Result<(), Error> {
    let nvml = Nvml::init().map_err(|e| Error::Custom(format!("NVML Init Error: {:?}", e)))?;
    let pci_bus_num = gpu_id / 256;
    let device_count = nvml.device_count().map_err(|e| Error::Custom(format!("NVML Device Count Error: {:?}", e)))?;
    for i in 0..device_count {
        if let Ok(mut dev) = nvml.device_by_index(i) {
            if let Ok(pci_info) = dev.pci_info() {
                if pci_info.bus == pci_bus_num {
                    return dev.reset_mem_locked_clocks()
                        .map_err(|e| Error::Custom(format!("NVML Reset Memory Locked Clocks Error: {:?}", e)));
                }
            }
        }
    }
    Err(Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}
