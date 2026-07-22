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
        "get-vfp" => format_vfp_output(output),
        "get-pstates" => format_object_array(
            output,
            &[
                ("pstate", "P-State"),
                ("min_core_mhz", "Core Min"),
                ("max_core_mhz", "Core Max"),
                ("min_memory_mhz", "Memory Min"),
                ("max_memory_mhz", "Memory Max"),
            ],
        ),
        "get-supported-app-clocks" => format_object_array(
            output,
            &[("memory_mhz", "Memory"), ("graphics_mhz", "Graphics")],
        ),
        "get-temperature-thresholds" => {
            format_object_array(output, &[("name", "Threshold"), ("celsius", "Limit")])
        }
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

                match value {
                    Value::Object(child) if key == "utilization" => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title(&format_label(key))
                        ));
                        lines.extend(format_utilization_entries(indent + 1, child));
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
                    _ => lines.push(format_field_line(indent, key, value)),
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

/// Render the VRAM info map. Size fields are kibibytes -> shown in MB;
/// `dedicated_evictions` is a plain count (no unit).
fn format_memory_entries(
    indent: usize,
    object: &serde_json::Map<String, Value>,
) -> Vec<String> {
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
        let descriptor = tuple.get(0).and_then(Value::as_object);
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
/// `name` and `sensor_mask_number` are emitted verbatim; `range` is a
/// temperature limit shown as `Max N C, Min N C`, skipped when it is `{0, 0}`
/// (the undocumented thermal-sensors API reports no limits for hot-spot /
/// memory sensors, so a zero range carries no information).
fn format_sensor_descriptor(indent: usize, descriptor: &serde_json::Map<String, Value>) -> Vec<String> {
    let field = |key: &str, value: &Value| {
        format!(
            "{}{}: {}",
            indent_spaces(indent),
            nvoc_cli_common::color::stylize_title(&format_label(key)),
            nvoc_cli_common::color::stylize(&format_scalar(key, value), false)
        )
    };

    let mut lines = Vec::new();
    if let Some(name) = descriptor.get("name") {
        lines.push(field("name", name));
    }
    if let Some(range) = descriptor.get("range").and_then(Value::as_object) {
        let max = range.get("max").and_then(Value::as_f64);
        let min = range.get("min").and_then(Value::as_f64);
        // Skip a {0, 0} range (no limit data for undocumented sensors).
        let is_zero_range = matches!((max, min), (Some(0.0), Some(0.0)) | (Some(0.0), None) | (None, Some(0.0)));
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
    if let Some(mask) = descriptor.get("sensor_mask_number") {
        lines.push(field("sensor_mask_number", mask));
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
    if context.contains("voltage") && !context.contains("domain") {
        return format_measurement(number / 1000.0, "mV");
    }
    format_scalar(value_key, value)
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
    } else if key.ends_with("_watt") {
        format!("{rendered} W")
    } else if key.ends_with("_percent") || key == "percent" {
        format!("{rendered}%")
    } else if key.ends_with("_c") || key == "celsius" {
        format!("{rendered} C")
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

        let rendered = format_human_output("get-vfp", &output).join("\n");

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
                        "name": "Core",
                        "sensor_mask_number": 8,
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
        // Memory size fields are KiB -> MB; the eviction *count* has no unit.
        assert!(rendered.contains("Dedicated: 8188 MB"));
        assert!(rendered.contains("Shared: 32573.785 MB"));
        assert!(rendered.contains("Dedicated Evictions: 0"));
        assert!(!rendered.contains("Dedicated: 8384512"));
        // Sensors: target dropped, temperature gets a C unit.
        assert!(!rendered.contains("Target"));
        assert!(rendered.contains("Name: Core"));
        assert!(rendered.contains("Sensor Mask Number: 8"));
        assert!(rendered.contains("Range: Max 139 C, Min -35 C"));
        assert!(rendered.contains("- 52.58203125 C"));
    }

    #[test]
    fn human_output_hides_zero_sensor_range() {
        nvoc_cli_common::color::init(true);
        // Undocumented sensors (hot spot / memory junction) carry no limit data,
        // so their range is {0, 0}; that uninformative line is suppressed.
        let output = json!({
            "sensors": [
                [
                    {
                        "target": "Gpu",
                        "name": "Hot Spot",
                        "sensor_mask_number": 9,
                        "range": { "max": 0, "min": 0 }
                    },
                    78.5
                ]
            ]
        });

        let rendered = format_human_output("get-status", &output).join("\n");

        assert!(rendered.contains("Name: Hot Spot"));
        assert!(rendered.contains("- 78.5 C"));
        assert!(!rendered.contains("Range"));
        assert!(!rendered.contains("Target"));
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
        assert!(rendered.contains("Voltage: Max 0 mV, Min 0 mV"));
        assert!(rendered.contains("Voltage Domain: Undefined"));
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
        assert!(rendered.contains("Virtual Frame Buffer: 6291456"));
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
            Command::ListGpus => json!({
                "index": 0,
                "gpu_id": 1,
                "gpu_id_hex": "0x0001",
                "pci_bus": 1,
                "backend_nvapi": true,
                "backend_nvml": true,
                "uuid": "GPU-12345678-abcd-1234-abcd-1234567890ab",
                "name": "GPU",
            }),
            Command::ListDisplays => json!([{
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
            Command::GetVfp => json!({
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
            Command::GetVfpPointVoltageMv => {
                json!({"point": 0, "voltage_uv": 800000, "voltage_mv": 800.0})
            }
            Command::GetPowerWatt => {
                json!({"min_watt": 100, "current_watt": 250, "max_watt": 350})
            }
            Command::GetClockOffsetMhz => {
                json!({"domain": "graphics", "pstate": "P0", "offset_mhz": 120})
            }
            Command::GetPstates => json!([{
                "pstate": "P0",
                "min_core_mhz": 300,
                "max_core_mhz": 2700,
                "min_memory_mhz": 405,
                "max_memory_mhz": 10500,
            }]),
            Command::GetSupportedAppClocks => {
                json!([{"memory_mhz": 10500, "graphics_mhz": 1800}])
            }
            Command::GetFanInfo => json!({"count": 2, "min_percent": 30, "max_percent": 100}),
            Command::GetTemperatureThresholds => {
                json!([{"name": "shutdown", "celsius": 95}])
            }
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
            Command::GetTdpTempLimits => json!({
                "min_tdp_percent": 50,
                "default_tdp_percent": 100,
                "max_tdp_percent": 120,
                "min_temp_c": 65,
                "default_temp_c": 83,
                "max_temp_c": 91,
                "curve": "Default",
            }),
            Command::ProbeVoltageLimits => json!({"lower_point": 0, "upper_point": 80}),
            Command::CheckVoltageFrequency => {
                json!({"point": 42, "precise": true, "matched_point": 42})
            }
            Command::GetLegacyOvervoltRanges => {
                json!([{"pstate": "P0", "min_uv": 0, "current_uv": 0, "max_uv": 100000}])
            }
            Command::GetLegacyP0CoreMaxVoltageDelta => json!({"max_delta_uv": 100000}),
            Command::GetPstateBaseVoltageUv => json!({
                "pstate": "P0",
                "voltage_domain": "core",
                "editable": true,
                "voltage_uv": 900000,
                "delta_uv": 100000,
                "min_delta_uv": 0,
                "max_delta_uv": 100000,
            }),
            Command::GetVoltageBoostPercent => json!({"voltage_boost_percent": 25}),
            Command::GetAutoBoost => json!({"enabled": true, "default_enabled": false}),
            Command::GetApiRestriction => {
                json!({"api": "app-clocks", "restricted": true})
            }
            Command::GetEdid => json!({
                "display_id": "0x00010001",
                "bytes": 4,
                "edid_hex": "00FFFFFF",
            }),
            Command::SetCoreOffsetMhz
            | Command::SetMemoryOffsetMhz
            | Command::SetClockOffsetMhz => json!({
                "applied": true,
                "backend": "nvapi",
                "domain": "graphics",
                "pstate": "P0",
                "offset_mhz": 120,
            }),
            Command::SetPowerWatt => json!({"applied": true, "power_watt": 250}),
            Command::SetPowerPercent => json!({"applied": true, "power_percent": 90}),
            Command::SetThermalLimitC => json!({"applied": true, "thermal_limit_c": 83}),
            Command::SetFanPercent => {
                json!({"applied": true, "fan": "all", "policy": "manual", "level_percent": 65})
            }
            Command::SetLockedClocksMhz => {
                json!({"applied": true, "domain": "graphics", "min_mhz": 1500, "max_mhz": 1800})
            }
            Command::SetVfpVoltageLock => json!({"applied": true, "target": "900mv"}),
            Command::SetVfpPointDeltaMhz => {
                json!({"applied": true, "point": 12, "delta_mhz": 15})
            }
            Command::SetVfpRangeDeltaMhz => {
                json!({"applied": true, "start": 12, "end": 16, "delta_mhz": 15})
            }
            Command::SetPstateLock => {
                json!({"applied": true, "pstate_range": "P0..P2", "min_lock_mhz": 300, "max_lock_mhz": 1800})
            }
            Command::SetApplicationsClocksMhz => {
                json!({"applied": true, "memory_mhz": 10500, "graphics_mhz": 1800})
            }
            Command::SetPstateBaseVoltageUv => {
                json!({"applied": true, "pstate": "P0", "delta_uv": 100000})
            }
            Command::SetVoltageBoostPercent => {
                json!({"applied": true, "voltage_boost_percent": 25})
            }
            Command::SetAutoBoost | Command::SetAutoBoostDefault => {
                json!({"applied": true, "enabled": true})
            }
            Command::SetApiRestriction => {
                json!({"applied": true, "api": "app-clocks", "restricted": true})
            }
            Command::SetEdid => {
                json!({"applied": true, "display_id": "0x00010001", "bytes": 128})
            }
            Command::ClearEdid => {
                json!({"applied": true, "display_id": "0x00010001"})
            }
            Command::SetLegacyClocksMhz => {
                json!({"applied": true, "core_mhz": 900, "memory_mhz": 1800})
            }
            Command::ResetCoreOffsetMhz | Command::ResetMemoryOffsetMhz => json!({
                "applied": true,
                "domain": "graphics",
                "pstate": "P0",
                "offset_mhz": 0,
            }),
            Command::ResetApplicationsClocks
            | Command::ResetVfpLock
            | Command::ResetPowerPercent
            | Command::ResetThermalLimitC
            | Command::ResetPstateBaseVoltages
            | Command::ResetPstateClockOffsets => json!({"applied": true}),
            Command::ResetVoltageBoostPercent => {
                json!({"applied": true, "voltage_boost_percent": 0})
            }
            Command::ResetLockedClocks | Command::ResetVfpDeltas => {
                json!({"applied": true, "domain": "graphics"})
            }
            Command::ResetFan => json!({"applied": true, "fan_indices": [0, 1]}),
        }
    }
}
