use std::sync::atomic::{AtomicBool, Ordering};

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::Clock;

static NVML_WARNED: AtomicBool = AtomicBool::new(false);

pub struct PowerSampler {
    start_power_mw: u32,
    nvml: Option<Nvml>,
    device_index: u32,
}

impl PowerSampler {
    pub fn start(device_index: u32) -> Self {
        let nvml = match Nvml::init() {
            Ok(n) => Some(n),
            Err(_) => {
                if !NVML_WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!("[power] NVML not available; power/freq data will be -1");
                }
                None
            }
        };

        let start_power_mw = nvml
            .as_ref()
            .and_then(|n| n.device_by_index(device_index).ok())
            .and_then(|d| d.power_usage().ok())
            .unwrap_or(0);

        Self {
            start_power_mw,
            nvml,
            device_index,
        }
    }

    pub fn stop_and_read(self) -> (i64, i64) {
        let end_power_mw = self
            .nvml
            .as_ref()
            .and_then(|n| n.device_by_index(self.device_index).ok())
            .and_then(|d| d.power_usage().ok())
            .unwrap_or(0);

        if self.start_power_mw == 0 || end_power_mw == 0 {
            return (-1, -1);
        }
        let mean = ((self.start_power_mw + end_power_mw) / 2) as i64;
        let peak = self.start_power_mw.max(end_power_mw) as i64;
        (mean, peak)
    }
}

pub fn read_freq_mhz_once(device_index: u32) -> i64 {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(_) => {
            if !NVML_WARNED.swap(true, Ordering::Relaxed) {
                eprintln!("[power] NVML not available; freq data will be -1");
            }
            return -1;
        }
    };
    let device = match nvml.device_by_index(device_index) {
        Ok(d) => d,
        Err(_) => return -1,
    };
    match device.clock_info(Clock::Graphics) {
        Ok(mhz) => mhz as i64,
        Err(_) => -1,
    }
}
