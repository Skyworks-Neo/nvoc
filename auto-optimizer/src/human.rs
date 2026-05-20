use nvoc_core::{
    ClockDomain, CoolerControl, CoolerPolicy, GpuInfo, GpuSettings, GpuStatus, GpuTarget,
    QueryPowerLimits, legacy_core_overvolt_ranges, run,
};
use std::iter;

const HEADER_LEN: usize = 20;
const SCAN_SEPARATOR: &str =
    "================================================================================";

pub fn print_scan_separator() {
    println!("{}", SCAN_SEPARATOR);
}

macro_rules! pline {
    ($header:expr, $($tt:tt)*) => {
        {
            let mut header = $header.to_string();
            while header.len() < HEADER_LEN {
                header.push('.');
            }
            print!("{}: ", header);
            println!($($tt)*);
        }
    };
}

fn n_a() -> String {
    "N/A".into()
}

fn vfp_lock_label<T: std::fmt::Display>(id: &T) -> String {
    match id.to_string().as_str() {
        "GPU" => "GPU Core Upperbound".to_string(),
        "GpuUnknown" => "GPU Core Lowerbound".to_string(),
        "Memory" => "Memory Upperbound".to_string(),
        "MemoryUnknown" => "Memory Lowerbound".to_string(),
        other => other.to_string(),
    }
}

pub fn print_settings(gpu: &GpuTarget<'_>, set: &GpuSettings) {
    if let Some(ref boost) = set.voltage_boost {
        pline!("Voltage Boost", "{} (range: 0%-100%)", boost);
    }
    for limit in &set.sensor_limits {
        pline!(
            "Thermal Limit",
            "{}{}{}",
            limit.value,
            match &limit.curve {
                Some(pff) => format!(": {}", pff),
                None => n_a(),
            },
            if limit.remove_tdp_limit {
                " (TDP Limit Removed)"
            } else {
                ""
            }
        );
    }
    for limit in &set.power_limits {
        pline!("Power Limit", "{}", limit);
    }
    for (id, cooler) in &set.coolers {
        let level_str = match cooler.level {
            Some(level) => format!("Level: {}", level),
            None => "Level: N/A".to_string(),
        };
        let policy_str = format!("Policy: {}", cooler.policy);
        pline!(format!("Cooler {}", id), "{} | {}", policy_str, level_str);
    }
    for (pstate, clock, delta) in set
        .pstate_deltas
        .iter()
        .flat_map(|(ps, d)| d.iter().map(move |(clock, d)| (ps, clock, d)))
    {
        pline!(format!("{} @ {} Offset", clock, pstate), "{}", delta);
    }
    let legacy_overvolt = legacy_core_overvolt_ranges(gpu).unwrap_or_default();
    if !legacy_overvolt.is_empty() {
        for (pstate, current, min, max) in legacy_overvolt {
            pline!(
                format!("Overvolt {}", pstate),
                "{} (range: {} - {})",
                current,
                min,
                max
            );
        }
    } else {
        for ov in &set.overvolt {
            pline!("Overvolt", "{}", ov);
        }
    }
    for (id, lock) in &set.vfp_locks {
        if let Some(value) = lock.lock_value {
            pline!(format!("VFP Lock {}", vfp_lock_label(id)), "{}", value);
        }
    }
}

pub fn print_status(status: &GpuStatus) {
    pline!("Power State", "{}", status.pstate);
    pline!(
        "Power Usage",
        "{}",
        status
            .power
            .iter()
            .fold(None, |state, (ch, power)| if let Some(state) = state {
                Some(format!("{}, {} ({})", state, power, ch))
            } else {
                Some(format!("{} ({})", power, ch))
            })
            .unwrap_or_else(n_a)
    );
    if let Some(memory) = &status.memory {
        pline!(
            "Memory Usage",
            "{:.2} / {:.2} ({} evictions totalling {:.2})",
            memory.dedicated_available - memory.dedicated_available_current,
            memory.dedicated_available,
            memory.dedicated_evictions,
            memory.dedicated_evictions_size,
        );
    }
    if status.ecc.enabled {
        pline!(
            "ECC Errors",
            "{} 1-bit, {} 2-bit",
            status.ecc.errors.current.single_bit_errors,
            status.ecc.errors.current.double_bit_errors
        );
        if status.ecc.errors.current != status.ecc.errors.aggregate {
            pline!(
                "ECC Errors",
                "{} 1-bit, {} 2-bit (Aggregate)",
                status.ecc.errors.aggregate.single_bit_errors,
                status.ecc.errors.aggregate.double_bit_errors
            );
        }
    }
    if let Some(lanes) = status.pcie_lanes {
        pline!("PCIe Bus Width", "x{}", lanes);
    }
    pline!(
        "Core Voltage",
        "{}",
        status.voltage.map(|v| v.to_string()).unwrap_or_else(n_a)
    );
    pline!(
        "Limits",
        "{}",
        status
            .perf
            .limits
            .fold(None, |state, v| if let Some(state) = state {
                Some(format!("{}, {}", state, v))
            } else {
                Some(v.to_string())
            })
            .unwrap_or_else(n_a)
    );
    pline!(
        "VFP Lock",
        "{}",
        if status.vfp_locks.is_empty() {
            "None".into()
        } else {
            status
                .vfp_locks
                .iter()
                .map(|(limit, lock)| format!("{}:{}", vfp_lock_label(limit), lock))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );

    for (clock, freq) in &status.clocks {
        pline!(format!("{} Clock", clock), "{}", freq);
    }

    for (res, util) in &status.utilization {
        pline!(format!("{} Load", res), "{}", util);
    }

    for (sensor, temp) in &status.sensors {
        pline!(
            "Sensor",
            "{} ({} / {})",
            temp,
            sensor.controller,
            sensor.target
        );
    }

    for (i, cooler) in &status.coolers {
        let variable_control = true; // TODO!!
        let level = match cooler.active {
            true if variable_control => cooler.current_level.to_string(),
            true => "On".into(),
            false => "Off".into(),
        };
        let tach = match cooler.current_tach {
            Some(tach) => format!(" ({})", tach),
            None => String::new(),
        };
        pline!(format!("Cooler {}", i), "{}{}", level, tach);
    }
}

pub fn print_info(gpu: &GpuTarget<'_>, info: &GpuInfo) {
    pline!(
        format!("GPU {}", info.id),
        "{} ({})",
        info.name,
        info.codename
    );
    pline!("Architecture", "{} ({})", info.arch, info.gpu_type);
    pline!("Vendor", "{}", info.vendor().unwrap_or_default());
    pline!(
        "GPU Shaders",
        "{} ({}:{} pipes)",
        info.core_count,
        info.shader_pipe_count,
        info.shader_sub_pipe_count
    );
    if let Some(memory) = &info.memory {
        pline!(
            "Video Memory",
            "{:.2} {}-bit",
            memory.dedicated,
            info.ram_bus_width
        );
    } else {
        pline!("Video Memory", "{} {}-bit", n_a(), info.ram_bus_width);
    }
    pline!("Memory Type", "{} ({})", info.ram_type, info.ram_maker);
    pline!(
        "Memory Banks",
        "{} ({} partitions)",
        info.ram_bank_count,
        info.ram_partition_count
    );
    if let Some(memory) = &info.memory {
        pline!("Memory Avail", "{:.2}", memory.dedicated_available);
        pline!(
            "Shared Memory",
            "{:.2} ({:.2} system)",
            memory.shared,
            memory.system
        );
    }
    pline!(
        "ECC",
        "{} ({})",
        if info.ecc.info.enabled {
            "Yes"
        } else if info.ecc.info.supported {
            "Disabled"
        } else {
            "N/A"
        },
        info.ecc.info.configuration
    );
    pline!("Foundry", "{}", info.foundry);
    pline!("Bus", "{}", info.bus);
    if let Some(ids) = info.bus.bus.pci_ids() {
        pline!("PCI IDs", "{}", ids);
    }
    pline!("BIOS Version", "{}", info.bios_version);
    if let Some(driver_model) = &info.driver_model {
        pline!("Driver Model", "{}", driver_model);
    }
    pline!(
        "Limit Support",
        "{}",
        info.perf
            .limits
            .fold(None, |state, v| if let Some(state) = state {
                Some(format!("{}, {}", state, v))
            } else {
                Some(v.to_string())
            })
            .unwrap_or_else(|| "None".into())
    );
    if info.vfp_limits.is_empty() {
        pline!("VFP", "No");
    } else {
        for (clock, limit) in &info.vfp_limits {
            pline!(format!("VFP ({})", clock), "{}", limit.range);
        }
    }

    for limit in info.power_limits.iter() {
        // 使用 NVAPI GPU ID 直接查询（公式：GPU_ID = PCI_Bus × 256）
        match run(gpu, QueryPowerLimits).map(|report| report.output) {
            Ok(power) => {
                pline!(
                    "Power Limit",
                    "{} ({} default) | {:.0}W min / {:.0}W current / {:.0}W max",
                    limit.range,
                    limit.default,
                    power.min_watts,
                    power.current_watts,
                    power.max_watts
                );
            }
            Err(_) => {
                pline!("Power Limit", "{} ({} default)", limit.range, limit.default);
            }
        }
    }

    for clock in ClockDomain::values() {
        if let (Some(base), boost) = (info.base_clocks.get(&clock), info.boost_clocks.get(&clock)) {
            pline!(
                format!("{} Clock", clock),
                "{} ({} boost)",
                base,
                boost.map(ToString::to_string).unwrap_or_else(n_a)
            );
        }
    }

    for (sensor, limit) in info.sensors.iter().zip(
        info.sensor_limits
            .iter()
            .map(Some)
            .chain(iter::repeat(None)),
    ) {
        pline!(
            "Thermal Sensor",
            "{} / {} ({} range)",
            sensor.controller,
            sensor.target,
            sensor.range
        );
        if let Some(limit) = limit {
            pline!(
                "Thermal Limit",
                "{} ({} default)",
                limit.range,
                limit.default
            );
            if let Some(pff) = &limit.throttle_curve {
                pline!("Thermal Throttle", "{}", pff);
            }
        }
    }

    for (id, cooler) in info.coolers.iter() {
        let range = match (cooler.default_level_range, cooler.tach_range) {
            (Some(level), Some(tach)) => Some(format!("{} / {}", level, tach)),
            (None, Some(tach)) => Some(tach.to_string()),
            (Some(level), None) => Some(level.to_string()),
            (None, None) => None,
        };
        pline!(
            format!("Cooler {}", id),
            "{} / {} / {}{}",
            cooler.kind,
            cooler.controller,
            cooler.target,
            match range {
                Some(range) => format!(" ({} range)", range),
                None => match cooler.control {
                    CoolerControl::Variable => "",
                    CoolerControl::Toggle => "(On/Off control)",
                    CoolerControl::None => " (Read-only)",
                    _ => "",
                }
                .into(),
            },
        );
        if cooler.default_policy != CoolerPolicy::None {
            pline!(
                format!("Cooler {} Default", id),
                "{} Mode",
                cooler.default_policy
            );
        }
    }
    let legacy_overvolt = legacy_core_overvolt_ranges(gpu).unwrap_or_default();
    if !legacy_overvolt.is_empty() {
        for (pstate, current, min, max) in legacy_overvolt {
            pline!(
                format!("Overvolt {}", pstate),
                "{} (range: {} - {})",
                current,
                min,
                max
            );
        }
    }
}
