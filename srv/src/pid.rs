//! Discrete PID controller for fan duty vs. temperature setpoint.
//!
//! `output% = base + kp·e + ki·∫e·dt − kd·dT/dt`, clamped to `[out_min, out_max]`.
//!
//! Design choices tuned for a GPU thermal plant:
//! - **Feed-forward `base_percent`**: the P/I/D terms act around a duty you
//!   set near the card's steady-state need at typical load. Without it, zero
//!   error means zero output and the loop limit-cycles around the setpoint.
//! - **Derivative on measurement** (`−kd·dT/dt`): changing the setpoint does
//!   not kick the output.
//! - **Conditional-integration anti-windup**: the integrator freezes while
//!   the output is saturated AND the error would push it further into that
//!   rail, so recovery from saturation is immediate.
//! - **Asymmetric integral discharge**: integral that *opposes* the current
//!   error unwinds [`INTEGRAL_UNWIND_GAIN`]× faster (clamped so it cannot
//!   cross zero). Without this, the integral built during the hot phase
//!   discharges at only `ki·|e|` per second — after a load drop the fan
//!   stays pinned high for minutes (observed as low-side undershoot and
//!   never idling).

use crate::config::PidParams;
use serde::Serialize;

/// Multiplier on the integration increment while the stored integral
/// opposes the current error sign. Discharge-only acceleration: clamped at
/// zero, so it can never wind the integral in the *error's* direction
/// faster than plain `ki` would.
const INTEGRAL_UNWIND_GAIN: f32 = 4.0;

/// One PID step's decomposition, for `/status` observability and tuning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PidTerms {
    /// `measurement − target` (°C). Positive = too hot.
    pub error_c: f32,
    pub p: f32,
    pub i: f32,
    pub d: f32,
    /// Clamped controller output (fan duty %).
    pub output_percent: f32,
}

#[derive(Debug, Clone)]
pub struct PidController {
    target_c: f32,
    kp: f32,
    ki: f32,
    kd: f32,
    base_percent: f32,
    out_min: f32,
    out_max: f32,
    integral: f32,
    prev_measurement: Option<f32>,
}

impl PidController {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_c: f32,
        kp: f32,
        ki: f32,
        kd: f32,
        base_percent: f32,
        out_min: f32,
        out_max: f32,
    ) -> Self {
        Self {
            target_c,
            kp,
            ki,
            kd,
            base_percent,
            out_min,
            out_max,
            integral: 0.0,
            prev_measurement: None,
        }
    }

    pub fn from_params(p: &PidParams) -> Self {
        Self::new(
            p.target_c,
            p.kp,
            p.ki,
            p.kd,
            p.base_percent,
            p.min_percent,
            p.max_percent,
        )
    }

    /// Zero the integrator and derivative memory (mode switches, emergency
    /// exit, setpoint semantics change).
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_measurement = None;
    }

    /// Live-tunable parameters (HTTP `/pid`); keeps integral/derivative state.
    pub fn set_target_c(&mut self, target_c: f32) {
        self.target_c = target_c;
    }

    pub fn set_gains(&mut self, kp: f32, ki: f32, kd: f32) {
        self.kp = kp;
        self.ki = ki;
        self.kd = kd;
    }

    pub fn set_base_percent(&mut self, base_percent: f32) {
        self.base_percent = base_percent;
    }

    pub fn set_limits(&mut self, out_min: f32, out_max: f32) {
        self.out_min = out_min;
        self.out_max = out_max;
    }

    /// Pull all tunables from a params snapshot while preserving state.
    pub fn sync_params(&mut self, p: &PidParams) {
        self.set_target_c(p.target_c);
        self.set_gains(p.kp, p.ki, p.kd);
        self.set_base_percent(p.base_percent);
        self.set_limits(p.min_percent, p.max_percent);
    }

    pub fn integral(&self) -> f32 {
        self.integral
    }

    /// Advance one control step. `dt_s` ≤ 0 or non-finite falls back to 1 s
    /// so a broken clock cannot inject a huge integral/derivative kick.
    pub fn step(&mut self, measurement_c: f32, dt_s: f32) -> PidTerms {
        let dt = if dt_s.is_finite() && dt_s > 0.0 {
            dt_s
        } else {
            1.0
        };
        let error = measurement_c - self.target_c;
        let p = self.kp * error;
        let d = match self.prev_measurement {
            Some(prev) => -self.kd * (measurement_c - prev) / dt,
            None => 0.0,
        };

        // Accelerated discharge when the integral opposes the error, clamped
        // so it discharges to (never through) zero.
        let opposed = (self.integral > 0.0 && error < 0.0) || (self.integral < 0.0 && error > 0.0);
        let delta = if opposed {
            let fast = error * dt * INTEGRAL_UNWIND_GAIN;
            if self.integral > 0.0 {
                fast.max(-self.integral)
            } else {
                fast.min(-self.integral)
            }
        } else {
            error * dt
        };
        let trial_i = self.integral + delta;
        let raw_trial = self.base_percent + p + self.ki * trial_i + d;
        let saturating_high = raw_trial > self.out_max && error > 0.0;
        let saturating_low = raw_trial < self.out_min && error < 0.0;
        if !(saturating_high || saturating_low) {
            self.integral = trial_i;
        }

        let output =
            (self.base_percent + p + self.ki * self.integral + d).clamp(self.out_min, self.out_max);
        self.prev_measurement = Some(measurement_c);

        PidTerms {
            error_c: error,
            p,
            i: self.ki * self.integral,
            d,
            output_percent: output,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(kp: f32, ki: f32, kd: f32) -> PidController {
        PidController::new(75.0, kp, ki, kd, 40.0, 0.0, 100.0)
    }

    #[test]
    fn proportional_step() {
        let mut pid = plain(2.0, 0.0, 0.0);
        // 85 °C vs 75 °C target: out = 40 + 2·10 = 60
        let t = pid.step(85.0, 1.0);
        assert!((t.output_percent - 60.0).abs() < 1e-4);
        assert!((t.error_c - 10.0).abs() < 1e-4);
    }

    #[test]
    fn output_clamps_to_limits() {
        let mut pid = plain(2.0, 0.0, 0.0);
        let t = pid.step(125.0, 1.0); // 40 + 2·50 = 140 → 100
        assert_eq!(t.output_percent, 100.0);
        let mut pid = plain(2.0, 0.0, 0.0);
        let t = pid.step(35.0, 1.0); // 40 + 2·(−40) = −40 → 0
        assert_eq!(t.output_percent, 0.0);
    }

    #[test]
    fn integral_eliminates_offset() {
        // Plant: each +1% fan duty cools −0.25 °C (temp = 80 − 0.25·(out−40)).
        // Holding 75 °C needs out = 60; PI must find it via the integral.
        let mut pid = PidController::new(75.0, 2.0, 0.1, 0.0, 40.0, 0.0, 100.0);
        let mut out = 40.0;
        for _ in 0..5000 {
            let temp = 80.0 - 0.25 * (out - 40.0);
            out = pid.step(temp, 1.0).output_percent;
        }
        assert!((out - 60.0).abs() < 0.1, "expected ~60%, got {out}");
    }

    #[test]
    fn anti_windup_recovers_immediately() {
        // base 40 + i: the output rail (100) is reached at integral = 60.
        let mut pid = plain(0.0, 1.0, 0.0);
        for _ in 0..200 {
            pid.step(85.0, 1.0); // saturated high the whole time
        }
        assert_eq!(pid.integral(), 60.0, "integral frozen at the rail");
        let t = pid.step(70.0, 1.0); // error flips negative
        assert!(
            t.output_percent < 100.0,
            "output must leave the rail on the first step, got {}",
            t.output_percent
        );
    }

    #[test]
    fn integral_unwinds_faster_when_error_opposes_it() {
        // Build integral +10 at e=+2 (kp=0, ki=0.5, base=40, no saturation).
        let mut pid = plain(0.0, 0.5, 0.0);
        for _ in 0..5 {
            pid.step(77.0, 1.0);
        }
        assert!((pid.integral() - 10.0).abs() < 1e-4);
        // Error flips to −1: the 4× discharge (clamped at zero) empties the
        // integral in 3 ticks instead of the plain ki·|e| trickle.
        for _ in 0..3 {
            pid.step(74.0, 1.0);
        }
        assert!(
            pid.integral().abs() < 0.5,
            "integral discharged to zero, got {}",
            pid.integral()
        );
        // The fast path stops at zero: the next tick integrates at the plain
        // error·dt rate (e = −1), not at 4×.
        pid.step(74.0, 1.0);
        assert!(
            (pid.integral() + 1.0).abs() < 1e-4,
            "normal integration rate must resume at zero, got {}",
            pid.integral()
        );
    }

    #[test]
    fn integral_discharge_is_symmetric() {
        // Negative integral built while cool discharges fast once hot.
        let mut pid = plain(0.0, 0.5, 0.0);
        for _ in 0..5 {
            pid.step(73.0, 1.0);
        }
        assert!((pid.integral() + 10.0).abs() < 1e-4);
        for _ in 0..2 {
            pid.step(77.0, 1.0);
        }
        assert!(
            pid.integral().abs() < 0.5,
            "integral discharged to zero, got {}",
            pid.integral()
        );
        // Normal rate resumes after the fast discharge lands on zero.
        pid.step(77.0, 1.0);
        assert!(
            (pid.integral() - 2.0).abs() < 1e-4,
            "normal integration rate must resume at zero, got {}",
            pid.integral()
        );
    }

    #[test]
    fn derivative_on_measurement_ignores_setpoint_jump() {
        let mut pid = plain(1.0, 0.0, 5.0);
        pid.step(80.0, 1.0);
        let t = pid.step(80.0, 1.0);
        pid.set_target_c(60.0); // huge setpoint jump; measurement unchanged
        let t2 = pid.step(80.0, 1.0);
        assert_eq!(t.d, 0.0);
        assert_eq!(t2.d, 0.0, "no derivative kick on setpoint change");
    }

    #[test]
    fn derivative_responds_to_ramp() {
        let mut pid = plain(0.0, 0.0, 1.0);
        pid.step(80.0, 1.0);
        let t = pid.step(81.0, 1.0); // +1 °C/s → d = −1
        assert!((t.d + 1.0).abs() < 1e-4);
    }

    #[test]
    fn bad_dt_falls_back_to_one_second() {
        let mut pid = plain(1.0, 0.1, 0.0);
        let t = pid.step(85.0, 0.0);
        assert!((t.i - 1.0).abs() < 1e-4, "error 10 × ki 0.1 × dt 1 = 1.0");
        let mut pid2 = plain(1.0, 0.1, 0.0);
        let t2 = pid2.step(85.0, f32::NAN);
        assert!((t2.i - t.i).abs() < 1e-4);
    }

    #[test]
    fn reset_clears_state() {
        let mut pid = plain(1.0, 0.1, 0.0);
        pid.step(85.0, 1.0);
        pid.reset();
        assert_eq!(pid.integral(), 0.0);
        let t = pid.step(85.0, 1.0);
        assert_eq!(t.d, 0.0, "prev_measurement cleared");
    }

    #[test]
    fn sync_params_keeps_state() {
        let mut pid = plain(1.0, 0.1, 0.0);
        pid.step(85.0, 1.0);
        let integral_before = pid.integral();
        let p = PidParams {
            target_c: 70.0,
            kp: 3.0,
            ..PidParams::default()
        };
        pid.sync_params(&p);
        assert_eq!(pid.integral(), integral_before);
        let t = pid.step(85.0, 1.0);
        assert!((t.error_c - 15.0).abs() < 1e-4);
    }
}
