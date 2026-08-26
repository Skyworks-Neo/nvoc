use crate::Execution;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn execution_to_json(execution: &Execution) -> Value {
    json!({
        "function": execution.function,
        "backend": execution.backend,
        "ok": !execution.has_errors(),
        "warnings": execution.warnings,
        "results": execution.results.iter().map(|result| {
            json!({
                "gpu_id": result.gpu_id,
                "backend": result.backend,
                "ok": result.ok,
                "output": result.output,
                "error": result.error,
            })
        }).collect::<Vec<_>>(),
    })
}

pub(super) fn format_human(execution: &Execution) -> String {
    let mut lines = Vec::new();
    lines.push(nvoc_cli_common::color::stylize_title(&format!(
        "{} via {}",
        execution.function, execution.backend
    )));

    for warning in &execution.warnings {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("Warning: {warning}"),
            true,
        ));
    }

    for result in &execution.results {
        let gpu = result
            .gpu_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        if result.ok {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("GPU {gpu} [{}]: ok", result.backend),
                false,
            ));
            if let Some(output) = &result.output {
                lines.extend(format_human_output(execution.function, output));
            }
        } else {
            let error = result.error.as_deref().unwrap_or("unknown error");
            lines.push(nvoc_cli_common::color::stylize(
                &format!("GPU {gpu} [{}]: error: {error}", result.backend),
                true,
            ));
        }
    }

    lines.join("\n")
}

fn format_human_output(function: &str, output: &Value) -> Vec<String> {
    match function {
        "get-settings" => format_get_settings_output(output),
        "get-public-vftable" => format_vfp_output(output),
        "get-pstate-freq-range" => format_object_array(
            output,
            &[
                ("pstate", "P-State"),
                ("min_core_mhz", "Core Min"),
                ("max_core_mhz", "Core Max"),
                ("min_memory_mhz", "Memory Min"),
                ("max_memory_mhz", "Memory Max"),
            ],
        ),
        "get-supported-legacy-application-freq" => format_object_array(
            output,
            &[("memory_mhz", "Memory"), ("graphics_mhz", "Graphics")],
        ),
        "get-temp-thresholds" => format_temperature_thresholds_output(output),
        "get-legacy-temp-sensor" => format_object_array(
            output,
            &[
                ("target", "Target"),
                ("controller", "Controller"),
                ("current_c", "Current"),
                ("min_c", "Min"),
                ("max_c", "Max"),
            ],
        ),
        "get-power-mode" => {
            let supported = output.get("supported").and_then(Value::as_bool);
            let active = output.get("active").and_then(Value::as_str).unwrap_or("?");
            match supported {
                Some(true) => vec![format!("  Power Mode: {active}")],
                _ => vec![format!("  Power Mode: N/A (unsupported on this GPU)")],
            }
        }
        "set-power-mode" => vec![format!(
            "  Power Mode set: {}",
            output
                .get("power_mode")
                .and_then(Value::as_str)
                .unwrap_or("?")
        )],
        // "get-dynamic-boost" withdrawn 2026-08-26: 0xC80068A1 reads PCF
        // platform status, not the PPAB enable readback (probe_pcf_dynamic_boost)
        "get-pstate-lock" => format_pstate_native_output(output),
        "get-throttle-reasons" => format_throttle_reasons_output(output),
        "get-legacy-overvolt-ranges" => format_object_array(
            output,
            &[
                ("pstate", "P-State"),
                ("min_uv", "Min"),
                ("current_uv", "Current"),
                ("max_uv", "Max"),
            ],
        ),
        _ => format_value_block(output, 1),
    }
}

fn format_get_settings_output(output: &Value) -> Vec<String> {
    let Some(object) = output.as_object() else {
        return format_value_block(output, 1);
    };

    let mut lines = Vec::new();
    for (key, value) in sorted_object_entries(object) {
        if key == "vfp" {
            lines.extend(format_vfp_delta_summary(1, value));
            continue;
        }

        match value {
            Value::Object(child) if object_is_compact_scalar_group(child) => {
                lines.push(format_scalar_object_line(1, key, child, key));
            }
            Value::Object(child) if object_is_measurement_map(key, child) => {
                lines.push(format_measurement_map_line(1, key, child));
            }
            Value::Object(_) | Value::Array(_) => {
                lines.push(format!(
                    "{}{}",
                    indent_spaces(1),
                    nvoc_cli_common::color::stylize_title(&format_label(key))
                ));
                lines.extend(format_value_block_with_context(value, 2, key));
            }
            _ => lines.push(format_field_line(1, key, value)),
        }
    }
    lines
}

fn format_vfp_output(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(object) = output.as_object() {
        for key in ["domain", "indexed", "infer_missing_default"] {
            if let Some(value) = object.get(key) {
                lines.push(format_field_line(1, key, value));
            }
        }

        if let Some(points) = object.get("points").and_then(Value::as_array) {
            lines.push(format!(
                "  {}",
                nvoc_cli_common::color::stylize_title("V-F Points")
            ));
            for point in points {
                let index = field_text(point, "index");
                let voltage = field_text(point, "voltage_mv");
                let frequency = field_text(point, "frequency_mhz");
                let delta = field_text(point, "delta_mhz");
                let default_frequency = field_text(point, "default_frequency_mhz");
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    #{index}: {voltage}, {frequency}, delta {delta}, default {default_frequency}"
                    ),
                    false,
                ));
            }
        }
    } else {
        lines.extend(format_value_block(output, 1));
    }
    lines
}

fn format_object_array(output: &Value, fields: &[(&str, &str)]) -> Vec<String> {
    match output.as_array() {
        Some(items) if items.is_empty() => {
            vec![format!(
                "  {}",
                nvoc_cli_common::color::stylize("No entries", false)
            )]
        }
        Some(items) => items
            .iter()
            .map(|item| {
                let parts = fields
                    .iter()
                    .filter_map(|(key, label)| {
                        item.get(*key).map(|value| {
                            format!(
                                "{} {}",
                                nvoc_cli_common::color::stylize_title(label),
                                nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                format!("  {}", parts.join(" | "))
            })
            .collect(),
        None => format_value_block(output, 1),
    }
}

/// Format the combined throttle-reasons + violation-status output.
///
/// Prints the instantaneous per-reason active snapshot, then appends the
/// driver's cumulative per-policy violation times (the "how long was each
/// modality limiting" breakdown), mirroring the historical `status` output.
/// Render the temp-thresholds array. Two row shapes:
/// - NVML: `Shutdown | Limit 94 C` (name + celsius only).
/// - NVAPI target-temp (温度墙): `Threshold 2 (TargetTemp) | Current 87 C | Min 75 / Max 87 C`
///   — the auto-discovered wall slot is tagged `(TargetTemp)`, and the VBIOS
///   min/max range is appended when GetInfo exposed it. An entry is treated as
///   NVAPI when it carries a `policy_index` field.
fn format_temperature_thresholds_output(output: &Value) -> Vec<String> {
    let Some(items) = output.as_array() else {
        return format_value_block(output, 1);
    };
    if items.is_empty() {
        return vec![format!(
            "  {}",
            nvoc_cli_common::color::stylize("No entries", false)
        )];
    }
    items
        .iter()
        .map(|item| {
            let obj = item.as_object();
            let name = obj
                .and_then(|o| o.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let render_temp = |key: &str| -> String {
                obj.and_then(|o| o.get(key))
                    .map(|v| format_scalar(key, v))
                    .unwrap_or_else(|| "---".into())
            };
            let label = nvoc_cli_common::color::stylize_title("Threshold");
            let row = if obj.is_some_and(|o| o.contains_key("policy_index")) {
                // NVAPI row: Current + Min/Max range. format_scalar already
                // appends the ` C` unit (key ends in _celsius/_c), so don't
                // add it again here.
                let cur = nvoc_cli_common::color::stylize(
                    &format!("Current {}", render_temp("celsius")),
                    false,
                );
                let min = obj
                    .and_then(|o| o.get("min_c"))
                    .map(|v| format_scalar("min_c", v));
                let max = obj
                    .and_then(|o| o.get("max_c"))
                    .map(|v| format_scalar("max_c", v));
                match (min, max) {
                    (Some(mn), Some(mx)) => {
                        let range = nvoc_cli_common::color::stylize(
                            &format!("Min {} / Max {}", mn, mx),
                            false,
                        );
                        format!("{} {} | {} | {}", label, name, cur, range)
                    }
                    _ => format!("{} {} | {}", label, name, cur),
                }
            } else {
                // NVML row: single Limit value. celsius is a u32 (no _c key),
                // so render it bare and add the unit once.
                let raw = obj
                    .and_then(|o| o.get("celsius"))
                    .map(|v| format_scalar("celsius", v))
                    .unwrap_or_else(|| "N/A".into());
                let limit = nvoc_cli_common::color::stylize(&format!("Limit {}", raw), false);
                format!("{} {} | {}", label, name, limit)
            };
            format!("  {}", row)
        })
        .collect()
}

fn format_pstate_native_output(output: &Value) -> Vec<String> {
    use nvoc_cli_common::color::stylize_title;
    let mut lines = Vec::new();
    let object = match output.as_object() {
        Some(o) => o,
        None => return lines,
    };

    // Locked pstates summary line (e.g. "Locked: P0, P3").
    if let Some(locked) = object.get("locked_pstates").and_then(Value::as_array) {
        let labels: Vec<String> = locked
            .iter()
            .filter_map(|v| v.as_u64().map(|n| format!("P{n}")))
            .collect();
        if labels.is_empty() {
            lines.push(format!("  {}", stylize_title("No locked P-States")));
        } else {
            lines.push(format!(
                "  {}: {}",
                stylize_title("Locked"),
                labels.join(", ")
            ));
        }
    }

    let Some(pstates) = object.get("pstates").and_then(Value::as_array) else {
        return lines;
    };
    lines.push(format!("  {}", stylize_title("P-States")));
    for entry in pstates {
        let pstate = entry.get("pstate").and_then(Value::as_str).unwrap_or("P?");
        let locked = entry
            .get("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let header = if locked {
            format!("{pstate} (locked)")
        } else {
            pstate.to_string()
        };
        lines.push(format!("    {}", stylize_title(&header)));
        // Per-domain frequency range. Keys are max_<domain>_mhz / min_<domain>_mhz.
        // Emit in a fixed, NVML-like order.
        for (dom, label) in [
            ("graphics", "Graphics"),
            ("memory", "Memory"),
            ("video", "Video"),
            ("host", "Host"),
        ] {
            let max = entry.get(format!("max_{dom}_mhz")).and_then(Value::as_f64);
            let min = entry.get(format!("min_{dom}_mhz")).and_then(Value::as_f64);
            if let (Some(max), Some(min)) = (max, min) {
                if max == 0.0 && min == 0.0 {
                    continue;
                }
                let values = format!("Max {} MHz, Min {} MHz", trim_float(max), trim_float(min));
                lines.push(format!(
                    "      {}: {}",
                    stylize_title(label),
                    nvoc_cli_common::color::stylize(&values, false)
                ));
            }
        }
    }
    lines
}

/// Render an f64 without a trailing ".0" when it is a whole number.
fn trim_float(v: f64) -> String {
    if (v.fract() == 0.0) && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn format_throttle_reasons_output(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();

    let reasons = output.get("reasons").unwrap_or(output);
    lines.extend(format_object_array(
        reasons,
        &[("name", "Reason"), ("active", "Active")],
    ));

    if let Some(violation) = output.get("violation").and_then(Value::as_object) {
        let entries = violation
            .get("entries")
            .and_then(Value::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(|entry| {
                        let name = entry.get("name")?.as_str()?;
                        let secs = entry.get("seconds")?.as_f64()?;
                        Some((name.to_string(), secs))
                    })
                    .filter(|(_, secs)| *secs > 0.0)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let since = violation.get("since").and_then(Value::as_str);
        let header = match since {
            Some(since) => format!("Violation Status (since {since})"),
            None => "Violation Status".to_string(),
        };
        lines.push(nvoc_cli_common::color::stylize_title(&header));

        if entries.is_empty() {
            lines.push(format!(
                "  {}",
                nvoc_cli_common::color::stylize("none", false)
            ));
        } else {
            for (name, secs) in &entries {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("  {:<10} {:>11.1}s", name, secs),
                    false,
                ));
            }
        }
    }

    lines
}

fn format_value_block(value: &Value, indent: usize) -> Vec<String> {
    format_value_block_with_context(value, indent, "")
}

fn format_value_block_with_context(value: &Value, indent: usize, context: &str) -> Vec<String> {
    match value {
        Value::Object(object) => {
            let compact_groups = compact_range_groups(object);
            let mut compacted_keys = compact_groups
                .iter()
                .flat_map(|group| group.keys.iter().copied())
                .collect::<Vec<_>>();
            let mut lines = compact_groups
                .iter()
                .map(|group| format_compact_group_line(indent, group))
                .collect::<Vec<_>>();

            for (key, value) in sorted_object_entries(object) {
                if compacted_keys.contains(&key.as_str()) {
                    continue;
                }
                // Skip empty pstate-limit fields to keep output terse:
                //   frequency_delta = null (rendered "N/A"),
                //   voltage / voltage_domain when both bounds are 0 or "Undefined".
                if is_empty_pstate_limit_field(key, value) {
                    continue;
                }

                match value {
                    Value::Object(child) if key == "ids" && is_pci_identifiers(child) => {
                        lines.extend(format_pci_identifiers(indent, key, child));
                    }
                    Value::Object(child) if key == "utilization" => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_utilization_entries(indent + 1, child));
                    }
                    // `power` is the NVAPI power-topology map
                    // (`NvAPI_GPU_ClientPowerTopologyGetStatus`): keys are channel
                    // names (TotalGpuPower / NormalizedTotalPower / …) and values
                    // are 0–100 plain percentages of the board power budget. Append
                    // a `%` unit to each numeric value (it is dimensionless but is
                    // conventionally reported as a percentage).
                    Value::Object(child) if is_power_topology_map(child) => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_power_entries(indent + 1, child));
                    }
                    // `perf` carries two raw NVAPI words from PerfPoliciesGetStatus:
                    // `limits` (a PerfFlags bitmask of throttling reasons) and
                    // `unknown` (a driver load-level indicator, not an error).
                    // Both are unreadable as raw ints, so decode them here.
                    Value::Object(child) if key == "perf" && child.contains_key("limits") => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_perf_block(indent + 1, child));
                    }
                    // `performance_decrease` is the serde form of nvapi-rs's
                    // `PerformanceDecreaseReason` bitflags struct (`{"bits": N}`);
                    // decode the bitmask to friendly reason text.
                    Value::Object(_) if key == "performance_decrease" => {
                        lines.extend(format_performance_decrease(indent, value));
                    }
                    Value::Object(child) if key == "memory" && child.contains_key("dedicated") => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_memory_entries(indent + 1, child));
                    }
                    Value::Array(items) if key == "sensors" => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_sensors_array(indent + 1, items));
                    }
                    Value::Object(child) if object_is_compact_scalar_group(child) => {
                        lines.push(format_scalar_object_line(
                            indent,
                            key,
                            child,
                            &join_context(context, key),
                        ));
                    }
                    // P0 voltage bounds (get-status `p0_voltage`): render as a
                    // multi-line section like Memory, not the generic
                    // comma-separated measurement-map line. Must precede the
                    // measurement-map arm — `p0_voltage` matches it (key contains
                    // "voltage"). Values are µV shown in mV, and the redundant
                    // `UV` per-field label suffix is dropped (the mV unit already
                    // carries the dimension).
                    Value::Object(child) if key == "p0_voltage" => {
                        lines.extend(format_p0_voltage_block(indent, child));
                    }
                    Value::Object(child) if object_is_measurement_map(key, child) => {
                        lines.push(format_measurement_map_line(indent, key, child));
                    }
                    Value::Array(items) if key == "points" && array_is_pff_points(items) => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title("Points")
                        ));
                        lines.extend(format_pff_points(indent + 1, items));
                    }
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_value_block_with_context(
                            value,
                            indent + 1,
                            &join_context(context, key),
                        ));
                    }
                    _ => lines.push(format_leaf_line(indent, key, value, context)),
                }
            }

            compacted_keys.clear();
            lines
        }
        Value::Array(items) => {
            if items.is_empty() {
                return vec![format!(
                    "{}{}",
                    indent_spaces(indent),
                    nvoc_cli_common::color::stylize("No entries", false)
                )];
            }

            items
                .iter()
                .flat_map(|item| match item {
                    Value::Object(_) | Value::Array(_) => {
                        format_value_block_with_context(item, indent, context)
                    }
                    _ => vec![format!(
                        "{}- {}",
                        indent_spaces(indent),
                        nvoc_cli_common::color::stylize(&format_scalar("", item), false)
                    )],
                })
                .collect()
        }
        _ => vec![format!(
            "{}{}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize(&format_scalar("", value), false)
        )],
    }
}

fn join_context(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn sorted_object_entries(object: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries = object.iter().collect::<Vec<_>>();
    if entries.iter().all(|(key, _)| key.parse::<i64>().is_ok()) {
        entries.sort_by_key(|(key, _)| key.parse::<i64>().unwrap_or_default());
    }
    entries
}

struct CompactGroup<'a> {
    label_key: String,
    keys: Vec<&'a str>,
    values: Vec<(&'static str, &'a str, &'a Value)>,
}

/// True when a pstate-limit field carries no useful data and should be hidden
/// from human output (keeps get-info pstate tables terse). Applies to:
///   - `frequency_delta` = null (no offset set; otherwise rendered "N/A")
///   - `voltage` object whose max AND min are both 0/null
///   - `voltage_domain` = "Undefined"
fn is_empty_pstate_limit_field(key: &str, value: &Value) -> bool {
    match key {
        "frequency_delta" => value.is_null(),
        "voltage_domain" => value
            .as_str()
            .map(|s| s.eq_ignore_ascii_case("Undefined"))
            .unwrap_or(false),
        "voltage" => value
            .as_object()
            .map(|o| {
                let max = o.get("max").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let min = o.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                max == 0.0 && min == 0.0
            })
            .unwrap_or(false),
        _ => false,
    }
}

fn compact_range_groups<'a>(object: &'a serde_json::Map<String, Value>) -> Vec<CompactGroup<'a>> {
    let mut groups: BTreeMap<String, CompactGroup<'a>> = BTreeMap::new();

    for (key, value) in object {
        if !is_scalar_value(value) {
            continue;
        }
        let Some((group_key, part_label)) = split_compact_range_key(key) else {
            continue;
        };
        let group = groups.entry(group_key.to_string()).or_insert(CompactGroup {
            label_key: strip_trailing_unit_key(group_key).to_string(),
            keys: Vec::new(),
            values: Vec::new(),
        });
        group.keys.push(key);
        group.values.push((part_label, key, value));
    }

    groups
        .into_values()
        .filter(|group| group.values.len() >= 2)
        .collect()
}

fn split_compact_range_key(key: &str) -> Option<(&str, &'static str)> {
    for (prefix, label) in [
        ("max_", "Max"),
        ("current_", "Current"),
        ("default_", "Default"),
        ("min_", "Min"),
    ] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return Some((rest, label));
        }
    }
    None
}

fn object_is_compact_scalar_group(object: &serde_json::Map<String, Value>) -> bool {
    let mut compact_count = 0;
    for (key, value) in object {
        if !is_scalar_value(value) {
            return false;
        }
        if compact_scalar_object_label(key).is_some() {
            compact_count += 1;
        }
    }
    compact_count >= 2 && compact_count == object.len()
}

fn object_is_measurement_map(key: &str, object: &serde_json::Map<String, Value>) -> bool {
    let context = key.to_ascii_lowercase();
    let is_measurement = context.contains("frequency")
        || context.contains("clock")
        || (context.contains("voltage") && !context.contains("domain"));
    is_measurement && object.len() >= 2 && object.values().all(is_scalar_value)
}

fn array_is_pff_points(items: &[Value]) -> bool {
    !items.is_empty()
        && items.iter().all(|item| {
            let Some(object) = item.as_object() else {
                return false;
            };
            object.len() == 2
                && object.get("x").and_then(Value::as_f64).is_some()
                && object.get("y").and_then(Value::as_f64).is_some()
        })
}

fn compact_scalar_object_label(key: &str) -> Option<&'static str> {
    match key {
        "max" | "maximum" => Some("Max"),
        "current" | "value" => Some("Current"),
        "default" => Some("Default"),
        "min" | "minimum" => Some("Min"),
        _ => None,
    }
}

fn format_compact_group_line(indent: usize, group: &CompactGroup<'_>) -> String {
    let values = ordered_compact_values(&group.values)
        .into_iter()
        .map(|(label, key, value)| {
            format!(
                "{label} {}",
                format_contextual_scalar(&group.label_key, key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(&group.label_key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

fn format_scalar_object_line(
    indent: usize,
    key: &str,
    object: &serde_json::Map<String, Value>,
    context: &str,
) -> String {
    let values = ordered_scalar_object_values(object)
        .into_iter()
        .map(|(label, field_key, value)| {
            format!(
                "{label} {}",
                format_contextual_scalar(context, field_key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

fn format_measurement_map_line(
    indent: usize,
    key: &str,
    object: &serde_json::Map<String, Value>,
) -> String {
    let values = object
        .iter()
        .map(|(field_key, value)| {
            format!(
                "{} {}",
                format_label(field_key),
                format_contextual_scalar(key, field_key, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&values, false)
    )
}

/// Render the P0 voltage-bounds block (`p0_voltage` from get-status) as a
/// multi-line section — a header on its own line followed by one indented
/// `Label: N mV` line per bound — mirroring the Memory block rather than the
/// generic comma-separated `format_measurement_map_line`.
///
/// Values are stored in microvolts (`*_uV`) but shown in millivolts. The
/// redundant `UV` label suffix that `format_label` would append (from the
/// `_uV` key suffix) is stripped, since the `mV` unit already carries the
/// dimension — otherwise each line read "Current UV 900 mV".
fn format_p0_voltage_block(indent: usize, object: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut lines = vec![format!(
        "{}{}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title("P0 Voltage Limit")
    )];
    // Fixed logical order: current, then the wall hierarchy (target → effective
    // → VBIOS → VRM-max), then the remaining offset headroom and min-hold floor.
    // Values are stored under `<stem>_uV` keys.
    const ORDER: [&str; 7] = [
        "current_uV",
        "target_wall_uV",
        "effective_wall_uV",
        "vbios_wall_uV",
        "vrm_max_wall_uV",
        "offset_ceiling_uV",
        "min_hold_uV",
    ];
    for (field_key, value) in ORDER
        .iter()
        .copied()
        .filter_map(|k| object.get(k).map(|v| (k, v)))
    {
        // Label from the key minus the `_uV` suffix (e.g. "effective_wall").
        // `format_contextual_scalar` still receives the original `*_uV` key so
        // the "voltage" context maps µV -> mV.
        let label_key = field_key.strip_suffix("_uV").unwrap_or(field_key);
        lines.push(format!(
            "{}{}: {}",
            indent_spaces(indent + 1),
            nvoc_cli_common::color::stylize_title(&format_label(label_key)),
            nvoc_cli_common::color::stylize(
                &format_contextual_scalar("p0_voltage", field_key, value),
                false,
            ),
        ));
    }
    lines
}

fn format_pff_points(indent: usize, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let object = item.as_object()?;
            let raw_temp = object.get("x")?.as_f64()?;
            let raw_frequency = object.get("y")?.as_f64()?;
            Some(nvoc_cli_common::color::stylize(
                &format!(
                    "{}#{}: Temperature {} -> Frequency {}",
                    indent_spaces(indent),
                    index,
                    format_measurement(raw_temp / 256.0, "C"),
                    format_measurement(raw_frequency / 1000.0, "MHz")
                ),
                false,
            ))
        })
        .collect()
}

fn format_vfp_delta_summary(indent: usize, value: &Value) -> Vec<String> {
    let Some(object) = value.as_object() else {
        return format_value_block_with_context(value, indent, "vfp");
    };

    let mut lines = vec![format!(
        "{}{}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title("VFP Deltas")
    )];
    for domain in ["graphics", "memory"] {
        let Some(points) = object.get(domain).and_then(Value::as_object) else {
            continue;
        };
        lines.push(format_vfp_delta_domain_summary(indent + 1, domain, points));
    }
    lines
}

fn format_vfp_delta_domain_summary(
    indent: usize,
    domain: &str,
    points: &serde_json::Map<String, Value>,
) -> String {
    let entries = sorted_object_entries(points);
    let changed = entries
        .iter()
        .filter_map(|(point, value)| {
            let delta = value.as_f64()?;
            (delta != 0.0).then_some((point.as_str(), delta))
        })
        .collect::<Vec<_>>();

    let summary = if entries.is_empty() {
        "no points".to_string()
    } else if changed.is_empty() {
        format!("{} points, all 0 MHz", entries.len())
    } else {
        let preview = changed
            .iter()
            .take(12)
            .map(|(point, delta)| format!("#{point} {}", format_measurement(delta / 1000.0, "MHz")))
            .collect::<Vec<_>>()
            .join(", ");
        if changed.len() > 12 {
            format!(
                "{} points, {} changed: {preview}, ...",
                entries.len(),
                changed.len()
            )
        } else {
            format!(
                "{} points, {} changed: {preview}",
                entries.len(),
                changed.len()
            )
        }
    };

    nvoc_cli_common::color::stylize(
        &format!(
            "{}{}: {summary}",
            indent_spaces(indent),
            format_label(domain)
        ),
        false,
    )
}

fn ordered_compact_values<'a>(
    values: &[(&'static str, &'a str, &'a Value)],
) -> Vec<(&'static str, &'a str, &'a Value)> {
    ["Max", "Current", "Default", "Min"]
        .iter()
        .flat_map(|wanted| {
            values
                .iter()
                .filter(move |(label, _, _)| label == wanted)
                .copied()
        })
        .collect()
}

fn ordered_scalar_object_values(
    object: &serde_json::Map<String, Value>,
) -> Vec<(&'static str, &str, &Value)> {
    [
        "max", "maximum", "current", "value", "default", "min", "minimum",
    ]
    .iter()
    .filter_map(|key| {
        object.get_key_value(*key).and_then(|(field_key, value)| {
            compact_scalar_object_label(field_key).map(|label| (label, field_key.as_str(), value))
        })
    })
    .collect()
}

fn is_scalar_value(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

fn format_field_line(indent: usize, key: &str, value: &Value) -> String {
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
    )
}

/// Leaf scalar line that is aware of its dotted context path (e.g.
/// `bus.pci_express.lanes`, `driver_model.value`, `physical_frame_buffer`). Used for
/// `get-info` fields whose formatting depends on the surrounding object, not just the
/// key suffix. Falls back to the plain field line for non-numeric / unmatched values.
fn format_leaf_line(indent: usize, key: &str, value: &Value, context: &str) -> String {
    let rendered = if value.is_number() {
        format_contextual_scalar(context, key, value)
    } else {
        format_scalar(key, value)
    };
    format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key)),
        nvoc_cli_common::color::stylize(&rendered, false)
    )
}

/// Render the per-domain utilization map with friendlier labels (FrameBuffer is
/// NVAPI's name for the memory-controller domain) and a `%` unit on each value.
fn format_utilization_entries(
    indent: usize,
    object: &serde_json::Map<String, Value>,
) -> Vec<String> {
    sorted_object_entries(object)
        .iter()
        .map(|(key, value)| {
            let label = match key.as_str() {
                "FrameBuffer" => "Memory Controller",
                "VideoEngine" => "Video Engine",
                "BusInterface" => "Bus Interface",
                other => other,
            };
            let rendered = match value {
                Value::Number(number) => format!("{}%", number),
                _ => format_scalar(key, value),
            };
            format!(
                "{}{}: {}",
                indent_spaces(indent),
                nvoc_cli_common::color::stylize_title(label),
                nvoc_cli_common::color::stylize(&rendered, false)
            )
        })
        .collect()
}

/// NVAPI PerfFlags bit → friendly reason name. Bit semantics mirror
/// `nvapi-rs/sys/src/gpu/power.rs` (`NV_GPU_PERF_FLAGS` + its display table):
/// 1 Power, 2 Temperature, 4 Reliability Voltage, 8 Operating Voltage,
/// 16 No Load, 32 Unknown32. Kept in ascending bit order so the decoded
/// list reads consistently regardless of which reasons are active.
const PERF_LIMIT_BITS: &[(u64, &str)] = &[
    (1, "Power"),
    (2, "Temperature"),
    (4, "Reliability Voltage"),
    (8, "Operating Voltage"),
    (16, "No Load"),
    (32, "Unknown32"),
];

/// Extract a bitmask from a JSON value that may be either a raw integer
/// (`N`, as emitted by pynvoc) or a bitflags object (`{"bits": N}`, as serde
/// renders nvapi-rs `nvbits!` structs on the CLI's native get-status path).
fn bitmask_from_value(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::Object(obj) => obj.get("bits").and_then(Value::as_u64),
        _ => None,
    }
}

/// Is this object the NVAPI power-topology channel map? Its keys are channel
/// names (`TotalGpuPower`, `NormalizedTotalPower`, …) with scalar values; any
/// all-scalar object with at least one known power-topology channel key matches.
fn is_power_topology_map(object: &serde_json::Map<String, Value>) -> bool {
    if !object.values().all(is_scalar_value) {
        return false;
    }
    object.keys().any(|k| {
        matches!(
            k.as_str(),
            "TotalGpuPower" | "NormalizedTotalPower" | "TotalBoardPower"
        )
    })
}

/// Render the power-topology channel map. Each value is a 0–100 plain
/// percentage of the board power budget, so a `%` unit is appended.
fn format_power_entries(indent: usize, object: &serde_json::Map<String, Value>) -> Vec<String> {
    sorted_object_entries(object)
        .iter()
        .map(|(key, value)| {
            let rendered = match value {
                Value::Number(number) => format!("{}%", number),
                _ => format_scalar(key, value),
            };
            format!(
                "{}{}: {}",
                indent_spaces(indent),
                nvoc_cli_common::color::stylize_title(&format_label(key)),
                nvoc_cli_common::color::stylize(&rendered, false)
            )
        })
        .collect()
}

/// Decode the NVAPI perf-policy status (`perf` object from PerfPoliciesGetStatus).
/// `limits` is a PerfFlags bitmask of active throttling reasons; `unknown` is the
/// driver's load-status level (1 = on load, 3 = low clocks, 7 = idle — see
/// `nvapi-rs/sys/src/gpu/power.rs` field comment), not an error/unknown value.
fn format_perf_block(indent: usize, object: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut lines = Vec::new();

    let limits_raw = object
        .get("limits")
        .and_then(bitmask_from_value)
        .unwrap_or(0);
    let reasons: Vec<&str> = PERF_LIMIT_BITS
        .iter()
        .filter(|(bit, _)| limits_raw & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    let limits_text = if reasons.is_empty() {
        "None".to_string()
    } else {
        reasons.join(", ")
    };
    lines.push(format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title("Limits"),
        nvoc_cli_common::color::stylize(&limits_text, false)
    ));

    if let Some(unknown) = object.get("unknown").and_then(Value::as_u64) {
        let load_text = match unknown {
            1 => "Load".to_string(),
            3 => "Low Clock".to_string(),
            7 => "Idle".to_string(),
            other => format!("{other} (raw)"),
        };
        lines.push(format!(
            "{}{}: {}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize_title("Load Level"),
            nvoc_cli_common::color::stylize(&load_text, false)
        ));
    }

    lines
}

/// PerformanceDecreaseReason bits (NVAPI_GPU_PERF_DECREASE, `nvapi-rs/sys/src/
/// gpu/mod.rs`): 1 Thermal Protection, 2 Power Control, 4 AC-Battery,
/// 8 API Triggered, 16 Insufficient Power. 0 = none.
const PERF_DECREASE_BITS: &[(u64, &str)] = &[
    (1, "Thermal Protection"),
    (2, "Power Control"),
    (4, "AC-Battery"),
    (8, "API Triggered"),
    (16, "Insufficient Power"),
];

/// Decode the `performance_decrease` value. On the CLI's native get-status path
/// serde renders the nvapi-rs `PerformanceDecreaseReason` bitflags struct as
/// `{"bits": N}`; decode the bitmask into reason names (0 -> "None").
fn format_performance_decrease(indent: usize, value: &Value) -> Vec<String> {
    let bits = bitmask_from_value(value).unwrap_or(0);
    let reasons: Vec<&str> = PERF_DECREASE_BITS
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    let text = if reasons.is_empty() {
        "None".to_string()
    } else {
        reasons.join(", ")
    };
    vec![format!(
        "{}{}: {}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title("Performance Decrease"),
        nvoc_cli_common::color::stylize(&text, false)
    )]
}

/// Render the VRAM info map. Size fields are kibibytes -> shown in MB;
/// `dedicated_evictions` is a plain count (no unit).
fn format_memory_entries(indent: usize, object: &serde_json::Map<String, Value>) -> Vec<String> {
    sorted_object_entries(object)
        .iter()
        .map(|(key, value)| {
            let rendered = if key.as_str() == "dedicated_evictions" {
                format_scalar(key, value)
            } else if let Some(kib) = value.as_f64() {
                format_measurement(kib / 1024.0, "MB")
            } else {
                format_scalar(key, value)
            };
            format!(
                "{}{}: {}",
                indent_spaces(indent),
                nvoc_cli_common::color::stylize_title(&format_label(key)),
                nvoc_cli_common::color::stylize(&rendered, false)
            )
        })
        .collect()
}

/// Render the thermal `sensors` array. Each entry is a `[descriptor, temp]`
/// tuple (sub-degree celsius). The `target` field is dropped (it adds no useful
/// information beyond the sensor name) and each temperature gets a `C` unit.
/// Sensor ranges are temperature limits, also shown with `C`; an all-zero range
/// (the undocumented sensors carry no limit data) is omitted as uninformative.
fn format_sensors_array(indent: usize, items: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for sensor in items {
        let Some(tuple) = sensor.as_array() else {
            lines.extend(format_value_block_with_context(sensor, indent, "sensors"));
            continue;
        };
        let descriptor = tuple.first().and_then(Value::as_object);
        let temp = tuple.get(1);

        if let Some(descriptor) = descriptor {
            lines.extend(format_sensor_descriptor(indent, descriptor));
        }
        if let Some(temp) = temp {
            let rendered = match temp {
                Value::Number(number) => format!("{} C", number),
                _ => format_scalar("", temp),
            };
            lines.push(format!(
                "{}- {}",
                indent_spaces(indent),
                nvoc_cli_common::color::stylize(&rendered, false)
            ));
        }
    }
    lines
}

/// Render a sensor descriptor (everything except `target`) as indented fields.
/// `channel_num` is emitted verbatim; `channel_type` gets a human tag
/// (GPU_AVG/GPU_MAX/BOARD/MEMORY/PWR_SUPPLY/unclassified); `range` is a
/// temperature limit shown as `Max N C, Min N C`, skipped when it is `{0, 0}`
/// (the RTSS GetInfo record may report no limits for a channel, so a zero
/// range carries no information).
fn format_sensor_descriptor(
    indent: usize,
    descriptor: &serde_json::Map<String, Value>,
) -> Vec<String> {
    let field = |key: &str, value: &Value| {
        format!(
            "{}{}: {}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize_title(&format_label(key)),
            nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
        )
    };

    let mut lines = Vec::new();
    if let Some(range) = descriptor.get("range").and_then(Value::as_object) {
        let max = range.get("max").and_then(Value::as_f64);
        let min = range.get("min").and_then(Value::as_f64);
        // Skip a {0, 0} range (no limit data for undocumented sensors).
        let is_zero_range = matches!(
            (max, min),
            (Some(0.0), Some(0.0)) | (Some(0.0), None) | (None, Some(0.0))
        );
        if !is_zero_range
            && let Some(max) = max
            && let Some(min) = min
        {
            lines.push(format!(
                "{}{}: Max {}, Min {}",
                indent_spaces(indent),
                nvoc_cli_common::color::stylize_title("Range"),
                nvoc_cli_common::color::stylize(&format_measurement(max, "C"), false),
                nvoc_cli_common::color::stylize(&format_measurement(min, "C"), false)
            ));
        }
    }
    if let Some(chan) = descriptor.get("channel_num") {
        lines.push(field("channel_num", chan));
    }
    // Cross-reference to the raw sibling channel: RTSS exposes two channels
    // per physical sensor — `(dev, 0)` raw and `(dev, 1)` with `offset_hw`
    // already applied by the driver. Annotate the `(dev, 1)` half.
    if let Some(sibling) = descriptor.get("same_sensor_as").and_then(Value::as_i64) {
        lines.push(format!(
            "{}{}: ch[{}] with offset_hw",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize_title("Same Sensor As"),
            nvoc_cli_common::color::stylize(&sibling.to_string(), false)
        ));
    }
    // RTSS ThermChannel metadata (research fields from GetInfo). channel_type
    // gets a human tag; offsets/scaling are shown raw (semantics undocumented).
    if let Some(ch_type) = descriptor.get("channel_type").and_then(Value::as_i64) {
        let tag = match ch_type {
            0 => " (GPU_AVG)",
            1 => " (GPU_MAX)",
            2 => " (BOARD)",
            3 => " (MEMORY)",
            4 => " (PWR_SUPPLY)",
            255 => " (unclassified)",
            _ => "",
        };
        lines.push(format!(
            "{}{}: {}{}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize_title("Channel Type"),
            nvoc_cli_common::color::stylize(&ch_type.to_string(), false),
            tag
        ));
    }
    for key in ["offset_sw", "offset_hw", "scaling"] {
        if let Some(val) = descriptor.get(key) {
            lines.push(field(key, val));
        }
    }
    lines
}

/// True when an object looks like the NVAPI PCI identifier block
/// (`device_id`, `subsystem_id`, `ext_device_id`, `revision_id`).
fn is_pci_identifiers(object: &serde_json::Map<String, Value>) -> bool {
    object.contains_key("device_id")
        && object.contains_key("subsystem_id")
        && (object.contains_key("ext_device_id") || object.contains_key("revision_id"))
}

/// Known PCI sub-vendor ids (subset of `NV_GPU_VENDOR`) for human-friendly labeling.
/// Matches the value in the low 16 bits of `subsystem_id`.
fn pci_vendor_name(subvendor: u16) -> Option<&'static str> {
    match subvendor {
        0x10de => Some("NVIDIA"),
        0x1043 => Some("ASUS"),
        0x1458 => Some("Gigabyte"),
        0x1462 => Some("MSI"),
        0x10b0 => Some("Gainward"),
        0x107d => Some("Leadtek"),
        0x1048 => Some("Elsa"),
        0x19da => Some("Zotac"),
        0x196e => Some("PNY"),
        _ => None,
    }
}

/// Render the NVAPI PCI identifier block as Vendor/Device/Subvendor/Subdevice/Revision
/// in hex. NVAPI packs: `device_id = (product << 16) | vendor`; for NVIDIA (vendor
/// `0x10de`) `subsystem_id = (subproduct << 16) | subvendor`. `ext_device_id` is the
/// real device id (it differs from the high half of `device_id` on some boards).
fn format_pci_identifiers(
    indent: usize,
    key: &str,
    object: &serde_json::Map<String, Value>,
) -> Vec<String> {
    let get = |k: &str| object.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;

    let device_id = get("device_id");
    let subsystem_id = get("subsystem_id");
    let ext_device_id = get("ext_device_id");
    let revision_id = get("revision_id");

    // device_id low 16 = vendor, high 16 = (NVIDIA's internal) product.
    let vendor = device_id as u16;
    let product = (device_id >> 16) as u16;
    // ext_device_id is the canonical PCI device id when present and non-zero.
    let device = if ext_device_id != 0 {
        ext_device_id as u16
    } else {
        product
    };
    // subsystem_id low 16 = subvendor, high 16 = subdevice (NVIDIA packing).
    let subvendor = subsystem_id as u16;
    let subdevice = (subsystem_id >> 16) as u16;

    let mut lines = vec![format!(
        "{}{}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(&format_label(key))
    )];

    let mut row = |label: &str, value: String| {
        lines.push(format!(
            "{}{}: {}",
            indent_spaces(indent + 1),
            nvoc_cli_common::color::stylize_title(label),
            nvoc_cli_common::color::stylize(&value, false)
        ));
    };

    let vendor_str = match pci_vendor_name(vendor) {
        Some(name) => format!("0x{:04X} ({})", vendor, name),
        None => format!("0x{:04X}", vendor),
    };
    row("Vendor", vendor_str);
    row("Device", format!("0x{:04X}", device));

    if subsystem_id != 0 {
        let subvendor_str = match pci_vendor_name(subvendor) {
            Some(name) => format!("0x{:04X} ({})", subvendor, name),
            None => format!("0x{:04X}", subvendor),
        };
        row("Subvendor", subvendor_str);
        row("Subdevice", format!("0x{:04X}", subdevice));
    }
    if revision_id != 0 {
        // Labeled "CHIP Revision" to distinguish from `Arch.Revision` (NV_GPU_CHIP_REVISION)
        // elsewhere in get-info.
        row("CHIP Revision", format!("0x{:02X}", revision_id as u8));
    }
    lines
}

fn field_text(object: &Value, key: &str) -> String {
    object
        .get(key)
        .map(|value| format_scalar(key, value))
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_scalar(key: &str, value: &Value) -> String {
    match value {
        Value::Null => "N/A".to_string(),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        Value::Number(number) => {
            let rendered = number.to_string();
            format_with_unit(key, &rendered)
        }
        Value::String(text) => {
            if text.is_empty() {
                "N/A".to_string()
            } else {
                format_with_unit(key, text)
            }
        }
        Value::Array(items) => items
            .iter()
            .map(|item| format_scalar(key, item))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => "see details".to_string(),
    }
}

fn format_contextual_scalar(context_key: &str, value_key: &str, value: &Value) -> String {
    let Some(number) = value.as_f64() else {
        return format_scalar(value_key, value);
    };
    let context = context_key.to_ascii_lowercase();
    if context.contains("frequency")
        || context.contains("clock")
        || (context.contains("vfp") && context.contains("range"))
    {
        return format_measurement(number / 1000.0, "MHz");
    }
    // Per-rail power (PowerMonitor `power_rails_w`): values are already in
    // watts (mW ÷ 1000 at the source); just append the unit.
    if context.contains("power_rails") {
        return format_measurement(number, "W");
    }
    if context.contains("voltage") && !context.contains("domain") {
        return format_measurement(number / 1000.0, "mV");
    }
    // PCI Express link width (e.g. x8 / x16). The JSON key is `lanes`. (serde renders
    // the bus variant as `pciexpress`, so match that form here.)
    if context.contains("pciexpress") && value_key == "lanes" {
        return format!("x{}", number as i64);
    }
    // RAM bus width is in bits (top-level GpuInfo field, so check the value key too).
    if context.contains("bus_width") || value_key.contains("bus_width") {
        return format!("{} bit", number as i64);
    }
    // Frame-buffer sizes are kibibytes -> megabytes (top-level fields too).
    if context.contains("frame_buffer") || value_key.contains("frame_buffer") {
        return format_measurement(number / 1024.0, "MB");
    }
    // Driver-model value is a packed WDDM version word -> show hex + decoded version.
    // major = (value >> 12) & 0xf; minor = (value >> 8) & 0xf when major != 2.
    if context.contains("driver_model") || value_key == "driver_model" {
        let word = number as u32;
        let major = ((word >> 12) & 0xf) as u8;
        let minor = if major == 2 {
            0
        } else {
            ((word >> 8) & 0xf) as u8
        };
        return format!("0x{:08X} (WDDM {}.{})", word, major, minor);
    }
    // Temperature sensor range bounds (get-info descriptor path): report in degrees C.
    if context.contains("sensors") && context.contains("range") {
        return format_measurement(number, "C");
    }
    // Compute-capability flags: decode the NV_GPU_COMPUTE_CAPS bitmask into names.
    // Mirrors the bit definitions in nvapi-rs/sys/src/gpu/mod.rs.
    if context.contains("compute_capabilities") && value_key == "flags" {
        return format_compute_caps(number as u32);
    }
    format_scalar(value_key, value)
}

/// Decode the `NV_GPU_COMPUTE_CAPS` bitmask (from `NvAPI_GPU_GetComputeCapabilities`)
/// into `<dec> (0x<hex>: NAME | NAME | ...)`. Unknown set bits are folded into a
/// trailing `0x...` so no information is lost. Mirrors the bit layout documented on
/// `NV_GPU_COMPUTE_CAPS` in `nvapi-rs/sys/src/gpu/mod.rs`.
///
/// NOTE: despite the "compute caps" name, the bits are PhysX / compute-software /
/// framebuffer oriented (reversed from handler @0x1801ABAD0), NOT SR-IOV / virt / large-BAR.
fn format_compute_caps(word: u32) -> String {
    const KNOWN: &[(u32, &str)] = &[
        (0x1, "BASE_COMPUTE"),
        (0x2, "COMPUTE_CAPABLE"),
        (0x4, "BOARD_DB_MATCH"),
        (0x100, "PHYSX_INSTALLED"),
        (0x200, "VRAM_GE_256MB"),
        (0x400, "PHYSX_GPU_SELECTED"),
    ];
    let mut names: Vec<&str> = Vec::new();
    let mut known_mask = 0u32;
    for &(bit, name) in KNOWN {
        if word & bit == bit {
            names.push(name);
        }
        known_mask |= bit;
    }
    let unknown = word & !known_mask;
    let suffix = if unknown != 0 {
        format!(" | 0x{:X}", unknown)
    } else {
        String::new()
    };
    if names.is_empty() && unknown == 0 {
        "0 (none)".to_string()
    } else {
        format!("{} (0x{:X}: {}{})", word, word, names.join(" | "), suffix)
    }
}

fn format_measurement(value: f64, unit: &str) -> String {
    let rendered = if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };
    format!("{rendered} {unit}")
}

fn format_with_unit(key: &str, rendered: &str) -> String {
    if key.ends_with("_mhz") {
        format!("{rendered} MHz")
    } else if key.ends_with("_khz") {
        format!("{rendered} kHz")
    } else if key.ends_with("_mv") {
        format!("{rendered} mV")
    } else if key.ends_with("_uv") {
        format!("{rendered} uV")
    } else if key.ends_with("_watt") || key.ends_with("_w") {
        format!("{rendered} W")
    } else if key.ends_with("_percent") || key == "percent" {
        format!("{rendered}%")
    } else if key.ends_with("_c") || key == "celsius" {
        format!("{rendered} C")
    } else if key.ends_with("_mibps") {
        format!("{rendered} MiB/s")
    } else if key == "voltage" {
        // Core voltage is reported in microvolts.
        format!("{rendered} uV")
    } else if key == "pcie_lanes" {
        // Link width (downstream lane count), e.g. x16.
        format!("x{rendered}")
    } else {
        rendered.to_string()
    }
}

fn strip_trailing_unit_key(key: &str) -> &str {
    for suffix in ["_mhz", "_khz", "_mv", "_uv", "_watt", "_percent", "_c"] {
        if let Some(stripped) = key.strip_suffix(suffix) {
            return stripped;
        }
    }
    key
}

fn format_label(key: &str) -> String {
    key.split('_')
        .map(|word| match word {
            "gpu" => "GPU".to_string(),
            "id" => "ID".to_string(),
            "pci" => "PCI".to_string(),
            "nvapi" => "NVAPI".to_string(),
            "nvml" => "NVML".to_string(),
            "tdp" => "TDP".to_string(),
            "vfp" => "VFP".to_string(),
            "uv" => "uV".to_string(),
            "mv" => "mV".to_string(),
            "mhz" => "MHz".to_string(),
            "khz" => "kHz".to_string(),
            "c" => "C".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn indent_spaces(indent: usize) -> String {
    "  ".repeat(indent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{COMMANDS, Command, TargetResult};
    use serde_json::json;

    #[test]
    fn human_output_formats_objects_without_json_dump() {
        nvoc_cli_common::color::init(true);
        let execution = Execution {
            function: "get-power-watt",
            backend: "nvml".to_string(),
            warnings: Vec::new(),
            results: vec![TargetResult {
                gpu_id: Some(7),
                backend: "nvml",
                ok: true,
                output: Some(json!({
                    "min_watt": 100,
                    "current_watt": 250,
                    "max_watt": 350,
                })),
                error: None,
            }],
        };

        let rendered = format_human(&execution);

        assert!(rendered.contains("Watt: Max 350 W, Current 250 W, Min 100 W"));
        assert!(!rendered.contains('{'));
        assert!(!rendered.contains("\"current_watt\""));
    }

    #[test]
    fn get_info_formats_pci_ids_lanes_buffers_and_ranges() {
        nvoc_cli_common::color::init(true);
        // Mirrors the GpuInfo shape from `get-info`. device_id 0x28E010DE packs
        // vendor 0x10DE (low) + product; ext_device_id 0x28E0 is the canonical device;
        // subsystem_id 0x20BD1043 packs subdevice 0x20BD (high) + subvendor 0x1043 ASUS.
        // driver_model.value 0x3200 decodes to WDDM 3.2; frame buffers are KiB.
        // (serde renders the `Bus::PciExpress` variant tag as `pciexpress`.)
        let output = json!({
            "bus": {
                "bus": {
                    "pciexpress": {
                        "ids": {
                            "device_id": 0x28E010DE_u32,
                            "ext_device_id": 0x28E0_u32,
                            "revision_id": 0xA1_u32,
                            "subsystem_id": 0x20BD1043_u32
                        },
                        "lanes": 8
                    }
                }
            },
            "driver_model": { "value": 0x3200_u32 },
            "ram_bus_width": 128,
            "physical_frame_buffer": 8384000,
            "virtual_frame_buffer": 8384512,
            "compute_capabilities": { "flags": 1795 },
            "sensors": [
                {
                    "name": "Core",
                    "range": { "max": 139, "min": -35 }
                }
            ]
        });

        let rendered = format_value_block(&output, 0).join("\n");

        // PCI ids split into hex vendor/device/subvendor/subdevice/revision.
        assert!(rendered.contains("Vendor: 0x10DE (NVIDIA)"));
        assert!(rendered.contains("Device: 0x28E0"));
        assert!(rendered.contains("Subvendor: 0x1043 (ASUS)"));
        assert!(rendered.contains("Subdevice: 0x20BD"));
        // PCI revision labeled "CHIP Revision" to disambiguate from Arch.Revision.
        assert!(rendered.contains("CHIP Revision: 0xA1"));
        assert!(!rendered.contains("685773022"));
        // Link width prefixed with `x`.
        assert!(rendered.contains("Lanes: x8"));
        // RAM bus width in bits.
        assert!(rendered.contains("Ram Bus Width: 128 bit"));
        // Frame buffers KiB -> MB.
        assert!(rendered.contains("Physical Frame Buffer: 8187.5 MB"));
        assert!(rendered.contains("Virtual Frame Buffer: 8188 MB"));
        assert!(!rendered.contains("Physical Frame Buffer: 8384000"));
        // Compute-capability flags decoded: 1795 = 0x703 = 0x1|0x2|0x100|0x200|0x400 =
        // BASE_COMPUTE | COMPUTE_CAPABLE | PHYSX_INSTALLED | VRAM_GE_256MB | PHYSX_GPU_SELECTED.
        // (Bit 0x4 BOARD_DB_MATCH absent — this SKU matched no board-DB row.)
        assert!(rendered.contains(
            "Flags: 1795 (0x703: BASE_COMPUTE | COMPUTE_CAPABLE | PHYSX_INSTALLED | VRAM_GE_256MB | PHYSX_GPU_SELECTED)"
        ));
        // Driver model value as hex + decoded WDDM version.
        assert!(rendered.contains("Value: 0x00003200 (WDDM 3.2)"));
        // Sensor range bounds carry a C unit.
        assert!(rendered.contains("Range: Max 139 C, Min -35 C"));
    }

    #[test]
    fn human_output_formats_vfp_points_as_rows() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "domain": "graphics",
            "indexed": true,
            "infer_missing_default": true,
            "points": [
                {
                    "index": 12,
                    "voltage_mv": 900.0,
                    "frequency_mhz": 1800.0,
                    "delta_mhz": 15.0,
                    "default_frequency_mhz": 1785.0,
                }
            ],
        });

        let rendered = format_human_output("get-public-vftable", &output).join("\n");

        assert!(rendered.contains("V-F Points"));
        assert!(rendered.contains("#12: 900.0 mV, 1800.0 MHz, delta 15.0 MHz"));
        assert!(!rendered.contains("\"points\""));
    }

    #[test]
    fn human_output_relabels_utilization_domains_with_percent() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "utilization": {
                "Graphics": 100,
                "FrameBuffer": 0,
                "VideoEngine": 0,
                "BusInterface": 2
            }
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        assert!(rendered.contains("Utilization"));
        // FrameBuffer is NVAPI's memory-controller domain -> relabelled.
        assert!(rendered.contains("Memory Controller: 0%"));
        assert!(rendered.contains("Graphics: 100%"));
        assert!(!rendered.contains("FrameBuffer"));
    }

    #[test]
    fn human_output_formats_status_units() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "voltage": 940000,
            "pcie_lanes": 8,
            "pcie_tx_mibps": 1234.5,
            "pcie_rx_mibps": 7.8,
            "memory": {
                "dedicated": 8384512,
                "dedicated_available": 8146944,
                "dedicated_available_current": 8144412,
                "dedicated_evictions": 0,
                "dedicated_evictions_size": 38224,
                "shared": 33355556,
                "system": 0
            },
            "sensors": [
                [
                    {
                        "target": "Gpu",
                        "channel_num": 0,
                        "channel_type": 0,
                        "range": { "max": 139, "min": -35 }
                    },
                    52.58203125
                ]
            ]
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        // Voltage reported in microvolts.
        assert!(rendered.contains("Voltage: 940000 uV"));
        // PCIe link width gets an `x` prefix.
        assert!(rendered.contains("Pcie Lanes: x8"));
        // Bidirectional PCIe bandwidth (NVML nvmlDeviceGetPcieThroughput) gets
        // a MiB/s unit, nvitop-style.
        assert!(rendered.contains("Pcie Tx Mibps: 1234.5 MiB/s"));
        assert!(rendered.contains("Pcie Rx Mibps: 7.8 MiB/s"));
        // Memory size fields are KiB -> MB; the eviction *count* has no unit.
        assert!(rendered.contains("Dedicated: 8188 MB"));
        assert!(rendered.contains("Shared: 32573.785 MB"));
        assert!(rendered.contains("Dedicated Evictions: 0"));
        assert!(!rendered.contains("Dedicated: 8384512"));
        // Sensors: target dropped, classified by channel type (no Name line),
        // temperature gets a C unit.
        assert!(!rendered.contains("Target"));
        assert!(!rendered.contains("Name:"));
        assert!(rendered.contains("Channel Num: 0"));
        assert!(rendered.contains("Channel Type: 0 (GPU_AVG)"));
        assert!(rendered.contains("Range: Max 139 C, Min -35 C"));
        assert!(rendered.contains("- 52.58203125 C"));
    }

    #[test]
    fn human_output_hides_zero_sensor_range() {
        nvoc_cli_common::color::init(true);
        // A channel whose GetInfo record reports no limit data has range
        // {0, 0}; that uninformative line is suppressed.
        let output = json!({
            "sensors": [
                [
                    {
                        "target": "Gpu",
                        "channel_num": 1,
                        "channel_type": 1,
                        "range": { "max": 0, "min": 0 }
                    },
                    78.5
                ]
            ]
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        assert!(rendered.contains("Channel Type: 1 (GPU_MAX)"));
        assert!(rendered.contains("- 78.5 C"));
        assert!(!rendered.contains("Range"));
        assert!(!rendered.contains("Target"));
        assert!(!rendered.contains("Name:"));
    }

    #[test]
    fn human_output_renders_p2_monitoring_fields() {
        nvoc_cli_common::color::init(true);
        // P2 dimensions surfaced in get-status: the board/GPU power split
        // (both topology channels), the perf-decrease reason bitset, the fan
        // arbiter (zero-RPM) status/control, and the legacy thermal/fan levels.
        let output = json!({
            "power": {
                "TotalGpuPower": 84,
                "NormalizedTotalPower": 82
            },
            "performance_decrease": { "bits": 2 },
            "fan_arbiter_status": { "0": { "fan_stopped": false } },
            "fan_arbiter_control": { "0": { "stop_fan": true } },
            "current_thermal_level": 3,
            "current_fan_speed_level": 2
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        // Both power topology channels (board/GPU split) are shown, each with a
        // `%` unit (the values are 0–100 plain percentages). Keys are PascalCase
        // enum variant names, rendered verbatim by format_label (no `_` to split).
        assert!(rendered.contains("TotalGpuPower: 84%"));
        assert!(rendered.contains("NormalizedTotalPower: 82%"));
        // Perf-decrease reason is decoded to friendly text (bitflags name array
        // -> "Power Control"), not the raw enum name.
        assert!(rendered.contains("Power Control"));
        assert!(rendered.contains("Performance Decrease"));
        // Fan arbiter (zero-RPM) and legacy levels. Field labels are title-cased
        // by the generic renderer (snake_case -> "Fan Stopped" / "Stop Fan").
        assert!(rendered.contains("Fan Arbiter Status"));
        assert!(rendered.contains("Fan Arbiter Control"));
        assert!(rendered.contains("Fan Stopped"));
        assert!(rendered.contains("Stop Fan"));
        assert!(rendered.contains("Current Thermal Level"));
        assert!(rendered.contains("Current Fan Speed Level"));
    }

    #[test]
    fn human_output_renders_p0_voltage_block() {
        nvoc_cli_common::color::init(true);
        // P0 voltage bounds from the private VoltRails status entry. Values are
        // stored in µV and rendered in mV; the block is a multi-line section
        // (like Memory), not a comma-separated measurement-map line, and the
        // redundant per-field "UV" label suffix is dropped (the mV unit carries
        // the dimension).
        let output = json!({
            "p0_voltage": {
                "current_uV": 900000,
                "target_wall_uV": 1200000,
                "effective_wall_uV": 1200000,
                "vbios_wall_uV": 0,
                "vrm_max_wall_uV": 1200000,
                "min_hold_uV": 625000,
                "offset_ceiling_uV": 195500
            }
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        // Section title is "P0 Voltage Limit" (not "P0 Voltage:"), header has
        // no trailing colon, and each bound is its own indented `Label: N mV`.
        assert!(rendered.contains("P0 Voltage Limit"));
        assert!(!rendered.contains("P0 Voltage:"));
        assert!(rendered.contains("Current: 900 mV"));
        assert!(rendered.contains("Effective Wall: 1200 mV"));
        assert!(rendered.contains("Min Hold: 625 mV"));
        assert!(rendered.contains("Offset Ceiling: 195.5 mV"));
        assert!(rendered.contains("Target Wall: 1200 mV"));
        assert!(rendered.contains("Vbios Wall: 0 mV"));
        assert!(rendered.contains("Vrm Max Wall: 1200 mV"));
        // No comma-separated single line, no redundant "UV" label suffix.
        assert!(!rendered.contains("Current UV"));
        assert!(!rendered.contains("Effective Wall UV"));
        assert!(!rendered.contains("Offset Ceiling UV 195.5 mV"));
    }

    #[test]
    fn human_output_decodes_perf_bitset() {
        nvoc_cli_common::color::init(true);
        // On the native get-status path serde renders PerfFlags as `{"bits": N}`;
        // perf.unknown is a load-level indicator (7 = idle). Both must be decoded,
        // not shown as raw ints.
        let output = json!({
            "perf": { "unknown": 7, "limits": { "bits": 16 } }
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        assert!(rendered.contains("Limits: No Load"));
        assert!(rendered.contains("Load Level: Idle"));
        // Raw-int rendering is gone.
        assert!(!rendered.contains("Bits: 16"));
        assert!(!rendered.contains("Unknown"));
    }

    #[test]
    fn human_output_decodes_perf_multiple_reasons() {
        nvoc_cli_common::color::init(true);
        // limits bits = 3 = POWER_LIMIT(1) | THERMAL_LIMIT(2), decoded in bit
        // order. The raw-int form (pynvoc path) is also accepted.
        let rendered_obj = format_human_output(
            "get-status",
            &json!({ "perf": { "unknown": 1, "limits": { "bits": 3 } } }),
        )
        .join("\n");
        assert!(rendered_obj.contains("Limits: Power, Temperature"));
        assert!(rendered_obj.contains("Load Level: Load"));

        let rendered_raw = format_human_output(
            "get-status",
            &json!({ "perf": { "unknown": 1, "limits": 3 } }),
        )
        .join("\n");
        assert!(rendered_raw.contains("Limits: Power, Temperature"));
    }

    #[test]
    fn human_output_decodes_performance_decrease() {
        nvoc_cli_common::color::init(true);
        // bits = 3 = THERMAL_PROTECTION(1) | POWER_CONTROL(2), decoded in bit
        // order to friendly text on a single line.
        let output = json!({
            "performance_decrease": { "bits": 3 }
        });
        let rendered = format_human_output("get-status", &output).join("\n");
        assert!(rendered.contains("Performance Decrease: Thermal Protection, Power Control"));

        // bits = 0 (idle GPU) -> None.
        let rendered_none = format_human_output(
            "get-status",
            &json!({ "performance_decrease": { "bits": 0 } }),
        )
        .join("\n");
        assert!(rendered_none.contains("Performance Decrease: None"));
    }

    #[test]
    fn human_output_compacts_range_fields() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "max_voltage_uv": 0,
            "min_voltage_uv": 0,
            "voltage": {
                "max": 0,
                "min": 0,
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(!rendered.contains("Max Voltage"));
        assert!(!rendered.contains("Min Voltage"));
    }

    #[test]
    fn human_output_adds_contextual_units_to_nested_ranges() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "graphics": {
                "frequency": {
                    "max": 2145000,
                    "min": 300000,
                },
                "frequency_delta": {
                    "max": 1000000,
                    "min": -1000000,
                },
                "voltage": {
                    "max": 0,
                    "min": 0,
                },
                "voltage_domain": "Undefined",
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Frequency: Max 2145 MHz, Min 300 MHz"));
        assert!(rendered.contains("Frequency Delta: Max 1000 MHz, Min -1000 MHz"));
        // Empty pstate-limit fields are hidden to keep output terse:
        // voltage {max:0, min:0} and voltage_domain "Undefined" are dropped.
        assert!(!rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(!rendered.contains("Voltage Domain: Undefined"));
    }

    #[test]
    fn human_output_compacts_clock_maps_with_units() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "base_clocks": {
                "graphics": 1530000,
                "memory": 4001000,
            },
            "boost_clocks": {
                "graphics": 1830000,
                "memory": 4001000,
            },
            "bios_version": "90.16.34.00.60",
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Base Clocks: Graphics 1530 MHz, Memory 4001 MHz"));
        assert!(rendered.contains("Boost Clocks: Graphics 1830 MHz, Memory 4001 MHz"));
        assert!(rendered.contains("Bios Version: 90.16.34.00.60"));
    }

    #[test]
    fn human_output_labels_pff_throttle_curve_points() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "throttle_curve": {
                "points": [
                    {"x": 21248, "y": 1830000},
                    {"x": 22528, "y": 1830000},
                    {"x": 23040, "y": 1530000},
                ],
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("#0: Temperature 83 C -> Frequency 1830 MHz"));
        assert!(rendered.contains("#1: Temperature 88 C -> Frequency 1830 MHz"));
        assert!(rendered.contains("#2: Temperature 90 C -> Frequency 1530 MHz"));
        assert!(!rendered.contains("X:"));
        assert!(!rendered.contains("Y:"));
    }

    #[test]
    fn human_output_labels_vfp_limit_ranges_as_mhz_delta() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "vfp_limits": {
                "graphics": {
                    "range": {
                        "max": 500000,
                        "min": -500000,
                    },
                },
                "memory": {
                    "range": {
                        "max": 1500000,
                        "min": -500000,
                    },
                },
            },
            "virtual_frame_buffer": 6291456,
        });

        let rendered = format_human_output("get-info", &output).join("\n");

        assert!(rendered.contains("Range: Max 500 MHz, Min -500 MHz"));
        assert!(rendered.contains("Range: Max 1500 MHz, Min -500 MHz"));
        // Frame-buffer sizes are KiB -> MB (6291456 KiB = 6144 MB).
        assert!(rendered.contains("Virtual Frame Buffer: 6144 MB"));
    }

    #[test]
    fn human_output_summarizes_get_settings_vfp_deltas() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "vfp": {
                "graphics": {
                    "0": 0,
                    "1": 0,
                    "2": 15000,
                    "10": -30000,
                },
                "memory": {
                    "0": 0,
                    "1": 0,
                    "2": 0,
                },
            },
        });

        let rendered = format_human_output("get-settings", &output).join("\n");

        assert!(rendered.contains("VFP Deltas"));
        assert!(rendered.contains("Graphics: 4 points, 2 changed: #2 15 MHz, #10 -30 MHz"));
        assert!(rendered.contains("Memory: 3 points, all 0 MHz"));
        assert!(!rendered.contains("  10:"));
    }

    #[test]
    fn human_output_sorts_integer_keyed_maps_numerically() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "points": {
                "10": "ten",
                "2": "two",
                "1": "one",
            },
        });

        let rendered = format_human_output("get-info", &output).join("\n");
        let one = rendered.find("1: one").unwrap();
        let two = rendered.find("2: two").unwrap();
        let ten = rendered.find("10: ten").unwrap();

        assert!(one < two);
        assert!(two < ten);
    }

    #[test]
    fn human_output_throttle_reasons_appends_violation_block() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "reasons": [
                {"name": "GPU Idle", "active": true},
                {"name": "HW Slowdown", "active": false},
            ],
            "violation": {
                "entries": [
                    {"name": "Pwr", "seconds": 1026.2},
                    {"name": "Idle", "seconds": 58552.4},
                    {"name": "AppClk", "seconds": 0.0},
                ],
                "since": "2026-05-26 18:00:41 UTC",
            },
        });

        let rendered = format_human_output("get-throttle-reasons", &output).join("\n");

        // Instantaneous reasons come first.
        assert!(rendered.contains("Reason GPU Idle | Active yes"));
        assert!(rendered.contains("Reason HW Slowdown | Active no"));
        // Violation block header carries the since timestamp.
        assert!(rendered.contains("Violation Status (since 2026-05-26 18:00:41 UTC)"));
        // Non-zero cumulative times are listed; the zero entry is dropped.
        assert!(rendered.contains("Pwr") && rendered.contains("1026.2s"));
        assert!(rendered.contains("Idle") && rendered.contains("58552.4s"));
        assert!(!rendered.contains("AppClk"));
    }

    #[test]
    fn human_output_throttle_reasons_handles_missing_violation() {
        nvoc_cli_common::color::init(true);
        // Device exposes throttle reasons but no violation counters.
        let output = json!({"reasons": [{"name": "GPU Idle", "active": true}]});

        let rendered = format_human_output("get-throttle-reasons", &output).join("\n");

        assert!(rendered.contains("Reason GPU Idle | Active yes"));
        assert!(!rendered.contains("Violation Status"));
    }

    #[test]
    fn human_output_renders_every_function_without_json_dump() {
        nvoc_cli_common::color::init(true);

        for command in COMMANDS {
            let rendered = format_human_output(command.name(), &sample_output(*command)).join("\n");
            assert!(
                !rendered.contains('{') && !rendered.contains('}') && !rendered.contains('"'),
                "{} still renders JSON-like output:\n{}",
                command.name(),
                rendered
            );
        }
    }

    #[test]
    fn json_output_is_compact() {
        let execution = Execution {
            function: "get-power-watt",
            backend: "nvml".to_string(),
            warnings: Vec::new(),
            results: vec![TargetResult {
                gpu_id: Some(7),
                backend: "nvml",
                ok: true,
                output: Some(json!({
                    "min_watt": 100,
                    "current_watt": 250,
                    "max_watt": 350,
                })),
                error: None,
            }],
        };

        let rendered = serde_json::to_string(&execution_to_json(&execution)).unwrap();

        assert!(!rendered.contains('\n'));
        assert!(rendered.contains("\"function\":\"get-power-watt\""));
    }

    fn sample_output(command: Command) -> Value {
        match command {
            Command::GetGpuList => json!({
                "index": 0,
                "gpu_id": 1,
                "gpu_id_hex": "0x0001",
                "pci_bus": 1,
                "backend_nvapi": true,
                "backend_nvml": true,
                "uuid": "GPU-12345678-abcd-1234-abcd-1234567890ab",
                "name": "GPU",
            }),
            Command::GetDisplayList => json!([{
                "display_id": "0x00010001",
                "display_id_u32": 65537,
                "connector": "DisplayPort",
                "flags_hex": "0x00020054",
                "connected": true,
                "physically_connected": true,
                "active": true,
                "os_visible": true,
                "dynamic": false,
                "mst_root": false,
                "wireless": false,
            }]),
            Command::GetInfo => json!({
                "name": "GPU",
                "architecture": "Ada",
                "driver_version": "555.0",
            }),
            Command::GetUuid => json!("GPU-12345678-abcd-1234-abcd-1234567890ab"),
            Command::GetStatus => json!({
                "temperature_c": 65,
                "core_clock_mhz": 1800,
                "memory_clock_mhz": 10500,
            }),
            Command::GetSettings => json!({
                "power_percent": 100,
                "thermal_limit_c": 83,
                "voltage_boost_percent": 0,
            }),
            Command::GetPublicVftable => json!({
                "domain": "graphics",
                "indexed": true,
                "infer_missing_default": true,
                "points": [{
                    "index": 0,
                    "voltage_mv": 800.0,
                    "frequency_mhz": 1500.0,
                    "delta_mhz": 0.0,
                    "default_frequency_mhz": 1500.0,
                }],
            }),
            Command::GetPowerLimit => {
                json!({"source": "nvml", "min_watt": 100, "current_watt": 250, "max_watt": 350})
            }
            Command::GetPstateGlobalFreqOffset => {
                json!({"domain": "graphics", "pstate": "P0", "offset_mhz": 120})
            }
            Command::GetPstateFreqRange => json!([{
                "pstate": "P0",
                "min_core_mhz": 300,
                "max_core_mhz": 2700,
                "min_memory_mhz": 405,
                "max_memory_mhz": 10500,
            }]),
            Command::GetPStateLock => json!({
                "supported": true,
                "locked_pstates": [3],
                "pstates": [
                    {"pstate": "P3", "locked": true, "clocks": {"graphics": {"max_mhz": 2565.0, "min_mhz": 780.0}, "memory": {"max_mhz": 7001.0, "min_mhz": 7001.0}, "video": {"max_mhz": 2565.0, "min_mhz": 780.0}}},
                ],
            }),
            Command::GetSupportedLegacyApplicationFreq => {
                json!([{"memory_mhz": 10500, "graphics_mhz": 1800}])
            }
            Command::GetFanInfo => json!({"count": 2, "min_percent": 30, "max_percent": 100}),
            Command::GetTemperatureThresholds => {
                json!([{"name": "shutdown", "celsius": 95}])
            }
            Command::GetLegacyTempSensor => json!([
                {"target": "Gpu", "controller": "GpuInternal", "current_c": 50, "min_c": -5, "max_c": 95},
                {"target": "Memory", "controller": "GpuInternal", "current_c": 52, "min_c": -5, "max_c": 95},
                {"target": "Board", "controller": "GpuInternal", "current_c": 48, "min_c": 0, "max_c": 100},
            ]),
            Command::GetPowerMode => {
                json!({"supported": true, "active": "Max", "mode_mask": 1, "max_mode_idx": 1})
            }
            Command::SetPowerMode => json!({"applied": true, "power_mode": "Max"}),
            Command::GetThrottleReasons => json!({
                "reasons": [
                    {"name": "GPU Idle", "active": true},
                    {"name": "HW Slowdown", "active": false},
                ],
                "violation": {
                    "entries": [
                        {"name": "Pwr", "seconds": 1026.2},
                        {"name": "Idle", "seconds": 58552.4},
                        {"name": "AppClk", "seconds": 0.0},
                    ],
                    "since": "2026-05-26 18:00:41 UTC",
                },
            }),
            Command::GetPublicPowerLimit => json!({
                "min_tdp_percent": 50,
                "default_tdp_percent": 100,
                "max_tdp_percent": 120,
            }),
            Command::GetPublicTempLimit => json!({
                "min_temp_c": 65,
                "default_temp_c": 83,
                "max_temp_c": 91,
                "curve": "Default",
            }),
            Command::GetLegacyOvervoltRanges => {
                json!([{"pstate": "P0", "min_uv": 0, "current_uv": 0, "max_uv": 100000}])
            }
            Command::GetLegacyP0CoreMaxVoltageDelta => json!({"max_delta_uv": 100000}),
            Command::GetLegacyGpcRailOvervoltLimit => json!({
                "pstate": "P0",
                "voltage_domain": "core",
                "editable": true,
                "voltage_uv": 900000,
                "delta_uv": 100000,
                "min_delta_uv": 0,
                "max_delta_uv": 100000,
            }),
            Command::GetPublicGpcRailVoltBoost => json!({"voltage_boost_percent": 25}),
            Command::GetAutoboostStatus => json!({"enabled": true, "default_enabled": false}),
            Command::GetAutoboostSupport => {
                json!({"api": "app-clocks", "restricted": true})
            }
            Command::GetEdid => json!({
                "display_id": "0x00010001",
                "bytes": 4,
                "edid_hex": "00FFFFFF",
            }),
            Command::SetPstateGlobalFreqOffset => json!({
                "applied": true,
                "backend": "nvapi",
                "domain": "graphics",
                "pstate": "P0",
                "offset_mhz": 120,
            }),
            Command::SetPublicTgpPercent => json!({"applied": true, "power_percent": 90}),
            Command::SetPpabStatus => json!({"applied": true, "dynamic_boost": true}),
            Command::SetPowerLimit => json!({"applied": true, "tgp_watt": 140, "tgp_mw": 140000}),
            Command::ResetPowerLimit => json!({"applied": true, "default_watt": 100.0}),
            Command::GetDNotifier => json!({
                "active": "D2",
                "levels": [
                    {"level": "D1", "watts": null, "active": false},
                    {"level": "D2", "watts": 55.0, "active": true},
                    {"level": "D3", "watts": 45.0, "active": false},
                    {"level": "D4", "watts": 33.0, "active": false},
                    {"level": "D5", "watts": 10.0, "active": false},
                ],
            }),
            Command::SetDNotifier => json!({"applied": true, "dnotifier_level": "D3"}),
            Command::SetTempLimit => json!({"applied": true, "thermal_limit_c": 83}),
            Command::SetPrivateTargetTempLimit => {
                json!({"applied": true, "policy_index": 2, "celsius": 85.0})
            }
            Command::SetFanSpeed => {
                json!({"applied": true, "fan": "all", "policy": "manual", "level_percent": 65})
            }
            Command::SetFreqLock => {
                json!({"applied": true, "domain": "graphics", "min_mhz": 1500, "max_mhz": 1800})
            }
            Command::SetGpcVoltLock => json!({"applied": true, "target": "900mv"}),
            Command::OemOcScanner => {
                json!({"applied": true, "action": "start"})
            }
            Command::SetPublicVftablePointOffset => {
                json!({"applied": true, "point": 12, "delta_mhz": 15})
            }
            Command::SetPublicVftableRangeOffset => {
                json!({"applied": true, "start": 12, "end": 16, "delta_mhz": 15})
            }
            Command::SetPstateLockViaMemRange => {
                json!({"applied": true, "pstate_range": "P0..P2", "min_lock_mhz": 300, "max_lock_mhz": 1800})
            }
            Command::SetLegacyApplicationFreqLock => {
                json!({"applied": true, "memory_mhz": 10500, "graphics_mhz": 1800})
            }
            Command::SetLegacyGpcRailOvervoltLimit => {
                json!({"applied": true, "pstate": "P0", "delta_uv": 100000})
            }
            Command::SetOvervoltUv => {
                json!({"applied": true, "overvolt_delta_uv": 50000})
            }
            Command::SetPublicGpcRailVoltBoost => {
                json!({"applied": true, "voltage_boost_percent": 25})
            }
            Command::SetAutoboostStatus | Command::ResetAutoboostStatus => {
                json!({"applied": true, "enabled": true})
            }
            Command::SetAutoboostSupport => {
                json!({"applied": true, "api": "app-clocks", "restricted": true})
            }
            Command::SetEdid => {
                json!({"applied": true, "display_id": "0x00010001", "bytes": 128})
            }
            Command::ClearEdid => {
                json!({"applied": true, "display_id": "0x00010001"})
            }
            Command::SetLegacyFreq => {
                json!({"applied": true, "domain": "core", "mhz": 900, "core_mhz": 900, "memory_mhz": 0})
            }
            Command::ResetLegacyApplicationFreqLock
            | Command::ResetPublicVftableGpcLock
            | Command::ResetPublicTgpPercent
            | Command::ResetTempLimit
            | Command::ResetLegacyGpcRailOvervoltLimit
            | Command::ResetPstateGlobalFreqOffset => json!({"applied": true}),
            Command::ResetPublicGpcRailVoltBoost => {
                json!({"applied": true, "voltage_boost_percent": 0})
            }
            Command::ResetFreqLock | Command::ResetPublicVftableOffset => {
                json!({"applied": true, "domain": "graphics"})
            }
            Command::ResetFanSpeed => json!({"applied": true, "fan_indices": [0, 1]}),
            Command::SetPStateLock => json!({"applied": true, "pstate": "P3"}),
            Command::ResetPStateLock => json!({"applied": true}),
            Command::GetVoltRailInfo => json!({
                "rail_mask": "0x00000001",
                "p0": {
                    "current_uV": 700000,
                    "target_wall_uV": 750000,
                    "effective_wall_uV": 750000,
                    "vbios_wall_uV": 0,
                    "vrm_max_wall_uV": 1200000,
                    "min_hold_uV": 600000,
                    "offset_ceiling_uV": 450000,
                },
                "rail_descriptors": [{
                    "rail_bit": 0,
                    "type": 1,
                }],
                "control": [{
                    "rail_bit": 0, "type": 3, "values_uV": [0, 0, 0, 0, 0, 0],
                }],
                "status": [{
                    "rail_bit": 0, "type": 1, "values_uV": [700000, 750000, 0, 1200000, 750000, 600000],
                }],
            }),
            Command::SetVoltRailLimit => json!({
                "applied": true,
                "rail_bit": 0,
                "previous_uV": 0,
                "applied_uV": -25000,
                "effective_wall_uV": 980000,
            }),
            Command::GetPrivateFreqDomainInfo => json!({
                "controllable_mask": "0x000000FF",
                "entries": [{
                    "bit": 1, "type": 10, "value_modifiable": false, "offset_kHz": 0,
                    "range_min_kHz": 0, "range_max_kHz": 0, "applied_kHz": 0,
                }],
            }),
            Command::GetPrivateFreqDomainStatus => json!({
                "domain_bit": 1, "domain": "Xbar", "freq_mhz": 2004.0,
            }),
            Command::GetPrivateVftable => json!({
                // Output shape mirrors the Command::GetPrivateVftable execution
                // arm (banked masks + contiguous same-type segments + the
                // flat point grid).
                "masks": ["0x0000000000000000"],
                "segments": [{
                    "bank": 0, "domain": "gpc", "kind": "vf_curve", "type": 1,
                    "start_index": 0, "end_index": 39, "count": 40,
                    "voltage_uV_min": 450000, "voltage_uV_max": 1080500,
                    "freq_default_mhz_min": 210, "freq_default_mhz_max": 2700,
                }],
                "points": [{
                    "bank": 0, "index": 0, "type": 1,
                    "voltage_uV": 450000,
                    "freq_default_mhz": 210, "freq_current_mhz": 210,
                }],
            }),
            Command::SetPrivateFreqDomainGlobalOffset => json!({
                "applied": true, "bit": 1, "type": 10,
                "previous_kHz": 0, "applied_kHz": -60000, "temporary_restored": true,
            }),
            Command::SetPrivateVftablePointOffset => json!({
                "applied": true, "bank": 0, "index": 191,
                "mode": "raw_f_offset_control", "value": 100,
                "unit": "raw", "retained": 100,
            }),
            Command::SetPrivateVftableRangeOffset => json!({
                "applied": true, "bank": 0, "start": 191, "end": 191,
                "raw_f_offset_control_value": 100, "points_written": 1,
            }),
            _ => json!({}),
        }
    }
}
