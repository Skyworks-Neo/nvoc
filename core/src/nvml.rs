use super::conv::{nvml_pstate_to_index, nvml_pstate_to_str};
use super::error::Error;
use super::target::GpuId;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Api, PerformanceState};
use nvml_wrapper::enums::device::FanControlPolicy;

pub type NvmlPStateClockRange = (PerformanceState, u32, u32, u32, u32);

// ---------------------------------------------------------------------------
// Private helper: find an NVML device by NVAPI-style GPU ID (PCI bus * 256)
// ---------------------------------------------------------------------------

fn find_nvml_device(nvml: &'_ Nvml, gpu_id: u32) -> Option<nvml_wrapper::Device<'_>> {
    let pci_bus = GpuId(gpu_id).pci_bus();
    let count = nvml.device_count().ok()?;
    for i in 0..count {
        if let Ok(dev) = nvml.device_by_index(i)
            && let Ok(pci) = dev.pci_info()
            && pci.bus == pci_bus
        {
            return Some(dev);
        }
    }
    None
}

fn find_nvml_device_err(nvml: &'_ Nvml, gpu_id: u32) -> Result<nvml_wrapper::Device<'_>, Error> {
    find_nvml_device(nvml, gpu_id)
        .ok_or_else(|| Error::Custom(format!("GPU {} not found in NVML", gpu_id)))
}

// ---------------------------------------------------------------------------
// UUID query
// ---------------------------------------------------------------------------

/// Query GPU UUID via NVML.
///
/// `gpu_id` uses the NVAPI encoding: `PCI_Bus_Number × 256`.
pub fn query_nvml_uuid(nvml: &Nvml, gpu_id: u32) -> Option<String> {
    let device = find_nvml_device(nvml, gpu_id)?;
    device.uuid().ok()
}

// ---------------------------------------------------------------------------
// Power queries
// ---------------------------------------------------------------------------

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

/// Query the live (instantaneous) GPU power draw in watts via NVML.
///
/// Wraps `nvmlDeviceGetPowerUsage` (`Device::power_usage`), which reports the
/// current board power draw in milliwatts — the same reading `nvidia-smi`
/// shows. This is the only reliable live-draw source on laptop GPUs: the
/// NVAPI power-topology path (`NvAPI_GPU_ClientPowerTopologyGetStatus`) returns
/// a dimensionless percentage and is typically empty on mobile parts, while
/// [`query_nvml_power_watts`] reads the configurable power *limit*, not the
/// actual draw (and laptops usually have no user-configurable limit, so it
/// returns `None`).
///
/// `gpu_id` uses the NVAPI encoding: `PCI_Bus_Number × 256`.
pub fn query_nvml_power_draw_watts(nvml: &Nvml, gpu_id: u32) -> Option<f32> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let mw = device.power_usage().ok()?;
    Some(mw as f32 / 1000.0)
}

pub fn set_nvml_auto_boost(nvml: &Nvml, gpu_id: u32, enabled: bool) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_auto_boosted_clocks(enabled)
        .map_err(|e| Error::Custom(format!("NVML Set Auto Boost Error: {:?}", e)))
}

pub fn query_nvml_auto_boost(nvml: &Nvml, gpu_id: u32) -> Result<(bool, bool), Error> {
    let device = find_nvml_device_err(nvml, gpu_id)?;
    let info = device
        .auto_boosted_clocks_enabled()
        .map_err(|e| Error::Custom(format!("NVML Query Auto Boost Error: {:?}", e)))?;
    Ok((info.is_enabled, info.is_enabled_default))
}

pub fn set_nvml_auto_boost_default(nvml: &Nvml, gpu_id: u32, enabled: bool) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_auto_boosted_clocks_default(enabled)
        .map_err(|e| Error::Custom(format!("NVML Set Auto Boost Default Error: {:?}", e)))
}

pub fn set_nvml_api_restriction(
    nvml: &Nvml,
    gpu_id: u32,
    api_type: Api,
    restricted: bool,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .set_api_restricted(api_type, restricted)
        .map_err(|e| Error::Custom(format!("NVML Set API Restriction Error: {:?}", e)))
}

pub fn query_nvml_api_restriction(nvml: &Nvml, gpu_id: u32, api_type: Api) -> Result<bool, Error> {
    let device = find_nvml_device_err(nvml, gpu_id)?;
    device
        .is_api_restricted(api_type)
        .map_err(|e| Error::Custom(format!("NVML Query API Restriction Error: {:?}", e)))
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

/// Scale factor for NVML memory clock offsets (GDDR historical reason:
/// NVML reports/expects double the actual frequency).
const MEM_CLOCK_OFFSET_SCALE: i32 = 2;

/// Map a NVML `Clock` domain to the offset scale factor.
fn clock_offset_scale(clock: nvml_wrapper::enum_wrappers::device::Clock) -> i32 {
    match clock {
        nvml_wrapper::enum_wrappers::device::Clock::Memory => MEM_CLOCK_OFFSET_SCALE,
        _ => 1,
    }
}

pub fn get_nvml_clock_offset(
    nvml: &Nvml,
    gpu_id: u32,
    clock: nvml_wrapper::enum_wrappers::device::Clock,
    pstate: PerformanceState,
) -> Option<i32> {
    let device = find_nvml_device(nvml, gpu_id)?;
    let scale = clock_offset_scale(clock);
    device
        .clock_offset(clock, pstate)
        .ok()
        .map(|o| o.clock_offset_mhz / scale)
}

pub fn set_nvml_clock_offset(
    nvml: &Nvml,
    gpu_id: u32,
    clock: nvml_wrapper::enum_wrappers::device::Clock,
    pstate: PerformanceState,
    offset: i32,
) -> Result<(), Error> {
    let mut device = find_nvml_device_err(nvml, gpu_id)?;
    let scale = clock_offset_scale(clock);
    device
        .set_clock_offset(clock, pstate, offset.saturating_mul(scale))
        .map_err(|e| Error::Custom(format!("NVML Set Clock Offset Error: {:?}", e)))
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

#[allow(clippy::type_complexity)]
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

pub fn get_nvml_throttle_reasons(nvml: &Nvml, gpu_id: u32) -> Option<Vec<(&'static str, bool)>> {
    use nvml_wrapper::bitmasks::device::ThrottleReasons;
    let device = find_nvml_device(nvml, gpu_id)?;
    let reasons = device.current_throttle_reasons().ok()?;
    let names: &[(&str, ThrottleReasons)] = &[
        ("GPU Idle", ThrottleReasons::GPU_IDLE),
        (
            "App Clock Setting",
            ThrottleReasons::APPLICATIONS_CLOCKS_SETTING,
        ),
        ("SW Power Cap", ThrottleReasons::SW_POWER_CAP),
        ("HW Slowdown", ThrottleReasons::HW_SLOWDOWN),
        ("Sync Boost", ThrottleReasons::SYNC_BOOST),
        ("SW Thermal", ThrottleReasons::SW_THERMAL_SLOWDOWN),
        ("HW Thermal", ThrottleReasons::HW_THERMAL_SLOWDOWN),
        ("HW Power Brake", ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN),
        ("Display Clock", ThrottleReasons::DISPLAY_CLOCK_SETTING),
    ];
    Some(
        names
            .iter()
            .map(|(name, flag)| (*name, reasons.contains(*flag)))
            .collect(),
    )
}

pub struct ViolationStatus {
    pub violation_time_ns: u64,
    pub reference_time_us: u64,
}

pub fn get_nvml_violation_status(
    nvml: &Nvml,
    gpu_id: u32,
) -> Option<Vec<(&'static str, ViolationStatus)>> {
    use nvml_wrapper::enum_wrappers::device::PerformancePolicy;
    let device = find_nvml_device(nvml, gpu_id)?;
    let policies: &[(&str, PerformancePolicy)] = &[
        ("Pwr", PerformancePolicy::Power),
        ("Thrm", PerformancePolicy::Thermal),
        ("Syn-Boost", PerformancePolicy::SyncBoost),
        ("Brd-Lim", PerformancePolicy::BoardLimit),
        ("Idle", PerformancePolicy::LowUtilization),
        ("Rel", PerformancePolicy::Reliability),
        ("AppClk", PerformancePolicy::TotalAppClocks),
        ("BaseClk", PerformancePolicy::TotalBaseClocks),
    ];
    let mut results = Vec::new();
    for (name, policy) in policies {
        let status = device
            .violation_status(*policy)
            .ok()
            .map(|v| ViolationStatus {
                violation_time_ns: v.violation_time,
                reference_time_us: v.reference_time,
            })
            .unwrap_or(ViolationStatus {
                violation_time_ns: 0,
                reference_time_us: 0,
            });
        results.push((*name, status));
    }
    Some(results)
}

// ---------------------------------------------------------------------------
// P-State info and clock ranges
// ---------------------------------------------------------------------------

pub fn get_nvml_pstate_info(nvml: &Nvml, gpu_id: u32) -> Option<Vec<NvmlPStateClockRange>> {
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
#[allow(clippy::type_complexity)]
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

pub(crate) fn build_supported_pstate_list(pstates: &[NvmlPStateClockRange]) -> String {
    pstates
        .iter()
        .map(|(reported_pstate, _, _, _, _)| {
            nvml_pstate_to_str(*reported_pstate)
                .trim_start_matches('P')
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn find_pstate_entry<'a>(
    pstates: &'a [NvmlPStateClockRange],
    target_pstate: PerformanceState,
    gpu_id: u32,
    supported_list: &str,
) -> Result<&'a NvmlPStateClockRange, Error> {
    pstates
        .iter()
        .find(|(reported_pstate, _, _, _, _)| *reported_pstate == target_pstate)
        .ok_or_else(|| {
            Error::Custom(format!(
                "{} is not reported by NVML for GPU {}. Supported NVML P-States: {}",
                nvml_pstate_to_str(target_pstate),
                gpu_id,
                supported_list
            ))
        })
}

pub(crate) fn collect_outside_requested_range(
    overlapping_pstates: &[(Result<u8, Error>, &'static str)],
    min_index: u8,
    max_index: u8,
) -> Vec<&'static str> {
    overlapping_pstates
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
        .collect()
}

// ---------------------------------------------------------------------------
// P-State lock (via memory clock window)
// ---------------------------------------------------------------------------

pub fn set_nvml_pstate_lock(
    nvml: &Nvml,
    gpu_id: u32,
    first_pstate: PerformanceState,
    second_pstate: PerformanceState,
) -> Result<(String, u32, u32), Error> {
    let pstates = get_nvml_pstate_info(nvml, gpu_id).ok_or_else(|| {
        Error::Custom(format!(
            "Failed to query NVML P-State information for GPU {}",
            gpu_id
        ))
    })?;

    let first_index = nvml_pstate_to_index(first_pstate)?;
    let second_index = nvml_pstate_to_index(second_pstate)?;
    let (high_perf_pstate, low_perf_pstate, min_index, max_index) = if first_index <= second_index {
        (first_pstate, second_pstate, first_index, second_index)
    } else {
        (second_pstate, first_pstate, second_index, first_index)
    };
    let range_label = if min_index == max_index {
        nvml_pstate_to_str(high_perf_pstate).to_string()
    } else {
        format!(
            "{}-{}",
            nvml_pstate_to_str(high_perf_pstate),
            nvml_pstate_to_str(low_perf_pstate)
        )
    };
    let supported_list = build_supported_pstate_list(&pstates);
    let high_perf_entry = find_pstate_entry(&pstates, high_perf_pstate, gpu_id, &supported_list)?;
    let low_perf_entry = find_pstate_entry(&pstates, low_perf_pstate, gpu_id, &supported_list)?;

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
                nvml_pstate_to_index(*reported_pstate),
                nvml_pstate_to_str(*reported_pstate),
            )
        })
        .collect::<Vec<_>>();

    let outside_requested_range =
        collect_outside_requested_range(&overlapping_pstates, min_index, max_index);

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
