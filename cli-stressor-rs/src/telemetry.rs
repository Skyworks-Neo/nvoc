//! In-run GPU telemetry via NVML (driver-shipped, dlopen'd at runtime) —
//! sampled on a fixed cadence into the SDC event stream and on demand at
//! fault time, so every recorded SDC hit carries the clock / temperature /
//! power context it happened under. This is the measurement half of the
//! "SDC-rate vs frequency/temperature mapping" the verify config promises.
//!
//! The device is matched by CUDA UUID when available (NVML's device ordering
//! is its own PCI-sorted enumeration and may differ from CUDA's).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// One telemetry snapshot (all fields best-effort: `None` = query failed or
/// unsupported by the SKU).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TelemetrySample {
    pub unix_ms: u64,
    pub graphics_clock_mhz: Option<u32>,
    pub mem_clock_mhz: Option<u32>,
    pub temp_gpu_c: Option<u32>,
    pub power_watts: Option<f32>,
    pub power_limit_watts: Option<f32>,
}

pub struct Telemetry {
    nvml: Option<nvml_wrapper::Nvml>,
    device_index: u32,
    /// Matched by CUDA UUID instead of falling back to NVML ordering.
    uuid_matched: bool,
}

fn uuid_hex_lower(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|b| format!("{:02x}", b)).collect()
}

impl Telemetry {
    /// Open NVML and resolve the device. `cuda_uuid` wins over `device_index`
    /// when the NVML-side UUID matches; otherwise the CUDA-sorted index is
    /// used as-is (correct on single-GPU boxes, best-effort on multi-GPU).
    pub fn open(device_index: u32, cuda_uuid: Option<[u8; 16]>) -> Self {
        let nvml = match nvml_wrapper::Nvml::init() {
            Ok(nvml) => Some(nvml),
            Err(err) => {
                std::println!(
                    "Warning: NVML init failed — SDC events will carry no telemetry: {err}"
                );
                None
            }
        };
        let mut this = Self {
            nvml,
            device_index,
            uuid_matched: false,
        };
        if let (Some(nvml), Some(uuid)) = (this.nvml.as_ref(), cuda_uuid)
            && uuid != [0u8; 16]
            && let Ok(count) = nvml.device_count()
        {
            let hex = uuid_hex_lower(&uuid);
            for idx in 0..count {
                let Ok(dev) = nvml.device_by_index(idx) else {
                    continue;
                };
                let Ok(dev_uuid) = dev.uuid() else { continue };
                if dev_uuid.to_lowercase().contains(&hex) {
                    this.device_index = idx;
                    this.uuid_matched = true;
                    break;
                }
            }
        }
        this
    }

    pub fn available(&self) -> bool {
        self.nvml.is_some()
    }

    /// True when the device was pinned by CUDA UUID (vs NVML index order).
    pub fn uuid_matched(&self) -> bool {
        self.uuid_matched
    }

    /// One best-effort snapshot; `None` when NVML is unavailable.
    pub fn sample(&self) -> Option<TelemetrySample> {
        let nvml = self.nvml.as_ref()?;
        let device = nvml.device_by_index(self.device_index).ok()?;
        use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
        let graphics_clock_mhz = device.clock_info(Clock::Graphics).ok();
        let mem_clock_mhz = device.clock_info(Clock::Memory).ok();
        let temp_gpu_c = device.temperature(TemperatureSensor::Gpu).ok();
        // NVML reports power in milliwatts.
        let power_watts = device.power_usage().ok().map(|mw| mw as f32 / 1000.0);
        let power_limit_watts = device
            .power_management_limit()
            .ok()
            .map(|mw| mw as f32 / 1000.0);
        Some(TelemetrySample {
            unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            graphics_clock_mhz,
            mem_clock_mhz,
            temp_gpu_c,
            power_watts,
            power_limit_watts,
        })
    }
}
