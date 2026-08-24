use nvapi_hi::{
    Celsius, ClockDomain, CoolerPolicy, KilohertzDelta, MicrovoltsDelta, PState, Percentage,
};
use nvml_wrapper::enum_wrappers::device::{Api, PerformanceState};
use nvoc_core::{
    BackendSet, CheckVoltageFrequency, ClearEdid, ConvertEnum, GpuTarget, GpuType,
    NvapiPerfFreqCap, QueryApiRestriction, QueryAutoBoost, QueryDisplays, QueryDomainVfpPoints,
    QueryEdid, QueryFanInfo, QueryGpuInfo, QueryGpuSettings, QueryGpuStatus,
    QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta, QueryNvapiClkDomainFreq,
    QueryNvapiClkDomainFreqsBatch, QueryNvapiClkDomains, QueryNvapiClkVfPoints,
    QueryNvapiDNotifier, QueryNvapiTargetTempPolicies, QueryNvapiTgpWattRange, QueryNvapiVoltRails,
    QueryPowerLimits, QueryPstateBaseVoltage, QueryPstates, QuerySupportedApplicationsClocks,
    QueryTdpTempLimits, QueryTemperatureThresholds, QueryThrottleReasons, QueryVfpPointVoltage,
    QueryVoltageBoost, ResetApplicationsClocks, ResetCoolerLevels, ResetFanCurve, ResetFanSpeed,
    SetFanStop, SetFanRpm, QueryNvapiCoolerInfo,
    ResetLockedClocks, ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetNvapiTgpWatt,
    ResetPstateBaseVoltages, ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock,
    ResetVfpLock, SetApiRestriction, SetApplicationsClocks, SetAutoBoost, SetAutoBoostDefault,
    SetClockOffset, SetCoolerLevels, SetDomainVfpDeltas, SetEdid, SetFanSpeed, SetLegacyClocks,
    SetLockedClocks, SetNvapiClkDomainOffset, SetNvapiDNotifier, SetNvapiDynamicBoost,
    SetNvapiPerfFreqCap, SetNvapiPowerLimits, SetNvapiPstateLock, SetNvapiSensorLimits,
    SetNvapiTargetTemp, SetNvapiTgpWatt, SetNvapiVfpPointPrivate, SetNvapiVoltRailOffset,
    SetNvapiVoltRailTarget, SetNvmlPstateLock, SetPowerLimit, SetPstateBaseVoltage,
    SetPstateClockOffset, SetTemperatureLimit, SetVfpFrequencyLock, SetVfpPointDelta,
    SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, VfpResetDomain, detect_gpu_type,
    discover_targets, fetch_gpu_type, nvml_pstate_to_str, parse_nvml_fan_control_policy, run,
    try_parse_nvml_pstate,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use serde_json::{Map, Number, Value};
use std::sync::Arc;

type PyResultValue = PyResult<Value>;

fn to_py_err(err: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

fn invalid_value(err: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn parse_backends(raw: &str) -> PyResult<BackendSet> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "both" | "all" => Ok(BackendSet::Both),
        "nvapi" => Ok(BackendSet::Nvapi),
        "nvml" => Ok(BackendSet::Nvml),
        other => Err(invalid_value(format!(
            "invalid backend set {other:?}; expected 'both'/'all', 'nvapi', or 'nvml'"
        ))),
    }
}

fn parse_backend(raw: &str) -> PyResult<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "nvapi" => Ok("nvapi"),
        "nvml" => Ok("nvml"),
        "nvapi-cooler" => Ok("nvapi-cooler"),
        "nvml-cooler" => Ok("nvml-cooler"),
        other => Err(invalid_value(format!(
            "invalid backend {other:?}; expected 'nvapi', 'nvml', 'nvapi-cooler', or 'nvml-cooler'"
        ))),
    }
}

fn parse_domain(raw: &str) -> PyResult<ClockDomain> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "core" | "gpu" | "graphics" => Ok(ClockDomain::Graphics),
        "mem" | "memory" => Ok(ClockDomain::Memory),
        other => Err(invalid_value(format!(
            "invalid clock domain {other:?}; expected 'graphics'/'core'/'gpu' or 'memory'/'mem'"
        ))),
    }
}

fn parse_pstate(raw: &str) -> PyResult<PState> {
    let normalized = raw.trim().to_ascii_uppercase();
    PState::from_str(normalized.as_str()).map_err(invalid_value)
}

fn parse_nvml_pstate(raw: &str) -> PyResult<PerformanceState> {
    try_parse_nvml_pstate(raw).map_err(invalid_value)
}

fn parse_api_restriction_api(raw: &str) -> PyResult<Api> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "app-clocks" | "application-clocks" => Ok(Api::ApplicationClocks),
        "auto-boost" | "autoboost" => Ok(Api::AutoBoostedClocks),
        other => Err(invalid_value(format!(
            "invalid API {other:?}; expected app-clocks, application-clocks, auto-boost, or autoboost"
        ))),
    }
}

fn api_restriction_api_label(api_type: Api) -> &'static str {
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

fn parse_display_id(raw: &str) -> PyResult<u32> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    u32::from_str_radix(digits, 16)
        .map_err(|_| invalid_value(format!("invalid display ID {raw:?}; expected hex")))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_edid_hex(raw: &str) -> PyResult<Vec<u8>> {
    let hex = raw.trim();
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(invalid_value(
            "EDID hex must contain an even number of digits",
        ));
    }

    bytes
        .chunks_exact(2)
        .enumerate()
        .map(|(pair_index, pair)| {
            let high_index = pair_index * 2;
            let high = hex_nibble(pair[0]).ok_or_else(|| {
                invalid_value(format!("invalid EDID hex digit at offset {high_index}"))
            })?;
            let low_index = high_index + 1;
            let low = hex_nibble(pair[1]).ok_or_else(|| {
                invalid_value(format!("invalid EDID hex digit at offset {low_index}"))
            })?;
            Ok((high << 4) | low)
        })
        .collect()
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

fn selected_target<'a>(
    inventory: &'a nvoc_core::TargetInventory,
    gpu: &str,
) -> PyResult<GpuTarget<'a>> {
    for target in inventory.targets() {
        if gpu_id_matches(target.id.0, gpu)? {
            return Ok(target);
        }
    }
    Err(to_py_err("no GPU selected"))
}

fn gpu_id_matches(gpu_id: u32, raw: &str) -> PyResult<bool> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(false);
    }
    if let Some(rest) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        let parsed = u32::from_str_radix(rest, 16).map_err(invalid_value)?;
        return Ok(parsed == gpu_id);
    }
    if let Ok(parsed) = raw.parse::<u32>() {
        return Ok(parsed == gpu_id || parsed < 256 && gpu_id == parsed.saturating_mul(256));
    }
    Ok(raw.eq_ignore_ascii_case(&format!("gpu {gpu_id}"))
        || raw.eq_ignore_ascii_case(&format!("gpu{gpu_id}"))
        || raw.eq_ignore_ascii_case(&format!("0x{gpu_id:X}")))
}

/// NVAPI/NVML 句柄是驱动侧不透明值；NVML 官方线程安全，NVAPI 查询同样
/// 被前端多线程并发调用，跨线程共享 inventory 安全。
struct SyncInventory(nvoc_core::TargetInventory);
unsafe impl Send for SyncInventory {}
unsafe impl Sync for SyncInventory {}

/// 进程级 inventory 缓存（按 BackendSet 各一份）。
///
/// GUI/TUI 以 0.5-1 Hz 轮询 pynvoc，若每次调用都 `discover_targets`，
/// 会反复 NVML init + NVAPI 重枚举：Linux 上 libnvidia-api 每次 NVAPI
/// 调用泄漏 fd（约 1 fd/tick，30-60 分钟耗尽 ulimit 1024 → "too many
/// open files"），Windows 上驱动/内核句柄累积一整天，退出时逐个拆除
/// 导致卡死数分钟。缓存后仅在首次使用或 `discover_gpus` 显式刷新时发现。
struct InventoryCache {
    both: Option<Arc<SyncInventory>>,
    nvapi: Option<Arc<SyncInventory>>,
    nvml: Option<Arc<SyncInventory>>,
}

static INVENTORY_CACHE: std::sync::Mutex<InventoryCache> = std::sync::Mutex::new(InventoryCache {
    both: None,
    nvapi: None,
    nvml: None,
});

fn lock_inventory_cache() -> std::sync::MutexGuard<'static, InventoryCache> {
    INVENTORY_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Process-level cache of the NVML-enforced power limit (TGP wall, watts),
/// keyed by GPU id. Populated by `normalize_info` (which runs at GPU-switch /
/// Refresh-Info time — a low-frequency, user-initiated path where the dGPU is
/// already D0 and the NVML device handle is valid). Consumed by
/// `normalize_status`'s 1Hz dashboard poll.
///
/// Why this split: `nvmlDeviceGetEnforcedPowerLimit` (like every NVML
/// device-level query) segfaults inside nvml.dll on a stale device handle when
/// the dGPU is mid-transition (D3cold↔D0) — verified crash at
/// `nvmlDeviceGetEnforcedPowerLimit+0x276` when the user re-enables a powered-
/// off dGPU. NVAPI recovers BEFORE NVML's handle state during that window, so
/// an earlier attempt to gate NVML with an NVAPI-derived liveness proof was
/// true while the NVML handle was still stale — it crashed on re-enable. There
/// is NO pre-call way to detect this (the `GpuLost` error code is never
/// returned — the fault is a native segfault). The only safe course is to
/// never call NVML device queries from the high-frequency poll.
/// The enforced limit is quasi-static (TGP wall, rarely changes), so caching it
/// from the info path loses no real-time accuracy that matters.
static POWER_LIMIT_CACHE: std::sync::Mutex<Option<(u32, std::time::Instant, f64)>> =
    std::sync::Mutex::new(None);

/// Cache TTL: the enforced power limit is the TGP wall, which only moves on a
/// deliberate user action (set-tgp-watt / D-Notifier change). 5 minutes is far
/// shorter than any real change cadence while bounding how stale a cached value
/// can get after a dGPU round-trip.
const POWER_LIMIT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

fn cache_power_limit(gpu_id: u32, watts: f64) {
    if let Ok(mut cache) = POWER_LIMIT_CACHE.lock() {
        *cache = Some((gpu_id, std::time::Instant::now(), watts));
    }
}

/// Return the cached enforced power limit for `gpu_id` if fresh, else `None`.
fn cached_power_limit(gpu_id: u32) -> Option<f64> {
    let cache = POWER_LIMIT_CACHE.lock().ok()?;
    let (cached_id, ts, watts) = cache.as_ref()?;
    if *cached_id != gpu_id {
        return None;
    }
    if ts.elapsed() > POWER_LIMIT_CACHE_TTL {
        return None;
    }
    Some(*watts)
}

impl InventoryCache {
    /// 获取指定 backend 集的 inventory；首次访问时发现并缓存。
    /// 返回 Arc 快照 —— 调用方在锁外持有并查询，并发查询互不阻塞。
    fn entry(&mut self, backends: BackendSet) -> PyResult<Arc<SyncInventory>> {
        let slot = match backends {
            BackendSet::Both => &mut self.both,
            BackendSet::Nvapi => &mut self.nvapi,
            BackendSet::Nvml => &mut self.nvml,
        };
        if slot.is_none() {
            *slot = Some(Arc::new(SyncInventory(
                discover_targets(backends).map_err(to_py_err)?,
            )));
        }
        Ok(slot.as_ref().expect("slot just filled").clone())
    }

    /// 强制重新发现指定集并更新缓存（discover_gpus 的显式刷新入口）。
    /// 刷新会替换槽位；仍在途的旧快照查询由 Arc 引用计数保活，安全。
    fn refresh(&mut self, backends: BackendSet) -> PyResult<Arc<SyncInventory>> {
        let fresh = discover_targets(backends).map_err(to_py_err)?;
        let slot = match backends {
            BackendSet::Both => &mut self.both,
            BackendSet::Nvapi => &mut self.nvapi,
            BackendSet::Nvml => &mut self.nvml,
        };
        *slot = Some(Arc::new(SyncInventory(fresh)));
        Ok(slot.as_ref().expect("slot just filled").clone())
    }
}

fn with_target<F>(gpu: &str, backends: &str, f: F) -> PyResultValue
where
    F: FnOnce(&GpuTarget<'_>) -> PyResultValue,
{
    let backends = parse_backends(backends)?;
    // Lock ONLY for the cache lookup (or first discovery); the query itself
    // runs against the Arc'd snapshot with no lock held — concurrent
    // NVAPI/NVML queries from multiple GUI/TUI worker threads no longer
    // serialize on this mutex (SyncInventory is driver-side thread-safe).
    let inventory = {
        let mut cache = lock_inventory_cache();
        cache.entry(backends)?
    };
    // Resolve the requested GPU against the cached inventory. If it isn't
    // listed (the dGPU was switched off / removed from the bus between polls),
    // refresh the inventory ONCE and retry against the freshly re-enumerated
    // set. `refresh` re-runs `discover_targets` (NVAPI re-enumerate +
    // `Nvml::init`), rebuilding the stale handle table so subsequent polls
    // either find the GPU again (it came back / was just GCOFF) or cleanly
    // omit it. Without this, a dGPU that disappears mid-session leaves
    // `selected_target` permanently erroring and the dashboard never recovers
    // even after the dGPU returns.
    match selected_target(&inventory.0, gpu) {
        Ok(target) => f(&target),
        Err(first_err) => {
            // Cache miss: re-enumerate once. The refreshed Arc<SyncInventory>
            // lives in this frame for the duration of the query (the borrow
            // into `target` is valid while `inventory` is alive). If the GPU is
            // still absent, surface the original error.
            let inventory = {
                let mut cache = lock_inventory_cache();
                cache.refresh(backends)?
            };
            match selected_target(&inventory.0, gpu) {
                Ok(target) => f(&target),
                Err(_) => Err(first_err),
            }
        }
    }
}

fn value_object(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        if !value.is_null() {
            map.insert(key.into(), value);
        }
    }
    Value::Object(map)
}

fn py_value<'py>(py: Python<'py>, value: &Value) -> PyResult<Py<PyAny>> {
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(v) => Ok(PyBool::new(py, *v).to_owned().into_any().unbind()),
        Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                Ok(PyInt::new(py, i).into_any().unbind())
            } else if let Some(u) = v.as_u64() {
                Ok(PyInt::new(py, u).into_any().unbind())
            } else if let Some(f) = v.as_f64() {
                Ok(PyFloat::new(py, f).into_any().unbind())
            } else {
                Ok(py.None())
            }
        }
        Value::String(v) => Ok(PyString::new(py, v).into_any().unbind()),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(py_value(py, item)?)?;
            }
            Ok(list.into_any().unbind())
        }
        Value::Object(items) => {
            let dict = PyDict::new(py);
            for (key, item) in items {
                dict.set_item(key, py_value(py, item)?)?;
            }
            Ok(dict.into_any().unbind())
        }
    }
}

fn text<T: std::fmt::Display>(value: T) -> Value {
    Value::String(value.to_string())
}

fn i64_value(value: i64) -> Value {
    Value::Number(Number::from(value))
}

fn u64_value(value: u64) -> Value {
    Value::Number(Number::from(value))
}

fn f64_value(value: f64) -> Value {
    Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn option_u32(value: Option<u32>) -> Value {
    value.map(|v| u64_value(v as u64)).unwrap_or(Value::Null)
}

fn khz_to_mhz_i64(value: i32) -> i64 {
    (value / 1000) as i64
}

fn uv_to_mv_i64(value: i32) -> i64 {
    (value / 1000) as i64
}

fn bool_value(value: bool) -> Value {
    Value::Bool(value)
}

/// NVAPI PerfFlags bit -> reason name. Bit semantics mirror nvapi-rs
/// (`sys/src/gpu/power.rs`, NV_GPU_PERF_FLAGS + its display table). Ascending
/// bit order so the decoded list reads consistently regardless of active set.
const PERF_LIMIT_BITS: &[(u32, &str)] = &[
    (1, "Power"),
    (2, "Temperature"),
    (4, "Reliability Voltage"),
    (8, "Operating Voltage"),
    (16, "No Load"),
    (32, "Unknown32"),
];

/// Decode a PerfFlags bitmask into reason names; an empty mask yields `["None"]`.
fn decode_perf_flags(bits: u32) -> Vec<&'static str> {
    let reasons: Vec<&'static str> = PERF_LIMIT_BITS
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if reasons.is_empty() {
        vec!["None"]
    } else {
        reasons
    }
}

/// Build a JSON array of strings from the given reason names.
fn text_array_value(items: Vec<&str>) -> Value {
    Value::Array(items.into_iter().map(text).collect())
}

fn percent_value(value: Percentage) -> Value {
    u64_value(value.0 as u64)
}

fn first_number_in_display<T: std::fmt::Display>(value: T) -> Option<f64> {
    let rendered = value.to_string();
    let mut token = String::new();
    let mut started = false;
    for ch in rendered.chars() {
        if ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.' {
            token.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if token.is_empty() || token == "-" || token == "+" || token == "." {
        None
    } else {
        token.parse().ok()
    }
}

fn target_nvml_device<'a>(target: &GpuTarget<'a>) -> PyResult<nvml_wrapper::Device<'a>> {
    let nvml = target.nvml().map_err(to_py_err)?;
    let count = nvml
        .device_count()
        .map_err(|err| to_py_err(format!("NVML device_count failed: {err:?}")))?;
    for index in 0..count {
        let device = nvml
            .device_by_index(index)
            .map_err(|err| to_py_err(format!("NVML device_by_index({index}) failed: {err:?}")))?;
        let id = nvoc_core::gpu_id_from_nvml_device(&device).map_err(to_py_err)?;
        if id.0 == target.id.0 {
            return Ok(device);
        }
    }
    Err(to_py_err(format!(
        "NVML device for GPU {} not found",
        target.id.0
    )))
}

fn normalize_info_nvml(target: &GpuTarget<'_>) -> PyResultValue {
    let device = target_nvml_device(target)?;
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));
    map.insert("vendor".into(), text("NVIDIA"));

    let name = device
        .name()
        .map_err(|err| to_py_err(format!("NVML name failed: {err:?}")))?;
    map.insert("name".into(), text(&name));
    map.insert("gpu_name".into(), text(name));

    if let Ok(pci) = device.pci_info() {
        map.insert(
            "bus".into(),
            text(format!(
                "{:04x}:{:02x}:{:02x}.0",
                pci.domain, pci.bus, pci.device
            )),
        );
    }
    if let Ok(uuid) = device.uuid() {
        map.insert("uuid".into(), text(uuid));
    }
    if let Ok(brand) = device.brand() {
        map.insert("brand".into(), text(format!("{brand:?}")));
    }

    Ok(Value::Object(map))
}

fn normalize_info(target: &GpuTarget<'_>) -> PyResultValue {
    let info = match run(target, QueryGpuInfo) {
        Ok(report) => report.output,
        Err(_) if !target.has_nvapi() && target.has_nvml() => return normalize_info_nvml(target),
        Err(error) => return Err(to_py_err(error)),
    };
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));
    map.insert("name".into(), text(&info.name));
    map.insert("gpu_name".into(), text(&info.name));
    map.insert("codename".into(), text(&info.codename));
    map.insert("arch".into(), text(info.arch));
    map.insert("gpu_architecture".into(), text(info.arch));
    map.insert("gpu_type".into(), text(info.gpu_type));
    // Generation series from core's gpu_type.rs detect_gpu_type (name +
    // codename) — the single source of truth. On Ada the ArchInfo enum has
    // no AD variant and `gpu_architecture` reads 'Unknown:400:7:161', so
    // capability flags must come from here, not the arch string.
    let series = fetch_gpu_type(&info).unwrap_or(GpuType::Unknown);
    map.insert("gpu_series".into(), text(series.to_string()));
    map.insert("is_mobile".into(), bool_value(series.is_mobile()));
    map.insert(
        "is_legacy_voltage".into(),
        bool_value(series.is_legacy_voltage()),
    );
    map.insert(
        "xbar_supported".into(),
        bool_value(series.supports_xbar_offset()),
    );
    map.insert("bios_version".into(), text(&info.bios_version));
    map.insert("bus".into(), text(info.bus));
    if let Some(vendor) = info.vendor() {
        map.insert("vendor".into(), text(vendor));
    }

    for (clock, limit) in &info.vfp_limits {
        let key_prefix = match *clock {
            ClockDomain::Graphics => "core_clock",
            ClockDomain::Memory => "mem_clock",
            _ => continue,
        };
        map.insert(format!("{key_prefix}_range"), text(limit.range));
        map.insert(
            format!("{key_prefix}_min"),
            i64_value(khz_to_mhz_i64(limit.range.min.0)),
        );
        map.insert(
            format!("{key_prefix}_max"),
            i64_value(khz_to_mhz_i64(limit.range.max.0)),
        );
    }

    if let Some(limit) = info.power_limits.first() {
        map.insert(
            "power_limit_min".into(),
            u64_value(limit.range.min.0 as u64),
        );
        map.insert(
            "power_limit_max".into(),
            u64_value(limit.range.max.0 as u64),
        );
        map.insert(
            "power_limit_default".into(),
            u64_value(limit.default.0 as u64),
        );
    }
    if let Ok(power) = run(target, QueryPowerLimits).map(|report| report.output) {
        map.insert(
            "power_limit_nvml_min_w".into(),
            f64_value(power.min_watts as f64),
        );
        map.insert(
            "power_limit_nvml_current_w".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert(
            "power_limit_nvml_max_w".into(),
            f64_value(power.max_watts as f64),
        );
        map.insert("power_watt_min".into(), f64_value(power.min_watts as f64));
        map.insert(
            "power_watt_current".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert("power_watt_max".into(), f64_value(power.max_watts as f64));
    }
    // Populate the process-level enforced-power-limit cache for the 1Hz
    // `normalize_status` poll. `nvmlDeviceGetEnforcedPowerLimit` (the live TGP
    // wall) is an NVML device-level query that segfaults during dGPU power-state
    // transitions, so it CANNOT run in the per-poll path. It runs here instead
    // — `normalize_info` executes at GPU-switch / Refresh-Info time, a
    // low-frequency user-initiated path where the dGPU is D0 and the NVML
    // handle is valid (the same path already runs `QueryPowerLimits`, another
    // NVML device query, without incident). On failure the cache is left
    // untouched (a stale but plausible value is better than nothing; the poll
    // path falls back to `---` only if the cache is empty/expired).
    if target.has_nvml()
        && let Ok(device) = target_nvml_device(target)
        && let Ok(mw) = device.enforced_power_limit()
    {
        cache_power_limit(target.id.0, mw as f64 / 1000.0);
    }
    if let Some(limit) = info.sensor_limits.first() {
        map.insert(
            "thermal_limit_min".into(),
            i64_value(limit.range.min.0 as i64),
        );
        map.insert(
            "thermal_limit_max".into(),
            i64_value(limit.range.max.0 as i64),
        );
        map.insert(
            "thermal_limit_default".into(),
            i64_value(limit.default.0 as i64),
        );
    }
    let overvolts = run(target, QueryLegacyCoreOvervoltRanges)
        .map(|report| report.output)
        .unwrap_or_default();
    if let Some((pstate, current, min, max)) = overvolts.first() {
        map.insert("legacy_overvolt_pstate".into(), text(pstate));
        map.insert(
            "legacy_overvolt_current_mv".into(),
            i64_value(uv_to_mv_i64(current.0)),
        );
        map.insert(
            "legacy_overvolt_min_mv".into(),
            i64_value(uv_to_mv_i64(min.0)),
        );
        map.insert(
            "legacy_overvolt_max_mv".into(),
            i64_value(uv_to_mv_i64(max.0)),
        );
    }
    Ok(Value::Object(map))
}

fn normalize_status(target: &GpuTarget<'_>) -> PyResultValue {
    let status = run(target, QueryGpuStatus).map_err(to_py_err)?.output;
    // NOTE on NVML: this function is the GUI/TUI 1Hz dashboard poll and must
    // NOT call any NVML device-level query (`power_usage`,
    // `enforced_power_limit`, `pcie_throughput`, ...). Those segfault inside
    // nvml.dll on a stale device handle during dGPU power-state transitions
    // (D3cold↔D0), and NVAPI recovers BEFORE NVML's handle state in that window
    // — so there is no pre-call liveness proof that makes NVML safe. The only
    // previously-live NVML reads were `power_usage`, `enforced_power_limit`, and
    // the PCIe block; all three are now either replaced by NVAPI
    // (`power_w` ← NVAPI PowerMonitor Board rail) or served from the
    // process-level `POWER_LIMIT_CACHE` (filled by the info path). NVAPI
    // returns clean error codes on a lost dGPU (no deref), so the NVAPI reads
    // below are all segfault-safe — they simply yield nothing and the dashboard
    // shows `---`.
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));
    map.insert("pstate".into(), text(status.pstate));
    if let Some(voltage) = status.voltage {
        map.insert("voltage_mv".into(), f64_value(voltage.0 as f64 / 1000.0));
    }
    for (clock, freq) in &status.clocks {
        match *clock {
            ClockDomain::Graphics => {
                map.insert("gpu_clock_mhz".into(), f64_value(freq.0 as f64 / 1000.0));
            }
            ClockDomain::Memory => {
                map.insert("mem_clock_mhz".into(), f64_value(freq.0 as f64 / 1000.0));
            }
            _ => {}
        }
    }
    // Effective (actually-running) clocks from GetAllClocks V2. Emitted as
    // parallel keys since the TUI native path reads this dict directly.
    if let Some(eff) = &status.effective_clocks {
        for (clock, freq) in eff {
            match *clock {
                ClockDomain::Graphics => {
                    map.insert(
                        "eff_gpu_clock_mhz".into(),
                        f64_value(freq.0 as f64 / 1000.0),
                    );
                }
                ClockDomain::Memory => {
                    map.insert(
                        "eff_mem_clock_mhz".into(),
                        f64_value(freq.0 as f64 / 1000.0),
                    );
                }
                _ => {}
            }
        }
    }
    // All 32 clock domains from GetAllClocks V2 (superset of effective_clocks):
    // includes the internal fabric clocks (Gpc, Xbar/crossbar, Sys, Hub, ...).
    // Emitted as a {domain_name: mhz} dict so the TUI/CLI can render the full
    // clock breakdown GPU-Z-style.
    //
    // MOBILE FALLBACK: on mobile GPUs the driver returns extended_domain[] all-
    // zero, so all_clocks is empty/None. In that case, supplement via the private
    // ClockClient MEASURE_FREQ batch (get-clk-domain-freq's backend) — it reads
    // every controllable domain's physical clock directly, covering the fabric
    // domains that GetAllClocks V2 omits on mobile.
    let all_clocks_mhz = if let Some(all) = &status.all_clocks
        && !all.is_empty()
    {
        let entries = all
            .iter()
            .map(|(domain, freq)| (domain.to_string(), f64_value(freq.0 as f64 / 1000.0)))
            .collect::<Vec<_>>();
        Some(value_object(
            entries.iter().map(|(k, v)| (k.as_str(), v.clone())),
        ))
    } else {
        // GetAllClocks V2 yielded no fabric clocks — try MEASURE_FREQ batch.
        // Query the controllable domain set first (for the domain bit list),
        // then batch-measure them.
        let domains = run(target, QueryNvapiClkDomains)
            .map(|report| report.output)
            .ok()
            .flatten()
            .map(|c| c.entries.iter().map(|e| e.bit).collect::<Vec<u32>>())
            .unwrap_or_default();
        if domains.is_empty() {
            None
        } else {
            let freqs = run(target, QueryNvapiClkDomainFreqsBatch { domains })
                .map(|report| report.output)
                .ok()
                .flatten();
            freqs.and_then(|fs| {
                if fs.is_empty() {
                    return None;
                }
                let entries = fs
                    .iter()
                    .map(|f| (f.domain.to_string(), f64_value(f.freq_mhz)))
                    .collect::<Vec<_>>();
                Some(value_object(
                    entries.iter().map(|(k, v)| (k.as_str(), v.clone())),
                ))
            })
        }
    };
    if let Some(v) = all_clocks_mhz {
        map.insert("all_clocks_mhz".into(), v);
    }
    if let Some((_sensor, temp)) = status.sensors.first() {
        map.insert("temperature_c".into(), f64_value(*temp as f64));
    }
    // Full thermal-sensor list as `[descriptor, temp_celsius]` pairs (same shape
    // as get-status JSON), plus the three primary typed temperatures pulled out
    // by channel_type (0=GPU_AVG/core, 1=GPU_MAX/hot spot, 3=MEMORY/VRAM). The
    // TUI dashboard reads temp_core/temp_hotspot/temp_memory directly (the
    // native path bypasses normalize_query_output, so it cannot re-parse the
    // sensors array itself); `temperature_c` above stays as the core fallback.
    if !status.sensors.is_empty() {
        if let Ok(v) = serde_json::to_value(&status.sensors)
            && !v.is_null()
        {
            map.insert("sensors".into(), v);
        }
        for (desc, temp) in &status.sensors {
            match desc.channel_type {
                Some(0) if !map.contains_key("temp_core") => {
                    map.insert("temp_core".into(), f64_value(*temp as f64));
                }
                Some(1) if !map.contains_key("temp_hotspot") => {
                    map.insert("temp_hotspot".into(), f64_value(*temp as f64));
                }
                Some(3) if !map.contains_key("temp_memory") => {
                    map.insert("temp_memory".into(), f64_value(*temp as f64));
                }
                _ => {}
            }
        }
    }
    // Thermal policy thresholds from the private ClientThermalTarget table
    // (GET-prime 0xC4554575). Two values the TUI dashboard pairs with the live
    // sensor readings: policy 2 = the target-temperature wall (nvidia-smi "GPU
    // Target Temperature" = NVML's GpsCurr channel), policy 1 = max operating
    // temp (NVML's GpuMax). Emitted as plain keys so the TUI can render
    // `CORE <live> / <target> C` and `HOTSPOT <live> / <max> C`. Best-effort:
    // omitted where the driver doesn't expose the slot (desktop GPUs).
    if target.has_nvapi()
        && let Ok(policies) = run(target, QueryNvapiTargetTempPolicies)
    {
        for p in policies.output {
            match p.policy_index {
                2 => {
                    map.insert("target_temp_c".into(), f64_value(p.celsius as f64));
                }
                1 => {
                    map.insert("max_temp_c".into(), f64_value(p.celsius as f64));
                }
                _ => {}
            }
        }
    }
    // Live board power draw (watts). PREFER the NVAPI PowerMonitor "Board"
    // rail (InputTotalBoard, pwr_rail 245/223) over NVML's
    // `nvmlDeviceGetPowerUsage`:
    //
    //   - SAME semantic (total board power draw ≈ nvidia-smi's power_usage).
    //   - NVAPI returns a clean NvAPI_Status error when the dGPU is
    //     D3cold/GCOFF; NVML's `nvmlDeviceGetPowerUsage` segfaults inside
    //     nvml.dll on the stale handle (verified: INVALID_POINTER_READ at
    //     nvmlDeviceGetPowerUsage+0x268, killing NVOC-GUI when the user
    //     switches to 核显模式 / powers the dGPU off). The NVML call is a
    //     native segfault — uncatchable by `panic="abort"` or any try/except,
    //     so it MUST be prevented, not caught.
    //   - Zero extra cost: `status.power_rails` is already populated by
    //     `Gpu::status()` above (the NVAPI PowerMonitor sweep). No new NVAPI
    //     call, no dGPU wake — the dashboard's 1Hz poll does NOT wake the GPU.
    //
    // NVML's `nvmlDeviceGetPowerUsage` is intentionally NOT used as a fallback.
    // It segfaults inside nvml.dll on a stale handle during dGPU transitions
    // (verified: `nvmlDeviceGetPowerUsage+0x268`), and NVAPI recovers before
    // NVML in that window so no pre-call check can make it safe. If neither
    // NVAPI path yields a reading, `power_w` is simply omitted (`---`).
    let mut power_w_set = false;
    if let Some(rails) = &status.power_rails {
        // InputTotalBoard (245) / InputTotalBoard2 (223) = total board power,
        // the GPU-Z "Board Power" equivalent of NVML power_usage. Prefer 245
        // (primary), then 223 (second board-total on some SKUs).
        let board = rails
            .iter()
            .find(|r| r.pwr_rail == 245 && r.pwr_mw > 0)
            .or_else(|| rails.iter().find(|r| r.pwr_rail == 223 && r.pwr_mw > 0));
        if let Some(r) = board {
            map.insert("power_w".into(), f64_value(r.pwr_mw as f64 / 1000.0));
            power_w_set = true;
        }
    }
    if !power_w_set
        && let Some((_channel, power)) = status.power.iter().next()
        && let Some(watts) = first_number_in_display(power)
    {
        // Legacy NVAPI power-topology fallback (dimensionless % on some GPUs,
        // but real watts where present). Does not touch NVML.
        map.insert("power_w".into(), f64_value(watts));
    }
    // Current enforced power limit (the live TGP cap, watts). NVML-only
    // (`nvmlDeviceGetEnforcedPowerLimit`, same "Current Power Limit" nvidia-smi
    // -q -d POWER reports).
    //
    // NOT read live here: `nvmlDeviceGetEnforcedPowerLimit` is an NVML
    // device-level query that segfaults inside nvml.dll on a stale handle
    // during dGPU power-state transitions (verified crash at
    // `nvmlDeviceGetEnforcedPowerLimit+0x276` when re-enabling a powered-off
    // dGPU — NVAPI recovers before NVML's handle state, so no pre-call liveness
    // proof can make NVML device queries safe). The enforced limit is
    // quasi-static (TGP wall), so it is populated by the info path
    // (`normalize_info`, user-initiated / GPU-switch time, when the dGPU is D0)
    // and served from the process-level `POWER_LIMIT_CACHE` here. On cache
    // miss/expiry (e.g. the dGPU was off when info last ran) the field is
    // simply omitted — the dashboard shows `---` rather than crashing.
    if let Some(watts) = cached_power_limit(target.id.0) {
        map.insert("power_limit_w".into(), f64_value(watts));
    }

    // Per-rail power (watts) from NVAPI PowerMonitor, keyed by the
    // descriptor's rail IDENTITY (correct on every GPU — laptop vs desktop
    // expose different rail sets/orderings). Emits a { "<RailName>": <watts> }
    // object. The key carries a confidence marker: plain (Measured, private
    // GetStatus offset), `~` (Inferred, disambiguated from a shared offset), or
    // `?` (Ambiguous, full-board view). Unavailable rails (no GetStatus data)
    // are omitted entirely.
    if let Some(rails) = &status.power_rails {
        let mut rail_map = Map::new();
        for r in rails {
            if r.pwr_mw == 0 {
                continue;
            }
            let suffix = match r.confidence {
                nvapi_hi::nvapi::Confidence::Measured => "",
                nvapi_hi::nvapi::Confidence::Inferred => "~",
                _ => "?", // Ambiguous (or Unavailable, though pwr_mw!=0 filters most)
            };
            let key = if suffix.is_empty() {
                r.rail_name.clone()
            } else {
                format!("{}{}", r.rail_name, suffix)
            };
            rail_map.insert(key, f64_value(r.pwr_mw as f64 / 1000.0));
        }
        if !rail_map.is_empty() {
            map.insert("power_rails_w".into(), Value::Object(rail_map));
        }
    }

    // Bidirectional real-time PCIe bandwidth (MiB/s), nvitop/HWMonitor-style.
    // `nvmlDeviceGetPcieThroughput` reports KB/s averaged over a ~20ms byte-counter
    // interval (i.e. it IS the live rate — no sliding window needed). TX = bytes
    // the GPU sends (GPU->host), RX = bytes the GPU receives (host->GPU), matching
    // the nvidia-smi / nvitop "Tx/Rx" convention. Maxwell+ only; vGPU unsupported.
    //
    // REMOVED from the per-poll path: these are NVML device-level queries on
    // the cached handle and segfault inside nvml.dll during dGPU power-state
    // transitions (same fault class as `nvmlDeviceGetEnforcedPowerLimit` — NVAPI
    // recovers before NVML's handle state, so no pre-call proof can guard them).
    // The fields are simply omitted on the dashboard poll; they are not part of
    // the metric rows (GPU/MEM/VOLT/TEMP/PWR) and only appeared in the
    // `status -a` JSON, so omitting them has no dashboard impact. A future
    // low-frequency info-path read (mirroring the power-limit cache) could
    // restore them if a consumer needs them.
    // (previously: target_nvml_device + pcie_throughput/replay_counter/link_gen)
    map.insert(
        "vfp_locked".into(),
        bool_value(!status.vfp_locks.is_empty()),
    );
    for lock in status.vfp_locks.values() {
        if let Some(mv) = first_number_in_display(lock) {
            map.insert("vfp_lock_mv".into(), f64_value(mv));
            break;
        }
    }

    // Per-domain utilization %: { Graphics, FrameBuffer(=memory controller),
    // VideoEngine, BusInterface }. Serialized verbatim (matches get-status JSON).
    if let Ok(v) = serde_json::to_value(&status.utilization)
        && !v.is_null()
    {
        map.insert("utilization".into(), v);
    }

    // VRAM (KiB): used = dedicated - dedicated_available_current.
    if let Some(mem) = status.memory {
        let total = mem.dedicated.0;
        let free = mem.dedicated_available_current.0;
        let used = total.saturating_sub(free);
        map.insert(
            "vram".into(),
            value_object([
                ("total_kib", u64_value(total as u64)),
                ("used_kib", u64_value(used as u64)),
                ("free_kib", u64_value(free as u64)),
                ("shared_kib", u64_value(mem.shared.0 as u64)),
            ]),
        );
    }

    // Per-cooler fan status (rpm + level % + active), serialized verbatim.
    if let Ok(v) = serde_json::to_value(&status.coolers)
        && !v.is_null()
    {
        map.insert("coolers".into(), v);
    }

    // PCIe link width (downstream lane count).
    if let Some(lanes) = status.pcie_lanes {
        map.insert("pcie_lanes".into(), u64_value(lanes as u64));
    }

    // NVAPI perf / throttle-limit flags (raw bitset; overlaps NVML throttle
    // reasons). `limits_decoded` is the same mask rendered as reason names so
    // consumers (TUI/CLI) don't each have to re-decode the bits.
    let perf_limits_bits = status.perf.limits.bits() as u32;
    map.insert(
        "perf".into(),
        value_object([
            ("unknown", u64_value(status.perf.unknown as u64)),
            ("limits", u64_value(perf_limits_bits as u64)),
            (
                "limits_decoded",
                text_array_value(decode_perf_flags(perf_limits_bits)),
            ),
        ]),
    );

    Ok(Value::Object(map))
}

fn normalize_settings(target: &GpuTarget<'_>) -> PyResultValue {
    let settings = run(target, QueryGpuSettings).map_err(to_py_err)?.output;
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));

    if let Some(boost) = settings.voltage_boost {
        map.insert("voltage_boost_current".into(), u64_value(boost.0 as u64));
    }
    if let Some(limit) = settings.power_limits.first() {
        map.insert("power_limit_current".into(), i64_value(limit.0 as i64));
    }
    if let Some(limit) = settings.sensor_limits.first() {
        map.insert(
            "thermal_limit_current".into(),
            i64_value(limit.value.0 as i64),
        );
    }

    for (pstate, clocks) in &settings.pstate_deltas {
        for (clock, delta) in clocks {
            if *pstate != PState::P0 {
                continue;
            }
            match *clock {
                ClockDomain::Graphics => {
                    map.insert(
                        "core_clock_current".into(),
                        i64_value(khz_to_mhz_i64(delta.0)),
                    );
                }
                ClockDomain::Memory => {
                    map.insert(
                        "mem_clock_current".into(),
                        i64_value(khz_to_mhz_i64(delta.0)),
                    );
                }
                _ => {}
            }
        }
    }

    if let Ok(pstates) = run(target, QueryPstates).map(|report| report.output) {
        let mut labels = Vec::new();
        let mut ranges = Vec::new();
        for item in pstates {
            let label = nvml_pstate_to_str(item.pstate).to_string();
            labels.push(Value::String(label.clone()));
            ranges.push(value_object([
                ("pstate", Value::String(label)),
                ("min_core_mhz", u64_value(item.min_core_mhz as u64)),
                ("max_core_mhz", u64_value(item.max_core_mhz as u64)),
                ("min_memory_mhz", u64_value(item.min_memory_mhz as u64)),
                ("max_memory_mhz", u64_value(item.max_memory_mhz as u64)),
            ]));
        }
        map.insert("supported_pstates".into(), Value::Array(labels));
        map.insert("pstate_ranges".into(), Value::Array(ranges));
    }

    if let Ok(power) = run(target, QueryPowerLimits).map(|report| report.output) {
        map.insert(
            "power_limit_nvml_min_w".into(),
            f64_value(power.min_watts as f64),
        );
        map.insert(
            "power_limit_nvml_current_w".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert(
            "power_limit_nvml_max_w".into(),
            f64_value(power.max_watts as f64),
        );
    }
    if let Ok(fan) = run(target, QueryFanInfo).map(|report| report.output) {
        map.insert("fan_count".into(), u64_value(fan.count as u64));
        map.insert("fan_min".into(), option_u32(fan.min_speed));
        map.insert("fan_max".into(), option_u32(fan.max_speed));
    }
    if let Ok(thresholds) = run(target, QueryTemperatureThresholds).map(|report| report.output) {
        map.insert(
            "temperature_thresholds".into(),
            Value::Array(
                thresholds
                    .into_iter()
                    .map(|threshold| {
                        value_object([
                            ("name", Value::String(threshold.name.to_string())),
                            ("celsius", option_u32(threshold.celsius)),
                        ])
                    })
                    .collect(),
            ),
        );
    }
    let overvolts = run(target, QueryLegacyCoreOvervoltRanges)
        .map(|report| report.output)
        .unwrap_or_default();
    if let Some((pstate, current, min, max)) = overvolts.first() {
        map.insert("legacy_overvolt_pstate".into(), text(pstate));
        map.insert(
            "legacy_overvolt_current_mv".into(),
            i64_value(uv_to_mv_i64(current.0)),
        );
        map.insert(
            "legacy_overvolt_min_mv".into(),
            i64_value(uv_to_mv_i64(min.0)),
        );
        map.insert(
            "legacy_overvolt_max_mv".into(),
            i64_value(uv_to_mv_i64(max.0)),
        );
    }

    let mut locks = Map::new();
    for (id, lock) in &settings.vfp_locks {
        if let Some(value) = lock.lock_value {
            locks.insert(id.to_string(), text(value));
        }
    }
    map.insert("vfp_locks".into(), Value::Object(locks));
    Ok(Value::Object(map))
}

fn normalize_supported_app_clocks(target: &GpuTarget<'_>) -> PyResultValue {
    let items = run(target, QuerySupportedApplicationsClocks)
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|item| {
            value_object([
                ("memory_mhz", u64_value(item.memory_mhz as u64)),
                (
                    "graphics_mhz",
                    Value::Array(
                        item.graphics_mhz
                            .into_iter()
                            .map(|v| u64_value(v as u64))
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_power_limits(target: &GpuTarget<'_>) -> PyResultValue {
    let power = run(target, QueryPowerLimits).map_err(to_py_err)?.output;
    Ok(value_object([
        ("min_watt", f64_value(power.min_watts as f64)),
        ("current_watt", f64_value(power.current_watts as f64)),
        ("max_watt", f64_value(power.max_watts as f64)),
    ]))
}

fn normalize_pstates(target: &GpuTarget<'_>) -> PyResultValue {
    let items = run(target, QueryPstates)
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|item| {
            value_object([
                (
                    "pstate",
                    Value::String(nvml_pstate_to_str(item.pstate).to_string()),
                ),
                ("min_core_mhz", u64_value(item.min_core_mhz as u64)),
                ("max_core_mhz", u64_value(item.max_core_mhz as u64)),
                ("min_memory_mhz", u64_value(item.min_memory_mhz as u64)),
                ("max_memory_mhz", u64_value(item.max_memory_mhz as u64)),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_fan_info(target: &GpuTarget<'_>) -> PyResultValue {
    let fan = run(target, QueryFanInfo).map_err(to_py_err)?.output;
    Ok(value_object([
        ("count", u64_value(fan.count as u64)),
        ("min_percent", option_u32(fan.min_speed)),
        ("max_percent", option_u32(fan.max_speed)),
    ]))
}

fn normalize_temperature_thresholds(target: &GpuTarget<'_>) -> PyResultValue {
    let items = run(target, QueryTemperatureThresholds)
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|item| {
            value_object([
                ("name", Value::String(item.name.to_string())),
                ("celsius", option_u32(item.celsius)),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_throttle_reasons(target: &GpuTarget<'_>) -> PyResultValue {
    let items = run(target, QueryThrottleReasons)
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|item| {
            value_object([
                ("name", Value::String(item.name)),
                ("active", bool_value(item.active)),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_legacy_overvolt_ranges(target: &GpuTarget<'_>) -> PyResultValue {
    let items = run(target, QueryLegacyCoreOvervoltRanges)
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|(pstate, current, min, max)| {
            value_object([
                ("pstate", text(pstate)),
                ("min_uv", i64_value(min.0 as i64)),
                ("current_uv", i64_value(current.0 as i64)),
                ("max_uv", i64_value(max.0 as i64)),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_pstate_base_voltage(target: &GpuTarget<'_>, pstate: PState) -> PyResultValue {
    let voltage = run(target, QueryPstateBaseVoltage { pstate })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([
        ("pstate", text(voltage.pstate)),
        (
            "voltage_domain",
            Value::String(voltage_domain_label(voltage.voltage_domain).to_string()),
        ),
        ("editable", bool_value(voltage.editable)),
        ("voltage_uv", u64_value(voltage.voltage.0 as u64)),
        ("delta_uv", i64_value(voltage.delta.0 as i64)),
        ("min_delta_uv", i64_value(voltage.min_delta.0 as i64)),
        ("max_delta_uv", i64_value(voltage.max_delta.0 as i64)),
    ]))
}

fn normalize_voltage_boost(target: &GpuTarget<'_>) -> PyResultValue {
    let boost = run(target, QueryVoltageBoost).map_err(to_py_err)?.output;
    Ok(value_object([(
        "voltage_boost_percent",
        boost
            .voltage_boost
            .map(percent_value)
            .unwrap_or(Value::Null),
    )]))
}

fn normalize_auto_boost(target: &GpuTarget<'_>) -> PyResultValue {
    let state = run(target, QueryAutoBoost).map_err(to_py_err)?.output;
    Ok(value_object([
        ("enabled", bool_value(state.enabled)),
        ("default_enabled", bool_value(state.default_enabled)),
    ]))
}

fn normalize_api_restriction(target: &GpuTarget<'_>, api_type: Api) -> PyResultValue {
    let state = run(target, QueryApiRestriction { api_type })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([
        (
            "api",
            Value::String(api_restriction_api_label(state.api_type).to_string()),
        ),
        ("restricted", bool_value(state.restricted)),
    ]))
}

fn normalize_displays(target: &GpuTarget<'_>, all: bool) -> PyResultValue {
    let items = run(target, QueryDisplays { all })
        .map_err(to_py_err)?
        .output
        .into_iter()
        .map(|display| {
            value_object([
                (
                    "display_id",
                    Value::String(format!("0x{:08X}", display.display_id)),
                ),
                ("display_id_u32", u64_value(display.display_id as u64)),
                ("connector", Value::String(display.connector)),
                (
                    "flags_hex",
                    Value::String(format!("0x{:08X}", display.flags_bits)),
                ),
                ("connected", bool_value(display.connected)),
                (
                    "physically_connected",
                    bool_value(display.physically_connected),
                ),
                ("active", bool_value(display.active)),
                ("os_visible", bool_value(display.os_visible)),
                ("dynamic", bool_value(display.dynamic)),
                ("mst_root", bool_value(display.mst_root)),
                ("wireless", bool_value(display.wireless)),
            ])
        })
        .collect();
    Ok(Value::Array(items))
}

fn normalize_edid(target: &GpuTarget<'_>, display_id: u32) -> PyResultValue {
    let edid = run(target, QueryEdid { display_id })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([
        (
            "display_id",
            Value::String(format!("0x{:08X}", edid.display_id)),
        ),
        ("bytes", u64_value(edid.bytes.len() as u64)),
        ("edid_hex", Value::String(bytes_to_upper_hex(&edid.bytes))),
    ]))
}

fn normalize_query_vfp_point(target: &GpuTarget<'_>, point: usize) -> PyResultValue {
    let voltage = run(target, QueryVfpPointVoltage { point })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([("microvolts", u64_value(voltage.0 as u64))]))
}

fn normalize_legacy_p0_delta(target: &GpuTarget<'_>) -> PyResultValue {
    let value = run(target, QueryLegacyP0CoreMaxVoltageDelta)
        .map_err(to_py_err)?
        .output;
    Ok(value_object([(
        "microvolts",
        value.map(|v| u64_value(v.0 as u64)).unwrap_or(Value::Null),
    )]))
}

fn normalize_tdp_temp_limits(target: &GpuTarget<'_>) -> PyResultValue {
    let limits = run(target, QueryTdpTempLimits).map_err(to_py_err)?.output;
    Ok(value_object([
        ("min_tdp", percent_value(limits.min_tdp)),
        ("default_tdp", percent_value(limits.default_tdp)),
        ("max_tdp", percent_value(limits.max_tdp)),
        ("min_temp", u64_value(limits.min_temp.0 as u64)),
        ("default_temp", u64_value(limits.default_temp.0 as u64)),
        ("max_temp", u64_value(limits.max_temp.0 as u64)),
    ]))
}

fn normalize_voltage_limits(target: &GpuTarget<'_>) -> PyResultValue {
    let value = run(target, nvoc_core::ProbeVoltageLimits)
        .map_err(to_py_err)?
        .output;
    Ok(value_object([
        ("lower_point", u64_value(value.lower_point as u64)),
        ("upper_point", u64_value(value.upper_point as u64)),
    ]))
}

fn normalize_voltage_check(target: &GpuTarget<'_>, point: usize) -> PyResultValue {
    let check = run(target, CheckVoltageFrequency { point })
        .map_err(to_py_err)?
        .output;
    let voltage = run(target, QueryVfpPointVoltage { point })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([
        ("precise", bool_value(check.precise)),
        (
            "matched_point",
            check
                .matched_point
                .map(|point| u64_value(point as u64))
                .unwrap_or(Value::Null),
        ),
        ("microvolts", u64_value(voltage.0 as u64)),
    ]))
}

fn normalize_query_clock_offset(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
    pstate: PerformanceState,
) -> PyResultValue {
    let value = run(target, nvoc_core::QueryClockOffset { domain, pstate })
        .map_err(to_py_err)?
        .output;
    Ok(value_object([("mhz", i64_value(value.mhz as i64))]))
}

fn normalize_domain_vfp_points(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
    infer_missing_default: bool,
) -> PyResultValue {
    let points = run(
        target,
        QueryDomainVfpPoints {
            domain,
            infer_missing_default,
            indexed: true,
        },
    )
    .map_err(to_py_err)?
    .output
    .into_iter()
    .map(|(index, point)| {
        value_object([
            ("index", u64_value(index as u64)),
            ("voltage_uv", u64_value(point.voltage.0 as u64)),
            ("frequency_khz", u64_value(point.frequency.0 as u64)),
            ("delta_khz", i64_value(point.delta.0 as i64)),
            (
                "default_frequency_khz",
                u64_value(point.default_frequency.0 as u64),
            ),
        ])
    })
    .collect();
    Ok(Value::Array(points))
}

#[pyfunction]
fn discover_gpus(py: Python<'_>, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let backends_str = backends.unwrap_or("both").to_string();
    // Release the GIL: discovery loads nvapi64 + inits NVAPI (hundreds of
    // ms) and the GUI calls it at tick zero to overlap UI construction —
    // holding the GIL would stall the main thread's Python widget build.
    let items = py.detach(|| -> PyResult<Vec<Value>> {
        let backends = parse_backends(&backends_str)?;
        let inventory = {
            let mut cache = lock_inventory_cache();
            cache.refresh(backends)?
        };
        let mut items = Vec::new();
        for target in inventory.0.targets() {
            let mut item = Map::new();
            item.insert("index".into(), u64_value(target.index as u64));
            item.insert("gpu_id".into(), u64_value(target.id.0 as u64));
            item.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
            item.insert("backend_nvapi".into(), bool_value(target.has_nvapi()));
            item.insert("backend_nvml".into(), bool_value(target.has_nvml()));
            if let Ok(info) = run(&target, QueryGpuInfo).map(|report| report.output) {
                // Capability flags from gpu_type.rs detect_gpu_type (name +
                // codename) right at probe time — the GUI builds the correct
                // mobile/desktop layout on FIRST paint instead of drawing the
                // desktop modal and re-packing when the info query lands.
                let series = detect_gpu_type(&format!("{}{}", info.name, info.codename));
                item.insert("name".into(), text(info.name));
                item.insert("codename".into(), text(info.codename));
                item.insert("arch".into(), text(info.arch));
                item.insert("is_mobile".into(), bool_value(series.is_mobile()));
                item.insert(
                    "is_legacy_voltage".into(),
                    bool_value(series.is_legacy_voltage()),
                );
                item.insert(
                    "xbar_supported".into(),
                    bool_value(series.supports_xbar_offset()),
                );
            }
            items.push(Value::Object(item));
        }
        Ok(items)
    })?;
    py_value(py, &Value::Array(items))
}

#[pyfunction]
fn query_info(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let gpu = gpu.to_string();
    let backends = backends.unwrap_or("both").to_string();
    let value = py.detach(|| with_target(&gpu, &backends, normalize_info))?;
    py_value(py, &value)
}

#[pyfunction]
fn query_status(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    // GIL released: the 1 Hz dashboard poll runs this sweep while the GUI
    // main thread handles events — holding the GIL stalls drag/click streams.
    let gpu = gpu.to_string();
    let backends = backends.unwrap_or("both").to_string();
    let value = py.detach(|| with_target(&gpu, &backends, normalize_status))?;
    py_value(py, &value)
}

#[pyfunction]
fn query_settings(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let gpu = gpu.to_string();
    let backends = backends.unwrap_or("both").to_string();
    let value = py.detach(|| with_target(&gpu, &backends, normalize_settings))?;
    py_value(py, &value)
}

#[pyfunction]
fn query_supported_applications_clocks(
    py: Python<'_>,
    gpu: &str,
    backends: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let value = with_target(
        gpu,
        backends.unwrap_or("nvml"),
        normalize_supported_app_clocks,
    )?;
    py_value(py, &value)
}

#[pyfunction]
fn query_power_limits(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_power_limits)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_pstates(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_pstates)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_fan_info(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_fan_info)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_temperature_thresholds(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_temperature_thresholds)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_throttle_reasons(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_throttle_reasons)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_legacy_overvolt_ranges(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", normalize_legacy_overvolt_ranges)?;
    py_value(py, &value)
}

#[pyfunction]
#[pyo3(signature = (gpu, pstate = None))]
fn query_pstate_base_voltage(
    py: Python<'_>,
    gpu: &str,
    pstate: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let pstate = parse_pstate(pstate.unwrap_or("P0"))?;
    let value = with_target(gpu, "nvapi", |target| {
        normalize_pstate_base_voltage(target, pstate)
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn query_voltage_boost(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", normalize_voltage_boost)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_auto_boost(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvml", normalize_auto_boost)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_api_restriction(py: Python<'_>, gpu: &str, api_type: &str) -> PyResult<Py<PyAny>> {
    let api_type = parse_api_restriction_api(api_type)?;
    let value = with_target(gpu, "nvml", |target| {
        normalize_api_restriction(target, api_type)
    })?;
    py_value(py, &value)
}

#[pyfunction]
#[pyo3(signature = (gpu, all = false))]
fn list_displays(py: Python<'_>, gpu: &str, all: bool) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| normalize_displays(target, all))?;
    py_value(py, &value)
}

#[pyfunction]
fn query_edid(py: Python<'_>, gpu: &str, display_id: &str) -> PyResult<Py<PyAny>> {
    let display_id = parse_display_id(display_id)?;
    let value = with_target(gpu, "nvapi", |target| normalize_edid(target, display_id))?;
    py_value(py, &value)
}

#[pyfunction]
fn query_clock_offset(
    py: Python<'_>,
    gpu: &str,
    backends: Option<&str>,
    domain: &str,
    pstate: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let backend = parse_backend(backends.unwrap_or("nvml"))?;
    let domain = parse_domain(domain)?;
    let pstate = parse_nvml_pstate(pstate.unwrap_or("P0"))?;
    let backends = if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Both
    };
    let inventory = {
        let mut cache = lock_inventory_cache();
        cache.entry(backends)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    let value = normalize_query_clock_offset(&target, domain, pstate)?;
    py_value(py, &value)
}

#[pyfunction]
#[pyo3(signature = (gpu, domain = None, infer_missing_default = true))]
fn query_domain_vfp_points(
    py: Python<'_>,
    gpu: &str,
    domain: Option<&str>,
    infer_missing_default: bool,
) -> PyResult<Py<PyAny>> {
    let domain = parse_domain(domain.unwrap_or("graphics"))?;
    let gpu = gpu.to_string();
    // GIL released: the VFP table RM escape is the heaviest read (curve
    // refresh / auto-refresh) and must not stall the GUI event stream.
    let value = py.detach(|| {
        with_target(&gpu, "nvapi", |target| {
            normalize_domain_vfp_points(target, domain, infer_missing_default)
        })
    })?;
    py_value(py, &value)
}

/// 原生 GC6 唤醒（force_gc6_exit）。移动端 dGPU 空闲掉电（GCOFF）后，
/// NVAPI 读操作会失败并被上层误判为"不支持"；长驻 GUI/TUI 在读取
/// 能力/限制类数据前调用本函数把 GPU 拉回 D0。唤醒非持久。
/// 桌面 GPU（无 GC6）返回 Ok(false)，唤醒成功返回 Ok(true)。
#[pyfunction]
fn force_wake(gpu: &str) -> PyResult<bool> {
    let backends = parse_backends("nvapi")?;
    let inventory = {
        let mut cache = lock_inventory_cache();
        cache.entry(backends)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    // best-effort：桌面端（无 GC6）驱动返回 NoImplementation(-104) 等
    // 错误，同样按"无需唤醒"返回 false，绝不向 Python 抛异常。
    Ok(target.force_wake().is_ok())
}

#[pyfunction]
fn query_vfp_point_voltage(py: Python<'_>, gpu: &str, point: usize) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        normalize_query_vfp_point(target, point)
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn query_legacy_p0_core_max_voltage_delta(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", normalize_legacy_p0_delta)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_tdp_temp_limits(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", normalize_tdp_temp_limits)?;
    py_value(py, &value)
}

#[pyfunction]
fn probe_voltage_limits(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", normalize_voltage_limits)?;
    py_value(py, &value)
}

#[pyfunction]
fn check_voltage_frequency(py: Python<'_>, gpu: &str, point: usize) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        normalize_voltage_check(target, point)
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn set_clock_offset(
    py: Python<'_>,
    gpu: &str,
    backend: &str,
    domain: &str,
    value: f64,
    pstate: Option<&str>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let backend_own = backend.to_string();
    let domain_own = domain.to_string();
    let pstate_own = pstate.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let backend: &str = &backend_own;
        let domain: &str = &domain_own;
        let pstate: Option<&str> = pstate_own.as_deref();

        // One decimal MHz is allowed — the 2.5 MHz GUI grid divides both the
        // 7.5 MHz (30-series+) and 12.5 MHz (10/16/20-series) hardware steps.
        // NVAPI takes kHz (round to nearest); NVML's API is integer MHz only.
        if !value.is_finite() {
            return Err(PyRuntimeError::new_err(format!(
                "clock offset {value} MHz is not a finite number"
            )));
        }
        let backend = parse_backend(backend)?;
        let domain = parse_domain(domain)?;
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(if backend == "nvml" {
                BackendSet::Nvml
            } else {
                BackendSet::Nvapi
            })?
        };
        let target = selected_target(&inventory.0, gpu)?;
        match backend {
            "nvml" => {
                let pstate = parse_nvml_pstate(pstate.unwrap_or("P0"))?;
                run(
                    &target,
                    SetClockOffset {
                        domain,
                        pstate,
                        mhz: value.round() as i32,
                    },
                )
                .map_err(to_py_err)?;
            }
            "nvapi" => {
                let pstate = parse_pstate(pstate.unwrap_or("P0"))?;
                let delta_khz = (value * 1000.0).round() as i32;
                run(
                    &target,
                    SetPstateClockOffset {
                        pstate,
                        domain,
                        delta: KilohertzDelta(delta_khz),
                    },
                )
                .map_err(to_py_err)?;
            }
            _ => {
                return Err(invalid_value(
                    "clock offsets require backend 'nvapi' or 'nvml'",
                ));
            }
        }
        Ok(())
    })
}

#[pyfunction]
fn set_power_limit(py: Python<'_>, gpu: &str, backend: &str, value: u32) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let backend_own = backend.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let backend: &str = &backend_own;

        let backend = parse_backend(backend)?;
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(if backend == "nvml" {
                BackendSet::Nvml
            } else {
                BackendSet::Nvapi
            })?
        };
        let target = selected_target(&inventory.0, gpu)?;
        match backend {
            "nvml" => {
                run(&target, SetPowerLimit { watts: value }).map_err(to_py_err)?;
            }
            "nvapi" => {
                run(
                    &target,
                    SetNvapiPowerLimits {
                        limits: vec![Percentage(value)],
                    },
                )
                .map_err(to_py_err)?;
            }
            _ => {
                return Err(invalid_value(
                    "power limits require backend 'nvapi' or 'nvml'",
                ));
            }
        }
        Ok(())
    })
}

#[pyfunction]
fn set_thermal_limit(py: Python<'_>, gpu: &str, celsius: i32) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Both)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        if target.has_nvapi() {
            run(
                &target,
                SetNvapiSensorLimits {
                    limits: vec![Celsius(celsius).into()],
                },
            )
            .map_err(to_py_err)?;
        } else {
            run(&target, SetTemperatureLimit { celsius }).map_err(to_py_err)?;
        }
        Ok(())
    })
}

#[pyfunction]
fn set_dynamic_boost(gpu: &str, active: bool) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, SetNvapiDynamicBoost { active }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn query_tgp_watt_range(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let info = run(target, QueryNvapiTgpWattRange)
            .map_err(to_py_err)?
            .output;
        Ok(match info {
            None => Value::Null,
            Some(r) => value_object([
                ("policy_index", Value::from(r.policy_index)),
                (
                    "min_watt",
                    r.min_watt.map(Value::from).unwrap_or(Value::Null),
                ),
                (
                    "default_watt",
                    r.default_watt.map(Value::from).unwrap_or(Value::Null),
                ),
                (
                    "max_watt",
                    r.max_watt.map(Value::from).unwrap_or(Value::Null),
                ),
            ]),
        })
    })?;
    py_value(py, &value)
}

#[pyfunction]
#[pyo3(signature = (gpu, watts, policy_index = None))]
fn set_tgp_watt(
    py: Python<'_>,
    gpu: &str,
    watts: u32,
    policy_index: Option<usize>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetNvapiTgpWatt {
                watts,
                policy_index,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
#[pyo3(signature = (gpu, policy_index = None))]
fn reset_tgp_watt(py: Python<'_>, gpu: &str, policy_index: Option<usize>) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            ResetNvapiTgpWatt {
                policy_index: policy_index.or(Some(2)),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn query_dnotifier(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let info = run(target, QueryNvapiDNotifier).map_err(to_py_err)?.output;
        Ok(match info {
            None => Value::Null,
            Some(r) => value_object([
                (
                    "active",
                    r.active
                        .map(|l| Value::String(format!("D{l}")))
                        .unwrap_or(Value::Null),
                ),
                (
                    "levels",
                    Value::Array(
                        r.levels
                            .iter()
                            .map(|l| {
                                value_object([
                                    ("level", Value::String(format!("D{}", l.level))),
                                    ("watts", l.watts.map(Value::from).unwrap_or(Value::Null)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        })
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn set_dnotifier(py: Python<'_>, gpu: &str, level: u8) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        if !(1..=5).contains(&level) {
            return Err(invalid_value("D-Notifier level must be 1..5 (D1-D5)"));
        }
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(&target, SetNvapiDNotifier { level }).map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
#[pyo3(signature = (gpu, celsius, policy_index = None))]
fn set_target_temp(
    py: Python<'_>,
    gpu: &str,
    celsius: f32,
    policy_index: Option<usize>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetNvapiTargetTemp {
                celsius,
                policy_index,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn query_target_temp_policies(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let policies = run(target, QueryNvapiTargetTempPolicies).map_err(to_py_err)?;
        Ok(Value::Array(
            policies
                .output
                .into_iter()
                .map(|p| {
                    value_object([
                        ("policy_index", Value::from(p.policy_index)),
                        ("celsius", Value::from(p.celsius as f64)),
                        (
                            "min",
                            p.min.map(|v| Value::from(v as f64)).unwrap_or(Value::Null),
                        ),
                        (
                            "default",
                            p.default
                                .map(|v| Value::from(v as f64))
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "max",
                            p.max.map(|v| Value::from(v as f64)).unwrap_or(Value::Null),
                        ),
                    ])
                })
                .collect(),
        ))
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn query_volt_rails(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let rails = run(target, QueryNvapiVoltRails).map_err(to_py_err)?.output;
        Ok(match rails {
            Some(r) => {
                let entries = |list: &[nvapi_hi::nvapi::VoltRailEntry]| {
                    Value::Array(
                        list.iter()
                            .map(|e| {
                                value_object([
                                    ("rail_bit", Value::from(e.rail_bit)),
                                    ("type", Value::from(e.entry_type)),
                                    (
                                        "values_uV",
                                        Value::Array(
                                            e.values.iter().map(|v| Value::from(*v)).collect(),
                                        ),
                                    ),
                                ])
                            })
                            .collect(),
                    )
                };
                value_object([
                    ("rail_mask", Value::from(format!("0x{:08X}", r.rail_mask))),
                    (
                        "p0",
                        match r.p0_bounds() {
                            Some(b) => {
                                let mut ceiling = b.vrm_max_wall_uV;
                                if b.vbios_wall_uV > 0 && b.vbios_wall_uV < ceiling {
                                    ceiling = b.vbios_wall_uV;
                                }
                                #[allow(non_snake_case)]
                                // uV-suffixed local matches the nvapi-rs field naming
                                let ceiling_uV = (ceiling - b.effective_wall_uV).max(0);
                                value_object([
                                    ("current_uV", Value::from(b.current_uV)),
                                    ("target_wall_uV", Value::from(b.target_wall_uV)),
                                    ("effective_wall_uV", Value::from(b.effective_wall_uV)),
                                    ("vbios_wall_uV", Value::from(b.vbios_wall_uV)),
                                    ("vrm_max_wall_uV", Value::from(b.vrm_max_wall_uV)),
                                    ("min_hold_uV", Value::from(b.min_hold_uV)),
                                    ("offset_ceiling_uV", Value::from(ceiling_uV)),
                                ])
                            }
                            None => Value::Null,
                        },
                    ),
                    (
                        "rail_descriptors",
                        Value::Array(
                            r.rail_descriptors
                                .iter()
                                .map(|d| {
                                    value_object([
                                        ("rail_bit", Value::from(d.rail_bit)),
                                        ("type", Value::from(d.entry_type())),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("control", entries(&r.control)),
                    ("status", entries(&r.status)),
                ])
            }
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Set a volt-rail to an ABSOLUTE target voltage (millivolts, may carry one
/// decimal — e.g. 1082.5 mV for the 2.5 mV rail step on 10/20-series). The
/// required µV offset is derived inside the operation from the live control
/// offset + status target wall — callers think in mV, not offsets. Returns
/// the derived base wall, the offset written, and the effective wall read
/// back (clamped by the driver to min(target, vbios_wall, vrm_max_wall)).
#[pyfunction]
fn set_volt_rail_target(
    py: Python<'_>,
    gpu: &str,
    rail_bit: u32,
    target_mv: f64,
    expect_type: Option<u32>,
) -> PyResult<Py<PyAny>> {
    // mV → µV. One decimal mV (0.1 mV = 100 µV) is well below any hardware
    // rail step, so round to the nearest µV — the driver clamps to its own
    // step grid anyway. Reject NaN/inf before touching the value.
    if !target_mv.is_finite() {
        return Err(PyRuntimeError::new_err(format!(
            "target {target_mv}mV is not a finite number"
        )));
    }
    #[allow(non_snake_case)] // uV-suffixed local matches the nvapi-rs naming
    let target_uV = i32::try_from((target_mv * 1000.0).round() as i64).map_err(|_| {
        PyRuntimeError::new_err(format!("target {target_mv}mV overflows the µV range"))
    })?;
    let value = with_target(gpu, "nvapi", |target| {
        let out = run(
            target,
            SetNvapiVoltRailTarget {
                rail_bit,
                target_uV,
                expected_type: expect_type,
            },
        )
        .map_err(to_py_err)?
        .output;
        Ok(match out {
            Some(a) => value_object([
                ("applied", Value::from(true)),
                ("rail_bit", Value::from(a.rail_bit)),
                ("target_uV", Value::from(a.target_uV)),
                ("base_wall_uV", Value::from(a.base_wall_uV)),
                ("offset_uV", Value::from(a.offset_uV)),
                ("previous_offset_uV", Value::from(a.previous_offset_uV)),
                ("applied_uV", Value::from(a.applied_uV)),
                ("effective_wall_uV", Value::from(a.effective_wall_uV)),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Set the GPU frequency perf-cap (NVAPI PerfLimitsSetStatus 0x32CA4983, the
/// ref tool `-gpuclk:<MHz>` SETTER). Clamp the perf max/min frequency.
/// `max_mhz`/`min_mhz` are in MHz; pass -1 for both to reset the cap. The
/// GUI/CLI speak MHz; the underlying NVAPI struct takes kHz (×1000). Either
/// bound may be 0 to leave that side unset (GPUMon sets both to the same cap).
#[pyfunction]
fn set_perf_freq_cap(py: Python<'_>, gpu: &str, max_mhz: i32, min_mhz: i32) -> PyResult<Py<PyAny>> {
    let cap = if max_mhz < 0 && min_mhz < 0 {
        NvapiPerfFreqCap::Reset
    } else {
        NvapiPerfFreqCap::Cap {
            max_khz: (max_mhz.max(0) as u32).saturating_mul(1000),
            min_khz: (min_mhz.max(0) as u32).saturating_mul(1000),
        }
    };
    let value = py.detach(|| {
        with_target(gpu, "nvapi", |target| {
            run(target, SetNvapiPerfFreqCap { cap }).map_err(to_py_err)?;
            Ok(value_object([("applied", Value::from(true))]))
        })
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn set_volt_rail_offset(
    py: Python<'_>,
    gpu: &str,
    rail_bit: u32,
    offset_uv: i32,
    expect_type: Option<u32>,
) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let out = run(
            target,
            SetNvapiVoltRailOffset {
                rail_bit,
                offset_uV: offset_uv,
                expected_type: expect_type,
            },
        )
        .map_err(to_py_err)?
        .output;
        Ok(match out {
            Some(a) => value_object([
                ("applied", Value::from(true)),
                ("rail_bit", Value::from(a.rail_bit)),
                ("previous_uV", Value::from(a.previous_uV)),
                ("applied_uV", Value::from(a.applied_uV)),
                ("effective_wall_uV", Value::from(a.effective_wall_uV)),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Read the private ClockClient domain-control block: the controllable-domain
/// mask + per-domain offset/range records. This is the XBar physical-clock
/// family (RM 0x2080901b GET_CONTROL). Returns `{"supported": false}` when the
/// driver does not expose the private interface.
#[pyfunction]
fn query_clk_domains(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let ctrl = run(target, QueryNvapiClkDomains).map_err(to_py_err)?.output;
        Ok(match ctrl {
            Some(c) => value_object([
                (
                    "controllable_mask",
                    Value::from(format!("0x{:08X}", c.mask)),
                ),
                (
                    "entries",
                    Value::Array(
                        c.entries
                            .iter()
                            .map(|e| {
                                value_object([
                                    ("bit", Value::from(e.bit)),
                                    ("type", Value::from(e.entry_type)),
                                    // false = the protocol doesn't marshal this
                                    // record type's value fields (e.g. 0x02) —
                                    // values_kHz below is NOT driver data.
                                    ("value_modifiable", Value::from(e.value_modifiable)),
                                    // 8 value dwords (V2 rec+268..296); slot
                                    // semantics driver-opaque, slot 0 = the
                                    // signed frequency offset per the article
                                    (
                                        "values_kHz",
                                        Value::Array(
                                            e.values_kHz.iter().map(|v| Value::from(*v)).collect(),
                                        ),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Read the private ClockClient V/F-points family (GetInfo 0x8895B510 →
/// GetStatus 0x7FEE9032): per-bank point masks + V/F curve records.
/// Records are voltage-indexed; units live-calibrated vs the public GPC VFP
/// curve (voltage µV, default/current MHz). Bank 0 packs multiple domains:
/// type-8 segments are V/F curves (GPC first, then the 127-point XBAR
/// candidate), type-7 segments are per-domain pstate frequency lists.
/// Returns `{"supported": false}` when the driver doesn't expose it.
#[pyfunction]
fn query_clk_vf_points(py: Python<'_>, gpu: &str) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let vfp = run(target, QueryNvapiClkVfPoints)
            .map_err(to_py_err)?
            .output;
        Ok(match vfp {
            Some(v) => value_object([
                (
                    "masks",
                    Value::Array(
                        v.masks
                            .iter()
                            .map(|m| Value::from(format!("0x{:016X}", m)))
                            .collect(),
                    ),
                ),
                (
                    "segments",
                    Value::Array(
                        v.segments
                            .iter()
                            .map(|s| {
                                value_object([
                                    ("bank", Value::from(s.bank)),
                                    // EMPIRICAL advisory attribution
                                    ("domain", Value::from(s.domain_hint.as_str())),
                                    (
                                        "kind",
                                        Value::from(match s.kind {
                                            nvapi_hi::nvapi::ClkVfSegmentKind::VfCurve => {
                                                "vf_curve"
                                            }
                                            nvapi_hi::nvapi::ClkVfSegmentKind::PstateBins => {
                                                "pstate_bins"
                                            }
                                        }),
                                    ),
                                    ("type", Value::from(s.record_type)),
                                    ("start_index", Value::from(s.start_index)),
                                    ("end_index", Value::from(s.end_index)),
                                    ("count", Value::from(s.count)),
                                    ("voltage_uV_min", Value::from(s.voltage_uV_min)),
                                    ("voltage_uV_max", Value::from(s.voltage_uV_max)),
                                    ("freq_default_mhz_min", Value::from(s.freq_default_mhz_min)),
                                    ("freq_default_mhz_max", Value::from(s.freq_default_mhz_max)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "points",
                    Value::Array(
                        v.points
                            .iter()
                            .map(|p| {
                                value_object([
                                    ("bank", Value::from(p.bank)),
                                    ("index", Value::from(p.index)),
                                    ("type", Value::from(p.record_type)),
                                    // the V/F grid axis (µV): 450000 = 450 mV
                                    ("voltage_uV", Value::from(p.voltage_uV)),
                                    // default MHz at this voltage
                                    ("freq_default_mhz", Value::from(p.freq_default_mhz)),
                                    // current MHz = default + applied offset
                                    ("freq_current_mhz", Value::from(p.freq_current_mhz)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Measure one clock domain's physical clock via two-sample MEASURE_FREQ (RM
/// 0x20809006). `domain_bit` is the sequential domain index: GPC=0, XBAR=1,
/// SYS=2, MCLK=4. Returns the frequency in MHz, or `{"supported": false}`.
#[pyfunction]
fn query_clk_domain_freq(py: Python<'_>, gpu: &str, domain_bit: u32) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let freq = run(target, QueryNvapiClkDomainFreq { domain_bit })
            .map_err(to_py_err)?
            .output;
        Ok(match freq {
            Some(f) => value_object([
                ("domain_bit", Value::from(domain_bit)),
                ("freq_mhz", Value::from(f.freq_mhz)),
            ]),
            None => value_object([
                ("supported", Value::from(false)),
                ("domain_bit", Value::from(domain_bit)),
            ]),
        })
    })?;
    py_value(py, &value)
}

/// Write a signed kHz offset into one clock-domain control record (RM
/// 0x2080d01c SET_CONTROL). DANGEROUS GPU clock write: the operation snapshots
/// the full V2 GetControl block, version-gates (magic 0x261A4), patches a copy,
/// SETs, readbacks, and restores on mismatch. When `temporary` is true the
/// snapshot is restored before returning (the article's reversible experiment
/// recipe). `slot` picks which of the record's 8 value dwords to write (0-7,
/// default 0 = the signed frequency offset; other slots are driver-opaque).
/// No magnitude limit is enforced — the caller owns offset/range policy (the
/// article bounds XBAR ±60000 kHz on GB202).
#[pyfunction]
fn set_clk_domain_offset(
    py: Python<'_>,
    gpu: &str,
    domain_bit: u32,
    offset_khz: i32,
    slot: Option<u32>,
    temporary: Option<bool>,
) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, "nvapi", |target| {
        let out = run(
            target,
            SetNvapiClkDomainOffset {
                domain_bit,
                offset_kHz: offset_khz,
                slot: slot.unwrap_or(0),
                temporary: temporary.unwrap_or(false),
            },
        )
        .map_err(to_py_err)?
        .output;
        Ok(match out {
            Some(a) => value_object([
                ("applied", Value::from(true)),
                ("bit", Value::from(a.bit)),
                ("type", Value::from(a.entry_type)),
                ("slot", Value::from(a.slot)),
                ("previous_kHz", Value::from(a.previous_kHz)),
                ("applied_kHz", Value::from(a.applied_kHz)),
                (
                    "values_kHz",
                    Value::Array(a.values_kHz.iter().map(|v| Value::from(*v)).collect()),
                ),
                ("temporary_restored", Value::from(a.temporary_restored)),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

/// Write one V/F curve point via the private ClockClient V/F-POINTS
/// SetControl (ID 0xFEC00D04). DANGEROUS: snapshots the full control
/// block, patches one record (mode 0 freq-offset / mode 1 delta), SETs,
/// readbacks, restores on mismatch. `bank` 0 = V/F curve, 1 = pstate-class
/// curve points; `idx` 0..2047. `freq_mode` = mode 0 (u32 kHz) vs mode 1
/// (i16 delta). Returns `{"supported": false}` when the driver refuses.
#[pyfunction]
fn set_vfp_point_private(
    py: Python<'_>,
    gpu: &str,
    bank: usize,
    idx: usize,
    value_mhz: i32,
    freq_mode: Option<bool>,
) -> PyResult<Py<PyAny>> {
    let freq_mode = freq_mode.unwrap_or(false);
    let value = with_target(gpu, "nvapi", |target| {
        let out = run(
            target,
            SetNvapiVfpPointPrivate {
                bank,
                idx,
                freq_mode,
                value: value_mhz as u32,
            },
        )
        .map_err(to_py_err)?
        .output;
        Ok(match out {
            Some(retained) => value_object([
                ("applied", Value::from(true)),
                ("bank", Value::from(bank as u64)),
                ("index", Value::from(idx as u64)),
                (
                    "mode",
                    Value::from(if freq_mode { "freq" } else { "delta" }),
                ),
                ("value_mhz", Value::from(value_mhz)),
                ("retained_raw", Value::from(retained)),
            ]),
            None => value_object([("supported", Value::from(false))]),
        })
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn set_applications_clocks(gpu: &str, memory_mhz: u32, graphics_mhz: u32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetApplicationsClocks {
            memory_mhz,
            graphics_mhz,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_applications_clocks(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetApplicationsClocks).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_locked_clocks(
    py: Python<'_>,
    gpu: &str,
    backend: &str,
    domain: &str,
    min_mhz: u32,
    max_mhz: u32,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let backend_own = backend.to_string();
    let domain_own = domain.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let backend: &str = &backend_own;
        let domain: &str = &domain_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvml)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        let domain = parse_domain(domain)?;
        if parse_backend(backend)? != "nvml" {
            return Err(invalid_value("locked clocks currently use the NVML path"));
        }
        run(
            &target,
            SetLockedClocks {
                domain,
                min_mhz,
                max_mhz,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_locked_clocks(py: Python<'_>, gpu: &str, backend: &str, domain: &str) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let backend_own = backend.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let backend: &str = &backend_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvml)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        let domain = parse_domain(domain)?;
        if parse_backend(backend)? != "nvml" {
            return Err(invalid_value("locked clocks currently use the NVML path"));
        }
        run(&target, ResetLockedClocks { domain }).map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_fan_speed(gpu: &str, fan_index: u32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetFanSpeed { fan_index }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_pstate_base_voltage(gpu: &str, pstate: &str, delta_uv: i32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetPstateBaseVoltage {
            pstate: parse_pstate(pstate)?,
            delta_uv: MicrovoltsDelta(delta_uv),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_pstate_base_voltages(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetPstateBaseVoltages).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_pstate_clock_offset(gpu: &str, pstate: &str, domain: &str, delta: i32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetPstateClockOffset {
            pstate: parse_pstate(pstate)?,
            domain: parse_domain(domain)?,
            delta: KilohertzDelta(delta),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn sync_memory_pstate_as_p0(py: Python<'_>, gpu: &str) -> PyResult<()> {
    py.detach(|| -> PyResult<()> {
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        nvoc_core::sync_memory_pstate_as_p0(&target).map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_cooler_levels(
    gpu: &str,
    policy: &str,
    level: u32,
    target_name: Option<&str>,
) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    let cooler_target = match target_name.unwrap_or("all") {
        "1" => nvoc_core::CoolerTarget::Cooler1,
        "2" => nvoc_core::CoolerTarget::Cooler2,
        _ => nvoc_core::CoolerTarget::All,
    };
    let policy = CoolerPolicy::from_str(policy).map_err(invalid_value)?;
    run(
        &target,
        SetCoolerLevels {
            policy,
            level,
            cooler_target,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_vfp_frequency_lock(
    py: Python<'_>,
    gpu: &str,
    domain: &str,
    upper_khz: i32,
    lower_khz: Option<i32>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let domain_own = domain.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let domain: &str = &domain_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetVfpFrequencyLock {
                domain: parse_domain(domain)?,
                upper: nvapi_hi::Kilohertz(upper_khz.max(0) as u32),
                lower: lower_khz.map(|v| nvapi_hi::Kilohertz(v.max(0) as u32)),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_vfp_frequency_lock(py: Python<'_>, gpu: &str, domain: &str) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            ResetVfpFrequencyLock {
                domain: parse_domain(domain)?,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_vfp_voltage_lock(
    py: Python<'_>,
    gpu: &str,
    point: Option<usize>,
    voltage_uv: Option<i32>,
    feedback: Option<bool>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        let voltage_target = if let Some(point) = point {
            nvoc_core::NvapiLockedVoltageTarget::Point(point)
        } else if let Some(voltage_uv) = voltage_uv {
            nvoc_core::NvapiLockedVoltageTarget::Voltage(nvapi_hi::Microvolts(
                voltage_uv.max(0) as u32
            ))
        } else {
            return Err(invalid_value("expected either point or voltage"));
        };
        run(
            &target,
            SetVfpVoltageLock {
                voltage_target,
                feedback: feedback.unwrap_or(false),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_vfp_deltas(py: Python<'_>, gpu: &str, domain: Option<&str>) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let domain_own = domain.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let domain: Option<&str> = domain_own.as_deref();

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        let domain = match domain.unwrap_or("all") {
            "all" => VfpResetDomain::All,
            "core" => VfpResetDomain::Core,
            "memory" => VfpResetDomain::Memory,
            other => return Err(invalid_value(format!("invalid VFP reset domain {other:?}"))),
        };
        run(&target, ResetVfpDeltas { domain }).map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_vfp_point_delta(gpu: &str, point: usize, delta: i32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetVfpPointDelta {
            point,
            delta: KilohertzDelta(delta),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_vfp_range_delta(
    py: Python<'_>,
    gpu: &str,
    start: usize,
    end: usize,
    delta: i32,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetVfpRangeDelta {
                start,
                end,
                delta: KilohertzDelta(delta),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_domain_vfp_deltas(
    py: Python<'_>,
    gpu: &str,
    domain: &str,
    deltas: Vec<(usize, i32)>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let domain_own = domain.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let domain: &str = &domain_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        let deltas = deltas
            .into_iter()
            .map(|(p, d)| (p, KilohertzDelta(d)))
            .collect::<Vec<_>>();
        run(
            &target,
            SetDomainVfpDeltas {
                domain: parse_domain(domain)?,
                deltas,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_nvapi_power_limits(gpu: &str, limits: Vec<u32>) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetNvapiPowerLimits {
            limits: limits.into_iter().map(Percentage).collect(),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_nvapi_sensor_limits(gpu: &str, limits: Vec<i32>) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetNvapiSensorLimits {
            limits: limits.into_iter().map(|v| Celsius(v).into()).collect(),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_nvapi_power_limits(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetNvapiPowerLimits).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_nvapi_sensor_limits(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetNvapiSensorLimits).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_cooler_levels(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ResetCoolerLevels).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_pstate_clock_offsets(gpu: &str, offsets: Vec<(String, String)>) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    let offsets = offsets
        .into_iter()
        .map(|(pstate, domain)| Ok((parse_pstate(&pstate)?, parse_domain(&domain)?)))
        .collect::<PyResult<Vec<_>>>()?;
    run(&target, ResetPstateClockOffsets { offsets }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_legacy_clocks(gpu: &str, core_mhz: u32, memory_mhz: u32) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetLegacyClocks {
            core_mhz,
            memory_mhz,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_nvapi_pstate_lock(
    py: Python<'_>,
    gpu: &str,
    first_pstate: &str,
    second_pstate: Option<&str>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let first_pstate_own = first_pstate.to_string();
    let second_pstate_own = second_pstate.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let first_pstate: &str = &first_pstate_own;
        let second_pstate: Option<&str> = second_pstate_own.as_deref();

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Both)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetNvapiPstateLock {
                first_pstate: parse_nvml_pstate(first_pstate)?,
                second_pstate: parse_nvml_pstate(second_pstate.unwrap_or(first_pstate))?,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_nvml_pstate_lock(
    py: Python<'_>,
    gpu: &str,
    first_pstate: &str,
    second_pstate: Option<&str>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let first_pstate_own = first_pstate.to_string();
    let second_pstate_own = second_pstate.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let first_pstate: &str = &first_pstate_own;
        let second_pstate: Option<&str> = second_pstate_own.as_deref();

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvml)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetNvmlPstateLock {
                first_pstate: parse_nvml_pstate(first_pstate)?,
                second_pstate: parse_nvml_pstate(second_pstate.unwrap_or(first_pstate))?,
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_voltage_boost(py: Python<'_>, gpu: &str, value: u32) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetVoltageBoost {
                boost: Percentage(value),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_voltage_boost(gpu: &str) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetVoltageBoost {
            boost: Percentage(0),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_auto_boost(gpu: &str, enabled: bool) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, SetAutoBoost { enabled }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_auto_boost_default(gpu: &str, enabled: bool) -> PyResult<()> {
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, SetAutoBoostDefault { enabled }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_api_restriction(gpu: &str, api_type: &str, restricted: bool) -> PyResult<()> {
    let api_type = parse_api_restriction_api(api_type)?;
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvml)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(
        &target,
        SetApiRestriction {
            api_type,
            restricted,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_edid(gpu: &str, display_id: &str, edid_hex: &str) -> PyResult<()> {
    let display_id = parse_display_id(display_id)?;
    let bytes = parse_edid_hex(edid_hex)?;
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, SetEdid { display_id, bytes }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn clear_edid(gpu: &str, display_id: &str) -> PyResult<()> {
    let display_id = parse_display_id(display_id)?;
    let inventory = {
        let mut inventory_cache = lock_inventory_cache();
        inventory_cache.entry(BackendSet::Nvapi)?
    };
    let target = selected_target(&inventory.0, gpu)?;
    run(&target, ClearEdid { display_id }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_legacy_voltage_delta(
    py: Python<'_>,
    gpu: &str,
    uv: i32,
    pstate: Option<&str>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let pstate_own = pstate.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let pstate: Option<&str> = pstate_own.as_deref();

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(
            &target,
            SetPstateBaseVoltage {
                pstate: parse_pstate(pstate.unwrap_or("P0"))?,
                delta_uv: MicrovoltsDelta(uv),
            },
        )
        .map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn set_fan(
    py: Python<'_>,
    gpu: &str,
    backend: &str,
    fan_id: Option<&str>,
    policy: Option<&str>,
    level: Option<u32>,
) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let backend_own = backend.to_string();
    let fan_id_own = fan_id.map(|s| s.to_string());
    let policy_own = policy.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let backend: &str = &backend_own;
        let fan_id: Option<&str> = fan_id_own.as_deref();
        let policy: Option<&str> = policy_own.as_deref();

        let backend = parse_backend(backend)?;
        let fan_id = fan_id.unwrap_or("all");
        let level = level.unwrap_or(60);
        // "auto" is the reset semantic (the GUI/TUI "reset to auto" button).
        // True reset — NOT the SW temperature-continuous policy: on GPUs
        // whose ClientFanPolicies curve table is empty (default fan control
        // is firmware-side), switching to the curve-following mode parks the
        // fan near 0 RPM until the temp spikes.
        let is_reset = policy
            .is_some_and(|p| p.eq_ignore_ascii_case("auto"));
        match backend {
            "nvml" | "nvml-cooler" => {
                let inventory = {
                    let mut inventory_cache = lock_inventory_cache();
                    inventory_cache.entry(BackendSet::Nvml)?
                };
                let target = selected_target(&inventory.0, gpu)?;
                let fan_count = run(&target, QueryFanInfo)
                    .map(|report| report.output.count)
                    .unwrap_or(1);
                let fan_indices = if fan_id == "all" {
                    (0..fan_count).collect::<Vec<_>>()
                } else {
                    vec![fan_id.parse::<u32>().map_err(invalid_value)?]
                };
                if is_reset {
                    // nvmlDeviceSetDefaultFanSpeed_v2 — the documented
                    // "restore default control policy" call. The old path set
                    // the SW curve policy AND wrote 0% duty on top.
                    for fan_index in fan_indices {
                        run(&target, ResetFanSpeed { fan_index }).map_err(to_py_err)?;
                    }
                } else {
                    let policy = parse_nvml_fan_control_policy(policy.unwrap_or("continuous"))
                        .map_err(invalid_value)?;
                    for fan_index in fan_indices {
                        run(
                            &target,
                            SetFanSpeed {
                                fan_index,
                                policy,
                                level,
                            },
                        )
                        .map_err(to_py_err)?;
                    }
                }
            }
            "nvapi" | "nvapi-cooler" => {
                let inventory = {
                    let mut inventory_cache = lock_inventory_cache();
                    inventory_cache.entry(BackendSet::Nvapi)?
                };
                let target = selected_target(&inventory.0, gpu)?;
                if is_reset {
                    // GPUMon's NVAPI fan reset: FanPolicySetControl (NDA
                    // 0x2B2A2A45, struct 0x214AC) — GET the policy block, OR
                    // `1 << curve` into the +0x08 reset bitmask, SET. Unlike
                    // the public RestoreCoolerSettings (rejected with
                    // NOT_SUPPORTED on GPUs without a user-mode cooler table,
                    // e.g. desktop 3060/2070), this private path works there.
                    // Reset curve slot 0 (GPUMon's reset button).
                    run(&target, ResetFanCurve { index: 0 }).map_err(to_py_err)?;
                } else {
                    let cooler_target = match fan_id {
                        "1" => nvoc_core::CoolerTarget::Cooler1,
                        "2" => nvoc_core::CoolerTarget::Cooler2,
                        _ => nvoc_core::CoolerTarget::All,
                    };
                    let mode = match policy.unwrap_or("continuous").to_ascii_lowercase().as_str() {
                        "continuous" => CoolerPolicy::TemperatureContinuous,
                        "manual" => CoolerPolicy::Manual,
                        other => CoolerPolicy::from_str(other).map_err(invalid_value)?,
                    };
                    run(
                        &target,
                        SetCoolerLevels {
                            policy: mode,
                            level,
                            cooler_target,
                        },
                    )
                    .map_err(to_py_err)?;
                }
            }
            _ => {
                return Err(invalid_value(
                    "fan control requires nvapi/nvml cooler backend",
                ));
            }
        }
        Ok(())
    })
}

/// Toggle fan stop / zero-RPM for a fan-curve slot (NVAPI FanArbiterSet NDA
/// 0x44CD3014, struct magic 0x10144). RE'd from GPUMon.exe setFanCurve's
/// tail call. `curve_index` is the slot (default 0); `enable` true = allow
/// the fan to stop at idle, false = always spin.
#[pyfunction]
fn set_fan_stop(
    py: Python<'_>,
    gpu: &str,
    enable: bool,
    curve_index: Option<u8>,
) -> PyResult<Py<PyAny>> {
    let value = py.detach(|| {
        with_target(gpu, "nvapi", |target| {
            run(
                target,
                SetFanStop {
                    curve_index: curve_index.unwrap_or(0),
                    enable,
                },
            )
            .map_err(to_py_err)?;
            Ok(value_object([("applied", Value::from(true))]))
        })
    })?;
    py_value(py, &value)
}

/// Set fan speed by RPM via the private FanCoolerSetControl (NDA 0xEB44E8AA).
/// RE'd from GPUMon.exe setFanSim: RMW the control block, patch enable+level
/// per cooler type. Pass rpm=-1 to disable simulation (return to auto).
#[pyfunction]
fn set_fan_rpm(
    py: Python<'_>,
    gpu: &str,
    rpm: i32,
    cooler_index: Option<u32>,
) -> PyResult<Py<PyAny>> {
    let ci = cooler_index.unwrap_or(0);
    let value = py.detach(|| {
        with_target(gpu, "nvapi", |target| {
            let r = run(
                target,
                SetFanRpm {
                    cooler_index: ci,
                    rpm: if rpm < 0 { None } else { Some(rpm as u32) },
                },
            )
            .map_err(to_py_err)?
            .output;
            let applied = match r.applied_rpm {
                Some(rpm) => Value::from(rpm),
                None => Value::Null,
            };
            Ok(value_object([
                ("applied", Value::from(true)),
                ("cooler_index", Value::from(r.cooler_index)),
                ("cooler_type", Value::from(r.cooler_type)),
                ("min_rpm", Value::from(r.min_rpm)),
                ("max_rpm", Value::from(r.max_rpm)),
                ("applied_rpm", applied),
            ]))
        })
    })?;
    py_value(py, &value)
}

#[pyfunction]
fn reset_core_clocks(py: Python<'_>, gpu: &str, backend: &str) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let backend = parse_backend(backend)?;
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(if backend == "nvml" {
                BackendSet::Nvml
            } else {
                BackendSet::Nvapi
            })?
        };
        let target = selected_target(&inventory.0, gpu)?;
        match backend {
            "nvml" => {
                run(
                    &target,
                    ResetLockedClocks {
                        domain: ClockDomain::Graphics,
                    },
                )
                .map_err(to_py_err)?;
            }
            "nvapi" => {
                run(
                    &target,
                    ResetVfpFrequencyLock {
                        domain: ClockDomain::Graphics,
                    },
                )
                .map_err(to_py_err)?;
                run(
                    &target,
                    ResetPstateClockOffsets {
                        offsets: vec![(PState::P0, ClockDomain::Graphics)],
                    },
                )
                .map_err(to_py_err)?;
            }
            _ => {
                return Err(invalid_value(
                    "clock reset requires backend 'nvapi' or 'nvml'",
                ));
            }
        }
        Ok(())
    })
}

#[pyfunction]
fn reset_mem_clocks(py: Python<'_>, gpu: &str, backend: &str) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;

        let backend = parse_backend(backend)?;
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(if backend == "nvml" {
                BackendSet::Nvml
            } else {
                BackendSet::Nvapi
            })?
        };
        let target = selected_target(&inventory.0, gpu)?;
        match backend {
            "nvml" => {
                run(
                    &target,
                    ResetLockedClocks {
                        domain: ClockDomain::Memory,
                    },
                )
                .map_err(to_py_err)?;
            }
            "nvapi" => {
                run(
                    &target,
                    ResetVfpFrequencyLock {
                        domain: ClockDomain::Memory,
                    },
                )
                .map_err(to_py_err)?;
                run(
                    &target,
                    ResetPstateClockOffsets {
                        offsets: vec![(PState::P0, ClockDomain::Memory)],
                    },
                )
                .map_err(to_py_err)?;
            }
            _ => {
                return Err(invalid_value(
                    "clock reset requires backend 'nvapi' or 'nvml'",
                ));
            }
        }
        Ok(())
    })
}

#[pyfunction]
fn reset_vfp_lock(py: Python<'_>, gpu: &str) -> PyResult<()> {
    py.detach(|| -> PyResult<()> {
        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Nvapi)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        run(&target, ResetVfpLock).map_err(to_py_err)?;
        Ok(())
    })
}

#[pyfunction]
fn reset_all(py: Python<'_>, gpu: &str, domain: Option<&str>) -> PyResult<()> {
    let gpu_own = gpu.to_string();
    let domain_own = domain.map(|s| s.to_string());
    py.detach(|| -> PyResult<()> {
        let gpu: &str = &gpu_own;
        let domain: Option<&str> = domain_own.as_deref();

        let inventory = {
            let mut inventory_cache = lock_inventory_cache();
            inventory_cache.entry(BackendSet::Both)?
        };
        let target = selected_target(&inventory.0, gpu)?;
        if target.has_nvapi() {
            let vfp_domain = match domain.unwrap_or("all").to_ascii_lowercase().as_str() {
                "all" => VfpResetDomain::All,
                "core" | "graphics" => VfpResetDomain::Core,
                "memory" | "mem" => VfpResetDomain::Memory,
                other => return Err(invalid_value(format!("invalid reset domain {other:?}"))),
            };
            run(
                &target,
                SetVoltageBoost {
                    boost: Percentage(0),
                },
            )
            .map_err(to_py_err)?;
            run(&target, ResetNvapiSensorLimits).map_err(to_py_err)?;
            run(&target, ResetNvapiPowerLimits).map_err(to_py_err)?;
            run(&target, ResetCoolerLevels).map_err(to_py_err)?;
            run(&target, ResetVfpDeltas { domain: vfp_domain }).map_err(to_py_err)?;
            run(&target, ResetVfpLock).map_err(to_py_err)?;
            run(&target, ResetPstateBaseVoltages).map_err(to_py_err)?;
            run(
                &target,
                ResetPstateClockOffsets {
                    offsets: vec![
                        (PState::P0, ClockDomain::Graphics),
                        (PState::P0, ClockDomain::Memory),
                    ],
                },
            )
            .map_err(to_py_err)?;
        }
        if target.has_nvml() {
            let _ = run(
                &target,
                ResetLockedClocks {
                    domain: ClockDomain::Graphics,
                },
            );
            let _ = run(
                &target,
                ResetLockedClocks {
                    domain: ClockDomain::Memory,
                },
            );
        }
        Ok(())
    })
}

#[pymodule]
fn _native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(discover_gpus, m)?)?;
    m.add_function(wrap_pyfunction!(force_wake, m)?)?;
    m.add_function(wrap_pyfunction!(query_info, m)?)?;
    m.add_function(wrap_pyfunction!(query_status, m)?)?;
    m.add_function(wrap_pyfunction!(query_settings, m)?)?;
    m.add_function(wrap_pyfunction!(query_supported_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(query_power_limits, m)?)?;
    m.add_function(wrap_pyfunction!(query_pstates, m)?)?;
    m.add_function(wrap_pyfunction!(query_fan_info, m)?)?;
    m.add_function(wrap_pyfunction!(query_temperature_thresholds, m)?)?;
    m.add_function(wrap_pyfunction!(query_throttle_reasons, m)?)?;
    m.add_function(wrap_pyfunction!(query_legacy_overvolt_ranges, m)?)?;
    m.add_function(wrap_pyfunction!(query_pstate_base_voltage, m)?)?;
    m.add_function(wrap_pyfunction!(query_voltage_boost, m)?)?;
    m.add_function(wrap_pyfunction!(query_auto_boost, m)?)?;
    m.add_function(wrap_pyfunction!(query_api_restriction, m)?)?;
    m.add_function(wrap_pyfunction!(list_displays, m)?)?;
    m.add_function(wrap_pyfunction!(query_edid, m)?)?;
    m.add_function(wrap_pyfunction!(query_clock_offset, m)?)?;
    m.add_function(wrap_pyfunction!(query_domain_vfp_points, m)?)?;
    m.add_function(wrap_pyfunction!(query_vfp_point_voltage, m)?)?;
    m.add_function(wrap_pyfunction!(query_legacy_p0_core_max_voltage_delta, m)?)?;
    m.add_function(wrap_pyfunction!(query_tdp_temp_limits, m)?)?;
    m.add_function(wrap_pyfunction!(probe_voltage_limits, m)?)?;
    m.add_function(wrap_pyfunction!(check_voltage_frequency, m)?)?;
    m.add_function(wrap_pyfunction!(set_clock_offset, m)?)?;
    m.add_function(wrap_pyfunction!(set_power_limit, m)?)?;
    m.add_function(wrap_pyfunction!(set_thermal_limit, m)?)?;
    m.add_function(wrap_pyfunction!(set_dynamic_boost, m)?)?;
    m.add_function(wrap_pyfunction!(query_tgp_watt_range, m)?)?;
    m.add_function(wrap_pyfunction!(set_tgp_watt, m)?)?;
    m.add_function(wrap_pyfunction!(reset_tgp_watt, m)?)?;
    m.add_function(wrap_pyfunction!(query_dnotifier, m)?)?;
    m.add_function(wrap_pyfunction!(query_target_temp_policies, m)?)?;
    m.add_function(wrap_pyfunction!(set_dnotifier, m)?)?;
    m.add_function(wrap_pyfunction!(query_volt_rails, m)?)?;
    m.add_function(wrap_pyfunction!(set_volt_rail_offset, m)?)?;
    m.add_function(wrap_pyfunction!(set_volt_rail_target, m)?)?;
    m.add_function(wrap_pyfunction!(set_perf_freq_cap, m)?)?;
    m.add_function(wrap_pyfunction!(query_clk_domains, m)?)?;
    m.add_function(wrap_pyfunction!(query_clk_domain_freq, m)?)?;
    m.add_function(wrap_pyfunction!(query_clk_vf_points, m)?)?;
    m.add_function(wrap_pyfunction!(set_clk_domain_offset, m)?)?;
    m.add_function(wrap_pyfunction!(set_vfp_point_private, m)?)?;
    m.add_function(wrap_pyfunction!(set_target_temp, m)?)?;
    m.add_function(wrap_pyfunction!(set_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(set_locked_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_locked_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_fan_speed, m)?)?;
    m.add_function(wrap_pyfunction!(set_pstate_base_voltage, m)?)?;
    m.add_function(wrap_pyfunction!(reset_pstate_base_voltages, m)?)?;
    m.add_function(wrap_pyfunction!(set_pstate_clock_offset, m)?)?;
    m.add_function(wrap_pyfunction!(sync_memory_pstate_as_p0, m)?)?;
    m.add_function(wrap_pyfunction!(set_cooler_levels, m)?)?;
    m.add_function(wrap_pyfunction!(set_vfp_frequency_lock, m)?)?;
    m.add_function(wrap_pyfunction!(reset_vfp_frequency_lock, m)?)?;
    m.add_function(wrap_pyfunction!(set_vfp_voltage_lock, m)?)?;
    m.add_function(wrap_pyfunction!(reset_vfp_deltas, m)?)?;
    m.add_function(wrap_pyfunction!(reset_vfp_lock, m)?)?;
    m.add_function(wrap_pyfunction!(set_vfp_point_delta, m)?)?;
    m.add_function(wrap_pyfunction!(set_vfp_range_delta, m)?)?;
    m.add_function(wrap_pyfunction!(set_domain_vfp_deltas, m)?)?;
    m.add_function(wrap_pyfunction!(set_nvapi_power_limits, m)?)?;
    m.add_function(wrap_pyfunction!(set_nvapi_sensor_limits, m)?)?;
    m.add_function(wrap_pyfunction!(reset_nvapi_power_limits, m)?)?;
    m.add_function(wrap_pyfunction!(reset_nvapi_sensor_limits, m)?)?;
    m.add_function(wrap_pyfunction!(reset_cooler_levels, m)?)?;
    m.add_function(wrap_pyfunction!(reset_pstate_clock_offsets, m)?)?;
    m.add_function(wrap_pyfunction!(set_legacy_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(set_nvapi_pstate_lock, m)?)?;
    m.add_function(wrap_pyfunction!(set_nvml_pstate_lock, m)?)?;
    m.add_function(wrap_pyfunction!(set_voltage_boost, m)?)?;
    m.add_function(wrap_pyfunction!(reset_voltage_boost, m)?)?;
    m.add_function(wrap_pyfunction!(set_auto_boost, m)?)?;
    m.add_function(wrap_pyfunction!(set_auto_boost_default, m)?)?;
    m.add_function(wrap_pyfunction!(set_api_restriction, m)?)?;
    m.add_function(wrap_pyfunction!(set_edid, m)?)?;
    m.add_function(wrap_pyfunction!(clear_edid, m)?)?;
    m.add_function(wrap_pyfunction!(set_legacy_voltage_delta, m)?)?;
    m.add_function(wrap_pyfunction!(set_fan, m)?)?;
    m.add_function(wrap_pyfunction!(set_fan_stop, m)?)?;
    m.add_function(wrap_pyfunction!(set_fan_rpm, m)?)?;
    m.add_function(wrap_pyfunction!(reset_core_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_mem_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_vfp_lock, m)?)?;
    m.add_function(wrap_pyfunction!(reset_all, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_parsing() {
        assert_eq!(parse_backends("both").unwrap(), BackendSet::Both);
        assert_eq!(parse_backends("nvapi").unwrap(), BackendSet::Nvapi);
        assert_eq!(parse_backends("nvml").unwrap(), BackendSet::Nvml);
        assert!(parse_backends("cuda").is_err());
    }

    #[test]
    fn domain_parsing_accepts_ui_aliases() {
        assert_eq!(parse_domain("core").unwrap(), ClockDomain::Graphics);
        assert_eq!(parse_domain("graphics").unwrap(), ClockDomain::Graphics);
        assert_eq!(parse_domain("mem").unwrap(), ClockDomain::Memory);
        assert_eq!(parse_domain("memory").unwrap(), ClockDomain::Memory);
        assert!(parse_domain("video").is_err());
    }

    #[test]
    fn pstate_parsing() {
        assert_eq!(parse_pstate("p0").unwrap(), PState::P0);
        assert_eq!(parse_nvml_pstate("P0").unwrap(), PerformanceState::Zero);
        assert!(parse_pstate("P16").is_err());
        assert!(parse_nvml_pstate("P16").is_err());
    }

    #[test]
    fn numeric_display_parser_handles_units() {
        assert_eq!(first_number_in_display("912.5 mV").unwrap(), 912.5);
        assert_eq!(first_number_in_display("N/A"), None);
        assert_eq!(first_number_in_display("-12 MHz").unwrap(), -12.0);
    }
}
