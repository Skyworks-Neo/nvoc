//! NVAPI-backed [`ControlBackend`] with an NVML fallback for drivers/OS
//! combinations where the NDA fan surface is rejected (-104/-3).
//!
//! Write path priority (per the 2026-09-10 472.12 fan-surface audit and the
//! probe-fan-470 findings): `SetFanPercent` (NDA 0xEB44E8AA RMW — its RMW
//! presence-mask handling is what keeps the ghost-Cooler2 count mismatch
//! away) → NVML `set_fan_speed` manual pin (the write symbols exist on
//! Linux; the Windows nvml.dll exports none, so the fallback is dead there
//! by construction). Restore path: `ResetNvapiFanControl` (the only verified
//! un-pin: control block bit0=0 + policy Default) → NVML `set_default_fan_speed`.

use crate::controller::{ControlBackend, FanReading, SensorBundle};
use crate::monitor::{MonitorSample, OffsetBackend, OffsetDomain};
use log::{info, warn};
use nvapi::hi::Gpu;
use nvapi::{ClockDomain, ClockFrequencyType, Kilohertz, KilohertzDelta, PState, ThermalTarget};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureThreshold;
use nvml_wrapper::enum_wrappers::device::{Clock as NvmlClockType, ClockId, PerformanceState};
use nvml_wrapper::enums::device::FanControlPolicy;

use nvoc_core::{
    BackendSet, GpuId, GpuTarget, QueryClockOffset, QueryFanInfo, QueryNvapiThermalSettings,
    ResetFanSpeed, ResetFreqLock, ResetNvapiFanControl, ResetPstateGlobalFreqOffset,
    ResetVfpFrequencyLock, SetClockOffset, SetFanPercent, SetFanSpeed, SetLockedClocks,
    SetNvapiClkDomainOffset, SetPowerLimit, SetTemperatureLimit, SetVfpFrequencyLock,
    TargetInventory, discover_targets, run as run_gpu_operation,
};
use std::borrow::Cow;

/// GPU count cap for the `?gpu=` parameter (mirrors the legacy guard).
pub const GPU_INDEX_MAX: usize = 63;

pub struct NvapiBackend {
    inventory: TargetInventory,
    /// Index-aligned with the NVML enumeration order (the legacy service's
    /// pairing, guarded against drift by [`Self::gpu_id`]).
    gpus: Vec<Gpu>,
    names: Vec<String>,
    /// ThermChannel LUT per GPU, resolved lazily on first temperature read
    /// (`None` = the card does not expose the channel pair).
    channel_luts: Vec<Option<ThermalChannelLut>>,
    /// NVML handle for power reads and the locked-clocks fallback.
    nvml: Nvml,
}

impl NvapiBackend {
    pub fn discover() -> Result<Self, String> {
        let nvml = Nvml::init().map_err(|e| format!("NVML init failed: {e:?}"))?;
        let inventory =
            discover_targets(BackendSet::Both).map_err(|e| format!("GPU discovery failed: {e}"))?;
        let gpus = Gpu::enumerate().map_err(|e| format!("NVAPI GPU enumeration failed: {e}"))?;
        let count = nvml.device_count().unwrap_or(0);
        let mut names = Vec::with_capacity(count as usize);
        for i in 0..count {
            let name = nvml
                .device_by_index(i)
                .ok()
                .and_then(|d| d.name().ok())
                .unwrap_or_else(|| format!("GPU {i}"));
            names.push(name);
        }
        if gpus.len() != names.len() {
            warn!(
                "NVML sees {} GPU(s) but NVAPI enumerates {}; controlling the smaller set",
                names.len(),
                gpus.len()
            );
        }
        Ok(Self {
            channel_luts: vec![None; gpus.len()],
            inventory,
            gpus,
            names,
            nvml,
        })
    }

    pub fn gpu_count(&self) -> usize {
        self.gpus.len()
    }

    pub fn name(&self, index: usize) -> Cow<'_, str> {
        self.names
            .get(index)
            .map(|s| Cow::Borrowed(s.as_str()))
            .unwrap_or(Cow::Owned(format!("GPU {index}")))
    }

    fn gpu_id(&self, index: usize) -> Result<GpuId, String> {
        self.gpus
            .get(index)
            .map(|g| GpuId(g.id() as u32))
            .ok_or_else(|| {
                format!(
                    "GPU {index} out of range: NVML/NVAPI drift (NVAPI sees {})",
                    self.gpus.len()
                )
            })
    }

    fn target(&self, gpu_index: usize) -> Result<GpuTarget<'_>, String> {
        let id = self.gpu_id(gpu_index)?;
        self.inventory
            .target_by_id(id)
            .map_err(|e| format!("GPU {gpu_index}: {e}"))
    }

    /// Legacy `/oc_global`: one-shot P0 graphics clock delta.
    pub fn set_oc_global(&mut self, index: usize, delta_khz: i32) -> Result<(), String> {
        let gpu = self
            .gpus
            .get(index)
            .ok_or_else(|| format!("GPU {index} out of range"))?;
        gpu.inner()
            .set_pstates(
                [(PState::P0, ClockDomain::Graphics, KilohertzDelta(delta_khz))]
                    .iter()
                    .copied(),
            )
            .map_err(|e| format!("GPU {index}: set_pstates: {e}"))
    }
}

/// ThermChannel layout for one GPU (the priChIdx LUT is static per card, so
/// it is resolved once and cached). Channels missing on a card are `None`
/// and the corresponding reading falls back to the legacy integer sensor.
#[derive(Debug, Clone, Copy)]
struct ThermalChannelLut {
    mask: u32,
    /// GPU_AVG primary channel — the fine-grained core reading.
    core: Option<u8>,
    /// GPU_MAX primary channel — the hot spot.
    hotspot: Option<u8>,
    memory: Option<u8>,
    board: Option<u8>,
}

/// Plausibility guard for ThermChannel decodes: an unpopulated channel can
/// decode to 0 °C, which must never reach the controller (it would read as
/// an extreme over-cool).
fn plausible(t: Option<f32>) -> Option<f32> {
    t.filter(|v| *v > 0.0 && *v < 150.0)
}

impl NvapiBackend {
    /// Resolve (and cache) the ThermChannel LUT for a GPU. `None` when the
    /// card does not expose the channel pair (pre-Pascal) — callers then
    /// fall back to the legacy sensors only.
    fn channel_lut(&mut self, gpu_index: usize) -> Option<ThermalChannelLut> {
        if gpu_index >= self.channel_luts.len() {
            return None;
        }
        if self.channel_luts[gpu_index].is_none() {
            let lut = self.gpus.get(gpu_index).and_then(|g| {
                let info = g.inner().thermal_channel_info().ok()?;
                // `primary` is indexed by channel type:
                // 0=GPU_AVG(core), 1=GPU_MAX(hotspot), 2=BOARD, 3=MEMORY.
                Some(ThermalChannelLut {
                    mask: info.channel_mask,
                    core: info.primary.first().copied().flatten(),
                    hotspot: info.hotspot_index(),
                    board: info.primary.get(2).copied().flatten(),
                    memory: info.memory_index(),
                })
            });
            self.channel_luts[gpu_index] = lut;
        }
        self.channel_luts[gpu_index]
    }
}

fn offset_domain_bit(domain: OffsetDomain) -> u32 {
    match domain {
        OffsetDomain::Core => 0, // Graphics
        OffsetDomain::Mem => 2,  // Memory
    }
}

fn offset_clock_domain(domain: OffsetDomain) -> ClockDomain {
    match domain {
        OffsetDomain::Core => ClockDomain::Graphics,
        OffsetDomain::Mem => ClockDomain::Memory,
    }
}

impl ControlBackend for NvapiBackend {
    fn read_temps(&mut self, gpu_index: usize) -> Result<SensorBundle, String> {
        let target = self.target(gpu_index)?;
        let report = run_gpu_operation(&target, QueryNvapiThermalSettings)
            .map_err(|e| format!("GPU {gpu_index}: thermal read: {e}"))?;
        let mut bundle = SensorBundle::default();
        for s in report.output {
            match s.target {
                ThermalTarget::Gpu => bundle.core_c = Some(s.current_c as f32),
                ThermalTarget::Memory => bundle.memory_c = Some(s.current_c as f32),
                ThermalTarget::Board => bundle.board_c = Some(s.current_c as f32),
                _ => {}
            }
        }

        // Fine-grained overlay: ThermChannel decodes at 1/256 °C while the
        // legacy view above is integer-only. Channels a card does not
        // populate keep the legacy reading.
        if let Some(lut) = self.channel_lut(gpu_index) {
            let status = self
                .gpus
                .get(gpu_index)
                .and_then(|g| g.inner().thermal_channel_status(lut.mask).ok());
            if let Some(status) = status {
                if let Some(v) = lut.core.and_then(|i| plausible(status.get(i as usize))) {
                    bundle.core_c = Some(v);
                }
                if let Some(v) = lut.hotspot.and_then(|i| plausible(status.get(i as usize))) {
                    bundle.hotspot_c = Some(v);
                }
                if let Some(v) = lut.memory.and_then(|i| plausible(status.get(i as usize))) {
                    bundle.memory_c = Some(v);
                }
                if let Some(v) = lut.board.and_then(|i| plausible(status.get(i as usize))) {
                    bundle.board_c = Some(v);
                }
            }
        }
        Ok(bundle)
    }

    fn read_fan(&mut self, gpu_index: usize) -> Option<FanReading> {
        let target = self.target(gpu_index).ok()?;
        let report = run_gpu_operation(&target, QueryFanInfo).ok()?;
        Some(FanReading {
            percent: report.output.current_speed,
        })
    }

    fn read_power_watts(&mut self, gpu_index: usize) -> Option<f32> {
        let id = self.gpu_id(gpu_index).ok()?;
        nvoc_core::nvml::query_nvml_power_draw_watts(&self.nvml, id.0)
    }

    fn read_core_clock_mhz(&mut self, gpu_index: usize) -> Option<f32> {
        let gpu = self.gpus.get(gpu_index)?;
        let clocks = gpu
            .inner()
            .clock_frequencies(ClockFrequencyType::Current)
            .ok()?;
        clocks
            .get(&ClockDomain::Graphics)
            .map(|k| k.0 as f32 / 1000.0)
    }

    fn read_freq_ceiling_mhz(&mut self, gpu_index: usize) -> Option<f32> {
        let gpu = self.gpus.get(gpu_index)?;
        let status = gpu.status().ok()?;
        let vfp = status.vfp?;
        vfp.graphics
            .values()
            .map(|p| p.frequency.0)
            .max()
            .map(|khz| khz as f32 / 1000.0)
            .filter(|mhz| *mhz > 0.0)
    }

    fn write_freq_cap_khz(&mut self, gpu_index: usize, cap_khz: u32) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        match run_gpu_operation(
            &target,
            SetVfpFrequencyLock {
                domain: ClockDomain::Graphics,
                upper: Kilohertz(cap_khz),
                lower: None,
            },
        ) {
            Ok(_) => Ok(()),
            Err(e) if e.is_allowable_nvapi_reset_error() => {
                warn!(
                    "GPU {gpu_index}: NDA frequency lock rejected ({e}); \
                     falling back to NVML locked clocks"
                );
                run_gpu_operation(
                    &target,
                    SetLockedClocks {
                        domain: ClockDomain::Graphics,
                        min_mhz: 0,
                        max_mhz: cap_khz / 1000,
                    },
                )
                .map(|_| ())
                .map_err(|e| format!("GPU {gpu_index}: NVML locked clocks: {e}"))
            }
            Err(e) => Err(format!("GPU {gpu_index}: frequency lock: {e}")),
        }
    }

    fn read_monitor(&mut self, gpu_index: usize) -> Result<MonitorSample, String> {
        let mut m = MonitorSample::default();

        // NVML: util / power / mem clock / fan / (p-state fallback).
        if let Ok(device) = self.nvml.device_by_index(gpu_index as u32) {
            if let Ok(util) = device.utilization_rates() {
                m.util_pct = Some(util.gpu as f32);
            }
            if let Ok(mw) = device.power_usage() {
                m.power_w = Some(mw as f32 / 1000.0);
            }
            if let Ok(mhz) = device.clock(NvmlClockType::Memory, ClockId::Current) {
                m.mem_clock_mhz = Some(mhz as f32);
            }
            if let Ok(pct) = device.fan_speed(0) {
                m.fan_pct = Some(pct);
            }
            if let Ok(ps) = device.performance_state() {
                m.pstate = Some(format!("{ps:?}"));
            }
        }

        // NVAPI single status call: voltage / core clock / core temp /
        // authoritative p-state.
        let gpu = self.gpus.get(gpu_index).ok_or("GPU index out of range")?;
        if let Ok(status) = gpu.status() {
            m.volt_mv = status.voltage.map(|v| v.0 as f32 / 1000.0);
            m.pstate = Some(format!("{}", status.pstate));
            m.core_clock_mhz = status
                .clocks
                .get(&ClockDomain::Graphics)
                .map(|k| k.0 as f32 / 1000.0);
            m.temp_c = status.sensors.first().map(|(_, t)| *t);
        }
        Ok(m)
    }

    fn read_gpu_info_json(&mut self, gpu_index: usize) -> Result<serde_json::Value, String> {
        let gpu = self.gpus.get(gpu_index).ok_or("GPU index out of range")?;
        let info = gpu
            .info()
            .map_err(|e| format!("GPU {gpu_index}: info: {e}"))?;
        serde_json::to_value(&info).map_err(|e| format!("serialize info: {e}"))
    }

    fn read_vf_curve(&mut self, gpu_index: usize) -> Result<Vec<(f32, f32)>, String> {
        let gpu = self.gpus.get(gpu_index).ok_or("GPU index out of range")?;
        let status = gpu
            .status()
            .map_err(|e| format!("GPU {gpu_index}: status: {e}"))?;
        let vfp = status
            .vfp
            .ok_or_else(|| "VFP table unavailable".to_string())?;
        let mut pts: Vec<(f32, f32)> = vfp
            .graphics
            .values()
            .map(|p| (p.voltage.0 as f32 / 1000.0, p.frequency.0 as f32 / 1000.0))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(pts)
    }

    fn read_offset_mhz(
        &mut self,
        gpu_index: usize,
        domain: OffsetDomain,
        backend_kind: OffsetBackend,
    ) -> Result<i32, String> {
        match backend_kind {
            OffsetBackend::Nvml => {
                let target = self.target(gpu_index)?;
                let report = run_gpu_operation(
                    &target,
                    QueryClockOffset {
                        domain: offset_clock_domain(domain),
                        pstate: PerformanceState::Zero,
                    },
                )
                .map_err(|e| format!("GPU {gpu_index}: offset read: {e}"))?;
                Ok(report.output.mhz)
            }
            OffsetBackend::Nvapi => {
                let gpu = self.gpus.get(gpu_index).ok_or("GPU index out of range")?;
                let control = gpu
                    .clk_domains_control()
                    .map_err(|e| format!("GPU {gpu_index}: clk domains: {e}"))?
                    .ok_or_else(|| "clk domains not populated".to_string())?;
                let bit = offset_domain_bit(domain);
                control
                    .entries
                    .iter()
                    .find(|e| e.bit == bit)
                    .map(|e| (e.values_kHz[0] as f32 / 1000.0).round() as i32)
                    .ok_or_else(|| format!("GPU {gpu_index}: clk domain bit {bit} not populated"))
            }
        }
    }

    fn read_power_limit_w(&mut self, gpu_index: usize) -> Result<Option<(u32, u32, u32)>, String> {
        let device = self
            .nvml
            .device_by_index(gpu_index as u32)
            .map_err(|e| format!("GPU {gpu_index}: NVML: {e:?}"))?;
        let current = device
            .power_management_limit()
            .map_err(|e| format!("GPU {gpu_index}: power limit: {e:?}"))?;
        let bounds = device.power_management_limit_constraints().ok();
        Ok(Some((
            bounds.as_ref().map(|b| b.min_limit).unwrap_or(current),
            current,
            bounds.as_ref().map(|b| b.max_limit).unwrap_or(current),
        )))
    }

    fn read_temp_limit_c(&mut self, gpu_index: usize) -> Result<Option<(i32, i32, i32)>, String> {
        let device = self
            .nvml
            .device_by_index(gpu_index as u32)
            .map_err(|e| format!("GPU {gpu_index}: NVML: {e:?}"))?;
        let slow = device
            .temperature_threshold(TemperatureThreshold::Slowdown)
            .map_err(|e| format!("GPU {gpu_index}: slowdown: {e:?}"))? as i32;
        // NVML exposes no writable-temp window; offer a band around slowdown.
        Ok(Some((slow.saturating_sub(15), slow, slow)))
    }

    fn write_offset_mhz(
        &mut self,
        gpu_index: usize,
        domain: OffsetDomain,
        backend_kind: OffsetBackend,
        mhz: i32,
    ) -> Result<(), String> {
        match backend_kind {
            // NVML: MHz on P0 via SetClockOffset.
            OffsetBackend::Nvml => {
                let target = self.target(gpu_index)?;
                run_gpu_operation(
                    &target,
                    SetClockOffset {
                        domain: offset_clock_domain(domain),
                        pstate: PerformanceState::Zero,
                        mhz,
                    },
                )
                .map(|_| ())
                .map_err(|e| format!("GPU {gpu_index}: offset write: {e}"))
            }
            // NVAPI: private ClkDomains SetControl — kHz at slot 0.
            OffsetBackend::Nvapi => {
                let target = self.target(gpu_index)?;
                run_gpu_operation(
                    &target,
                    SetNvapiClkDomainOffset {
                        domain_bit: offset_domain_bit(domain),
                        offset_kHz: mhz.saturating_mul(1000),
                        slot: 0,
                        temporary: false,
                    },
                )
                .map(|_| ())
                .map_err(|e| format!("GPU {gpu_index}: offset write: {e}"))
            }
        }
    }

    fn write_power_limit_w(&mut self, gpu_index: usize, watts: u32) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        run_gpu_operation(&target, SetPowerLimit { watts })
            .map(|_| ())
            .map_err(|e| format!("GPU {gpu_index}: power write: {e}"))
    }

    fn write_temp_limit_c(&mut self, gpu_index: usize, celsius: i32) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        run_gpu_operation(&target, SetTemperatureLimit { celsius })
            .map(|_| ())
            .map_err(|e| format!("GPU {gpu_index}: temp write: {e}"))
    }

    fn reset_offset(&mut self, gpu_index: usize, domain: OffsetDomain) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        run_gpu_operation(
            &target,
            ResetPstateGlobalFreqOffset {
                offsets: vec![(PState::P0, offset_clock_domain(domain))],
            },
        )
        .map(|_| ())
        .map_err(|e| format!("GPU {gpu_index}: offset reset: {e}"))
    }

    fn reset_power_limit(&mut self, gpu_index: usize) -> Result<(), String> {
        let device = self
            .nvml
            .device_by_index(gpu_index as u32)
            .map_err(|e| format!("GPU {gpu_index}: NVML: {e:?}"))?;
        let default_w = device
            .power_management_limit_default()
            .map_err(|e| format!("GPU {gpu_index}: default limit: {e:?}"))?;
        let target = self.target(gpu_index)?;
        run_gpu_operation(&target, SetPowerLimit { watts: default_w })
            .map(|_| ())
            .map_err(|e| format!("GPU {gpu_index}: power reset: {e}"))
    }

    fn reset_temp_limit(&mut self, gpu_index: usize) -> Result<(), String> {
        // NVML thresholds are not restore-able per-call; write the slowdown
        // threshold back as the sensible default.
        let device = self
            .nvml
            .device_by_index(gpu_index as u32)
            .map_err(|e| format!("GPU {gpu_index}: NVML: {e:?}"))?;
        let slow = device
            .temperature_threshold(TemperatureThreshold::Slowdown)
            .map_err(|e| format!("GPU {gpu_index}: slowdown: {e:?}"))?;
        let target = self.target(gpu_index)?;
        run_gpu_operation(
            &target,
            SetTemperatureLimit {
                celsius: slow as i32,
            },
        )
        .map(|_| ())
        .map_err(|e| format!("GPU {gpu_index}: temp reset: {e}"))
    }

    fn restore_freq_auto(&mut self, gpu_index: usize) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        match run_gpu_operation(
            &target,
            ResetVfpFrequencyLock {
                domain: ClockDomain::Graphics,
            },
        ) {
            Ok(_) => {
                info!("GPU {gpu_index}: frequency lock cleared");
                Ok(())
            }
            Err(e) if e.is_allowable_nvapi_reset_error() => {
                warn!(
                    "GPU {gpu_index}: NDA frequency-lock reset rejected ({e}); \
                     falling back to NVML reset"
                );
                run_gpu_operation(
                    &target,
                    ResetFreqLock {
                        domain: ClockDomain::Graphics,
                    },
                )
                .map(|_| ())
                .map_err(|e| format!("GPU {gpu_index}: NVML freq reset: {e}"))
            }
            Err(e) => Err(format!("GPU {gpu_index}: frequency unlock: {e}")),
        }
    }

    fn write_fan_percent(&mut self, gpu_index: usize, percent: u32) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        match run_gpu_operation(
            &target,
            SetFanPercent {
                cooler_index: None,
                percent: Some(percent),
            },
        ) {
            Ok(_) => Ok(()),
            Err(e) if e.is_allowable_nvapi_reset_error() => {
                warn!(
                    "GPU {gpu_index}: NDA fan surface rejected ({e}); falling back to NVML manual pin"
                );
                run_gpu_operation(
                    &target,
                    SetFanSpeed {
                        fan_index: 0,
                        policy: FanControlPolicy::Manual,
                        level: percent,
                    },
                )
                .map(|_| ())
                .map_err(|e| format!("GPU {gpu_index}: NVML fan write: {e}"))
            }
            Err(e) => Err(format!("GPU {gpu_index}: fan write: {e}")),
        }
    }

    fn restore_fan_auto(&mut self, gpu_index: usize) -> Result<(), String> {
        let target = self.target(gpu_index)?;
        match run_gpu_operation(&target, ResetNvapiFanControl) {
            Ok(_) => {
                info!("GPU {gpu_index}: fan control restored to driver curve");
                Ok(())
            }
            Err(e) if e.is_allowable_nvapi_reset_error() => {
                warn!(
                    "GPU {gpu_index}: NDA fan reset rejected ({e}); falling back to NVML default"
                );
                run_gpu_operation(&target, ResetFanSpeed { fan_index: 0 })
                    .map(|_| ())
                    .map_err(|e| format!("GPU {gpu_index}: NVML fan reset: {e}"))
            }
            Err(e) => Err(format!("GPU {gpu_index}: fan reset: {e}")),
        }
    }
}
