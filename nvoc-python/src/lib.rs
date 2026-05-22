use nvapi_hi::{
    Celsius, ClockDomain, CoolerPolicy, KilohertzDelta, MicrovoltsDelta, PState, Percentage,
};
use nvml_wrapper::enum_wrappers::device::PerformanceState;
use nvoc_core::{
    BackendSet, ConvertEnum, GpuTarget, QueryFanInfo, QueryGpuInfo, QueryGpuSettings,
    QueryGpuStatus, QueryLegacyCoreOvervoltRanges, QueryLegacyP0CoreMaxVoltageDelta,
    QueryPowerLimits, QueryPstates, QuerySupportedApplicationsClocks, QueryTdpTempLimits,
    QueryTemperatureThresholds, QueryVfpPointVoltage, ResetApplicationsClocks, ResetCoolerLevels,
    ResetFanSpeed, ResetLockedClocks, ResetNvapiPowerLimits, ResetNvapiSensorLimits,
    ResetPstateBaseVoltages, ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock,
    ResetVfpLock, SetApplicationsClocks, SetClockOffset, SetCoolerLevels, SetDomainVfpDeltas,
    SetFanSpeed, SetLegacyClocks, SetLockedClocks, SetNvapiPowerLimits, SetNvapiPstateLock,
    SetNvapiSensorLimits, SetNvmlPstateLock, SetPowerLimit, SetPstateBaseVoltage,
    SetPstateClockOffset, SetTemperatureLimit, SetVfpFrequencyLock, SetVfpPointDelta,
    SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, VfpResetDomain, discover_targets,
    nvml_pstate_to_str, parse_nvml_fan_control_policy, run, try_parse_nvml_pstate,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use serde_json::{Map, Number, Value};

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

fn with_target<F>(gpu: &str, backends: &str, f: F) -> PyResultValue
where
    F: FnOnce(&GpuTarget<'_>) -> PyResultValue,
{
    let inventory = discover_targets(parse_backends(backends)?).map_err(to_py_err)?;
    let target = selected_target(&inventory, gpu)?;
    f(&target)
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

fn normalize_info(target: &GpuTarget<'_>) -> PyResultValue {
    let info = run(target, QueryGpuInfo).map_err(to_py_err)?.output;
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
    if let Some((_sensor, temp)) = status.sensors.first() {
        map.insert("temperature_c".into(), f64_value(temp.0 as f64));
    }
    if let Some((_channel, power)) = status.power.iter().next()
        && let Some(watts) = first_number_in_display(power)
    {
        map.insert("power_w".into(), f64_value(watts));
    }
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
    let (min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp, _curve) =
        run(target, QueryTdpTempLimits).map_err(to_py_err)?.output;
    Ok(value_object([
        ("min_tdp", percent_value(min_tdp)),
        ("default_tdp", percent_value(default_tdp)),
        ("max_tdp", percent_value(max_tdp)),
        ("min_temp", u64_value(min_temp.0 as u64)),
        ("default_temp", u64_value(default_temp.0 as u64)),
        ("max_temp", u64_value(max_temp.0 as u64)),
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
    let value = run(target, QueryVfpPointVoltage { point }).map_err(to_py_err)?;
    Ok(value_object([
        ("precise", bool_value(true)),
        ("matched_point", Value::Null),
        ("microvolts", u64_value(value.output.0 as u64)),
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

fn target_inventory(backends: BackendSet) -> PyResult<nvoc_core::TargetInventory> {
    discover_targets(backends).map_err(to_py_err)
}

#[pyfunction]
fn discover_gpus(py: Python<'_>, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let inventory = target_inventory(parse_backends(backends.unwrap_or("both"))?)?;
    let mut items = Vec::new();
    for target in inventory.targets() {
        let mut item = Map::new();
        item.insert("index".into(), u64_value(target.index as u64));
        item.insert("gpu_id".into(), u64_value(target.id.0 as u64));
        item.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
        item.insert("backend_nvapi".into(), bool_value(target.nvapi.is_some()));
        item.insert("backend_nvml".into(), bool_value(target.nvml.is_some()));
        if let Ok(info) = run(&target, QueryGpuInfo).map(|report| report.output) {
            item.insert("name".into(), text(info.name));
            item.insert("codename".into(), text(info.codename));
            item.insert("arch".into(), text(info.arch));
        }
        items.push(Value::Object(item));
    }
    py_value(py, &Value::Array(items))
}

#[pyfunction]
fn query_info(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, backends.unwrap_or("both"), normalize_info)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_status(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, backends.unwrap_or("both"), normalize_status)?;
    py_value(py, &value)
}

#[pyfunction]
fn query_settings(py: Python<'_>, gpu: &str, backends: Option<&str>) -> PyResult<Py<PyAny>> {
    let value = with_target(gpu, backends.unwrap_or("both"), normalize_settings)?;
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
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Both
    })?;
    let target = selected_target(&inventory, gpu)?;
    let value = normalize_query_clock_offset(&target, domain, pstate)?;
    py_value(py, &value)
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
    gpu: &str,
    backend: &str,
    domain: &str,
    value: i32,
    pstate: Option<&str>,
) -> PyResult<()> {
    let backend = parse_backend(backend)?;
    let domain = parse_domain(domain)?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, gpu)?;
    match backend {
        "nvml" => {
            let pstate = parse_nvml_pstate(pstate.unwrap_or("P0"))?;
            run(
                &target,
                SetClockOffset {
                    domain,
                    pstate,
                    mhz: value,
                },
            )
            .map_err(to_py_err)?;
        }
        "nvapi" => {
            let pstate = parse_pstate(pstate.unwrap_or("P0"))?;
            run(
                &target,
                SetPstateClockOffset {
                    pstate,
                    domain,
                    delta: KilohertzDelta(value.saturating_mul(1000)),
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
}

#[pyfunction]
fn set_power_limit(gpu: &str, backend: &str, value: u32) -> PyResult<()> {
    let backend = parse_backend(backend)?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn set_thermal_limit(gpu: &str, celsius: i32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Both)?;
    let target = selected_target(&inventory, gpu)?;
    if target.nvapi.is_some() {
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
}

#[pyfunction]
fn set_applications_clocks(gpu: &str, memory_mhz: u32, graphics_mhz: u32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
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
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetApplicationsClocks).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_locked_clocks(
    gpu: &str,
    backend: &str,
    domain: &str,
    min_mhz: u32,
    max_mhz: u32,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn reset_locked_clocks(gpu: &str, backend: &str, domain: &str) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
    let domain = parse_domain(domain)?;
    if parse_backend(backend)? != "nvml" {
        return Err(invalid_value("locked clocks currently use the NVML path"));
    }
    run(&target, ResetLockedClocks { domain }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_fan_speed(gpu: &str, fan_index: u32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetFanSpeed { fan_index }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_pstate_base_voltage(gpu: &str, pstate: &str, delta_uv: i32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetPstateBaseVoltages).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_pstate_clock_offset(gpu: &str, pstate: &str, domain: &str, delta: i32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
fn set_cooler_levels(
    gpu: &str,
    policy: &str,
    level: u32,
    target_name: Option<&str>,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
    gpu: &str,
    domain: &str,
    upper_khz: i32,
    lower_khz: Option<i32>,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn reset_vfp_frequency_lock(gpu: &str, domain: &str) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(
        &target,
        ResetVfpFrequencyLock {
            domain: parse_domain(domain)?,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_vfp_voltage_lock(
    gpu: &str,
    point: Option<usize>,
    voltage_uv: Option<i32>,
    feedback: Option<bool>,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    let voltage_target = if let Some(point) = point {
        nvoc_core::NvapiLockedVoltageTarget::Point(point)
    } else if let Some(voltage_uv) = voltage_uv {
        nvoc_core::NvapiLockedVoltageTarget::Voltage(nvapi_hi::Microvolts(voltage_uv.max(0) as u32))
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
}

#[pyfunction]
fn reset_vfp_deltas(gpu: &str, domain: Option<&str>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    let domain = match domain.unwrap_or("all") {
        "all" => VfpResetDomain::All,
        "core" => VfpResetDomain::Core,
        "memory" => VfpResetDomain::Memory,
        other => return Err(invalid_value(format!("invalid VFP reset domain {other:?}"))),
    };
    run(&target, ResetVfpDeltas { domain }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_vfp_point_delta(gpu: &str, point: usize, delta: i32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
fn set_vfp_range_delta(gpu: &str, start: usize, end: usize, delta: i32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn set_domain_vfp_deltas(gpu: &str, domain: &str, deltas: Vec<(usize, i32)>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn set_nvapi_power_limits(gpu: &str, limits: Vec<u32>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetNvapiPowerLimits).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_nvapi_sensor_limits(gpu: &str) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetNvapiSensorLimits).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_cooler_levels(gpu: &str) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetCoolerLevels).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_pstate_clock_offsets(gpu: &str, offsets: Vec<(String, String)>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    let offsets = offsets
        .into_iter()
        .map(|(pstate, domain)| Ok((parse_pstate(&pstate)?, parse_domain(&domain)?)))
        .collect::<PyResult<Vec<_>>>()?;
    run(&target, ResetPstateClockOffsets { offsets }).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_legacy_clocks(gpu: &str, core_mhz: u32, memory_mhz: u32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
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
    gpu: &str,
    first_pstate: &str,
    second_pstate: Option<&str>,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Both)?;
    let target = selected_target(&inventory, gpu)?;
    run(
        &target,
        SetNvapiPstateLock {
            first_pstate: parse_nvml_pstate(first_pstate)?,
            second_pstate: parse_nvml_pstate(second_pstate.unwrap_or(first_pstate))?,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_nvml_pstate_lock(
    gpu: &str,
    first_pstate: &str,
    second_pstate: Option<&str>,
) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvml)?;
    let target = selected_target(&inventory, gpu)?;
    run(
        &target,
        SetNvmlPstateLock {
            first_pstate: parse_nvml_pstate(first_pstate)?,
            second_pstate: parse_nvml_pstate(second_pstate.unwrap_or(first_pstate))?,
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_voltage_boost(gpu: &str, value: u32) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(
        &target,
        SetVoltageBoost {
            boost: Percentage(value),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_legacy_voltage_delta(gpu: &str, uv: i32, pstate: Option<&str>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(
        &target,
        SetPstateBaseVoltage {
            pstate: parse_pstate(pstate.unwrap_or("P0"))?,
            delta_uv: MicrovoltsDelta(uv),
        },
    )
    .map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn set_fan(
    gpu: &str,
    backend: &str,
    fan_id: Option<&str>,
    policy: Option<&str>,
    level: Option<u32>,
) -> PyResult<()> {
    let backend = parse_backend(backend)?;
    let fan_id = fan_id.unwrap_or("all");
    let level = level.unwrap_or(60);
    match backend {
        "nvml" | "nvml-cooler" => {
            let inventory = target_inventory(BackendSet::Nvml)?;
            let target = selected_target(&inventory, gpu)?;
            let policy = parse_nvml_fan_control_policy(policy.unwrap_or("continuous"))
                .map_err(invalid_value)?;
            let fan_count = run(&target, QueryFanInfo)
                .map(|report| report.output.count)
                .unwrap_or(1);
            let fan_indices = if fan_id == "all" {
                (0..fan_count).collect::<Vec<_>>()
            } else {
                vec![fan_id.parse::<u32>().map_err(invalid_value)?]
            };
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
        "nvapi" | "nvapi-cooler" => {
            let inventory = target_inventory(BackendSet::Nvapi)?;
            let target = selected_target(&inventory, gpu)?;
            let cooler_target = match fan_id {
                "1" => nvoc_core::CoolerTarget::Cooler1,
                "2" => nvoc_core::CoolerTarget::Cooler2,
                _ => nvoc_core::CoolerTarget::All,
            };
            let mode = match policy.unwrap_or("continuous").to_ascii_lowercase().as_str() {
                "auto" | "continuous" => CoolerPolicy::TemperatureContinuous,
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
        _ => {
            return Err(invalid_value(
                "fan control requires nvapi/nvml cooler backend",
            ));
        }
    }
    Ok(())
}

#[pyfunction]
fn reset_core_clocks(gpu: &str, backend: &str) -> PyResult<()> {
    let backend = parse_backend(backend)?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn reset_mem_clocks(gpu: &str, backend: &str) -> PyResult<()> {
    let backend = parse_backend(backend)?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, gpu)?;
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
}

#[pyfunction]
fn reset_vfp_lock(gpu: &str) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, gpu)?;
    run(&target, ResetVfpLock).map_err(to_py_err)?;
    Ok(())
}

#[pyfunction]
fn reset_all(gpu: &str, domain: Option<&str>) -> PyResult<()> {
    let inventory = target_inventory(BackendSet::Both)?;
    let target = selected_target(&inventory, gpu)?;
    if target.nvapi.is_some() {
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
    if target.nvml.is_some() {
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
}

#[pymodule]
fn _native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(discover_gpus, m)?)?;
    m.add_function(wrap_pyfunction!(query_info, m)?)?;
    m.add_function(wrap_pyfunction!(query_status, m)?)?;
    m.add_function(wrap_pyfunction!(query_settings, m)?)?;
    m.add_function(wrap_pyfunction!(query_supported_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(query_clock_offset, m)?)?;
    m.add_function(wrap_pyfunction!(query_vfp_point_voltage, m)?)?;
    m.add_function(wrap_pyfunction!(query_legacy_p0_core_max_voltage_delta, m)?)?;
    m.add_function(wrap_pyfunction!(query_tdp_temp_limits, m)?)?;
    m.add_function(wrap_pyfunction!(probe_voltage_limits, m)?)?;
    m.add_function(wrap_pyfunction!(check_voltage_frequency, m)?)?;
    m.add_function(wrap_pyfunction!(set_clock_offset, m)?)?;
    m.add_function(wrap_pyfunction!(set_power_limit, m)?)?;
    m.add_function(wrap_pyfunction!(set_thermal_limit, m)?)?;
    m.add_function(wrap_pyfunction!(set_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_applications_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(set_locked_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_locked_clocks, m)?)?;
    m.add_function(wrap_pyfunction!(reset_fan_speed, m)?)?;
    m.add_function(wrap_pyfunction!(set_pstate_base_voltage, m)?)?;
    m.add_function(wrap_pyfunction!(reset_pstate_base_voltages, m)?)?;
    m.add_function(wrap_pyfunction!(set_pstate_clock_offset, m)?)?;
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
    m.add_function(wrap_pyfunction!(set_legacy_voltage_delta, m)?)?;
    m.add_function(wrap_pyfunction!(set_fan, m)?)?;
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
