use nvoc_cli_common::color::{stylize, stylize_title};
use nvoc_core::{
    ClockDomain, CoolerControl, CoolerPolicy, GpuInfo, GpuSettings, GpuStatus, GpuTarget,
    QueryPowerLimits, ThermalSensors, legacy_core_overvolt_ranges, run,
};
use std::iter;

const HEADER_LEN: usize = 20;
const SCAN_SEPARATOR: &str =
    "================================================================================";

pub fn print_scan_separator() {
    println!("{}", stylize(SCAN_SEPARATOR, false));
}

fn parse_edid(edid: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if edid.len() < 128 {
        return out;
    }
    if &edid[0..8] != b"\x00\xFF\xFF\xFF\xFF\xFF\xFF\x00" {
        return out;
    }

    let mfg = u16::from_be_bytes([edid[8], edid[9]]);
    let char1 = ((mfg >> 10) & 0x1F) as u8 + b'A' - 1;
    let char2 = ((mfg >> 5) & 0x1F) as u8 + b'A' - 1;
    let char3 = (mfg & 0x1F) as u8 + b'A' - 1;
    let mfg_id = format!("{}{}{}", char1 as char, char2 as char, char3 as char);
    out.push(("Manufacturer".into(), mfg_id));

    let product_code = u16::from_le_bytes([edid[10], edid[11]]);
    out.push(("Product Code".into(), format!("0x{:04X}", product_code)));

    let s_no = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
    if s_no != 0 {
        out.push(("Serial Number".into(), s_no.to_string()));
    }

    let week = edid[16];
    let year = edid[17] as u16 + 1990;
    if week > 0 && week <= 54 {
        out.push(("Manufactured".into(), format!("Week {}, {}", week, year)));
    } else {
        out.push(("Manufactured".into(), year.to_string()));
    }

    let digital = (edid[20] & 0x80) != 0;
    out.push((
        "Input Signal".into(),
        if digital {
            "Digital".into()
        } else {
            "Analog".into()
        },
    ));

    let width_cm = edid[21];
    let height_cm = edid[22];
    if width_cm > 0 && height_cm > 0 {
        out.push((
            "Screen Size".into(),
            format!("{} cm x {} cm", width_cm, height_cm),
        ));
    }

    let gamma = edid[23];
    if gamma > 0 && gamma != 0xFF {
        out.push((
            "Gamma".into(),
            format!("{:.2}", (gamma as f32 + 100.0) / 100.0),
        ));
    }

    let features = edid[24];
    let mut dpms = Vec::new();
    if features & 0x80 != 0 {
        dpms.push("Standby");
    }
    if features & 0x40 != 0 {
        dpms.push("Suspend");
    }
    if features & 0x20 != 0 {
        dpms.push("ActiveOff");
    }
    if !dpms.is_empty() {
        out.push(("DPMS Features".into(), dpms.join(", ")));
    }

    let color_type = if digital {
        match (features >> 3) & 0x03 {
            0 => "RGB 4:4:4",
            1 => "RGB 4:4:4 & YCrCb 4:4:4",
            2 => "RGB 4:4:4 & YCrCb 4:2:2",
            _ => "RGB 4:4:4 & YCrCb 4:4:4 & 4:2:2",
        }
    } else {
        match (features >> 3) & 0x03 {
            0 => "Monochrome",
            1 => "RGB",
            2 => "Non-RGB",
            _ => "Undefined",
        }
    };
    out.push(("Color Format".into(), color_type.into()));

    let mut name = String::new();
    let mut serial_str = String::new();
    let mut range_limits = String::new();

    for i in 0..4 {
        let offset = 54 + i * 18;
        if offset + 18 > edid.len() {
            continue;
        }
        let block = &edid[offset..offset + 18];
        if block[0] != 0 || block[1] != 0 || block[2] != 0 {
            if i == 0 {
                let pixel_clock = u16::from_le_bytes([block[0], block[1]]);
                if pixel_clock > 0 {
                    let hactive = block[2] as u16 | (((block[4] >> 4) as u16) << 8);
                    let vactive = block[5] as u16 | (((block[7] >> 4) as u16) << 8);
                    out.push(("Native Res".into(), format!("{}x{}", hactive, vactive)));
                }
            }
        } else {
            let tag = block[3];
            if tag == 0xFC || tag == 0xFF {
                let mut text = String::new();
                for &b in &block[5..18] {
                    if b == 0x0A {
                        break;
                    }
                    if b.is_ascii_graphic() || b == b' ' {
                        text.push(b as char);
                    }
                }
                let text = text.trim().to_string();
                if tag == 0xFC {
                    name = text;
                } else if tag == 0xFF {
                    serial_str = text;
                }
            } else if tag == 0xFD {
                let v_min = block[5];
                let v_max = block[6];
                let h_min = block[7];
                let h_max = block[8];
                let max_clock = (block[9] as u16) * 10;
                range_limits = format!(
                    "{}~{} Hz (V) | {}~{} kHz (H) | Max {} MHz",
                    v_min, v_max, h_min, h_max, max_clock
                );
            }
        }
    }

    if !name.is_empty() {
        out.push(("Model Name".into(), name));
    }
    if !serial_str.is_empty() {
        out.push(("Serial Number".into(), serial_str));
    }
    if !range_limits.is_empty() {
        out.push(("Range Limits".into(), range_limits));
    }

    out
}

macro_rules! pline {
    ($header:expr, $($tt:tt)*) => {
        {
            let mut header = $header.to_string();
            while header.len() < HEADER_LEN {
                header.push('.');
            }
            println!(
                "{}: {}",
                stylize_title(&header),
                stylize(&format!($($tt)*), false)
            );
        }
    };
}

fn n_a() -> String {
    "N/A".into()
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
            pline!(format!("VFP Lock {}", id), "{}", value);
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
                .map(|(limit, lock)| format!("{}:{}", limit, lock))
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

#[allow(dead_code)]
pub fn print_thermal_sensors(sensors: &ThermalSensors) {
    if let Some(hotspot) = sensors.hotspot {
        pline!("GPU Hotspot", "{}", hotspot);
    } else {
        pline!("GPU Hotspot", "N/A");
    }
    if let Some(vram) = sensors.vram {
        pline!("VRAM Temp.", "{}", vram);
    } else {
        pline!("VRAM Temp.", "N/A");
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
    if info.physical_frame_buffer.0 > 0 {
        pline!(
            "VRAM Size",
            "{:.2} physical / {:.2} virtual",
            info.physical_frame_buffer,
            info.virtual_frame_buffer
        );
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
    if !info.connected_displays.is_empty() {
        for display in &info.connected_displays {
            pline!(
                "Connected Display",
                "0x{:08X} ({})",
                display.display_id,
                display.connector
            );
            if let Ok(nvapi) = gpu.nvapi() {
                if let Ok(edid) = nvapi.inner().get_edid(display.display_id) {
                    if !edid.is_empty() {
                        for (k, v) in parse_edid(&edid) {
                            pline!(format!("-- {}", k), "{}", v);
                        }
                        // pline!(
                        //     "EDID Bytes",
                        //     "{} bytes: {}",
                        //     edid.len(),
                        //     edid.iter()
                        //         .map(|b| format!("{:02X}", b))
                        //         .collect::<Vec<_>>()
                        //         .as_slice()
                        //         .chunks(16)
                        //         .map(|c| c.join(""))
                        //         .collect::<Vec<_>>()
                        //         .join(" ")
                        // );
                    }
                }
            }
        }
    }
}
