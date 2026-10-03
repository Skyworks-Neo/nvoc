//! Monitor/OC extension types shared between the backend and the HTTP API.

use serde::Serialize;

/// Backend selector for clock offsets (GUI parity): each backend exposes
/// its own interface and they are never mixed.
/// - NVML: `SetClockOffset` / `QueryClockOffset` (P0, MHz).
/// - NVAPI: private ClkDomains `SetControl`/`GetControl`
///   (bit 0 = graphics / 2 = memory, slot 0 = signed kHz offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffsetBackend {
    #[default]
    Nvml,
    Nvapi,
}

impl OffsetBackend {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nvml" => Some(Self::Nvml),
            "nvapi" => Some(Self::Nvapi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nvml => "nvml",
            Self::Nvapi => "nvapi",
        }
    }
}

/// Domain selector for clock offsets (core = graphics, mem = memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetDomain {
    Core,
    Mem,
}

impl std::fmt::Display for OffsetDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl OffsetDomain {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "core" | "graphics" => Some(Self::Core),
            "mem" | "memory" => Some(Self::Mem),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Mem => "mem",
        }
    }
}

/// Per-GPU monitor sample for the dashboard history and `/api/status`.
/// Every field is best-effort: absent sensors stay `None`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitorSample {
    /// GPU utilization % (NVML).
    pub util_pct: Option<f32>,
    /// Core voltage mV (VoltRails P0 rail).
    pub volt_mv: Option<f32>,
    /// Memory clock MHz.
    pub mem_clock_mhz: Option<f32>,
    /// P-state name ("P0"…"P8").
    pub pstate: Option<String>,
    /// Board power draw W (NVML).
    pub power_w: Option<f32>,
    /// Core clock MHz.
    pub core_clock_mhz: Option<f32>,
    /// Core temperature °C.
    pub temp_c: Option<f32>,
    /// Fan duty %.
    pub fan_pct: Option<u32>,
}
