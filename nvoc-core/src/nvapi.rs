use super::conv::{nvml_pstate_to_index, nvml_pstate_to_str};
use super::error::Error;
use super::gpu_type::{GpuType, fetch_gpu_type};
use super::nvml::get_nvml_pstate_info;
use super::types::{NvapiLockedVoltageTarget, VfpResetDomain};
use nvapi_hi::nvapi::{CelsiusShifted, VoltageDomain};
use nvapi_hi::{
    Celsius, ClockDomain, ClockLockEntry, ClockLockValue, CoolerPolicy, CoolerSettings,
    FanCoolerId, Gpu, Kilohertz, KilohertzDelta, Microvolts, MicrovoltsDelta, PState, Percentage,
    PerfLimitId, PffCurve, PffPoint, VfpPoint,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::iter;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

pub type GpuTdpTempLimits = (
    Percentage,
    Percentage,
    Percentage,
    Celsius,
    Celsius,
    Celsius,
    PffCurve,
);

#[derive(Clone, Copy, Debug)]
pub enum CoolerTarget {
    All,
    Cooler1,
    Cooler2,
}

#[derive(Clone, Copy, Debug)]
pub enum VfpLockRequest {
    VoltagePoint(usize),
    Voltage(Microvolts),
    Frequency {
        domain: ClockDomain,
        upper: Kilohertz,
        lower: Option<Kilohertz>,
    },
}

/// 通过 NvAPI_GPU_SetPstates20 的 baseVoltages 字段写入指定 pstate 的核心电压 delta。
/// 适用于 900 系（Maxwell）及更早不支持 ClientVoltRailsSetControl 的 GPU。
/// delta_uv 单位为 μV，正值加压，负值降压，范围由 GPU 自身 voltDelta_uV.{min,max} 决定。
/// target_pstate 指定目标 pstate，默认应传入 PState::P0。
#[allow(clippy::field_reassign_with_default)] // struct literal form fails: NV_GPU_PERF_PSTATES20_INFO V1/V2 type alias mismatch
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
        .find(|v| v.voltage_domain == VoltageDomain::Core)
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
        ve.domainId = VoltageDomain::Core.raw();
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

    Ok(())
}

pub fn legacy_p0_core_max_voltage_delta(gpu: &Gpu) -> Result<Option<MicrovoltsDelta>, Error> {
    let pstates = gpu
        .inner()
        .pstates()
        .map_err(|e| Error::from(format!("Failed to read pstates: {:?}", e)))?;

    Ok(pstates
        .pstates
        .into_iter()
        .find(|p| p.id == PState::P0)
        .and_then(|p0| {
            p0.base_voltages
                .into_iter()
                .find(|v| v.voltage_domain == VoltageDomain::Core)
                .map(|v| v.voltage_delta.range.max)
        }))
}

pub fn legacy_core_overvolt_ranges(
    gpu: &Gpu,
) -> Result<Vec<(PState, MicrovoltsDelta, MicrovoltsDelta, MicrovoltsDelta)>, Error> {
    let pstates = gpu.inner().pstates().map_err(Error::from)?;
    Ok(pstates
        .pstates
        .iter()
        .filter_map(|ps| {
            ps.base_voltages
                .iter()
                .find(|v| v.voltage_domain == VoltageDomain::Core && v.editable)
                .map(|v| {
                    (
                        ps.id,
                        v.voltage_delta.value,
                        v.voltage_delta.range.min,
                        v.voltage_delta.range.max,
                    )
                })
        })
        .collect())
}

/// Preserve all editable clock deltas in the target P-State, overriding only one domain.
/// This avoids `NvAPI_GPU_SetPstates20` clearing sibling domains when the caller wants
/// to change only Graphics or Memory.
pub fn set_pstate_clock_offset_preserve(
    gpu: &Gpu,
    target_pstate: PState,
    target_domain: ClockDomain,
    target_delta: KilohertzDelta,
) -> Result<(), Error> {
    let graphics_vfp = if target_domain == ClockDomain::Memory {
        capture_graphics_vfp(gpu)?
    } else {
        None
    };

    let pstates = gpu
        .inner()
        .pstates()
        .map_err(|e| Error::from(format!("Failed to read pstates: {:?}", e)))?;

    let target_ps = pstates
        .pstates
        .iter()
        .find(|p| p.id == target_pstate)
        .ok_or_else(|| Error::from(format!("{:?} pstate not found", target_pstate)))?;

    let mut entries: Vec<(PState, ClockDomain, KilohertzDelta)> = target_ps
        .clocks
        .iter()
        .filter(|clock| clock.editable())
        .map(|clock| {
            let delta = if clock.domain() == target_domain {
                target_delta
            } else {
                clock.frequency_delta().value
            };
            (target_pstate, clock.domain(), delta)
        })
        .collect();

    if entries.is_empty() {
        return Err(Error::from(format!(
            "{:?} has no editable clock entries",
            target_pstate
        )));
    }

    if !entries
        .iter()
        .any(|(_, domain, _)| *domain == target_domain)
    {
        return Err(Error::from(format!(
            "{:?} {:?} clock entry not found or not editable",
            target_pstate, target_domain
        )));
    }

    gpu.inner()
        .set_pstates(entries.drain(..))
        .map_err(Error::from)?;

    if let Some(vfp) = graphics_vfp {
        restore_graphics_vfp(gpu, &vfp)?;
    }

    Ok(())
}

fn capture_graphics_vfp(gpu: &Gpu) -> Result<Option<BTreeMap<usize, KilohertzDelta>>, Error> {
    let settings = gpu.settings().map_err(Error::from)?;
    Ok(settings.vfp.map(|vfp| vfp.graphics))
}

fn restore_graphics_vfp(gpu: &Gpu, vfp: &BTreeMap<usize, KilohertzDelta>) -> Result<(), Error> {
    gpu.set_vfp(
        vfp.iter().map(|(&point, &delta)| (point, delta)),
        iter::empty(),
    )?;
    Ok(())
}

/// 将所有 pstate 的 Core baseVoltage delta 清零（适用于 Maxwell / 9 系及更早）。
/// 遍历驱动报告的全部 pstate，对每个含有可编辑 Core baseVoltage 的条目发起单独写入，
/// 单个 pstate 失败时打印警告并继续，不中断其他 pstate 的清零。
#[allow(clippy::field_reassign_with_default)] // struct literal form fails: NV_GPU_PERF_PSTATES20_INFO V1/V2 type alias mismatch
pub fn reset_all_pstate_base_voltages(gpu: &Gpu) -> Result<(), Error> {
    use nvapi_hi::sys::gpu::pstate as sys_pstate;

    let pstates = gpu
        .inner()
        .pstates()
        .map_err(|e| Error::from(format!("Failed to read pstates: {:?}", e)))?;

    for ps in &pstates.pstates {
        // 找到 Core 域的 baseVoltage 条目
        let base_volt = match ps
            .base_voltages
            .iter()
            .find(|v| v.voltage_domain == VoltageDomain::Core)
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
            ve.domainId = VoltageDomain::Core.raw();
            ve.bIsEditable = nvapi_hi::sys::types::BoolU32::from(true);
            ve.volt_uV = base_volt.voltage.0;
            ve.voltDelta_uV.value = 0; // 清零
            ve.voltDelta_uV.min = range.min.0;
            ve.voltDelta_uV.max = range.max.0;
        }

        let status =
            unsafe { sys_pstate::private::NvAPI_GPU_SetPstates20(*gpu.inner().handle(), &info) };

        let _ = status;
    }

    Ok(())
}

pub fn set_cooler_levels(
    gpus: &[&Gpu],
    mode: CoolerPolicy,
    level: u32,
    target: CoolerTarget,
) -> Result<(), Error> {
    let settings = CoolerSettings {
        policy: mode,
        level: Some(Percentage(level)),
    };

    for gpu in gpus {
        match target {
            CoolerTarget::Cooler1 => {
                gpu.set_cooler_levels([(FanCoolerId::Cooler1, settings)])?;
            }
            CoolerTarget::Cooler2 => {
                gpu.set_cooler_levels([(FanCoolerId::Cooler2, settings)])?;
            }
            CoolerTarget::All => {
                gpu.set_cooler_levels([
                    (FanCoolerId::Cooler1, settings),
                    (FanCoolerId::Cooler2, settings),
                ])?;
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

pub fn set_vfp_frequency_lock(
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

pub fn parse_nvapi_locked_voltage_target(raw: &str) -> Result<NvapiLockedVoltageTarget, Error> {
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

// ---------------------------------------------------------------------------
// P-State lock (via memory VFP frequency window)
// ---------------------------------------------------------------------------

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

    let supported_pstates = pstates
        .iter()
        .map(|(reported_pstate, _, _, _, _)| {
            nvml_pstate_to_str(*reported_pstate)
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
                nvml_pstate_to_str(high_perf_pstate),
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
                nvml_pstate_to_str(low_perf_pstate),
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
                nvml_pstate_to_index(*reported_pstate),
                nvml_pstate_to_str(*reported_pstate),
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

pub fn lock_vfp(gpus: &[&Gpu], request: VfpLockRequest, feedback_flag: bool) -> Result<(), Error> {
    if let VfpLockRequest::Frequency {
        domain,
        upper,
        lower,
    } = request
    {
        for gpu in gpus {
            set_vfp_frequency_lock(gpu, domain, upper, lower)?;
        }
        return Ok(());
    }

    for gpu in gpus {
        let v = match request {
            VfpLockRequest::VoltagePoint(point) => get_voltage_by_point(gpu, point)?,
            VfpLockRequest::Voltage(voltage) => voltage,
            VfpLockRequest::Frequency { .. } => unreachable!(),
        };

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

        let mut count = 0;
        let mut flag = 0;
        let mut skew_rate = 45;

        while (v.0 as i32 - core_v.0 as i32) / 1000 > crit_volt_margin
            || (core_v.0 as i32 - v.0 as i32) / 1000 > crit_volt_margin
        {
            if count >= 4 {
                flag = 1;
                break;
            }

            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    ((v.0 as i32 - core_v.0 as i32).abs() / skew_rate) as u64,
                ));
            }
            gpu.set_vfp_lock_voltage(Some(v))?;
            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    ((v.0 as i32 - core_v.0 as i32).abs() / skew_rate) as u64,
                ));
            }
            core_v = gpu.inner().core_voltage()?;
            if skew_rate_enabled == 1 {
                sleep(Duration::from_millis(
                    ((v.0 as i32 - core_v.0 as i32).abs() / skew_rate) as u64,
                ));
            }
            count += 1;
            skew_rate -= 4;
        }

        if flag == 1 {
            return Err(Error::Str("Failed to lock voltage"));
        }
    }
    Ok(())
}

pub fn reset_vfp_deltas(gpu: &Gpu, domain: VfpResetDomain) -> Result<(), Error> {
    let info = gpu.info().map_err(Error::from)?;
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
        if reset_graphics
            && gpu
                .inner()
                .set_pstates(
                    [(PState::P0, ClockDomain::Graphics, KilohertzDelta(0))]
                        .iter()
                        .cloned(),
                )
                .is_ok()
        {
            any_ok = true;
        }
        if reset_memory
            && gpu
                .inner()
                .set_pstates(
                    [(PState::P0, ClockDomain::Memory, KilohertzDelta(0))]
                        .iter()
                        .cloned(),
                )
                .is_ok()
        {
            any_ok = true;
        }
        if any_ok {
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
            let _ = e;
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
            let _ = e;
        }
        r.is_ok()
    } else {
        false
    };

    if graphics_ok || memory_ok {
        return Ok(());
    }

    // 保底路径：逐点清零 VFP 曲线
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
                )
                .map_err(Error::from)?;
            }
            VfpResetDomain::Core => {
                gpu.set_vfp(iter::once((point, KilohertzDelta(0))), iter::empty())
                    .map_err(Error::from)?;
            }
            VfpResetDomain::Memory => {
                gpu.set_vfp(iter::empty(), iter::once((point, KilohertzDelta(0))))
                    .map_err(Error::from)?;
            }
        }
    }
    Ok(())
}

pub fn core_reset_vfp(gpu: &Gpu) -> Result<(), Error> {
    reset_vfp_deltas(gpu, VfpResetDomain::All)
}

pub fn adjust_single_vfp_point(gpus: &[&Gpu], point: usize, delta_khz: i32) -> Result<(), Error> {
    for gpu in gpus {
        gpu.set_vfp(
            iter::once((point, KilohertzDelta(delta_khz))),
            iter::empty(),
        )?;
    }

    Ok(())
}

pub fn set_pointwise_vfp_delta(
    gpus: &[&Gpu],
    start: usize,
    end: usize,
    delta_khz: i32,
) -> Result<(), Error> {
    if start > end {
        return Err(Error::from(format!(
            "Range start ({}) must be <= end ({}).",
            start, end
        )));
    }
    for gpu in gpus {
        set_vfp_range(gpu, start..=end, delta_khz)?;
    }

    Ok(())
}

pub fn query_domain_vf_points_indexed(
    gpu: &Gpu,
    domain: ClockDomain,
    infer_missing_default: bool,
) -> Result<Vec<(usize, nvapi_hi::VfPoint)>, Error> {
    let info = gpu.inner().vfp_info()?;
    let curve = gpu.inner().vfp_curve(&info)?;
    let table = gpu.inner().vfp_table(&info)?;

    let points = curve.points.get(&domain).cloned().unwrap_or_default();
    let deltas = table.delta_points.get(&domain).cloned().unwrap_or_default();

    let mut pts = points.into_iter().peekable();
    let mut dts = deltas.into_iter().peekable();
    let mut result = Vec::new();
    loop {
        let ord = match (pts.peek(), dts.peek()) {
            (None, _) | (_, None) => break,
            (Some((i0, _)), Some((i1, _))) => i0.cmp(i1),
        };
        match ord {
            Ordering::Equal => {
                let (i, point) = pts.next().unwrap();
                let (_, delta) = dts.next().unwrap();
                let mut point = nvapi_hi::VfPoint {
                    voltage: point.configured().voltage,
                    frequency: point.configured().frequency,
                    default_frequency: point.default().map(|p| p.frequency).unwrap_or_default(),
                    delta,
                };
                if infer_missing_default && point.default_frequency.0 == 0 {
                    let base = point.frequency.0 as i64 - point.delta.0 as i64;
                    point.default_frequency = Kilohertz(base.max(0) as u32);
                }
                result.push((i, point));
            }
            Ordering::Less => {
                pts.next();
            }
            Ordering::Greater => {
                dts.next();
            }
        }
    }
    Ok(result)
}

pub fn query_domain_vfp_indices(gpu: &Gpu, domain: ClockDomain) -> Result<Vec<usize>, Error> {
    let info = gpu.inner().vfp_info()?;
    Ok(info.iter(domain).collect())
}

pub fn set_nvapi_domain_vfp_deltas(
    gpu: &Gpu,
    domain: ClockDomain,
    deltas: &[(usize, KilohertzDelta)],
) -> Result<(), Error> {
    use nvapi_hi::sys::gpu::clock::private::NV_GPU_CLOCK_CLIENT_CLK_VF_POINTS_CONTROL;

    let info = gpu
        .inner()
        .vfp_info()
        .map_err(|e| Error::Custom(format!("NvAPI vfp_info failed: {}", e)))?;
    let domain_indices: HashSet<usize> = info.iter(domain).collect();

    let mut data = NV_GPU_CLOCK_CLIENT_CLK_VF_POINTS_CONTROL {
        mask: info.mask.mask,
        ..Default::default()
    };

    unsafe {
        let status = nvapi_hi::sys::api::NvAPI_GPU_ClockClientClkVfPointsGetControl(
            *gpu.inner().handle(),
            &mut data,
        );
        nvapi_hi::sys::status_result(status).map_err(|e| {
            Error::Custom(format!(
                "NvAPI_GPU_ClockClientClkVfPointsGetControl failed: {:?}",
                e
            ))
        })?;
    }

    for &(i, delta) in deltas {
        if !domain_indices.contains(&i) {
            return Err(Error::Custom(format!(
                "VFP point index {i} is not a {:?} domain point",
                domain
            )));
        }
        data.points[i].freqDeltaKHz = delta.0;
        data.mask.set_bit(i);
    }

    unsafe {
        let status = nvapi_hi::sys::api::NvAPI_GPU_ClockClientClkVfPointsSetControl(
            *gpu.inner().handle(),
            &data,
        );
        nvapi_hi::sys::status_result(status).map_err(|e| {
            Error::Custom(format!(
                "NvAPI_GPU_ClockClientClkVfPointsSetControl failed: {:?}",
                e
            ))
        })?;
    }

    Ok(())
}

pub fn handle_test_voltage_limits(
    gpus: &[&Gpu],
    mut print_separator: impl FnMut(),
) -> Result<(usize, usize), Error> {
    let mut upper_init_point: usize = 70;
    let mut lower_init_point: usize = 60;
    let mut vfp_strict_inc_flag = 0;
    let mut margin_threshold_check = 0;

    for gpu in gpus {
        core_reset_vfp(gpu)?;

        let info = gpu.info()?;
        let gpu_type = fetch_gpu_type(&info);

        if !(info.name.contains("Laptop") || info.name.contains("Device")) {
            let _ = gpu.set_voltage_boost(Percentage(100));
        }

        if let Ok(ref t) = gpu_type {
            let vlp = t.voltage_limit_params();
            upper_init_point = vlp.upper_init_point;
            lower_init_point = vlp.lower_init_point;
            if vlp.vfp_strict_inc_flag {
                vfp_strict_inc_flag = 1;
            }
            if vlp.margin_threshold_check {
                margin_threshold_check = 1;
            }

            if matches!(t, GpuType::Mobile9Series | GpuType::Desktop9Series) {
                drop(Error::VfpUnsupported);
            }
        }
    }

    let mut current_test_point = upper_init_point;
    let mut voltage: Option<Microvolts> = None;
    let mut flat_curve_modifier_flag = vfp_strict_inc_flag;
    let mut revert_scan_flag = 0;
    let mut upper_target_point: usize = 0;

    // Iterate over a range of test voltage values
    let lower_target_point: usize = loop {
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

        voltage.ok_or(Error::VfpUnsupported)?;

        match lock_vfp(gpus, VfpLockRequest::VoltagePoint(current_test_point), true) {
            Ok(_) => {
                if revert_scan_flag == 0 {
                    current_test_point += 1;
                } else {
                    current_test_point -= 1;
                }
                if vfp_strict_inc_flag == 1 {
                    flat_curve_modifier_flag = 1;
                }
            }
            Err(e) => {
                drop(e);

                if vfp_strict_inc_flag == 1 && flat_curve_modifier_flag == 1 {
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
                }

                if revert_scan_flag == 0 {
                    upper_target_point = current_test_point - 1;
                    print_separator();
                    print_separator();
                    revert_scan_flag = 1;
                    flat_curve_modifier_flag = 1;
                    for gpu in gpus {
                        core_reset_vfp(gpu)?;
                    }
                    current_test_point = lower_init_point;
                } else {
                    let lower_target_point = current_test_point + 1;
                    print_separator();
                    print_separator();
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
    gpus: &[&Gpu],
    mut print_separator: impl FnMut(),
) -> Result<GpuTdpTempLimits, Error> {
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

    for gpu in gpus {
        let info = gpu.info()?;

        //power limit readout
        for limit in info.power_limits.iter() {
            max_tdp_percentage = limit.range.max;
            min_tdp_percentage = limit.range.min;
            default_tdp_percentage = limit.default;
            print_separator();
            print_separator();
        }

        //temp limit readout
        for (_sensor, limit) in info.sensors.iter().zip(
            info.sensor_limits
                .iter()
                .map(Some)
                .chain(iter::repeat(None)),
        ) {
            if let Some(limit) = limit {
                min_temp_lim = limit.range.min;
                max_temp_lim = limit.range.max;
                default_temp_lim = limit.default;
                if let Some(pff) = &limit.throttle_curve {
                    current_pff_curve = pff.clone();
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

pub fn voltage_frequency_check(
    gpus: &[&Gpu],
    point: usize,
    mut print_separator: impl FnMut(),
) -> Result<bool, Error> {
    let mut precise_flag = false;

    for gpu in gpus {
        let status = gpu.status()?;
        let readout_v = status.voltage.ok_or_else(|| Error::Custom("GPU did not report voltage in status; check if the GPU supports voltage monitoring".into()))?;
        print_separator();

        let current_point = status.clone().vfp.ok_or(Error::VfpUnsupported)?.graphics;

        let default_v = current_point
            .get(&(point))
            .ok_or(Error::Str("invalid point index"))?
            .voltage;

        let sensor_v = gpu.inner().core_voltage()?;

        if let Some((index, vfp_point)) = find_matching_vfp_point(&current_point, sensor_v) {
            let _ = (readout_v, default_v, vfp_point);
            precise_flag = index.abs_diff(point) < 5;
        } else {
            precise_flag = false;
        }
        print_separator();
    }
    Ok(precise_flag)
}

// ---------------------------------------------------------------------------
// VFP batch-setting helpers used by the CLI autoscan path.
// ---------------------------------------------------------------------------

/// Set the same frequency delta on a VFP point range, propagating errors.
pub fn set_vfp_range(
    gpu: &&Gpu,
    range: std::ops::RangeInclusive<usize>,
    delta_khz: i32,
) -> Result<(), Error> {
    Ok(gpu.set_vfp(
        range.map(|offset| (offset, KilohertzDelta(delta_khz))),
        iter::empty(),
    )?)
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
