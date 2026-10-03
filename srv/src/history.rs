//! Per-GPU time-series ring buffer fed by the control loop (1 sample per
//! tick). Capacity bounds memory (~1 h at 1 s); `/api/history` drains the
//! last N seconds as JSON. Restart clears it — acceptable for a monitoring
//! panel, documented.

use crate::monitor::MonitorSample;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// Samples retained per GPU.
pub const HISTORY_CAPACITY: usize = 3600;

/// One timestamped observation of one GPU (control-loop state + monitor
/// readback merged by the runtime).
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    /// Unix epoch seconds.
    pub t: u64,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub core_clock_mhz: Option<f32>,
    pub mem_clock_mhz: Option<f32>,
    pub fan_pct: Option<u32>,
    pub util_pct: Option<f32>,
    pub volt_mv: Option<f32>,
    pub pstate: Option<String>,
    /// Restriction effort the loop wrote this tick (None = no write).
    pub effort_pct: Option<u32>,
}

#[derive(Debug, Default)]
pub struct History {
    rings: Vec<VecDeque<Sample>>,
}

impl History {
    pub fn new(gpu_count: usize) -> Self {
        Self {
            rings: vec![VecDeque::with_capacity(HISTORY_CAPACITY); gpu_count],
        }
    }

    pub fn push(&mut self, gpu_index: usize, sample: Sample) {
        let Some(ring) = self.rings.get_mut(gpu_index) else {
            return;
        };
        if ring.len() >= HISTORY_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(sample);
    }

    /// Last `seconds` samples for one GPU (oldest first).
    pub fn last(&self, gpu_index: usize, seconds: u64) -> Vec<Sample> {
        let Some(ring) = self.rings.get(gpu_index) else {
            return Vec::new();
        };
        let cutoff = now_s().saturating_sub(seconds);
        ring.iter().skip_while(|s| s.t < cutoff).cloned().collect()
    }

    pub fn gpu_count(&self) -> usize {
        self.rings.len()
    }
}

pub fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Merge a control-status snapshot with a monitor readback into a Sample.
pub fn sample_from(
    effort_pct: Option<u32>,
    temp_c: Option<f32>,
    monitor: &MonitorSample,
) -> Sample {
    Sample {
        t: now_s(),
        temp_c: temp_c.or(monitor.temp_c),
        power_w: monitor.power_w,
        core_clock_mhz: monitor.core_clock_mhz,
        mem_clock_mhz: monitor.mem_clock_mhz,
        fan_pct: monitor.fan_pct,
        util_pct: monitor.util_pct,
        volt_mv: monitor.volt_mv,
        pstate: monitor.pstate.clone(),
        effort_pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_at(t: u64, temp: f32) -> Sample {
        Sample {
            t,
            temp_c: Some(temp),
            power_w: None,
            core_clock_mhz: None,
            mem_clock_mhz: None,
            fan_pct: None,
            util_pct: None,
            volt_mv: None,
            pstate: None,
            effort_pct: None,
        }
    }

    #[test]
    fn ring_keeps_last_window_and_capacity() {
        let now = now_s();
        let mut h = History::new(1);
        for t in 0..(HISTORY_CAPACITY as u64 + 10) {
            h.push(0, sample_at(now - 3610 + t, t as f32));
        }
        // capacity bound + oldest dropped
        let all = h.last(0, u64::MAX);
        assert_eq!(all.len(), HISTORY_CAPACITY);
        assert_eq!(all[0].t, now - 3600);
        // window filter (last 31 s of the retained span)
        let recent = h.last(0, 31);
        assert_eq!(recent.len(), 31);
        assert_eq!(recent[0].temp_c.unwrap(), 3579.0);
    }

    #[test]
    fn oob_gpu_index_is_a_noop() {
        let mut h = History::new(1);
        h.push(5, sample_at(1, 50.0));
        assert!(h.last(5, 60).is_empty());
    }
}
