//! Per-GPU thermal closed-loop controller: mode state machine, forced
//! zones (overtemp / undertemp), failsafe handling, and write deadbanding
//! around the raw PID.
//!
//! The controller is backend-agnostic ([`ControlBackend`]) so the whole
//! state machine is unit-testable without a GPU.

use crate::config::{ControlMode, PidParams, RuntimeConfig, SensorKind};
use crate::pid::{PidController, PidTerms};
use serde::Serialize;

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
    fn write_fan_percent(&mut self, gpu_index: usize, percent: u32) -> Result<(), String>;
    /// Hand fan control back to the driver (undoes any pin).
    fn restore_fan_auto(&mut self, gpu_index: usize) -> Result<(), String>;
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
    idle_active: bool,
    /// Adaptive feed-forward state: the learned base duty. Only honored
    /// while `adaptive_base` is on (and seeded from the config otherwise).
    base: f32,
    base_seeded: bool,
    /// PID decomposition of the most recent evaluation (None while the PID
    /// did not run this tick: Auto/Manual modes, pre-first-read).
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
            idle_active: false,
            base: 0.0,
            base_seeded: false,
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
        // Base ownership: config-owned when fixed, controller-owned (learned)
        // once adaptive — seeded from the config on the first tick.
        if p.adaptive_base && self.base_seeded {
            self.pid.set_base_percent(self.base);
        } else {
            self.base = p.base_percent;
            self.base_seeded = true;
        }
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
        self.idle_active = false;
        self.fail_streak = 0;
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
                if self.fail_streak >= cfg.read_fail_reset.max(1) {
                    if self.last_written.is_some() {
                        // Hand the fan back to the driver and keep re-trying
                        // while the pin is ours: the driver may be mid-TDR
                        // (calls fail, then recover), and a stuck pin during
                        // that window is the worst case for a stress run.
                        log::error!(
                            "GPU {index}: {} consecutive sensor read failures; \
                             restoring fan control to driver",
                            self.fail_streak
                        );
                        match backend.restore_fan_auto(index) {
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
                if let Some(temp) = temp {
                    self.apply_pid_output(index, cfg, backend, temp, dt_s);
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
        let emergency_at = cfg.pid.target_c + cfg.pid.emergency_delta_c;
        let idle_at = cfg.pid.target_c - cfg.pid.idle_delta_c;

        // Forced-zone latches with 2 °C exit hysteresis. The PID keeps
        // stepping through both zones — its conditional anti-windup freezes
        // the integral at whatever value makes the raw output sit at the
        // forced rail, so leaving a zone is bumpless (no reset, no blast).
        if self.emergency_active {
            if temp <= emergency_at - ZONE_HYSTERESIS_C {
                log::info!("GPU {index}: left emergency zone ({temp:.1} °C)");
                self.emergency_active = false;
            }
        } else if temp >= emergency_at {
            log::warn!(
                "GPU {index}: {temp:.1} °C ≥ emergency line {emergency_at:.1} °C; forcing 100% duty"
            );
            self.emergency_active = true;
        }
        if self.idle_active {
            if temp >= idle_at + ZONE_HYSTERESIS_C {
                log::info!("GPU {index}: left idle zone ({temp:.1} °C); PID resumes");
                self.idle_active = false;
            }
        } else if cfg.pid.idle_delta_c > 0.0 && temp <= idle_at {
            log::info!(
                "GPU {index}: {temp:.1} °C ≤ idle line {idle_at:.1} °C; forcing {}% duty",
                cfg.pid.min_percent
            );
            self.idle_active = true;
        }

        let terms = self.pid.step(temp, dt_s);
        self.last_pid = Some(terms);
        if cfg.pid.adaptive_base {
            self.learn_base(cfg);
        }

        let output = if self.emergency_active {
            self.failsafe = FailsafeState::Emergency;
            100.0
        } else if self.idle_active {
            self.failsafe = FailsafeState::IdleHold;
            cfg.pid.min_percent
        } else {
            // Clear only our own zone flags; ReadFailures/RestoreFailed are
            // owned by the failure paths and must not be clobbered here.
            if self.failsafe == FailsafeState::Emergency || self.failsafe == FailsafeState::IdleHold
            {
                self.failsafe = FailsafeState::Ok;
            }
            terms.output_percent
        };

        // Write deadband (anti-chatter): quantized duty writes are a relay
        // nonlinearity; suppress writes while the output stays within the
        // deadband of the duty already on the wire. A forced-zone write
        // (|rail − last| ≫ deadband) always passes.
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

    /// Adaptive feed-forward: continuously re-center integral authority into
    /// `self.base`. The transfer is output-continuous (base moves by exactly
    /// what the integral counter-moves), so learning adds no loop dynamics —
    /// it only re-partitions state, which is how the base tracks the load
    /// level in both directions without configuration. Requires `ki > 0`.
    fn learn_base(&mut self, cfg: &RuntimeConfig) {
        let i_term = self.pid.i_term();
        if i_term.abs() < BASE_ABSORB_FLOOR {
            return;
        }
        let delta = (BASE_ABSORB_FRACTION * i_term).clamp(-BASE_ABSORB_MAX, BASE_ABSORB_MAX);
        let new_base = (self.base + delta).clamp(cfg.pid.min_percent, cfg.pid.max_percent);
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
    use crate::config::{GpuSelection, RuntimeConfig};

    /// Scriptable fake hardware.
    struct Mock {
        /// Temps returned per read_temps call (popped; last one repeats).
        temps: Vec<Result<SensorBundle, String>>,
        writes: Vec<u32>,
        restores: usize,
        fail_writes: bool,
        /// restore_fan_auto fails this many times before succeeding.
        restore_fails: u32,
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
            if self.restore_fails > 0 {
                self.restore_fails -= 1;
                return Err("restore rejected".to_string());
            }
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
    fn emergency_forces_max_with_hysteresis_and_bumpless_exit() {
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
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
        let mut c = GpuController::new(PidController::from_params(&config.pid));
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
        let mut c = GpuController::new(PidController::from_params(&config.pid));
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
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut config = cfg(ControlMode::Pid);
        config.read_fail_reset = 3;
        let mut m = Mock {
            temps: vec![Err("sensor gone".into())],
            writes: vec![50],
            restores: 0,
            fail_writes: false,
            restore_fails: 0,
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
        let mut c = GpuController::new(PidController::from_params(&PidParams::default()));
        let mut m = Mock {
            temps: vec![Err("sensor gone".into())],
            writes: Vec::new(),
            restores: 0,
            fail_writes: false,
            restore_fails: 2,
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
        };
        let mut c = GpuController::new(PidController::from_params(&config.pid));
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
        let mut c = GpuController::new(PidController::from_params(&config.pid));
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
        let mut c = GpuController::new(PidController::from_params(&config.pid));
        let mut m = Mock::steady(75.0);
        let st = tick(&mut c, &mut m, &config);
        assert!((st.pid.expect("pid ran").base_percent - 40.0).abs() < 1e-4);

        config.pid.base_percent = 55.0;
        let st = tick(&mut c, &mut m, &config);
        assert!((st.pid.expect("pid ran").base_percent - 55.0).abs() < 1e-4);
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
