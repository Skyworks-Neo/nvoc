//! Per-GPU thermal closed-loop controller: mode state machine, forced
//! zones (overtemp / undertemp), failsafe handling, and write deadbanding
//! around the raw PID.
//!
//! The controller is backend-agnostic ([`ControlBackend`]) so the whole
//! state machine is unit-testable without a GPU.

use crate::config::{ControlMode, FreqParams, LoopKind, PidParams, RuntimeConfig, SensorKind};
use crate::monitor::{OffsetBackend, OffsetDomain};
use crate::pid::{PidController, PidTerms};
use serde::Serialize;

/// The subset of loop parameters the shared PID/zones machinery consumes.
/// Implemented by both [`PidParams`] (fan loop) and [`FreqParams`]
/// (frequency loops) so the controller is loop-agnostic; delta units differ
/// per loop kind (°C for temp loops, W for the power loop).
pub trait LoopParams {
    fn target(&self) -> f32;
    fn kp(&self) -> f32;
    fn ki(&self) -> f32;
    fn kd(&self) -> f32;
    fn base_percent(&self) -> f32;
    fn min_percent(&self) -> f32;
    fn max_percent(&self) -> f32;
    fn idle_delta(&self) -> f32;
    fn emergency_delta(&self) -> f32;
    fn adaptive(&self) -> bool;
    fn write_deadband_percent(&self) -> f32;
}

impl LoopParams for PidParams {
    fn target(&self) -> f32 {
        self.target_c
    }
    fn kp(&self) -> f32 {
        self.kp
    }
    fn ki(&self) -> f32 {
        self.ki
    }
    fn kd(&self) -> f32 {
        self.kd
    }
    fn base_percent(&self) -> f32 {
        self.base_percent
    }
    fn min_percent(&self) -> f32 {
        self.min_percent
    }
    fn max_percent(&self) -> f32 {
        self.max_percent
    }
    fn idle_delta(&self) -> f32 {
        self.idle_delta_c
    }
    fn emergency_delta(&self) -> f32 {
        self.emergency_delta_c
    }
    fn adaptive(&self) -> bool {
        self.adaptive_base
    }
    fn write_deadband_percent(&self) -> f32 {
        self.write_deadband_percent
    }
}

impl LoopParams for FreqParams {
    fn target(&self) -> f32 {
        self.target
    }
    fn kp(&self) -> f32 {
        self.kp
    }
    fn ki(&self) -> f32 {
        self.ki
    }
    fn kd(&self) -> f32 {
        self.kd
    }
    fn base_percent(&self) -> f32 {
        self.base_percent
    }
    fn min_percent(&self) -> f32 {
        self.min_percent
    }
    fn max_percent(&self) -> f32 {
        self.max_percent
    }
    fn idle_delta(&self) -> f32 {
        self.idle_delta
    }
    fn emergency_delta(&self) -> f32 {
        self.emergency_delta
    }
    fn adaptive(&self) -> bool {
        self.adaptive_base
    }
    fn write_deadband_percent(&self) -> f32 {
        self.write_deadband_percent
    }
}

/// Extra cooling below (emergency) / above (idle) a forced-zone entry line
/// required to leave the zone; prevents flapping right at the threshold.
const ZONE_HYSTERESIS_C: f32 = 2.0;

// Adaptive feed-forward (`adaptive_base`): constants rather than knobs —
// the learning rate only has to be slow relative to the loop, not exact.
/// Fraction of the integral term re-centered into the base per tick.
const BASE_ABSORB_FRACTION: f32 = 0.25;
/// Do not bother re-centering tiny integrals.
const BASE_ABSORB_FLOOR: f32 = 0.5;
/// Per-tick cap on the absorbed duty (%).
const BASE_ABSORB_MAX: f32 = 2.0;

/// One GPU's sensor readings, from `QueryNvapiThermalSettings` (legacy
/// core/memory/board view) plus the ThermChannel hotspot when available.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SensorBundle {
    #[serde(rename = "core_c")]
    pub core_c: Option<f32>,
    #[serde(rename = "hotspot_c")]
    pub hotspot_c: Option<f32>,
    #[serde(rename = "memory_c")]
    pub memory_c: Option<f32>,
    #[serde(rename = "board_c")]
    pub board_c: Option<f32>,
}

impl SensorBundle {
    /// The reading `kind` controls against; `Hotspot` degrades to core.
    pub fn pick(&self, kind: SensorKind) -> Option<f32> {
        let candidates: [Option<f32>; 4] =
            [self.core_c, self.hotspot_c, self.memory_c, self.board_c];
        match kind {
            SensorKind::Core => self.core_c,
            SensorKind::Hotspot => self.hotspot_c.or(self.core_c),
            SensorKind::Memory => self.memory_c,
            SensorKind::Board => self.board_c,
            SensorKind::Max => candidates
                .into_iter()
                .flatten()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        }
    }
}

/// Best-effort live fan reading (NVML duty %).
#[derive(Debug, Clone, Copy, Default)]
pub struct FanReading {
    pub percent: Option<u32>,
}

/// Hardware boundary behind the controller. Implemented by the NVAPI(+NVML
/// fallback) backend in [`crate::backend`] and by in-test mocks.
pub trait ControlBackend: Send {
    fn read_temps(&mut self, gpu_index: usize) -> Result<SensorBundle, String>;
    fn read_fan(&mut self, gpu_index: usize) -> Option<FanReading>;
    /// Live board power draw (W), best effort — the `freq_power` loop sensor.
    fn read_power_watts(&mut self, gpu_index: usize) -> Option<f32>;
    /// Live core clock (MHz), best effort — `/status` readback for freq loops.
    fn read_core_clock_mhz(&mut self, gpu_index: usize) -> Option<f32>;
    /// Frequency ceiling (MHz) of the V/F table's current plane (P0 boost
    /// point, offset-inclusive). None = card does not expose the table.
    fn read_freq_ceiling_mhz(&mut self, gpu_index: usize) -> Option<f32>;

    /// Dashboard monitor sample (best effort, every field optional):
    /// utilization %, core voltage mV, memory clock MHz, P-state name.
    fn read_monitor(&mut self, gpu_index: usize) -> Result<crate::monitor::MonitorSample, String>;
    /// GPU identity/info snapshot as JSON (About page).
    fn read_gpu_info_json(&mut self, gpu_index: usize) -> Result<serde_json::Value, String>;
    /// V/F curve points (voltage uV, frequency MHz), ascending voltage.
    fn read_vf_curve(&mut self, gpu_index: usize) -> Result<Vec<(f32, f32)>, String>;
    /// Current core/mem offset (MHz), for the OC page readback — queried
    /// from the selected backend's own surface.
    fn read_offset_mhz(
        &mut self,
        gpu_index: usize,
        domain: OffsetDomain,
        backend: OffsetBackend,
    ) -> Result<i32, String>;
    /// Power-limit window (min, current, max) in watts, for slider bounds.
    fn read_power_limit_w(&mut self, gpu_index: usize) -> Result<Option<(u32, u32, u32)>, String>;
    /// Temperature-wall window (min, current, max) in °C, best effort.
    fn read_temp_limit_c(&mut self, gpu_index: usize) -> Result<Option<(i32, i32, i32)>, String>;
    fn write_offset_mhz(
        &mut self,
        gpu_index: usize,
        domain: OffsetDomain,
        backend: OffsetBackend,
        mhz: i32,
    ) -> Result<(), String>;
    fn write_power_limit_w(&mut self, gpu_index: usize, watts: u32) -> Result<(), String>;
    fn write_temp_limit_c(&mut self, gpu_index: usize, celsius: i32) -> Result<(), String>;
    fn reset_offset(&mut self, gpu_index: usize, domain: OffsetDomain) -> Result<(), String>;
    fn reset_power_limit(&mut self, gpu_index: usize) -> Result<(), String>;
    fn reset_temp_limit(&mut self, gpu_index: usize) -> Result<(), String>;
    fn write_fan_percent(&mut self, gpu_index: usize, percent: u32) -> Result<(), String>;
    /// Frequency soft wall: lock the graphics clock range to `0..cap_khz`
    /// (one-directional — boost may run anywhere at or below the cap).
    fn write_freq_cap_khz(&mut self, gpu_index: usize, cap_khz: u32) -> Result<(), String>;
    /// Hand fan control back to the driver (undoes any pin).
    fn restore_fan_auto(&mut self, gpu_index: usize) -> Result<(), String>;
    /// Clear the frequency lock (undoes the soft wall).
    fn restore_freq_auto(&mut self, gpu_index: usize) -> Result<(), String>;
}

/// Control states that override the raw PID output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailsafeState {
    /// Normal operation.
    Ok,
    /// `read_fail_reset` consecutive sensor failures — fan handed back to
    /// the driver; resumes automatically on the first good read.
    ReadFailures,
    /// Overtemp: `target_c + emergency_delta_c` reached — 100% duty forced
    /// (exits `ZONE_HYSTERESIS_C` below the entry line).
    Emergency,
    /// Undertemp: `target_c − idle_delta_c` reached — `min_percent` duty
    /// forced (fan stop at the default floor), so a stale feed-forward
    /// cannot keep the fan spinning when the target is far above (exits
    /// `ZONE_HYSTERESIS_C` above the entry line).
    IdleHold,
    /// A driver control hand-back write failed; retried on the next tick.
    RestoreFailed,
}

/// Snapshot for the HTTP `/status` endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct GpuControlStatus {
    pub index: usize,
    pub name: String,
    pub mode: ControlMode,
    pub failsafe: FailsafeState,
    /// Sensor reading the controller acted on this tick (°C).
    pub temp_c: Option<f32>,
    pub sensors: SensorBundle,
    /// Last duty we wrote (None = driver is in control).
    pub fan_written_percent: Option<u32>,
    /// Frequency-loop loop: the cap we last wrote (MHz). None on the fan loop.
    pub cap_mhz: Option<u32>,
    /// Live core clock (MHz), freq loops only.
    pub core_clock_mhz: Option<f32>,
    /// Live duty reported by NVML, best effort.
    pub fan_measured_percent: Option<u32>,
    pub pid: Option<PidTerms>,
    pub last_error: Option<String>,
}

/// One GPU's control state. Config (mode + params) is re-read every tick so
/// HTTP mutations apply within one interval.
#[derive(Debug)]
pub struct GpuController {
    pid: PidController,
    loop_kind: LoopKind,
    applied_mode: ControlMode,
    last_written: Option<u32>,
    failsafe: FailsafeState,
    fail_streak: u32,
    emergency_active: bool,
    idle_active: bool,
    /// Adaptive feed-forward state: the learned base duty. Only honored
    /// while the active loop's `adaptive_base` is on (seeded from config).
    base: f32,
    base_seeded: bool,
    /// PID decomposition of the most recent evaluation (None while the PID
    /// did not run this tick: Auto/Manual modes, pre-first-read).
    last_pid: Option<PidTerms>,
    last_error: Option<String>,
    last_sensors: SensorBundle,
    last_temp: Option<f32>,
    /// Frequency-loop state: the cap we last wrote (MHz) and the live core
    /// clock readback.
    last_cap_mhz: Option<u32>,
    core_clock_mhz: Option<f32>,
    /// Frequency ceiling in effect for the cap mapping (MHz): the detected
    /// V/F-table maximum raised by anything observed running faster.
    ceiling_mhz: Option<f32>,
    fan_measured: Option<u32>,
}

impl GpuController {
    pub fn new(pid: PidController, loop_kind: LoopKind) -> Self {
        Self {
            pid,
            loop_kind,
            applied_mode: ControlMode::Auto,
            last_written: None,
            failsafe: FailsafeState::Ok,
            fail_streak: 0,
            emergency_active: false,
            idle_active: false,
            base: 0.0,
            base_seeded: false,
            last_pid: None,
            last_error: None,
            last_sensors: SensorBundle::default(),
            last_temp: None,
            last_cap_mhz: None,
            core_clock_mhz: None,
            ceiling_mhz: None,
            fan_measured: None,
        }
    }

    fn params<'a>(&self, cfg: &'a RuntimeConfig) -> &'a dyn LoopParams {
        match self.loop_kind {
            LoopKind::FanTemp => &cfg.pid,
            LoopKind::FreqTemp | LoopKind::FreqPower => &cfg.freq,
        }
    }

    /// Pull tunables from the config snapshot; preserves PID state.
    pub fn sync_params(&mut self, cfg: &RuntimeConfig) {
        let p = self.params(cfg);
        self.pid.set_target_c(p.target());
        self.pid.set_gains(p.kp(), p.ki(), p.kd());
        self.pid.set_limits(p.min_percent(), p.max_percent());
        // Base ownership: config-owned when fixed, controller-owned (learned)
        // once adaptive — seeded from the config on the first tick.
        if p.adaptive() && self.base_seeded {
            self.pid.set_base_percent(self.base);
        } else {
            self.base = p.base_percent();
            self.base_seeded = true;
            self.pid.set_base_percent(self.base);
        }
    }

    /// Restore driver control unconditionally (shutdown/restore paths).
    pub fn restore(&mut self, index: usize, backend: &mut dyn ControlBackend) {
        if self.last_written.is_some() || self.applied_mode != ControlMode::Auto {
            let result = match self.loop_kind {
                LoopKind::FanTemp => backend.restore_fan_auto(index),
                LoopKind::FreqTemp | LoopKind::FreqPower => backend.restore_freq_auto(index),
            };
            match result {
                Ok(()) => log::info!("GPU {index}: control restored to driver"),
                Err(e) => {
                    log::error!("GPU {index}: driver control restore FAILED: {e}");
                    self.failsafe = FailsafeState::RestoreFailed;
                    self.last_error = Some(e);
                }
            }
        }
        self.last_written = None;
        self.applied_mode = ControlMode::Auto;
        self.emergency_active = false;
        self.idle_active = false;
    }

    /// One control tick; returns the status snapshot for `/status`.
    pub fn tick(
        &mut self,
        index: usize,
        name: &str,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        dt_s: f32,
    ) -> GpuControlStatus {
        self.handle_loop_switch(index, cfg, backend);
        self.handle_mode_transition(index, cfg, backend);
        self.sync_params(cfg);
        // Populated only by a PID evaluation this tick (below).
        self.last_pid = None;

        match self.applied_mode {
            ControlMode::Auto => {
                // Driver owns the fan; keep observing for /status.
                self.observe(index, backend);
            }
            ControlMode::Manual => self.tick_manual(index, cfg, backend),
            ControlMode::Pid => self.tick_pid(index, cfg, backend, dt_s),
        }

        self.fan_measured = backend.read_fan(index).and_then(|f| f.percent);
        self.core_clock_mhz = match self.loop_kind {
            LoopKind::FanTemp => None,
            LoopKind::FreqTemp | LoopKind::FreqPower => backend.read_core_clock_mhz(index),
        };
        GpuControlStatus {
            index,
            name: name.to_string(),
            mode: self.applied_mode,
            failsafe: self.failsafe,
            temp_c: self.last_temp,
            sensors: self.last_sensors.clone(),
            fan_written_percent: self.last_written,
            cap_mhz: self.last_cap_mhz,
            core_clock_mhz: self.core_clock_mhz,
            fan_measured_percent: self.fan_measured,
            pid: self.last_pid,
            last_error: self.last_error.clone(),
        }
    }

    /// Actuator hand-over when the loop kind changes: undo the old loop's
    /// pin/wall and reset the loop state (the plant is different).
    fn handle_loop_switch(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
    ) {
        if cfg.loop_kind == self.loop_kind {
            return;
        }
        match self.loop_kind {
            LoopKind::FanTemp => {
                let _ = backend.restore_fan_auto(index);
            }
            LoopKind::FreqTemp | LoopKind::FreqPower => {
                let _ = backend.restore_freq_auto(index);
            }
        }
        log::info!(
            "GPU {index}: control loop switched {:?} → {:?}",
            self.loop_kind,
            cfg.loop_kind
        );
        self.loop_kind = cfg.loop_kind;
        self.last_written = None;
        self.pid.reset();
        self.emergency_active = false;
        self.idle_active = false;
    }

    fn handle_mode_transition(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
    ) {
        if cfg.mode == self.applied_mode {
            return;
        }
        match cfg.mode {
            ControlMode::Auto => self.restore(index, backend),
            ControlMode::Pid => {
                self.pid.reset();
                log::info!(
                    "GPU {index}: PID mode engaged (target {})",
                    self.params(cfg).target()
                );
            }
            ControlMode::Manual => {
                self.pid.reset();
                log::info!("GPU {index}: manual mode ({}%)", cfg.manual_percent);
            }
        }
        self.applied_mode = cfg.mode;
        self.emergency_active = false;
        self.idle_active = false;
        self.fail_streak = 0;
        if self.failsafe == FailsafeState::ReadFailures {
            self.failsafe = FailsafeState::Ok;
        }
    }

    fn tick_manual(&mut self, index: usize, cfg: &RuntimeConfig, backend: &mut dyn ControlBackend) {
        match self.loop_kind {
            LoopKind::FanTemp => {
                let target = cfg.manual_percent.min(100);
                if self.last_written != Some(target) {
                    match backend.write_fan_percent(index, target) {
                        Ok(()) => {
                            self.last_written = Some(target);
                            self.last_error = None;
                        }
                        Err(e) => {
                            log::error!("GPU {index}: manual fan write {}% failed: {e}", target);
                            self.last_error = Some(e);
                        }
                    }
                }
            }
            LoopKind::FreqTemp | LoopKind::FreqPower => {
                // Pin the frequency cap: `manual_percent` is restriction effort.
                let effort = cfg.manual_percent.min(100);
                self.refresh_ceiling(index, cfg, backend);
                let Some(ceiling) = self.ceiling_mhz else {
                    return; // ceiling not yet known — never guess
                };
                let cap_khz = self.cap_khz_with(ceiling, cfg, effort as f32);
                if self.last_written != Some(effort) {
                    match backend.write_freq_cap_khz(index, cap_khz) {
                        Ok(()) => {
                            self.last_written = Some(effort);
                            self.last_cap_mhz = Some(cap_khz / 1000);
                            self.last_error = None;
                        }
                        Err(e) => {
                            log::error!("GPU {index}: manual freq-cap write failed: {e}");
                            self.last_error = Some(e);
                        }
                    }
                }
            }
        }
        self.observe(index, backend);
    }

    fn tick_pid(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        dt_s: f32,
    ) {
        // Loop-sensor read per kind; `freq_power` additionally observes the
        // temperature (best effort) for the overtemp guard and /status.
        let reading: Result<f32, String> = match cfg.loop_kind {
            LoopKind::FanTemp | LoopKind::FreqTemp => match backend.read_temps(index) {
                Ok(bundle) => {
                    let value = bundle
                        .pick(cfg.sensor)
                        .ok_or_else(|| "selected sensor absent".to_string());
                    self.last_sensors = bundle;
                    value
                }
                Err(e) => Err(e),
            },
            LoopKind::FreqPower => {
                let value = backend
                    .read_power_watts(index)
                    .ok_or_else(|| "power read failed".to_string());
                // Guard temperature: best effort, never fails the loop.
                if let Ok(bundle) = backend.read_temps(index) {
                    self.last_sensors = bundle;
                }
                value
            }
        };
        match reading {
            Err(e) => {
                self.fail_streak += 1;
                self.last_error = Some(e);
                if self.fail_streak >= cfg.read_fail_reset.max(1) {
                    if self.last_written.is_some() {
                        // Hand the actuator back to the driver and keep
                        // re-trying while the pin/wall is ours: the driver
                        // may be mid-TDR (calls fail, then recover), and a
                        // stuck override during that window is the worst
                        // case for a stress run.
                        log::error!(
                            "GPU {index}: {} consecutive sensor read failures; \
                             restoring driver control",
                            self.fail_streak
                        );
                        let restore = match cfg.loop_kind {
                            LoopKind::FanTemp => backend.restore_fan_auto(index),
                            LoopKind::FreqTemp | LoopKind::FreqPower => {
                                backend.restore_freq_auto(index)
                            }
                        };
                        match restore {
                            Ok(()) => {
                                self.last_written = None;
                                self.failsafe = FailsafeState::ReadFailures;
                            }
                            Err(re) => {
                                log::error!(
                                    "GPU {index}: failsafe restore failed: {re}; will retry"
                                );
                                self.failsafe = FailsafeState::RestoreFailed;
                            }
                        }
                        self.emergency_active = false;
                        self.idle_active = false;
                    } else if self.failsafe == FailsafeState::Ok {
                        // Nothing pinned — surface the degraded read state.
                        self.failsafe = FailsafeState::ReadFailures;
                    }
                }
            }
            Ok(value) => {
                self.fail_streak = 0;
                if self.failsafe == FailsafeState::ReadFailures {
                    log::info!("GPU {index}: sensor recovered; PID resumes");
                    self.failsafe = FailsafeState::Ok;
                    self.pid.reset();
                }
                match cfg.loop_kind {
                    LoopKind::FanTemp => {
                        self.last_temp = Some(value);
                        self.apply_pid_output(index, cfg, backend, value, dt_s);
                    }
                    LoopKind::FreqTemp | LoopKind::FreqPower => {
                        self.last_temp = self.last_sensors.pick(cfg.sensor);
                        self.apply_freq_output(index, cfg, backend, value, dt_s);
                    }
                }
            }
        }
    }

    fn apply_pid_output(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        temp: f32,
        dt_s: f32,
    ) {
        let p = self.params(cfg);
        let mut terms = self.pid.step(temp, dt_s);
        self.last_pid = Some(terms);
        if p.adaptive() {
            self.learn_base(cfg);
        }
        // Forced zones override the output while the PID keeps stepping —
        // its anti-windup freezes the integral at the rail, so the exit is
        // bumpless and the /status terms stay live.
        if let Some(forced) = self.update_zones(index, p, temp, "°C", false) {
            terms.output_percent = forced;
            self.last_pid = Some(terms);
            self.commit_fan(index, cfg, backend, forced, temp);
        } else {
            self.commit_fan(index, cfg, backend, terms.output_percent, temp);
        }
    }

    /// Frequency-lock loops: same PID/zones in effort space, then the effort
    /// maps to a frequency cap (`max_mhz` fully open … `min_mhz` deepest).
    fn apply_freq_output(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        value: f32,
        dt_s: f32,
    ) {
        let p = self.params(cfg);
        let unit = match self.loop_kind {
            LoopKind::FreqPower => "W",
            _ => "°C",
        };
        let guard = self.temp_guard_active(cfg);
        let mut terms = self.pid.step(value, dt_s);
        self.last_pid = Some(terms);
        if p.adaptive() {
            self.learn_base(cfg);
        }
        if let Some(forced) = self.update_zones(index, p, value, unit, guard) {
            terms.output_percent = forced;
            self.last_pid = Some(terms);
            self.commit_freq(index, cfg, backend, forced);
        } else {
            self.commit_freq(index, cfg, backend, terms.output_percent);
        }
    }

    /// The `freq_power` loop is blind to temperature by construction, so an
    /// overtemp guard overrides it: core temp ≥ `temp_guard_c` forces the
    /// deepest cap. Latched while the temperature stays above the guard.
    fn temp_guard_active(&self, cfg: &RuntimeConfig) -> bool {
        if cfg.loop_kind != LoopKind::FreqPower || cfg.freq.temp_guard_c <= 0.0 {
            return false;
        }
        self.last_temp.is_some_and(|t| t >= cfg.freq.temp_guard_c)
    }

    /// Forced-zone latches with 2-unit exit hysteresis, shared by every loop
    /// kind. The PID keeps stepping through both zones — its conditional
    /// anti-windup freezes the integral at whatever value makes the raw
    /// output sit at the forced rail, so leaving a zone is bumpless (no
    /// reset, no blast). Returns the forced effort while a zone (or the
    /// freq_power temperature guard) holds, else `None`.
    fn update_zones(
        &mut self,
        index: usize,
        p: &dyn LoopParams,
        value: f32,
        unit: &str,
        guard: bool,
    ) -> Option<f32> {
        let emergency_at = p.target() + p.emergency_delta();
        let idle_at = p.target() - p.idle_delta();
        if self.emergency_active {
            if !guard && value <= emergency_at - ZONE_HYSTERESIS_C {
                log::info!("GPU {index}: left emergency zone ({value:.1} {unit})");
                self.emergency_active = false;
            }
        } else if value >= emergency_at {
            log::warn!(
                "GPU {index}: {value:.1} {unit} ≥ emergency line {emergency_at:.1} {unit}; \
                 forcing maximum effort"
            );
            self.emergency_active = true;
        }
        if self.idle_active {
            if value >= idle_at + ZONE_HYSTERESIS_C {
                log::info!("GPU {index}: left idle zone ({value:.1} {unit}); PID resumes");
                self.idle_active = false;
            }
        } else if p.idle_delta() > 0.0 && value <= idle_at {
            log::info!(
                "GPU {index}: {value:.1} {unit} ≤ idle line {idle_at:.1} {unit}; \
                 forcing {}% (minimum effort)",
                p.min_percent()
            );
            self.idle_active = true;
        }
        if self.emergency_active {
            self.failsafe = FailsafeState::Emergency;
            return Some(100.0);
        }
        if self.idle_active {
            self.failsafe = FailsafeState::IdleHold;
            return Some(p.min_percent());
        }
        // The freq_power temperature guard forces the deepest cap — it is a
        // safety override, not a tuned state.
        if guard {
            self.failsafe = FailsafeState::Emergency;
            return Some(100.0);
        }
        // Clear only our own zone flags; ReadFailures/RestoreFailed are
        // owned by the failure paths and must not be clobbered here.
        if self.failsafe == FailsafeState::Emergency || self.failsafe == FailsafeState::IdleHold {
            self.failsafe = FailsafeState::Ok;
        }
        None
    }

    fn commit_fan(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        output: f32,
        temp: f32,
    ) {
        let duty = output.round().clamp(0.0, 100.0) as u32;
        let inside_deadband = match self.last_written {
            None => false,
            Some(last) => (output - last as f32).abs() < self.params(cfg).write_deadband_percent(),
        };
        if !inside_deadband {
            match backend.write_fan_percent(index, duty) {
                Ok(()) => {
                    self.last_written = Some(duty);
                    self.last_error = None;
                    log::info!("GPU {index}: fan duty {duty}% (temp {temp:.1} °C)");
                }
                Err(e) => {
                    log::error!("GPU {index}: fan write {duty}% failed: {e}");
                    self.last_error = Some(e);
                }
            }
        }
    }

    fn commit_freq(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        effort: f32,
    ) {
        self.refresh_ceiling(index, cfg, backend);
        let Some(ceiling) = self.ceiling_mhz else {
            // Ceiling not yet known (first tick, no detection and no
            // observation): never guess — an unwritten cap is the correct
            // "unknown" state; the next tick writes it.
            return;
        };
        let cap_khz = self.cap_khz_with(ceiling, cfg, effort);
        // Deadband measures the *cap* delta as a fraction of the ceiling —
        // effort alone is not enough since a moving ceiling shifts the cap
        // without any effort change (e.g. a freshly observed overclock).
        // Must be evaluated BEFORE last_cap_mhz takes the new value.
        let inside_deadband = match self.last_cap_mhz {
            Some(last_cap) => {
                ((cap_khz / 1000) as f32 - last_cap as f32).abs() / ceiling * 100.0
                    < self.params(cfg).write_deadband_percent()
            }
            None => false,
        };
        self.last_cap_mhz = Some(cap_khz / 1000);
        if !inside_deadband {
            match backend.write_freq_cap_khz(index, cap_khz) {
                Ok(()) => {
                    self.last_written = Some(effort.round() as u32);
                    self.last_error = None;
                    log::info!(
                        "GPU {index}: freq cap {} MHz (effort {effort:.0}%)",
                        cap_khz / 1000
                    );
                }
                Err(e) => {
                    log::error!("GPU {index}: freq cap write failed: {e}");
                    self.last_error = Some(e);
                }
            }
        }
    }

    /// Map restriction effort (0 = cap fully open, 100 = deepest cap) to a
    /// frequency cap in kHz.
    /// Resolve the frequency ceiling the cap maps against, per tick:
    /// - `freq.max_mhz > 0` pins it (manual override);
    /// - otherwise the detected V/F-table maximum (current plane, offset
    ///   inclusive), refreshed every tick and **raised** by anything
    ///   observed running faster — an external OC tool or a config change
    ///   lifts the ceiling without restart, and removing the overclock
    ///   lets it fall back on the next detection.
    fn refresh_ceiling(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
    ) {
        if cfg.freq.max_mhz > 0 {
            self.ceiling_mhz = Some(cfg.freq.max_mhz as f32);
            return;
        }
        let detected = backend.read_freq_ceiling_mhz(index);
        let observed = self.core_clock_mhz;
        let merged = match (detected, observed) {
            (Some(d), Some(o)) => Some(d.max(o)),
            (d, o) => d.or(o),
        };
        match (merged, self.ceiling_mhz) {
            (Some(v), _) => self.ceiling_mhz = Some(v),
            // First ticks with neither detection nor observation: keep the
            // previous ceiling if any, else defer (cap stays unwritten).
            (None, prev) => {
                self.ceiling_mhz = prev;
            }
        }
    }

    fn cap_khz_with(&self, ceiling: f32, cfg: &RuntimeConfig, effort: f32) -> u32 {
        // Effort 0 maps to the ceiling (cap fully open), effort 100 to
        // min_mhz (deepest cap).
        let floor = cfg.freq.min_mhz as f32;
        let cap_mhz = ceiling - effort / 100.0 * (ceiling - floor);
        (cap_mhz * 1000.0).round() as u32
    }

    /// Adaptive feed-forward: continuously re-center integral authority into
    /// `self.base`. The transfer is output-continuous (base moves by exactly
    /// what the integral counter-moves), so learning adds no loop dynamics —
    /// it only re-partitions state, which is how the base tracks the load
    /// level in both directions without configuration. Requires `ki > 0`.
    fn learn_base(&mut self, cfg: &RuntimeConfig) {
        let p = self.params(cfg);
        let i_term = self.pid.i_term();
        if i_term.abs() < BASE_ABSORB_FLOOR {
            return;
        }
        let delta = (BASE_ABSORB_FRACTION * i_term).clamp(-BASE_ABSORB_MAX, BASE_ABSORB_MAX);
        let new_base = (self.base + delta).clamp(p.min_percent(), p.max_percent());
        let delta = new_base - self.base;
        if delta == 0.0 {
            return;
        }
        self.base = new_base;
        self.pid.absorb_integral(delta);
    }

    /// Sensor/fan refresh without control action (Auto mode).
    fn observe(&mut self, index: usize, backend: &mut dyn ControlBackend) {
        if let Ok(bundle) = backend.read_temps(index) {
            self.last_sensors = bundle;
            self.last_temp = self.last_sensors.pick(SensorKind::Core);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FreqParams, GpuSelection, LoopKind as LK, RuntimeConfig};
    use crate::monitor::MonitorSample;

    /// Scriptable fake hardware.
    struct Mock {
        /// Temps returned per read_temps call (popped; last one repeats).
        temps: Vec<Result<SensorBundle, String>>,
        writes: Vec<u32>,
        restores: usize,
        fail_writes: bool,
        /// restore_fan_auto fails this many times before succeeding.
        restore_fails: u32,
        /// Power draw reported by read_power_watts.
        power: Option<f32>,
        /// Frequency caps written by write_freq_cap_khz (MHz).
        freq_caps: Vec<u32>,
        freq_restores: usize,
        /// V/F-table ceiling reported by read_freq_ceiling_mhz.
        ceiling: Option<f32>,
        /// Live core clock reported by read_core_clock_mhz (overrides the
        /// cap-derived default when set).
        clock: Option<f32>,
    }

    impl Mock {
        fn steady(temp: f32) -> Self {
            Self {
                temps: vec![Ok(SensorBundle {
                    core_c: Some(temp),
                    ..Default::default()
                })],
                writes: Vec::new(),
                restores: 0,
                fail_writes: false,
                restore_fails: 0,
                power: None,
                freq_caps: Vec::new(),
                freq_restores: 0,
                ceiling: None,
                clock: None,
            }
        }
    }

    impl ControlBackend for Mock {
        fn read_temps(&mut self, _i: usize) -> Result<SensorBundle, String> {
            if !self.temps.is_empty() {
                if self.temps.len() > 1 {
                    return self.temps.remove(0);
                }
                return self.temps[0].clone();
            }
            Err("no script".to_string())
        }
        fn read_fan(&mut self, _i: usize) -> Option<FanReading> {
            None
        }
        fn read_power_watts(&mut self, _i: usize) -> Option<f32> {
            self.power
        }
        fn read_core_clock_mhz(&mut self, _i: usize) -> Option<f32> {
            self.clock
                .or_else(|| self.freq_caps.last().map(|c| *c as f32 / 1000.0))
        }
        fn read_freq_ceiling_mhz(&mut self, _i: usize) -> Option<f32> {
            self.ceiling
        }
        fn write_fan_percent(&mut self, _i: usize, p: u32) -> Result<(), String> {
            if self.fail_writes {
                return Err("write rejected".to_string());
            }
            self.writes.push(p);
            Ok(())
        }
        fn write_freq_cap_khz(&mut self, _i: usize, cap_khz: u32) -> Result<(), String> {
            if self.fail_writes {
                return Err("write rejected".to_string());
            }
            self.freq_caps.push(cap_khz / 1000);
            self.writes.push(cap_khz / 1000);
            Ok(())
        }
        fn restore_fan_auto(&mut self, _i: usize) -> Result<(), String> {
            if self.restore_fails > 0 {
                self.restore_fails -= 1;
                return Err("restore rejected".to_string());
            }
            self.restores += 1;
            Ok(())
        }
        fn read_monitor(&mut self, _i: usize) -> Result<MonitorSample, String> {
            Ok(MonitorSample {
                power_w: self.power,
                ..MonitorSample::default()
            })
        }
        fn read_gpu_info_json(&mut self, _i: usize) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({ "name": "mock" }))
        }
        fn read_vf_curve(&mut self, _i: usize) -> Result<Vec<(f32, f32)>, String> {
            Ok(Vec::new())
        }
        fn read_offset_mhz(
            &mut self,
            _i: usize,
            _d: OffsetDomain,
            _b: OffsetBackend,
        ) -> Result<i32, String> {
            Ok(0)
        }
        fn read_power_limit_w(&mut self, _i: usize) -> Result<Option<(u32, u32, u32)>, String> {
            Ok(None)
        }
        fn read_temp_limit_c(&mut self, _i: usize) -> Result<Option<(i32, i32, i32)>, String> {
            Ok(None)
        }
        fn write_offset_mhz(
            &mut self,
            _i: usize,
            _d: OffsetDomain,
            _b: OffsetBackend,
            _mhz: i32,
        ) -> Result<(), String> {
            Ok(())
        }
        fn write_power_limit_w(&mut self, _i: usize, _w: u32) -> Result<(), String> {
            Ok(())
        }
        fn write_temp_limit_c(&mut self, _i: usize, _c: i32) -> Result<(), String> {
            Ok(())
        }
        fn reset_offset(&mut self, _i: usize, _d: OffsetDomain) -> Result<(), String> {
            Ok(())
        }
        fn reset_power_limit(&mut self, _i: usize) -> Result<(), String> {
            Ok(())
        }
        fn reset_temp_limit(&mut self, _i: usize) -> Result<(), String> {
            Ok(())
        }
        fn restore_freq_auto(&mut self, _i: usize) -> Result<(), String> {
            self.freq_restores += 1;
            Ok(())
        }
    }

    fn cfg(mode: ControlMode) -> RuntimeConfig {
        RuntimeConfig {
            mode,
            ..RuntimeConfig::default()
        }
    }

    fn tick(c: &mut GpuController, m: &mut Mock, config: &RuntimeConfig) -> GpuControlStatus {
        c.tick(0, "mock", config, m, 1.0)
    }

    #[test]
    fn pid_mode_writes_rising_duty_for_hot_temp() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock::steady(85.0);
        let mut config = cfg(ControlMode::Pid);
        config.pid.ki = 0.0; // no integral creep: out = 40 + 2·10 = 60 exactly
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(m.writes, vec![60]);
        assert_eq!(st.fan_written_percent, Some(60));
        assert_eq!(st.failsafe, FailsafeState::Ok);
    }

    #[test]
    fn deadband_skips_unchanged_writes() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock::steady(85.0);
        let mut config = cfg(ControlMode::Pid);
        config.pid.ki = 0.0; // constant plant + pure PD → constant duty
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        assert_eq!(m.writes, vec![60], "constant plant → single write");
    }

    #[test]
    fn manual_mode_pins_once_and_restore_hands_back() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock::steady(60.0);
        let mut config = cfg(ControlMode::Manual);
        config.manual_percent = 55;
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        assert_eq!(m.writes, vec![55]);
        config.mode = ControlMode::Auto;
        tick(&mut c, &mut m, &config);
        assert_eq!(m.restores, 1);
        assert_eq!(c.last_written, None);
    }

    #[test]
    fn emergency_forces_max_with_hysteresis_and_bumpless_exit() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        // default emergency line: 75 + 12 = 87; exit at 85.
        let mut m = Mock::steady(88.0);
        let config = cfg(ControlMode::Pid);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.writes, vec![100]);
        // Still inside the hysteresis band: stays at 100 (no rewrite).
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(86.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.writes, vec![100]);
        // Below the exit line: the PID resumes WITHOUT a reset — the
        // integral froze at the rail, so the exit output is continuous
        // (40 + 2·9 + i(0.65+0.45) ≈ 60.1), not a drop to 58.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(84.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Ok);
        let out = st.pid.expect("pid ran").output_percent;
        assert!(
            (58.0..62.0).contains(&out),
            "bumpless exit near 60, got {out}"
        );
    }

    #[test]
    fn idle_zone_forces_min_and_exit_outputs_decay() {
        // Stale high base (adaptive off): a load drop must still reach the
        // fan-stop floor. The stale base unwinds across repeated idle-zone
        // visits — the integral winds negative while the zone holds and the
        // exit output decays (50 → 44 → …) instead of re-firing the old 62.
        let mut config = cfg(ControlMode::Pid);
        config.pid.adaptive_base = false;
        config.pid.ki = 0.4;
        config.pid.base_percent = 60.0;
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock {
            temps: vec![
                Ok(SensorBundle {
                    core_c: Some(76.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(65.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(62.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(73.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(62.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(73.0),
                    ..Default::default()
                }),
            ],
            writes: Vec::new(),
            restores: 0,
            fail_writes: false,
            restore_fails: 0,
            power: None,
            freq_caps: Vec::new(),
            freq_restores: 0,
            ceiling: None,
            clock: None,
        };
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(m.writes, vec![62]); // 60 + 2·1 + 0.4·1
        let st = tick(&mut c, &mut m, &config); // 65 ≤ 71 → zone
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        assert_eq!(m.writes.last(), Some(&0));
        let st = tick(&mut c, &mut m, &config); // still in zone (62 ≤ 73? no: hold until ≥73)
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        assert_eq!(m.writes.len(), 2, "min held by the deadband");
        let st = tick(&mut c, &mut m, &config); // 73 ≥ 71+2 → exit
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(m.writes.last(), Some(&50));
        let st = tick(&mut c, &mut m, &config); // over-cooled → zone again
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        assert_eq!(m.writes.last(), Some(&0));
        let st = tick(&mut c, &mut m, &config); // exit again
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(
            m.writes.last(),
            Some(&44),
            "exit output decays cycle over cycle"
        );
    }

    #[test]
    fn idle_zone_disabled_at_zero() {
        let mut config = cfg(ControlMode::Pid);
        config.pid.adaptive_base = false;
        config.pid.ki = 0.0;
        config.pid.idle_delta_c = 0.0;
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(60.0);
        for _ in 0..3 {
            let st = tick(&mut c, &mut m, &config);
            assert_eq!(
                st.failsafe,
                FailsafeState::Ok,
                "idle_delta_c=0 disables the zone"
            );
        }
        assert_eq!(m.writes, vec![10]); // 40 + 2·(60−75), deadband holds it
    }

    #[test]
    fn read_failures_trip_failsafe_then_recover() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut config = cfg(ControlMode::Pid);
        config.read_fail_reset = 3;
        let mut m = Mock {
            temps: vec![Err("sensor gone".into())],
            writes: vec![50],
            restores: 0,
            fail_writes: false,
            restore_fails: 0,
            power: None,
            freq_caps: Vec::new(),
            freq_restores: 0,
            ceiling: None,
            clock: None,
        };
        // Seed: pretend a duty is currently pinned (restore must fire).
        c.last_written = Some(50);
        for _ in 0..2 {
            let st = tick(&mut c, &mut m, &config);
            assert_eq!(st.failsafe, FailsafeState::Ok);
        }
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::ReadFailures);
        assert_eq!(m.restores, 1, "driver control restored after 3 failures");
        assert_eq!(c.last_written, None);
        // Recovery: first good read clears the failsafe and PID re-engages.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(76.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(m.writes.last(), Some(&42)); // 40 + 2·1
    }

    #[test]
    fn failsafe_restore_is_retried_until_it_succeeds() {
        // The worst stress-run state is a stuck pin behind a mid-TDR driver:
        // the hand-back must surface as RestoreFailed and keep retrying
        // every tick until the driver accepts it.
        let mut config = cfg(ControlMode::Pid);
        config.read_fail_reset = 1;
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock {
            temps: vec![Err("sensor gone".into())],
            writes: Vec::new(),
            restores: 0,
            fail_writes: false,
            restore_fails: 2,
            power: None,
            freq_caps: Vec::new(),
            freq_restores: 0,
            ceiling: None,
            clock: None,
        };
        c.last_written = Some(50);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::RestoreFailed);
        assert_eq!(c.last_written, Some(50), "pin must stay flagged as ours");
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::RestoreFailed);
        let st = tick(&mut c, &mut m, &config); // driver accepts the restore now
        assert_eq!(st.failsafe, FailsafeState::ReadFailures);
        assert_eq!(c.last_written, None);
        assert_eq!(m.restores, 1);
    }

    #[test]
    fn auto_mode_never_writes() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock::steady(90.0);
        let st = tick(&mut c, &mut m, &cfg(ControlMode::Auto));
        assert!(m.writes.is_empty());
        assert_eq!(st.fan_written_percent, None);
        assert_eq!(st.temp_c, Some(90.0), "Auto still observes for /status");
    }

    #[test]
    fn write_failure_is_reported_not_fatal() {
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            LoopKind::FanTemp,
        );
        let mut m = Mock::steady(85.0);
        m.fail_writes = true;
        let st = tick(&mut c, &mut m, &cfg(ControlMode::Pid));
        assert_eq!(st.last_error.as_deref(), Some("write rejected"));
        assert_eq!(st.fan_written_percent, None);
    }

    #[test]
    fn write_deadband_gates_slow_output_drift() {
        // kp=0, ki=0.4, constant e=+2 → output ramps 0.8 %/tick.
        // (Gains go through the config: sync_params re-applies them per tick.)
        let mut config = cfg(ControlMode::Pid);
        config.pid.kp = 0.0;
        config.pid.ki = 0.4;
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(77.0);
        config.pid.write_deadband_percent = 1.0;
        for _ in 0..4 {
            tick(&mut c, &mut m, &config);
        }
        assert_eq!(
            m.writes,
            vec![41, 42, 43],
            "deadband 1 skips the 0.6 % drift"
        );

        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(77.0);
        config.pid.write_deadband_percent = 0.25;
        for _ in 0..4 {
            tick(&mut c, &mut m, &config);
        }
        assert_eq!(
            m.writes,
            vec![41, 42, 42, 43],
            "narrow deadband follows the ramp"
        );
    }

    #[test]
    fn adaptive_base_learns_the_load_level() {
        // kp=0, ki=0.4: the integral alone carries the load level.
        let mut config = cfg(ControlMode::Pid);
        config.pid.kp = 0.0;
        config.pid.ki = 0.4;
        config.pid.base_percent = 40.0;
        // 3 hot ticks build the integral; then the loop settles near target.
        let mut m = Mock {
            temps: vec![
                Ok(SensorBundle {
                    core_c: Some(78.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(78.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(78.0),
                    ..Default::default()
                }),
                Ok(SensorBundle {
                    core_c: Some(75.2),
                    ..Default::default()
                }),
            ],
            writes: Vec::new(),
            restores: 0,
            fail_writes: false,
            restore_fails: 0,
            power: None,
            freq_caps: Vec::new(),
            freq_restores: 0,
            ceiling: None,
            clock: None,
        };
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        for _ in 0..3 {
            tick(&mut c, &mut m, &config);
        }
        let i_after_hot = c.last_pid.expect("pid ran").i;
        assert!(i_after_hot > 1.0, "hot phase must load the integral");

        // Settled ticks: absorption re-centers the integral into the base
        // while the output stays constant (base + i invariant).
        for _ in 0..12 {
            tick(&mut c, &mut m, &config);
        }
        let terms = c.last_pid.expect("pid ran");
        assert!(
            terms.base_percent > 43.0,
            "base learned upward, got {}",
            terms.base_percent
        );
        assert!(terms.i.abs() < 1.0, "integral re-centered, got {}", terms.i);
        // Output kept constant throughout learning: only the initial
        // approach writes moved the duty.
        let duty = c.last_written.expect("a duty was written");
        assert_eq!(m.writes.last(), Some(&duty));
        assert_eq!(duty, 44); // round(40 + 0.4·(9+0.2·N)) ≈ 44, written once

        // A config base_percent change is ignored while adaptive: the
        // learned value is controller-owned.
        config.pid.base_percent = 10.0;
        let st = tick(&mut c, &mut m, &config);
        assert!(st.pid.expect("pid ran").base_percent > 43.0);
    }

    #[test]
    fn adaptive_base_decays_in_the_idle_zone() {
        // The idle zone is evidence of over-delivery: the learned base must
        // fall while the zone holds, so the exit does not re-fire the fan.
        let mut config = cfg(ControlMode::Pid);
        config.pid.kp = 0.0;
        config.pid.ki = 0.4;
        config.pid.base_percent = 60.0;
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(65.0); // ≤ target−4 → idle zone
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        let base_enter = st.pid.expect("pid ran").base_percent;
        for _ in 0..10 {
            tick(&mut c, &mut m, &config);
        }
        let base_after = c.last_pid.expect("pid ran").base_percent;
        assert!(
            base_after < base_enter - 10.0,
            "base must decay in the idle zone: {base_enter} → {base_after}"
        );
    }

    #[test]
    fn fixed_base_tracks_config_changes() {
        let mut config = cfg(ControlMode::Pid);
        config.pid.adaptive_base = false;
        config.pid.kp = 0.0;
        config.pid.ki = 0.4;
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(75.0);
        let st = tick(&mut c, &mut m, &config);
        assert!((st.pid.expect("pid ran").base_percent - 40.0).abs() < 1e-4);

        config.pid.base_percent = 55.0;
        let st = tick(&mut c, &mut m, &config);
        assert!((st.pid.expect("pid ran").base_percent - 55.0).abs() < 1e-4);
    }

    fn freq_power_cfg() -> RuntimeConfig {
        let mut config = cfg(ControlMode::Pid);
        config.loop_kind = LK::FreqPower;
        config.freq = FreqParams {
            target: 150.0,
            kp: 1.0,
            ki: 0.0,
            kd: 0.0,
            base_percent: 0.0,
            min_percent: 0.0,
            max_percent: 100.0,
            idle_delta: 30.0,
            emergency_delta: 30.0,
            temp_guard_c: 0.0,
            min_mhz: 300,
            max_mhz: 2100,
            adaptive_base: false,
            write_deadband_percent: 1.0,
        };
        config
    }

    #[test]
    fn freq_power_loop_caps_the_clock_on_overpower() {
        let config = freq_power_cfg();
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(70.0);
        m.power = Some(170.0);
        let st = tick(&mut c, &mut m, &config);
        // error 20 W, kp 1 → effort 20 % → cap = 2100 − 0.20·1800 = 1740
        assert_eq!(m.freq_caps, vec![1740]);
        assert_eq!(st.cap_mhz, Some(1740));
        assert_eq!(st.failsafe, FailsafeState::Ok);

        // Overpower past the emergency line: deepest cap.
        m.power = Some(185.0);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.freq_caps.last(), Some(&300));

        // Light load below the idle band: idle zone, cap fully open.
        m.power = Some(100.0);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        assert_eq!(m.freq_caps.last(), Some(&2100));
    }

    #[test]
    fn freq_power_temp_guard_overrides_the_power_loop() {
        let mut config = freq_power_cfg();
        config.freq.temp_guard_c = 88.0;
        config.freq.idle_delta = 0.0; // keep the idle zone out of the way
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        // Power at target, but temperature past the guard: deepest cap.
        let mut m = Mock::steady(90.0);
        m.power = Some(150.0);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.freq_caps.last(), Some(&300));
        // Guard clears with 2 °C hysteresis (88 − 2 = 86): PID resumes.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(85.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Ok);
        // error = 150 − 150 = 0 → effort 0 → cap open
        assert_eq!(m.freq_caps.last(), Some(&2100));
    }

    #[test]
    fn freq_temp_loop_caps_on_overtemp() {
        let mut config = cfg(ControlMode::Pid);
        config.loop_kind = LK::FreqTemp;
        config.freq.target = 75.0;
        config.freq.kp = 2.0;
        config.freq.ki = 0.0;
        config.freq.max_mhz = 2100; // pin: expectations below are in this space
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(80.0);
        let st = tick(&mut c, &mut m, &config);
        // error 5 °C, kp 2 → effort 10 % → cap 1920 MHz
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(m.freq_caps, vec![1920]);

        // Below the idle line: cap fully open (no restriction while cool).
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(60.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::IdleHold);
        assert_eq!(m.freq_caps.last(), Some(&2100));
    }

    fn freq_auto_cfg() -> RuntimeConfig {
        let mut config = freq_power_cfg();
        config.freq.max_mhz = 0; // auto ceiling
        config
    }

    #[test]
    fn freq_auto_ceiling_follows_the_detected_vf_maximum() {
        // A 40/50-class card: V/F max 2790, not the legacy 2100 constant.
        let config = freq_auto_cfg();
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(70.0);
        m.power = Some(170.0);
        m.ceiling = Some(2790.0);
        let st = tick(&mut c, &mut m, &config);
        // error 20 W, kp 1 -> effort 20 % -> cap = 2790 - 0.20*2490 = 2292
        assert_eq!(m.freq_caps, vec![2292]);
        assert_eq!(st.cap_mhz, Some(2292));
    }

    #[test]
    fn freq_auto_ceiling_is_raised_by_observed_overclock() {
        // External OC pushes the card past the detected table maximum: the
        // ceiling must follow, or the mapping would saturate.
        let config = freq_auto_cfg();
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(70.0);
        m.power = Some(170.0);
        m.ceiling = Some(2790.0);
        m.clock = Some(3120.0); // overclocked, running above the table
        // Tick 1: observation lags by one tick — ceiling = detected 2790.
        let _ = tick(&mut c, &mut m, &config);
        assert_eq!(m.freq_caps, vec![2292]);
        // Tick 2: the raised ceiling lands — cap = 3120 - 0.20*2820 = 2556.
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(m.freq_caps, vec![2292, 2556]);
        assert_eq!(st.cap_mhz, Some(2556));
    }

    #[test]
    fn freq_auto_ceiling_falls_back_to_observation() {
        // Cards without a readable V/F table: the observed clock becomes
        // the ceiling (restriction then works relative to where the card
        // actually runs).
        let config = freq_auto_cfg();
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(70.0);
        m.power = Some(170.0);
        m.ceiling = None;
        m.clock = Some(2640.0);
        // Tick 1: no detection, observation not yet sampled — no cap write.
        let _ = tick(&mut c, &mut m, &config);
        assert!(m.freq_caps.is_empty(), "must not guess a ceiling");
        // Tick 2: observed 2640 becomes the ceiling; cap = 2640 - 0.20*2340.
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(m.freq_caps, vec![2172]);
        assert_eq!(st.cap_mhz, Some(2172));
    }

    #[test]
    fn freq_manual_ceiling_pins_the_mapping() {
        let mut config = freq_auto_cfg();
        config.freq.max_mhz = 2100; // manual pin: detection ignored
        let mut c = GpuController::new(
            PidController::from_params(&PidParams::default()),
            config.loop_kind,
        );
        let mut m = Mock::steady(70.0);
        m.power = Some(170.0);
        m.ceiling = Some(2790.0);
        m.clock = Some(3120.0);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(m.freq_caps, vec![1740]); // legacy 2100 mapping, unchanged
        assert_eq!(st.cap_mhz, Some(1740));
    }

    #[test]
    fn loop_switch_hands_over_the_actuator() {
        let mut config = cfg(ControlMode::Pid);
        let mut c = GpuController::new(PidController::from_params(&config.pid), config.loop_kind);
        let mut m = Mock::steady(80.0);
        tick(&mut c, &mut m, &config);
        assert_eq!(m.writes, vec![50]); // fan: 40 + 2·(80−75)

        // Switch to the frequency loop: the fan pin is undone and the new
        // actuator takes over in the same tick.
        config.loop_kind = LK::FreqTemp;
        m.ceiling = Some(2100.0); // freq side needs a ceiling to map effort
        m.clock = Some(1850.0);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(m.restores, 1, "old actuator restored on switch");
        assert_eq!(m.freq_caps.len(), 1, "new actuator wrote a cap");
        assert_eq!(st.failsafe, FailsafeState::Ok);
    }

    #[test]
    fn sensor_kind_selection() {
        let b = SensorBundle {
            core_c: Some(60.0),
            hotspot_c: Some(75.0),
            memory_c: None,
            board_c: Some(45.0),
        };
        assert_eq!(b.pick(SensorKind::Core), Some(60.0));
        assert_eq!(b.pick(SensorKind::Hotspot), Some(75.0));
        assert_eq!(b.pick(SensorKind::Max), Some(75.0));
        let bare = SensorBundle {
            core_c: Some(60.0),
            ..Default::default()
        };
        assert_eq!(
            bare.pick(SensorKind::Hotspot),
            Some(60.0),
            "hotspot falls back to core"
        );
        assert_eq!(bare.pick(SensorKind::Max), Some(60.0));
    }

    #[test]
    fn gpu_selection_filters_oob_indices() {
        let config = RuntimeConfig {
            gpus: GpuSelection::Indices(vec![0, 5]),
            ..RuntimeConfig::default()
        };
        assert_eq!(config.selected_indices(2), vec![0]);
    }
}
