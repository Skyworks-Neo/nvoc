use clap::{
    Arg, ArgAction, ColorChoice, Command as ClapCommand,
    builder::{PossibleValue, PossibleValuesParser},
};
use nvoc_core::{
    BackendSet, ClearEdid, ClkVfDomainHint, ClockDomain, ConvertEnum, CoolerPolicy,
    CoolerTarget, FanCurvePointReadout, GetFanCurves, GetPowerMode, GpuSelector, GpuTarget,
    Kilohertz, KilohertzDelta, MicrovoltsDelta, NvapiPerfFreqCap, OemOcScanner, OemOcScannerAction,
    PState, Percentage, QueryApiRestriction, QueryAutoBoost, QueryClockOffset,
    QueryDisplays, QueryDomainVfpPoints, QueryEdid, QueryFanInfo, QueryGpuInfo, QueryGpuSettings,
    QueryGpuStatus, QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta,
    QueryNvapiClkDomainFreqDetail, QueryNvapiClkDomainFreqsBatch, QueryNvapiClkDomains,
    QueryNvapiClkVfPoints, QueryNvapiDNotifier, QueryNvapiPStateLevels, QueryNvapiPStateLockStatus,
    QueryNvapiTargetTempPolicies, QueryNvapiTargetTempPolicyIndex, QueryNvapiTgpWattRange,
    QueryNvapiThermalSettings, QueryNvapiVoltRails, QueryPowerLimits, QueryPstateBaseVoltage,
    QueryNvapiPowerMizer, QueryNvapiCoreVoltageControl,
    SetNvapiCoreVoltageControl, QueryNvapiPmgrVoltageArbiter, SetNvapiPmgrVoltageArbiter,
    QueryNvapiRatedTdp, SetNvapiBackgroundOcScanner, QueryNvapiOcScannerIncomplete,
    QueryNvapiThermalSim, SetNvapiThermalSim, DisableNvapiThermalSim, SetNvapiPerfLevelLock,
    QueryPstates, QuerySupportedApplicationsClocks, QueryTdpTempLimits, QueryTemperatureThresholds,
    QueryThrottleReasons, QueryViolationStatus, QueryVoltageBoost,
    ResetLegacyApplicationFreqLock, ResetCoolerLevels, ResetFanSpeed, ResetForcePstate, ResetFreqLock,
    ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetNvapiTgpWatt, ResetNvapiVfpPrivate,
    ResetLegacyGpcRailOvervoltLimit, ResetPstateGlobalFreqOffset, ResetPublicVftableOffset, ResetVfpFrequencyLock,
    ResetPublicVftableGpcLock,
    RestartDisplayDriver, SetAutoboostSupport, SetApplicationsClocks, SetAutoboostStatus,
    ResetAutoboostStatus, SetBb2Active, SetClockOffset, SetCoolerLevels, SetEdid, SetFanCurve,
    ResetFanCurve, SetFanStop, SetFanRpm, QueryNvapiCoolerInfo,
    SetFanSpeed, SetForcePstate, SetLegacyClocks, SetLockedClocks, SetNvapiClkDomainOffset,
    SetNvapiDNotifier, SetNvapiDynamicBoost, SetNvapiOvervolt, SetNvapiPStateNative,
    SetNvapiPerfFreqCap, SetNvapiPowerLimits, SetNvapiPstateLock, SetNvapiSensorLimits,
    SetNvapiTargetTemp, SetNvapiTgpWatt, SetNvapiVfpPointPrivate, SetNvapiVfpRangePerPointPrivate,
    SetNvapiVfpRangePrivate, SetNvapiVoltRailOffset, SetNvapiVoltRailTarget, SetNvmlPstateLock,
    SetNvmlAcousticTemp, SetPowerLimit as SetNvmlPowerLimit, SetPowerMode, SetPstateBaseVoltage, SetPstateClockOffset,
    SetTemperatureLimit,
    SetVfpFrequencyLock, SetPublicVftablePointOffset, SetPublicVftableRangeOffset, SetGpcVoltLock, SetVoltageBoost,
    SetWm2Active, SetWm2Mode, VfpResetDomain, Wm2AcousticMode, discover_targets,
    nvml_pstate_to_str, parse_nvapi_locked_voltage_target, parse_nvml_fan_control_policy,
    parse_nvml_pstate, run, select_targets,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

mod output;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub enum CliError {
    Message(String),
    Clap(clap::Error),
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn print_clap(&self) -> bool {
        if let Self::Clap(err) = self {
            let _ = err.print();
            true
        } else {
            false
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Message(_) => 2,
            Self::Clap(err) => err.exit_code(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Clap(err) if err.kind() == clap::error::ErrorKind::ArgumentConflict => {
                write!(f, "argument conflicts: {err}")
            }
            Self::Clap(err) => write!(f, "{err}"),
        }
    }
}

impl StdError for CliError {}

impl From<nvoc_core::Error> for CliError {
    fn from(value: nvoc_core::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(value: serde_json::Error) -> Self {
        Self::new(value.to_string())
    }
}

type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Auto,
    Nvapi,
    Nvml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendAdapter {
    Nvapi,
    Nvml,
}

impl BackendAdapter {
    fn label(self) -> &'static str {
        match self {
            Self::Nvapi => "nvapi",
            Self::Nvml => "nvml",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    GetGpuList,
    GetDisplayList,
    GetInfo,
    GetUuid,
    GetStatus,
    GetSettings,
    GetPublicVftable,
    GetPowerLimit,
    GetPstateGlobalFreqOffset,
    GetPstateFreqRange,
    GetSupportedLegacyApplicationFreq,
    GetFanInfo,
    GetFanCurve,
    SetFanCurve,
    ResetFanCurveCmd,
    SetFanstopStatus,
    GetTemperatureThresholds,
    GetLegacyTempSensor,
    GetPowerMode,
    SetPowerMode,
    GetThrottleReasons,
    GetPublicPowerLimit,
    GetPublicTempLimit,
    GetLegacyOvervoltRanges,
    GetLegacyP0CoreMaxVoltageDelta,
    GetLegacyGpcRailOvervoltLimit,
    GetPublicGpcRailVoltBoost,
    GetAutoboostStatus,
    GetAutoboostSupport,
    GetEdid,
    SetPstateGlobalFreqOffset,
    SetPublicTgpPercent,
    SetPpabStatus,
    SetPowerLimit,
    ResetPowerLimit,
    GetDNotifier,
    SetDNotifier,
    GetVoltRailInfo,
    SetVoltRailLimit,
    GetPowerMizer,
    GetCoreVoltageControl,
    SetCoreVoltageControl,
    GetPmgrArbiter,
    SetPmgrArbiter,
    GetRatedTdp,
    GetPrivateFreqDomainInfo,
    GetPrivateFreqDomainStatus,
    SetPrivateFreqDomainGlobalOffset,
    SetGpuClock,
    ResetGpuClock,
    SetPrivateVftablePointOffset,
    SetPrivateVftableRangeOffset,
    GetPrivateVftable,
    SetPrivatePermanentPstateLockUser,
    GetTempSim,
    SetTempSim,
    ResetTempSim,
    SetPrivateTargetTempLimit,
    SetTempLimit,
    SetFanSpeed,
    SetFreqLock,
    SetGpcVoltLock,
    OemOcScanner,
    SetPrivateForcedPstateLockUser,
    ResetPrivateForcedPstateLockUser,
    RestartDisplayDriver,
    SetBatteryBoost2Status,
    SetWhisperMode2Status,
    SetPublicVftablePointOffset,
    SetPublicVftableRangeOffset,
    SetPstateLockViaMemRange,
    GetPStateLock,
    SetPStateLock,
    ResetPStateLock,
    SetLegacyApplicationFreqLock,
    SetLegacyGpcRailOvervoltLimit,
    SetOvervoltUv,
    SetPublicGpcRailVoltBoost,
    SetAutoboostStatus,
    ResetAutoboostStatus,
    SetAutoboostSupport,
    SetEdid,
    ClearEdid,
    SetLegacyFreq,
    ResetLegacyApplicationFreqLock,
    ResetFreqLock,
    ResetFanSpeed,
    ResetPublicVftableOffset,
    ResetPublicVftableGpcLock,
    ResetPrivateVftableOffset,
    ResetPublicTgpPercent,
    ResetTempLimit,
    ResetLegacyGpcRailOvervoltLimit,
    ResetPstateGlobalFreqOffset,
    ResetPublicGpcRailVoltBoost,
}

static NVAPI_ONLY: [BackendAdapter; 1] = [BackendAdapter::Nvapi];
static NVML_ONLY: [BackendAdapter; 1] = [BackendAdapter::Nvml];
static BOTH_BACKENDS: [BackendAdapter; 2] = [BackendAdapter::Nvapi, BackendAdapter::Nvml];

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Self::GetGpuList => "get-gpu-list",
            Self::GetDisplayList => "get-display-list",
            Self::GetInfo => "get-info",
            Self::GetUuid => "get-uuid",
            Self::GetStatus => "get-status",
            Self::GetSettings => "get-settings",
            Self::GetPublicVftable => "get-public-vftable",
            Self::GetPowerLimit => "get-power-limit",
            Self::GetPstateGlobalFreqOffset => "get-pstate-global-freq-offset",
            Self::GetPStateLock => "get-pstate-lock",
            Self::SetPStateLock => "set-pstate-lock",
            Self::GetPstateFreqRange => "get-pstate-freq-range",
            Self::GetSupportedLegacyApplicationFreq => "get-supported-legacy-application-freq",
            Self::GetFanInfo => "get-fan-info",
            Self::GetFanCurve => "get-fan-curve",
            Self::SetFanCurve => "set-fan-curve",
            Self::ResetFanCurveCmd => "reset-fan-curve",
            Self::SetFanstopStatus => "set-fanstop-status",
            Self::GetTemperatureThresholds => "get-temp-thresholds",
            Self::GetLegacyTempSensor => "get-legacy-temp-sensor",
            Self::GetPowerMode => "get-power-mode",
            Self::SetPowerMode => "set-power-mode",
            Self::GetThrottleReasons => "get-throttle-reasons",
            Self::GetPublicPowerLimit => "get-public-power-limit",
            Self::GetPublicTempLimit => "get-public-temp-limit",
            Self::GetLegacyOvervoltRanges => "get-legacy-overvolt-ranges",
            Self::GetLegacyP0CoreMaxVoltageDelta => "get-legacy-p0-core-max-voltage-delta",
            Self::GetLegacyGpcRailOvervoltLimit => "get-legacy-gpc-rail-overvolt-limit",
            Self::GetPublicGpcRailVoltBoost => "get-public-gpc-rail-volt-boost",
            Self::GetAutoboostStatus => "get-autoboost-status",
            Self::GetAutoboostSupport => "get-autoboost-support",
            Self::GetEdid => "get-edid",
            Self::SetPstateGlobalFreqOffset => "set-pstate-global-freq-offset",
            Self::SetPublicTgpPercent => "set-public-tgp-percent",
            Self::SetPpabStatus => "set-ppab-status",
            Self::SetPowerLimit => "set-power-limit",
            Self::ResetPowerLimit => "reset-power-limit",
            Self::GetDNotifier => "get-dnotifier",
            Self::SetDNotifier => "set-dnotifier",
            Self::GetVoltRailInfo => "get-volt-rail-info",
            Self::SetVoltRailLimit => "set-volt-rail-limit",
            Self::GetPowerMizer => "get-power-mizer",
            Self::GetCoreVoltageControl => "get-core-voltage-control",
            Self::SetCoreVoltageControl => "set-core-voltage-control",
            Self::GetPmgrArbiter => "get-pmgr-arbiter",
            Self::SetPmgrArbiter => "set-pmgr-arbiter",
            Self::GetRatedTdp => "get-rated-tdp",
            Self::GetPrivateFreqDomainInfo => "get-private-freq-domain-info",
            Self::GetPrivateVftable => "get-private-vftable",
            Self::SetPrivatePermanentPstateLockUser => "set-private-permanent-pstate-lock-user",
            Self::GetTempSim => "get-temp-sim",
            Self::SetTempSim => "set-temp-sim",
            Self::ResetTempSim => "reset-temp-sim",
            Self::SetPrivateVftablePointOffset => "set-private-vftable-point-offset",
            Self::SetPrivateVftableRangeOffset => "set-private-vftable-range-offset",
            Self::GetPrivateFreqDomainStatus => "get-private-freq-domain-status",
            Self::SetPrivateFreqDomainGlobalOffset => "set-private-freq-domain-global-offset",
            Self::SetGpuClock => "set-perf-freq-caps",
            Self::ResetGpuClock => "reset-perf-freq-caps",
            Self::SetTempLimit => "set-temp-limit",
            Self::SetPrivateTargetTempLimit => "set-private-target-temp-limit",
            Self::SetFanSpeed => "set-fan-speed",
            Self::SetFreqLock => "set-freq-lock",
            Self::SetGpcVoltLock => "set-gpc-volt-lock",
            Self::OemOcScanner => "oem-oc-scanner",
            Self::SetPrivateForcedPstateLockUser => "set-private-forced-pstate-lock-user",
            Self::ResetPrivateForcedPstateLockUser => "reset-private-forced-pstate-lock-user",
            Self::RestartDisplayDriver => "restart-display-driver",
            Self::SetBatteryBoost2Status => "set-batteryboost2-status",
            Self::SetWhisperMode2Status => "set-whispermode2-status",
            Self::SetPublicVftablePointOffset => "set-public-vftable-point-offset",
            Self::SetPublicVftableRangeOffset => "set-public-vftable-range-offset",
            Self::SetPstateLockViaMemRange => "set-pstate-lock-via-mem-range",
            Self::SetLegacyApplicationFreqLock => "set-legacy-application-freq-lock",
            Self::SetLegacyGpcRailOvervoltLimit => "set-legacy-gpc-rail-overvolt-limit",
            Self::SetOvervoltUv => "set-overvolt-uv",
            Self::SetPublicGpcRailVoltBoost => "set-public-gpc-rail-volt-boost",
            Self::SetAutoboostStatus => "set-autoboost-status",
            Self::ResetAutoboostStatus => "reset-autoboost-status",
            Self::SetAutoboostSupport => "set-autoboost-support",
            Self::SetEdid => "set-edid",
            Self::ClearEdid => "clear-edid",
            Self::SetLegacyFreq => "set-legacy-freq",
            Self::ResetLegacyApplicationFreqLock => "reset-legacy-application-freq-lock",
            Self::ResetFreqLock => "reset-freq-lock",
            Self::ResetFanSpeed => "reset-fan-speed",
            Self::ResetPublicVftableOffset => "reset-public-vftable-offset",
            Self::ResetPublicVftableGpcLock => "reset-public-vftable-gpc-lock",
            Self::ResetPrivateVftableOffset => "reset-private-vftable-offset",
            Self::ResetPublicTgpPercent => "reset-public-tgp-percent",
            Self::ResetTempLimit => "reset-temp-limit",
            Self::ResetLegacyGpcRailOvervoltLimit => "reset-legacy-gpc-rail-overvolt-limit",
            Self::ResetPStateLock => "reset-pstate-lock",
            Self::ResetPstateGlobalFreqOffset => "reset-pstate-global-freq-offset",
            Self::ResetPublicGpcRailVoltBoost => "reset-public-gpc-rail-volt-boost",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Self::GetGpuList => "List discovered GPUs and available backends",
            Self::GetDisplayList => "List NVAPI display IDs for EDID operations",
            Self::GetInfo => "Read NVAPI GPU identity and capability information",
            Self::GetUuid => "Read GPU UUID",
            Self::GetStatus => "Read NVAPI live GPU status",
            Self::GetSettings => "Read NVAPI overclock settings",
            Self::GetPublicVftable => {
                "Read the public V-F curve table: default dumps all domains (graphics points plus the trailing memory entries, e.g. index 127..131 on 30/40 series); --domain gpc|memory narrows to one segment"
            }
            Self::GetPowerLimit => "Read power limits in watts: NVML min/current/max by default; falls back to the NVAPI TGP-watts range (min/default/max) where NVML is unsupported",
            Self::GetPstateGlobalFreqOffset => "Read clock offset in MHz",
            Self::GetPStateLock => "Read the native NVAPI P-State level table",
            Self::SetPStateLock => "Lock the native NVAPI P-State",
            Self::ResetPStateLock => "Clear all native NVAPI P-State locks",
            Self::GetPstateFreqRange => "Read NVML P-State clock ranges",
            Self::GetSupportedLegacyApplicationFreq => "Read NVML supported application clocks",
            Self::GetFanInfo => "Read fan/cooler info (NVML: count + min/max percent; NVAPI: per-cooler info via private FanCoolerGetInfo)",
            Self::GetFanCurve => {
                "Read the NVAPI fan-curve table (ClientFanPolicies, struct 0x200DC; desktop-only)"
            }
            Self::SetFanCurve => {
                "Write one fan-curve slot (RMW: --curve idx --points temp:rpm,temp:rpm,temp:rpm)"
            }
            Self::ResetFanCurveCmd => {
                "Reset one fan-curve slot to factory (GPUMon's private FanPolicy path 0x2B2A2A45; works where restore-fan/cooler-settings is NOT_SUPPORTED, e.g. desktop 3060/2070)"
            }
            Self::SetFanstopStatus => {
                "Toggle fan stop / zero-RPM for a curve slot (FanArbiterSet NDA 0x44CD3014): on | off"
            }
            Self::GetTemperatureThresholds => {
                "Read temperature thresholds (NVML by default; --nvapi exposes target-temp policy)"
            }
            Self::GetLegacyTempSensor => {
                "Read NVAPI legacy 3-sensor thermal view (GPU/Memory/Board, live + physical range)"
            }
            Self::GetPowerMode => "Read NVIDIA App power mode (Balanced/Max with support gate)",
            Self::SetPowerMode => {
                "Set NVIDIA App power mode: max | balanced (the App's Balanced/Max toggle)"
            }
            Self::GetThrottleReasons => "Read NVML throttle reasons",
            Self::GetPublicPowerLimit => {
                "Read the NVAPI public power-limit range (TDP min/default/max percent, ClientPowerPolicies)"
            }
            Self::GetPublicTempLimit => {
                "Read the NVAPI public temp-limit range (min/default/max Celsius + throttle curve)"
            }
            Self::GetLegacyOvervoltRanges => "Read NVAPI legacy core overvolt ranges",
            Self::GetLegacyP0CoreMaxVoltageDelta => "Read NVAPI legacy P0 max voltage delta",
            Self::GetLegacyGpcRailOvervoltLimit => "Read NVAPI P-State base voltage delta in microvolts",
            Self::GetPublicGpcRailVoltBoost => "Read NVAPI voltage boost percent",
            Self::GetAutoboostStatus => "Read NVML auto-boost state",
            Self::GetAutoboostSupport => "Read NVML API restriction state",
            Self::GetEdid => "Read display EDID through NVAPI",
            Self::SetPstateGlobalFreqOffset => "Set clock offset in MHz for any clock domain",
            Self::SetPublicTgpPercent => "Set NVAPI power limit in percent",
            Self::SetPpabStatus => "Set NVAPI PPAB / Dynamic-Boost enable (on/off)",
            Self::SetPowerLimit => "Set TGP in watts: NVAPI path writes the mobile TGP slider (ClientPowerPolicies, --policy-index); NVML path writes the power-management limit (nvidia-smi -pl). Auto prefers NVAPI",
            Self::ResetPowerLimit => "Reset NVAPI TGP to rated/default (mobile)",
            Self::GetDNotifier => {
                "Read NVAPI D-Notifier (D0-notify) level + D1-D5 power-cap table (mobile)"
            }
            Self::SetDNotifier => {
                "Set NVAPI D-Notifier limit level (D1-D5; shares the TGP power-policy table)"
            }
            Self::GetVoltRailInfo => {
                "Read private VoltRails family: rail mask + per-rail offsets + live voltages (melonVolt path)"
            }
            Self::SetVoltRailLimit => {
                "Set a volt-rail limit: --offset (default) writes a uV offset (melonVolt write path; 5090 MSVDD = rail 1 type 3); --target takes an absolute mV target and derives the uV offset from the live control/status snapshot"
            }
            Self::GetPowerMizer => "Read the PowerMizer mode (NVCP power dropdown readback, 0x76BFA16B; returns 6/7)",
            Self::GetCoreVoltageControl => "Read the core-voltage control object (0xA91F88EB, escape 0x07000045)",
            Self::SetCoreVoltageControl => "Set the core-voltage control (0xDC2BD4A6, escape 0x07000044; admin; distinct from volt-rail paths)",
            Self::GetPmgrArbiter => "Read the PMGR voltage-request arbiter values (0x717648FD, escape 0x0700019F)",
            Self::SetPmgrArbiter => "Set the PMGR voltage-request arbiter values (0x9C4BB8D0; admin; GET-patch-SET RMW recommended)",
            Self::GetRatedTdp => "Rated-TDP readback trio (0xED2BEA09/0x87BD35EF/0xFCBDF642)",
            Self::GetPrivateFreqDomainInfo => {
                "Read the private ClockClient domain-control block: controllable mask + per-domain offset/range records (XBar physical-clock path)"
            }
            Self::GetPrivateFreqDomainStatus => {
                "Measure one clock domain's physical clock via two-sample MEASURE_FREQ (XBar=1, GPC=0, SYS=2, MCLK=4)"
            }
            Self::SetPrivateFreqDomainGlobalOffset => {
                "Write a signed kHz offset into one clock-domain control record (dangerous XBar clock write; --temporary restores the snapshot)"
            }
            Self::SetGpuClock => {
                "Set the GPU frequency perf-cap in MHz (PerfLimitsSetStatus NDA; clamp perf max/min freq; --min for the lower bound, default both bounds equal). Use reset-perf-freq-caps to clear"
            }
            Self::ResetGpuClock => {
                "Clear the GPU frequency perf-cap (PerfLimitsSetStatus NDA, enable=0 on both entries; the GPUMonCmd -gpuclk:-1 path)"
            }
            Self::GetPrivateVftable => {
                "Read the private ClockClient V/F-points family: per-bank point masks + V/F curve records (voltage-indexed, units calibrated vs the public GPC VFP)"
            }
            Self::SetPrivatePermanentPstateLockUser => {
                "Admin-free pstate lock (SetPerfLevel 0x75DD3E6A, escape 0x7000040): level is an INDEX into the GPU's real available P-State list (see get-pstate-lock) — NOT a fixed P8..P0 enum and NOT the NVCP power-mode dropdown. No release value exists (only valid indices accepted); the lock survives reset-private-forced-pstate-lock-user/reset-pstate-lock and only a reboot/driver reload clears it; re-locking re-targets"
            }
            Self::GetTempSim => {
                "Read the temperature-simulation state (GetThermalSimulationMode; Secured-Overrides 'Temp faking allowed' gated)"
            }
            Self::SetTempSim => {
                "Fake the driver-visible GPU temperature in Celsius (DANGEROUS research tool; Extended->basic fallback; Secured-Overrides gated)"
            }
            Self::ResetTempSim => {
                "Disable temperature simulation and restore the real sensor reading"
            }
            Self::SetPrivateVftablePointOffset => {
                "Write one V/F curve point via the private SetControl (dangerous V/F edit; bank 0=V/F curve, 1=pstate-class; default/--freq-mode = kHz freq offset (same as public VFP, safest; also reaches xbar/host domains); --raw-converted = MHz target translated to a raw f-offset control value via the universal g(def) prior; --raw = write the raw f-offset control value verbatim)"
            }
            Self::SetPrivateVftableRangeOffset => {
                "Write a range of V/F curve points via the private SetControl (dangerous batch V/F edit; single RMW cycle; default/--freq-mode = same kHz freq offset on every point, --raw-converted = one MHz target translated per-point via g(def), --raw = one raw control word on every point)"
            }
            Self::SetTempLimit => "Set thermal limit in Celsius: NVAPI path writes the sensor limit; NVML path writes the GPU max-temp threshold, or the acoustic target temp with --domain acoustic (Linux channel; Windows rejects the NVML threshold setter -- use set-private-target-temp-limit there)",
            Self::SetPrivateTargetTempLimit => {
                "Set an NVAPI target-temp (temp-limit) policy slot in Celsius for mobile sku"
            }
            Self::SetFanSpeed => "Set fan speed: --percent (default) sets cooler level in percent (NVAPI SetCoolerLevels / NVML set_fan_speed); --rpm sets physical RPM via private FanCoolerSetControl (NVAPI-only)",
            Self::SetFreqLock => "Lock core or memory clocks to a MHz range",
            Self::SetGpcVoltLock => "Lock VFP by point or voltage",
            Self::OemOcScanner => {
                "Control NVIDIA's driver-side (OEM) OC Scanner: --start (driver scans in background and applies V/F offsets itself), --stop, --revert (restore pre-scan curve); drivers >= 455.00; no console progress output"
            }
            Self::SetPrivateForcedPstateLockUser => {
                "Force a P-State via private SetForcePstate (0x025BFB10); set_type 0/1/2 all force-lock, none release (to unlock use reset-private-forced-pstate-lock-user)"
            }
            Self::ResetPrivateForcedPstateLockUser => {
                "Release a force-locked pstate via SetForcePstate(pstate=16, set_type=0) — pstate=16 is the bitmask=0 sentinel (GetForcePstate returns 16 when no force active). IDA-verified as the most likely release path."
            }
            Self::RestartDisplayDriver => {
                "Restart the display driver (0xB4B26B65); legacy apply-OC trigger"
            }
            Self::SetBatteryBoost2Status => {
                "Battery Boost 2.0 enable/disable (0xD27D0629); mobile-only; 1=enable 0=disable"
            }
            Self::SetWhisperMode2Status => {
                "Whisper Mode 2.0 status (0xD27D0629, mobile-only): on/off enable; --mode quieter|quiet|balanced also writes the acoustic mode (0xD2561B69)"
            }
            Self::SetPublicVftablePointOffset => "Set one VFP point delta in MHz",
            Self::SetPublicVftableRangeOffset => "Set a VFP point range delta in MHz",
            Self::SetPstateLockViaMemRange => {
                "Lock one NVML P-State or a contiguous range via memory freq range"
            }
            Self::SetLegacyApplicationFreqLock => "Set NVML application clocks in MHz",
            Self::SetLegacyGpcRailOvervoltLimit => "Set NVAPI P-State base voltage delta in microvolts",
            Self::SetOvervoltUv => {
                "Set global NVAPI over-voltage offset in microvolts (PSTATES20 V2 OV array)"
            }
            Self::SetPublicGpcRailVoltBoost => "Set NVAPI voltage boost percent",
            Self::SetAutoboostStatus => "Set NVML auto-boost state",
            Self::ResetAutoboostStatus => "Set NVML default auto-boost state",
            Self::SetAutoboostSupport => "Set NVML API restriction state",
            Self::SetEdid => "Set display EDID through NVAPI",
            Self::ClearEdid => "Clear display EDID through NVAPI",
            Self::SetLegacyFreq => "Set an absolute clock for legacy (Kepler) GPUs in MHz: --domain core (default) or mem picks which clock the value targets",
            Self::ResetLegacyApplicationFreqLock => "Reset NVML application clocks",
            Self::ResetFreqLock => "Reset core or memory locked clocks",
            Self::ResetFanSpeed => "Restore fan/cooler control: default resets the NVAPI cooler levels / NVML fan to default; --rpm (NVAPI-only) instead disables fan-speed simulation and clears the enable bit (--cooler N picks one cooler)",
            Self::ResetPublicVftableOffset => "Reset NVAPI VFP deltas",
            Self::ResetPublicVftableGpcLock => "Reset NVAPI VFP lock",
            Self::ResetPrivateVftableOffset => "Reset private V/F-POINTS mode-0 overrides (clear raw/converted kHz offsets the public/pstate20 reset paths cannot reach)",
            Self::ResetPublicTgpPercent => "Reset NVAPI power limits",
            Self::ResetTempLimit => "Reset NVAPI sensor limits",
            Self::ResetLegacyGpcRailOvervoltLimit => "Reset NVAPI P-State base voltages",
            Self::ResetPstateGlobalFreqOffset => "Reset NVAPI P-State clock offsets (all touched pstate/domain pairs by default; --domain filters to one clock domain)",
            Self::ResetPublicGpcRailVoltBoost => "Reset NVAPI voltage boost percent",
        }
    }

    fn adapters(self) -> &'static [BackendAdapter] {
        match self {
            Self::GetGpuList
            | Self::GetPstateGlobalFreqOffset
            | Self::SetPstateGlobalFreqOffset
            | Self::SetTempLimit
            | Self::SetFanSpeed
            | Self::SetFreqLock
            | Self::SetPstateLockViaMemRange
            | Self::ResetFreqLock
            | Self::ResetFanSpeed
            | Self::GetTemperatureThresholds
            | Self::GetFanInfo
            | Self::SetPowerLimit
            | Self::GetPowerLimit => &BOTH_BACKENDS,
            Self::GetPstateFreqRange
            | Self::GetSupportedLegacyApplicationFreq
            | Self::GetThrottleReasons
            | Self::GetAutoboostStatus
            | Self::GetAutoboostSupport
            | Self::SetLegacyApplicationFreqLock
            | Self::SetAutoboostStatus
            | Self::ResetAutoboostStatus
            | Self::SetAutoboostSupport
            | Self::ResetLegacyApplicationFreqLock => &NVML_ONLY,
            _ => &NVAPI_ONLY,
        }
    }

    /// Backend an `auto` run should prefer when the command advertises both
    /// (`BOTH_BACKENDS`). Defaults to Nvapi; overridden per-command where the
    /// classic backend is the more useful default. `GetTemperatureThresholds`
    /// prefers NVML because the Shutdown/Slowdown/... table is the established
    /// output — its NVAPI branch (target-temp policy indices) is an opt-in via
    /// `--nvapi` while the index↔channel mapping is still being worked out.
    fn auto_preferred_backend(self) -> BackendAdapter {
        match self {
            Self::GetTemperatureThresholds | Self::GetPowerLimit => BackendAdapter::Nvml,
            _ => BackendAdapter::Nvapi,
        }
    }

    fn arity(self) -> (usize, usize) {
        match self {
            Self::GetAutoboostSupport | Self::GetEdid => (1, 1),
            Self::SetPstateGlobalFreqOffset
            | Self::SetPublicTgpPercent
            | Self::SetPpabStatus
            | Self::SetPowerLimit
            | Self::SetDNotifier
            | Self::SetPStateLock
            | Self::SetTempLimit
            | Self::SetPrivateTargetTempLimit
            | Self::SetFanSpeed
            | Self::SetGpcVoltLock
            | Self::SetLegacyGpcRailOvervoltLimit
            | Self::SetOvervoltUv
            | Self::SetAutoboostStatus
            | Self::ResetAutoboostStatus
            | Self::ClearEdid
            | Self::SetPublicGpcRailVoltBoost
            | Self::SetPowerMode
            | Self::SetLegacyFreq => (1, 1),
            Self::SetFanCurve => (2, 2),
            Self::ResetFanCurveCmd => (0, 0),
            Self::SetFanstopStatus => (1, 1),
            Self::OemOcScanner => (0, 0),
            Self::ResetPrivateForcedPstateLockUser => (0, 0),
            Self::RestartDisplayDriver => (0, 0),
            Self::SetBatteryBoost2Status => (1, 1),
            Self::SetWhisperMode2Status => (1, 1),
            Self::SetPrivateForcedPstateLockUser => (1, 1),
            Self::SetFreqLock
            | Self::SetPublicVftablePointOffset
            | Self::SetLegacyApplicationFreqLock
            | Self::SetAutoboostSupport
            | Self::SetEdid
            | Self::SetVoltRailLimit
            | Self::SetPrivateFreqDomainGlobalOffset => (2, 2),
            Self::SetGpuClock => (1, 1),
            Self::ResetGpuClock => (0, 0),
            Self::SetPrivateVftablePointOffset => (3, 3),
            Self::SetPrivateVftableRangeOffset => (4, 4),
            Self::SetPrivatePermanentPstateLockUser => (1, 1),
            Self::SetTempSim => (1, 1),
            Self::GetPowerMizer => (0, 1),
            Self::SetCoreVoltageControl => (1, 1),
            Self::SetPmgrArbiter => (1, 1),
            Self::ResetPrivateVftableOffset => (1, 1),
            Self::GetPrivateFreqDomainStatus => (0, 1),
            Self::SetPublicVftableRangeOffset => (3, 3),
            Self::SetPstateLockViaMemRange => (1, 2),
            _ => (0, 0),
        }
    }

    fn allowed_options(self) -> &'static [&'static str] {
        match self {
            Self::GetStatus => &["verbose"],
            Self::GetPStateLock => &["pstate-domain"],
            Self::SetPStateLock => &["pstate"],
            Self::GetPublicVftable => &[
                "domain",
                "indexed",
                "infer-missing-default",
                "no-infer-missing-default",
            ],
            Self::GetDisplayList => &["all"],
            Self::GetPstateGlobalFreqOffset => &["domain", "pstate"],
            Self::SetPstateGlobalFreqOffset => &["domain", "pstate"],
            Self::GetLegacyGpcRailOvervoltLimit | Self::SetLegacyGpcRailOvervoltLimit => {
                &["pstate"]
            }
            Self::SetFanSpeed => &["fan", "policy", "cooler", "percent", "rpm"],
            Self::ResetFanSpeed => &["fan", "cooler", "rpm"],
            Self::ResetFanCurveCmd | Self::SetFanstopStatus => &["curve"],
            Self::SetFreqLock | Self::ResetFreqLock | Self::ResetPublicVftableOffset => {
                &["domain"]
            }
            Self::GetPrivateVftable => &["domain"],
            Self::ResetPrivateVftableOffset => &["domain"],
            Self::SetGpcVoltLock => &["feedback"],
            Self::SetGpuClock => &["min"],
            Self::OemOcScanner => &["start", "stop", "revert", "status", "background-on", "background-off", "incomplete"],
            Self::SetPrivateForcedPstateLockUser => &["set-type"],
            Self::SetPowerLimit | Self::ResetPowerLimit | Self::SetPrivateTargetTempLimit => {
                &["policy-index"]
            }
            Self::SetVoltRailLimit => &["expect-type", "offset", "target"],
            Self::SetPrivateFreqDomainGlobalOffset => &["temporary", "slot"],
            Self::SetPrivateVftablePointOffset => &["freq-mode", "raw", "raw-converted"],
            Self::SetPrivateVftableRangeOffset => &["freq-mode", "raw", "raw-converted"],
            Self::SetTempLimit => &["domain"],
            Self::SetWhisperMode2Status => &["mode"],
            Self::SetLegacyFreq => &["domain"],
            Self::ResetPstateGlobalFreqOffset => &["domain"],
            _ => &[],
        }
    }

    fn positional_args(self) -> Vec<PositionalArg> {
        match self {
            Self::GetAutoboostSupport => vec![PositionalArg::finite(
                "arg_api",
                "API",
                "NVML API to query",
                PositionalValueKind::ApiRestrictionApi,
            )],
            Self::GetEdid | Self::ClearEdid => vec![PositionalArg::free(
                "arg_display_id",
                "DISPLAY_ID",
                "NVAPI display ID as hex, for example 0x00010001",
            )],
            Self::SetPstateGlobalFreqOffset => {
                vec![PositionalArg::hyphen(
                    "arg_offset_mhz",
                    "OFFSET_MHZ",
                    "Clock offset in MHz, for example -100 or 125MHz",
                )]
            }
            Self::SetPublicTgpPercent => vec![PositionalArg::free(
                "arg_power_percent",
                "PERCENT",
                "Power limit percentage, for example 90 or 90%",
            )],
            Self::SetPpabStatus => vec![PositionalArg::finite(
                "arg_dynamic_boost",
                "ENABLED",
                "Whether to enable Dynamic Boost / PPAB (on/off, yes/no, 1/0)",
                PositionalValueKind::Bool,
            )],
            Self::SetPowerLimit => vec![PositionalArg::free(
                "arg_tgp_watt",
                "WATT",
                "TGP in watts, for example 140 or 140W",
            )],
            Self::SetDNotifier => vec![PositionalArg::free(
                "arg_dnotifier_level",
                "LEVEL",
                "D-Notifier level 1-5 (D1=Unlimited .. D5=lowest cap)",
            )],
            Self::SetPStateLock => vec![PositionalArg::free(
                "arg_pstate_native_pstate",
                "PSTATE",
                "P-State to pin (e.g. P3 or 3); also settable via --pstate",
            )],
            Self::SetTempLimit => vec![PositionalArg::hyphen(
                "arg_celsius",
                "CELSIUS",
                "Temperature limit in Celsius, for example 83 or 83C; on the NVML path --domain acoustic targets the acoustic temp instead",
            )],
            Self::SetPrivateTargetTempLimit => vec![PositionalArg::hyphen(
                "arg_celsius",
                "CELSIUS",
                "Target-temperature threshold in Celsius, for example 85 or 85C",
            )],
            Self::SetVoltRailLimit => vec![
                PositionalArg::free(
                    "arg_rail_bit",
                    "RAIL_BIT",
                    "Volt-rail bit index from get-volt-rail-info (e.g. 0 for the single rail on a 4060 laptop, 1 for 5090 MSVDD)",
                ),
                PositionalArg::hyphen(
                    "arg_value",
                    "VALUE",
                    "Volt-rail limit value: --offset (default) = microvolt offset (e.g. -25000 or +50000uV); --target = absolute millivolts (e.g. 1150 or 1150mV, one decimal allowed for 10/20-series 12.5mV step). The driver clamps the effective wall to min(target, vbios_wall, vrm_max_wall)",
                ),
            ],
            Self::GetPowerMizer => vec![PositionalArg::free(
                "arg_power_source",
                "POWER_SOURCE",
                "1=AC, 2=DC (default 1)",
            )],
            Self::SetCoreVoltageControl => vec![PositionalArg::hyphen(
                "arg_value",
                "VALUE",
                "Raw control word to write (units uncalibrated — read with get-core-voltage-control first)",
            )],
            Self::SetPmgrArbiter => vec![PositionalArg::hyphen(
                "arg_values",
                "CSV",
                "Exactly 11 comma-separated dwords (get-pmgr-arbiter output order)",
            )],
            Self::GetPrivateFreqDomainStatus => vec![PositionalArg::free(
                "arg_domain",
                "DOMAIN",
                "Clock domain: xbar (1), gpc/core (0), sys (2), or mclk/mem (4); omit to measure every controllable domain",
            )],
            Self::SetPrivateFreqDomainGlobalOffset => vec![
                PositionalArg::free(
                    "arg_domain",
                    "DOMAIN",
                    "Clock domain to offset: xbar (1), gpc/core (0), sys (2), or mclk/mem (4)",
                ),
                PositionalArg::hyphen(
                    "arg_offset_khz",
                    "OFFSET_KHZ",
                    "Signed kilohertz offset, for example -60000 or +30000kHz; 0 is a no-op stock write. The driver may reject or clamp; the post-SET readback is returned. Pass --temporary to restore the snapshot before returning",
                ),
            ],
            Self::ResetGpuClock => vec![],
            Self::SetGpuClock => vec![PositionalArg::hyphen(
                "arg_max_mhz",
                "MAX_MHZ",
                "Perf max-frequency cap in MHz (e.g. 300). --min sets the min-frequency cap (defaults to MAX_MHZ); both are clamped by the driver. Distinct from a clock offset or P-state lock; use reset-perf-freq-caps to clear",
            )],
            Self::SetPrivateVftableRangeOffset => vec![
                PositionalArg::free(
                    "arg_bank",
                    "BANK",
                    "Bank: 0 = V/F curve points, 1 = pstate-class records",
                ),
                PositionalArg::free("arg_start", "START", "Start point index (inclusive)"),
                PositionalArg::free("arg_end", "END", "End point index (inclusive)"),
                PositionalArg::hyphen(
                    "arg_delta",
                    "VALUE",
                    "default/--freq-mode: kHz freq offset applied to every point (e.g. 200000 = +200 MHz). --raw-converted: MHz target translated per-point to a raw f-offset control value via g(def) (each point gets its own C(def)/D0). --raw: raw f-offset control word applied to every point",
                ),
            ],
            Self::ResetPrivateVftableOffset => vec![PositionalArg::free(
                "arg_bank",
                "BANK",
                "Bank to reset: 0 = V/F curve points (clears mode-0 kHz offsets written via set-vfp-point/range-private default/--freq-mode), 1 = pstate-class records; --domain gpc|xbar|host restricts the reset to that domain's segments (bank 0 only)",
            )],
            Self::SetPrivateVftablePointOffset => vec![
                PositionalArg::free(
                    "arg_bank",
                    "BANK",
                    "Bank: 0 = V/F curve points, 1 = pstate-class records",
                ),
                PositionalArg::free(
                    "arg_index",
                    "INDEX",
                    "Point index within the bank (0-2047; use get-clk-vf-points to see which indices are present)",
                ),
                PositionalArg::hyphen(
                    "arg_value",
                    "VALUE",
                    "default/--freq-mode: kHz freq offset (e.g. 200000 = +200 MHz). --raw-converted: MHz target translated to a raw f-offset control value via the universal g(def) prior (effect_mhz = C(def)*(delta-D0)). --raw: raw f-offset control value verbatim",
                ),
            ],
            Self::SetFanSpeed => vec![PositionalArg::free(
                "arg_fan_value",
                "VALUE",
                "Fan speed value: --percent (default) = percentage 0-100; --rpm = physical RPM (NVAPI-only, clamped to cooler [min, max] from get-fan-info --nvapi)",
            )],
            Self::SetPrivatePermanentPstateLockUser => vec![PositionalArg::free(
                "arg_level",
                "LEVEL",
                "Index into this GPU's real P-State list (get-pstate-native; on the 4060 Laptop: 0=P8, 1=P5, 2=P4, 3=P3, 4=P0). Admin-free; no release value — reboot clears",
            )],
            Self::SetTempSim => vec![PositionalArg::free(
                "arg_temp_c",
                "TEMP_C",
                "Fake temperature in Celsius the driver will see (DANGEROUS research tool)",
            )],
            Self::SetFreqLock => vec![
                PositionalArg::free("arg_min_mhz", "MIN_MHZ", "Minimum clock in MHz"),
                PositionalArg::free("arg_max_mhz", "MAX_MHZ", "Maximum clock in MHz"),
            ],
            Self::SetGpcVoltLock => vec![PositionalArg::free(
                "arg_voltage_target",
                "TARGET",
                "VFP point index or voltage, for example 42, 900mV, or 900000uV",
            )],
            Self::SetPublicVftablePointOffset => vec![
                PositionalArg::free("arg_point", "POINT", "VFP point index"),
                PositionalArg::hyphen(
                    "arg_delta_mhz",
                    "DELTA_MHZ",
                    "Frequency delta in MHz, for example -30 or 15MHz",
                ),
            ],
            Self::SetPublicVftableRangeOffset => vec![
                PositionalArg::free("arg_start_point", "START_POINT", "First VFP point index"),
                PositionalArg::free("arg_end_point", "END_POINT", "Last VFP point index"),
                PositionalArg::hyphen(
                    "arg_delta_mhz",
                    "DELTA_MHZ",
                    "Frequency delta in MHz, for example -30 or 15MHz",
                ),
            ],
            Self::SetPstateLockViaMemRange => vec![
                PositionalArg::finite(
                    "arg_first_pstate",
                    "FIRST_PSTATE",
                    "First P-State to lock",
                    PositionalValueKind::Pstate,
                ),
                PositionalArg::finite(
                    "arg_second_pstate",
                    "SECOND_PSTATE",
                    "Optional final P-State to lock",
                    PositionalValueKind::Pstate,
                ),
            ],
            Self::SetLegacyApplicationFreqLock => vec![
                PositionalArg::free("arg_memory_mhz", "MEMORY_MHZ", "Memory clock in MHz"),
                PositionalArg::free("arg_graphics_mhz", "GRAPHICS_MHZ", "Graphics clock in MHz"),
            ],
            Self::SetLegacyGpcRailOvervoltLimit => vec![PositionalArg::hyphen(
                "arg_delta_uv",
                "DELTA_UV",
                "Base voltage delta in microvolts, for example 100000 or -25000uV",
            )],
            Self::SetOvervoltUv => vec![PositionalArg::hyphen(
                "arg_delta_uv",
                "DELTA_UV",
                "Global over-voltage offset in microvolts (PSTATES20 V2 OV array; HYDRA NvApiSetOverVoltageOffset path)",
            )],
            Self::SetPrivateForcedPstateLockUser => vec![PositionalArg::free(
                "arg_pstate",
                "PSTATE",
                "P-State number to force (e.g. 0 for P0)",
            )],
            Self::SetBatteryBoost2Status => vec![PositionalArg::free(
                "arg_state",
                "ENABLE",
                "1=enable, 0=disable",
            )],
            Self::SetWhisperMode2Status => vec![PositionalArg::finite(
                "arg_state",
                "ENABLE",
                "Whether to enable Whisper Mode 2.0 (on/off, yes/no, 1/0)",
                PositionalValueKind::Bool,
            )],
            Self::SetPublicGpcRailVoltBoost => vec![PositionalArg::free(
                "arg_boost_percent",
                "PERCENT",
                "Voltage boost percentage",
            )],
            Self::SetPowerMode => vec![PositionalArg::free(
                "arg_mode",
                "MODE",
                "Power mode: max | balanced",
            )],
            Self::SetFanCurve => vec![
                PositionalArg::free(
                    "arg_curve",
                    "CURVE",
                    "Fan-curve slot index (0-3, driver count reports available slots)",
                ),
                PositionalArg::free(
                    "arg_points",
                    "POINTS",
                    "Three monotonic points temp:rpm, e.g. 40:800,60:1200,75:1800",
                ),
            ],
            Self::ResetFanCurveCmd => vec![],
            Self::SetFanstopStatus => vec![PositionalArg::free(
                "arg_state",
                "STATE",
                "on = allow the fan to stop at idle (zero-RPM), off = always spin",
            )],
            Self::SetAutoboostStatus | Self::ResetAutoboostStatus => vec![PositionalArg::finite(
                "arg_enabled",
                "ENABLED",
                "Whether auto-boost is enabled",
                PositionalValueKind::Bool,
            )],
            Self::SetAutoboostSupport => vec![
                PositionalArg::finite(
                    "arg_api",
                    "API",
                    "NVML API to restrict",
                    PositionalValueKind::ApiRestrictionApi,
                ),
                PositionalArg::finite(
                    "arg_restriction_state",
                    "STATE",
                    "Restriction state",
                    PositionalValueKind::ApiRestrictionState,
                ),
            ],
            Self::SetEdid => vec![
                PositionalArg::free(
                    "arg_display_id",
                    "DISPLAY_ID",
                    "NVAPI display ID as hex, for example 0x00010001",
                ),
                PositionalArg::free(
                    "arg_edid_hex",
                    "EDID_HEX",
                    "EDID bytes as an even-length hex string",
                ),
            ],
            Self::SetLegacyFreq => vec![PositionalArg::free(
                "arg_mhz",
                "MHZ",
                "Absolute clock in MHz for --domain core (default) or mem; the other clock is left untouched (passed as 0 to the legacy SetClocks call)",
            )],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionalValueKind {
    Free,
    ApiRestrictionApi,
    ApiRestrictionState,
    Bool,
    Pstate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositionalArg {
    id: &'static str,
    value_name: &'static str,
    help: &'static str,
    allow_hyphen_values: bool,
    value_kind: PositionalValueKind,
}

impl PositionalArg {
    const fn free(id: &'static str, value_name: &'static str, help: &'static str) -> Self {
        Self {
            id,
            value_name,
            help,
            allow_hyphen_values: false,
            value_kind: PositionalValueKind::Free,
        }
    }

    const fn hyphen(id: &'static str, value_name: &'static str, help: &'static str) -> Self {
        Self {
            id,
            value_name,
            help,
            allow_hyphen_values: true,
            value_kind: PositionalValueKind::Free,
        }
    }

    const fn finite(
        id: &'static str,
        value_name: &'static str,
        help: &'static str,
        value_kind: PositionalValueKind,
    ) -> Self {
        Self {
            id,
            value_name,
            help,
            allow_hyphen_values: false,
            value_kind,
        }
    }
}

// Commands listed in strict lexicographic order by their CLI subcommand name
// (the order here is the order they appear in `--help`; kept sorted by test).
// The enum variant order is independent and left unchanged (family-grouped).
const COMMANDS: &[Command] = &[
    Command::ClearEdid,
    Command::GetAutoboostStatus,
    Command::GetAutoboostSupport,
    Command::GetCoreVoltageControl,
    Command::GetDisplayList,
    Command::GetDNotifier,
    Command::GetEdid,
    Command::GetFanCurve,
    Command::GetFanInfo,
    Command::GetGpuList,
    Command::GetInfo,
    Command::GetLegacyGpcRailOvervoltLimit,
    Command::GetLegacyOvervoltRanges,
    Command::GetLegacyP0CoreMaxVoltageDelta,
    Command::GetLegacyTempSensor,
    Command::GetPmgrArbiter,
    Command::GetPowerLimit,
    Command::GetPowerMizer,
    Command::GetPowerMode,
    Command::GetPrivateFreqDomainInfo,
    Command::GetPrivateFreqDomainStatus,
    Command::GetPrivateVftable,
    Command::GetPstateFreqRange,
    Command::GetPstateGlobalFreqOffset,
    Command::GetPStateLock,
    Command::GetPublicGpcRailVoltBoost,
    Command::GetPublicPowerLimit,
    Command::GetPublicTempLimit,
    Command::GetPublicVftable,
    Command::GetRatedTdp,
    Command::GetSettings,
    Command::GetStatus,
    Command::GetSupportedLegacyApplicationFreq,
    Command::GetTempSim,
    Command::GetTemperatureThresholds,
    Command::GetThrottleReasons,
    Command::GetUuid,
    Command::GetVoltRailInfo,
    Command::OemOcScanner,
    Command::ResetAutoboostStatus,
    Command::ResetFanCurveCmd,
    Command::ResetFanSpeed,
    Command::ResetFreqLock,
    Command::ResetLegacyApplicationFreqLock,
    Command::ResetLegacyGpcRailOvervoltLimit,
    Command::ResetGpuClock,
    Command::ResetPowerLimit,
    Command::ResetPrivateForcedPstateLockUser,
    Command::ResetPrivateVftableOffset,
    Command::ResetPstateGlobalFreqOffset,
    Command::ResetPStateLock,
    Command::ResetPublicGpcRailVoltBoost,
    Command::ResetPublicTgpPercent,
    Command::ResetPublicVftableGpcLock,
    Command::ResetPublicVftableOffset,
    Command::ResetTempLimit,
    Command::ResetTempSim,
    Command::RestartDisplayDriver,
    Command::SetAutoboostStatus,
    Command::SetAutoboostSupport,
    Command::SetBatteryBoost2Status,
    Command::SetCoreVoltageControl,
    Command::SetDNotifier,
    Command::SetEdid,
    Command::SetFanCurve,
    Command::SetFanSpeed,
    Command::SetFanstopStatus,
    Command::SetFreqLock,
    Command::SetGpcVoltLock,
    Command::SetLegacyApplicationFreqLock,
    Command::SetLegacyFreq,
    Command::SetLegacyGpcRailOvervoltLimit,
    Command::SetOvervoltUv,
    Command::SetGpuClock,
    Command::SetPmgrArbiter,
    Command::SetPowerLimit,
    Command::SetPowerMode,
    Command::SetPpabStatus,
    Command::SetPrivateForcedPstateLockUser,
    Command::SetPrivateFreqDomainGlobalOffset,
    Command::SetPrivatePermanentPstateLockUser,
    Command::SetPrivateTargetTempLimit,
    Command::SetPrivateVftablePointOffset,
    Command::SetPrivateVftableRangeOffset,
    Command::SetPstateGlobalFreqOffset,
    Command::SetPStateLock,
    Command::SetPstateLockViaMemRange,
    Command::SetPublicGpcRailVoltBoost,
    Command::SetPublicTgpPercent,
    Command::SetPublicVftablePointOffset,
    Command::SetPublicVftableRangeOffset,
    Command::SetTempLimit,
    Command::SetTempSim,
    Command::SetVoltRailLimit,
    Command::SetWhisperMode2Status,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub backend: BackendChoice,
    pub output: OutputFormat,
    pub no_color: bool,
    pub gpu_specs: Vec<String>,
    pub command: Option<Command>,
    pub positionals: Vec<String>,
    options: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct RunOutput {
    pub rendered: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
struct TargetResult {
    gpu_id: Option<u32>,
    backend: &'static str,
    ok: bool,
    output: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct Execution {
    function: &'static str,
    backend: String,
    warnings: Vec<String>,
    results: Vec<TargetResult>,
}

impl Execution {
    fn has_errors(&self) -> bool {
        self.results.iter().any(|result| !result.ok)
    }
}

pub fn parse_args<I, S>(args: I) -> CliResult<Invocation>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut argv = vec!["nvoc-cli".to_string()];
    argv.extend(args.into_iter().map(Into::into));

    let command_hint = command_hint_from_argv(&argv[1..]);
    let mut cli = cli_command(command_hint);
    if argv.iter().any(|arg| arg == "--no-color") || std::env::var_os("NO_COLOR").is_some() {
        cli = cli.color(ColorChoice::Never);
    }

    let matches = match cli.try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(err) => return Err(CliError::Clap(err)),
    };

    let (command_name, command_matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::new("missing function name"))?;
    let parsed_command = parse_command(command_name)?;
    let command = Some(parsed_command);
    let backend = if command_matches.get_flag("nvapi") {
        BackendChoice::Nvapi
    } else if command_matches.get_flag("nvml") {
        BackendChoice::Nvml
    } else {
        BackendChoice::Auto
    };
    let output = command_matches
        .get_one::<String>("output")
        .map_or(Ok(OutputFormat::Human), |raw| parse_output_format(raw))?;
    let no_color = command_matches.get_flag("no-color");
    let gpu_specs = command_matches
        .get_many::<String>("gpu")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let positionals = parsed_command
        .positional_args()
        .into_iter()
        .filter_map(|arg| command_matches.get_one::<String>(arg.id).cloned())
        .collect();
    let options = collect_named_options(command_matches, parsed_command.allowed_options());

    let invocation = Invocation {
        backend,
        output,
        no_color,
        gpu_specs,
        command,
        positionals,
        options,
    };

    validate_invocation(&invocation)?;
    Ok(invocation)
}

fn validate_invocation(invocation: &Invocation) -> CliResult<()> {
    let command = invocation
        .command
        .ok_or_else(|| CliError::new("missing function name"))?;

    let supported = command.adapters();
    match invocation.backend {
        BackendChoice::Nvapi if !supported.contains(&BackendAdapter::Nvapi) => {
            return Err(CliError::new(format!(
                "{} does not support --nvapi",
                command.name()
            )));
        }
        BackendChoice::Nvml if !supported.contains(&BackendAdapter::Nvml) => {
            return Err(CliError::new(format!(
                "{} does not support --nvml",
                command.name()
            )));
        }
        _ => {}
    }

    let (min_args, max_args) = command.arity();
    if invocation.positionals.len() < min_args || invocation.positionals.len() > max_args {
        let expected = if min_args == max_args {
            min_args.to_string()
        } else {
            format!("{min_args}..={max_args}")
        };
        return Err(CliError::new(format!(
            "{} expects {expected} positional args, got {}",
            command.name(),
            invocation.positionals.len()
        )));
    }

    for option in invocation.options.keys() {
        if !command.allowed_options().contains(&option.as_str()) {
            return Err(CliError::new(format!(
                "--{option} is not valid for {}",
                command.name()
            )));
        }
    }

    if command == Command::ResetFanSpeed
        && option_one(invocation, "fan").is_some_and(|fan| !fan.eq_ignore_ascii_case("all"))
        && invocation.backend != BackendChoice::Nvml
    {
        return Err(CliError::new(
            "reset-fan-speed with a specific --fan requires --nvml; NVAPI resets all coolers",
        ));
    }

    Ok(())
}

fn parse_output_format(raw: &str) -> CliResult<OutputFormat> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        other => Err(CliError::new(format!(
            "invalid output format {other:?}; expected human or json"
        ))),
    }
}

fn parse_command(raw: &str) -> CliResult<Command> {
    // Naming-compat aliases: the per-pstate baseVoltage SET predates the
    // "legacy overvolt" GET naming; alias keeps the get/set pair obvious.
    let raw = match raw {
        "set-legacy-overvolt-uv" => "set-legacy-gpc-rail-overvolt-limit",
        _ => raw,
    };
    COMMANDS
        .iter()
        .copied()
        .find(|command| command.name() == raw)
        .ok_or_else(|| CliError::new(format!("unknown function {raw:?}")))
}

fn command_hint_from_argv(argv: &[String]) -> Option<Command> {
    let mut index = 0;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" {
            return None;
        }
        if option_takes_value(token) {
            index += 2;
            continue;
        }
        if let Ok(command) = parse_command(token) {
            return Some(command);
        }
        index += 1;
    }
    None
}

fn option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-g" | "-O"
            | "--gpu"
            | "--output"
            | "--domain"
            | "--pstate"
            | "--fan"
            | "--policy"
            | "--policy-index"
            | "--infer-missing-default"
    )
}

fn cli_command(command_hint: Option<Command>) -> ClapCommand {
    let mut command = ClapCommand::new("nvoc-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Focused command-line wrapper for nvoc-core")
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("gpu")
                .short('g')
                .long("gpu")
                .value_name("GPU_ID")
                .action(ArgAction::Append)
                .global(true)
                .help("GPU selector; repeat for multiple GPUs"),
        )
        .arg(
            Arg::new("nvapi")
                .long("nvapi")
                .action(ArgAction::SetTrue)
                .conflicts_with("nvml")
                .global(true)
                .help("Force the NVAPI backend"),
        )
        .arg(
            Arg::new("nvml")
                .long("nvml")
                .action(ArgAction::SetTrue)
                .conflicts_with("nvapi")
                .global(true)
                .help("Force the NVML backend"),
        )
        .arg(
            Arg::new("output")
                .short('O')
                .long("output")
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .global(true)
                .help("Output format"),
        )
        .arg(
            Arg::new("no-color")
                .long("no-color")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Disable ANSI color output"),
        );

    if let Some(command_hint) = command_hint {
        for option in command_hint.allowed_options() {
            command = command.arg(command_specific_arg(option));
        }
    }

    for nvoc_command in COMMANDS {
        command = command.subcommand(clap_subcommand(*nvoc_command));
    }
    command
}

fn command_specific_arg(name: &'static str) -> Arg {
    match name {
        "verbose" => Arg::new("verbose")
            .long("verbose")
            .action(ArgAction::SetTrue)
            .help("Show verbose status (VFP table, raw power-monitor descriptors, D-Notifier D1-D5 cap table)"),
        "pstate-domain" => Arg::new("pstate-domain")
            .long("domain")
            .value_name("DOMAIN")
            .action(ArgAction::Set)
            .help("Clock-domain index for get-pstate-native MHz values (0=GPC/core default; the ref tool resolves GPC via 0x57B5A5DF)"),
        "domain" => Arg::new("domain")
            .long("domain")
            .value_name("DOMAIN")
            .action(ArgAction::Append)
            .global(true)
            .help("Domain selector, meaning depends on the command: clock domain (core/memory/processor/video), gpu|acoustic (set-temp-limit NVML path), core|mem (set-legacy-freq), or gpc|xbar|host (reset-private-vftable-offset)"),
        "pstate" => Arg::new("pstate")
            .long("pstate")
            .value_name("PSTATE")
            .action(ArgAction::Append)
            .global(true)
            .help("P-State such as P0 or P2"),
        "fan" => Arg::new("fan")
            .long("fan")
            .value_name("FAN")
            .action(ArgAction::Append)
            .global(true)
            .help("Fan/cooler target: all, 0, 1, or 2"),
        "policy" => Arg::new("policy")
            .long("policy")
            .value_name("POLICY")
            .action(ArgAction::Append)
            .global(true)
            .help("Fan policy such as manual or continuous"),
        "policy-index" => Arg::new("policy-index")
            .long("policy-index")
            .value_name("INDEX")
            .action(ArgAction::Set)
            .help("TGP power-policy table index (default 2); see get-power-limit (NVAPI fallback)"),
        "infer-missing-default" => Arg::new("infer-missing-default")
            .long("infer-missing-default")
            .value_name("BOOL")
            .action(ArgAction::Append)
            .global(true)
            .help("Infer missing default VFP values"),
        "indexed" => Arg::new("indexed")
            .long("indexed")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Preserve hardware VFP indices"),
        "no-infer-missing-default" => Arg::new("no-infer-missing-default")
            .long("no-infer-missing-default")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Do not infer missing default VFP values"),
        "feedback" => Arg::new("feedback")
            .long("feedback")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("Enable feedback for VFP voltage lock"),
        "start" => Arg::new("start")
            .long("start")
            .action(ArgAction::SetTrue)
            .help("Start the driver-side (OEM) OC scanner; it runs in the background and applies the resulting V/F offsets itself (no console progress output)"),
        "stop" => Arg::new("stop")
            .long("stop")
            .action(ArgAction::SetTrue)
            .help("Stop the driver-side OC scanner"),
        "revert" => Arg::new("revert")
            .long("revert")
            .action(ArgAction::SetTrue)
            .help("Revert the OC applied by the driver-side scanner (restore the pre-scan curve)"),
        "status" => Arg::new("status")
            .long("status")
            .action(ArgAction::SetTrue)
            .help("Query the last OC scanner run status (Ok = idle/has-result; error = busy/not-supported)"),
        "set-type" => Arg::new("set-type")
            .long("set-type")
            .value_name("TYPE")
            .action(ArgAction::Set)
            .help("SetPrivateForcedPstateLockUser type (0/1/2 all force-lock; none release; default 2 = nvapioc convention)"),
        "all" => Arg::new("all")
            .long("all")
            .action(ArgAction::SetTrue)
            .global(true)
            .help("List all display IDs instead of only connected display IDs"),
        "percent" => Arg::new("percent")
            .long("percent")
            .action(ArgAction::SetTrue)
            .help("Fan speed mode: VALUE is a percentage 0-100 (default)"),
        "rpm" => Arg::new("rpm")
            .long("rpm")
            .action(ArgAction::SetTrue)
            .help("Fan speed mode: VALUE is physical RPM (NVAPI-only, private FanCoolerSetControl)"),
        "offset" => Arg::new("offset")
            .long("offset")
            .action(ArgAction::SetTrue)
            .help("Volt-rail limit mode: VALUE is a microvolt offset (default)"),
        "target" => Arg::new("target")
            .long("target")
            .action(ArgAction::SetTrue)
            .help("Volt-rail limit mode: VALUE is an absolute target in millivolts"),
        "expect-type" => Arg::new("expect-type")
            .long("expect-type")
            .value_name("TYPE")
            .action(ArgAction::Set)
            .help("Require the rail control entry to report this type (e.g. 3 = 5090 MSVDD µV offset); omit to skip the guard"),
        "temporary" => Arg::new("temporary")
            .long("temporary")
            .action(ArgAction::SetTrue)
            .help("Restore the pre-write control snapshot before returning (safe XBar experiment mode; see xbar.txt safety recipe)"),
        "freq-mode" => Arg::new("freq-mode")
            .long("freq-mode")
            .action(ArgAction::SetTrue)
            .help("kHz frequency offset mode (same as public VFP freqDeltaKHz; this is the DEFAULT — flag is an explicit alias. Reaches xbar/host domains unlike the public API)"),
        "slot" => Arg::new("slot")
            .long("slot")
            .value_name("SLOT")
            .action(ArgAction::Set)
            .help("Which of the record's 8 value dwords to write (0-7; default 0 = the signed frequency offset; other slots are driver-opaque — identify via A/B with get-clk-domain-freq)"),
        "raw" => Arg::new("raw")
            .long("raw")
            .action(ArgAction::SetTrue)
            .help("Write VALUE as the raw f-offset control value (mode 1) verbatim"),
        "raw-converted" => Arg::new("raw-converted")
            .long("raw-converted")
            .action(ArgAction::SetTrue)
            .help("Translate VALUE (a MHz target) to a raw f-offset control value via the universal g(def) prior"),
        "min" => Arg::new("min")
            .long("min")
            .value_name("MIN_MHZ")
            .action(ArgAction::Set)
            .help("Perf min-frequency cap in MHz (defaults to MAX_MHZ); both bounds are clamped by the driver"),
        "curve" => Arg::new("curve")
            .long("curve")
            .value_name("IDX")
            .action(ArgAction::Set)
            .help("Fan-curve slot index (0-3; default 0)"),
        "cooler" => Arg::new("cooler")
            .long("cooler")
            .value_name("IDX")
            .action(ArgAction::Set)
            .help("Cooler index (0-31; default 0 — see get-fan-info --nvapi for the presence mask)"),
        "background-on" => Arg::new("background-on")
            .long("background-on")
            .action(ArgAction::SetTrue)
            .help("Enable the background OC scanner (0x06DC7CE8)"),
        "background-off" => Arg::new("background-off")
            .long("background-off")
            .action(ArgAction::SetTrue)
            .help("Disable the background OC scanner"),
        "incomplete" => Arg::new("incomplete")
            .long("incomplete")
            .action(ArgAction::SetTrue)
            .help("Query the last INCOMPLETE OC-scanner run's partial results (0xBE371D0A)"),
        "mode" => Arg::new("mode")
            .long("mode")
            .value_name("MODE")
            .action(ArgAction::Set)
            .help("Whisper Mode 2.0 acoustic mode: quieter, quiet, or balanced (or 0/1/2)"),
        _ => unreachable!("unknown command-specific option {name}"),
    }
}

fn clap_subcommand(command: Command) -> ClapCommand {
    let mut subcommand = ClapCommand::new(command.name()).about(command.about());
    // Zero-arg invocation prints the FULL help (including every option)
    // instead of a terse usage line that hides the options — the error
    // usage line only lists missing positionals, which made e.g.
    // `set-fan-rpm` (no args) not show --cooler at all.
    let (min_args, _) = command.arity();
    if min_args > 0 {
        subcommand = subcommand.arg_required_else_help(true);
    }
    // Naming-compat alias: pairs the per-pstate baseVoltage SET with the
    // "legacy overvolt" GET naming (same operation, Maxwell-era path).
    if command == Command::SetLegacyGpcRailOvervoltLimit {
        subcommand = subcommand.alias("set-legacy-overvolt-uv");
    }
    for (index, positional) in command.positional_args().into_iter().enumerate() {
        subcommand = subcommand.arg(positional_arg(positional, index < min_args));
    }
    for option in command.allowed_options() {
        subcommand = subcommand.arg(command_specific_arg(option));
    }
    subcommand
}

fn positional_arg(spec: PositionalArg, required: bool) -> Arg {
    let mut arg = Arg::new(spec.id)
        .value_name(spec.value_name)
        .help(spec.help)
        .required(required)
        .num_args(1)
        .allow_hyphen_values(spec.allow_hyphen_values);

    if let Some(parser) = possible_values_parser(spec.value_kind) {
        arg = arg.value_parser(parser).ignore_case(true);
    }

    arg
}

fn possible_values_parser(kind: PositionalValueKind) -> Option<PossibleValuesParser> {
    match kind {
        PositionalValueKind::Free => None,
        PositionalValueKind::ApiRestrictionApi => Some(PossibleValuesParser::new([
            PossibleValue::new("app-clocks").alias("application-clocks"),
            PossibleValue::new("auto-boost").alias("autoboost"),
        ])),
        PositionalValueKind::ApiRestrictionState => Some(PossibleValuesParser::new([
            PossibleValue::new("open"),
            PossibleValue::new("restricted"),
        ])),
        PositionalValueKind::Bool => Some(PossibleValuesParser::new([
            PossibleValue::new("on").aliases(["true", "yes", "1"]),
            PossibleValue::new("off").aliases(["false", "no", "0"]),
        ])),
        PositionalValueKind::Pstate => Some(PossibleValuesParser::new([
            PossibleValue::new("P0").alias("0"),
            PossibleValue::new("P1").alias("1"),
            PossibleValue::new("P2").alias("2"),
            PossibleValue::new("P3").alias("3"),
            PossibleValue::new("P4").alias("4"),
            PossibleValue::new("P5").alias("5"),
            PossibleValue::new("P6").alias("6"),
            PossibleValue::new("P7").alias("7"),
            PossibleValue::new("P8").alias("8"),
            PossibleValue::new("P9").alias("9"),
            PossibleValue::new("P10").alias("10"),
            PossibleValue::new("P11").alias("11"),
            PossibleValue::new("P12").alias("12"),
            PossibleValue::new("P13").alias("13"),
            PossibleValue::new("P14").alias("14"),
            PossibleValue::new("P15").alias("15"),
        ])),
    }
}

fn collect_named_options(
    matches: &clap::ArgMatches,
    allowed_options: &[&'static str],
) -> BTreeMap<String, Vec<String>> {
    let mut options = BTreeMap::new();
    for name in allowed_options {
        match *name {
            "indexed"
            | "no-infer-missing-default"
            | "feedback"
            | "all"
            | "verbose"
            | "temporary"
            | "freq-mode"
            | "raw"
            | "raw-converted"
            | "start"
            | "stop"
            | "revert"
            | "status"
            | "background-on"
            | "background-off"
            | "incomplete"
            | "percent"
            | "rpm"
            | "offset"
            | "target" => {
                if matches.get_flag(name) {
                    options.insert(name.to_string(), vec!["true".to_string()]);
                }
            }
            _ => {
                if let Some(values) = matches.get_many::<String>(name) {
                    options.insert(name.to_string(), values.cloned().collect());
                }
            }
        }
    }
    options
}

pub fn run_invocation(invocation: &Invocation) -> CliResult<RunOutput> {
    let execution = execute(invocation)?;
    let rendered = match invocation.output {
        OutputFormat::Human => output::format_human(&execution),
        OutputFormat::Json => serde_json::to_string(&output::execution_to_json(&execution))?,
    };
    Ok(RunOutput {
        rendered,
        exit_code: i32::from(execution.has_errors()),
    })
}

fn execute(invocation: &Invocation) -> CliResult<Execution> {
    let command = invocation
        .command
        .ok_or_else(|| CliError::new("missing function name"))?;

    match invocation.backend {
        BackendChoice::Nvapi => execute_backend(invocation, command, BackendAdapter::Nvapi),
        BackendChoice::Nvml => execute_backend(invocation, command, BackendAdapter::Nvml),
        BackendChoice::Auto => execute_auto(invocation, command),
    }
}

fn execute_auto(invocation: &Invocation, command: Command) -> CliResult<Execution> {
    if command == Command::GetGpuList {
        return execute_list_gpus_auto(invocation);
    }

    let supports_nvapi = command.adapters().contains(&BackendAdapter::Nvapi);
    let supports_nvml = command.adapters().contains(&BackendAdapter::Nvml);

    // Where a command advertises both backends, honour its preferred default
    // before falling back. Commands that prefer NVML (e.g. GetTemperatureThresholds)
    // short-circuit straight to the NVML path on auto; the NVAPI branch stays
    // reachable via an explicit `--nvapi`.
    if supports_nvml && command.auto_preferred_backend() == BackendAdapter::Nvml {
        return execute_backend(invocation, command, BackendAdapter::Nvml);
    }

    if supports_nvapi {
        let nvapi_attempt = execute_backend(invocation, command, BackendAdapter::Nvapi);
        match nvapi_attempt {
            Ok(mut execution) if !execution.has_errors() => {
                if supports_nvml && let Ok(selected_ids) = selected_auto_target_ids(invocation) {
                    let missing_ids = uncovered_target_ids(&selected_ids, &execution);
                    if !missing_ids.is_empty() {
                        let nvml_execution = execute_backend_for_gpu_ids(
                            invocation,
                            command,
                            BackendAdapter::Nvml,
                            &missing_ids,
                        )?;
                        execution.backend = "auto".to_string();
                        execution.results.extend(nvml_execution.results);
                    }
                }
                return Ok(execution);
            }
            Ok(nvapi_execution) if supports_nvml => {
                let mut nvml_execution =
                    execute_backend(invocation, command, BackendAdapter::Nvml)?;
                nvml_execution.warnings.insert(
                    0,
                    format!(
                        "NVAPI attempt for {} failed; fell back to NVML",
                        command.name()
                    ),
                );
                if nvml_execution.has_errors() {
                    nvml_execution.warnings.insert(
                        1,
                        format!(
                            "NVAPI result was also unsuccessful: {}",
                            summarize_errors(&nvapi_execution)
                        ),
                    );
                }
                return Ok(nvml_execution);
            }
            Ok(execution) => return Ok(execution),
            Err(nvapi_error) if supports_nvml => {
                let mut nvml_execution =
                    execute_backend(invocation, command, BackendAdapter::Nvml)?;
                nvml_execution.warnings.insert(
                    0,
                    format!("NVAPI attempt failed; fell back to NVML: {nvapi_error}"),
                );
                return Ok(nvml_execution);
            }
            Err(error) => return Err(error),
        }
    }

    if supports_nvml {
        return execute_backend(invocation, command, BackendAdapter::Nvml);
    }

    Err(CliError::new(format!(
        "{} has no runnable backend",
        command.name()
    )))
}

fn execute_backend(
    invocation: &Invocation,
    command: Command,
    adapter: BackendAdapter,
) -> CliResult<Execution> {
    let discovery = discovery_backend_set(command, adapter);
    let inventory = discover_targets(discovery)?;
    let all_targets = inventory.targets();

    if command == Command::GetGpuList {
        return list_gpus_execution(
            command,
            adapter.label().to_string(),
            &all_targets,
            invocation,
            adapter,
        );
    }

    let selector = gpu_selector(invocation);
    let selected = select_targets(&all_targets, &selector)?;
    let filtered = selected
        .into_iter()
        .filter(|target| target_supports(*target, command, adapter))
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return Err(CliError::new(format!(
            "no selected GPUs expose the {} backend required by {}",
            adapter.label(),
            command.name()
        )));
    }

    let mut results = Vec::with_capacity(filtered.len());
    for target in filtered {
        let result = match execute_target(command, adapter, &target, invocation) {
            Ok(output) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: true,
                output: Some(output),
                error: None,
            },
            Err(error) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: false,
                output: None,
                error: Some(error.to_string()),
            },
        };
        results.push(result);
    }

    Ok(Execution {
        function: command.name(),
        backend: adapter.label().to_string(),
        warnings: Vec::new(),
        results,
    })
}

fn selected_auto_target_ids(invocation: &Invocation) -> CliResult<Vec<u32>> {
    let inventory = discover_targets(BackendSet::Both)?;
    let all_targets = inventory.targets();
    let selector = gpu_selector(invocation);
    let selected = select_targets(&all_targets, &selector)?;
    Ok(selected.into_iter().map(|target| target.id.0).collect())
}

fn execute_backend_for_gpu_ids(
    invocation: &Invocation,
    command: Command,
    adapter: BackendAdapter,
    gpu_ids: &[u32],
) -> CliResult<Execution> {
    let discovery = discovery_backend_set(command, adapter);
    let inventory = discover_targets(discovery)?;
    let all_targets = inventory.targets();
    let requested = gpu_ids.iter().copied().collect::<BTreeSet<_>>();
    let filtered = all_targets
        .into_iter()
        .filter(|target| requested.contains(&target.id.0))
        .filter(|target| target_supports(*target, command, adapter))
        .collect::<Vec<_>>();

    execute_targets(invocation, command, adapter, filtered)
}

fn execute_targets(
    invocation: &Invocation,
    command: Command,
    adapter: BackendAdapter,
    filtered: Vec<GpuTarget<'_>>,
) -> CliResult<Execution> {
    if filtered.is_empty() {
        return Err(CliError::new(format!(
            "no selected GPUs expose the {} backend required by {}",
            adapter.label(),
            command.name()
        )));
    }

    let mut results = Vec::with_capacity(filtered.len());
    for target in filtered {
        let result = match execute_target(command, adapter, &target, invocation) {
            Ok(output) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: true,
                output: Some(output),
                error: None,
            },
            Err(error) => TargetResult {
                gpu_id: Some(target.id.0),
                backend: adapter.label(),
                ok: false,
                output: None,
                error: Some(error.to_string()),
            },
        };
        results.push(result);
    }

    Ok(Execution {
        function: command.name(),
        backend: adapter.label().to_string(),
        warnings: Vec::new(),
        results,
    })
}

fn uncovered_target_ids(target_ids: &[u32], execution: &Execution) -> Vec<u32> {
    let covered = execution
        .results
        .iter()
        .filter_map(|result| result.gpu_id)
        .collect::<BTreeSet<_>>();
    target_ids
        .iter()
        .copied()
        .filter(|id| !covered.contains(id))
        .collect()
}

fn execute_list_gpus_auto(invocation: &Invocation) -> CliResult<Execution> {
    let inventory = discover_targets(BackendSet::Both)
        .or_else(|_| discover_targets(BackendSet::Nvapi))
        .or_else(|_| discover_targets(BackendSet::Nvml))?;
    let all_targets = inventory.targets();
    list_gpus_execution(
        Command::GetGpuList,
        "auto".to_string(),
        &all_targets,
        invocation,
        BackendAdapter::Nvapi,
    )
}

fn list_gpus_execution(
    command: Command,
    backend: String,
    all_targets: &[GpuTarget<'_>],
    invocation: &Invocation,
    adapter_filter: BackendAdapter,
) -> CliResult<Execution> {
    let selector = gpu_selector(invocation);
    let selected = select_targets(all_targets, &selector)?;
    let explicit_backend = invocation.backend != BackendChoice::Auto;
    let mut results = Vec::new();

    for target in selected {
        if explicit_backend && !target_supports(target, command, adapter_filter) {
            continue;
        }

        let (name, uuid) = if target.has_nvapi() {
            run(&target, QueryGpuInfo)
                .ok()
                .map(|report| (Some(report.output.name), report.output.uuid))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        results.push(TargetResult {
            gpu_id: Some(target.id.0),
            backend: if target.has_nvapi() && target.has_nvml() {
                "both"
            } else if target.has_nvapi() {
                "nvapi"
            } else {
                "nvml"
            },
            ok: true,
            output: Some(json!({
                "index": target.index,
                "gpu_id": target.id.0,
                "gpu_id_hex": format!("0x{:04X}", target.id.0),
                "pci_bus": target.id.pci_bus(),
                "backend_nvapi": target.has_nvapi(),
                "backend_nvml": target.has_nvml(),
                "uuid": uuid,
                "name": name,
            })),
            error: None,
        });
    }

    if results.is_empty() {
        return Err(CliError::new("no GPUs matched the selector"));
    }

    Ok(Execution {
        function: command.name(),
        backend,
        warnings: Vec::new(),
        results,
    })
}

fn discovery_backend_set(command: Command, adapter: BackendAdapter) -> BackendSet {
    match (command, adapter) {
        (Command::GetInfo, BackendAdapter::Nvapi) => BackendSet::Both,
        (Command::GetStatus, BackendAdapter::Nvapi) => BackendSet::Both,
        (Command::GetUuid, BackendAdapter::Nvapi) => BackendSet::Both,
        (Command::SetPstateLockViaMemRange, BackendAdapter::Nvapi) => BackendSet::Both,
        // get-power-limit tries NVML first then falls back to the NVAPI TGP
        // range inside one execute — both handles must be on the target
        // whichever adapter routes the run.
        (Command::GetPowerLimit, BackendAdapter::Nvapi) => BackendSet::Both,
        (Command::GetPowerLimit, BackendAdapter::Nvml) => BackendSet::Both,
        (_, BackendAdapter::Nvapi) => BackendSet::Nvapi,
        (_, BackendAdapter::Nvml) => BackendSet::Nvml,
    }
}

fn gpu_selector(invocation: &Invocation) -> GpuSelector {
    if invocation.gpu_specs.is_empty() {
        GpuSelector::all()
    } else {
        GpuSelector::from_specs(invocation.gpu_specs.clone())
    }
}

fn target_supports(target: GpuTarget<'_>, command: Command, adapter: BackendAdapter) -> bool {
    match adapter {
        BackendAdapter::Nvapi => {
            target.has_nvapi() && (command != Command::SetPstateLockViaMemRange || target.has_nvml())
        }
        BackendAdapter::Nvml => target.has_nvml(),
    }
}

fn execute_target(
    command: Command,
    adapter: BackendAdapter,
    target: &GpuTarget<'_>,
    invocation: &Invocation,
) -> CliResult<Value> {
    match command {
        Command::GetGpuList => unreachable!("get-gpu-list is handled before target execution"),
        Command::GetDisplayList => {
            let all = option_bool(invocation, "all", false)?;
            let displays = run(target, QueryDisplays { all })?.output;
            Ok(Value::Array(
                displays
                    .into_iter()
                    .map(|display| {
                        json!({
                            "display_id": format!("0x{:08X}", display.display_id),
                            "display_id_u32": display.display_id,
                            "connector": display.connector,
                            "flags_hex": format!("0x{:08X}", display.flags_bits),
                            "connected": display.connected,
                            "physically_connected": display.physically_connected,
                            "active": display.active,
                            "os_visible": display.os_visible,
                            "dynamic": display.dynamic,
                            "mst_root": display.mst_root,
                            "wireless": display.wireless,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetInfo => {
            let mut value = serde_json::to_value(run(target, QueryGpuInfo)?.output)?;
            // Augment with the max PCIe link generation from NVML
            // (`nvmlDeviceGetMaxPcieLinkGeneration`) — the platform/slot cap.
            // NVAPI's GpuInfo doesn't carry PCIe gen. Omitted when unsupported.
            if let Ok(nvml) = target.nvml()
                && let Some(map) = value.as_object_mut()
            {
                let (_current, max) = nvoc_core::nvml::query_nvml_pcie_link_gen(nvml, target.id.0);
                if let Some(max) = max {
                    map.insert("max_pcie_link_gen".to_string(), json!(max));
                }
            }
            Ok(value)
        }
        Command::GetUuid => {
            let info = run(target, QueryGpuInfo)?.output;
            Ok(Value::String(info.uuid.unwrap_or_default()))
        }
        Command::GetStatus => {
            let verbose = option_bool(invocation, "verbose", false)?;
            let mut value = serde_json::to_value(run(target, QueryGpuStatus)?.output)?;
            // The VFP table is large; omit it from get-status unless --verbose.
            if let Some(map) = value.as_object_mut() {
                if !verbose {
                    map.remove("vfp");
                    // power_monitor is the research-grade raw descriptor table
                    // (units/layout still under validation); keep only the
                    // confirmed power_rails in the default status output.
                    map.remove("power_monitor");
                }
                // NVAPI's power topology reports a dimensionless percentage and
                // is empty on most laptop GPUs. Augment with the live board
                // power draw (watts) from NVML (`nvmlDeviceGetPowerUsage`, the
                // same source nvidia-smi uses) when an NVML backend is present.
                if let Ok(nvml) = target.nvml()
                    && let Some(draw_w) =
                        nvoc_core::nvml::query_nvml_power_draw_watts(nvml, target.id.0)
                {
                    map.insert("power_draw_w".to_string(), json!(draw_w));
                }
                // Current enforced power limit (the live TGP cap), from NVML
                // `nvmlDeviceGetEnforcedPowerLimit` — the same "Current Power
                // Limit" nvidia-smi -q -d POWER reports (e.g. the `30W` in
                // `1W / 30W`). Preferred over the configurable management limit,
                // which returns NotSupported on most mobile GPUs. Available on
                // both mobile and desktop.
                if let Ok(nvml) = target.nvml()
                    && let Some(limit_w) =
                        nvoc_core::nvml::query_nvml_power_limit_watts(nvml, target.id.0)
                {
                    map.insert("power_limit_w".to_string(), json!(limit_w));
                }
                // Current D-Notifier (D0-notify / "extern power state") level,
                // e.g. "D3". D-Notifier shares the TGP power-policy table, so
                // its active cap silently clamps the TGP wall above — surfacing
                // it here makes that interaction visible. Only emitted when the
                // GPU exposes the private ClientPowerPoliciesGetInfo (0x67F31384)
                // AND reports an active level (mobile-only feature). With
                // --verbose, also include the full D1-D5 power-cap table (watts).
                if let Ok(Some(d)) = run(target, QueryNvapiDNotifier).map(|r| r.output) {
                    if let Some(active) = d.active {
                        map.insert("d_notifier".to_string(), json!(format!("D{active}")));
                    }
                    if verbose && !d.levels.is_empty() {
                        let table: Vec<serde_json::Value> = d
                            .levels
                            .iter()
                            .map(|l| {
                                json!({
                                    "level": format!("D{}", l.level),
                                    "watts": l.watts,
                                    "active": d.active == Some(l.level),
                                })
                            })
                            .collect();
                        map.insert("d_notifier_table".to_string(), json!(table));
                    }
                }
                // P0 voltage bounds from the private VoltRails status entry
                // (melonVolt path): current / target / effective / vBIOS /
                // VRM-max walls + min-hold + offset_ceiling. The effective wall
                // (index 4) is what the driver clamps to min(target, vbios_wall,
                // vrm_max_wall); offset_ceiling is how much higher the wall can
                // still go. Only the seven-value p0 block here — the raw
                // rail_descriptors / control / status arrays live in
                // get-volt-rails.
                if let Ok(Some(rails)) = run(target, QueryNvapiVoltRails).map(|r| r.output)
                    && let Some(p0) = volt_rails_p0_json(&rails)
                {
                    map.insert("p0_voltage".to_string(), p0);
                }
                // Per-rail power (watts) from NVAPI PowerMonitor GetStatus
                // Per-rail power (watts) from NVAPI PowerMonitor, keyed by the
                // descriptor's rail IDENTITY (not a fixed name) so it's correct
                // on every GPU: e.g. a 4060 laptop shows InputTotalBoard/
                // InputNvvdd/..., a desktop Turing shows InputPex12v1 (PCIe
                // slot)/InputExt12v8pin*/InputTotalBoard. Build a
                // { "<RailName>": <watts> } map from the power_rails array;
                // unnamed rails use "UNNAMED_<id>". The key carries a confidence
                // marker: plain (Measured), `~` (Inferred — disambiguated from a
                // shared offset), `?` (Ambiguous — full-board view). Unavailable
                // rails are omitted. Drop the raw array.
                if let Some(rails) = map.get("power_rails").and_then(|v| v.as_array()).cloned() {
                    let mut rail_map = serde_json::Map::new();
                    for r in &rails {
                        let name = r
                            .get("rail_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("UNNAMED");
                        let mw = r.get("pwr_mw").and_then(|v| v.as_u64()).unwrap_or(0);
                        if mw == 0 {
                            continue;
                        }
                        let confidence = r.get("confidence").and_then(|v| v.as_str()).unwrap_or("");
                        let key = match confidence {
                            "Measured" => name.to_string(),
                            "Inferred" => format!("{}~", name),
                            _ => format!("{}?", name), // Ambiguous; Unavailable filtered by mw==0
                        };
                        rail_map.insert(key, json!(mw as f64 / 1000.0));
                    }
                    if !rail_map.is_empty() {
                        map.insert("power_rails_w".to_string(), Value::Object(rail_map));
                    }
                    map.remove("power_rails");
                }
                // Bidirectional real-time PCIe bandwidth (MiB/s),
                // nvitop/HWMonitor-style, from NVML `nvmlDeviceGetPcieThroughput`.
                // TX = GPU→host, RX = host→GPU. Omitted per-direction when
                // unsupported (Maxwell+ only; vGPU unsupported).
                if let Ok(nvml) = target.nvml() {
                    let (tx, rx) =
                        nvoc_core::nvml::query_nvml_pcie_throughput_mibps(nvml, target.id.0);
                    if let Some(tx) = tx {
                        map.insert("pcie_tx_mibps".to_string(), json!(tx));
                    }
                    if let Some(rx) = rx {
                        map.insert("pcie_rx_mibps".to_string(), json!(rx));
                    }
                    if let Some(replay) =
                        nvoc_core::nvml::query_nvml_pcie_replay_counter(nvml, target.id.0)
                    {
                        map.insert("pcie_replay_counter".to_string(), json!(replay));
                    }
                    let (current_gen, max_gen) =
                        nvoc_core::nvml::query_nvml_pcie_link_gen(nvml, target.id.0);
                    if let Some(current) = current_gen {
                        map.insert("pcie_link_gen".to_string(), json!(current));
                    }
                    if let Some(max) = max_gen {
                        map.insert("pcie_max_link_gen".to_string(), json!(max));
                    }
                }
            }
            Ok(value)
        }
        Command::GetSettings => Ok(serde_json::to_value(run(target, QueryGpuSettings)?.output)?),
        Command::GetPublicVftable => get_vfp(target, invocation),
        Command::GetPowerLimit => {
            // Merged power-limit getter: the NVML power-management limits
            // (min/current/max) are the primary surface; where NVML is
            // unavailable or unsupported (or on an explicit --nvapi run),
            // fall back to the NVAPI TGP-watts range (min/default/max,
            // the old get-power-limit-range surface).
            match run(target, QueryPowerLimits) {
                Ok(power) => Ok(json!({
                    "source": "nvml",
                    "min_watt": power.output.min_watts,
                    "current_watt": power.output.current_watts,
                    "max_watt": power.output.max_watts,
                })),
                Err(nvml_error) => {
                    let range = run(target, QueryNvapiTgpWattRange)?.output;
                    match range {
                        Some(r) => Ok(json!({
                            "source": "nvapi_tgp_range",
                            "policy_index": r.policy_index,
                            "min_watt": r.min_watt,
                            "default_watt": r.default_watt,
                            "max_watt": r.max_watt,
                        })),
                        // NVAPI family absent too — surface the original
                        // NVML failure (the more likely user-facing cause).
                        None => Err(nvml_error.into()),
                    }
                }
            }
        }
        Command::GetPstateGlobalFreqOffset => get_clock_offset(target, adapter, invocation),
        Command::GetPstateFreqRange => {
            let pstates = run(target, QueryPstates)?.output;
            Ok(Value::Array(
                pstates
                    .into_iter()
                    .map(|pstate| {
                        json!({
                            "pstate": nvml_pstate_to_str(pstate.pstate),
                            "min_core_mhz": pstate.min_core_mhz,
                            "max_core_mhz": pstate.max_core_mhz,
                            "min_memory_mhz": pstate.min_memory_mhz,
                            "max_memory_mhz": pstate.max_memory_mhz,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetPStateLock => {
            // Native NVAPI P-State level table (the ref tool `-pstate` GET listing).
            // Mirrors the ref tool's "Level[N] P*.Max / Level[N+1] P*.Min" output:
            // level 0 is reserved for "P0.TDP" (the rated-TDP toggle, index 0 in
            // the ref tool's SETTER), then each present pstate contributes a .Max then
            // .Min slot. `index` is the value the ref tool's `-pstate:<index>` SETTER
            // takes; use it with set-pstate-native (TODO).
            // Query all 4 clock-domains (the private table exposes per-pstate
            // min/max for each domain: 0/1/3 are core-ish, 2 is memory on RTX
            // 4060 Laptop). `--domain` restricts to a single domain if given.
            let single_domain = option_one(invocation, "pstate-domain")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --domain: {e}")))?;
            let domains: Vec<usize> = match &single_domain {
                Some(d) => vec![*d],
                None => vec![0, 1, 2, 3],
            };
            // Pull each domain's table; bail on the first that's unsupported.
            let mut per_domain: Vec<Vec<nvoc_core::PStateLevelEntry>> = Vec::new();
            for d in &domains {
                let r = run(target, QueryNvapiPStateLevels { domain: *d })?.output;
                match r {
                    Some(mut info) => {
                        info.pstates.sort_by_key(|p| p.pstate);
                        per_domain.push(info.pstates);
                    }
                    None => {
                        return Ok(json!({"supported": false}));
                    }
                }
            }
            // Build the union of pstate numbers across all domains, ascending.
            let mut pstates: Vec<u8> = per_domain
                .first()
                .map(|v| v.iter().map(|p| p.pstate).collect())
                .unwrap_or_default();
            pstates.sort_unstable();
            pstates.dedup();
            // The set of pstates currently LOCKED (via PerfClientLimits
            // 0x39442CFB), from the private limit-status (0x9962C97C). A level's
            // pstate is "locked" when it's in this set — the only way to confirm
            // a SET actually took. None when the driver doesn't expose it.
            let locked: std::collections::HashSet<u8> = run(target, QueryNvapiPStateLockStatus)?
                .output
                .unwrap_or_default()
                .into_iter()
                .collect();
            let domain_mhz = |d: usize, pstate: u8, max: bool| -> Option<f64> {
                per_domain
                    .get(d)?
                    .iter()
                    .find(|p| p.pstate == pstate)
                    .and_then(|p| if max { p.max_mhz } else { p.min_mhz })
            };
            // Clock-domain identity (GPU-specific in general; resolved by the ref tool
            // via 0x57B5A5DF). User-confirmed on RTX 4060 Laptop and matching
            // NVML's pstate-limits naming: 0=Graphics/core, 2=Memory, 3=Video.
            // We emit one sub-object per domain named after NVML's domain, each
            // with max_frequency_mhz/min_frequency_mhz so the renderer compacts
            // them to "Frequency: Max X MHz, Min Y MHz" (and we omit zeroed
            // domains entirely to keep output terse).
            let domain_name = |d: usize| -> Option<&'static str> {
                match d {
                    0 => Some("graphics"),
                    1 => Some("host"), // domain 1 = Host (confirmed via TUI)
                    2 => Some("memory"),
                    3 => Some("video"),
                    _ => None,
                }
            };
            let mut levels: Vec<serde_json::Value> = Vec::new();
            for pstate in pstates {
                // One flat entry per pstate: max_<domain>_mhz / min_<domain>_mhz
                // sibling keys (NVML-style) so the renderer's compact-range
                // grouping yields one line per domain ("Graphics: Max X MHz,
                // Min Y MHz"). Plus a locked flag from 0x9962C97C. Pn.Max/Min
                // are the bounds of the SAME pstate. P0 often reports nothing
                // (dynamic, not pinned) and is skipped.
                let mut entry = serde_json::Map::new();
                entry.insert("pstate".to_string(), json!(format!("P{}", pstate)));
                if locked.contains(&pstate) {
                    entry.insert("locked".to_string(), json!(true));
                }
                for &d in &domains {
                    let Some(name) = domain_name(d) else { continue };
                    let min = domain_mhz(d, pstate, false);
                    let max = domain_mhz(d, pstate, true);
                    if max.unwrap_or(0.0) == 0.0 && min.unwrap_or(0.0) == 0.0 {
                        continue;
                    }
                    entry.insert(format!("max_{name}_mhz"), json!(max));
                    entry.insert(format!("min_{name}_mhz"), json!(min));
                }
                // Skip a pstate with no surviving domain ranges UNLESS it is
                // locked (a locked P0 may report all-zero clocks because it's
                // dynamic, but the user still wants to see the lock).
                let header_len = 1 + if locked.contains(&pstate) { 1 } else { 0 };
                if entry.len() <= header_len && !locked.contains(&pstate) {
                    continue;
                }
                levels.push(Value::Object(entry));
            }
            Ok(json!({
                "supported": true,
                "locked_pstates": locked.iter().copied().collect::<Vec<u8>>(),
                "pstates": levels,
            }))
        }
        Command::GetSupportedLegacyApplicationFreq => {
            let clocks = run(target, QuerySupportedApplicationsClocks)?.output;
            Ok(Value::Array(
                clocks
                    .into_iter()
                    .map(|clock| {
                        json!({
                            "memory_mhz": clock.memory_mhz,
                            "graphics_mhz": clock.graphics_mhz,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetFanInfo => {
            match adapter {
                BackendAdapter::Nvapi => {
                    // Private FanCoolers family: presence mask (GetInfo
                    // 0x65CE5BFC) + type/min/max (GetControl 0xCF86B990) +
                    // current speed/PWM (GetStatus 0x3CC2D181). RE'd from
                    // GPUMon pollFanSpeed. Speed fields are in the DRIVER's
                    // scale — on some GPUs that's the 0..65536 duty grid
                    // (2070 desktop observed), not physical RPM; the duty
                    // percent is the cross-checkable observable.
                    let coolers = run(target, QueryNvapiCoolerInfo)?.output;
                    Ok(json!({
                        "count": coolers.len(),
                        "coolers": coolers.iter().map(|c| json!({
                            "index": c.index,
                            "type": c.cooler_type,
                            "min": c.min,
                            "max": c.max,
                            "current": c.current,
                            "current_pwm_percent": c.current_pwm_percent,
                        })).collect::<Vec<_>>(),
                    }))
                }
                BackendAdapter::Nvml => {
                    let fan = run(target, QueryFanInfo)?.output;
                    Ok(json!({
                        "count": fan.count,
                        "min_percent": fan.min_speed,
                        "max_percent": fan.max_speed,
                    }))
                }
            }
        }
        Command::GetFanCurve => {
            let curves = run(target, GetFanCurves)?.output;
            Ok(json!({
                "curves": curves
                    .iter()
                    .map(|c| json!({
                        "index": c.index,
                        "points": c.points.iter().map(|p| json!({
                            "temp_c": p.temp_c,
                            "rpm": p.rpm,
                        })).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            }))
        }
        Command::SetFanCurve => {
            let index: u8 = invocation.positionals[0].parse().map_err(|e| {
                CliError::new(format!(
                    "invalid curve index {:?}: {e}",
                    invocation.positionals[0]
                ))
            })?;
            let points = parse_fan_curve_points(&invocation.positionals[1])?;
            if points.len() != 3 {
                return Err(CliError::new(format!(
                    "expected 3 points (temp:rpm,temp:rpm,temp:rpm), got {}",
                    points.len()
                )));
            }
            // Driver requires strictly-increasing temperature AND RPM lanes.
            for w in points.windows(2) {
                if w[1].temp_c <= w[0].temp_c || w[1].rpm <= w[0].rpm {
                    return Err(CliError::new(
                        "fan-curve points must be strictly increasing in both temperature and RPM",
                    ));
                }
            }
            let curve = run(target, SetFanCurve { index, points })?;
            Ok(json!({
                "applied": curve.output.applied.iter().map(|p| json!({
                    "temp_c": p.temp_c,
                    "rpm": p.rpm,
                })).collect::<Vec<_>>(),
            }))
        }
        Command::ResetFanCurveCmd => {
            // GPUMon's NVAPI fan reset: private FanPolicySetControl (NDA
            // 0x2B2A2A45, struct 0x214AC) — GET the policy block, OR
            // `1 << curve` into the +0x08 reset bitmask, SET. Works where the
            // public RestoreCoolerSettings is rejected with NOT_SUPPORTED
            // (desktop 3060/2070 without a user-mode cooler table).
            let curve = option_one(invocation, "curve")
                .map(|s| s.parse::<u8>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --curve: {e}")))?
                .unwrap_or(0);
            if curve >= 4 {
                return Err(CliError::new(format!(
                    "invalid --curve {curve}: slot range is 0-3"
                )));
            }
            run(target, ResetFanCurve { index: curve })?;
            Ok(json!({"applied": true, "curve": curve, "reset": true}))
        }
        Command::SetFanstopStatus => {
            // FanArbiterSet NDA 0x44CD3014 (struct 0x10144): toggle
            // zero-RPM fan stop for a curve slot. RE'd from GPUMon
            // setFanCurve's tail call.
            let enable = match invocation.positionals[0].to_lowercase().as_str() {
                "on" | "1" | "true" => true,
                "off" | "0" | "false" => false,
                other => {
                    return Err(CliError::new(format!(
                        "invalid fan-stop state {other:?}: expected on|off"
                    )))
                }
            };
            let curve = option_one(invocation, "curve")
                .map(|s| s.parse::<u8>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --curve: {e}")))?
                .unwrap_or(0);
            run(target, SetFanStop { curve_index: curve, enable })?;
            Ok(json!({"applied": true, "curve": curve, "fan_stop": enable}))
        }
        Command::GetTemperatureThresholds => {
            // Two backend flavours of "temperature threshold":
            //  - NVML: the classic Shutdown/Slowdown/GpuMax/GpsCurr/... table.
            //  - NVAPI: the private target-temp (温度墙) policy table — every
            //    policy slot the driver exposes (private GET-prime 0xC4554575).
            //    On RTX 4060 Laptop idx 2 is the "GPU Target Temperature" wall
            //    (matches nvidia-smi and NVML's GpsCurr channel); surfacing all
            //    slots here lets callers discover the right index per-GPU
            //    instead of hardcoding 2.
            //
            // Backend routing (per the user's explicit contract — no merging
            // while the index↔channel mapping is still being worked out):
            //  - `--nvapi`         → NVAPI policy indices only.
            //  - `--nvml` or auto  → NVML table only.
            let mut entries: Vec<Value> = Vec::new();
            if adapter == BackendAdapter::Nvapi {
                // Auto-discover the target-temp wall index (private GetInfo:
                // GPS idx, else acoustics fallback) so the "(TargetTemp)" tag
                // follows the driver's own choice instead of a hardcoded 2.
                let chosen_idx = run(target, QueryNvapiTargetTempPolicyIndex)
                    .ok()
                    .and_then(|r| r.output);
                let policies = run(target, QueryNvapiTargetTempPolicies)?.output;
                for p in policies {
                    // Name is just the index (the human renderer prefixes the
                    // "Threshold" label, so "2" -> "Threshold 2"). Only the
                    // auto-discovered wall slot gets the "(TargetTemp)" tag.
                    let is_wall = chosen_idx == Some(p.policy_index);
                    let name = if is_wall {
                        format!("{} (TargetTemp)", p.policy_index)
                    } else {
                        format!("{}", p.policy_index)
                    };
                    let mut entry = json!({
                        "name": name,
                        "policy_index": p.policy_index,
                        "celsius": p.celsius,
                    });
                    if let Some(obj) = entry.as_object_mut() {
                        if let Some(mn) = p.min {
                            obj.insert("min_c".into(), json!(mn));
                        }
                        if let Some(def) = p.default {
                            obj.insert("default_c".into(), json!(def));
                        }
                        if let Some(mx) = p.max {
                            obj.insert("max_c".into(), json!(mx));
                        }
                    }
                    entries.push(entry);
                }
            } else {
                let thresholds = run(target, QueryTemperatureThresholds)?.output;
                for item in thresholds {
                    entries.push(json!({"name": item.name, "celsius": item.celsius}));
                }
            }
            Ok(Value::Array(entries))
        }
        Command::GetLegacyTempSensor => {
            let sensors = run(target, QueryNvapiThermalSettings)?.output;
            Ok(Value::Array(
                sensors
                    .into_iter()
                    .map(|s| {
                        json!({
                            "target": format!("{:?}", s.target),
                            "controller": format!("{:?}", s.controller),
                            "current_c": s.current_c,
                            "min_c": s.min_c,
                            "max_c": s.max_c,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetPowerMode => {
            let s = run(target, GetPowerMode)?.output;
            Ok(json!({
                "supported": s.supported,
                "active": s.active,
                "mode_mask": s.mode_mask,
                "max_mode_idx": s.max_mode_idx,
            }))
        }
        Command::SetPowerMode => {
            let arg = invocation.positionals[0].to_lowercase();
            let max = match arg.as_str() {
                "max" | "high" | "performance" => true,
                "balanced" | "normal" | "adaptive" => false,
                _ => {
                    return Err(CliError::new(format!(
                        "invalid power mode {arg:?}: expected max|balanced"
                    )));
                }
            };
            run(target, SetPowerMode { max })?;
            Ok(json!({"applied": true, "power_mode": if max { "Max" } else { "Balanced" } }))
        }
        Command::GetThrottleReasons => {
            let reasons = run(target, QueryThrottleReasons)?.output;
            let reasons_json = Value::Array(
                reasons
                    .into_iter()
                    .map(|item| json!({"name": item.name, "active": item.active}))
                    .collect(),
            );
            // NVML violation status is queried off the same NVML handle and
            // appends the driver's cumulative per-policy violation times
            // (the "how long was each modality limiting" breakdown). It is
            // best-effort: if the device exposes no violation counters we
            // still return the throttle-reason snapshot.
            let violation = run(target, QueryViolationStatus)?.output;
            let violation_json = violation.map(|report| {
                json!({
                    "entries": report.entries.iter().map(|entry| {
                        json!({
                            "name": entry.name,
                            "seconds": entry.violation_time_ns as f64 / 1_000_000_000.0,
                        })
                    }).collect::<Vec<_>>(),
                    "since": format_reference_time(report.reference_time_us),
                })
            });
            Ok(json!({
                "reasons": reasons_json,
                "violation": violation_json,
            }))
        }
        Command::GetPublicPowerLimit => {
            // Power-limit half of the old get-tdp-temp-limits (NVAPI
            // ClientPowerPolicies TDP percent range).
            let limits = run(target, QueryTdpTempLimits)?.output;
            Ok(json!({
                "min_tdp_percent": limits.min_tdp.0,
                "default_tdp_percent": limits.default_tdp.0,
                "max_tdp_percent": limits.max_tdp.0,
            }))
        }
        Command::GetPublicTempLimit => {
            // Temp-limit half of the old get-tdp-temp-limits.
            let limits = run(target, QueryTdpTempLimits)?.output;
            Ok(json!({
                "min_temp_c": limits.min_temp.0,
                "default_temp_c": limits.default_temp.0,
                "max_temp_c": limits.max_temp.0,
                "curve": format!("{:?}", limits.throttle_curve),
            }))
        }
        Command::GetLegacyOvervoltRanges => {
            let ranges = run(target, QueryLegacyCoreOvervoltRanges)?.output;
            Ok(Value::Array(
                ranges
                    .into_iter()
                    .map(|(pstate, min, current, max)| {
                        json!({
                            "pstate": pstate_label(pstate),
                            "min_uv": min.0,
                            "current_uv": current.0,
                            "max_uv": max.0,
                        })
                    })
                    .collect(),
            ))
        }
        Command::GetLegacyP0CoreMaxVoltageDelta => {
            let delta = run(target, QueryLegacyP0CoreMaxVoltageDelta)?.output;
            Ok(json!({"max_delta_uv": delta.map(|v| v.0)}))
        }
        Command::GetLegacyGpcRailOvervoltLimit => {
            let pstate = option_pstate_nvapi(invocation)?;
            let voltage = run(target, QueryPstateBaseVoltage { pstate })?.output;
            Ok(json!({
                "pstate": pstate_label(voltage.pstate),
                "voltage_domain": voltage_domain_label(voltage.voltage_domain),
                "editable": voltage.editable,
                "voltage_uv": voltage.voltage.0,
                "delta_uv": voltage.delta.0,
                "min_delta_uv": voltage.min_delta.0,
                "max_delta_uv": voltage.max_delta.0,
            }))
        }
        Command::GetPublicGpcRailVoltBoost => {
            let boost = run(target, QueryVoltageBoost)?.output;
            Ok(json!({"voltage_boost_percent": boost.voltage_boost.map(|v| v.0)}))
        }
        Command::GetAutoboostStatus => {
            let state = run(target, QueryAutoBoost)?.output;
            Ok(json!({
                "enabled": state.enabled,
                "default_enabled": state.default_enabled,
            }))
        }
        Command::GetAutoboostSupport => {
            let api_type = parse_api_restriction_api(&invocation.positionals[0])?;
            let state = run(target, QueryApiRestriction { api_type })?.output;
            Ok(json!({
                "api": api_restriction_api_label(state.api_type),
                "restricted": state.restricted,
            }))
        }
        Command::GetEdid => {
            let display_id = parse_display_id(&invocation.positionals[0])?;
            let edid = run(target, QueryEdid { display_id })?.output;
            let interpreted: Vec<Value> = parse_edid(&edid.bytes)
                .into_iter()
                .map(|(k, v)| json!({ k: v }))
                .collect();
            Ok(json!({
                "display_id": format!("0x{:08X}", edid.display_id),
                "bytes": edid.bytes.len(),
                "edid_hex": bytes_to_upper_hex(&edid.bytes),
                "interpreted": interpreted,
            }))
        }
        Command::SetPstateGlobalFreqOffset => {
            let domain = option_domain(invocation, ClockDomain::Graphics)?;
            set_clock_offset(target, adapter, invocation, domain)
        }
        Command::SetPublicTgpPercent => {
            let percent = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
            run(
                target,
                SetNvapiPowerLimits {
                    limits: vec![Percentage(percent)],
                },
            )?;
            Ok(json!({"applied": true, "power_percent": percent}))
        }
        Command::SetPpabStatus => {
            let active = parse_bool(&invocation.positionals[0])?;
            run(target, SetNvapiDynamicBoost { active })?;
            Ok(json!({"applied": true, "dynamic_boost": active}))
        }
        Command::SetPStateLock => {
            // Pin the active NVAPI P-State (mode-1 PstateSelect via 0x39442CFB).
            // <PSTATE> is a P-State number (P0, P3, ... or bare 0,3,...); also
            // settable via --pstate. To ALSO lock a frequency, use
            // set-locked-clocks-mhz; to clear, use reset-pstate-native.
            use nvoc_core::NvapiPStateNativeLock;
            let raw = option_one(invocation, "pstate")
                .map(str::to_string)
                .or_else(|| invocation.positionals.first().map(|s| s.to_string()))
                .ok_or_else(|| {
                    CliError::new(
                        "set-pstate-native requires a P-State, e.g. `set-pstate-native P3`",
                    )
                })?;
            let pstate = raw
                .trim_start_matches('P')
                .trim_start_matches('p')
                .parse::<u8>()
                .map_err(|e| CliError::new(format!("invalid P-State '{raw}': {e}")))?;
            run(
                target,
                SetNvapiPStateNative {
                    lock: NvapiPStateNativeLock::PstateOnly { pstate },
                },
            )?;
            Ok(json!({"applied": true, "pstate": format!("P{pstate}")}))
        }
        Command::ResetPStateLock => {
            run(
                target,
                SetNvapiPStateNative {
                    lock: nvoc_core::NvapiPStateNativeLock::Reset,
                },
            )?;
            Ok(json!({"applied": true}))
        }

        Command::SetPowerLimit => {
            // Merged TGP-watt setter: NVAPI path writes ClientPowerPolicies
            // (SetNvapiTgpWatt, mobile watts-form TGP slider, honors
            // --policy-index); NVML path writes the power-management limit
            // (SetNvmlPowerLimit, the classic nvidia-smi -pl input). On auto the
            // NVAPI path is preferred (it understands the mobile TGP table);
            // NVML is the fallback for desktops without ClientPowerPolicies.
            let watts = parse_u32_unit(&invocation.positionals[0], "w", "watt")?;
            match adapter {
                BackendAdapter::Nvapi => {
                    let policy_index = option_one(invocation, "policy-index")
                        .map(|s| s.parse::<usize>())
                        .transpose()
                        .map_err(|e| CliError::new(format!("invalid --policy-index: {e}")))?;
                    let mw = run(
                        target,
                        SetNvapiTgpWatt {
                            watts,
                            policy_index,
                        },
                    )?
                    .output;
                    Ok(json!({"applied": true, "backend": "nvapi", "tgp_watt": watts, "tgp_mw": mw}))
                }
                BackendAdapter::Nvml => {
                    run(target, SetNvmlPowerLimit { watts })?;
                    Ok(json!({"applied": true, "backend": "nvml", "power_watt": watts}))
                }
            }
        }
        Command::ResetPowerLimit => {
            let policy_index = option_one(invocation, "policy-index")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --policy-index: {e}")))?;
            let default_mw = run(target, ResetNvapiTgpWatt { policy_index })?.output;
            Ok(json!({
                "applied": true,
                "default_watt": default_mw.map(|mw| mw as f64 / 1000.0),
            }))
        }
        Command::GetDNotifier => {
            let info = run(target, QueryNvapiDNotifier)?.output;
            Ok(match info {
                Some(d) => {
                    let levels: Vec<serde_json::Value> = d
                        .levels
                        .iter()
                        .map(|l| {
                            json!({
                                "level": format!("D{}", l.level),
                                "watts": l.watts,
                                "active": d.active == Some(l.level),
                            })
                        })
                        .collect();
                    json!({
                        "active": d.active.map(|a| format!("D{a}")),
                        "levels": levels,
                    })
                }
                None => json!({"supported": false}),
            })
        }
        Command::SetDNotifier => {
            let level: u8 = invocation.positionals[0]
                .parse()
                .map_err(|e| CliError::new(format!("invalid D-Notifier level: {e}")))?;
            run(target, SetNvapiDNotifier { level })?;
            Ok(json!({"applied": true, "dnotifier_level": format!("D{level}")}))
        }
        Command::GetPowerMizer => {
            let power_source: u32 = invocation
                .positionals
                .first()
                .map(|v| v.parse())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid POWER_SOURCE: {e}")))?
                .unwrap_or(1);
            if power_source != 1 && power_source != 2 {
                return Err(CliError::new("POWER_SOURCE must be 1 (AC) or 2 (DC)"));
            }
            let out = run(target, QueryNvapiPowerMizer { power_source })?.output;
            Ok(match out {
                Some(mode) => json!({
                    "power_source": power_source,
                    "mode_raw": mode,
                    "mode": if mode == 6 { "first" } else if mode == 7 { "second" } else { "unknown" },
                }),
                None => json!({"supported": false}),
            })
        }
        Command::GetCoreVoltageControl => {
            let out = run(target, QueryNvapiCoreVoltageControl)?.output;
            Ok(match out {
                Some(v) => json!({"value_raw": v}),
                None => json!({"supported": false}),
            })
        }
        Command::SetCoreVoltageControl => {
            let value = parse_usize(&invocation.positionals[0], "value")? as u32;
            let out = run(target, SetNvapiCoreVoltageControl { value })?.output;
            Ok(match out {
                Some(()) => json!({"applied": true, "value": value}),
                None => json!({"supported": false}),
            })
        }
        Command::GetPmgrArbiter => {
            let out = run(target, QueryNvapiPmgrVoltageArbiter)?.output;
            Ok(match out {
                Some(values) => json!({"values": values}),
                None => json!({"supported": false}),
            })
        }
        Command::SetPmgrArbiter => {
            let parts: Vec<&str> = invocation.positionals[0].split(',').collect();
            if parts.len() != 11 {
                return Err(CliError::new("CSV must contain exactly 11 comma-separated dwords"));
            }
            let mut values = [0u32; 11];
            for (i, part) in parts.iter().enumerate() {
                values[i] = part
                    .trim()
                    .parse()
                    .map_err(|e| CliError::new(format!("invalid dword {i}: {e}")))?;
            }
            let out = run(target, SetNvapiPmgrVoltageArbiter { values })?.output;
            Ok(match out {
                Some(()) => json!({"applied": true}),
                None => json!({"supported": false}),
            })
        }
        Command::GetRatedTdp => {
            let out = run(target, QueryNvapiRatedTdp)?.output;
            Ok(match out {
                Some((control_mode, caps, raw)) => json!({
                    "control_mode": control_mode,
                    "info_capabilities": caps,
                    "status_raw": raw,
                }),
                None => json!({"supported": false}),
            })
        }
        Command::SetPrivatePermanentPstateLockUser => {
            let level = parse_usize(&invocation.positionals[0], "level")? as u32;
            let out = run(target, SetNvapiPerfLevelLock { level })?.output;
            Ok(json!({
                "applied": true,
                "level": out.applied,
                "note": "level is an index into this GPU's real P-State list (see get-pstate-native), not a fixed P8..P0 enum; no release value exists — reboot/driver reload clears the lock",
            }))
        }
        Command::GetTempSim => {
            let out = run(target, QueryNvapiThermalSim)?.output;
            Ok(match out {
                Some((enable, temp)) => json!({"enabled": enable, "temperature_c": temp}),
                None => json!({"supported": false}),
            })
        }
        Command::SetTempSim => {
            let temp = parse_i32_unit(&invocation.positionals[0], "temp-c", "celsius")?;
            let out = run(target, SetNvapiThermalSim { temperature_c: temp })?.output;
            Ok(match out {
                Some(()) => json!({"applied": true, "temperature_c": temp}),
                None => json!({"supported": false}),
            })
        }
        Command::ResetTempSim => {
            let out = run(target, DisableNvapiThermalSim)?.output;
            Ok(match out {
                Some(()) => json!({"applied": true, "disabled": true}),
                None => json!({"supported": false}),
            })
        }
        Command::GetVoltRailInfo => {
            let rails = run(target, QueryNvapiVoltRails)?.output;
            Ok(match rails {
                Some(r) => {
                    json!({
                        "rail_mask": format!("0x{:08X}", r.rail_mask),
                        "p0": volt_rails_p0_json(&r),
                        "rail_descriptors": r.rail_descriptors.iter().map(|d| json!({
                            "rail_bit": d.rail_bit,
                            "type": d.entry_type(),
                        })).collect::<Vec<_>>(),
                        "control": r.control.iter().map(|e| json!({
                            "rail_bit": e.rail_bit, "type": e.entry_type, "values_uV": e.values,
                        })).collect::<Vec<_>>(),
                        "status": r.status.iter().map(|e| json!({
                            "rail_bit": e.rail_bit, "type": e.entry_type, "values_uV": e.values,
                        })).collect::<Vec<_>>(),
                    })
                }
                None => json!({"supported": false}),
            })
        }
        Command::SetVoltRailLimit => {
            // Merged volt-rail setter: --offset (default) writes a µV offset
            // directly (SetNvapiVoltRailOffset); --target takes an absolute mV
            // target and derives the µV offset from the live control/status
            // snapshot (SetNvapiVoltRailTarget). Both share the melonVolt
            // write path (0x87C55C8A); the driver clamps the effective wall to
            // min(target, vbios_wall, vrm_max_wall) regardless of mode.
            let rail_bit = parse_usize(&invocation.positionals[0], "rail-bit")? as u32;
            let expect_type = option_one(invocation, "expect-type")
                .map(|s| s.parse::<u32>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --expect-type: {e}")))?;
            let target_mode = option_bool(invocation, "target", false)?;
            let offset_explicit = option_bool(invocation, "offset", false)?;
            if target_mode && offset_explicit {
                return Err(CliError::new(
                    "--offset and --target are mutually exclusive",
                ));
            }
            if target_mode {
                // Absolute-target convenience: the caller thinks in mV (one
                // decimal allowed — 10/20-series rail step is 12.5 mV), we
                // recover the factory/default wall from the live control
                // offset + status target wall and derive the µV offset to
                // write. The driver still clamps the effective wall itself.
                let target_mv = parse_mv_f64(&invocation.positionals[1])?;
                if !target_mv.is_finite() {
                    return Err(CliError::new(format!(
                        "target {target_mv}mV is not a finite number"
                    )));
                }
                #[allow(non_snake_case)] // uV-suffixed local matches the nvapi-rs naming
                let target_uV = i32::try_from((target_mv * 1000.0).round() as i64).map_err(|_| {
                    CliError::new(format!("target {target_mv}mV overflows the µV range"))
                })?;
                let out = run(
                    target,
                    SetNvapiVoltRailTarget {
                        rail_bit,
                        target_uV,
                        expected_type: expect_type,
                    },
                )?
                .output;
                Ok(match out {
                    Some(a) => json!({
                        "applied": true,
                        "mode": "target",
                        "rail_bit": a.rail_bit,
                        "target_uV": a.target_uV,
                        // factory/default wall recovered from
                        // (status target wall) − (control current offset)
                        "base_wall_uV": a.base_wall_uV,
                        // derived µV offset actually written
                        "offset_uV": a.offset_uV,
                        "previous_offset_uV": a.previous_offset_uV,
                        "applied_uV": a.applied_uV,
                        // effective wall read back after SET (clamped to
                        // min(target, vbios_wall, vrm_max_wall)); 0 = driver
                        // hasn't refreshed status yet — re-run get-volt-rail-info.
                        "effective_wall_uV": a.effective_wall_uV,
                    }),
                    None => json!({"supported": false}),
                })
            } else {
                let uv = parse_i32_unit(&invocation.positionals[1], "uv", "microvolt")?;
                let out = run(
                    target,
                    SetNvapiVoltRailOffset {
                        rail_bit,
                        offset_uV: uv,
                        expected_type: expect_type,
                    },
                )?
                .output;
                Ok(match out {
                    Some(a) => json!({
                        "applied": true,
                        "mode": "offset",
                        "rail_bit": a.rail_bit,
                        "previous_uV": a.previous_uV,
                        "applied_uV": a.applied_uV,
                        // effective wall read back after SET (clamped to
                        // min(target, vbios_wall, vrm_max_wall)); 0 = driver
                        // hasn't refreshed status yet — re-run get-volt-rail-info.
                        "effective_wall_uV": a.effective_wall_uV,
                    }),
                    None => json!({"supported": false}),
                })
            }
        }
        Command::GetPrivateFreqDomainInfo => {
            let ctrl = run(target, QueryNvapiClkDomains)?.output;
            Ok(match ctrl {
                Some(c) => json!({
                    "controllable_mask": format!("0x{:08X}", c.mask),
                    "entries": c.entries.iter().map(|e| json!({
                        "bit": e.bit,
                        "domain": e.domain().map(|d| format!("{:?}", d)).unwrap_or_else(|| "Unknown".to_string()),
                        "type": e.entry_type,
                        // false = the protocol doesn't marshal this record
                        // type's value fields (e.g. type 0x02) — values_kHz
                        // below is NOT driver data.
                        "value_modifiable": e.value_modifiable,
                        // the record's 8 value dwords (V2 rec+268..296);
                        // slot semantics driver-opaque, slot 0 = signed
                        // frequency offset per the article
                        "values_kHz": e.values_kHz,
                    })).collect::<Vec<_>>(),
                }),
                None => json!({"supported": false}),
            })
        }
        Command::GetPrivateVftable => {
            let vfp = run(target, QueryNvapiClkVfPoints)?.output;
            Ok(match vfp {
                Some(v) => json!({
                    // bank0 = masks[0..4], bank1 = masks[4..8] (2048 bits each)
                    "masks": v.masks.iter().map(|m| format!("0x{:016X}", m)).collect::<Vec<_>>(),
                    // contiguous same-type runs — bank 0 packs multiple
                    // domains back-to-back (GPC curve, mem pstate bins,
                    // XBAR curve, HOST curve, ...), so plot ONE curve per
                    // vf_curve segment, not the whole point list
                    "segments": v.segments.iter().map(|s| json!({
                        "bank": s.bank,
                        // EMPIRICAL advisory attribution (ordinal-based;
                        // confirm by domain-offset A/B)
                        "domain": s.domain_hint.as_str(),
                        "kind": match s.kind {
                            nvoc_core::ClkVfSegmentKind::VfCurve => "vf_curve",
                            nvoc_core::ClkVfSegmentKind::PstateBins => "pstate_bins",
                        },
                        "type": s.record_type,
                        "start_index": s.start_index,
                        "end_index": s.end_index,
                        "count": s.count,
                        "voltage_uV_min": s.voltage_uV_min,
                        "voltage_uV_max": s.voltage_uV_max,
                        "freq_default_mhz_min": s.freq_default_mhz_min,
                        "freq_default_mhz_max": s.freq_default_mhz_max,
                    })).collect::<Vec<_>>(),
                    "points": v.points.iter().map(|p| json!({
                        "bank": p.bank,
                        "index": p.index,
                        "type": p.record_type,
                        // the V/F grid axis (µV): 450000 = 450 mV
                        "voltage_uV": p.voltage_uV,
                        // default MHz at this voltage (public "default" column)
                        "freq_default_mhz": p.freq_default_mhz,
                        // current MHz = default + applied offset
                        "freq_current_mhz": p.freq_current_mhz,
                    })).collect::<Vec<_>>(),
                }),
                None => json!({"supported": false}),
            })
        }
        Command::GetPrivateFreqDomainStatus => {
            // with a domain argument: detailed single-domain measure — the
            // raw second-sample {counter, timestamp, extra} plus the
            // accepted protocol form (V1 0x10020 / V2 0x20020), for
            // counter-unit calibration (Pascal M) and forensics
            if let Some(pos) = invocation.positionals.first() {
                let domain_bit = parse_clk_domain(pos)?;
                let detail = run(target, QueryNvapiClkDomainFreqDetail { domain_bit })?.output;
                return Ok(match detail {
                    Some(d) => json!({
                        "domain_bit": domain_bit,
                        "domain": format!("{:?}", d.domain),
                        "freq_mhz": (d.freq_mhz * 1000.0).round() / 1000.0,
                        // 1 = V1 magic 0x10020 (u32 counter), 2 = V2 0x20020 (u64)
                        "protocol": d.protocol,
                        // raw second-sample values
                        "counter": d.counter,
                        "timestamp_ns": d.timestamp_ns,
                        "extra": d.extra,
                    }),
                    None => json!({"supported": false, "domain_bit": domain_bit}),
                });
            }
            // no argument: batch-measure EVERY controllable domain
            let domains = run(target, QueryNvapiClkDomains)?
                .output
                .map(|c| c.entries.iter().map(|e| e.bit).collect::<Vec<u32>>())
                .unwrap_or_default();
            let freqs = run(
                target,
                QueryNvapiClkDomainFreqsBatch {
                    domains: domains.clone(),
                },
            )?
            .output;
            Ok(match freqs {
                Some(fs) => {
                    // readability census: requested domains that don't come
                    // back were skipped by the per-domain fallback (the
                    // driver refuses their measure — e.g. Pascal gpc/xbar)
                    let returned: Vec<u32> = fs.iter().map(|f| f.domain as u32).collect();
                    let skipped: Vec<String> = domains
                        .iter()
                        .filter(|b| !returned.contains(b))
                        .map(|b| format!("{}(bit {b})", parse_clk_domain_name(*b)))
                        .collect();
                    json!({
                        "freqs": fs.iter().map(|f| json!({
                            "domain_bit": f.domain as u32,
                            "domain": format!("{:?}", f.domain),
                            "freq_mhz": (f.freq_mhz * 1000.0).round() / 1000.0,
                        })).collect::<Vec<_>>(),
                        "skipped_unreadable": skipped,
                    })
                }
                None => json!({"supported": false}),
            })
        }
        Command::SetPrivateVftablePointOffset => {
            // DANGEROUS V/F curve write via private SetControl 0xFEC00D04.
            //
            // VALUE semantics:
            //   --freq-mode: mode 0, VALUE is a kHz frequency offset (200000 = +200 MHz)
            //   --raw:       mode 1, VALUE is the raw f-offset control value
            //   default:     mode 1, VALUE is a MHz target — translated to a raw
            //                f-offset control value via the universal g(def) prior
            //                (effect_mhz = C(def)*(delta-D0)) using this point's
            //                default frequency + domain class from get-clk-vf-points.
            let bank = parse_usize(&invocation.positionals[0], "bank")?;
            let idx = parse_usize(&invocation.positionals[1], "index")?;
            let value: i32 = invocation.positionals[2]
                .trim()
                .parse()
                .map_err(|e| CliError::new(format!("invalid VALUE: {e}")))?;
            let raw_flag = option_bool(invocation, "raw", false)?;
            let raw_converted = option_bool(invocation, "raw-converted", false)?;
            if raw_flag && raw_converted {
                return Err(CliError::new(
                    "--raw and --raw-converted are mutually exclusive",
                ));
            }
            // default (no flag) = freq_mode, same as public VFP but reaches
            // xbar/host; --freq-mode is the explicit alias of the default.
            let freq_mode = !raw_flag && !raw_converted;

            let (mode_label, raw_value, translated_mhz) = if freq_mode {
                ("freq_offset", value, Some(value as f64 / 1000.0))
            } else if raw_flag {
                ("raw_f_offset_control", value, None)
            } else {
                // --raw-converted: translate MHz target -> raw f-offset via g(def)
                let vfp = run(target, QueryNvapiClkVfPoints)?.output;
                let point = vfp
                    .as_ref()
                    .and_then(|v| v.points.iter().find(|p| p.bank as usize == bank && p.index as usize == idx))
                    .ok_or_else(|| CliError::new("could not read default frequency for this point — pass --raw to write a raw control value"))?;
                let def = point.freq_default_mhz as u32;
                if def == 0 {
                    return Err(CliError::new(
                        "default frequency is 0 for this point — pass --raw to write a raw control value",
                    ));
                }
                let class = match vfp.as_ref().and_then(|v| {
                    v.segments
                        .iter()
                        .find(|s| {
                            s.bank as usize == bank
                                && idx >= s.start_index as usize
                                && idx <= s.end_index as usize
                        })
                        .map(|s| s.domain_hint)
                }) {
                    Some(nvoc_core::ClkVfDomainHint::Xbar)
                    | Some(nvoc_core::ClkVfDomainHint::Host) => nvoc_core::ClkVfDomainClass::Fabric,
                    _ => nvoc_core::ClkVfDomainClass::Graphics,
                };
                let delta = nvoc_core::clk_vf_delta_for_target(def, value as f64, class)
                    .ok_or_else(|| {
                        CliError::new(format!("no g(def) prior for def={def} MHz — pass --raw"))
                    })?;
                ("raw_f_offset_control", delta, Some(value as f64))
            };

            let out = run(
                target,
                SetNvapiVfpPointPrivate {
                    bank,
                    idx,
                    freq_mode,
                    value: raw_value as u32,
                },
            )?
            .output;
            Ok(match out {
                Some(retained) => json!({
                    "applied": true,
                    "bank": bank,
                    "index": idx,
                    "mode": mode_label,
                    "value": raw_value,
                    "unit": translated_mhz.map(|m| format!("{:.0} MHz", m)).unwrap_or_else(|| "raw".to_string()),
                    "retained": retained,
                }),
                None => json!({"supported": false}),
            })
        }
        Command::SetPrivateVftableRangeOffset => {
            let bank = parse_usize(&invocation.positionals[0], "bank")?;
            let start = parse_usize(&invocation.positionals[1], "start")?;
            let end = parse_usize(&invocation.positionals[2], "end")?;
            if start > end {
                return Err(CliError::new("start must be <= end"));
            }
            let val: i32 = invocation.positionals[3]
                .trim()
                .parse()
                .map_err(|e| CliError::new(format!("invalid VALUE: {e}")))?;
            let raw_flag = option_bool(invocation, "raw", false)?;
            let raw_converted = option_bool(invocation, "raw-converted", false)?;
            if raw_flag && raw_converted {
                return Err(CliError::new(
                    "--raw and --raw-converted are mutually exclusive",
                ));
            }
            let freq_mode = !raw_flag && !raw_converted;

            if freq_mode {
                // mode 0 kHz offset: the batch range method only writes mode 1,
                // so loop the single-point setter (verified safe). One RMW per
                // point — slower than a single SET but correct for mode 0.
                // The setter readback-verifies; if the driver silently rejects
                // (e.g. CMP 170HX doesn't support mode 0), hi returns Ok(None)
                // — surface that as "not supported" instead of "applied".
                for idx in start..=end {
                    let out = run(
                        target,
                        SetNvapiVfpPointPrivate {
                            bank,
                            idx,
                            freq_mode: true,
                            value: val as u32,
                        },
                    )?
                    .output;
                    if out.is_none() {
                        return Ok(json!({"supported": false}));
                    }
                }
                Ok(json!({
                    "applied": true,
                    "bank": bank,
                    "start": start,
                    "end": end,
                    "mode": "freq_offset",
                    "value": val,
                    "unit": "kHz",
                    "points_written": end - start + 1,
                }))
            } else if raw_flag {
                // mode 1: same raw control word on every point
                let out = run(
                    target,
                    SetNvapiVfpRangePrivate {
                        bank,
                        start,
                        end,
                        delta_mhz: val as i16,
                    },
                )?
                .output;
                Ok(match out {
                    Some(()) => json!({
                        "applied": true,
                        "bank": bank,
                        "start": start,
                        "end": end,
                        "mode": "raw_f_offset_control",
                        "value": val,
                        "unit": "raw",
                        "points_written": end - start + 1,
                    }),
                    None => json!({"supported": false}),
                })
            } else {
                // --raw-converted: translate the MHz target per-point via g(def)
                let vfp = run(target, QueryNvapiClkVfPoints)?.output;
                let vfp = vfp
                    .as_ref()
                    .ok_or_else(|| CliError::new("could not read V/F points — pass --raw"))?;
                let class_for = |idx: usize| -> nvoc_core::ClkVfDomainClass {
                    match vfp
                        .segments
                        .iter()
                        .find(|s| {
                            s.bank as usize == bank
                                && idx >= s.start_index as usize
                                && idx <= s.end_index as usize
                        })
                        .map(|s| s.domain_hint)
                    {
                        Some(nvoc_core::ClkVfDomainHint::Xbar)
                        | Some(nvoc_core::ClkVfDomainHint::Host) => {
                            nvoc_core::ClkVfDomainClass::Fabric
                        }
                        _ => nvoc_core::ClkVfDomainClass::Graphics,
                    }
                };
                let mut deltas: Vec<i16> = Vec::with_capacity(end - start + 1);
                for idx in start..=end {
                    let point = vfp
                        .points
                        .iter()
                        .find(|p| p.bank as usize == bank && p.index as usize == idx)
                        .ok_or_else(|| {
                            CliError::new(format!("point {idx} not present — pass --raw"))
                        })?;
                    let def = point.freq_default_mhz as u32;
                    if def == 0 {
                        return Err(CliError::new(format!(
                            "point {idx} default frequency is 0 — pass --raw"
                        )));
                    }
                    let delta = nvoc_core::clk_vf_delta_for_target(def, val as f64, class_for(idx))
                        .ok_or_else(|| {
                            CliError::new(format!(
                                "no g(def) prior for point {idx} def={def} — pass --raw"
                            ))
                        })?;
                    deltas.push(delta.clamp(-1000, 1000) as i16);
                }
                let out = run(
                    target,
                    SetNvapiVfpRangePerPointPrivate {
                        bank,
                        start,
                        end,
                        deltas: deltas.clone(),
                    },
                )?
                .output;
                Ok(match out {
                    Some(()) => json!({
                        "applied": true,
                        "bank": bank,
                        "start": start,
                        "end": end,
                        "mode": "raw_f_offset_control",
                        "target_mhz": val,
                        "per_point_raw_values": deltas,
                        "points_written": end - start + 1,
                    }),
                    None => json!({"supported": false}),
                })
            }
        }
        Command::SetPrivateFreqDomainGlobalOffset => {
            // DANGEROUS clock write — see xbar.txt safety recipe. The medium
            // layer snapshots the full V2 GetControl block, version-gates
            // (magic 0x261A4), patches a copy, SETs, readbacks, and restores
            // on mismatch. --temporary additionally restores the snapshot
            // before returning (the article's reversible experiment recipe).
            // --slot picks which of the record's 8 value dwords to write
            // (default 0 = the article's signed frequency offset; the other
            // slots are driver-opaque range/voltage terms — identify by
            // A/B with get-clk-domain-freq).
            let domain_bit = parse_clk_domain(&invocation.positionals[0])?;
            #[allow(non_snake_case)] // kHz suffix matches the nvapi-rs field naming
            let offset_kHz = parse_i32_unit(&invocation.positionals[1], "khz", "kilohertz")?;
            let slot = option_one(invocation, "slot")
                .map(|s| s.parse::<u32>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --slot: {e}")))?
                .unwrap_or(0);
            // reject out-of-range slots HERE with a clear message — the
            // medium layer also guards, but its ArgumentRange error reads
            // the same as a readback mismatch ("data out of range"), which
            // made slot >= 8 look like an unprotected write. Live slot
            // behavior on a 4060 (XBAR): 0/1 accepted (0 = freq offset),
            // 2-4 rejected by the driver (NVAPI error), 5-7 readback
            // mismatch — only 0 is documented semantics; treat the rest as
            // experiments.
            if slot >= 8 {
                return Err(CliError::new(format!(
                    "invalid --slot {slot}: the record has 8 value dwords (0-7)"
                )));
            }
            let temporary = option_one(invocation, "temporary")
                .map(parse_bool)
                .transpose()?
                .unwrap_or(false);
            let out = run(
                target,
                SetNvapiClkDomainOffset {
                    domain_bit,
                    offset_kHz,
                    slot,
                    temporary,
                },
            )?
            .output;
            Ok(match out {
                Some(a) => json!({
                    "applied": true,
                    "bit": a.bit,
                    "type": a.entry_type,
                    "slot": a.slot,
                    "previous_kHz": a.previous_kHz,
                    "applied_kHz": a.applied_kHz,
                    "values_kHz": a.values_kHz,
                    "temporary_restored": a.temporary_restored,
                }),
                None => json!({"supported": false}),
            })
        }
        Command::SetGpuClock => {
            // GPU frequency perf-cap (PerfLimitsSetStatus NDA 0x32CA4983, the
            // ref tool `-gpuclk:<MHz>`): clamp the perf max/min frequency to a
            // cap value — NOT an offset, NOT a P-state lock. The positional is
            // the max cap; --min the min cap (defaults to max). Use
            // reset-perf-freq-caps to clear (not -1).
            let max_mhz = parse_i32_unit(&invocation.positionals[0], "mhz", "mhz")?;
            if max_mhz < 0 {
                return Err(CliError::new(
                    "negative MHz is not valid for set-perf-freq-caps; use `reset-perf-freq-caps` to clear the cap",
                ));
            }
            let min_mhz = option_one(invocation, "min")
                .map(|s| s.parse::<i32>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --min: {e}")))?
                .unwrap_or(max_mhz);
            if min_mhz < 0 {
                return Err(CliError::new(
                    "negative --min is not valid for set-perf-freq-caps; use `reset-perf-freq-caps` to clear the cap",
                ));
            }
            run(
                target,
                SetNvapiPerfFreqCap {
                    cap: NvapiPerfFreqCap::Cap {
                        max_khz: (max_mhz as u32).saturating_mul(1000),
                        min_khz: (min_mhz as u32).saturating_mul(1000),
                    },
                },
            )?;
            Ok(json!({
                "applied": true,
                "max_mhz": max_mhz,
                "min_mhz": min_mhz,
            }))
        }
        Command::ResetGpuClock => {
            // Clear the GPU frequency perf-cap (PerfLimitsSetStatus NDA, the
            // -gpuclk:-1 path): enable=0 on both entries, no frequency written.
            run(
                target,
                SetNvapiPerfFreqCap {
                    cap: NvapiPerfFreqCap::Reset,
                },
            )?;
            Ok(json!({"applied": true, "reset": true}))
        }
        Command::SetPrivateTargetTempLimit => {
            // NVAPI-only SET of one target-temperature (温度墙) policy slot.
            // `--policy-index` picks the slot (default 2 = the wall); the
            // driver accepts other indices but may reject them — useful for
            // probing which slots are actually writable.
            let celsius = parse_celsius_f32(&invocation.positionals[0])?;
            let policy_index = option_one(invocation, "policy-index")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|e| CliError::new(format!("invalid --policy-index: {e}")))?;
            run(
                target,
                SetNvapiTargetTemp {
                    celsius,
                    policy_index,
                },
            )?;
            let applied_index = policy_index.unwrap_or(2);
            Ok(json!({
                "applied": true,
                "policy_index": applied_index,
                "celsius": celsius,
            }))
        }
        Command::SetTempLimit => {
            let celsius = parse_i32_unit(&invocation.positionals[0], "c", "celsius")?;
            match adapter {
                BackendAdapter::Nvapi => {
                    if option_one(invocation, "domain").is_some() {
                        return Err(CliError::new(
                            "--domain is only valid on the NVML path of set-temp-limit",
                        ));
                    }
                    run(
                        target,
                        SetNvapiSensorLimits {
                            limits: vec![nvoc_core::Celsius(celsius).into()],
                        },
                    )?;
                    Ok(json!({"applied": true, "thermal_limit_c": celsius}))
                }
                BackendAdapter::Nvml => {
                    // --domain picks the NVML threshold to write: gpu
                    // (default) = the GPU max-temp threshold; acoustic =
                    // the ACOUSTIC_CURR target temp (the old
                    // set-acoustic-temp-c; Linux-native channel — Windows
                    // rejects the NVML threshold setter, use
                    // set-private-target-temp-limit / --nvapi there).
                    match option_one(invocation, "domain").unwrap_or("gpu") {
                        "gpu" => {
                            run(target, SetTemperatureLimit { celsius })?;
                            Ok(json!({
                                "applied": true,
                                "domain": "gpu",
                                "thermal_limit_c": celsius,
                            }))
                        }
                        "acoustic" => {
                            run(target, SetNvmlAcousticTemp { celsius })?;
                            Ok(json!({
                                "applied": true,
                                "domain": "acoustic",
                                "acoustic_target_temp_c": celsius,
                            }))
                        }
                        other => Err(CliError::new(format!(
                            "invalid --domain {other:?}; expected gpu or acoustic"
                        ))),
                    }
                }
            }
        }
        Command::SetFanSpeed => {
            // Merged fan-speed setter: --percent (default) dispatches to the
            // cooler-level percent path (NVAPI SetCoolerLevels / NVML
            // SetFanSpeed); --rpm dispatches to the private FanCoolerSetControl
            // RPM simulation path (NVAPI-only). The --rpm path is rejected on
            // NVML with a clear error.
            let rpm_mode = option_bool(invocation, "rpm", false)?;
            if rpm_mode {
                if adapter != BackendAdapter::Nvapi {
                    return Err(CliError::new(
                        "set-fan-speed --rpm requires --nvapi (NVML has no RPM-direct fan simulation)",
                    ));
                }
                // Private FanCoolerSetControl (NDA 0xEB44E8AA): RPM-direct
                // fan simulation. RE'd from GPUMon setFanSim: RMW the
                // control block, patch enable+level per cooler type. Use
                // reset-fan-speed --rpm to disable simulation.
                let rpm_raw = invocation.positionals[0]
                    .parse::<i32>()
                    .map_err(|e| CliError::new(format!("invalid RPM: {e}")))?;
                if rpm_raw < 0 {
                    return Err(CliError::new(
                        "negative RPM is not valid for set-fan-speed --rpm; use `reset-fan-speed --rpm` to disable simulation",
                    ));
                }
                let cooler = option_one(invocation, "cooler")
                    .map(|s| s.parse::<u32>())
                    .transpose()
                    .map_err(|e| CliError::new(format!("invalid --cooler: {e}")))?;
                let r = run(
                    target,
                    SetFanRpm {
                        cooler_index: cooler,
                        rpm: Some(rpm_raw as u32),
                    },
                )?;
                let coolers: Vec<Value> = r
                    .output
                    .iter()
                    .map(|c| {
                        json!({
                            "cooler_index": c.cooler_index,
                            "cooler_type": c.cooler_type,
                            "min_rpm": c.min_rpm,
                            "max_rpm": c.max_rpm,
                            "applied_rpm": c.applied_rpm,
                        })
                    })
                    .collect();
                Ok(json!({
                    "applied": true,
                    "mode": "rpm",
                    "coolers": coolers,
                }))
            } else {
                set_fan_percent(target, adapter, invocation)
            }
        }
        Command::SetFreqLock => set_locked_clocks(target, adapter, invocation),
        Command::SetGpcVoltLock => {
            let voltage_target = parse_nvapi_locked_voltage_target(&invocation.positionals[0])?;
            run(
                target,
                SetGpcVoltLock {
                    voltage_target,
                    feedback: option_bool(invocation, "feedback", false)?,
                },
            )?;
            Ok(json!({"applied": true, "target": invocation.positionals[0]}))
        }
        Command::OemOcScanner => {
            let start = option_bool(invocation, "start", false)?;
            let stop = option_bool(invocation, "stop", false)?;
            let revert = option_bool(invocation, "revert", false)?;
            let status = option_bool(invocation, "status", false)?;
            let background_on = option_bool(invocation, "background-on", false)?;
            let background_off = option_bool(invocation, "background-off", false)?;
            let incomplete = option_bool(invocation, "incomplete", false)?;
            if background_on || background_off {
                let out = run(target, SetNvapiBackgroundOcScanner { enable: background_on })?
                    .output;
                return Ok(match out {
                    Some(()) => json!({"applied": true, "background_scanner": background_on}),
                    None => json!({"supported": false}),
                });
            }
            if incomplete {
                let out = run(target, QueryNvapiOcScannerIncomplete)?.output;
                return Ok(match out {
                    Some(()) => json!({"queried": true, "incomplete_results": "accepted"}),
                    None => json!({"supported": false}),
                });
            }
            let action = match (start, stop, revert, status) {
                (true, false, false, false) => OemOcScannerAction::Start,
                (false, true, false, false) => OemOcScannerAction::Stop,
                (false, false, true, false) => OemOcScannerAction::Revert,
                (false, false, false, true) => OemOcScannerAction::Status,
                _ => {
                    return Err(CliError::new(
                        "exactly one of --start / --stop / --revert / --status (or --background-on/--background-off/--incomplete) is required",
                    ));
                }
            };
            run(target, OemOcScanner { action })?;
            Ok(json!({
                "applied": true,
                "action": match action {
                    OemOcScannerAction::Start => "start",
                    OemOcScannerAction::Stop => "stop",
                    OemOcScannerAction::Revert => "revert",
                    OemOcScannerAction::Status => "status",
                }
            }))
        }
        Command::SetPrivateForcedPstateLockUser => {
            let pstate = parse_u32(&invocation.positionals[0], "pstate")?;
            let set_type = match invocation.options.get("set-type").and_then(|v| v.first()) {
                Some(s) => parse_u32(s, "set-type")?,
                None => 2,
            };
            run(target, SetForcePstate { pstate, set_type })?;
            Ok(json!({"applied": true, "pstate": pstate, "set_type": set_type}))
        }
        Command::RestartDisplayDriver => {
            run(target, RestartDisplayDriver)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetPrivateForcedPstateLockUser => {
            run(target, ResetForcePstate)?;
            Ok(json!({"applied": true, "action": "release"}))
        }
        Command::SetBatteryBoost2Status => {
            let enable = parse_bool(&invocation.positionals[0])?;
            run(target, SetBb2Active { enable })?;
            Ok(json!({"applied": true, "bb2": enable}))
        }
        Command::SetWhisperMode2Status => {
            let enable = parse_bool(&invocation.positionals[0])?;
            run(target, SetWm2Active { enable })?;
            let mut out = json!({"applied": true, "whispermode2": enable});
            if let Some(mode_raw) = invocation.options.get("mode").and_then(|v| v.first()) {
                let raw = mode_raw.trim().to_ascii_lowercase();
                let (mode, label) = match raw.as_str() {
                    "0" | "quieter" => (Wm2AcousticMode::Quieter, "quieter"),
                    "1" | "quiet" => (Wm2AcousticMode::Quiet, "quiet"),
                    "2" | "balanced" => (Wm2AcousticMode::Balanced, "balanced"),
                    _ => {
                        return Err(CliError::new(
                            "mode must be quieter, quiet, or balanced (or 0/1/2)",
                        ));
                    }
                };
                run(target, SetWm2Mode { mode })?;
                out["mode"] = json!(label);
            }
            Ok(out)
        }
        Command::SetPublicVftablePointOffset => {
            let point = parse_usize(&invocation.positionals[0], "point")?;
            let mhz = parse_i32_unit(&invocation.positionals[1], "mhz", "mhz")?;
            run(
                target,
                SetPublicVftablePointOffset {
                    point,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({"applied": true, "point": point, "delta_mhz": mhz}))
        }
        Command::SetPublicVftableRangeOffset => {
            let start = parse_usize(&invocation.positionals[0], "start")?;
            let end = parse_usize(&invocation.positionals[1], "end")?;
            if start > end {
                return Err(CliError::new("start point must be <= end point"));
            }
            let mhz = parse_i32_unit(&invocation.positionals[2], "mhz", "mhz")?;
            run(
                target,
                SetPublicVftableRangeOffset {
                    start,
                    end,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({"applied": true, "start": start, "end": end, "delta_mhz": mhz}))
        }
        Command::SetPstateLockViaMemRange => {
            let first = parse_nvml_pstate(&invocation.positionals[0])?;
            let second_raw = invocation
                .positionals
                .get(1)
                .map(String::as_str)
                .unwrap_or(&invocation.positionals[0]);
            let second = parse_nvml_pstate(second_raw)?;
            let (range, min_mhz, max_mhz) = match adapter {
                BackendAdapter::Nvapi => {
                    run(
                        target,
                        SetNvapiPstateLock {
                            first_pstate: first,
                            second_pstate: second,
                        },
                    )?
                    .output
                }
                BackendAdapter::Nvml => {
                    run(
                        target,
                        SetNvmlPstateLock {
                            first_pstate: first,
                            second_pstate: second,
                        },
                    )?
                    .output
                }
            };
            Ok(json!({
                "applied": true,
                "pstate_range": range,
                "min_lock_mhz": min_mhz,
                "max_lock_mhz": max_mhz,
            }))
        }
        Command::SetLegacyApplicationFreqLock => {
            let memory_mhz = parse_u32_unit(&invocation.positionals[0], "mhz", "mhz")?;
            let graphics_mhz = parse_u32_unit(&invocation.positionals[1], "mhz", "mhz")?;
            run(
                target,
                SetApplicationsClocks {
                    memory_mhz,
                    graphics_mhz,
                },
            )?;
            Ok(json!({
                "applied": true,
                "memory_mhz": memory_mhz,
                "graphics_mhz": graphics_mhz,
            }))
        }
        Command::SetLegacyGpcRailOvervoltLimit => {
            let delta_uv = parse_i32_unit(&invocation.positionals[0], "uv", "microvolt")?;
            let pstate = option_pstate_nvapi(invocation)?;
            run(
                target,
                SetPstateBaseVoltage {
                    pstate,
                    delta_uv: MicrovoltsDelta(delta_uv),
                },
            )?;
            Ok(json!({
                "applied": true,
                "pstate": pstate_label(pstate),
                "delta_uv": delta_uv,
            }))
        }
        Command::SetOvervoltUv => {
            let delta_uv = parse_i32_unit(&invocation.positionals[0], "uv", "microvolt")?;
            let result = run(
                target,
                SetNvapiOvervolt {
                    delta_uv: MicrovoltsDelta(delta_uv),
                },
            )?;
            Ok(json!({
                "applied": true,
                "overvolt_delta_uv": delta_uv,
                "driver_ov_entries": result.output.driver_ov_entries,
                "note": if result.output.driver_ov_entries { "" } else {
                    "driver reports no global OV entries — SET accepted but silently ignored on this GPU (observed on Ada mobile)"
                },
            }))
        }
        Command::SetPublicGpcRailVoltBoost => {
            let percent = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
            run(
                target,
                SetVoltageBoost {
                    boost: Percentage(percent),
                },
            )?;
            Ok(json!({"applied": true, "voltage_boost_percent": percent}))
        }
        Command::SetAutoboostStatus => {
            let enabled = parse_bool(&invocation.positionals[0])?;
            run(target, SetAutoboostStatus { enabled })?;
            Ok(json!({"applied": true, "enabled": enabled}))
        }
        Command::ResetAutoboostStatus => {
            let enabled = parse_bool(&invocation.positionals[0])?;
            run(target, ResetAutoboostStatus { enabled })?;
            Ok(json!({"applied": true, "enabled": enabled}))
        }
        Command::SetAutoboostSupport => {
            let api_type = parse_api_restriction_api(&invocation.positionals[0])?;
            let restricted = parse_api_restriction_state(&invocation.positionals[1])?;
            run(
                target,
                SetAutoboostSupport {
                    api_type,
                    restricted,
                },
            )?;
            Ok(json!({
                "applied": true,
                "api": invocation.positionals[0],
                "restricted": restricted,
            }))
        }
        Command::SetEdid => {
            let display_id = parse_display_id(&invocation.positionals[0])?;
            let edid = parse_edid_hex(&invocation.positionals[1])?;
            let bytes = edid.len();
            run(
                target,
                SetEdid {
                    display_id,
                    bytes: edid,
                },
            )?;
            Ok(json!({
                "applied": true,
                "display_id": format!("0x{display_id:08X}"),
                "bytes": bytes,
            }))
        }
        Command::ClearEdid => {
            let display_id = parse_display_id(&invocation.positionals[0])?;
            run(target, ClearEdid { display_id })?;
            Ok(json!({
                "applied": true,
                "display_id": format!("0x{display_id:08X}"),
            }))
        }
        Command::SetLegacyFreq => {
            // Legacy (Kepler) SetClocks 0x6F151055: single absolute MHz
            // value + --domain core (default) | mem. The untouched clock is
            // passed as 0 to the legacy call (Kepler path untested).
            let mhz = parse_u32_unit(&invocation.positionals[0], "mhz", "mhz")?;
            let domain = option_one(invocation, "domain")
                .unwrap_or("core")
                .trim()
                .to_ascii_lowercase();
            let (core_mhz, memory_mhz) = match domain.as_str() {
                "core" | "gpu" | "graphics" => (mhz, 0),
                "mem" | "memory" => (0, mhz),
                other => {
                    return Err(CliError::new(format!(
                        "invalid --domain {other:?}; expected core or mem"
                    )))
                }
            };
            run(
                target,
                SetLegacyClocks {
                    core_mhz,
                    memory_mhz,
                },
            )?;
            Ok(json!({
                "applied": true,
                "domain": domain,
                "mhz": mhz,
                "core_mhz": core_mhz,
                "memory_mhz": memory_mhz,
            }))
        }
        Command::ResetLegacyApplicationFreqLock => {
            run(target, ResetLegacyApplicationFreqLock)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetFreqLock => {
            let domain = option_domain(invocation, ClockDomain::Graphics)?;
            match adapter {
                BackendAdapter::Nvapi => {
                    run(target, ResetVfpFrequencyLock { domain })?;
                }
                BackendAdapter::Nvml => {
                    run(target, ResetFreqLock { domain })?;
                }
            }
            Ok(json!({"applied": true, "domain": domain_label(domain)}))
        }
        Command::ResetFanSpeed => {
            // Merged fan reset: default restores the NVAPI cooler levels /
            // NVML default fan speed; --rpm (NVAPI-only) instead disables
            // fan-speed simulation — RMW the control block and clear the
            // cooler's enable bit → auto/driver control. Without --cooler
            // the --rpm path resets EVERY present cooler.
            if option_bool(invocation, "rpm", false)? {
                if adapter != BackendAdapter::Nvapi {
                    return Err(CliError::new(
                        "reset-fan-speed --rpm requires --nvapi (NVML has no RPM-direct fan simulation)",
                    ));
                }
                let cooler = option_one(invocation, "cooler")
                    .map(|s| s.parse::<u32>())
                    .transpose()
                    .map_err(|e| CliError::new(format!("invalid --cooler: {e}")))?;
                let r = run(
                    target,
                    SetFanRpm {
                        cooler_index: cooler,
                        rpm: None,
                    },
                )?;
                let coolers: Vec<Value> = r
                    .output
                    .iter()
                    .map(|c| json!({ "cooler_index": c.cooler_index }))
                    .collect();
                Ok(json!({
                    "applied": true,
                    "reset": true,
                    "mode": "rpm",
                    "coolers": coolers,
                }))
            } else {
                reset_fan(target, adapter, invocation)
            }
        }
        Command::ResetPublicVftableOffset => {
            let domain = option_vfp_reset_domain(invocation)?;
            run(target, ResetPublicVftableOffset { domain })?;
            Ok(json!({"applied": true, "domain": vfp_reset_domain_label(domain)}))
        }
        Command::ResetPublicVftableGpcLock => {
            run(target, ResetPublicVftableGpcLock)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetPrivateVftableOffset => {
            let bank = parse_usize(&invocation.positionals[0], "bank")?;
            if bank > 1 {
                return Err(CliError::new("bank must be 0 or 1"));
            }
            // --domain gpc|xbar|host|mem: restrict the reset to that
            // domain's segments within the bank (per-point mode-0/value-0
            // writes via the private point setter — the same write the
            // whole-bank reset performs, scoped to the segment's index
            // range from get-private-vftable's advisory attribution).
            if let Some(domain_raw) = option_one(invocation, "domain") {
                let hint = match domain_raw.trim().to_ascii_lowercase().as_str() {
                    "gpc" | "core" | "gpu" | "graphics" => ClkVfDomainHint::Gpc,
                    "xbar" => ClkVfDomainHint::Xbar,
                    "host" => ClkVfDomainHint::Host,
                    "mem" | "memory" => ClkVfDomainHint::Mem,
                    other => {
                        return Err(CliError::new(format!(
                            "invalid --domain {other:?}; expected gpc, xbar, host, or mem"
                        )))
                    }
                };
                let vfp = run(target, QueryNvapiClkVfPoints)?
                    .output
                    .ok_or_else(|| CliError::new("private V/F-POINTS family not supported"))?;
                let mut reset = 0usize;
                for segment in vfp
                    .segments
                    .iter()
                    .filter(|s| s.bank as usize == bank && s.domain_hint == hint)
                {
                    for idx in segment.start_index as usize..=segment.end_index as usize {
                        run(
                            target,
                            SetNvapiVfpPointPrivate {
                                bank,
                                idx,
                                freq_mode: true,
                                value: 0,
                            },
                        )?;
                        reset += 1;
                    }
                }
                if reset == 0 {
                    return Err(CliError::new(format!(
                        "no {:?}-attributed segments found on bank {bank} (see get-private-vftable segments)",
                        domain_raw.trim()
                    )));
                }
                return Ok(json!({
                    "applied": true,
                    "bank": bank,
                    "domain": domain_raw.trim(),
                    "mode": "freq_offset_clear",
                    "points_reset": reset,
                }));
            }
            let out = run(target, ResetNvapiVfpPrivate { bank })?.output;
            Ok(match out {
                Some(count) => json!({
                    "applied": true,
                    "bank": bank,
                    "mode": "freq_offset_clear",
                    "points_reset": count,
                }),
                None => json!({"supported": false}),
            })
        }
        Command::ResetPublicTgpPercent => {
            run(target, ResetNvapiPowerLimits)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetTempLimit => {
            run(target, ResetNvapiSensorLimits)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetLegacyGpcRailOvervoltLimit => {
            run(target, ResetLegacyGpcRailOvervoltLimit)?;
            Ok(json!({"applied": true}))
        }
        Command::ResetPstateGlobalFreqOffset => {
            // Bulk-clear every touched (pstate, domain) offset pair by
            // default; --domain filters the reset to one clock domain
            // (only that domain's offsets are set back to 0).
            let domain_filter = option_one(invocation, "domain")
                .map(parse_domain)
                .transpose()?;
            let info = run(target, QueryGpuInfo)?.output;
            let mut offsets = info
                .pstate_limits
                .iter()
                .flat_map(|(&pstate, limits)| {
                    limits
                        .iter()
                        .filter(|&(_, info)| info.frequency_delta.is_some())
                        .map(move |(&domain, _)| (pstate, domain))
                })
                .collect::<Vec<_>>();
            if let Some(domain) = domain_filter {
                offsets.retain(|&(_, d)| d == domain);
                if offsets.is_empty() {
                    return Err(CliError::new(format!(
                        "no P-State clock offsets set for domain {}",
                        domain_label(domain)
                    )));
                }
            }
            run(target, ResetPstateGlobalFreqOffset { offsets })?;
            Ok(json!({
                "applied": true,
                "domain": domain_filter.map(domain_label).unwrap_or("all"),
            }))
        }
        Command::ResetPublicGpcRailVoltBoost => {
            run(
                target,
                SetVoltageBoost {
                    boost: Percentage(0),
                },
            )?;
            Ok(json!({"applied": true, "voltage_boost_percent": 0}))
        }
    }
}

fn get_vfp(target: &GpuTarget<'_>, invocation: &Invocation) -> CliResult<Value> {
    // Default: dump every domain the public VFP table exposes (graphics 0..N
    // plus the trailing memory entries, e.g. 127..131 on 30/40 series);
    // --domain gpc|memory narrows to one segment via the per-domain table.
    let domains: Vec<ClockDomain> = match option_one(invocation, "domain") {
        Some(raw) => vec![parse_domain(raw)?],
        None => vec![ClockDomain::Graphics, ClockDomain::Memory],
    };
    let indexed = option_bool(invocation, "indexed", true)?;
    let infer_missing_default = if option_bool(invocation, "no-infer-missing-default", false)? {
        false
    } else {
        option_bool(invocation, "infer-missing-default", true)?
    };

    let mut points = Vec::new();
    let mut missing_domains = Vec::new();
    for &domain in &domains {
        match run(
            target,
            QueryDomainVfpPoints {
                domain,
                infer_missing_default,
                indexed,
            },
        ) {
            Ok(out) => {
                for (index, point) in out.output {
                    points.push(json!({
                        "domain": domain_label(domain),
                        "index": index,
                        "voltage_uv": point.voltage.0,
                        "voltage_mv": point.voltage.0 as f64 / 1000.0,
                        "frequency_khz": point.frequency.0,
                        "frequency_mhz": point.frequency.0 as f64 / 1000.0,
                        "delta_khz": point.delta.0,
                        "delta_mhz": point.delta.0 as f64 / 1000.0,
                        "default_frequency_khz": point.default_frequency.0,
                        "default_frequency_mhz": point.default_frequency.0 as f64 / 1000.0,
                    }));
                }
            }
            // A GPU without a memory V/F segment (or a driver that rejects the
            // domain) is not fatal for the default all-domains dump — report
            // it and still emit the graphics curve.
            Err(err) if domains.len() > 1 => missing_domains.push(json!({
                "domain": domain_label(domain),
                "error": err.to_string(),
            })),
            Err(err) => return Err(err.into()),
        }
    }

    Ok(json!({
        "domain": if domains.len() == 1 { json!(domain_label(domains[0])) } else { json!("all") },
        "indexed": indexed,
        "infer_missing_default": infer_missing_default,
        "missing_domains": missing_domains,
        "points": points,
    }))
}

fn get_clock_offset(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let domain = option_domain(invocation, ClockDomain::Graphics)?;
    match adapter {
        BackendAdapter::Nvapi => {
            let pstate = option_pstate_nvapi(invocation)?;
            let settings = run(target, QueryGpuSettings)?.output;
            let offset_khz = settings
                .pstate_deltas
                .get(&pstate)
                .and_then(|domains| domains.get(&domain))
                .map(|delta| delta.0)
                .unwrap_or(0);
            Ok(json!({
                "domain": domain_label(domain),
                "pstate": pstate_label(pstate),
                "offset_mhz": offset_khz as f64 / 1000.0,
                "offset_khz": offset_khz,
            }))
        }
        BackendAdapter::Nvml => {
            let pstate = option_pstate_nvml(invocation)?;
            let offset = run(target, QueryClockOffset { domain, pstate })?.output;
            Ok(json!({
                "domain": domain_label(domain),
                "pstate": nvml_pstate_to_str(pstate),
                "offset_mhz": offset.mhz,
            }))
        }
    }
}

fn set_clock_offset(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
    domain: ClockDomain,
) -> CliResult<Value> {
    let mhz = parse_i32_unit(&invocation.positionals[0], "mhz", "mhz")?;
    match adapter {
        BackendAdapter::Nvapi => {
            let pstate = option_pstate_nvapi(invocation)?;
            run(
                target,
                SetPstateClockOffset {
                    pstate,
                    domain,
                    delta: KilohertzDelta(mhz_to_khz_i32(mhz)?),
                },
            )?;
            Ok(json!({
                "applied": true,
                "backend": adapter.label(),
                "domain": domain_label(domain),
                "pstate": pstate_label(pstate),
                "offset_mhz": mhz,
            }))
        }
        BackendAdapter::Nvml => {
            let pstate = option_pstate_nvml(invocation)?;
            run(
                target,
                SetClockOffset {
                    domain,
                    pstate,
                    mhz,
                },
            )?;
            Ok(json!({
                "applied": true,
                "backend": adapter.label(),
                "domain": domain_label(domain),
                "pstate": nvml_pstate_to_str(pstate),
                "offset_mhz": mhz,
            }))
        }
    }
}

fn set_fan_percent(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let level = parse_u32_unit(&invocation.positionals[0], "%", "percent")?;
    let fan = option_one(invocation, "fan").unwrap_or("all");
    let policy = option_one(invocation, "policy").unwrap_or("manual");

    match adapter {
        BackendAdapter::Nvapi => {
            let cooler_target = parse_cooler_target(fan)?;
            let policy = parse_nvapi_cooler_policy(policy)?;
            run(
                target,
                SetCoolerLevels {
                    policy,
                    level,
                    cooler_target,
                },
            )?;
            Ok(json!({
                "applied": true,
                "fan": fan,
                "policy": policy_label(policy),
                "level_percent": level,
            }))
        }
        BackendAdapter::Nvml => {
            let policy = parse_nvml_fan_control_policy(policy)?;
            let fan_indices = nvml_fan_indices(target, fan)?;
            for fan_index in &fan_indices {
                run(
                    target,
                    SetFanSpeed {
                        fan_index: *fan_index,
                        policy,
                        level,
                    },
                )?;
            }
            Ok(json!({
                "applied": true,
                "fan_indices": fan_indices,
                "policy": format!("{policy:?}"),
                "level_percent": level,
            }))
        }
    }
}

fn set_locked_clocks(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let min_mhz = parse_u32_unit(&invocation.positionals[0], "mhz", "mhz")?;
    let max_mhz = parse_u32_unit(&invocation.positionals[1], "mhz", "mhz")?;
    if min_mhz > max_mhz {
        return Err(CliError::new("min MHz must be <= max MHz"));
    }
    let domain = option_domain(invocation, ClockDomain::Graphics)?;

    match adapter {
        BackendAdapter::Nvapi => {
            run(
                target,
                SetVfpFrequencyLock {
                    domain,
                    upper: Kilohertz(mhz_to_khz_u32(max_mhz)?),
                    lower: Some(Kilohertz(mhz_to_khz_u32(min_mhz)?)),
                },
            )?;
        }
        BackendAdapter::Nvml => {
            run(
                target,
                SetLockedClocks {
                    domain,
                    min_mhz,
                    max_mhz,
                },
            )?;
        }
    }

    Ok(json!({
        "applied": true,
        "domain": domain_label(domain),
        "min_mhz": min_mhz,
        "max_mhz": max_mhz,
    }))
}

fn reset_fan(
    target: &GpuTarget<'_>,
    adapter: BackendAdapter,
    invocation: &Invocation,
) -> CliResult<Value> {
    let fan = option_one(invocation, "fan").unwrap_or("all");
    match adapter {
        BackendAdapter::Nvapi => {
            if !fan.eq_ignore_ascii_case("all") {
                return Err(CliError::new(
                    "reset-fan-speed with a specific --fan requires --nvml; NVAPI resets all coolers",
                ));
            }
            run(target, ResetCoolerLevels)?;
            Ok(json!({"applied": true, "fan": fan}))
        }
        BackendAdapter::Nvml => {
            let fan_indices = nvml_fan_indices(target, fan)?;
            for fan_index in &fan_indices {
                run(
                    target,
                    ResetFanSpeed {
                        fan_index: *fan_index,
                    },
                )?;
            }
            Ok(json!({"applied": true, "fan_indices": fan_indices}))
        }
    }
}

fn nvml_fan_indices(target: &GpuTarget<'_>, raw: &str) -> CliResult<Vec<u32>> {
    if raw.eq_ignore_ascii_case("all") {
        let fan_count = run(target, QueryFanInfo)?.output.count;
        return Ok((0..fan_count).collect());
    }
    Ok(vec![parse_u32(raw, "fan")?])
}

fn parse_api_restriction_api(raw: &str) -> CliResult<nvml_wrapper::enum_wrappers::device::Api> {
    use nvml_wrapper::enum_wrappers::device::Api;
    match raw.trim().to_ascii_lowercase().as_str() {
        "app-clocks" | "application-clocks" => Ok(Api::ApplicationClocks),
        "auto-boost" | "autoboost" => Ok(Api::AutoBoostedClocks),
        other => Err(CliError::new(format!(
            "invalid API {other:?}; expected app-clocks or auto-boost"
        ))),
    }
}

fn api_restriction_api_label(api_type: nvml_wrapper::enum_wrappers::device::Api) -> &'static str {
    use nvml_wrapper::enum_wrappers::device::Api;
    match api_type {
        Api::ApplicationClocks => "app-clocks",
        Api::AutoBoostedClocks => "auto-boost",
    }
}

fn voltage_domain_label(domain: nvoc_core::VoltageDomain) -> &'static str {
    match domain {
        nvoc_core::VoltageDomain::Core => "core",
        nvoc_core::VoltageDomain::Undefined => "undefined",
        _ => "unknown",
    }
}

fn bytes_to_upper_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

/// Interpret a raw EDID base block into a list of `(label, value)` pairs.
///
/// Ported from the recovered commit 4b31b1a (`auto-optimizer/src/human.rs::parse_edid`):
/// decodes the manufacturer PNP code, product code, manufacture date, screen size,
/// gamma, DPMS/color features, the native detailed timing, and the `0xFC`/`0xFF`/`0xFD`
/// descriptor tags (model name / serial number / range limits). Returns an empty vec
/// for anything that is not a valid 128-byte EDID base block.
fn parse_edid(edid: &[u8]) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    if edid.len() < 128 || &edid[0..8] != b"\x00\xFF\xFF\xFF\xFF\xFF\xFF\x00" {
        return out;
    }

    let mfg = u16::from_be_bytes([edid[8], edid[9]]);
    let mfg_id = format!(
        "{}{}{}",
        (((mfg >> 10) & 0x1F) as u8 + b'A' - 1) as char,
        (((mfg >> 5) & 0x1F) as u8 + b'A' - 1) as char,
        ((mfg & 0x1F) as u8 + b'A' - 1) as char,
    );
    out.push(("Manufacturer".into(), json!(mfg_id)));

    let product_code = u16::from_le_bytes([edid[10], edid[11]]);
    out.push((
        "Product Code".into(),
        json!(format!("0x{:04X}", product_code)),
    ));

    let s_no = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
    if s_no != 0 {
        out.push(("Serial Number".into(), json!(s_no)));
    }

    let week = edid[16];
    let year = edid[17] as u16 + 1990;
    out.push((
        "Manufactured".into(),
        if week > 0 && week <= 54 {
            json!(format!("Week {}, {}", week, year))
        } else {
            json!(year.to_string())
        },
    ));

    let digital = (edid[20] & 0x80) != 0;
    out.push((
        "Input Signal".into(),
        json!(if digital { "Digital" } else { "Analog" }),
    ));

    let width_cm = edid[21];
    let height_cm = edid[22];
    if width_cm > 0 && height_cm > 0 {
        out.push((
            "Screen Size".into(),
            json!(format!("{} cm x {} cm", width_cm, height_cm)),
        ));
    }

    let gamma = edid[23];
    if gamma > 0 && gamma != 0xFF {
        out.push((
            "Gamma".into(),
            json!(format!("{:.2}", (gamma as f32 + 100.0) / 100.0)),
        ));
    }

    let features = edid[24];
    let mut dpms = Vec::new();
    if features & 0x80 != 0 {
        dpms.push("Standby");
    }
    if features & 0x40 != 0 {
        dpms.push("Suspend");
    }
    if features & 0x20 != 0 {
        dpms.push("ActiveOff");
    }
    if !dpms.is_empty() {
        out.push(("DPMS Features".into(), json!(dpms.join(", "))));
    }

    let color_type = if digital {
        match (features >> 3) & 0x03 {
            0 => "RGB 4:4:4",
            1 => "RGB 4:4:4 & YCrCb 4:4:4",
            2 => "RGB 4:4:4 & YCrCb 4:2:2",
            _ => "RGB 4:4:4 & YCrCb 4:4:4 & 4:2:2",
        }
    } else {
        match (features >> 3) & 0x03 {
            0 => "Monochrome",
            1 => "RGB",
            2 => "Non-RGB",
            _ => "Undefined",
        }
    };
    out.push(("Color Format".into(), json!(color_type)));

    let mut name = String::new();
    let mut serial_str = String::new();
    let mut range_limits = String::new();

    for i in 0..4 {
        let offset = 54 + i * 18;
        if offset + 18 > edid.len() {
            continue;
        }
        let block = &edid[offset..offset + 18];
        if block[0] != 0 || block[1] != 0 || block[2] != 0 {
            if i == 0 {
                let pixel_clock = u16::from_le_bytes([block[0], block[1]]);
                if pixel_clock > 0 {
                    let hactive = block[2] as u16 | (((block[4] >> 4) as u16) << 8);
                    let vactive = block[5] as u16 | (((block[7] >> 4) as u16) << 8);
                    out.push((
                        "Native Res".into(),
                        json!(format!("{}x{}", hactive, vactive)),
                    ));
                }
            }
        } else {
            let tag = block[3];
            if tag == 0xFC || tag == 0xFF {
                let mut text = String::new();
                for &b in &block[5..18] {
                    if b == 0x0A {
                        break;
                    }
                    if b.is_ascii_graphic() || b == b' ' {
                        text.push(b as char);
                    }
                }
                let text = text.trim().to_string();
                if tag == 0xFC {
                    name = text;
                } else if tag == 0xFF {
                    serial_str = text;
                }
            } else if tag == 0xFD {
                let v_min = block[5];
                let v_max = block[6];
                let h_min = block[7];
                let h_max = block[8];
                let max_clock = (block[9] as u16) * 10;
                range_limits = format!(
                    "{}~{} Hz (V) | {}~{} kHz (H) | Max {} MHz",
                    v_min, v_max, h_min, h_max, max_clock
                );
            }
        }
    }

    if !name.is_empty() {
        out.push(("Model Name".into(), json!(name)));
    }
    if !serial_str.is_empty() {
        out.push(("Serial Number".into(), json!(serial_str)));
    }
    if !range_limits.is_empty() {
        out.push(("Range Limits".into(), json!(range_limits)));
    }

    out
}

fn parse_api_restriction_state(raw: &str) -> CliResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "restricted" => Ok(true),
        "open" => Ok(false),
        other => Err(CliError::new(format!(
            "invalid API restriction state {other:?}; expected open or restricted"
        ))),
    }
}

fn parse_display_id(raw: &str) -> CliResult<u32> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    u32::from_str_radix(digits, 16)
        .map_err(|_| CliError::new(format!("invalid display ID {raw:?}; expected hex")))
}

fn parse_edid_hex(raw: &str) -> CliResult<Vec<u8>> {
    let hex = raw.trim();
    if !hex.len().is_multiple_of(2) {
        return Err(CliError::new(
            "EDID hex must contain an even number of digits",
        ));
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| CliError::new(format!("invalid EDID hex byte at offset {index}")))
        })
        .collect()
}

fn option_one<'a>(invocation: &'a Invocation, name: &str) -> Option<&'a str> {
    invocation
        .options
        .get(name)
        .and_then(|values| values.last())
        .map(String::as_str)
}

/// Build the P0 voltage-bounds JSON block shared by get-status and
/// get-volt-rails. Returns `None` if the driver exposes no type-1 status
/// entry (p0_bounds() plausibility check failed).
#[allow(non_snake_case)]
fn volt_rails_p0_json(rails: &nvoc_core::VoltRails) -> Option<Value> {
    let b = rails.p0_bounds()?;
    // ceiling = min(vbios_wall, vrm_max_wall) − base wall; the µV still
    // available before the driver clamps the effective wall.
    let ceiling_uV = rails
        .offset_ceiling_uV(0)
        .or_else(|| {
            let mut c = b.vrm_max_wall_uV;
            if b.vbios_wall_uV > 0 && b.vbios_wall_uV < c {
                c = b.vbios_wall_uV;
            }
            Some((c - b.effective_wall_uV).max(0))
        })
        .unwrap_or(0);
    Some(json!({
        "current_uV": b.current_uV,
        "target_wall_uV": b.target_wall_uV,
        "effective_wall_uV": b.effective_wall_uV,
        "vbios_wall_uV": b.vbios_wall_uV,
        "vrm_max_wall_uV": b.vrm_max_wall_uV,
        "min_hold_uV": b.min_hold_uV,
        "offset_ceiling_uV": ceiling_uV,
    }))
}

fn option_bool(invocation: &Invocation, name: &str, default: bool) -> CliResult<bool> {
    option_one(invocation, name).map_or(Ok(default), parse_bool)
}

fn option_domain(invocation: &Invocation, default: ClockDomain) -> CliResult<ClockDomain> {
    option_one(invocation, "domain").map_or(Ok(default), parse_domain)
}

fn option_pstate_nvapi(invocation: &Invocation) -> CliResult<PState> {
    option_one(invocation, "pstate").map_or(Ok(PState::P0), parse_pstate_nvapi)
}

fn option_pstate_nvml(
    invocation: &Invocation,
) -> CliResult<nvml_wrapper::enum_wrappers::device::PerformanceState> {
    option_one(invocation, "pstate")
        .map_or_else(|| parse_nvml_pstate("P0"), parse_nvml_pstate)
        .map_err(CliError::from)
}

fn option_vfp_reset_domain(invocation: &Invocation) -> CliResult<VfpResetDomain> {
    match option_one(invocation, "domain")
        .unwrap_or("all")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "all" => Ok(VfpResetDomain::All),
        "core" | "graphics" | "gpu" => Ok(VfpResetDomain::Core),
        "mem" | "memory" => Ok(VfpResetDomain::Memory),
        other => Err(CliError::new(format!(
            "invalid VFP reset domain {other:?}; expected all, core, or memory"
        ))),
    }
}

fn parse_bool(raw: &str) -> CliResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(CliError::new(format!(
            "invalid bool {other:?}; expected on/off"
        ))),
    }
}

fn parse_domain(raw: &str) -> CliResult<ClockDomain> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "core" | "gpu" | "gpc" | "graphics" => Ok(ClockDomain::Graphics),
        "mem" | "memory" => Ok(ClockDomain::Memory),
        "processor" | "sm" => Ok(ClockDomain::Processor),
        "video" => Ok(ClockDomain::Video),
        other => Err(CliError::new(format!(
            "invalid domain {other:?}; expected core, memory, processor, or video"
        ))),
    }
}

/// Parse a clock-domain name or numeric bit for the private ClockClient
/// family. Unlike `parse_domain` (which maps to the 4-value public
/// `ClockDomain`), this returns the raw domain bit used by GET_CONTROL /
/// MEASURE_FREQ — XBAR (1) is not representable in the public enum. Accepts
/// names (xbar/gpc/sys/mclk) or a bare integer bit.
/// Canonical domain name for a raw domain bit (reverse of
/// [`parse_clk_domain`]'s alias table; "bit N" when unmapped).
fn parse_clk_domain_name(bit: u32) -> String {
    match bit {
        0 => "Gpc".into(),
        1 => "Xbar".into(),
        2 => "Sys".into(),
        3 => "Hub".into(),
        4 => "M".into(),
        5 => "Host".into(),
        6 => "Disp".into(),
        7 => "Hotclk".into(),
        8 => "Pclk0".into(),
        9 => "Pclk1".into(),
        10 => "Bypclk".into(),
        11 => "Xclk".into(),
        12 => "Vpv".into(),
        13 => "Vps".into(),
        14 => "Gpucacheclk".into(),
        15 => "Gpc2".into(),
        16 => "Xbar2".into(),
        17 => "Sys2".into(),
        18 => "Hub2".into(),
        19 => "Leg".into(),
        20 => "Pwr".into(),
        21 => "Msd".into(),
        22 => "Utils".into(),
        23 => "ColdNv".into(),
        24 => "ColdHotclk".into(),
        25 => "Ltc2".into(),
        28 => "Host1x".into(),
        _ => format!("bit {bit}"),
    }
}

fn parse_clk_domain(raw: &str) -> CliResult<u32> {
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "gpc" | "core" | "gpu" | "graphics" | "nv" => Ok(0),
        "xbar" | "xbarclk" => Ok(1),
        "sys" => Ok(2),
        "hub" => Ok(3),
        "mclk" | "mem" | "memory" | "m" => Ok(4),
        "host" => Ok(5),
        "disp" | "display" => Ok(6),
        "hotclk" => Ok(7),
        "pclk0" => Ok(8),
        "pclk1" => Ok(9),
        "bypclk" => Ok(10),
        "xclk" => Ok(11),
        "vpv" => Ok(12),
        "vps" => Ok(13),
        "gpucacheclk" | "cache" => Ok(14),
        "gpc2" => Ok(15),
        "xbar2" | "xbar2clk" => Ok(16),
        "sys2" => Ok(17),
        "hub2" => Ok(18),
        "leg" => Ok(19),
        "pwr" => Ok(20),
        "msd" => Ok(21),
        "utils" => Ok(22),
        "coldnv" => Ok(23),
        "coldhotclk" => Ok(24),
        "ltc2" => Ok(25),
        "host1x" => Ok(28),
        _ => trimmed.parse::<u32>().map_err(|_| {
            CliError::new(format!(
                "invalid clock domain {raw:?}: use a domain name (gpc/xbar/sys/hub/mclk/host/disp/... ) or a raw domain bit (0-31)"
            ))
        }),
    }
}

fn parse_pstate_nvapi(raw: &str) -> CliResult<PState> {
    let normalized = raw.trim().to_ascii_uppercase();
    <PState as ConvertEnum>::from_str(&normalized).map_err(CliError::from)
}

fn parse_nvapi_cooler_policy(raw: &str) -> CliResult<CoolerPolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" | "continuous" => Ok(CoolerPolicy::TemperatureContinuous),
        "manual" => Ok(CoolerPolicy::Manual),
        other => <CoolerPolicy as ConvertEnum>::from_str(other).map_err(CliError::from),
    }
}

fn parse_cooler_target(raw: &str) -> CliResult<CoolerTarget> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(CoolerTarget::All),
        "0" | "1" => Ok(CoolerTarget::Cooler1),
        "2" => Ok(CoolerTarget::Cooler2),
        other => Err(CliError::new(format!(
            "invalid fan target {other:?}; expected all, 1, or 2"
        ))),
    }
}

fn parse_usize(raw: &str, label: &str) -> CliResult<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| CliError::new(format!("invalid {label} {raw:?}")))
}

fn parse_u32(raw: &str, label: &str) -> CliResult<u32> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| CliError::new(format!("invalid {label} {raw:?}")))
}

/// Parse a fan-curve points positional (`40:800,60:1200,75:1800`) into
/// strictly-applied `FanCurvePointReadout`s. The driver requires strictly
/// increasing temperature AND RPM across the three points.
fn parse_fan_curve_points(raw: &str) -> CliResult<Vec<FanCurvePointReadout>> {
    raw.split(',')
        .map(|pair| {
            let (t, r) = pair.split_once(':').ok_or_else(|| {
                CliError::new(format!(
                    "invalid fan-curve point {pair:?}: expected temp:rpm"
                ))
            })?;
            Ok(FanCurvePointReadout {
                temp_c: t
                    .trim()
                    .parse()
                    .map_err(|_| CliError::new(format!("invalid fan-curve temperature {t:?}")))?,
                rpm: r
                    .trim()
                    .parse()
                    .map_err(|_| CliError::new(format!("invalid fan-curve rpm {r:?}")))?,
            })
        })
        .collect()
}

fn parse_i32_unit(raw: &str, suffix: &str, label: &str) -> CliResult<i32> {
    strip_unit(raw, suffix, label)
        .parse::<i32>()
        .map_err(|_| CliError::new(format!("invalid {label} value {raw:?}")))
}

fn parse_u32_unit(raw: &str, suffix: &str, label: &str) -> CliResult<u32> {
    strip_unit(raw, suffix, label)
        .parse::<u32>()
        .map_err(|_| CliError::new(format!("invalid {label} value {raw:?}")))
}

/// Parse a Celsius positional (`85`, `85C`, `85c`) into f32 for the
/// target-temperature threshold setter (Q8 fixed-point accepts sub-degree).
fn parse_celsius_f32(raw: &str) -> CliResult<f32> {
    strip_unit(raw, "c", "celsius")
        .parse::<f32>()
        .map_err(|_| CliError::new(format!("invalid celsius value {raw:?}")))
}

/// Parse a millivolt positional (`1082.5`, `1082.5mV`, `1082.5millivolts`)
/// into f64. Volt-rail targets may carry one decimal on 10/20-series GPUs
/// whose hardware step is 12.5 mV; the value is floored to µV downstream.
fn parse_mv_f64(raw: &str) -> CliResult<f64> {
    strip_unit(raw, "mv", "millivolt")
        .parse::<f64>()
        .map_err(|_| CliError::new(format!("invalid millivolt value {raw:?}")))
}

fn strip_unit<'a>(raw: &'a str, suffix: &str, label: &str) -> &'a str {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    for candidate in [suffix, label] {
        if let Some(without) = lower.strip_suffix(candidate) {
            return trimmed[..without.len()].trim();
        }
    }
    let plural_label = format!("{label}s");
    if let Some(without) = lower.strip_suffix(&plural_label) {
        return trimmed[..without.len()].trim();
    }
    trimmed
}

fn mhz_to_khz_i32(mhz: i32) -> CliResult<i32> {
    mhz.checked_mul(1000)
        .ok_or_else(|| CliError::new("MHz value is too large"))
}

fn mhz_to_khz_u32(mhz: u32) -> CliResult<u32> {
    mhz.checked_mul(1000)
        .ok_or_else(|| CliError::new("MHz value is too large"))
}

fn domain_label(domain: ClockDomain) -> &'static str {
    match domain {
        ClockDomain::Graphics => "graphics",
        ClockDomain::Memory => "memory",
        ClockDomain::Processor => "processor",
        ClockDomain::Video => "video",
        _ => "unknown",
    }
}

fn pstate_label(pstate: PState) -> &'static str {
    <PState as ConvertEnum>::to_str(&pstate)
}

/// Format an NVML violation-status `reference_time` (a Unix epoch microsecond
/// stamp marking when the driver's cumulative counters started) as a UTC
/// wall-clock string. Returns `None` when the stamp is missing/zero.
fn format_reference_time(reference_time_us: u64) -> Option<String> {
    if reference_time_us == 0 {
        return None;
    }
    let nanos = reference_time_us as i128 * 1000;
    let dt = OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
    dt.format(&format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second] UTC"
    ))
    .ok()
}

fn policy_label(policy: CoolerPolicy) -> &'static str {
    <CoolerPolicy as ConvertEnum>::to_str(&policy)
}

fn vfp_reset_domain_label(domain: VfpResetDomain) -> &'static str {
    match domain {
        VfpResetDomain::All => "all",
        VfpResetDomain::Core => "core",
        VfpResetDomain::Memory => "memory",
    }
}

fn summarize_errors(execution: &Execution) -> String {
    execution
        .results
        .iter()
        .filter_map(|result| result.error.as_ref())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_listed_in_lexicographic_order() {
        let names: Vec<&str> = COMMANDS.iter().map(|c| c.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "COMMANDS must stay sorted by CLI name");
    }

    #[test]
    fn parses_named_args_before_and_after_function() {
        let invocation = parse_args([
            "--domain",
            "memory",
            "get-public-vftable",
            "--gpu",
            "0",
            "--output=json",
            "--indexed",
        ])
        .unwrap();

        assert_eq!(invocation.command, Some(Command::GetPublicVftable));
        assert_eq!(invocation.output, OutputFormat::Json);
        assert_eq!(invocation.gpu_specs, vec!["0"]);
        assert_eq!(option_one(&invocation, "domain"), Some("memory"));
        assert!(option_bool(&invocation, "indexed", false).unwrap());
    }

    #[test]
    fn parses_negative_positional_after_function() {
        let invocation = parse_args(["set-pstate-global-freq-offset", "-100", "--nvml"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetPstateGlobalFreqOffset));
        assert_eq!(invocation.backend, BackendChoice::Nvml);
        assert_eq!(invocation.positionals, vec!["-100"]);
    }

    #[test]
    fn parses_command_specific_named_args_before_function() {
        let invocation = parse_args(["--fan", "1", "set-fan-speed", "65"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetFanSpeed));
        assert_eq!(invocation.positionals, vec!["65"]);
        assert_eq!(option_one(&invocation, "fan"), Some("1"));
    }

    #[test]
    fn parses_new_getter_commands() {
        let invocation = parse_args(["get-display-list", "--all"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetDisplayList));
        assert!(option_bool(&invocation, "all", false).unwrap());

        let invocation = parse_args(["get-legacy-gpc-rail-overvolt-limit", "--pstate", "P2"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetLegacyGpcRailOvervoltLimit));
        assert_eq!(option_one(&invocation, "pstate"), Some("P2"));

        let invocation = parse_args(["get-autoboost-support", "auto-boost"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetAutoboostSupport));
        assert_eq!(invocation.positionals, vec!["auto-boost"]);

        let invocation = parse_args(["get-edid", "0x00010001"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetEdid));
        assert_eq!(invocation.positionals, vec!["0x00010001"]);

        let invocation = parse_args(["set-edid", "0x00010001", "00FFFFFF"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetEdid));
        assert_eq!(invocation.positionals, vec!["0x00010001", "00FFFFFF"]);

        let invocation = parse_args(["clear-edid", "0x00010001"]).unwrap();
        assert_eq!(invocation.command, Some(Command::ClearEdid));
        assert_eq!(invocation.positionals, vec!["0x00010001"]);
    }

    #[test]
    fn command_help_names_positionals_and_lists_finite_values() {
        let help = parse_args(["get-autoboost-support", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("<API>"));
        assert!(help.contains("[possible values: app-clocks, auto-boost]"));
        assert!(!help.contains("[ARGS]"));

        let help = parse_args(["set-autoboost-support", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("<API> <STATE>"));
        assert!(help.contains("[possible values: app-clocks, auto-boost]"));
        assert!(help.contains("[possible values: open, restricted]"));

        let help = parse_args(["set-autoboost-status", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("<ENABLED>"));
        assert!(help.contains("[possible values: on, off]"));
    }

    #[test]
    fn finite_positionals_keep_existing_aliases() {
        let invocation =
            parse_args(["set-autoboost-support", "application-clocks", "restricted"]).unwrap();
        assert_eq!(
            invocation.positionals,
            vec!["application-clocks", "restricted"]
        );

        let invocation = parse_args(["set-autoboost-status", "yes"]).unwrap();
        assert_eq!(invocation.positionals, vec!["yes"]);

        let invocation = parse_args(["set-ppab-status", "on"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetPpabStatus));
        assert_eq!(invocation.positionals, vec!["on"]);

        let invocation = parse_args(["set-pstate-lock-via-mem-range", "0", "p2"]).unwrap();
        assert_eq!(invocation.positionals, vec!["0", "p2"]);
    }

    #[test]
    fn rejects_backend_conflict() {
        let err = parse_args(["--nvapi", "--nvml", "get-gpu-list"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("conflicts"));
    }

    #[test]
    fn temperature_thresholds_supports_both_backends() {
        // get-temp-thresholds advertises BOTH backends so `--nvapi`
        // (target-temp policy indices) is accepted, but auto prefers NVML
        // (the classic Shutdown/Slowdown/... table).
        assert!(
            Command::GetTemperatureThresholds
                .adapters()
                .contains(&BackendAdapter::Nvapi)
        );
        assert!(
            Command::GetTemperatureThresholds
                .adapters()
                .contains(&BackendAdapter::Nvml)
        );
        assert_eq!(
            Command::GetTemperatureThresholds.auto_preferred_backend(),
            BackendAdapter::Nvml
        );
        // Explicit --nvapi parses and routes to the nvapi backend.
        let invocation = parse_args(["get-temp-thresholds", "--nvapi"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetTemperatureThresholds));
        assert_eq!(invocation.backend, BackendChoice::Nvapi);
        // No flag parses to auto (which routes to NVML via auto_preferred_backend).
        let invocation = parse_args(["get-temp-thresholds"]).unwrap();
        assert_eq!(invocation.backend, BackendChoice::Auto);
    }

    #[test]
    fn rejects_option_not_valid_for_command() {
        let err = parse_args(["get-power-limit", "--domain", "memory"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--domain"));
    }

    #[test]
    fn command_help_only_lists_supported_named_args() {
        let get_info_help = parse_args(["get-info", "--help"]).unwrap_err().to_string();
        assert!(!get_info_help.contains("--fan"));
        assert!(!get_info_help.contains("--domain"));

        let set_fan_help = parse_args(["set-fan-speed", "--help"])
            .unwrap_err()
            .to_string();
        assert!(set_fan_help.contains("--fan"));
        assert!(set_fan_help.contains("--policy"));
        assert!(!set_fan_help.contains("--domain"));

        let reset_fan_help = parse_args(["reset-fan-speed", "--help"]).unwrap_err().to_string();
        assert!(reset_fan_help.contains("--fan"));
        assert!(!reset_fan_help.contains("--policy"));
    }

    #[test]
    fn rejects_command_specific_named_args_on_other_commands() {
        let err = parse_args(["--fan", "1", "get-info"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--fan"));

        let err = parse_args(["--all", "get-info"]).unwrap_err().to_string();
        assert!(err.contains("--all"));
    }

    #[test]
    fn reset_fan_rejects_ignored_policy_and_nvapi_specific_fan() {
        let err = parse_args(["reset-fan-speed", "--policy", "manual"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--policy"));

        let err = parse_args(["--fan", "1", "reset-fan-speed"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires --nvml"));

        let invocation = parse_args(["--nvml", "--fan", "1", "reset-fan-speed"]).unwrap();
        assert_eq!(invocation.backend, BackendChoice::Nvml);
        assert_eq!(option_one(&invocation, "fan"), Some("1"));
    }

    #[test]
    fn parses_units() {
        assert_eq!(parse_i32_unit("-125MHz", "mhz", "mhz").unwrap(), -125);
        assert_eq!(parse_u32_unit("350W", "w", "watt").unwrap(), 350);
        assert_eq!(parse_u32_unit("350watts", "w", "watt").unwrap(), 350);
        assert_eq!(parse_u32_unit("90%", "%", "percent").unwrap(), 90);
        assert_eq!(parse_u32_unit("90percent", "%", "percent").unwrap(), 90);
        assert_eq!(parse_i32_unit("83celsius", "c", "celsius").unwrap(), 83);
        assert_eq!(mhz_to_khz_i32(150).unwrap(), 150_000);
        assert_eq!(mhz_to_khz_u32(150).unwrap(), 150_000);
        assert!(mhz_to_khz_u32(u32::MAX).is_err());
        assert_eq!(bytes_to_upper_hex(&[0x00, 0xab, 0xff]), "00ABFF");
        assert_eq!(parse_display_id("0x00010001").unwrap(), 0x00010001);
        assert_eq!(parse_display_id("00010001").unwrap(), 0x00010001);
        assert_eq!(parse_edid_hex("00abFF").unwrap(), vec![0x00, 0xab, 0xff]);
        assert!(parse_display_id("display-1").is_err());
        assert!(parse_edid_hex("ABC").is_err());
        assert!(parse_edid_hex("00GG").is_err());
    }

    #[test]
    fn parses_volt_rail_target_mv_as_f64() {
        // One decimal mV is allowed (2.5 mV rail step on 10/20-series).
        assert_eq!(parse_mv_f64("1082.5").unwrap(), 1082.5);
        assert_eq!(parse_mv_f64("1085").unwrap(), 1085.0);
        // Unit suffixes are accepted (mirrors parse_*_unit).
        assert_eq!(parse_mv_f64("1082.5mV").unwrap(), 1082.5);
        assert_eq!(parse_mv_f64("1082.5millivolts").unwrap(), 1082.5);
        assert!(parse_mv_f64("abc").is_err());
    }

    #[test]
    fn set_volt_rail_target_floors_decimal_mv_to_uv() {
        // The command path converts the f64 mV to i32 µV (rounds to nearest µV).
        // Exercise the same parse+convert the dispatcher runs, without needing
        // a live GPU.
        let mv = parse_mv_f64("1082.5").unwrap();
        let uv = i32::try_from((mv * 1000.0).round() as i64).unwrap();
        assert_eq!(uv, 1_082_500);
        let mv = parse_mv_f64("1085").unwrap();
        let uv = i32::try_from((mv * 1000.0).round() as i64).unwrap();
        assert_eq!(uv, 1_085_000);
    }

    #[test]
    fn parse_edid_interprets_real_u2790b_block() {
        // Real EDID dumped via `nvoc-cli get-edid 0x80061086` (U2790B 4K monitor).
        let hex = "00FFFFFFFFFFFF0005E39027B91401001F1D0103803C22782A67A1A5554DA2270E5054BFEF00D1C0B30095008180814081C0010101014DD000A0F0703E803020350055502100001AA36600A0F0701F803020350055502100001A000000FC005532373930420A202020202020000000FD0017501EA03C000A20202020202001DC020333F14C9004031F1301125D5E5F606123090707830100006D030C001000387820006001020367D85DC401788003E30F000C565E00A0A0A029503020350055502100001E023A801871382D40582C450055502100001E011D007251D01E206E28550055502100001E4D6C80A070703E8030203A0055502100001A000000004E";
        let edid = parse_edid_hex(hex).unwrap();
        let fields = parse_edid(&edid);

        let lookup = |key: &str| -> String {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str().unwrap_or("").to_string())
                .unwrap_or_default()
        };

        assert_eq!(lookup("Manufacturer"), "AOC");
        assert_eq!(lookup("Model Name"), "U2790B");
        assert_eq!(lookup("Input Signal"), "Digital");
        assert_eq!(
            lookup("Range Limits"),
            "23~80 Hz (V) | 30~160 kHz (H) | Max 600 MHz"
        );
        // Header is parsed (non-empty), and invalid input yields nothing.
        assert!(!fields.is_empty());
        assert!(parse_edid(&[0u8; 64]).is_empty());
        assert!(parse_edid(&[0xFFu8; 128]).is_empty());
    }

    #[test]
    fn parses_domain_aliases() {
        assert_eq!(parse_domain("core").unwrap(), ClockDomain::Graphics);
        assert_eq!(parse_domain("mem").unwrap(), ClockDomain::Memory);
    }

    #[test]
    fn command_backend_support_is_explicit() {
        assert!(
            Command::SetTempLimit
                .adapters()
                .contains(&BackendAdapter::Nvapi)
        );
        assert!(
            Command::SetTempLimit
                .adapters()
                .contains(&BackendAdapter::Nvml)
        );
        assert_eq!(Command::SetPowerLimit.adapters(), &BOTH_BACKENDS);
        assert_eq!(Command::SetPublicTgpPercent.adapters(), &NVAPI_ONLY);
        assert_eq!(Command::GetDisplayList.adapters(), &NVAPI_ONLY);
        assert_eq!(Command::GetAutoboostStatus.adapters(), &NVML_ONLY);
        assert_eq!(Command::GetAutoboostSupport.adapters(), &NVML_ONLY);
        assert_eq!(Command::GetEdid.adapters(), &NVAPI_ONLY);
        assert_eq!(Command::SetEdid.adapters(), &NVAPI_ONLY);
        assert_eq!(Command::ClearEdid.adapters(), &NVAPI_ONLY);
    }

    #[test]
    fn merged_fan_and_thermal_commands_parse() {
        // set-fan-speed --rpm (merged setter)
        let invocation = parse_args(["set-fan-speed", "--rpm", "1200", "--cooler", "0"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetFanSpeed));
        assert_eq!(invocation.positionals, vec!["1200"]);
        assert!(option_bool(&invocation, "rpm", false).unwrap());
        assert_eq!(option_one(&invocation, "cooler"), Some("0"));

        // set-whispermode2-status on --mode quiet (merged set-wm2 + set-wm2-mode)
        let invocation = parse_args(["set-whispermode2-status", "--mode", "quiet", "on"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetWhisperMode2Status));
        assert_eq!(invocation.positionals, vec!["on"]);
        assert_eq!(option_one(&invocation, "mode"), Some("quiet"));
        let invocation = parse_args(["set-whispermode2-status", "off"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetWhisperMode2Status));
        assert_eq!(option_one(&invocation, "mode"), None);

        // set-fanstop-status (renamed set-fan-stop; fanstop is a proper noun)
        let invocation = parse_args(["set-fanstop-status", "on"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetFanstopStatus));

        // set-ppab-status / set-batteryboost2-status / set-private-target-temp-limit
        let invocation = parse_args(["set-ppab-status", "on"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetPpabStatus));
        let invocation = parse_args(["set-batteryboost2-status", "1"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetBatteryBoost2Status));
        let invocation = parse_args(["set-private-target-temp-limit", "80"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetPrivateTargetTempLimit));

        // reset-fan-speed --rpm (merged reset; --cooler picks one cooler)
        let invocation = parse_args(["reset-fan-speed", "--rpm", "--nvapi", "--cooler", "1"])
            .unwrap();
        assert_eq!(invocation.command, Some(Command::ResetFanSpeed));
        assert_eq!(invocation.backend, BackendChoice::Nvapi);
        assert!(option_bool(&invocation, "rpm", false).unwrap());
        assert_eq!(option_one(&invocation, "cooler"), Some("1"));

        // default reset-fan-speed keeps the plain reset behavior
        let invocation = parse_args(["reset-fan-speed"]).unwrap();
        assert_eq!(invocation.command, Some(Command::ResetFanSpeed));
        assert!(!option_bool(&invocation, "rpm", false).unwrap());

        // get-power-limit (merged NVML + NVAPI-range fallback)
        let invocation = parse_args(["get-power-limit"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetPowerLimit));
        assert_eq!(invocation.backend, BackendChoice::Auto);
        assert_eq!(Command::GetPowerLimit.adapters(), &BOTH_BACKENDS);
        assert_eq!(
            Command::GetPowerLimit.auto_preferred_backend(),
            BackendAdapter::Nvml
        );

        // get-public-power-limit / get-public-temp-limit (split of the old
        // get-tdp-temp-limits)
        let invocation = parse_args(["get-public-power-limit"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetPublicPowerLimit));
        assert_eq!(Command::GetPublicPowerLimit.adapters(), &NVAPI_ONLY);
        let invocation = parse_args(["get-public-temp-limit"]).unwrap();
        assert_eq!(invocation.command, Some(Command::GetPublicTempLimit));
        assert_eq!(Command::GetPublicTempLimit.adapters(), &NVAPI_ONLY);

        // set-temp-limit --domain acoustic (folded set-acoustic-temp-c)
        let invocation = parse_args(["set-temp-limit", "--domain", "acoustic", "80", "--nvml"])
            .unwrap();
        assert_eq!(invocation.command, Some(Command::SetTempLimit));
        assert_eq!(invocation.positionals, vec!["80"]);
        assert_eq!(option_one(&invocation, "domain"), Some("acoustic"));
        // plain set-temp-limit still parses (default gpu domain)
        let invocation = parse_args(["set-temp-limit", "83"]).unwrap();
        assert_eq!(invocation.positionals, vec!["83"]);
        assert_eq!(option_one(&invocation, "domain"), None);
    }

    #[test]
    fn domain_filtered_resets_and_legacy_freq_parse() {
        // reset-private-vftable-offset --domain gpc
        let invocation = parse_args(["reset-private-vftable-offset", "--domain", "gpc", "0"])
            .unwrap();
        assert_eq!(
            invocation.command,
            Some(Command::ResetPrivateVftableOffset)
        );
        assert_eq!(invocation.positionals, vec!["0"]);
        assert_eq!(option_one(&invocation, "domain"), Some("gpc"));

        // reset-pstate-global-freq-offset --domain mem
        let invocation =
            parse_args(["reset-pstate-global-freq-offset", "--domain", "mem"]).unwrap();
        assert_eq!(
            invocation.command,
            Some(Command::ResetPstateGlobalFreqOffset)
        );
        assert_eq!(option_one(&invocation, "domain"), Some("mem"));

        // set-legacy-freq single value + --domain core/mem
        let invocation = parse_args(["set-legacy-freq", "--domain", "mem", "4001"]).unwrap();
        assert_eq!(invocation.command, Some(Command::SetLegacyFreq));
        assert_eq!(invocation.positionals, vec!["4001"]);
        assert_eq!(option_one(&invocation, "domain"), Some("mem"));
        // core is the default domain
        let invocation = parse_args(["set-legacy-freq", "900"]).unwrap();
        assert_eq!(invocation.positionals, vec!["900"]);
        assert_eq!(option_one(&invocation, "domain"), None);
    }

    #[test]
    fn finds_auto_targets_not_covered_by_primary_backend() {
        let execution = Execution {
            function: "get-pstate-global-freq-offset",
            backend: "nvapi".to_string(),
            warnings: Vec::new(),
            results: vec![
                TargetResult {
                    gpu_id: Some(256),
                    backend: "nvapi",
                    ok: true,
                    output: None,
                    error: None,
                },
                TargetResult {
                    gpu_id: Some(768),
                    backend: "nvapi",
                    ok: true,
                    output: None,
                    error: None,
                },
            ],
        };

        assert_eq!(
            uncovered_target_ids(&[256, 512, 768], &execution),
            vec![512]
        );
    }
}
