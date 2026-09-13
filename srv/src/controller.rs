//! Per-GPU thermal closed-loop controller: mode state machine, failsafe
//! handling, and write deadbanding around the raw PID.
//!
//! The controller is backend-agnostic ([`ControlBackend`]) so the whole
//! state machine is unit-testable without a GPU.

use crate::config::{ControlMode, PidParams, RuntimeConfig, SensorKind};
use crate::pid::{PidController, PidTerms};
use serde::Serialize;

/// Extra cooling below the emergency line required to leave the 100%-duty
/// forced state; prevents flapping right at the threshold.
const EMERGENCY_HYSTERESIS_C: f32 = 2.0;

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
    fn write_fan_percent(&mut self, gpu_index: usize, percent: u32) -> Result<(), String>;
    /// Hand fan control back to the driver (undoes any pin).
    fn restore_fan_auto(&mut self, gpu_index: usize) -> Result<(), String>;
}

/// Degraded states that override normal PID behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailsafeState {
    /// Normal operation.
    Ok,
    /// `read_fail_reset` consecutive sensor failures — fan handed back to
    /// the driver; resumes automatically on the first good read.
    ReadFailures,
    /// Overtemp: `target_c + emergency_delta_c` reached — 100% duty forced
    /// (exits `EMERGENCY_HYSTERESIS_C` below the entry line).
    Emergency,
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
    /// True while the PID has released the fan to the driver's own curve
    /// (idle-release hysteresis; see `PidParams::release_below_c`).
    pub released: bool,
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
    applied_mode: ControlMode,
    last_written: Option<u32>,
    failsafe: FailsafeState,
    fail_streak: u32,
    emergency_active: bool,
    /// Idle-release hysteresis state (Pid mode only).
    released: bool,
    /// Consecutive ticks with temp ≤ the release line (dwell counter).
    below_streak: u32,
    /// PID decomposition of the most recent evaluation (None while the PID
    /// did not run this tick: Auto/Manual modes, released, pre-first-read).
    last_pid: Option<PidTerms>,
    last_error: Option<String>,
    last_sensors: SensorBundle,
    last_temp: Option<f32>,
    fan_measured: Option<u32>,
}

impl GpuController {
    pub fn new(pid: PidController) -> Self {
        Self {
            pid,
            applied_mode: ControlMode::Auto,
            last_written: None,
            failsafe: FailsafeState::Ok,
            fail_streak: 0,
            emergency_active: false,
            released: false,
            below_streak: 0,
            last_pid: None,
            last_error: None,
            last_sensors: SensorBundle::default(),
            last_temp: None,
            fan_measured: None,
        }
    }

    /// Pull tunables from the config snapshot; preserves PID state.
    pub fn sync_params(&mut self, p: &PidParams) {
        self.pid.sync_params(p);
    }

    /// Restore driver control unconditionally (shutdown/restore paths).
    pub fn restore(&mut self, index: usize, backend: &mut dyn ControlBackend) {
        if self.last_written.is_some() || self.applied_mode != ControlMode::Auto {
            match backend.restore_fan_auto(index) {
                Ok(()) => log::info!("GPU {index}: fan control restored to driver"),
                Err(e) => {
                    log::error!("GPU {index}: driver fan-control restore FAILED: {e}");
                    self.failsafe = FailsafeState::RestoreFailed;
                    self.last_error = Some(e);
                }
            }
        }
        self.last_written = None;
        self.applied_mode = ControlMode::Auto;
        self.released = false;
        self.below_streak = 0;
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
        self.handle_mode_transition(index, cfg, backend);
        self.sync_params(&cfg.pid);
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
        GpuControlStatus {
            index,
            name: name.to_string(),
            mode: self.applied_mode,
            failsafe: self.failsafe,
            temp_c: self.last_temp,
            sensors: self.last_sensors.clone(),
            fan_written_percent: self.last_written,
            released: self.released,
            fan_measured_percent: self.fan_measured,
            pid: self.last_pid,
            last_error: self.last_error.clone(),
        }
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
                    "GPU {index}: PID mode engaged (target {} °C)",
                    cfg.pid.target_c
                );
            }
            ControlMode::Manual => {
                self.pid.reset();
                log::info!("GPU {index}: manual mode ({}%)", cfg.manual_percent);
            }
        }
        self.applied_mode = cfg.mode;
        self.emergency_active = false;
        self.fail_streak = 0;
        self.released = false;
        self.below_streak = 0;
        if self.failsafe == FailsafeState::ReadFailures {
            self.failsafe = FailsafeState::Ok;
        }
    }

    fn tick_manual(&mut self, index: usize, cfg: &RuntimeConfig, backend: &mut dyn ControlBackend) {
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
        self.observe(index, backend);
    }

    fn tick_pid(
        &mut self,
        index: usize,
        cfg: &RuntimeConfig,
        backend: &mut dyn ControlBackend,
        dt_s: f32,
    ) {
        match backend.read_temps(index) {
            Err(e) => {
                self.fail_streak += 1;
                self.last_error = Some(e);
                // While released the driver already owns the fan — nothing
                // to restore, so the read-failure failsafe stays parked.
                if !self.released
                    && self.fail_streak >= cfg.read_fail_reset.max(1)
                    && self.failsafe != FailsafeState::ReadFailures
                {
                    log::error!(
                        "GPU {index}: {} consecutive sensor read failures; \
                         restoring fan control to driver",
                        self.fail_streak
                    );
                    match backend.restore_fan_auto(index) {
                        Ok(()) => self.last_written = None,
                        Err(e) => {
                            log::error!("GPU {index}: failsafe restore failed: {e}");
                            self.failsafe = FailsafeState::RestoreFailed;
                        }
                    }
                    self.failsafe = FailsafeState::ReadFailures;
                    self.emergency_active = false;
                }
            }
            Ok(bundle) => {
                self.fail_streak = 0;
                self.last_sensors = bundle.clone();
                let temp = bundle.pick(cfg.sensor);
                self.last_temp = temp;
                if self.failsafe == FailsafeState::ReadFailures {
                    log::info!("GPU {index}: sensor recovered; PID resumes");
                    self.failsafe = FailsafeState::Ok;
                    self.pid.reset();
                }
                let Some(temp) = temp else { return };
                let engage_at = cfg.pid.target_c - cfg.pid.engage_below_c;
                let release_at = cfg.pid.target_c - cfg.pid.release_below_c;

                // Idle-release hysteresis: cool → hand the fan to the
                // driver curve (it idles — zero RPM — better than the PID
                // can park it); warm again → take control back.
                if self.released {
                    if temp >= engage_at {
                        log::info!(
                            "GPU {index}: {temp:.1} °C ≥ re-engage line {engage_at:.1} °C; PID control resumes"
                        );
                        self.released = false;
                        self.pid.reset();
                    } else {
                        return; // stay on the driver curve this tick
                    }
                }
                if cfg.pid.release_below_c > 0.0 && temp <= release_at {
                    self.below_streak += 1;
                    if self.below_streak >= cfg.pid.release_ticks.max(1) {
                        match backend.restore_fan_auto(index) {
                            Ok(()) => {
                                log::info!(
                                    "GPU {index}: {temp:.1} °C ≤ release line {release_at:.1} °C \
                                     for {} tick(s); fan released to driver curve",
                                    self.below_streak
                                );
                                self.last_written = None;
                                self.pid.reset();
                                self.released = true;
                                self.below_streak = 0;
                                return;
                            }
                            Err(e) => {
                                // Keep controlling; retry the release next tick.
                                log::error!("GPU {index}: release restore failed: {e}");
                                self.last_error = Some(e);
                            }
                        }
                    }
                } else {
                    self.below_streak = 0;
                }
                self.apply_pid_output(index, cfg, backend, temp, dt_s);
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
        let emergency_at = cfg.pid.target_c + cfg.pid.emergency_delta_c;
        let output = if self.emergency_active {
            if temp <= emergency_at - EMERGENCY_HYSTERESIS_C {
                log::info!("GPU {index}: left emergency state; PID resumes from reset");
                self.emergency_active = false;
                self.pid.reset();
                self.failsafe = FailsafeState::Ok;
                let terms = self.pid.step(temp, dt_s);
                self.last_pid = Some(terms);
                terms.output_percent
            } else {
                // Keep the /status decomposition truthful while the PID is
                // not being stepped: show what it would compute, with the
                // forced output.
                let error = temp - cfg.pid.target_c;
                self.last_pid = Some(PidTerms {
                    error_c: error,
                    p: cfg.pid.kp * error,
                    i: cfg.pid.ki * self.pid.integral(),
                    d: 0.0,
                    output_percent: 100.0,
                });
                100.0 // stay forced while inside the hysteresis band
            }
        } else if temp >= emergency_at {
            log::warn!(
                "GPU {index}: {temp} °C ≥ emergency line {emergency_at} °C; forcing 100% duty"
            );
            self.emergency_active = true;
            self.failsafe = FailsafeState::Emergency;
            let error = temp - cfg.pid.target_c;
            self.last_pid = Some(PidTerms {
                error_c: error,
                p: cfg.pid.kp * error,
                i: cfg.pid.ki * self.pid.integral(),
                d: 0.0,
                output_percent: 100.0,
            });
            100.0
        } else {
            let terms = self.pid.step(temp, dt_s);
            self.last_pid = Some(terms);
            terms.output_percent
        };

        // Write deadband (anti-chatter): quantized duty writes are a relay
        // nonlinearity; suppress writes while the output stays within the
        // deadband of the duty already on the wire. A forced emergency write
        // (|100 − last| ≫ deadband) always passes.
        let duty = output.round().clamp(0.0, 100.0) as u32;
        let inside_deadband = match self.last_written {
            None => false,
            Some(last) => (output - last as f32).abs() < cfg.pid.write_deadband_percent,
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
    use crate::config::{GpuSelection, RuntimeConfig};

    /// Scriptable fake hardware.
    struct Mock {
        /// Temps returned per read_temps call (popped; last one repeats).
        temps: Vec<Result<SensorBundle, String>>,
        writes: Vec<u32>,
        restores: usize,
        fail_writes: bool,
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
        fn write_fan_percent(&mut self, _i: usize, p: u32) -> Result<(), String> {
            if self.fail_writes {
                return Err("write rejected".to_string());
            }
            self.writes.push(p);
            Ok(())
        }
        fn restore_fan_auto(&mut self, _i: usize) -> Result<(), String> {
            self.restores += 1;
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
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
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
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
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
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
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
    fn emergency_forces_max_with_hysteresis() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        // default emergency line: 75 + 12 = 87; exit at 85.
        let mut m = Mock::steady(88.0);
        let config = cfg(ControlMode::Pid);
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.writes.last(), Some(&100));
        // Still inside hysteresis band: stays at 100.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(86.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.writes.last(), Some(&100));
        // Below exit line: PID resumes (out = 40 + 2·9 = 58).
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(84.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert_eq!(st.failsafe, FailsafeState::Ok);
        assert_eq!(m.writes.last(), Some(&58));
    }

    #[test]
    fn read_failures_trip_failsafe_then_recover() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut config = cfg(ControlMode::Pid);
        config.read_fail_reset = 3;
        let mut m = Mock {
            temps: vec![Err("sensor gone".into())],
            writes: vec![50],
            restores: 0,
            fail_writes: false,
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
    fn auto_mode_never_writes() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(90.0);
        let st = tick(&mut c, &mut m, &cfg(ControlMode::Auto));
        assert!(m.writes.is_empty());
        assert_eq!(st.fan_written_percent, None);
        assert_eq!(st.temp_c, Some(90.0), "Auto still observes for /status");
    }

    #[test]
    fn write_failure_is_reported_not_fatal() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(85.0);
        m.fail_writes = true;
        let st = tick(&mut c, &mut m, &cfg(ControlMode::Pid));
        assert_eq!(st.last_error.as_deref(), Some("write rejected"));
        assert_eq!(st.fan_written_percent, None);
    }

    /// PID config for release tests: no integral creep, 2-tick dwell.
    fn release_cfg() -> RuntimeConfig {
        let mut config = cfg(ControlMode::Pid);
        config.pid.ki = 0.0;
        config.pid.release_ticks = 2;
        config // defaults: release at target−4, re-engage at target−1
    }

    #[test]
    fn release_hands_fan_back_to_driver_when_cool() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(75.0);
        let config = release_cfg();
        tick(&mut c, &mut m, &config);
        assert_eq!(m.writes, vec![40]);

        // Below the release line (71): first tick still controls (dwell 1/2),
        // second tick releases to the driver curve.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(70.5),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert!(!st.released);
        let st = tick(&mut c, &mut m, &config);
        assert!(st.released);
        assert_eq!(m.restores, 1);
        assert_eq!(st.fan_written_percent, None);

        // Released: no further writes while it stays cool.
        let writes_after_release = m.writes.len();
        let st = tick(&mut c, &mut m, &config);
        assert!(st.released);
        assert_eq!(m.writes.len(), writes_after_release);
    }

    #[test]
    fn pid_reengages_when_warm_and_holds_state_inside_the_band() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(70.5);
        let config = release_cfg();
        // Force into the released state (2 ticks below the line).
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        assert!(c.released);

        // Warm again past the re-engage line (74): PID resumes immediately.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(74.5),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert!(!st.released);
        assert_eq!(m.writes.last(), Some(&39)); // 40 + 2·(74.5−75)

        // Inside the hysteresis band (71–74) while engaged: keep controlling.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(72.5),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert!(!st.released);
        assert_eq!(m.writes.last(), Some(&35)); // 40 + 2·(72.5−75)

        // Inside the band while released: stay released.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(70.0),
            ..Default::default()
        })];
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        assert!(c.released);
        let writes_released = m.writes.len();
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(72.5),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert!(st.released, "band must hold the released state");
        assert_eq!(m.writes.len(), writes_released);
    }

    #[test]
    fn emergency_overrides_released_state() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(70.5);
        let config = release_cfg();
        tick(&mut c, &mut m, &config);
        tick(&mut c, &mut m, &config);
        assert!(c.released);

        // A spike past the emergency line re-engages and forces 100% in the
        // same tick — release must never gate the overtemp response.
        m.temps = vec![Ok(SensorBundle {
            core_c: Some(88.0),
            ..Default::default()
        })];
        let st = tick(&mut c, &mut m, &config);
        assert!(!st.released);
        assert_eq!(st.failsafe, FailsafeState::Emergency);
        assert_eq!(m.writes.last(), Some(&100));
    }

    #[test]
    fn release_disabled_when_release_below_zero() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock::steady(60.0);
        let mut config = release_cfg();
        config.pid.release_below_c = 0.0;
        for _ in 0..5 {
            let st = tick(&mut c, &mut m, &config);
            assert!(!st.released, "release_below_c=0 must disable releasing");
        }
        assert_eq!(m.restores, 0);
        assert_eq!(m.writes, vec![10]); // 40 + 2·(60−75), deadband holds it
    }

    #[test]
    fn write_deadband_gates_slow_output_drift() {
        // kp=0, ki=0.4, constant e=+2 → output ramps 0.8 %/tick.
        // (Gains go through the config: sync_params re-applies them per tick.)
        let mut config = cfg(ControlMode::Pid);
        config.pid.kp = 0.0;
        config.pid.ki = 0.4;
        let mut c = GpuController::new(PidController::from_params(&config.pid));
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

        let mut c = GpuController::new(PidController::from_params(&config.pid));
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
