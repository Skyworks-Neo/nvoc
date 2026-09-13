//! Layered runtime configuration for the control service.
//!
//! Precedence (lowest → highest): built-in defaults → TOML config file →
//! CLI flag overrides → HTTP runtime mutations. The file is read once at
//! startup; HTTP mutations are live-only and are not written back, so a
//! restart reverts to file + defaults (documented breaking-change vs. the
//! old `NVOCServiceConfig`, which had no file layer at all).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;

/// Loopback HTTP control-plane port (unchanged from the legacy service).
pub const DEFAULT_PORT: u16 = 14514;
pub const DEFAULT_INTERVAL_MS: u64 = 1000;
pub const INTERVAL_MS_MIN: u64 = 200;
pub const INTERVAL_MS_MAX: u64 = 10_000;
pub const TARGET_C_MIN: f32 = 30.0;
pub const TARGET_C_MAX: f32 = 110.0;

/// Which temperature reading drives the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SensorKind {
    /// GPU core (legacy `GetThermalSettings` TARGET_GPU).
    #[default]
    #[serde(rename = "core")]
    Core,
    /// Hotspot (ThermChannel GPU_MAX); falls back to core when the channel
    /// pair is unavailable (pre-Pascal).
    #[serde(rename = "hotspot")]
    Hotspot,
    /// VRAM (legacy TARGET_MEMORY).
    #[serde(rename = "memory")]
    Memory,
    /// Board ambient (legacy TARGET_BOARD).
    #[serde(rename = "board")]
    Board,
    /// Maximum of every available reading.
    #[serde(rename = "max")]
    Max,
}

impl SensorKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "core" => Some(Self::Core),
            "hotspot" => Some(Self::Hotspot),
            "memory" => Some(Self::Memory),
            "board" => Some(Self::Board),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// Which GPUs the service controls: `"all"` or an explicit index list.
/// (De)serialization is hand-written: `"all"` | `[0,1]` | `"0,1"`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GpuSelection {
    #[default]
    All,
    Indices(Vec<usize>),
}

impl Serialize for GpuSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_str("all"),
            Self::Indices(v) => v.serialize(serializer),
        }
    }
}

struct GpuSelectionVisitor;

impl<'de> serde::de::Visitor<'de> for GpuSelectionVisitor {
    type Value = GpuSelection;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"all\" or a list of GPU indices")
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
        if s.eq_ignore_ascii_case("all") {
            return Ok(GpuSelection::All);
        }
        // Comma-separated indices: "0,1"
        let mut out = Vec::new();
        for part in s.split(',') {
            out.push(
                part.trim()
                    .parse::<usize>()
                    .map_err(|_| E::custom(format!("invalid GPU index {part:?}")))?,
            );
        }
        Ok(GpuSelection::Indices(out))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(i) = seq.next_element::<usize>()? {
            out.push(i);
        }
        Ok(GpuSelection::Indices(out))
    }
}

impl<'de> Deserialize<'de> for GpuSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(GpuSelectionVisitor)
    }
}

/// Startup control mode. `Pid` is the default because closed-loop control is
/// the service's purpose; `/mode?value=auto` hands the fan back to the
/// driver's own curve at any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ControlMode {
    /// Driver fan curve (no writes; controller only observes).
    #[serde(rename = "auto")]
    Auto,
    /// Closed-loop PID fan control against `pid.target_c`.
    #[serde(rename = "pid")]
    #[default]
    Pid,
    /// Pinned duty (`manual_percent`) until the mode changes.
    #[serde(rename = "manual")]
    Manual,
}

/// PID gains and output limits. Output (fan %) = `base_percent` + P + I + D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PidParams {
    /// Control setpoint (°C).
    pub target_c: f32,
    pub kp: f32,
    pub ki: f32,
    pub kd: f32,
    /// Feed-forward duty the P/I/D terms act around — set this near the
    /// steady-state fan % your card needs at typical load; a good feed
    /// forward keeps the integral small and tuning forgiving.
    pub base_percent: f32,
    pub min_percent: f32,
    pub max_percent: f32,
    /// At `target_c + emergency_delta_c` the controller forces 100% duty
    /// regardless of PID state (last-resort overtemp response, with 2 °C
    /// hysteresis on the exit side).
    pub emergency_delta_c: f32,
    /// Mirror of the emergency zone on the cold side: at
    /// `target_c − idle_delta_c` the controller forces `min_percent` duty
    /// (0 % = fan stopped) instead of letting a stale feed-forward keep
    /// spinning the fan. The PID keeps stepping through both zones, so
    /// leaving either one is bumpless. `0` disables the idle zone.
    pub idle_delta_c: f32,
    /// Learn `base_percent` online: the controller continuously re-centers
    /// integral authority into the feed-forward (output-continuous, so it
    /// adds no loop dynamics), and the base decays while the idle zone
    /// holds — together these track the load level in both directions
    /// without configuration. The configured `base_percent` acts as the
    /// initial value; requires `ki > 0` (the integral is the teacher).
    pub adaptive_base: bool,
    /// Anti-chatter deadband: skip a fan write while the PID output sits
    /// within ±`write_deadband_percent` of the last written duty. Quantized
    /// duty writes are a relay nonlinearity — too little deadband turns
    /// sensor quantization into a fast self-excited duty cycle. `0` writes
    /// on every output change.
    pub write_deadband_percent: f32,
}

impl Default for PidParams {
    fn default() -> Self {
        Self {
            target_c: 75.0,
            kp: 2.0,
            ki: 0.05,
            kd: 0.0,
            base_percent: 40.0,
            min_percent: 0.0,
            max_percent: 100.0,
            emergency_delta_c: 12.0,
            idle_delta_c: 4.0,
            write_deadband_percent: 1.0,
            adaptive_base: true,
        }
    }
}

impl PidParams {
    pub fn validate(&self) -> Result<(), String> {
        if !(TARGET_C_MIN..=TARGET_C_MAX).contains(&self.target_c) {
            return Err(format!(
                "target_c must be {TARGET_C_MIN}–{TARGET_C_MAX} °C, got {}",
                self.target_c
            ));
        }
        for (name, v) in [("kp", self.kp), ("ki", self.ki), ("kd", self.kd)] {
            if !(v.is_finite()) || v < 0.0 {
                return Err(format!("{name} must be finite and >= 0, got {v}"));
            }
        }
        if self.kp > 50.0 || self.ki > 10.0 || self.kd > 50.0 {
            return Err("kp ≤ 50, ki ≤ 10, kd ≤ 50 (sanity bounds)".to_string());
        }
        if !(0.0..=100.0).contains(&self.base_percent) {
            return Err(format!(
                "base_percent must be 0–100, got {}",
                self.base_percent
            ));
        }
        if !(0.0..=100.0).contains(&self.min_percent)
            || !(0.0..=100.0).contains(&self.max_percent)
            || self.min_percent > self.max_percent
        {
            return Err("0 ≤ min_percent ≤ max_percent ≤ 100 required".to_string());
        }
        if !(3.0..=50.0).contains(&self.emergency_delta_c) {
            return Err(format!(
                "emergency_delta_c must be 3–50 °C, got {}",
                self.emergency_delta_c
            ));
        }
        if !(0.0..=50.0).contains(&self.idle_delta_c) {
            return Err(format!(
                "idle_delta_c must be 0–50 °C (0 disables the idle zone), got {}",
                self.idle_delta_c
            ));
        }
        if !(0.0..=10.0).contains(&self.write_deadband_percent) {
            return Err(format!(
                "write_deadband_percent must be 0–10, got {}",
                self.write_deadband_percent
            ));
        }
        Ok(())
    }
}

/// Full service configuration (one TOML document).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Loopback HTTP control-plane port.
    pub port: u16,
    /// Control tick period; also the PID dt.
    pub interval_ms: u64,
    /// Sensor the PID controls against.
    pub sensor: SensorKind,
    /// GPUs under control.
    pub gpus: GpuSelection,
    /// Startup control mode.
    pub mode: ControlMode,
    /// Pinned duty for `ControlMode::Manual`.
    pub manual_percent: u32,
    pub pid: PidParams,
    /// Consecutive sensor read failures before the controller restores
    /// driver fan control (failsafe against a pinned-stuck fan).
    pub read_fail_reset: u32,
    /// Heartbeat staleness that trips the watchdog restore (seconds).
    pub watchdog_timeout_s: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            interval_ms: DEFAULT_INTERVAL_MS,
            sensor: SensorKind::default(),
            gpus: GpuSelection::default(),
            mode: ControlMode::default(),
            manual_percent: 50,
            pid: PidParams::default(),
            read_fail_reset: 3,
            watchdog_timeout_s: 30,
        }
    }
}

pub fn default_config_path() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("nvoc")
            .join("nvoc-srv.toml")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/nvoc/nvoc-srv.toml")
    }
}

/// Parse a TOML document into a full config (defaults fill any omission).
pub fn parse_toml(s: &str) -> Result<RuntimeConfig, String> {
    let cfg: RuntimeConfig = toml::from_str(s).map_err(|e| format!("config parse error: {e}"))?;
    cfg.validate()?;
    Ok(cfg)
}

/// Load the config file at `path` if present; a missing file is not an error
/// (defaults apply). Parse/validation failures are hard errors.
pub fn load_file(path: &std::path::Path) -> Result<RuntimeConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse_toml(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RuntimeConfig::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

impl RuntimeConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.pid.validate()?;
        if self.port == 0 {
            return Err("port must be non-zero".to_string());
        }
        if !(INTERVAL_MS_MIN..=INTERVAL_MS_MAX).contains(&self.interval_ms) {
            return Err(format!(
                "interval_ms must be {INTERVAL_MS_MIN}–{INTERVAL_MS_MAX}, got {}",
                self.interval_ms
            ));
        }
        if self.manual_percent > 100 {
            return Err("manual_percent must be 0–100".to_string());
        }
        if self.read_fail_reset == 0 || self.read_fail_reset > 100 {
            return Err("read_fail_reset must be 1–100".to_string());
        }
        if !(5..=600).contains(&self.watchdog_timeout_s) {
            return Err("watchdog_timeout_s must be 5–600".to_string());
        }
        Ok(())
    }

    /// Resolve the selection against the discovered GPU count.
    pub fn selected_indices(&self, count: usize) -> Vec<usize> {
        match &self.gpus {
            GpuSelection::All => (0..count).collect(),
            GpuSelection::Indices(v) => v.iter().copied().filter(|&i| i < count).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.mode, ControlMode::Pid);
        cfg.validate().expect("defaults must validate");
        cfg.pid.validate().expect("default pid must validate");
    }

    #[test]
    fn toml_partial_document_fills_defaults() {
        let cfg = parse_toml("[pid]\ntarget_c = 68.0\n").expect("parse");
        assert_eq!(cfg.pid.target_c, 68.0);
        assert_eq!(cfg.pid.kp, PidParams::default().kp);
        assert_eq!(cfg.sensor, SensorKind::Core);
        assert_eq!(cfg.mode, ControlMode::Pid);
    }

    #[test]
    fn toml_rejects_unknown_fields() {
        assert!(parse_toml("no_such_key = 1\n").is_err());
    }

    #[test]
    fn toml_rejects_out_of_range_target() {
        assert!(parse_toml("[pid]\ntarget_c = 200.0\n").is_err());
    }

    #[test]
    fn gpu_selection_shapes() {
        let all: GpuSelection = serde_json::from_str("\"all\"").expect("str all");
        assert_eq!(all, GpuSelection::All);
        let list: GpuSelection = serde_json::from_str("[0,2]").expect("seq");
        assert_eq!(list, GpuSelection::Indices(vec![0, 2]));
        let csv: GpuSelection = serde_json::from_str("\"0, 2\"").expect("csv str");
        assert_eq!(csv, GpuSelection::Indices(vec![0, 2]));
        let cfg = RuntimeConfig {
            gpus: list,
            ..RuntimeConfig::default()
        };
        assert_eq!(cfg.selected_indices(3), vec![0, 2]);
        // Out-of-range indices are dropped, in-range ones kept.
        assert_eq!(cfg.selected_indices(1), vec![0]);
    }

    #[test]
    fn pid_validation_bounds() {
        let p = PidParams {
            min_percent: 90.0,
            max_percent: 10.0,
            ..PidParams::default()
        };
        assert!(p.validate().is_err());
        let p = PidParams {
            kp: -1.0,
            ..PidParams::default()
        };
        assert!(p.validate().is_err());
    }
}
