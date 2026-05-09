use crate::human;
use crate::types::{OutputFormat, ResetSettings, VfpResetDomain};
use nvapi_hi::{
    allowable_result, Celsius, Gpu, GpuSettings, KilohertzDelta, MicrovoltsDelta, PState,
    Percentage,
};
use nvml_wrapper::Nvml;
use std::io;

use crate::conv::ConvertEnum;
use crate::error::Error;
use crate::oc_get_set_function_nvapi::{reset_all_pstate_base_voltages, reset_vfp_deltas};
use clap::ArgMatches;
use time::{format_description::parse, OffsetDateTime};

pub fn local_time_hms() -> String {
    let format = match parse("[hour]:[minute]:[second]") {
        Ok(format) => format,
        Err(_) => return String::from("??:??:??"),
    };

    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());

    now.format(&format)
        .unwrap_or_else(|_| String::from("??:??:??"))
}

// display_info replaced by direct Win32 API call to avoid pulling in the entire `windows` crate
#[cfg(windows)]
fn get_primary_screen_size_raw() -> (u32, u32) {
    use std::ffi::c_int;
    unsafe extern "system" {
        fn EnumDisplaySettingsW(
            lpsz_device_name: *const u16,
            i_mode_num: u32,
            lp_dev_mode: *mut u8,
        ) -> c_int;
    }
    // DEVMODEW（wingdi.h）Win32 ABI 固定布局，x86-64 下：
    //   +  0  dmDeviceName[32]  = 64 字节
    //   + 64  dmSpecVersion     = 2 字节
    //   + 66  dmDriverVersion   = 2 字节
    //   + 68  dmSize            = 2 字节  ← OFFSET_DM_SIZE
    //   + 70  dmDriverExtra     = 2 字节
    //   + 72  dmFields          = 4 字节
    //   +76~+171  union（显示模式字段，96 字节）
    //   +172  dmPelsWidth       = 4 字节  ← OFFSET_PELS_WIDTH
    //   +176  dmPelsHeight      = 4 字节  ← OFFSET_PELS_HEIGHT
    //   …（后续字段省略，总结构体大小 220 字节）
    // 参考：https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-devmodew
    const DEVMODEW_SIZE: usize = 220;
    const OFFSET_DM_SIZE: usize = 68;
    const OFFSET_PELS_WIDTH: usize = 172;
    const OFFSET_PELS_HEIGHT: usize = 176;
    const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;
    unsafe {
        let mut buf = [0u8; DEVMODEW_SIZE];
        buf[OFFSET_DM_SIZE] = DEVMODEW_SIZE as u8;
        buf[OFFSET_DM_SIZE + 1] = (DEVMODEW_SIZE >> 8) as u8;
        let ret = EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, buf.as_mut_ptr());
        if ret != 0 {
            let w = u32::from_le_bytes(
                buf[OFFSET_PELS_WIDTH..OFFSET_PELS_WIDTH + 4]
                    .try_into()
                    .unwrap(),
            );
            let h = u32::from_le_bytes(
                buf[OFFSET_PELS_HEIGHT..OFFSET_PELS_HEIGHT + 4]
                    .try_into()
                    .unwrap(),
            );
            (w, h)
        } else {
            (0, 0)
        }
    }
}
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResolution {
    R3600x1920,
    R3072x1600,
    R2560x1440,
    R2048x1536,
    R1920x1200,
    R1680x1050,
    R1440x1080,
    R1200x960,
    R1080x864,
    R960x800,
    R864x640,
    R800x600,
    R768x576,
    R720x480,
    R640x384,
    R576x360,
    R400x300,
}

impl TestResolution {
    // Get the next lower resolution
    pub fn downgrade(self) -> Option<Self> {
        match self {
            TestResolution::R3600x1920 => Some(TestResolution::R3072x1600),
            TestResolution::R3072x1600 => Some(TestResolution::R2560x1440),
            TestResolution::R2560x1440 => Some(TestResolution::R2048x1536),
            TestResolution::R2048x1536 => Some(TestResolution::R1920x1200),
            TestResolution::R1920x1200 => Some(TestResolution::R1680x1050),
            TestResolution::R1680x1050 => Some(TestResolution::R1440x1080),
            TestResolution::R1440x1080 => Some(TestResolution::R1200x960),
            TestResolution::R1200x960 => Some(TestResolution::R1080x864),
            TestResolution::R1080x864 => Some(TestResolution::R960x800),
            TestResolution::R960x800 => Some(TestResolution::R864x640),
            TestResolution::R864x640 => Some(TestResolution::R800x600),
            TestResolution::R800x600 => Some(TestResolution::R768x576),
            TestResolution::R768x576 => Some(TestResolution::R720x480),
            TestResolution::R720x480 => Some(TestResolution::R640x384),
            TestResolution::R640x384 => Some(TestResolution::R640x384),
            // TestResolution::R640x384 => Some(TestResolution::R576x360),
            TestResolution::R576x360 => Some(TestResolution::R400x300),
            TestResolution::R400x300 => Some(TestResolution::R400x300),
            // Lowest resolution, no downgrade available
        }
    }

    // Get width and height as values
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            TestResolution::R3600x1920 => (3600, 1920),
            TestResolution::R3072x1600 => (3072, 1600),
            TestResolution::R2560x1440 => (2560, 1440),
            TestResolution::R2048x1536 => (2048, 1536),
            TestResolution::R1920x1200 => (1920, 1200),
            TestResolution::R1680x1050 => (1680, 1050),
            TestResolution::R1440x1080 => (1440, 1080),
            TestResolution::R1200x960 => (1200, 960),
            TestResolution::R1080x864 => (1080, 864),
            TestResolution::R960x800 => (960, 800),
            TestResolution::R864x640 => (864, 640),
            TestResolution::R800x600 => (800, 600),
            TestResolution::R768x576 => (768, 576),
            TestResolution::R720x480 => (720, 480),
            TestResolution::R640x384 => (640, 384),
            TestResolution::R576x360 => (576, 360),
            TestResolution::R400x300 => (400, 300),
        }
    }
    // Check if the resolution fits within the given width and height
    pub fn fits_in(self, width: u32, height: u32) -> bool {
        let (res_width, res_height) = self.dimensions();
        res_width < width && res_height < height
    }
}

pub fn get_primary_monitor_resolution() -> (u32, u32) {
    #[cfg(windows)]
    {
        let (w, h) = get_primary_screen_size_raw();
        if w > 0 && h > 0 {
            return (w, h);
        }
    }
    // Default resolution in case API call fails
    (1920, 1080)
}

// Function to determine the second-largest test resolution that fits the primary monitor
pub fn get_second_largest_resolution() -> Option<TestResolution> {
    // Get the primary display resolution
    let (width, height) = get_primary_monitor_resolution();

    // Define all test resolutions in descending order of size
    let resolutions = [
        TestResolution::R3600x1920,
        TestResolution::R3072x1600,
        TestResolution::R2560x1440,
        TestResolution::R2048x1536,
        TestResolution::R1920x1200,
        TestResolution::R1680x1050,
        TestResolution::R1440x1080,
        TestResolution::R1200x960,
        TestResolution::R1080x864,
        TestResolution::R960x800,
        TestResolution::R864x640,
        TestResolution::R800x600,
        TestResolution::R768x576,
        TestResolution::R720x480,
        TestResolution::R640x384,
        TestResolution::R576x360,
        TestResolution::R400x300,
    ];

    // Find the largest resolution that fits
    let mut last_fitting_resolution: Option<TestResolution> = None;
    for resolution in resolutions.iter() {
        if resolution.fits_in(width, height) {
            if let Some(last) = last_fitting_resolution {
                return Some(last); // Return the second-largest fitting resolution
            }
            last_fitting_resolution = Some(*resolution); // Save the largest fitting resolution
        }
    }
    None
}

// GpuType、GpuOcParams、detect_gpu_type、fetch_gpu_type 已迁移至 nvidia_gpu_type.rs
pub use crate::nvidia_gpu_type::*;

pub fn single_gpu<'a>(gpus: &[&'a Gpu]) -> anyhow::Result<&'a Gpu, Error> {
    let mut gpus = gpus.iter();
    gpus.next()
        .ok_or_else(|| Error::from("no GPU selected"))
        .and_then(|g| match gpus.next() {
            None => Ok(*g),
            Some(..) => Err(Error::from("multiple GPUs selected")),
        })
}

fn parse_gpu_id(s: &str) -> anyhow::Result<usize> {
    let s = s.trim();

    // Detect values that look like they came from a single-dash long option,
    // e.g. `-gpu=0` is parsed by clap as short flag `-g` with value `pu=0`.
    // The canonical form is `--gpu=<N>`.
    if let Some(rest) = s.strip_prefix("pu=").or_else(|| s.strip_prefix("pu ")) {
        anyhow::bail!(
            "invalid GPU id {:?} -- did you mean --gpu={}?",
            s,
            rest.trim()
        );
    }
    // Generic guard: reject anything that doesn't start with a digit.
    if !s.starts_with(|c: char| c.is_ascii_digit()) {
        anyhow::bail!(
            "invalid GPU id {:?}: expected a decimal or hex (0x…) number",
            s
        );
    }

    let value = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16).map_err(|_| anyhow::anyhow!("invalid hex GPU id {:?}", s))?
    } else {
        usize::from_str(s).map_err(|_| anyhow::anyhow!("invalid decimal GPU id {:?}", s))?
    };

    Ok(value)
}

pub fn select_gpus<'a>(
    gpus: &'a [Gpu],
    selections: Option<&[String]>,
) -> anyhow::Result<Vec<&'a Gpu>, Error> {
    let selected = match selections {
        Some(values) => {
            let inputs = values
                .iter()
                .map(|s| parse_gpu_id(s.as_str()))
                .collect::<anyhow::Result<Vec<_>, _>>()
                .map_err(|e| Error::Custom(e.to_string()))?;

            let mut result = Vec::new();
            for input in inputs {
                // ① 人类可读序号（强语义，禁止 fallback）
                if input < 256 {
                    if let Some(g) = gpus.get(input) {
                        result.push(g);
                        continue;
                    } else {
                        // index 不存在，直接认为无效
                        continue;
                    }
                }

                // ② 直接 GPU ID（dec / hex）
                if let Some(g) = gpus.iter().find(|g| g.id() == input) {
                    result.push(g);
                    continue;
                }

                // ③ legacy fallback（只允许 input >= 256）
                let legacy = input << 8;
                if let Some(g) = gpus.iter().find(|g| g.id() == legacy) {
                    result.push(g);
                    continue;
                }
            }
            result
        }
        None => gpus.iter().collect(),
    };

    if selected.is_empty() {
        Err(Error::DeviceNotFound)
    } else {
        Ok(selected)
    }
}

pub fn get_sorted_gpus() -> nvapi_hi::Result<Vec<Gpu>> {
    let mut gpus = Gpu::enumerate()?;
    gpus.sort_by_key(|g| g.id());
    Ok(gpus)
}

pub fn get_sorted_gpu_ids_nvml(nvml: &Nvml) -> Result<Vec<u32>, Error> {
    let count = nvml
        .device_count()
        .map_err(|e| Error::Custom(format!("NVML device_count failed: {:?}", e)))?;

    let mut gpu_ids = Vec::new();
    for i in 0..count {
        let device = nvml
            .device_by_index(i)
            .map_err(|e| Error::Custom(format!("NVML device_by_index({}) failed: {:?}", i, e)))?;
        let pci = device
            .pci_info()
            .map_err(|e| Error::Custom(format!("NVML pci_info({}) failed: {:?}", i, e)))?;

        // Keep ID semantics compatible with existing NVML helpers: gpu_id / 256 = PCI bus.
        gpu_ids.push(pci.bus.saturating_mul(256));
    }

    gpu_ids.sort_unstable();
    gpu_ids.dedup();
    Ok(gpu_ids)
}

pub fn select_gpu_ids(
    gpu_ids: &[u32],
    selections: Option<&[String]>,
) -> anyhow::Result<Vec<u32>, Error> {
    let selected = match selections {
        Some(values) => {
            let inputs = values
                .iter()
                .map(|s| parse_gpu_id(s.as_str()))
                .collect::<anyhow::Result<Vec<_>, _>>()
                .map_err(|e| Error::Custom(e.to_string()))?;

            let mut result = Vec::new();
            for input in inputs {
                if input < 256 {
                    if let Some(id) = gpu_ids.get(input) {
                        result.push(*id);
                        continue;
                    } else {
                        continue;
                    }
                }

                if let Some(&id) = gpu_ids.iter().find(|&&id| id as usize == input) {
                    result.push(id);
                    continue;
                }

                let legacy = (input as u32) << 8;
                if let Some(&id) = gpu_ids.iter().find(|&&id| id == legacy) {
                    result.push(id);
                    continue;
                }
            }
            result
        }
        None => gpu_ids.to_vec(),
    };

    if selected.is_empty() {
        Err(Error::DeviceNotFound)
    } else {
        Ok(selected)
    }
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
    // let gpus = crate::custom_wrapper::enumerate_raw_gpus()?;
    // for (gpu, handle) in gpus.iter().enumerate() {
    //     println!("GPU {} raw handle = {:?}", gpu, handle);
    //     let serial = get_board_info_raw(*handle)?;
    //     println!("GPU serial:{}", serial );
    // }
    Ok(())
}

/// Display detailed NVAPI GPU info for pre-selected GPUs.
pub fn handle_info(
    nvml: Option<&Nvml>,
    gpus: &[&Gpu],
    oformat: OutputFormat,
    output_file: Option<&str>,
) -> Result<(), Error> {
    for (i, gpu) in gpus.iter().enumerate() {
        println!("GPU {}: ID:0x{:04X}", i, gpu.id());
    }

    match oformat {
        OutputFormat::Human => {
            let mut success = 0usize;
            for gpu in gpus {
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
                human::print_info(gpu, &info, nvml);
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
                for gpu in gpus {
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
                for gpu in gpus {
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

    Ok(())
}

/// Display basic GPU info using NVML only — used when NVAPI is unavailable.
pub fn handle_info_nvml_only(
    nvml: &Nvml,
    gpu_ids: &[u32],
    oformat: OutputFormat,
) -> Result<(), Error> {
    use nvml_wrapper::enum_wrappers::device::Clock;

    if gpu_ids.is_empty() {
        return Err(Error::DeviceNotFound);
    }

    match oformat {
        OutputFormat::Human => {
            for &gpu_id in gpu_ids {
                let pci_bus = gpu_id / 256;
                let count = nvml
                    .device_count()
                    .map_err(|e| Error::Custom(format!("NVML device_count: {:?}", e)))?;
                for i in 0..count {
                    if let Ok(dev) = nvml.device_by_index(i) {
                        if let Ok(pci) = dev.pci_info() {
                            if pci.bus != pci_bus {
                                continue;
                            }
                            let name = dev.name().unwrap_or_else(|_| "Unknown".into());
                            let uuid = dev.uuid().unwrap_or_else(|_| "Unknown".into());
                            println!("GPU {} (NVML):", gpu_id);
                            println!("  Name          : {}", name);
                            println!("  UUID          : {}", uuid);
                            println!(
                                "  PCI           : {:04x}:{:02x}:{:02x}",
                                pci.domain, pci.bus, pci.device
                            );
                            if let Ok(mem) = dev.memory_info() {
                                println!(
                                    "  Total VRAM    : {} MiB",
                                    mem.total / (1024 * 1024)
                                );
                            }
                            if let Ok(cap) = dev.cuda_compute_capability() {
                                println!("  Compute Cap.  : {}.{}", cap.major, cap.minor);
                            }
                            if let Ok(c) = dev.power_management_limit_constraints() {
                                println!(
                                    "  Power Limits  : {} W – {} W",
                                    c.min_limit / 1000,
                                    c.max_limit / 1000
                                );
                            }
                            if let Ok(clk) = dev.clock(Clock::Graphics, nvml_wrapper::enum_wrappers::device::ClockId::Current) {
                                println!("  Core Clock    : {} MHz", clk);
                            }
                            if let Ok(clk) = dev.clock(Clock::Memory, nvml_wrapper::enum_wrappers::device::ClockId::Current) {
                                println!("  Mem Clock     : {} MHz", clk);
                            }
                            println!();
                        }
                    }
                }
            }
        }
        OutputFormat::Json => {
            use serde_json::{json, Value};
            let mut entries: Vec<Value> = Vec::new();
            let count = nvml
                .device_count()
                .map_err(|e| Error::Custom(format!("NVML device_count: {:?}", e)))?;
            for &gpu_id in gpu_ids {
                let pci_bus = gpu_id / 256;
                for i in 0..count {
                    if let Ok(dev) = nvml.device_by_index(i) {
                        if let Ok(pci) = dev.pci_info() {
                            if pci.bus != pci_bus {
                                continue;
                            }
                            let entry = json!({
                                "gpu_id": gpu_id,
                                "name": dev.name().ok(),
                                "uuid": dev.uuid().ok(),
                                "pci_bus": pci.bus,
                                "total_vram_mib": dev.memory_info().ok().map(|m| m.total / (1024*1024)),
                            });
                            entries.push(entry);
                        }
                    }
                }
            }
            serde_json::to_writer_pretty(io::stdout(), &entries)?;
        }
    }
    Ok(())
}

pub fn handle_status(
    nvml: Option<&Nvml>,
    gpus: &[&Gpu],
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
        match oformat {
            OutputFormat::Human => {
                let mut shown = false;
                for gpu in gpus.iter() {
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
                    if let (Ok(info), Some(n)) = (gpu.info(), nvml) {
                        if let Some(thresholds) =
                            crate::oc_get_set_function_nvml::get_nvml_temperature_thresholds(
                                n,
                                info.id as u32,
                            )
                        {
                            println!("NVML Temperature Thresholds:");
                            for (name, value) in thresholds {
                                match value {
                                    Some(temp) => println!("  {:<16} : {} C", name, temp),
                                    None => println!("  {:<16} : N/A", name),
                                }
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
                for gpu in gpus.iter() {
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

        if let Some(monitor) = monitor {
            sleep(monitor)
        } else {
            break;
        }
    }

    Ok(())
}

/// Display GPU status using NVML only — used when NVAPI is unavailable.
pub fn handle_status_nvml_only(
    nvml: &Nvml,
    gpu_ids: &[u32],
    matches: &ArgMatches,
    oformat: OutputFormat,
) -> Result<(), Error> {
    use nvml_wrapper::enum_wrappers::device::{Clock, ClockId, TemperatureSensor};

    const NANOS_IN_SECOND: f64 = 1e9;

    let monitor = matches
        .get_one::<String>("monitor")
        .map(|s| f64::from_str(s.as_str()))
        .transpose()?
        .map(|v| Duration::new(v as u64, (v.fract() * NANOS_IN_SECOND) as u32));

    if gpu_ids.is_empty() {
        return Err(Error::DeviceNotFound);
    }

    loop {
        let count = nvml
            .device_count()
            .map_err(|e| Error::Custom(format!("NVML device_count: {:?}", e)))?;

        match oformat {
            OutputFormat::Human => {
                for &gpu_id in gpu_ids {
                    let pci_bus = gpu_id / 256;
                    for i in 0..count {
                        if let Ok(dev) = nvml.device_by_index(i) {
                            if let Ok(pci) = dev.pci_info() {
                                if pci.bus != pci_bus {
                                    continue;
                                }
                                let name = dev.name().unwrap_or_else(|_| "Unknown".into());
                                println!("GPU {} ({}):", gpu_id, name);
                                if let Ok(t) = dev.temperature(TemperatureSensor::Gpu) {
                                    println!("  Temperature   : {} C", t);
                                }
                                if let Ok(pw) = dev.power_usage() {
                                    println!("  Power Usage   : {:.1} W", pw as f32 / 1000.0);
                                }
                                if let Ok(util) = dev.utilization_rates() {
                                    println!("  GPU Util      : {}%", util.gpu);
                                    println!("  Mem Util      : {}%", util.memory);
                                }
                                if let Ok(clk) = dev.clock(Clock::Graphics, ClockId::Current) {
                                    println!("  Core Clock    : {} MHz", clk);
                                }
                                if let Ok(clk) = dev.clock(Clock::Memory, ClockId::Current) {
                                    println!("  Mem Clock     : {} MHz", clk);
                                }
                                if let Ok(fan) = dev.fan_speed(0) {
                                    println!("  Fan Speed     : {}%", fan);
                                }
                                if let Ok(ps) = dev.performance_state() {
                                    println!("  P-State       : {:?}", ps);
                                }
                                println!();
                            }
                        }
                    }
                }
            }
            OutputFormat::Json => {
                use serde_json::{json, Value};
                let mut entries: Vec<Value> = Vec::new();
                for &gpu_id in gpu_ids {
                    let pci_bus = gpu_id / 256;
                    for i in 0..count {
                        if let Ok(dev) = nvml.device_by_index(i) {
                            if let Ok(pci) = dev.pci_info() {
                                if pci.bus != pci_bus {
                                    continue;
                                }
                                let entry = json!({
                                    "gpu_id": gpu_id,
                                    "name": dev.name().ok(),
                                    "temperature_c": dev.temperature(TemperatureSensor::Gpu).ok(),
                                    "power_usage_w": dev.power_usage().ok().map(|p| p as f32 / 1000.0),
                                    "core_clock_mhz": dev.clock(Clock::Graphics, ClockId::Current).ok(),
                                    "mem_clock_mhz": dev.clock(Clock::Memory, ClockId::Current).ok(),
                                    "fan_speed_pct": dev.fan_speed(0).ok(),
                                });
                                entries.push(entry);
                            }
                        }
                    }
                }
                if monitor.is_some() {
                    let _ = serde_json::to_writer(io::stdout(), &entries);
                    println!();
                } else {
                    let _ = serde_json::to_writer_pretty(io::stdout(), &entries);
                }
            }
        }

        if let Some(d) = monitor {
            sleep(d);
        } else {
            break;
        }
    }

    Ok(())
}

// In commands.rs
pub fn handle_get(
    nvml: Option<&Nvml>,
    gpus: &[&Gpu],
    oformat: OutputFormat,
) -> Result<(), Error> {
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
                if let (Ok(info), Some(n)) = (gpu.info(), nvml) {
                    let gpu_id = info.id as u32;
                    let power_limit =
                        crate::oc_get_set_function_nvml::query_nvml_power_watts(n, gpu_id);
                    let temp_thresholds =
                        crate::oc_get_set_function_nvml::get_nvml_temperature_thresholds(n, gpu_id);
                    let pstate_info = crate::oc_get_set_function_nvml::get_nvml_pstate_info(n, gpu_id);
                    let app_clocks =
                        crate::oc_get_set_function_nvml::get_nvml_supported_applications_clocks(
                            n, gpu_id,
                        );
                    let min_max_fan_speed =
                        crate::oc_get_set_function_nvml::get_nvml_min_max_fan_speed(n, gpu_id);
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
                                let pstate_str = crate::conv::nvml_pstate_to_str(pstate);
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
                                    crate::oc_get_set_function_nvml::get_nvml_core_clock_vf_offset(
                                        n, gpu_id, pstate,
                                    );
                                let mem_offset =
                                    crate::oc_get_set_function_nvml::get_nvml_mem_clock_vf_offset(
                                        n, gpu_id, pstate,
                                    );
                                if let Some(c) = core_offset {
                                    println!("      Core Clock Offset  : {} MHz", c);
                                }
                                if let Some(m) = mem_offset {
                                    println!("      Mem Clock Offset   : {} MHz", m);
                                }
                            }
                        } else {
                            let core_offset =
                                crate::oc_get_set_function_nvml::get_nvml_core_clock_vf_offset(
                                    n,
                                    gpu_id,
                                    nvml_wrapper::enum_wrappers::device::PerformanceState::Zero,
                                );
                            let mem_offset =
                                crate::oc_get_set_function_nvml::get_nvml_mem_clock_vf_offset(
                                    n,
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

// In commands.rs

pub fn handle_reset(
    gpus: &[&Gpu],
    matches: &ArgMatches,
) -> Result<(), Error> {
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
        match (allowable_result(r).map_err(|e| (setting, e))?, explicit) {
            (Err(e), true) => Err((setting, e).into()),
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

pub fn handle_set_command(nvml: &Nvml, gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    match matches.subcommand() {
        Some(("nvapi", sub)) => handle_nvapi(nvml, gpus, sub)?,
        Some(("nvml", sub)) => handle_nvml(nvml, gpus, sub)?,
        Some(("nvml-cooler", sub)) => handle_nvml_cooler(nvml, gpus, sub)?,
        _ => {}
    }
    Ok(())
}

fn handle_nvapi(nvml: &Nvml, gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    if let Some(vboost) = matches
        .get_one::<String>("vboost")
        .map(|s| u32::from_str(s.as_str()))
        .transpose()?
    {
        for gpu in gpus {
            gpu.set_voltage_boost(Percentage(vboost))?;
        }
    }
    if let Some(plimit) = matches.get_many::<String>("plimit") {
        let plimit = plimit
            .map(|s| u32::from_str(s.as_str()))
            .map(|v| v.map(Percentage))
            .collect::<Result<Vec<_>, _>>()?;
        for gpu in gpus {
            gpu.set_power_limits(plimit.iter().cloned())?;
        }
    }
    if let Some(tlimit) = matches.get_many::<String>("tlimit") {
        let tlimit = tlimit
            .map(|s| i32::from_str(s.as_str()))
            .map(|v| v.map(|v| Celsius(v).into()))
            .collect::<Result<Vec<_>, _>>()?;
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

    if let Some(delta_uv) = matches
        .get_one::<String>("voltage_delta")
        .map(|s| i32::from_str(s.as_str()))
        .transpose()?
    {
        for gpu in gpus {
            crate::oc_get_set_function_nvapi::set_pstate_base_voltage(
                gpu,
                MicrovoltsDelta(delta_uv),
                nvapi_pstate,
            )?;
        }
    }

    if let Some(core_offset) = matches
        .get_one::<String>("core_offset")
        .map(|s| i32::from_str(s.as_str()))
        .transpose()?
    {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match gpu.inner().set_pstates(
                [(
                    nvapi_pstate,
                    nvapi_hi::ClockDomain::Graphics,
                    KilohertzDelta(core_offset),
                )]
                .iter()
                .cloned(),
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

    if let Some(mem_offset) = matches
        .get_one::<String>("mem_offset")
        .map(|s| i32::from_str(s.as_str()))
        .transpose()?
    {
        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match gpu.inner().set_pstates(
                [(
                    nvapi_pstate,
                    nvapi_hi::ClockDomain::Memory,
                    KilohertzDelta(mem_offset),
                )]
                .iter()
                .cloned(),
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
        let first_pstate = crate::conv::try_parse_nvml_pstate(requested_pstates[0])?;
        let second_pstate = if requested_pstates.len() >= 2 {
            crate::conv::try_parse_nvml_pstate(requested_pstates[1])?
        } else {
            first_pstate
        };

        for gpu in gpus {
            let gpu_info = gpu.info()?;
            match crate::oc_get_set_function_nvapi::set_nvapi_pstate_lock(
                nvml,
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
        crate::oc_get_set_function_nvapi::handle_lock_vfp(gpus, matches, 0, false)?;
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
            match crate::oc_get_set_function_nvapi::reset_vfp_frequency_lock(
                gpu,
                nvapi_hi::ClockDomain::Graphics,
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
            let gpu_info = gpu.info()?;
            match crate::oc_get_set_function_nvapi::reset_vfp_frequency_lock(
                gpu,
                nvapi_hi::ClockDomain::Memory,
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
        crate::oc_get_set_function_nvapi::handle_test_voltage_limits(gpus, matches)?;
    }

    Ok(())
}

pub fn handle_nvml_with_ids(nvml: &Nvml, gpu_ids: &[u32], matches: &ArgMatches) -> Result<(), Error> {
    let nvml_pstate_val = matches
        .get_one::<String>("pstate")
        .map(|s| s.as_str())
        .unwrap_or("0");
    let target_nvml_pstate = crate::conv::parse_nvml_pstate(nvml_pstate_val);

    if let Some(core_offset) = matches
        .get_one::<String>("core_offset")
        .map(|s| i32::from_str(s.as_str()))
        .transpose()?
    {
        for &gpu_id in gpu_ids {
            match crate::oc_get_set_function_nvml::set_nvml_core_clock_vf_offset(
                nvml,
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

    if let Some(mem_offset) = matches
        .get_one::<String>("mem_offset")
        .map(|s| i32::from_str(s.as_str()))
        .transpose()?
    {
        for &gpu_id in gpu_ids {
            match crate::oc_get_set_function_nvml::set_nvml_mem_clock_vf_offset(
                nvml,
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

    if let Some(power_w) = matches
        .get_one::<String>("power_limit")
        .map(|s| u32::from_str(s.as_str()))
        .transpose()?
    {
        for &gpu_id in gpu_ids {
            match crate::oc_get_set_function_nvml::set_nvml_power_limit(nvml, gpu_id, power_w) {
                Ok(_) => println!(
                    "Successfully applied NVML power limit {} W to GPU {}",
                    power_w, gpu_id
                ),
                Err(e) => eprintln!("Failed to set NVML power limit for GPU {}: {:?}", gpu_id, e),
            }
        }
    }

    if let Some(app_clocks) = matches.get_many::<String>("app_clock") {
        let clocks: Vec<u32> = app_clocks
            .map(|s| u32::from_str(s.as_str()).unwrap_or(0))
            .collect();
        if clocks.len() == 2 {
            let mem_clock = clocks[0];
            let core_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match crate::oc_get_set_function_nvml::set_nvml_applications_clocks(
                    nvml, gpu_id, mem_clock, core_clock,
                ) {
                    Ok(_) => println!(
                        "Successfully applied NVML app clocks (Mem: {}, Core: {}) to GPU {}",
                        mem_clock, core_clock, gpu_id
                    ),
                    Err(e) => {
                        eprintln!("Failed to set NVML app clocks for GPU {}: {:?}", gpu_id, e)
                    }
                }
            }
        } else {
            eprintln!(
                "Invalid arguments for --nvml-app-clock, expected 2 arguments (MEM_MHZ CORE_MHZ)"
            );
        }
    }

    if let Some(locked_core_clocks) = matches.get_many::<String>("locked_core_clocks") {
        let clocks: Vec<u32> = locked_core_clocks
            .map(|s| u32::from_str(s.as_str()).unwrap_or(0))
            .collect();
        if clocks.len() == 2 {
            let min_clock = clocks[0];
            let max_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match crate::oc_get_set_function_nvml::set_nvml_core_locked_clocks(
                    nvml, gpu_id, min_clock, max_clock,
                ) {
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
            match crate::oc_get_set_function_nvml::reset_nvml_core_locked_clocks(nvml, gpu_id) {
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

    if let Some(locked_mem_clocks) = matches.get_many::<String>("locked_mem_clocks") {
        let clocks: Vec<u32> = locked_mem_clocks
            .map(|s| u32::from_str(s.as_str()).unwrap_or(0))
            .collect();
        if clocks.len() == 2 {
            let min_clock = clocks[0];
            let max_clock = clocks[1];
            for &gpu_id in gpu_ids {
                match crate::oc_get_set_function_nvml::set_nvml_mem_locked_clocks(
                    nvml, gpu_id, min_clock, max_clock,
                ) {
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
        let first_pstate = crate::conv::try_parse_nvml_pstate(requested_pstates[0])?;
        let second_pstate = if requested_pstates.len() >= 2 {
            crate::conv::try_parse_nvml_pstate(requested_pstates[1])?
        } else {
            first_pstate
        };

        for &gpu_id in gpu_ids {
            match crate::oc_get_set_function_nvml::set_nvml_pstate_lock(
                nvml,
                gpu_id,
                first_pstate,
                second_pstate,
            ) {
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
            match crate::oc_get_set_function_nvml::reset_nvml_mem_locked_clocks(nvml, gpu_id) {
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

fn handle_nvml(nvml: &Nvml, gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let mut gpu_ids = Vec::with_capacity(gpus.len());
    for gpu in gpus {
        gpu_ids.push(gpu.info()?.id as u32);
    }
    handle_nvml_with_ids(nvml, &gpu_ids, matches)
}

pub fn handle_nvml_cooler_with_ids(nvml: &Nvml, gpu_ids: &[u32], matches: &ArgMatches) -> Result<(), Error> {
    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    let policy = matches
        .get_one::<String>("policy")
        .map(|s| crate::oc_get_set_function_nvml::parse_nvml_fan_control_policy(s.as_str()))
        .transpose()?
        .ok_or_else(|| Error::from("Missing required argument: --policy <MODE>"))?;
    let level = matches
        .get_one::<String>("level")
        .map(|s| u32::from_str(s.as_str()))
        .transpose()?
        .ok_or_else(|| Error::from("Missing required argument: --level <LEVEL>"))?;

    for &gpu_id in gpu_ids {
        let fan_count =
            crate::oc_get_set_function_nvml::get_nvml_num_fans(nvml, gpu_id).ok_or_else(|| {
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
            match crate::oc_get_set_function_nvml::set_fan_speed(nvml, gpu_id, fan_idx, policy, level) {
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

pub fn handle_nvml_cooler(nvml: &Nvml, gpus: &[&Gpu], matches: &ArgMatches) -> Result<(), Error> {
    let mut gpu_ids = Vec::with_capacity(gpus.len());
    for gpu in gpus {
        gpu_ids.push(gpu.info()?.id as u32);
    }
    handle_nvml_cooler_with_ids(nvml, &gpu_ids, matches)
}

pub fn handle_reset_nvml_cooler(
    nvml: &Nvml,
    gpus: &[&Gpu],
    matches: &ArgMatches,
) -> Result<(), Error> {
    let cooler_id = matches
        .get_one::<String>("id")
        .map(|s| s.as_str())
        .unwrap_or("all");

    for gpu in gpus {
        handle_reset_nvml_cooler_single_gpu(nvml, gpu, cooler_id)?;
    }

    Ok(())
}

pub fn handle_reset_nvml_cooler_single_gpu(nvml: &Nvml, gpu: &Gpu, cooler_id: &str) -> Result<(), Error> {
    let gpu_info = gpu.info()?;
    let gpu_id = gpu_info.id as u32;
    let fan_count =
        crate::oc_get_set_function_nvml::get_nvml_num_fans(nvml, gpu_id).ok_or_else(|| {
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
        match crate::oc_get_set_function_nvml::set_default_fan_speed(nvml, gpu_id, fan_idx) {
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
