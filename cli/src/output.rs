use crate::{Command, Execution};
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

/// Recursively annotate every `{"supported": false}` object in the JSON
/// tree with the thread's last NVAPI status failure — the JSON-mode twin
/// of the human renderer's "Last NVAPI error" line.
pub(super) fn annotate_last_error(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.get("supported").and_then(Value::as_bool) == Some(false)
                && let Some(err) = nvoc_core::last_status_error()
            {
                map.insert("last_error".into(), Value::String(err));
            }
            for child in map.values_mut() {
                annotate_last_error(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(annotate_last_error),
        _ => {}
    }
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
                lines.extend(render_command_output(execution.command, output));
                // The wrapper layers soft-fail unsupported surfaces to
                // {"supported": false} — surface the ORIGINAL NVAPI status
                // recorded underneath (e.g. "-9 IncompatibleStructVersion"
                // on a stamp-gated family vs a true absence).
                if output.get("supported").and_then(Value::as_bool) == Some(false)
                    && let Some(err) = nvoc_core::last_status_error()
                {
                    lines.push(nvoc_cli_common::color::stylize(
                        &format!("Last NVAPI error: {err}"),
                        true,
                    ));
                }
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

/// Per-command human renderer, dispatched through the CommandSpec table
/// (`formatter` field); commands without a dedicated renderer fall back to
/// the generic value block.
pub(super) fn render_command_output(command: Command, output: &Value) -> Vec<String> {
    match command.spec().formatter {
        Some(formatter) => formatter(output),
        None => format_value_block(output, 1),
    }
}

/// Human output for `get-info`: promotes the NVAPI interface version
/// (`NvAPI_GetInterfaceVersionString`, e.g. "R580") to a banner directly
/// under the per-GPU "ok" marker — it marks which API generation the
/// loaded driver exports, which gates every private/stamp-gated family —
/// and renders the identity/capability block below it without repeating
/// the version line.
pub(super) fn format_gpu_info(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(version) = output
        .get("nvapi_interface_version")
        .and_then(Value::as_str)
    {
        lines.push(format!("NVAPI Interface Version: {version}"));
    }
    let mut rest = output.clone();
    if let Some(map) = rest.as_object_mut() {
        map.remove("nvapi_interface_version");
    }
    lines.extend(format_value_block(&rest, 1));
    lines
}

pub(super) fn format_pstate_freq_range(output: &Value) -> Vec<String> {
    format_object_array(
        output,
        &[
            ("pstate", "P-State"),
            ("min_core_mhz", "Core Min"),
            ("max_core_mhz", "Core Max"),
            ("min_memory_mhz", "Memory Min"),
            ("max_memory_mhz", "Memory Max"),
        ],
    )
}

/// Human renderer for `get-private-legacy-pstates20-freq-domain-info` — the private pstates-2.0
/// delta table (GetPstates20Private 0xC5DDF56E). All numbers are RAW table
/// words (no unit conversion): `delta_raw` is in the table's native units
/// (percent-of-domainMax per the public-path marshalling, unverified per
/// SKU), `domain_id` is the 32-domain RTSS-space clock-domain id.
pub(super) fn format_pstates20_private(output: &Value) -> Vec<String> {
    use nvoc_cli_common::color::stylize_title;

    fn domain_name(id: u64) -> &'static str {
        match id {
            0 => "Gpc",
            1 => "Xbar",
            2 => "Sys",
            3 => "Hub/L2C",
            4 => "M",
            5 => "Host",
            6 => "Disp",
            7 => "Hotclk",
            8 => "Pclk0",
            9 => "Pclk1",
            10 => "Bypclk",
            11 => "Xclk",
            12 => "Vpv",
            13 => "Vps",
            14 => "Gpucacheclk",
            15 => "Gpc2",
            16 => "Xbar2",
            17 => "Sys2",
            18 => "Hub2",
            19 => "Leg",
            20 => "Pwr",
            21 => "Msd/Vid",
            22 => "Utils",
            23 => "ColdNv",
            24 => "ColdHotclk",
            25 => "Ltc2",
            26 => "2D",
            27 => "3D",
            28 => "Host1x",
            29 => "Disp0",
            30 => "Disp1",
            31 => "Pciegen",
            _ => "?",
        }
    }

    let mut lines = Vec::new();
    let caps = output.get("caps_editable").and_then(Value::as_bool);
    if let Some(editable) = caps {
        lines.push(format!(
            "{} {}",
            stylize_title("Caps Editable"),
            if editable { "yes" } else { "no" }
        ));
    }
    for (key, label) in [
        ("flags_raw", "Flags Raw"),
        ("num_pstates", "Num Pstates"),
        ("num_clocks", "Num Clocks"),
        ("num_voltages", "Num Voltages"),
    ] {
        if let Some(v) = output.get(key).and_then(Value::as_u64) {
            lines.push(format!("{} {v}", stylize_title(label)));
        }
    }
    let Some(pstates) = output.get("pstates").and_then(Value::as_array) else {
        return lines;
    };
    for pstate in pstates {
        let pid = pstate.get("pstate_id").and_then(Value::as_u64);
        let enabled = pstate.get("enabled").and_then(Value::as_bool);
        lines.push(format!(
            "{} P{}{}",
            stylize_title("Pstate"),
            pid.unwrap_or_default(),
            match enabled {
                Some(true) => " (enabled)",
                Some(false) => " (disabled)",
                None => "",
            }
        ));
        let Some(clocks) = pstate.get("clocks").and_then(Value::as_array) else {
            continue;
        };
        for clock in clocks {
            let id = clock.get("domain_id").and_then(Value::as_u64);
            let fmt = clock.get("fmt").and_then(Value::as_u64);
            let cen = clock.get("enabled").and_then(Value::as_bool);
            let delta = clock.get("delta_raw").and_then(Value::as_i64);
            lines.push(format!(
                "  domain {:>2} {:<11} fmt {} enabled {:<5} delta_raw {}",
                id.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                domain_name(id.unwrap_or(u64::MAX)),
                fmt.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                match cen {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "?",
                },
                delta.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            ));
        }
    }
    lines
}

pub(super) fn format_supported_legacy_app_freq(output: &Value) -> Vec<String> {
    format_object_array(
        output,
        &[("memory_mhz", "Memory"), ("graphics_mhz", "Graphics")],
    )
}

pub(super) fn format_legacy_temp_sensor(output: &Value) -> Vec<String> {
    format_object_array(
        output,
        &[
            ("target", "Target"),
            ("controller", "Controller"),
            ("current_c", "Current"),
            ("min_c", "Min"),
            ("max_c", "Max"),
        ],
    )
}

pub(super) fn format_legacy_gpc_rail_volt_range(output: &Value) -> Vec<String> {
    format_object_array(
        output,
        &[
            ("pstate", "P-State"),
            ("min_uv", "Min"),
            ("current_uv", "Current"),
            ("max_uv", "Max"),
        ],
    )
}

/// Human output for `get-power-ceiling` — mirrors nvidia-smi's PPAB
/// `GPU Ceiling Power Limit` trio (Current/Requested/Default) plus the
/// D-Notifier cap that often is the binding constraint.
pub(super) fn format_power_ceiling(output: &Value) -> Vec<String> {
    let supported = output.get("supported").and_then(Value::as_bool);
    if supported == Some(false) {
        return vec!["  Power Ceiling: N/A (private power-policy family unavailable)".to_string()];
    }
    fn watts(output: &Value, key: &str) -> String {
        match output.get(key).and_then(Value::as_f64) {
            Some(w) => format!("{w:.1} W"),
            None => "N/A".to_string(),
        }
    }
    vec![
        format!("  Ceiling (effective): {}", watts(output, "ceiling_watt")),
        format!("  Requested TGP: {}", watts(output, "requested_watt")),
        format!("  D-Notifier Cap: {}", watts(output, "dnotify_watt")),
        format!("  Default TGP: {}", watts(output, "default_watt")),
    ]
}

/// Human output for `get-power-command`: one ExtendedLimits power-command
/// lease cell (raw value, or the mW/W view on the request command where the
/// live unit is known).
pub(super) fn format_power_command(output: &Value) -> Vec<String> {
    let channel = output.get("channel").and_then(Value::as_u64).unwrap_or(0);
    let command = output.get("command").and_then(Value::as_str).unwrap_or("?");
    if output.get("unset").and_then(Value::as_bool) == Some(true) {
        return vec![format!(
            "  Power command (channel {channel}, {command}): unset (0xFFFFFFFF)"
        )];
    }
    let value = output.get("value").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = vec![format!(
        "  Power command (channel {channel}, {command}): {value}"
    )];
    if let (Some(mw), Some(watt)) = (
        output.get("value_mW").and_then(Value::as_u64),
        output.get("value_W").and_then(Value::as_f64),
    ) {
        lines.push(format!("    {mw} mW ({watt:.1} W)"));
    }
    if command == "request" {
        lines.push(
            "    note: echo layer — load-time enforcement stays at the legal slider window (set-pwr-cur-limit)".to_string(),
        );
    }
    lines
}

/// Human output for `set-power-command` — the post-write readback nvapi-rs
/// verified is what the formatter prints, with the envelope facts and the
/// echo-layer warning (the lease does NOT move load-time enforcement).
/// The `--kmd` shape (`"kmd": true`) renders the kernel patch cycle instead:
/// there the enforced wall WAS patched in driver memory via pmxdrv, so the
/// echo-layer warning would be wrong.
pub(super) fn format_set_power_command(output: &Value) -> Vec<String> {
    if output.get("kmd").and_then(Value::as_bool) == Some(true) {
        return format_set_power_command_kmd(output);
    }
    let channel = output.get("channel").and_then(Value::as_u64).unwrap_or(0);
    let command = output.get("command").and_then(Value::as_str).unwrap_or("?");
    let value = output.get("value").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = match (
        output.get("value_mW").and_then(Value::as_u64),
        output.get("value_W").and_then(Value::as_f64),
    ) {
        (Some(mw), Some(watt)) => vec![format!(
            "  Power command set: channel {channel} {command} = {mw} mW ({watt:.1} W)"
        )],
        _ => vec![format!(
            "  Power command set: channel {channel} {command} = {value}"
        )],
    };
    if let Some(baseline) = output.get("baseline_before_W").and_then(Value::as_f64) {
        lines.push(format!(
            "    baseline: {baseline:.1} W — restore with set-power-command {baseline:.0}"
        ));
    } else if let Some(baseline) = output.get("baseline_before").and_then(Value::as_u64) {
        if baseline == u32::MAX as u64 {
            lines
                .push("    baseline: unset sentinel (0xFFFFFFFF) — nothing to restore".to_string());
        } else {
            lines.push(format!("    baseline: {baseline} raw"));
        }
    }
    if let Some(default_mw) = output.get("board_default_mW").and_then(Value::as_u64) {
        lines.push(format!("    board default: {default_mw} mW"));
    }
    if let Some(envelope) = output.get("envelope").and_then(Value::as_str) {
        lines.push(format!("    envelope: {envelope}"));
    }
    lines.push(
        "    ECHO LAYER ONLY: GET/NVML/nvidia-smi read this value; load-time enforcement stays at the legal slider window (set-pwr-cur-limit)".to_string(),
    );
    lines
}

/// Human output for `set-power-command --kmd`: leads with the verified wall
/// transition, replays the kernel cycle log (service register → chain walk →
/// arm write with readback → lease → recheck) so the user can see the patch
/// actually happened in driver memory, and closes with the kernel-patch fact
/// plus the volatility caveat — never the echo-layer warning.
fn format_set_power_command_kmd(output: &Value) -> Vec<String> {
    let arm = output.get("arm").and_then(Value::as_str).unwrap_or("?");
    let before = output
        .get("wall_before_mw")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut lines = match output.get("wall_after_mw").and_then(Value::as_u64) {
        Some(after) => vec![format!(
            "  Kernel wall patched via pmxdrv ({arm} arm): {before} mW → {after} mW ({:.1} W)",
            after as f64 / 1000.0
        )],
        None => vec![format!("  Kernel wall patched via pmxdrv ({arm} arm)")],
    };
    if let Some(layout) = output.get("layout").and_then(Value::as_str) {
        lines.push(format!("    layout: {layout}"));
    }
    // tgp/lease notes are already appended to `steps` by core — replaying the
    // step log is the whole verbose block, no per-note summary on top.
    for step in output
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(step) = step.as_str() {
            lines.push(format!("    {}", step.trim()));
        }
    }
    lines.push(
        "    KERNEL PATCH APPLIED: the enforced wall in driver memory was rewritten through the pmxdrv kernel driver — load-time enforcement follows the new wall; this is NOT the echo-layer-only lease write (set-pwr-cur-limit still moves the legal slider)".to_string(),
    );
    if let Some(note) = output.get("note").and_then(Value::as_str) {
        lines.push(format!("    note: {note}"));
    }
    lines
        .into_iter()
        .map(|line| nvoc_cli_common::color::stylize_kmd_step(&line))
        .collect()
}

pub(super) fn format_power_mode(output: &Value) -> Vec<String> {
    let supported = output.get("supported").and_then(Value::as_bool);
    let active = output.get("active").and_then(Value::as_str).unwrap_or("?");
    match supported {
        Some(true) => vec![format!("  Power Mode: {active}")],
        _ => vec![format!("  Power Mode: N/A (unsupported on this GPU)")],
    }
}

pub(super) fn format_set_power_mode(output: &Value) -> Vec<String> {
    vec![format!(
        "  Power Mode set: {}",
        output
            .get("power_mode")
            .and_then(Value::as_str)
            .unwrap_or("?")
    )]
}

pub(super) fn format_get_settings_output(output: &Value) -> Vec<String> {
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

pub(super) fn format_vfp_output(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(object) = output.as_object() {
        for key in ["domain", "indexed", "infer_missing_field"] {
            if let Some(value) = object.get(key) {
                lines.push(format_field_line(1, key, value));
            }
        }

        if let Some(points) = object.get("points").and_then(Value::as_array) {
            lines.push(format!(
                "  {}",
                nvoc_cli_common::color::stylize_title("V-F Points")
            ));
            let mut current_domain: Option<&str> = None;
            for point in points {
                let domain = point.get("domain").and_then(Value::as_str);
                if domain != current_domain {
                    current_domain = domain;
                    // Segment separator: the public table concatenates the
                    // graphics curve (index 0..~126) and the trailing memory
                    // entries (~127..131); make the boundary explicit.
                    let seg_note = object
                        .get("segments")
                        .and_then(Value::as_array)
                        .and_then(|segs| {
                            segs.iter()
                                .find(|seg| seg.get("domain").and_then(Value::as_str) == domain)
                        })
                        .map(|seg| {
                            format!(
                                " (index {}..{})",
                                field_text(seg, "first_index"),
                                field_text(seg, "last_index")
                            )
                        })
                        .unwrap_or_default();
                    if let Some(domain) = domain {
                        lines.push(nvoc_cli_common::color::stylize(
                            &format!("    --- {domain}{seg_note} ---"),
                            false,
                        ));
                    }
                }
                let index = field_text(point, "index");
                let point_type = point
                    .get("point_type")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let voltage = field_text(point, "voltage_mv");
                let frequency = field_text(point, "frequency_mhz");
                let delta = field_text(point, "delta_mhz");
                let default_frequency = field_text(point, "default_frequency_mhz");
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    #{index} [{point_type}]: {voltage}, {frequency}, delta {delta}, default {default_frequency}"
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

// ── private V/F table column geometry (get-private-vftable human table) ──
const PVFP_W_ID: usize = 4;
const PVFP_W_VAL: usize = 7;
const PVFP_W_MODE: usize = 4;
const PVFP_SEP: &str = "|";

/// One table cell: right-aligned bare number, or a width-blank when this
/// generation's record layout does not report the field.
fn pvfp_cell(text: Option<String>, w: usize) -> String {
    match text {
        Some(t) => format!("{t:>w$}", w = w),
        None => " ".repeat(w),
    }
}

/// Human format for `get-vbios --enable-detail-parser` (and its deprecated
/// alias `--maxwell-vftable-decode`) — one aligned bare-number table per
/// ladder source, mirroring the private-vftable table style, followed by the
/// shared detail sections (power/thermal/fan/identity/memory/falcon/NVGI),
/// so pre-Pascal (Maxwell/Kepler) images read exactly like Pascal ones:
/// - `vbios-boost-ladder` (Maxwell GPU Boost 2.0): `Id | V_min | V_max | Freq`,
///   voltage = the vmap node's (min, max) in mV.
/// - `vbios-vp-ladder` (Pascal+ Virtual P-State): `Id | Freq` — VP points
///   carry no per-point voltage.
///
/// Any other get-vbios shape (brief summary, combined `--out` payload) falls
/// back to the generic value block.
pub(super) fn format_maxwell_vftable_output(output: &Value) -> Vec<String> {
    match output.get("source").and_then(Value::as_str) {
        Some("vbios-boost-ladder") => format_boost_ladder_table(output),
        Some("vbios-vp-ladder") => format_vp_ladder_table(output),
        Some("vbios-blackwell") => format_blackwell_table(output),
        _ => format_value_block(output, 1),
    }
}

/// Human format for the Blackwell container detail (`vbios-blackwell`):
/// container header + power record + thermal/fan lines, mirroring the
/// vp-ladder renderer's compact key-line style.
fn format_blackwell_table(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let container = output.get("container");
    let magic = container
        .and_then(|c| c.get("magic"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let base = container
        .and_then(|c| c.get("image_base"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bit = container
        .and_then(|c| c.get("bit_offset"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    lines.push(nvoc_cli_common::color::stylize(
        &format!("    --- Blackwell container {magic} | 55AA base {base:#x} | BIT {bit:#x} ---"),
        false,
    ));
    if let Some(v) = output.get("bios_version").and_then(Value::as_str) {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    bios: {v}"),
            false,
        ));
    }
    let iu = output.get("internal_use");
    if iu.and_then(|i| i.get("present")).and_then(Value::as_bool) == Some(true) {
        let parts: Vec<String> = [
            iu.and_then(|i| i.get("version"))
                .and_then(Value::as_str)
                .map(|s| s.to_string()),
            iu.and_then(|i| i.get("project"))
                .and_then(Value::as_str)
                .map(|s| format!("project {s}")),
            iu.and_then(|i| i.get("build_date"))
                .and_then(Value::as_str)
                .map(|s| format!("built {s}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !parts.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    internal: {}", parts.join(" | ")),
                false,
            ));
        }
    }
    if let Some(p) = output.get("power").filter(|p| !p.is_null()) {
        let rated = p.get("rated_watt").and_then(Value::as_f64).unwrap_or(0.0);
        let max = p.get("max_watt").and_then(Value::as_f64).unwrap_or(0.0);
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    power: rated {rated:.0} W, max {max:.0} W"),
            false,
        ));
    }
    if let Some(t) = output
        .get("thermal")
        .and_then(|t| t.get("slowdown_c"))
        .and_then(Value::as_array)
    {
        let temps: Vec<String> = t
            .iter()
            .filter_map(Value::as_u64)
            .map(|c| format!("{c}C"))
            .collect();
        if !temps.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    thermal slowdown: {}", temps.join("/")),
                false,
            ));
        }
    }
    if let Some(curve) = output
        .get("fan")
        .and_then(|f| f.get("curve"))
        .and_then(Value::as_array)
    {
        let points: Vec<String> = curve
            .iter()
            .filter_map(|p| {
                let t = p.get("temp_c").and_then(Value::as_f64)?;
                let r = p.get("rpm").and_then(Value::as_u64)?;
                Some(format!("{t:.1}C->{r}rpm"))
            })
            .collect();
        if !points.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    fan curve: {}", points.join(", ")),
                false,
            ));
        }
    }
    let pairs = output
        .get("fan")
        .and_then(|f| f.get("cooler_pairs"))
        .and_then(Value::as_array);
    if let Some(pairs) = pairs.filter(|p| !p.is_empty()) {
        let items: Vec<String> = pairs
            .iter()
            .filter_map(|p| {
                let min = p.get("min_rpm").and_then(Value::as_u64)?;
                let max = p.get("max_rpm").and_then(Value::as_u64)?;
                Some(format!("{min}/{max}"))
            })
            .collect();
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    fan coolers ({}): min/max rpm {}",
                items.len(),
                items.join(", ")
            ),
            false,
        ));
    }
    let delta = output
        .get("perf_pointers")
        .and_then(|p| p.get("delta"))
        .and_then(Value::as_str);
    if let Some(delta) = delta {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    perf ptrs: relocation delta {delta}"),
            false,
        ));
    }
    push_structural_sections(output, &mut lines);
    lines
}

fn format_boost_ladder_table(output: &Value) -> Vec<String> {
    let Some(points) = output.get("points").and_then(Value::as_array) else {
        return format_value_block(output, 1);
    };
    let mut lines = Vec::new();
    let version = output
        .get("table_version")
        .and_then(Value::as_str)
        .unwrap_or("?");
    lines.push(nvoc_cli_common::color::stylize(
        &format!(
            "    --- GPU Boost 2.0 ladder v{version} ({} points) --- [V=mV f=MHz]",
            points.len()
        ),
        false,
    ));
    let cols = [
        ("Id", PVFP_W_ID),
        ("V_min", PVFP_W_VAL),
        ("V_max", PVFP_W_VAL),
        ("Freq", PVFP_W_VAL),
    ];
    let header = cols
        .iter()
        .map(|(h, w)| format!("{h:>w$}", w = w))
        .collect::<Vec<_>>()
        .join(" | ");
    lines.push(nvoc_cli_common::color::stylize(
        &format!("    {header}"),
        false,
    ));
    for point in points {
        let id = point
            .get("index")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let v_min = point
            .get("voltage_uv")
            .and_then(Value::as_f64)
            .unwrap_or_default()
            / 1000.0;
        let v_max = point
            .get("voltage_max_uv")
            .and_then(Value::as_f64)
            .unwrap_or_default()
            / 1000.0;
        let freq = point
            .get("frequency_mhz")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let cells = [
            pvfp_cell(Some(id.to_string()), PVFP_W_ID),
            pvfp_cell(Some(format!("{v_min:.2}")), PVFP_W_VAL),
            pvfp_cell(Some(format!("{v_max:.2}")), PVFP_W_VAL),
            pvfp_cell(Some(format!("{freq:.1}")), PVFP_W_VAL),
        ];
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    {}", cells.join(" | ")),
            false,
        ));
    }
    // pstate boundary marks as one compact legend line under the table
    if let Some(marks) = output.get("pstate_marks").and_then(Value::as_array) {
        let items: Vec<String> = marks
            .iter()
            .map(|m| {
                let idx = m.get("index").and_then(Value::as_i64).unwrap_or_default();
                match m.get("pstate").and_then(Value::as_str) {
                    Some(ps) => format!("{ps}@{idx}"),
                    None => format!("@{idx}"),
                }
            })
            .collect();
        if !items.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    pstate marks: {}", items.join(", ")),
                false,
            ));
        }
    }
    push_detail_shared_sections(output, &mut lines);
    lines
}

/// Human format for the Pascal+ VP ladder: `Id | Freq` (VP points carry no
/// voltage), a boost-profile legend under the table, and the memory clock.
fn format_vp_ladder_table(output: &Value) -> Vec<String> {
    let Some(points) = output.get("points").and_then(Value::as_array) else {
        return format_value_block(output, 1);
    };
    let mut lines = Vec::new();
    let generation = output
        .get("generation")
        .and_then(Value::as_str)
        .unwrap_or("?");
    lines.push(nvoc_cli_common::color::stylize(
        &format!(
            "    --- VP ladder ({generation}, {} points) --- [f=MHz; no per-point voltage]",
            points.len()
        ),
        false,
    ));
    let cols = [("Id", PVFP_W_ID), ("Freq", PVFP_W_VAL)];
    let header = cols
        .iter()
        .map(|(h, w)| format!("{h:>w$}", w = w))
        .collect::<Vec<_>>()
        .join(" | ");
    lines.push(nvoc_cli_common::color::stylize(
        &format!("    {header}"),
        false,
    ));
    for point in points {
        let id = point
            .get("index")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let freq = point
            .get("frequency_mhz")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let cells = [
            pvfp_cell(Some(id.to_string()), PVFP_W_ID),
            pvfp_cell(Some(format!("{freq:.1}")), PVFP_W_VAL),
        ];
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    {}", cells.join(" | ")),
            false,
        ));
    }
    // boost profiles as a compact legend (limit clocks + mem clock per ID)
    if let Some(profiles) = output.get("profiles").and_then(Value::as_array) {
        let items: Vec<String> = profiles
            .iter()
            .map(|p| {
                let id = p.get("id").and_then(Value::as_str).unwrap_or("?");
                let l1 = p.get("limit1_mhz").and_then(Value::as_f64).unwrap_or(0.0);
                let l3 = p.get("limit3_mhz").and_then(Value::as_f64).unwrap_or(0.0);
                match p.get("pstate").and_then(Value::as_str) {
                    Some(ps) => format!("{id}({ps}) {l1:.0}/{l3:.0}"),
                    None => format!("{id} {l1:.0}/{l3:.0}"),
                }
            })
            .collect();
        if !items.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    profiles (limit1/limit3 MHz): {}", items.join(", ")),
                false,
            ));
        }
    }
    if let Some(mem) = output.get("mem_clock_ddr_mhz").and_then(Value::as_i64) {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    mem clock (DDR): {mem} MHz"),
            false,
        ));
    }
    // VP footers: per-ID memory clocks (CPR footer walk, file order). The
    // footer id is the pstate slot encoding; label it like the profiles row.
    if let Some(footers) = output.get("footers").and_then(Value::as_array) {
        let items: Vec<String> = footers
            .iter()
            .map(|f| {
                let id = f.get("id").and_then(Value::as_str).unwrap_or("?");
                let pstate = f
                    .get("pstate")
                    .and_then(Value::as_str)
                    .map(|p| format!("({p})"))
                    .unwrap_or_default();
                let mem = f.get("mem_clock_mhz").and_then(Value::as_i64).unwrap_or(0);
                let ddr = f
                    .get("mem_clock_ddr_mhz")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                format!("{id}{pstate} {mem}/{ddr}")
            })
            .collect();
        if !items.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    footers (DRAM/DDR MHz): {}", items.join(", ")),
                false,
            ));
        }
    }
    if let Some(dual) = output.get("dual_bank") {
        let banks = dual.get("banks").and_then(Value::as_u64).unwrap_or(0);
        match dual.get("identical").and_then(Value::as_bool) {
            Some(true) => {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    dual bank: {banks} banks, contents identical (bank0 rendered)"),
                    false,
                ));
            }
            Some(false) => {
                let fields: Vec<String> = dual
                    .get("differences")
                    .and_then(Value::as_array)
                    .map(|ds| {
                        ds.iter()
                            .filter_map(|d| d.get("field").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                    "    dual bank: {banks} banks DIFFER in {} (bank0 rendered; see dual_bank.differences)",
                    fields.join(", ")
                )));
            }
            _ => {}
        }
    }
    push_detail_shared_sections(output, &mut lines);
    lines
}

/// Detail sections shared by every ladder source (Pascal+ VP ladder and
/// the pre-Pascal Boost 2.0 fallback — Maxwell/Kepler render the same
/// lines as Pascal): power, thermal, DCB display map, identity,
/// internal-use, flash directory, memory, thermal limits, fan curve /
/// coolers, falcon ucode, NVGI and the PERF_PTR slot map.
/// Structural layer (vix 对照补差) human rendering: image chain + 架构门 +
/// 按族表解码，每表一段紧凑行。unsupported 形状以 warning 行呈现（fail
/// closed 保持可见）。
fn push_structural_sections(output: &Value, lines: &mut Vec<String>) {
    let Some(st) = output.get("structural") else {
        return;
    };
    // 镜像链 + 架构门。
    if let Some(arch) = st.get("architecture") {
        let (pci, npds, issues) = match st.get("image_chain") {
            Some(chain) => (
                chain
                    .get("pci_images")
                    .and_then(Value::as_array)
                    .map_or(0, |a| a.len()),
                chain
                    .get("npds_images")
                    .and_then(Value::as_array)
                    .map_or(0, |a| a.len()),
                chain
                    .get("issues")
                    .and_then(Value::as_array)
                    .map_or(0, |a| a.len()),
            ),
            None => (0, 0, 0),
        };
        let family = arch
            .get("family")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let product = arch.get("product").and_then(Value::as_str).unwrap_or("?");
        let device = arch
            .get("pci_device")
            .and_then(Value::as_str)
            .unwrap_or("????");
        let inferred = arch
            .get("family_inferred")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let versions: Vec<String> = arch
            .get("bios_versions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let ver_note = versions
            .first()
            .map(|v| format!(" bios {v}"))
            .unwrap_or_default();
        let scope_note = if arch.get("in_scope").and_then(Value::as_bool) == Some(false) {
            " OUT-OF-SCOPE"
        } else {
            ""
        };
        let infer_note = if inferred {
            " (family inferred from BIOS prefix)"
        } else {
            ""
        };
        let issue_note = if issues > 0 {
            format!(", {issues} ISSUES")
        } else {
            String::new()
        };
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    structural: {pci} PCI + {npds} NPDS images{issue_note} | {family}{scope_note}{infer_note} {device} {product}{ver_note}"
            ),
            false,
        ));
    } else if let Some(status) = st
        .get("image_chain")
        .and_then(|c| c.get("status"))
        .and_then(Value::as_str)
    {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    structural: {status}"),
            false,
        ));
    }
    // 3x 世代表（Turing/Ampere/Ada）。
    if let Some(t) = st.get("tables_v3") {
        if let Some(reason) = t.get("reason").and_then(Value::as_str) {
            lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                "    structural: {reason}"
            )));
        } else {
            let family = t.get("family").and_then(Value::as_str).unwrap_or("?");
            if let Some(p) = t.get("power_policy").and_then(Value::as_object)
                && let (Some(rows), Some(tgp)) = (
                    p.get("rows").and_then(Value::as_array),
                    p.get("tgp_index").and_then(Value::as_u64),
                )
                && let Some(tgp_row) = rows.get(tgp as usize)
            {
                let ver = p.get("version").and_then(Value::as_str).unwrap_or("?");
                let nb = if p.get("notebook").and_then(Value::as_bool) == Some(true) {
                    " notebook"
                } else {
                    ""
                };
                let unit = tgp_row.get("unit").and_then(Value::as_str).unwrap_or("mW");
                let scale = if unit == "mA" { 1.0 } else { 1000.0 };
                let fmt = |v: u64| {
                    if unit == "mA" {
                        format!("{v} {unit}")
                    } else {
                        format!("{:.0} W", v as f64 / scale)
                    }
                };
                let empty = rows
                    .iter()
                    .filter(|r| {
                        r.get("kind").and_then(Value::as_u64) == Some(0)
                            && r.get("maximum_raw").and_then(Value::as_u64) == Some(0)
                    })
                    .count();
                let empty_note = if empty > 0 {
                    format!(", {empty} empty")
                } else {
                    String::new()
                };
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    power policy (v3x {ver} {family}{nb}): TGP row{tgp} {}/{}/{}, {} rows{empty_note}",
                        fmt(tgp_row
                            .get("minimum_raw")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)),
                        fmt(tgp_row
                            .get("default_raw")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)),
                        fmt(tgp_row
                            .get("maximum_raw")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)),
                        rows.len()
                    ),
                    false,
                ));

                // 全行解码（对照 NVAPI GetPwrCurInfo：Index = 行号、Subtype = 通道
                // 字节、min/def/max 逐值一致；空行是驱动运行时隐藏的超集）。kind
                // 名为 vix 语料观测语义，未列出的按 raw 显示。
                if let Some(rows) = t
                    .get("power_policy")
                    .and_then(Value::as_object)
                    .and_then(|p| p.get("rows"))
                    .and_then(Value::as_array)
                {
                    let tgp = t
                        .get("power_policy")
                        .and_then(Value::as_object)
                        .and_then(|p| p.get("tgp_index"))
                        .and_then(Value::as_u64);
                    let items: Vec<String> = rows
                        .iter()
                        .filter(|r| {
                            !(r.get("kind").and_then(Value::as_u64) == Some(0)
                                && r.get("maximum_raw").and_then(Value::as_u64) == Some(0))
                        })
                        .map(|r| {
                            let row = r.get("row").and_then(Value::as_u64).unwrap_or(0);
                            // 角色名（vix 通道模板识别）优先于 kind 字节名。
                            if let Some(role) = r.get("role").and_then(Value::as_str)
                                && !role.is_empty()
                            {
                                return format_power_row(row, role, r);
                            }
                            let kind = r.get("kind").and_then(Value::as_u64).unwrap_or(0);
                            let ch = r.get("channel").and_then(Value::as_u64).unwrap_or(0);
                            // 存储单位旗标原样标注（mW/mA）；实际物理单位由 RM 策略
                            // 侧决定（NVAPI 实测与旗标在哨兵行上不一致），不做断言性
                            // 换算。
                            let unit = r.get("unit").and_then(Value::as_str).unwrap_or("mW");
                            let star = if Some(row) == tgp { "*" } else { "" };
                            format!(
                                "#{row}{star} {}/{} {}..{}..{} {}",
                                power_row_kind_name(kind as u8),
                                ch,
                                r.get("minimum_raw").and_then(Value::as_u64).unwrap_or(0),
                                r.get("default_raw").and_then(Value::as_u64).unwrap_or(0),
                                r.get("maximum_raw").and_then(Value::as_u64).unwrap_or(0),
                                unit
                            )
                        })
                        .collect();
                    if !items.is_empty() {
                        lines.push(nvoc_cli_common::color::stylize(
                            &format!("    power rows (* = TGP): {}", items.join(" | ")),
                            false,
                        ));
                    }
                }
            }
            render_rails(t, lines);
            render_rel_limits(t.get("rel_limits"), lines);
            render_vmin(t.get("vmin"), lines);
            render_clocks(t.get("clocks"), true, lines);
            render_memory_nvml_scale(t.get("clocks"), lines);
            render_fans_v3(t, lines);
        }
    }
    // GB20x（50 系）。
    if let Some(g) = st.get("gb20x") {
        if let Some(reason) = g.get("reason").and_then(Value::as_str) {
            lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                "    structural: {reason}"
            )));
            return;
        }
        if let Some(i) = g.get("integrity") {
            let ok = i.get("valid").and_then(Value::as_bool).unwrap_or(false);
            let objects = i.get("object_count").and_then(Value::as_u64).unwrap_or(0);
            let legacy = i
                .get("legacy_directory_present")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let legacy_note = if legacy { ", legacy dir ok" } else { "" };
            let text = if ok {
                format!("    integrity: {objects} objects sha384 OK, image sums zero{legacy_note}")
            } else {
                let bad = i
                    .get("bad_object_ids")
                    .and_then(Value::as_array)
                    .map_or(0, |a| a.len());
                format!("    integrity: FAILED ({bad} bad objects)")
            };
            if ok {
                lines.push(nvoc_cli_common::color::stylize(&text, false));
            } else {
                lines.push(nvoc_cli_common::color::stylize_warning(&text));
            }
        }
        if let Some(p) = g.get("power_policy").and_then(Value::as_object)
            && let Some(rows) = p.get("rows").and_then(Value::as_array)
        {
            let notebook = p.get("notebook").and_then(Value::as_bool) == Some(true);
            let tgp_row_idx = if notebook { 2 } else { 8 };
            if let Some(r) = rows.get(tgp_row_idx) {
                let ver = p.get("version").and_then(Value::as_str).unwrap_or("?");
                let nb = if notebook { " notebook" } else { "" };
                let fmt = |v: u64| format!("{:.0} W", v as f64 / 1000.0);
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    power policy (gb20x {ver}{nb}): TGP row{tgp_row_idx} {}/{}/{}, {} rows",
                        fmt(r.get("minimum_raw").and_then(Value::as_u64).unwrap_or(0)),
                        fmt(r.get("default_raw").and_then(Value::as_u64).unwrap_or(0)),
                        fmt(r.get("maximum_raw").and_then(Value::as_u64).unwrap_or(0)),
                        rows.len()
                    ),
                    false,
                ));
            }
        }
        render_rel_limits(g.get("rel_limits"), lines);
        render_vmin(g.get("vmin"), lines);
        render_clocks(g.get("clocks"), false, lines);
        render_memory_nvml_scale(g.get("clocks"), lines);
        if let Some(profiles) = g.get("profiles").and_then(Value::as_array)
            && !profiles.is_empty()
        {
            let items: Vec<String> = profiles
                .iter()
                .map(|p| {
                    let entries: Vec<String> = p
                        .get("entries")
                        .and_then(Value::as_array)
                        .map(|es| {
                            es.iter()
                                .map(|e| {
                                    format!(
                                        "p{}/{}/{:#x}",
                                        e.get("policy").and_then(Value::as_u64).unwrap_or(0),
                                        e.get("field").and_then(Value::as_str).unwrap_or("?"),
                                        e.get("value_raw").and_then(Value::as_u64).unwrap_or(0)
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    format!(
                        "#{}: {}",
                        p.get("index").and_then(Value::as_u64).unwrap_or(0),
                        entries.join(" ")
                    )
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    override profiles: {}", items.join(" | ")),
                false,
            ));
        }
    }
}

/// vix 语料观测的 3x power 行 type 字节语义（extended-fields.md / GB20x
/// 行类型交叉）；未收录的按 `type0x{n}` 显示。
fn power_row_kind_name(kind: u8) -> String {
    match kind {
        0 => "out".into(),
        1 => "board".into(),
        2 => "slot".into(),
        4 => "conn".into(),
        5 => "in-cur".into(),
        7 => "ocp".into(),
        10 => "ocp".into(),
        11 => "in".into(),
        12 => "board".into(),
        13 => "link".into(),
        other => format!("type{other:#x}"),
    }
}

/// power rows 图例的行格式（vix 通道模板 role 命中时使用；命名对应
/// vix-editor GUI：pcie-slot=PCIe slot power、12v-input=12V input current、
/// nvvdd-power/current=NVVDD output power/current、total-board=board power）。
fn format_power_row(row: u64, role: &str, r: &Value) -> String {
    let unit = r.get("unit").and_then(Value::as_str).unwrap_or("mW");
    format!(
        "#{row} {role} {}..{}..{} {}",
        r.get("minimum_raw").and_then(Value::as_u64).unwrap_or(0),
        r.get("default_raw").and_then(Value::as_u64).unwrap_or(0),
        r.get("maximum_raw").and_then(Value::as_u64).unwrap_or(0),
        unit
    )
}

/// DRAM 行的 NVML 换算注记：offset 工具（NVML/Linux、MSI Afterburner 的
/// memory 轴）按表值 ×2 呈现（vix extended-fields："NVML = 2 × table MHz"；
/// vix GUI 显示 "6000 MHz · Windows (MSI Afterburner): 3000 MHz"）。
fn render_memory_nvml_scale(clocks: Option<&Value>, lines: &mut Vec<String>) {
    let Some(clocks) = clocks.and_then(Value::as_array) else {
        return;
    };
    let Some(dram) = clocks
        .iter()
        .find(|c| c.get("domain").and_then(Value::as_str) == Some("DRAM"))
    else {
        return;
    };
    if dram.get("range_stored").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let (lo, hi) = (
        dram.get("min_mhz").and_then(Value::as_i64).unwrap_or(0),
        dram.get("max_mhz").and_then(Value::as_i64).unwrap_or(0),
    );
    if (lo, hi) == (0, 0) {
        return;
    }
    lines.push(nvoc_cli_common::color::stylize(
        &format!(
            "    memory offset (NVML x2 scale): {}..{} MHz (table {}..{})",
            lo * 2,
            hi * 2,
            lo,
            hi
        ),
        false,
    ));
}

fn render_rails(t: &Value, lines: &mut Vec<String>) {
    let Some(rails) = t.get("rails").and_then(Value::as_array) else {
        return;
    };
    let items: Vec<String> = rails
        .iter()
        .map(|r| {
            let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
            let boot = r
                .get("boot_voltage_uv")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let sels: Vec<String> = r
                .get("selectors")
                .and_then(Value::as_array)
                .map(|ss| {
                    ss.iter()
                        .filter(|s| s.get("absent").and_then(Value::as_bool) != Some(true))
                        .map(|s| {
                            format!(
                                "{}={}",
                                s.get("role").and_then(Value::as_str).unwrap_or("?"),
                                s.get("raw").and_then(Value::as_u64).unwrap_or(0)
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            format!("{name} boot {}mV {}", boot / 1000, sels.join(" "))
        })
        .collect();
    if !items.is_empty() {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    rails: {}", items.join(" | ")),
            false,
        ));
    }
}

fn render_rel_limits(limits: Option<&Value>, lines: &mut Vec<String>) {
    let Some(limits) = limits.and_then(Value::as_array) else {
        return;
    };
    let items: Vec<String> = limits
        .iter()
        .map(|l| {
            let rail = l.get("rail").and_then(Value::as_str).unwrap_or("?");
            let mv = l.get("max_mv").and_then(Value::as_f64).unwrap_or(0.0);
            match l.get("kind").and_then(Value::as_str) {
                Some(kind) => format!("{rail} REL max {mv:.1} mV ({kind})"),
                None => format!("{rail} REL max {mv:.1} mV"),
            }
        })
        .collect();
    if !items.is_empty() {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    rel limits: {}", items.join(" | ")),
            false,
        ));
    }
}

fn render_vmin(vmins: Option<&Value>, lines: &mut Vec<String>) {
    let Some(vmins) = vmins.and_then(Value::as_array) else {
        return;
    };
    let items: Vec<String> = vmins
        .iter()
        .map(|v| {
            let rail = v.get("rail").and_then(Value::as_str).unwrap_or("?");
            match (
                v.get("value_mv").and_then(Value::as_f64),
                v.get("diagnostic").and_then(Value::as_str),
            ) {
                (Some(mv), _) => format!("{rail} {mv:.1} mV pinned"),
                (None, Some(d)) => format!("{rail} {d}"),
                (None, None) => format!("{rail} unpinned"),
            }
        })
        .collect();
    if !items.is_empty() {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    vmin: {}", items.join(" | ")),
            false,
        ));
    }
}

fn render_clocks(clocks: Option<&Value>, filter_stored: bool, lines: &mut Vec<String>) {
    let Some(clocks) = clocks.and_then(Value::as_array) else {
        return;
    };
    let items: Vec<String> = clocks
        .iter()
        .filter(|c| !filter_stored || c.get("range_stored").and_then(Value::as_bool) == Some(true))
        .map(|c| {
            format!(
                "{} {}..{}",
                c.get("domain").and_then(Value::as_str).unwrap_or("?"),
                c.get("min_mhz").and_then(Value::as_i64).unwrap_or(0),
                c.get("max_mhz").and_then(Value::as_i64).unwrap_or(0)
            )
        })
        .collect();
    if !items.is_empty() {
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    clock ranges (MHz): {}", items.join(", ")),
            false,
        ));
    }
}

fn render_fans_v3(t: &Value, lines: &mut Vec<String>) {
    match t.get("fans") {
        Some(f) if f.get("present").and_then(Value::as_bool) == Some(true) => {
            let channels = f
                .get("channels")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
                .unwrap_or_default();
            let rows: Vec<String> = f
                .get("rows")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|r| r.get("active").and_then(Value::as_bool) == Some(true))
                .map(|r| {
                    format!(
                        "ch{} {}..{}% {}..{}rpm {}Hz",
                        r.get("index").and_then(Value::as_u64).unwrap_or(0),
                        r.get("level_min").and_then(Value::as_u64).unwrap_or(0),
                        r.get("level_max").and_then(Value::as_u64).unwrap_or(0),
                        r.get("rpm_min").and_then(Value::as_u64).unwrap_or(0),
                        r.get("rpm_max").and_then(Value::as_u64).unwrap_or(0),
                        r.get("pwm_frequency_hz")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    )
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    fan channels {:?}: {}", channels, rows.join(" | ")),
                false,
            ));
            let extras: Vec<String> = f
                .get("extra_curves")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|c| {
                    c.get("points")
                        .and_then(Value::as_array)
                        .is_some_and(|p| !p.is_empty())
                })
                .map(|c| {
                    let pts: Vec<String> = c
                        .get("points")
                        .and_then(Value::as_array)
                        .unwrap_or(&Vec::new())
                        .iter()
                        .map(|p| {
                            format!(
                                "{}C/{}rpm",
                                p.get("temp_c").and_then(Value::as_f64).unwrap_or(0.0),
                                p.get("rpm").and_then(Value::as_u64).unwrap_or(0)
                            )
                        })
                        .collect();
                    pts.join(" ")
                })
                .enumerate()
                .map(|(i, pts)| format!("extra{}: {}", i + 1, pts))
                .collect();
            if !extras.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    fan extra curves: {}", extras.join(" | ")),
                    false,
                ));
            }
        }
        Some(f) if f.get("present").and_then(Value::as_bool) == Some(false) => {
            lines.push(nvoc_cli_common::color::stylize(
                "    fan channels: none (no GPU-driven fan)",
                false,
            ));
        }
        Some(f) => {
            if let Some(reason) = f.get("reason").and_then(Value::as_str) {
                lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                    "    fan channels: unsupported ({reason})"
                )));
            }
        }
        None => {}
    }
}

fn push_detail_shared_sections(output: &Value, lines: &mut Vec<String>) {
    let sort_start = lines.len();
    // structural layer (vix 对照补差：镜像链/架构门/按族表解码)
    push_structural_sections(output, lines);
    // power：锚点扫描（CPR 路径）与 BIT 'P'+0x2C PowerBudget（结构化路径）
    // 合并为一行。两条读取器读同一份数据——budget 表占位通道的 avg/peak
    // 哨兵对正是锚点扫描的魔数；一致时缩写为一行互证，不一致即解析器红旗
    // （warning 高亮两侧数值）。仅一侧可用时退回各自的原格式。
    let power_arr = output.get("power").and_then(Value::as_array);
    let anchor_power = power_arr.and_then(|a| a.first());
    let budget = output
        .get("power_v2")
        .and_then(|p2| p2.get("budget"))
        .filter(|b| b.get("cap_entry_power").is_some_and(Value::is_object));
    let watts = |min: Option<f64>, t: f64, l: f64| match min {
        Some(m) => format!("{m:.0}/{t:.0}/{l:.0} W"),
        None => format!("{t:.0}/{l:.0} W"),
    };
    match (anchor_power, budget) {
        (Some(p), Some(b)) => {
            let cap = b.get("cap_entry_power").unwrap();
            let a_min = p.get("min_watt").and_then(Value::as_f64);
            let a_target = p.get("target_watt").and_then(Value::as_f64).unwrap_or(0.0);
            let a_limit = p.get("limit_watt").and_then(Value::as_f64).unwrap_or(0.0);
            let uw = |k: &str| {
                cap.get(k)
                    .and_then(Value::as_u64)
                    .map(|v| v as f64 / 1000.0)
            };
            let (b_min, b_avg, b_peak) = (uw("min_mw"), uw("avg_mw"), uw("peak_mw"));
            let agree = (a_min.is_none() || b_min.is_none() || a_min == b_min)
                && Some(a_target) == b_avg
                && Some(a_limit) == b_peak;
            if agree {
                let platform = p.get("platform").and_then(Value::as_str).unwrap_or("?");
                let copies = if power_arr.is_some_and(|a| a.len() > 1) {
                    format!(", {} copies", power_arr.map(|a| a.len()).unwrap_or(1))
                } else {
                    String::new()
                };
                let slider = p
                    .get("slider")
                    .and_then(|s| s.get("enabled"))
                    .and_then(Value::as_bool);
                let slider_text = match slider {
                    Some(true) => ", slider enabled",
                    Some(false) => ", slider disabled",
                    None => "",
                };
                let range = p
                    .get("adjustment_percent")
                    .and_then(Value::as_str)
                    .map(|r| format!(", range {r}"))
                    .unwrap_or_default();
                let anchor_off = p
                    .get("anchor_offset")
                    .and_then(Value::as_u64)
                    .map(|v| format!("{v:#x}"))
                    .unwrap_or_else(|| "?".into());
                let version = b
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .trim_start_matches("0x")
                    .to_string();
                let cap_entry = b.get("cap_entry").and_then(Value::as_u64).unwrap_or(0);
                // min 缺席侧（mobile 锚点路径）由另一侧补齐。
                let min = a_min.or(b_min);
                lines.push(nvoc_cli_common::color::stylize(
                    &format!(
                        "    power: {}{range}{slider_text} — anchor {anchor_off} ({platform}{copies}) ≡ P+0x2C v{version} cap#{cap_entry}",
                        watts(min, a_target, a_limit)
                    ),
                    false,
                ));
            } else {
                let platform = p.get("platform").and_then(Value::as_str).unwrap_or("?");
                let version = b
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .trim_start_matches("0x")
                    .to_string();
                let cap_entry = b.get("cap_entry").and_then(Value::as_u64).unwrap_or(0);
                lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                    "    power MISMATCH: anchor scan ({platform}) {} vs P+0x2C v{version} cap#{cap_entry} {} — cross-check failed",
                    watts(a_min, a_target, a_limit),
                    watts(b_min, b_avg.unwrap_or(0.0), b_peak.unwrap_or(0.0)),
                )));
            }
        }
        (Some(p), None) => {
            let platform = p.get("platform").and_then(Value::as_str).unwrap_or("?");
            let target = p.get("target_watt").and_then(Value::as_f64).unwrap_or(0.0);
            let limit = p.get("limit_watt").and_then(Value::as_f64).unwrap_or(0.0);
            let slider = p
                .get("slider")
                .and_then(|s| s.get("enabled"))
                .and_then(Value::as_bool);
            let slider_text = match slider {
                Some(true) => "enabled",
                Some(false) => "disabled",
                None => "not found",
            };
            let min_w = p
                .get("min_watt")
                .and_then(Value::as_f64)
                .map(|v| format!("min {v:.0} W, "));
            let range = p
                .get("adjustment_percent")
                .and_then(Value::as_str)
                .map(|r| format!(", range {r}"))
                .unwrap_or_default();
            let count = if power_arr.is_some_and(|a| a.len() > 1) {
                format!(" ({} copies)", power_arr.map(|a| a.len()).unwrap_or(1))
            } else {
                String::new()
            };
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "    power ({platform}): {}target {target:.0} W, limit {limit:.0} W{range}, slider {slider_text}{count}",
                    min_w.unwrap_or_default()
                ),
                false,
            ));
        }
        (None, Some(b)) => {
            let cap = b.get("cap_entry_power").unwrap();
            let fmt = |k: &str| {
                cap.get(k)
                    .and_then(Value::as_u64)
                    .map(|v| v / 1000)
                    .unwrap_or(0)
            };
            let version = b
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .trim_start_matches("0x")
                .to_string();
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "    power budget (P+0x2C v{version}): cap entry {} = min {} W / avg {} W / peak {} W",
                    b.get("cap_entry").and_then(Value::as_u64).unwrap_or(0),
                    fmt("min_mw"),
                    fmt("avg_mw"),
                    fmt("peak_mw")
                ),
                false,
            ));
        }
        (None, None) => {}
    }
    // thermal/fan: documented BIT 'P' table (nouveau semantics). The
    // absence note only prints when no modern-slot fan/thermal data rendered
    // above — on Pascal those slots carry the actionable tables instead.
    if let Some(thermal) = output.get("thermal_fan") {
        match thermal.get("present").and_then(Value::as_bool) {
            Some(true) => {
                let mode = thermal
                    .get("fan_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let mut text = format!("    thermal/fan: mode {mode}");
                if let (Some(min), Some(max)) = (
                    thermal.get("linear_min_temp_c").and_then(Value::as_i64),
                    thermal.get("linear_max_temp_c").and_then(Value::as_i64),
                ) {
                    text.push_str(&format!(", linear {min}..{max} C"));
                }
                if let (Some(min), Some(max)) = (
                    thermal.get("min_duty_percent").and_then(Value::as_i64),
                    thermal.get("max_duty_percent").and_then(Value::as_i64),
                ) {
                    text.push_str(&format!(", duty {min}..{max}%"));
                }
                if let Some(trips) = thermal.get("trips").and_then(Value::as_array) {
                    let items: Vec<String> = trips
                        .iter()
                        .map(|t| {
                            format!(
                                "{}C->{}%",
                                t.get("temp_c").and_then(Value::as_i64).unwrap_or(0),
                                t.get("duty_percent").and_then(Value::as_i64).unwrap_or(0)
                            )
                        })
                        .collect();
                    if !items.is_empty() {
                        text.push_str(&format!(", trips: {}", items.join(", ")));
                    }
                }
                lines.push(nvoc_cli_common::color::stylize(&text, false));
            }
            _ => {
                let has_modern_fan_data = output
                    .get("fan_cooler")
                    .and_then(|f| f.get("present"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if !has_modern_fan_data
                    && let Some(note) = thermal.get("note").and_then(Value::as_str)
                {
                    lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                        "    thermal/fan: {note}"
                    )));
                }
            }
        }
    }
    // display map (DCB connector table)
    if let Some(dcb) = output.get("dcb")
        && let Some(pads) = dcb.get("pads").and_then(Value::as_array)
    {
        let items: Vec<String> = pads
            .iter()
            .map(|p| {
                let pad = p.get("pad").and_then(Value::as_str).unwrap_or("?");
                let roles = p
                    .get("roles")
                    .and_then(Value::as_array)
                    .map(|rs| {
                        rs.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("+")
                    })
                    .unwrap_or_default();
                format!("{pad}={roles}")
            })
            .collect();
        if !items.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    display: {}", items.join(" ")),
                false,
            ));
        }
    }
    // General (GPU-Z aligned: version / build date / message / board)
    if let Some(id) = output.get("identity") {
        let mut bits: Vec<String> = Vec::new();
        for key in ["version", "build_date", "message", "board_id"] {
            if let Some(v) = id.get(key).and_then(Value::as_str) {
                bits.push(v.to_string());
            }
        }
        if !bits.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    bios: {}", bits.join(" | ")),
                false,
            ));
        }
    }
    // memory subsystem summary (clock ranges / timings / variants)
    if let Some(mem) = output.get("memory") {
        let mut parts: Vec<String> = Vec::new();
        if let Some(info) = mem
            .get("info")
            .and_then(|i| i.get("variants"))
            .and_then(Value::as_array)
        {
            let mut types = std::collections::BTreeSet::new();
            let mut vendors = std::collections::BTreeSet::new();
            for v in info {
                if let Some(t) = v.get("type").and_then(Value::as_str) {
                    types.insert(t.to_string());
                }
                if let Some(vd) = v.get("vendor").and_then(Value::as_str) {
                    vendors.insert(vd.to_string());
                }
            }
            if !types.is_empty() {
                parts.push(format!(
                    "{} {}",
                    types.iter().cloned().collect::<Vec<_>>().join("/"),
                    vendors.iter().cloned().collect::<Vec<_>>().join("/")
                ));
            }
        }
        if let Some(clock) = mem
            .get("clock_table")
            .and_then(|c| c.get("freq_ranges"))
            .and_then(Value::as_array)
        {
            parts.push(format!("{} freq ranges", clock.len()));
        }
        if let Some(tw) = mem
            .get("tweak_table")
            .and_then(|t| t.get("timing_sets"))
            .and_then(Value::as_i64)
        {
            parts.push(format!("{tw} timing sets"));
        }
        if !parts.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    memory: {}", parts.join(", ")),
                false,
            ));
        }
        // strap variants 聚合渲染：按 (type/vendor/density/org) 分组，列出
        // 各组的 strap 槽（同组热/功率参数一致才省略）。
        if let Some(variants) = mem
            .get("info")
            .and_then(|i| i.get("variants"))
            .and_then(Value::as_array)
            && !variants.is_empty()
        {
            let mut groups: Vec<(String, Vec<String>)> = Vec::new();
            let mut extras = std::collections::BTreeSet::new();
            for v in variants {
                let mut key_bits: Vec<String> = Vec::new();
                for key in ["type", "vendor", "density"] {
                    match v.get(key).and_then(Value::as_str) {
                        Some(val) if val != "?" => key_bits.push(val.to_string()),
                        _ => key_bits.push("?".into()),
                    }
                }
                let org = v.get("org").and_then(Value::as_u64);
                let org_part = org
                    .map(|o| format!("org x{o:02x}"))
                    .unwrap_or_else(|| "org ?".into());
                key_bits.push(org_part);
                let key = key_bits.join(" ");
                let strap = v
                    .get("strap")
                    .and_then(Value::as_u64)
                    .map(|st| format!("x{st:02x}"))
                    .unwrap_or_else(|| "?".into());
                // 同键下其它字段不一致就记进 extras（不省略差异）。
                let tj = v.get("tj_max_fan_limit").and_then(Value::as_u64);
                if let Some(tj) = tj
                    && tj != 0
                {
                    extras.insert(format!("tjmax {tj}C"));
                }
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, straps)) => straps.push(strap),
                    None => groups.push((key, vec![strap])),
                }
            }
            // type/vendor 双缺的组是未填的 strap 占位槽，不渲染。
            groups.retain(|(k, _)| !k.starts_with("? ? "));
            if !groups.is_empty() {
                let items: Vec<String> = groups
                    .iter()
                    .map(|(key, straps)| {
                        let shown = straps.iter().take(8).cloned().collect::<Vec<_>>().join(",");
                        let more = if straps.len() > 8 {
                            format!(",…(+{})", straps.len() - 8)
                        } else {
                            String::new()
                        };
                        format!("{key} [{shown}{more}]")
                    })
                    .collect();
                let extra_note = if extras.is_empty() {
                    String::new()
                } else {
                    format!(
                        " | {}",
                        extras.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                };
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    memory variants: {}{}", items.join(" | "), extra_note),
                    false,
                ));
            }
        }
        // 时序表形状（时序位段 hexpat 亦未命名，只给结构）。
        if let Some(tw) = mem.get("tweak_table").and_then(Value::as_object)
            && tw.get("timing_sets").is_some()
        {
            let base = tw
                .get("base_entry_size")
                .and_then(Value::as_u64)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into());
            let ext = tw
                .get("extended_entry_size")
                .and_then(Value::as_u64)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into());
            let eec = tw
                .get("extended_entry_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let ver = tw.get("version").and_then(Value::as_str).unwrap_or("?");
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "    memory timings: v{ver}, {} sets (base {base}B + {eec}×{ext}B reg; bitfields decoded below, full bytes in JSON)",
                    tw.get("timing_sets").and_then(Value::as_i64).unwrap_or(0)
                ),
                false,
            ));
        }
        // v0x20 位段参数摘要：按 (CL, p44, p45, CCDL, CCDS) 元组聚合 set。
        if let Some(decoded) = mem
            .get("tweak_table")
            .and_then(|t| t.get("decoded"))
            .and_then(Value::as_array)
            && !decoded.is_empty()
        {
            let key = |d: &Value| {
                (
                    d.get("cl_ramtype2").and_then(Value::as_u64).unwrap_or(0),
                    d.get("param44").and_then(Value::as_u64).unwrap_or(0),
                    d.get("param45").and_then(Value::as_u64).unwrap_or(0),
                    d.get("ccdl").and_then(Value::as_u64).unwrap_or(0),
                    d.get("ccds").and_then(Value::as_u64).unwrap_or(0),
                )
            };
            /// (CL, p44, p45, CCDL, CCDS) 参数元组 → 覆盖的 set 下标段。
            type TimingKey = ((u64, u64, u64, u64, u64), Vec<usize>);
            let mut groups: Vec<TimingKey> = Vec::new();
            for (i, d) in decoded.iter().enumerate() {
                let k = key(d);
                match groups.last_mut() {
                    Some((gk, idx)) if *gk == k => idx.push(i),
                    _ => groups.push((k, vec![i])),
                }
            }
            let items: Vec<String> = groups
                .iter()
                .map(|((cl, p44, p45, ccdl, ccds), idx)| {
                    let span = if idx.len() == 1 {
                        format!("#{}", idx[0])
                    } else {
                        format!("#{}..#{}", idx[0], idx[idx.len() - 1])
                    };
                    format!("{span}: CL{cl} p44={p44} p45={p45} CCDL{ccdl}/CCDS{ccds}")
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    memory timing params: {}", items.join(" | ")),
                false,
            ));
        }
        // shunt/感测倍率 ≠ 1.0 的通道（Q4.12；4090 CC07B 连接器 1.3333、
        // 4070S 2.0、3090 NVVDD 相位 4.0 实测）。1.0 通道不列。
        if let Some(scales) = output
            .get("structural")
            .and_then(|t| t.get("tables_v3"))
            .and_then(|t| t.get("power_channels"))
            .and_then(|c| c.get("channel_scaling"))
            .and_then(Value::as_array)
            && let Some(roles) = output
                .get("structural")
                .and_then(|t| t.get("tables_v3"))
                .and_then(|t| t.get("power_channels"))
                .and_then(|c| c.get("roles"))
                .and_then(Value::as_array)
        {
            // 只列 12V 感测族（voltage_uv == 12,000,000 且有模板 role）：
            // 该族的 bytes 6..8 = 感测倍率（shunt 校准）。其它 class 的同位
            // 置字节语义未证实（vix 观察 class-2 求和通道在 bytes 6..7 存
            // 别的值），不标 shunt。
            let items: Vec<String> = scales
                .iter()
                .zip(roles.iter())
                .enumerate()
                .filter_map(|(i, (sc, role))| {
                    if sc.get("voltage_uv").and_then(Value::as_u64) != Some(12_000_000) {
                        return None;
                    }
                    let role = role.as_str().unwrap_or("");
                    if role.is_empty() {
                        return None;
                    }
                    let scale = sc.get("scale").and_then(Value::as_f64)?;
                    if (scale - 1.0).abs() < 1e-4 {
                        return None;
                    }
                    Some(format!("ch{i} {role} x{scale:.4}"))
                })
                .collect();
            if !items.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    shunt/sense scales: {}", items.join(" | ")),
                    false,
                ));
            }
        }
        // range → tweak set 映射（strap 副本 byte 0 的关联键）。各 strap 同
        // 组时折叠为 #k；0xFF（无）折叠为 none。
        if let Some(ranges) = mem
            .get("clock_table")
            .and_then(|c| c.get("freq_ranges"))
            .and_then(Value::as_array)
        {
            let items: Vec<String> = ranges
                .iter()
                .filter(|r| {
                    // 空槽 range（0..0）不渲染。
                    !matches!(
                        (
                            r.get("freq_min_raw").and_then(Value::as_i64),
                            r.get("freq_max_raw").and_then(Value::as_i64),
                        ),
                        (Some(0), Some(0))
                    )
                })
                .map(|r| {
                    let lo = r.get("freq_min_raw").and_then(Value::as_i64).unwrap_or(0);
                    let hi = r.get("freq_max_raw").and_then(Value::as_i64).unwrap_or(0);
                    let idxs: Vec<u8> = r
                        .get("tweak_indices")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_u64)
                                .map(|v| v as u8)
                                .collect()
                        })
                        .unwrap_or_default();
                    let label = if idxs.is_empty() {
                        "?".into()
                    } else {
                        let uniq: std::collections::BTreeSet<u8> = idxs.iter().copied().collect();
                        if uniq.len() == 1 {
                            match uniq.iter().next().unwrap() {
                                0xFF => "none".into(),
                                k => format!("#{k}"),
                            }
                        } else {
                            format!(
                                "[{}]",
                                idxs.iter()
                                    .map(|k| if *k == 0xFF {
                                        "-".into()
                                    } else {
                                        format!("{k}")
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )
                        }
                    };
                    format!("{lo}..{hi}→{label}")
                })
                .collect();
            if !items.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    memory timing map: {}", items.join("  ")),
                    false,
                ));
            }
        }
    }
    // Thermal limits (GPU-Z aligned: rated / maximum from ThermalPolicy)
    if let Some(tp) = output.get("thermal_policy")
        && tp.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(entries) = tp.get("entries").and_then(Value::as_array)
    {
        let temps: Vec<String> = entries
            .iter()
            .filter(|e| e.get("enabled").and_then(Value::as_bool) == Some(true))
            .map(|e| {
                format!(
                    "{}..{}C",
                    e.get("temp_a_c").and_then(Value::as_i64).unwrap_or(0),
                    e.get("temp_c_c").and_then(Value::as_i64).unwrap_or(0)
                )
            })
            .collect();
        if !temps.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    thermal limits: {}", temps.join(", ")),
                false,
            ));
        }
    }
    // Fan curve (FanPolicy tail: (temp °C<<5, RPM) points, ascending)
    if let Some(fp) = output.get("fan_policy")
        && fp.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(curves) = fp.get("curves").and_then(Value::as_array)
        && let Some(curve) = curves.first().and_then(Value::as_array)
        && !curve.is_empty()
    {
        let pts: Vec<String> = curve
            .iter()
            .map(|p| {
                format!(
                    "{}C->{}rpm",
                    p.get("temp_c").and_then(Value::as_f64).unwrap_or(0.0),
                    p.get("rpm").and_then(Value::as_i64).unwrap_or(0)
                )
            })
            .collect();
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    fan curve: {}", pts.join(", ")),
            false,
        ));
    }
    // Fan cooler envelope (per-cooler max duty / max RPM; count = cooler
    // controllers the ROM provisions — NVAPI cooler Count same semantics).
    // Skipped when the structural layer already rendered the same cooler
    // table with its full channel rows (3x families): the old line is a
    // strict subset (duty/max-rpm) of the structural fan channels row.
    let structural_fans_rendered = output
        .get("structural")
        .and_then(|st| st.get("tables_v3"))
        .and_then(|t| t.get("fans"))
        .and_then(|f| f.get("present"))
        .and_then(Value::as_bool)
        == Some(true);
    if !structural_fans_rendered
        && let Some(fc) = output.get("fan_cooler")
        && fc.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(coolers) = fc.get("coolers").and_then(Value::as_array)
    {
        let count = fc
            .get("cooler_count")
            .and_then(Value::as_i64)
            .unwrap_or(coolers.len() as i64);
        let items: Vec<String> = coolers
            .iter()
            .map(|c| {
                let duty = c.get("max_duty_percent").and_then(Value::as_i64);
                let rpm = c.get("max_rpm").and_then(Value::as_i64);
                format!(
                    "duty {}%, max rpm {}",
                    duty.map(|d| d.to_string()).unwrap_or_else(|| "?".into()),
                    rpm.map(|r| r.to_string()).unwrap_or_else(|| "?".into())
                )
            })
            .collect();
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    fan coolers ({}): {}", count, items.join("; ")),
            false,
        ));
    }
    // Falcon ucode inventory (PMU/devinit/security-license firmware)
    if let Some(falcon) = output.get("falcon")
        && falcon.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(entries) = falcon.get("entries").and_then(Value::as_array)
    {
        let items: Vec<String> = entries
            .iter()
            .filter(|e| e.get("desc").is_some_and(|d| !d.is_null()))
            .map(|e| {
                let app = e.get("application").and_then(Value::as_str).unwrap_or("?");
                let tgt = e.get("target").and_then(Value::as_str).unwrap_or("?");
                let d = e.get("desc").filter(|d| !d.is_null());
                let stored = d
                    .and_then(|d| d.get("stored_size"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                // FWSEC 族（V2/V3 描述符）追加 IMEM/DMEM 几何与签名位图。
                let geom = d
                    .and_then(|d| {
                        let ver = d.get("version").and_then(Value::as_u64)?;
                        let imem = d.get("imem_load_size").and_then(Value::as_u64)?;
                        let dmem = d.get("dmem_load_size").and_then(Value::as_u64)?;
                        let sig = d.get("signature_count").and_then(Value::as_u64)?;
                        let sigv = d.get("signature_versions").and_then(Value::as_u64)?;
                        Some(if ver >= 2 && sig > 0 {
                            format!(" imem {imem:#x} dmem {dmem:#x} sig {sig}@{sigv:#x}")
                        } else if ver >= 2 {
                            format!(" imem {imem:#x} dmem {dmem:#x}")
                        } else {
                            String::new()
                        })
                    })
                    .unwrap_or_default();
                format!("{app}/{tgt} {stored:#x}{geom}")
            })
            .collect();
        if !items.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    falcon ucode: {}", items.join(", ")),
                false,
            ));
        }
    }
    // NVGI root header (programmer dumps only)
    if let Some(nvgi) = output.get("nvgi")
        && !nvgi.is_null()
    {
        let sub = nvgi
            .get("xve_subsystem_id")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let vend = nvgi
            .get("xve_sub_vendor")
            .and_then(Value::as_str)
            .unwrap_or("?");
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    NVGI: version {}, total {:#x}, subsystem {}:{}",
                nvgi.get("version").and_then(Value::as_i64).unwrap_or(0),
                nvgi.get("total_data_size")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                vend,
                sub
            ),
            false,
        ));
    }
    // PERF_PTR slot map — one line per non-zero pointer: slot offset, table
    // name, rebased absolute file offset, and the target's (ver, hdr, len,
    // cnt) header. "header invalid" = the target bytes don't decode as a
    // table header (pointer target outside any known layout).
    if let Some(pp) = output.get("perf_pointers")
        && let Some(slots) = pp.get("slots").and_then(Value::as_array)
        && !slots.is_empty()
    {
        let plausible_count = slots
            .iter()
            .filter(|s| s.get("plausible").and_then(Value::as_bool) == Some(true))
            .count();
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    perf ptrs: {} non-zero, {} plausible tables",
                slots.len(),
                plausible_count
            ),
            false,
        ));
        for s in slots {
            let name = s.get("name").and_then(Value::as_str).unwrap_or("?");
            let slot = s.get("slot_offset").and_then(Value::as_i64).unwrap_or(0);
            let abs = s.get("abs_offset").and_then(Value::as_i64).unwrap_or(0);
            let plausible = s.get("plausible").and_then(Value::as_bool) == Some(true);
            let header = s
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or("?? ?? ?? ??");
            let bytes: Vec<&str> = header.split(' ').collect();
            let detail = if plausible && bytes.len() == 4 {
                // header string is hex; ver keeps hex, the rest print decimal
                let byte = |i: usize| u8::from_str_radix(bytes[i], 16).unwrap_or(0);
                format!(
                    "ver=0x{:02X} hdr={} len={} cnt={}",
                    byte(0),
                    byte(1),
                    byte(2),
                    byte(3)
                )
            } else {
                format!("header invalid ({header})")
            };
            lines.push(nvoc_cli_common::color::stylize(
                &format!("      +0x{slot:04X} {name:<18} -> 0x{abs:X}  {detail}"),
                false,
            ));
        }
    }
    // BIT 'i' InternalUse (board id / compile date / SKU / project)
    if let Some(iu) = output.get("internal_use")
        && iu.get("present").and_then(Value::as_bool) == Some(true)
    {
        let mut bits: Vec<String> = Vec::new();
        for key in ["version", "project", "build_date"] {
            if let Some(v) = iu.get(key).and_then(Value::as_str) {
                bits.push(v.to_string());
            }
        }
        if !bits.is_empty() {
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    internal: {}", bits.join(" | ")),
                false,
            ));
        }
    }
    // Clock States (perf table v0x40: per-Pstate domain clocks, Kepler/Maxwell)
    if let Some(cs) = output.get("clock_states")
        && cs.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(states) = cs.get("states").and_then(Value::as_array)
    {
        let names: Vec<&str> = cs
            .get("domain_names")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for st in states {
            let ps = st.get("pstate").and_then(Value::as_str).unwrap_or("?");
            let items: Vec<String> = st
                .get("domains_mhz")
                .and_then(Value::as_array)
                .map(|d| {
                    d.iter()
                        .enumerate()
                        .map(|(k, v)| {
                            let name = names.get(k).copied().unwrap_or("?");
                            format!("{name} {}", v.as_u64().unwrap_or(0))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !items.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    clock states {ps}: {}", items.join(" ")),
                    false,
                ));
            }
        }
    }
    // Boost States (P+0x30 table: per-Pstate domain min/max, half-MHz)
    if let Some(bs) = output.get("boost_states")
        && bs.get("present").and_then(Value::as_bool) == Some(true)
        && let Some(states) = bs.get("states").and_then(Value::as_array)
    {
        for st in states {
            let ps = st.get("pstate").and_then(Value::as_str).unwrap_or("?");
            let items: Vec<String> = st
                .get("ranges")
                .and_then(Value::as_array)
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            format!(
                                "{} {:.1}-{:.1}",
                                r.get("domain").and_then(Value::as_str).unwrap_or("?"),
                                r.get("min_mhz").and_then(Value::as_f64).unwrap_or(0.0),
                                r.get("max_mhz").and_then(Value::as_f64).unwrap_or(0.0)
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !items.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    boost states {ps}: {}", items.join(" ")),
                    false,
                ));
            }
        }
    }
    // RFFS/RFRD flash directory (NVGI full dumps, Turing+)
    if let Some(fd) = output.get("flash_directory")
        && fd.get("present").and_then(Value::as_bool) == Some(true)
    {
        let pci_ok = fd.get("pci_rom_magic_ok").and_then(Value::as_bool);
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    flash dir: RFFS v{}, romdir v{}, PCI ROM @ {:#x} ({})",
                fd.get("rffs_version").and_then(Value::as_i64).unwrap_or(0),
                fd.get("rom_dir_version")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                fd.get("pci_option_rom_offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                match pci_ok {
                    Some(true) => "55AA ok",
                    Some(false) => "magic mismatch",
                    None => "unchecked",
                }
            ),
            false,
        ));
    }
    for warning in output
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(w) = warning.as_str() {
            lines.push(nvoc_cli_common::color::stylize_warning(&format!(
                "    warning: {w}"
            )));
        }
    }
    // DCB 指针子表族（envytools nvbios 移植）：I2C 端口/GPIO 命名线/
    // EXTDEV 外设/CONN。
    if let Some(ext) = output.get("dcb_ext") {
        if let Some(i2c) = ext.get("i2c") {
            let ports: Vec<String> = i2c
                .get("ports")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|p| p.get("kind").and_then(Value::as_str) != Some("UNUSED"))
                        .map(|p| {
                            let idx = p.get("index").and_then(Value::as_u64).unwrap_or(0);
                            let kind = p.get("kind").and_then(Value::as_str).unwrap_or("?");
                            match p.get("kind").and_then(Value::as_str) {
                                Some("PMGR") => {
                                    let drive = p.get("drive").and_then(Value::as_u64);
                                    let aux = p.get("auxch").and_then(Value::as_u64);
                                    let d =
                                        drive.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                                    let x =
                                        aux.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                                    format!("p{idx}:PMGR(drv {d},aux {x})")
                                }
                                _ => format!("p{idx}:{kind}"),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !ports.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    dcb i2c ports: {}", ports.join(", ")),
                    false,
                ));
            }
        }
        if let Some(gpio) = ext.get("gpio")
            && let Some(gpio_lines) = gpio.get("lines").and_then(Value::as_array)
        {
            let named: Vec<String> = gpio_lines
                .iter()
                .filter(|l| {
                    !matches!(
                        l.get("name").and_then(Value::as_str),
                        Some("UNUSED") | Some("UNKNOWN")
                    )
                })
                .map(|l| {
                    format!(
                        "{}:{}",
                        l.get("line").and_then(Value::as_u64).unwrap_or(0),
                        l.get("name").and_then(Value::as_str).unwrap_or("?")
                    )
                })
                .collect();
            if !named.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    dcb gpio: {}", named.join(", ")),
                    false,
                ));
            }
        }
        if let Some(extdev) = ext.get("extdev")
            && let Some(devs) = extdev.get("devices").and_then(Value::as_array)
            && !devs.is_empty()
        {
            let items: Vec<String> = devs
                .iter()
                .map(|d| {
                    format!(
                        "{}@{}",
                        d.get("name").and_then(Value::as_str).unwrap_or("?"),
                        d.get("address").and_then(Value::as_str).unwrap_or("?")
                    )
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    dcb extdev: {}", items.join(", ")),
                false,
            ));
        }
        if let Some(conn) = ext.get("conn")
            && let Some(entries) = conn.get("entries").and_then(Value::as_array)
            && !entries.is_empty()
        {
            let items: Vec<String> = entries
                .iter()
                .map(|e| {
                    let hpd = e
                        .get("hpd")
                        .and_then(Value::as_u64)
                        .map(|v| format!("/HPD{v}"))
                        .unwrap_or_default();
                    format!(
                        "{}{}",
                        e.get("type").and_then(Value::as_str).unwrap_or("?"),
                        hpd
                    )
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    dcb conn: {}", items.join(", ")),
                false,
            ));
        }
    }
    // BIT token 级子表：目录 + 'M' 内存类型 + NVINIT 脚本指针。
    if let Some(bt) = output.get("bit_tokens") {
        if let Some(dir) = bt.get("directory").and_then(Value::as_array)
            && !dir.is_empty()
        {
            let items: Vec<String> = dir
                .iter()
                .map(|t| {
                    format!(
                        "{}{}({:#04x})",
                        t.get("id").and_then(Value::as_str).unwrap_or("?"),
                        t.get("ver").and_then(Value::as_u64).unwrap_or(0),
                        t.get("len").and_then(Value::as_u64).unwrap_or(0)
                    )
                })
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    bit tokens: {}", items.join(" ")),
                false,
            ));
        }
        if let Some(mem) = bt.get("mem")
            && let Some(tt) = mem.get("type_table")
        {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for e in tt
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = e
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                match counts.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((name, 1)),
                }
            }
            let items: Vec<String> = counts
                .into_iter()
                .map(|(n, c)| format!("{n}x{c}"))
                .collect();
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "    bit 'M': groups {}, ramcfg types {}",
                    mem.get("group_count").and_then(Value::as_u64).unwrap_or(0),
                    items.join(", ")
                ),
                false,
            ));
        }
        if let Some(nvinit) = bt.get("nvinit")
            && let Some(slots) = nvinit.get("slots").and_then(Value::as_array)
        {
            let items: Vec<String> = slots
                .iter()
                .filter(|s| s.get("ptr").and_then(Value::as_u64).unwrap_or(0) != 0)
                .map(|s| {
                    format!(
                        "{}={:#06x}",
                        s.get("slot").and_then(Value::as_str).unwrap_or("?"),
                        s.get("ptr").and_then(Value::as_u64).unwrap_or(0)
                    )
                })
                .collect();
            if !items.is_empty() {
                lines.push(nvoc_cli_common::color::stylize(
                    &format!("    nvinit (BIT 'I' u16 ptrs): {}", items.join(", ")),
                    false,
                ));
            }
        }
    }
    // 段名字典序：本函数追加的各段行按行首段名（首个单词，大小写不敏感）
    // 稳定排序——同族段保持插入序（power policy → power rows、memory →
    // memory variants），warning 行一律置尾。函数起点之前的行（VP 阶梯
    // 表/pstate marks/dual bank）不参与。深缩进续行黏附其组头（perf ptrs
    // 槽表），排序键取自剥离 ANSI 序列后的纯文本（着色模式下 stylize 会
    // 逐 token 包裹）。
    fn plain_text(l: &str) -> String {
        let mut out = String::with_capacity(l.len());
        let mut chars = l.chars();
        while let Some(c) = chars.next() {
            if c == '' {
                for e in chars.by_ref() {
                    if e.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
    let tail: Vec<String> = lines.drain(sort_start..).collect();
    let header_indent = tail
        .first()
        .map(|l| {
            let plain = plain_text(l);
            plain.len() - plain.trim_start().len()
        })
        .unwrap_or(0);
    let mut groups: Vec<(bool, String, Vec<String>)> = Vec::new();
    for l in tail {
        let plain = plain_text(&l);
        let indent = plain.len() - plain.trim_start().len();
        let keyword = plain
            .trim_start()
            .split([':', ' '])
            .next()
            .unwrap_or("")
            .to_lowercase();
        let is_warning = plain.contains("warning:");
        if indent > header_indent
            && let Some(last) = groups.last_mut()
        {
            last.2.push(l);
            continue;
        }
        groups.push((is_warning, keyword, vec![l]));
    }
    groups.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (_, _, group_lines) in groups {
        lines.extend(group_lines);
    }
}

/// Per-segment table header for the private V/F dump. Units are declared
/// once on the segment separator (`V=mV f=MHz`), the column row is
/// `ID | V_cur | V_def | F_cur | F_def | V_ext<k> | f_ext<k> … | mode |
/// offset`, and present EXT columns get a roster-attribution legend line
/// (slot → curve domain without its own main record block).
fn push_pvfp_header(
    lines: &mut Vec<String>,
    seg: Option<&Value>,
    ext_slots: &[usize],
    roster: Option<&[String]>,
    volt_off: bool,
) {
    let units = "[V=mV f=MHz; mode: 0=freq, 1=raw; offset=effect MHz]";
    let sep = match seg {
        Some(seg) => {
            let corrected = seg
                .get("freq_scale_corrected")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let corrected_tag = if corrected { " (corrected)" } else { "" };
            format!(
                "    --- bank{} {} {}{} (index {}..{}) --- {units}",
                seg.get("bank").and_then(Value::as_i64).unwrap_or_default(),
                seg.get("domain").and_then(Value::as_str).unwrap_or("?"),
                seg.get("kind").and_then(Value::as_str).unwrap_or("?"),
                corrected_tag,
                field_text(seg, "start_index"),
                field_text(seg, "end_index"),
            )
        }
        None => format!("    --- points --- {units}"),
    };
    lines.push(nvoc_cli_common::color::stylize(&sep, false));

    let mut cols: Vec<(String, usize)> = vec![
        ("ID".to_string(), PVFP_W_ID),
        ("V_cur".to_string(), PVFP_W_VAL),
        ("V_def".to_string(), PVFP_W_VAL),
        ("F_cur".to_string(), PVFP_W_VAL),
        ("F_def".to_string(), PVFP_W_VAL),
    ];
    for k in ext_slots {
        cols.push((format!("V_ext{k}"), PVFP_W_VAL));
        cols.push((format!("f_ext{k}"), PVFP_W_VAL));
    }
    if volt_off {
        cols.push(("V_off".to_string(), PVFP_W_VAL));
    }
    cols.push(("mode".to_string(), PVFP_W_MODE));
    cols.push(("offset".to_string(), PVFP_W_VAL));
    let header = cols
        .iter()
        .map(|(h, w)| format!("{h:>w$}", w = w))
        .collect::<Vec<_>>()
        .join(PVFP_SEP);
    lines.push(nvoc_cli_common::color::stylize(
        &format!("    {header}"),
        false,
    ));
    if !ext_slots.is_empty() {
        // EXT-slot attribution: pstate-bin segments have LIVE-CERTIFIED
        // per-segment semantics (2026-09-06, FreqsEnum cross-match) that
        // override the roster — the mem bins' ext0 column carries the
        // PCIe gen mode per pstate (P8→1, P5→2, P3/P2/P0→3), the disp
        // bins' ext0 column matches Hub's frequency table point-for-point
        // (display clock follows hub). vf_curve segments keep the
        // roster-minus-owner attribution (XBAR current — Ada A/B).
        let seg_kind = seg.and_then(|s| s.get("kind")).and_then(Value::as_str);
        let seg_domain = seg.and_then(|s| s.get("domain")).and_then(Value::as_str);
        let bin_ext_name: Option<&str> = match (seg_kind, seg_domain) {
            (Some("pstate_bins"), Some("mem")) => Some("PCIe gen"),
            (Some("pstate_bins"), Some("disp")) => Some("Hub"),
            _ => None,
        };
        let legend: Vec<String> = ext_slots
            .iter()
            .map(|&k| {
                if k == 0
                    && let Some(nm) = bin_ext_name
                {
                    return format!("ext{k}={nm}");
                }
                match roster.and_then(|r| r.get(k)) {
                    Some(nm) => format!("ext{k}={nm}"),
                    None => format!("ext{k}=?"),
                }
            })
            .collect();
        lines.push(nvoc_cli_common::color::stylize(
            &format!("    ext: {}", legend.join(" ")),
            false,
        ));
    }
}

/// Human format for `get-private-vftable` — one aligned bare-number table
/// per segment (`ID | V_cur | V_def | F_cur | F_def | V_ext<k> | f_ext<k> …
/// | mode | offset`; units declared once on the segment separator) plus the
/// extended-currents roster summary and the segment list.
pub(super) fn format_private_vfp_output(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(object) = output.as_object() else {
        return format_value_block(output, 1);
    };
    let segments = object.get("segments").and_then(Value::as_array);
    let points = object.get("points").and_then(Value::as_array);
    // extended-section per-(owner, slot) current-pair collector — filled
    // while the row loop walks the points (it already resolves each
    // point's owning segment for the row grouping). Value tuple:
    // (count, f_min, f_max, v_min, v_max).
    type ExtCurrentStats = std::collections::BTreeMap<(String, usize), (usize, f64, f64, f64, f64)>;
    let mut ext_stats = ExtCurrentStats::new();
    // roster attribution (EXT slot → curve domain): the pool minus every
    // domain owning a main vf_curve block, which is what the driver's
    // ext-slot packing follows. Read the relation core derived from the
    // WHOLE table (`fabric_relations.roster`) — never re-derive it from
    // this view's `segments`, which is --bank/--domain filtered: a domain
    // filtered out of the view would reappear in the roster and shift
    // every slot label after it. `None` = no block at all (an older build,
    // or a failed read): the relation is unknown and the slots stay
    // unnamed rather than guessed. Hoisted so the per-segment table legend
    // and the bottom summary share one attribution.
    let roster: Option<Vec<String>> = object
        .get("fabric_relations")
        .and_then(|f| f.get("roster"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_uppercase)
                .collect()
        });

    if let Some(points) = points {
        lines.push(format!(
            "  {}",
            nvoc_cli_common::color::stylize_title("V-F Points")
        ));
        // Table layout: one header + aligned bare-number rows per segment
        // (units declared once on the separator line). The column set —
        // which EXT slots are present, whether the per-point volt-offset
        // column is needed — must be known BEFORE a header prints, so
        // pre-scan the points first.
        let matches_seg = |point: &Value, seg: &Value| -> bool {
            let (Some(seg_bank), Some(seg_type)) = (
                seg.get("bank").and_then(Value::as_i64),
                seg.get("type").and_then(Value::as_i64),
            ) else {
                return false;
            };
            let bank_matches = point.get("bank").and_then(Value::as_i64) == Some(seg_bank);
            let type_matches = point.get("type").and_then(Value::as_i64) == Some(seg_type);
            let index = point.get("index").and_then(Value::as_i64).unwrap_or(-1);
            let start = seg.get("start_index").and_then(Value::as_i64).unwrap_or(-1);
            let end = seg.get("end_index").and_then(Value::as_i64).unwrap_or(-1);
            bank_matches && type_matches && start <= index && index <= end
        };
        let seg_of = |point: &Value| -> Option<usize> {
            segments.and_then(|segs| segs.iter().position(|seg| matches_seg(point, seg)))
        };
        let mut group_ext: std::collections::HashMap<
            Option<usize>,
            std::collections::BTreeSet<usize>,
        > = std::collections::HashMap::new();
        let mut group_volt_off: std::collections::HashSet<Option<usize>> =
            std::collections::HashSet::new();
        for point in points {
            let group = seg_of(point);
            if let Some(dc) = point.get("domain_currents").and_then(Value::as_object) {
                for k in dc
                    .keys()
                    .filter_map(|k| k.parse::<usize>().ok())
                    .filter(|k| *k < 4)
                {
                    group_ext.entry(group).or_default().insert(k);
                }
            }
            if point
                .get("volt_offset_mv")
                .and_then(Value::as_f64)
                .unwrap_or_default()
                != 0.0
            {
                group_volt_off.insert(group);
            }
        }
        // table group per row: Some(segment position) or None when the
        // point attributes to no segment
        let mut current_group: Option<Option<usize>> = None;
        for point in points {
            let matching_segment = seg_of(point);
            if current_group != Some(matching_segment) {
                current_group = Some(matching_segment);
                let ext_slots: Vec<usize> = group_ext
                    .get(&matching_segment)
                    .map(|set| set.iter().copied().collect())
                    .unwrap_or_default();
                let has_volt_off = group_volt_off.contains(&matching_segment);
                let seg = matching_segment.and_then(|i| segments.map(|s| &s[i]));
                push_pvfp_header(&mut lines, seg, &ext_slots, roster.as_deref(), has_volt_off);
            }
            // extended-section slots, attributed by the owning segment's
            // domain (roster-minus-owner packing — see clk_vfp_status)
            if let Some(dc) = point.get("domain_currents").and_then(Value::as_object) {
                let owner = matching_segment
                    .and_then(|i| segments.map(|s| &s[i]))
                    .and_then(|seg| seg.get("domain").and_then(Value::as_str))
                    .unwrap_or("?")
                    .to_string();
                for (k_str, pair) in dc {
                    let Ok(k) = k_str.parse::<usize>() else {
                        continue;
                    };
                    if k >= 4 {
                        continue;
                    }
                    let Some(pair) = pair.as_array() else {
                        continue;
                    };
                    let (Some(f), Some(v)) = (
                        pair.first().and_then(Value::as_f64),
                        pair.get(1).and_then(Value::as_f64),
                    ) else {
                        continue;
                    };
                    if f == 0.0 && v == 0.0 {
                        continue;
                    }
                    let e = ext_stats
                        .entry((owner.clone(), k))
                        .or_insert((0, f, f, v, v));
                    e.0 += 1;
                    e.1 = e.1.min(f);
                    e.2 = e.2.max(f);
                    e.3 = e.3.min(v);
                    e.4 = e.4.max(v);
                }
            }
            // ── table row: bare numbers, blank cell = not reported ──
            // Voltage axis is µV (450000 = 450.0 mV); V_cur only exists on
            // modern records (@rec+0x68; legacy/BW report 0 → blank), F_cur
            // on everything but Pascal's type-1 records (all-zero → blank).
            let volt_cur = point
                .get("volt_current_mv")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let volt_def = point
                .get("voltage_uV")
                .and_then(Value::as_f64)
                .unwrap_or_default()
                / 1000.0;
            // MHz values — plain integers in faithful mode, one-decimal
            // floats under --infer-missing-field's Pascal remap (true
            // default = current − offset); as_f64 covers both
            let f_cur = point
                .get("freq_current_mhz")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let f_def = point
                .get("freq_default_mhz")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            // per-point curve voltage offset (Blackwell records; 0 elsewhere)
            let volt_off = point
                .get("volt_offset_mv")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            // raw control override readback (GetControl 0xDA025C3E):
            // mode 0 = absolute kHz offset ("freq"), mode 1 = raw delta;
            // effect MHz comes pre-computed (g(def) prior for mode 1)
            let mode = point.get("mode").and_then(Value::as_i64);
            let effect = point.get("offset_effect_mhz").and_then(Value::as_f64);

            let ext_slots: Vec<usize> = group_ext
                .get(&matching_segment)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default();
            let has_volt_off = group_volt_off.contains(&matching_segment);
            let idx = point
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let mut cells: Vec<String> = Vec::new();
            cells.push(pvfp_cell(Some(idx.to_string()), PVFP_W_ID));
            cells.push(pvfp_cell(
                (volt_cur != 0.0).then(|| format!("{volt_cur:.1}")),
                PVFP_W_VAL,
            ));
            cells.push(pvfp_cell(Some(format!("{volt_def:.1}")), PVFP_W_VAL));
            cells.push(pvfp_cell(
                (f_cur != 0.0).then(|| format!("{f_cur:.1}")),
                PVFP_W_VAL,
            ));
            cells.push(pvfp_cell(
                (f_def != 0.0).then(|| format!("{f_def:.1}")),
                PVFP_W_VAL,
            ));
            let dc = point.get("domain_currents").and_then(Value::as_object);
            for k in &ext_slots {
                let (f, v) = dc
                    .and_then(|o| o.get(&k.to_string()))
                    .and_then(Value::as_array)
                    .and_then(|pair| {
                        match (
                            pair.first().and_then(Value::as_f64),
                            pair.get(1).and_then(Value::as_f64),
                        ) {
                            (Some(f), Some(v)) => Some((f, v)),
                            _ => None,
                        }
                    })
                    .unwrap_or((0.0, 0.0));
                if f == 0.0 && v == 0.0 {
                    cells.push(pvfp_cell(None, PVFP_W_VAL));
                    cells.push(pvfp_cell(None, PVFP_W_VAL));
                } else {
                    // slot volts arrive as µV
                    cells.push(pvfp_cell(Some(format!("{:.1}", v / 1000.0)), PVFP_W_VAL));
                    cells.push(pvfp_cell(Some(format!("{f:.1}")), PVFP_W_VAL));
                }
            }
            if has_volt_off {
                cells.push(pvfp_cell(
                    (volt_off != 0.0).then(|| format!("{volt_off:+.1}")),
                    PVFP_W_VAL,
                ));
            }
            cells.push(pvfp_cell(mode.map(|m| m.to_string()), PVFP_W_MODE));
            cells.push(pvfp_cell(effect.map(|e| format!("{e:+.1}")), PVFP_W_VAL));
            lines.push(nvoc_cli_common::color::stylize(
                &format!("    {}", cells.join(PVFP_SEP)),
                false,
            ));
        }
    }

    // ── Extended-section per-domain current pairs ──
    // Slots @+0x074+0x10*k (freq MHz / volt µV) pack the curve domains
    // WITHOUT their own main record block, ascending — Turing (only gpc
    // main): XBAR/SYS/MSD/HOST; Ampere (xbar main): SYS/MSD/HOST; Ada
    // (msd main too): SYS/HOST (live A/B: the 35-distinct 225..1335
    // slot is HOST, not MSD). Reported per owning segment so
    // multi-block tables never mix.
    if !ext_stats.is_empty() {
        // Print the roster THIS table resolves to, not the pool the rule
        // starts from: on a table where a domain owns a main block (Ada:
        // MSD does) the pool text names a domain the driver never packs
        // into the record's ext slots. No `fabric_relations` block at all
        // = the relation was never read; say so rather than fall back to
        // the pool guess.
        let roster_note = match roster.as_ref() {
            None => "unknown: no fabric relation in this payload".to_string(),
            Some(r) if r.is_empty() => {
                "unattributed: every pool domain owns a main block here".to_string()
            }
            Some(r) => r.join(","),
        };
        lines.push(nvoc_cli_common::color::stylize(
            &format!(
                "    Extended-section currents (slots @+0x074+0x10*k; \
                 roster [{roster_note}] = pool minus this table's main-block owners; \
                 mem/disp bin owners: ext0 = PCIe gen / Hub):"
            ),
            false,
        ));
        for ((owner, k), (n, fmin, fmax, vmin, vmax)) in &ext_stats {
            // slot-0 columns of pstate-bin owners carry per-segment
            // quantities (mem = the PCIe gen ladder per pstate, disp = the
            // Hub domain's frequencies — FreqsEnum cross-match); only
            // vf_curve owners route through the roster.
            let tag = if *k == 0 && matches!(owner.as_str(), "mem" | "disp") {
                format!("ext0 ({})", if owner == "mem" { "PCIe gen" } else { "Hub" })
            } else {
                match roster.as_ref().and_then(|r| r.get(*k)) {
                    Some(nm) => format!("ext{k} ({nm})"),
                    None => format!("ext{k}"),
                }
            };
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "      {owner} {tag}: {n} pts, f {fmin:.1}..{fmax:.1} MHz, v {:.1}..{:.1} mV",
                    vmin / 1000.0,
                    vmax / 1000.0
                ),
                false,
            ));
        }
    }

    // segment summaries stay visible even when their points were filtered
    // out or attributed only by ordinal — one compact line per segment
    if let Some(segments) = segments {
        lines.push(format!(
            "  {}",
            nvoc_cli_common::color::stylize_title("Segments")
        ));
        for seg in segments {
            let domain = seg.get("domain").and_then(Value::as_str).unwrap_or("?");
            let kind = seg.get("kind").and_then(Value::as_str).unwrap_or("?");
            let bank = seg.get("bank").and_then(Value::as_i64).unwrap_or(0);
            let record_type = field_text(seg, "type");
            lines.push(nvoc_cli_common::color::stylize(
                &format!(
                    "    bank{bank} {domain} {kind} type {record_type}: index {}..{} ({} pts)",
                    field_text(seg, "start_index"),
                    field_text(seg, "end_index"),
                    field_text(seg, "count"),
                ),
                false,
            ));
        }
    }

    if points.is_none() && segments.is_none() {
        lines.extend(format_value_block(output, 1));
    }

    // --dump-records: raw 488B record slot map (per-offset dword stats +
    // first-record hex) — the layout-discovery section
    if let Some(map) = object.get("record_slot_map").and_then(Value::as_object) {
        lines.push(format!(
            "  {}",
            nvoc_cli_common::color::stylize_title("Record Slot Map")
        ));
        if let Some(err) = map.get("error").and_then(Value::as_str) {
            lines.push(format!("    {err}"));
            return lines;
        }
        let records = map.get("records").and_then(Value::as_i64).unwrap_or(0);
        let stride = map.get("stride").and_then(Value::as_i64).unwrap_or(488);
        let layout = map
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or("modern (R610 large-table)");
        lines.push(format!(
            "    {records} raw records × {stride} B [{layout}]; per-dword stats (non-zero slots):"
        ));
        if let Some(slot_list) = map.get("slots").and_then(Value::as_array) {
            for slot in slot_list {
                let tag = slot
                    .get("offset_hex")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let min = slot.get("min").and_then(Value::as_u64).unwrap_or(0);
                let max = slot.get("max").and_then(Value::as_u64).unwrap_or(0);
                let distinct = slot.get("distinct").and_then(Value::as_u64).unwrap_or(0);
                let mut line = if min == max {
                    format!("    {tag}: constant {min} (0x{min:08X})")
                } else {
                    format!("    {tag}: {min}..{max} ({distinct} distinct)")
                };
                if let (Some(imin), Some(imax)) = (
                    slot.get("i32_min").and_then(Value::as_i64),
                    slot.get("i32_max").and_then(Value::as_i64),
                ) {
                    line.push_str(&format!("  [i32 {imin}..{imax}]"));
                }
                if let Some(equals) = slot.get("equals").and_then(Value::as_array) {
                    let labels: Vec<&str> = equals.iter().filter_map(Value::as_str).collect();
                    if !labels.is_empty() {
                        line.push_str(&format!("  [= {}]", labels.join(" = ")));
                    }
                }
                lines.push(line);
            }
        }
        let zeros = map
            .get("zero_slot_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        lines.push(format!(
            "    ({zeros} of {} dword slots identically zero)",
            stride / 4
        ));
        if let Some(hex_rows) = map.get("record_hex").and_then(Value::as_array) {
            lines.push(format!(
                "    first record (#{}) hex:",
                map.get("first_record_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ));
            for row in hex_rows {
                if let Some(text) = row.as_str() {
                    lines.push(format!("      {text}"));
                }
            }
        }
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
pub(super) fn format_temperature_thresholds_output(output: &Value) -> Vec<String> {
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

pub(super) fn format_pstate_native_output(output: &Value) -> Vec<String> {
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
            let max = max.filter(|v| *v != 0.0);
            let min = min.filter(|v| *v != 0.0);
            // Either side may be absent — the legacy pstate table rescues
            // clocks per domain and a parked domain can report max only.
            let values = match (max, min) {
                (Some(max), Some(min)) => {
                    format!("Max {} MHz, Min {} MHz", trim_float(max), trim_float(min))
                }
                (Some(max), None) => format!("Max {} MHz", trim_float(max)),
                (None, Some(min)) => format!("Min {} MHz", trim_float(min)),
                (None, None) => continue,
            };
            lines.push(format!(
                "      {}: {}",
                stylize_title(label),
                nvoc_cli_common::color::stylize(&values, false)
            ));
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

pub(super) fn format_throttle_reasons_output(output: &Value) -> Vec<String> {
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
                    // `all_clocks` is superseded by `all_clocks_detailed` (same
                    // GetAllClocks V2 call, richer rows) — drop the flat line
                    // when the detailed table is present.
                    _ if key == "all_clocks" && object.contains_key("all_clocks_detailed") => {}
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
                    // `freq_ranges` (get-private-freq-domain-status) — the
                    // FreqsEnum legal-frequency enumeration per domain:
                    // kind=freq_range renders as
                    //   {domain}: Freq Range {min} - {max} MHz / Step {step} MHz
                    // (non-uniform grids omit Step), kind=pstate_bins as
                    //   {domain}: Freq Bins [a, b, c] MHz
                    Value::Array(items) if key == "freq_ranges" => {
                        lines.push(format!(
                            "{}{}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize_title("Freq Ranges")
                        ));
                        for item in items {
                            let domain = item.get("domain").and_then(Value::as_str).unwrap_or("?");
                            let sel = item.get("selector").and_then(Value::as_u64).unwrap_or(0);
                            match item.get("kind").and_then(Value::as_str) {
                                Some("freq_range") => {
                                    let min =
                                        item.get("min_mhz").and_then(Value::as_u64).unwrap_or(0);
                                    let max =
                                        item.get("max_mhz").and_then(Value::as_u64).unwrap_or(0);
                                    let pts =
                                        item.get("points").and_then(Value::as_u64).unwrap_or(0);
                                    let step = item.get("step_mhz").and_then(Value::as_u64);
                                    let step_part = step
                                        .map(|s| format!(" / Step {s} MHz"))
                                        .unwrap_or_else(|| " / non-uniform grid".to_string());
                                    lines.push(format!(
                                        "{}{}(sel {sel}): Freq Range {min} - {max} MHz{step_part}  [{pts} pts]",
                                        indent_spaces(indent + 1),
                                        nvoc_cli_common::color::stylize(domain, false),
                                    ));
                                }
                                Some("pstate_bins") => {
                                    let pts = item
                                        .get("points_mhz")
                                        .and_then(Value::as_array)
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(Value::as_u64)
                                                .map(|v| v.to_string())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        })
                                        .unwrap_or_default();
                                    // PcieGen's "bins" are the gen ladder
                                    // [1,2,3], not MHz
                                    if domain == "PcieGen" {
                                        lines.push(format!(
                                            "{}{}(sel {sel}): PCIe Gen Bins [{pts}]",
                                            indent_spaces(indent + 1),
                                            nvoc_cli_common::color::stylize(domain, false),
                                        ));
                                    } else {
                                        lines.push(format!(
                                            "{}{}(sel {sel}): Freq Bins [{pts}] MHz",
                                            indent_spaces(indent + 1),
                                            nvoc_cli_common::color::stylize(domain, false),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
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
                        lines.extend(format_p0_voltage_block(indent, "P0 Voltage Limit", child));
                    }
                    // Per-rail P0 bounds (get-volt-rail-info `p0_rails`, one
                    // entry per rail in the mask): each renders the same
                    // multi-line block as `p0_voltage`, titled with its rail
                    // bit — on multi-rail parts (GB10 / 50-series) this
                    // surfaces the Xbar rail's bounds below the core's.
                    Value::Array(items)
                        if key == "p0_rails"
                            && items.iter().all(|v| {
                                v.as_object().is_some_and(|o| o.contains_key("rail_bit"))
                            }) =>
                    {
                        for item in items {
                            let object = item.as_object().expect("checked above");
                            let bit = object.get("rail_bit").and_then(Value::as_i64);
                            // Rail identity: bit 0 is the core rail on every
                            // observed part; higher bits are platform-specific
                            // (5090 MSVDD vs GB10 Xbar) and stay unnamed.
                            let title = match bit {
                                Some(0) => "P0 Voltage Limit (rail 0 / gpc)".to_string(),
                                Some(bit) => format!("P0 Voltage Limit (rail {bit})"),
                                None => "P0 Voltage Limit".to_string(),
                            };
                            lines.extend(format_p0_voltage_block(indent, &title, object));
                        }
                    }
                    // `vrm_devices` (get-volt-rail-info): melonVolt
                    // voltage-domain windows (VoltVoltDevicesGetInfo
                    // 0xA38ACF9D) — one titled block per device with the
                    // packed id / controllable range / step, same style as
                    // the P0 block. Omitted entirely when the enumerator
                    // refused (the JSON key carries null then).
                    Value::Null if key == "vrm_devices" => {}
                    Value::Array(items) if key == "vrm_devices" => {
                        for item in items {
                            let Some(object) = item.as_object() else {
                                continue;
                            };
                            let n = object.get("device").and_then(Value::as_u64).unwrap_or(0);
                            lines.push(format!(
                                "{}{}",
                                indent_spaces(indent),
                                nvoc_cli_common::color::stylize_title(&format!("VRM Device {n}"))
                            ));
                            let id = object.get("id").and_then(Value::as_str).unwrap_or("?");
                            lines.push(format!(
                                "{}{}: {}",
                                indent_spaces(indent + 1),
                                nvoc_cli_common::color::stylize_title("Id"),
                                nvoc_cli_common::color::stylize(id, false),
                            ));
                            let (min, max) = object
                                .get("range_mV")
                                .and_then(Value::as_array)
                                .and_then(|r| Some((r.first()?.as_u64()?, r.get(1)?.as_u64()?)))
                                .unwrap_or((0, 0));
                            let default = object
                                .get("default_mV")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            lines.push(format!(
                                "{}{}: {}",
                                indent_spaces(indent + 1),
                                nvoc_cli_common::color::stylize_title("Range"),
                                nvoc_cli_common::color::stylize(
                                    &format!("{min} - {max} mV ({default} mV default)"),
                                    false,
                                ),
                            ));
                            let step = match object.get("step_mV").and_then(Value::as_f64) {
                                Some(s) if (s - s.round()).abs() < 1e-9 => {
                                    format!("{} mV", s as u64)
                                }
                                Some(s) => format!("{s} mV"),
                                None => "N/A".to_string(),
                            };
                            lines.push(format!(
                                "{}{}: {}",
                                indent_spaces(indent + 1),
                                nvoc_cli_common::color::stylize_title("Step"),
                                nvoc_cli_common::color::stylize(&step, false),
                            ));
                        }
                    }
                    // `all_clocks_detailed` (get-status): GetAllClocks V2
                    // extended-domain entries — effective frequency plus the
                    // driver's own ratio_domain/ratio declaration and the four
                    // reserved dwords. Aligned table, same style as the
                    // private V/F table.
                    Value::Object(child) if key == "all_clocks_detailed" => {
                        lines.extend(format_all_clocks_detailed(indent, child));
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

            // 对象/数组项渲染为多行"块"；块与块之间插一条分隔线，否则相邻
            // 块的字段行无法目视归属（如 get-pwr-cur-info 的 Policies 列表）。
            // 标量项保持紧凑列表，不插线。
            let rendered: Vec<(bool, Vec<String>)> = items
                .iter()
                .map(|item| match item {
                    Value::Object(_) | Value::Array(_) => {
                        let lines = format_value_block_with_context(item, indent, context);
                        (lines.len() >= 2, lines)
                    }
                    _ => (
                        false,
                        vec![format!(
                            "{}- {}",
                            indent_spaces(indent),
                            nvoc_cli_common::color::stylize(&format_scalar("", item), false)
                        )],
                    ),
                })
                .collect();
            let mut out: Vec<String> = Vec::new();
            let mut prev_block = false;
            for (is_block, mut lines) in rendered {
                if is_block && prev_block {
                    out.push(format!("{}{}", indent_spaces(indent), "-".repeat(30)));
                }
                out.append(&mut lines);
                prev_block = is_block;
            }
            out
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
    // Keys pinned to the front regardless of the alphabetical pass below:
    // "vrm_devices" (get-volt-rail-info) is the per-domain summary — the
    // window each rail clamps into — and reads before the VoltRails walls.
    const PINNED_FIRST: [&str; 1] = ["vrm_devices"];
    let (mut pinned, mut entries): (Vec<_>, Vec<_>) = object
        .iter()
        .partition(|(key, _)| PINNED_FIRST.contains(&key.as_str()));
    // Numeric keys (clock domains, pstate numbers) sort numerically.
    if entries.iter().all(|(key, _)| key.parse::<i64>().is_ok()) {
        entries.sort_by_key(|(key, _)| key.parse::<i64>().unwrap_or_default());
        pinned.append(&mut entries);
        return pinned;
    }
    // C-style enum keys serialized by serde as variant names (e.g. "P0", "P8",
    // "P12" from NV_GPU_PERF_PSTATE_ID). Without this the BTreeMap's numeric
    // order is lost on the JSON boundary (serde serializes enum variants as
    // their name string, not their discriminant) and we'd emit P0/P12/P8 in
    // insertion order. Sort by the numeric suffix.
    if entries
        .iter()
        .all(|(key, _)| key.len() > 1 && key.starts_with('P') && key[1..].parse::<u32>().is_ok())
    {
        entries.sort_by_key(|(key, _)| key[1..].parse::<u32>().unwrap_or_default());
        pinned.append(&mut entries);
        return pinned;
    }
    pinned.append(&mut entries);
    pinned
}

#[cfg(test)]
mod sort_tests {
    use super::*;

    #[test]
    fn pstate_keys_sort_numerically() {
        // Reproduces the classic "P0 / P12 / P8" ordering bug: BTreeMap<PState>
        // is numeric on the Rust side, but serde serializes enum variants as
        // their name strings ("P0", "P8", "P12"), so the JSON object loses the
        // numeric ordering and we'd emit P0/P12/P8 (insertion order).
        let mut object = serde_json::Map::new();
        // Insert deliberately out of numeric order.
        object.insert("P8".to_string(), json!({"v": 8}));
        object.insert("P0".to_string(), json!({"v": 0}));
        object.insert("P12".to_string(), json!({"v": 12}));
        let entries = sorted_object_entries(&object);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["P0", "P8", "P12"],
            "pstate keys must be numeric, not lexicographic"
        );
    }

    #[test]
    fn plain_numeric_keys_still_sort() {
        let mut object = serde_json::Map::new();
        object.insert("12".to_string(), json!(12));
        object.insert("0".to_string(), json!(0));
        object.insert("8".to_string(), json!(8));
        let entries = sorted_object_entries(&object);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["0", "8", "12"]);
    }

    #[test]
    fn mixed_keys_do_not_panic() {
        let mut object = serde_json::Map::new();
        object.insert("Graphics".to_string(), json!("g"));
        object.insert("Memory".to_string(), json!("m"));
        // Should return in insertion order (no numeric/P-prefix sort applied).
        let entries = sorted_object_entries(&object);
        assert_eq!(entries.len(), 2);
    }
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

/// Render get-status `all_clocks_detailed` (GetAllClocks V2 extended-domain
/// entries: effective frequency + the driver's own ratio_domain/ratio +
/// reserved dwords) in the section style used by the rest of get-status —
/// a title plus one `Name: value` line per domain. The ratio/reserved extras
/// are appended to a domain's line only when they carry information
/// (ratio parent set / ratio raw nonzero / reserved nonzero), keeping the
/// common all-zero case identical in shape to the old flat line.
fn format_all_clocks_detailed(
    indent: usize,
    object: &serde_json::Map<String, Value>,
) -> Vec<String> {
    let mut rows: Vec<(String, &serde_json::Map<String, Value>)> = object
        .iter()
        .filter_map(|(name, entry)| Some((name.clone(), entry.as_object()?)))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let pad = indent_spaces(indent);
    let row_pad = indent_spaces(indent + 1);
    let mut lines = vec![format!(
        "{pad}{}",
        nvoc_cli_common::color::stylize_title("All Clocks")
    )];
    for (name, entry) in rows {
        let mut text = match entry.get("frequency_khz").and_then(Value::as_f64) {
            Some(khz) => format!("{name}: {}", format_measurement(khz / 1000.0, "MHz")),
            None => continue,
        };
        if let Some(dom) = entry.get("ratio_domain").and_then(Value::as_str) {
            let ratio = entry.get("ratio").and_then(Value::as_u64).unwrap_or(0);
            if ratio != 0 {
                // ratio is already a percentage (88 = 88% of the parent domain)
                text.push_str(&format!(", ratio → {dom} × {ratio}%"));
            } else {
                text.push_str(&format!(", ratio → {dom}"));
            }
        } else if let Some(ratio) = entry.get("ratio").and_then(Value::as_u64)
            && ratio != 0
        {
            text.push_str(&format!(", ratio × {ratio}%"));
        }
        let reserved: Vec<u64> = entry
            .get("reserved")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        if !reserved.is_empty() && reserved.iter().any(|&v| v != 0) {
            let joined = reserved
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(":");
            text.push_str(&format!(", rsv {joined}"));
        }
        lines.push(format!(
            "{row_pad}{}",
            nvoc_cli_common::color::stylize(&text, false)
        ));
    }
    lines
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
fn format_p0_voltage_block(
    indent: usize,
    title: &str,
    object: &serde_json::Map<String, Value>,
) -> Vec<String> {
    let mut lines = vec![format!(
        "{}{}",
        indent_spaces(indent),
        nvoc_cli_common::color::stylize_title(title)
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

/// Render the thermal `sensors` array in COMPACT form: one line naming only
/// the classified primary sensors (`GPU_AVG: 44.5 C  HOT_SPOT: 60.8 C`).
/// The full research metadata per channel (Channel Num/Type, Offset Sw/Hw,
/// Scaling, Range, sensor pairing) lives in `get-thermal-channels` — the
/// JSON emitted here is unchanged, only the human rendering slims down.
/// Non-tuple entries (legacy descriptor objects, e.g. the NVML/legacy
/// path's named sensors) keep the generic block rendering; metadata-less
/// tuple entries keep a bare temperature line.
fn format_sensors_array(indent: usize, items: &[Value]) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for sensor in items {
        let Some(tuple) = sensor.as_array() else {
            other.extend(format_value_block_with_context(sensor, indent, "sensors"));
            continue;
        };
        let descriptor = tuple.first().and_then(Value::as_object);
        let temp = tuple.get(1);
        let tag = match descriptor.map(|d| d.get("channel_type")).unwrap_or(None) {
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        };
        let temp_text = temp.map(|t| match t {
            Value::Number(number) => format_measurement(number.as_f64().unwrap_or(0.0), "C"),
            _ => format_scalar("", t),
        });
        match (tag, temp_text) {
            (Some(t), Some(text)) if (0..=4).contains(&t) => {
                let name = match t {
                    0 => "GPU_AVG",
                    1 => "HOT_SPOT",
                    2 => "BOARD",
                    3 => "VRAM",
                    _ => "PWR_SUPPLY",
                };
                named.push(format!(
                    "{}: {}",
                    nvoc_cli_common::color::stylize_title(name),
                    nvoc_cli_common::color::stylize(&text, false)
                ));
            }
            (_, Some(text)) => {
                // unclassified (255) or metadata-less: bare temperature line
                other.push(format!(
                    "{}- {}",
                    indent_spaces(indent),
                    nvoc_cli_common::color::stylize(&text, false)
                ));
            }
            (_, None) => {}
        }
    }
    let mut lines = Vec::new();
    if !named.is_empty() {
        lines.push(format!("{}{}", indent_spaces(indent), named.join("  ")));
    }
    if named.is_empty() && other.is_empty() {
        lines.push(format!(
            "{}(research channels: get-thermal-channels)",
            indent_spaces(indent)
        ));
    }
    lines.extend(other);
    lines
}

/// Render the full ThermChannel map (get-thermal-channels): the classified
/// primary channels with live temperatures, then every populated channel with
/// its research metadata (type tag, offsets/scaling raw, decoded range, and
/// the same-sensor pairing annotation for the `(dev, 1)` half whose STATUS
/// reading already carries `offset_hw`).
pub(super) fn format_thermal_channels(output: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if output.get("supported").and_then(Value::as_bool) == Some(false) {
        return vec!["  Thermal Channels: N/A (ThermChannel family unavailable)".to_string()];
    }
    let temp_text = |v: Option<&Value>| -> Option<String> {
        v.and_then(Value::as_f64)
            .map(|t| format_measurement(t, "C"))
    };
    let ch_tag = |t: f64| -> String {
        match t as i64 {
            0 => "GPU_AVG".to_string(),
            1 => "HOT_SPOT".to_string(),
            2 => "BOARD".to_string(),
            3 => "VRAM".to_string(),
            4 => "PWR_SUPPLY".to_string(),
            255 => "unclassified".to_string(),
            other => format!("type {other}"),
        }
    };
    if let Some(primary) = output.get("primary").and_then(Value::as_array) {
        for entry in primary {
            let channel = entry.get("channel").and_then(Value::as_u64);
            let temp = temp_text(entry.get("temp_c"));
            let label = entry
                .get("type")
                .and_then(Value::as_str)
                // Same vocabulary as the compact get-status line.
                .map(|l| match l {
                    "GPU_MAX(hotspot)" => "HOT_SPOT",
                    "MEMORY(vram)" => "VRAM",
                    other => other,
                })
                .unwrap_or("UNKNOWN");
            match (channel, temp) {
                (Some(ch), Some(t)) => lines.push(format!(
                    "  {}: ch{} = {}",
                    nvoc_cli_common::color::stylize_title(label),
                    nvoc_cli_common::color::stylize(&ch.to_string(), false),
                    nvoc_cli_common::color::stylize(&t, false)
                )),
                (Some(ch), None) => lines.push(format!(
                    "  {}: ch{} (no live reading)",
                    nvoc_cli_common::color::stylize_title(label),
                    nvoc_cli_common::color::stylize(&ch.to_string(), false)
                )),
                _ => {}
            }
        }
    }
    if let Some(channels) = output.get("channels").and_then(Value::as_array) {
        if !channels.is_empty() {
            lines.push(nvoc_cli_common::color::stylize_title("Channels").to_string());
        }
        for entry in channels {
            let idx = entry
                .get("channel")
                .and_then(Value::as_u64)
                .map(|i| i.to_string())
                .unwrap_or_else(|| "?".to_string());
            let ty = entry.get("ch_type").and_then(Value::as_f64);
            let ty_text = match ty {
                Some(t) => ch_tag(t),
                None => "?".to_string(),
            };
            let temp = temp_text(entry.get("temp_c")).unwrap_or_else(|| "no reading".to_string());
            let mut parts = vec![format!(
                "ch{}: {} = {}",
                nvoc_cli_common::color::stylize(&idx, false),
                ty_text,
                nvoc_cli_common::color::stylize(&temp, false)
            )];
            if let Some(dev) = entry.get("therm_dev").and_then(Value::as_array) {
                let d = dev.first().and_then(Value::as_u64).unwrap_or(0);
                let p = dev.get(1).and_then(Value::as_u64).unwrap_or(0);
                parts.push(format!("dev({d},{p})"));
                if p == 1
                    && let Some(sib) = entry.get("same_sensor_as").and_then(Value::as_u64)
                {
                    parts.push(format!("same sensor as ch{sib} (+offset_hw)"));
                }
            }
            for key in ["offset_sw", "offset_hw", "scaling"] {
                if let Some(v) = entry.get(key).and_then(Value::as_i64) {
                    parts.push(format!("{key} {v}"));
                }
            }
            if let Some(range) = entry.get("range_c").and_then(Value::as_array) {
                let lo = range.first().and_then(Value::as_f64);
                let hi = range.get(1).and_then(Value::as_f64);
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    parts.push(format!(
                        "range {}..{}",
                        format_measurement(lo, "C"),
                        format_measurement(hi, "C")
                    ));
                }
            }
            lines.push(format!("  {}", parts.join(" | ")));
        }
    }
    if output.get("status_supported").and_then(Value::as_bool) == Some(false) {
        lines.push("  (live STATUS refused — metadata only)".to_string());
    }
    // RegisterOp raw reads (LACT parity): per-chip GDDR6x/7 sensors + the
    // Blackwell+ hotspot register fallback. On Blackwell prefer the register
    // hotspot over the channel map's HOT_SPOT slot (LACT does the same).
    match output.get("register_views") {
        Some(views) if views.get("supported").and_then(Value::as_bool) == Some(false) => {
            let reason = views
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("RegisterOp unavailable");
            lines.push(format!(
                "  {} {}",
                nvoc_cli_common::color::stylize_title("Register views:"),
                nvoc_cli_common::color::stylize(&format!("N/A ({reason})"), false)
            ));
        }
        Some(views) => {
            lines.push(nvoc_cli_common::color::stylize_title("Register views").to_string());
            if let Some(ram) = views.get("ram_type").and_then(Value::as_str) {
                lines.push(format!(
                    "  {}: {}",
                    nvoc_cli_common::color::stylize_title("ram_type"),
                    nvoc_cli_common::color::stylize(ram, false)
                ));
            }
            if let Some(error) = views.get("vram_chip_error").and_then(Value::as_str) {
                // A call-level NVAPI_ERROR (-1) is the non-admin signature of
                // the kernel register lane (RE-proven on 610.47; every user-
                // mode check passes first). Hint so the reading is actionable.
                let hint = if error.contains("NVAPI_ERROR") {
                    "; the register lane requires an elevated (admin) process"
                } else {
                    ""
                };
                lines.push(format!(
                    "  {}: {}",
                    nvoc_cli_common::color::stylize_title("VRAM chip probe"),
                    nvoc_cli_common::color::stylize(&format!("aborted ({error}){hint}"), false)
                ));
            } else {
                match views.get("vram_chips").and_then(Value::as_array) {
                    Some(chips) if !chips.is_empty() => {
                        for chip in chips {
                            let label = chip.get("label").and_then(Value::as_str).unwrap_or("?");
                            let temp = temp_text(chip.get("temp_c"))
                                .unwrap_or_else(|| "no reading".to_string());
                            lines.push(format!(
                                "  {}: {}",
                                nvoc_cli_common::color::stylize_title(&format!(
                                    "VRAM chip {label}"
                                )),
                                nvoc_cli_common::color::stylize(&temp, false)
                            ));
                        }
                    }
                    Some(_) => {
                        let ram = views.get("ram_type").and_then(Value::as_str).unwrap_or("?");
                        lines.push(format!(
                            "  {}: {}",
                            nvoc_cli_common::color::stylize_title("VRAM chips"),
                            nvoc_cli_common::color::stylize(
                                &format!("none (probe set is GDDR6x/GDDR7; this is {ram})"),
                                false
                            )
                        ));
                    }
                    None => {}
                }
            }
            if let Some(temp) = views
                .get("blackwell_hotspot_register_c")
                .and_then(Value::as_f64)
            {
                lines.push(format!(
                    "  {}: {}",
                    nvoc_cli_common::color::stylize_title("Blackwell hotspot register"),
                    nvoc_cli_common::color::stylize(&format_measurement(temp, "C"), false)
                ));
            }
        }
        None => {}
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
    // major = (value >> 12) & 0xf; minor = (value >> 8) & 0xf — one decimal digit
    // for every major (WDDM 2.x spans 2.0-2.9, e.g. 0x00002300 = WDDM 2.3).
    if context.contains("driver_model") || value_key == "driver_model" {
        let word = number as u32;
        let major = ((word >> 12) & 0xf) as u8;
        let minor = ((word >> 8) & 0xf) as u8;
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
            "tgp" => "TGP".to_string(),
            "ocp" => "OCP".to_string(),
            "nvvdd" => "NVVDD".to_string(),
            "msvdd" => "MSVDD".to_string(),
            "vfp" => "VFP".to_string(),
            // SI unit words match case-insensitively — keys arrive both as
            // "mv" and "mV" (JSON field names carry the unit suffix), and a
            // case-sensitive match turned "mV" into "MV" (m ≠ M in SI).
            "uv" | "uV" | "UV" => "uV".to_string(),
            "mv" | "mV" | "MV" => "mV".to_string(),
            "mhz" | "MHz" | "MHZ" => "MHz".to_string(),
            "khz" | "kHz" | "KHZ" => "kHz".to_string(),
            "ns" | "Ns" | "NS" => "ns".to_string(),
            "ma" | "mA" | "MA" => "mA".to_string(),
            "mw" | "mW" | "MW" => "mW".to_string(),
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

    #[test]
    fn set_power_command_renders_envelope_and_echo_warning() {
        let value = serde_json::json!({
            "channel": 0,
            "command": "request",
            "value": 300000,
            "value_mW": 300000,
            "value_W": 300.0,
            "baseline_before": 250000,
            "baseline_before_W": 250.0,
            "board_default_mW": 250000,
            "envelope": "board_default_x2",
        });
        let lines = super::format_set_power_command(&value);
        assert!(lines[0].contains("300000 mW (300.0 W)"), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("baseline: 250.0 W")
                && l.contains("restore with set-power-command 250")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("board default: 250000 mW")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("envelope: board_default_x2")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("ECHO LAYER ONLY")),
            "{lines:?}"
        );
    }

    #[test]
    fn set_power_command_kmd_renders_patch_cycle_without_echo_warning() {
        let value = serde_json::json!({
            "kmd": true,
            "arm": "root",
            "wall_before_mw": 115_000,
            "wall_after_mw": 190_000,
            "tgp_written_w": 190,
            "tgp_note": "root arm: tgp write 190 W ✓ readback match",
            "lease_written_mw": 190_000,
            "lease_note": "lease channel0 = 190000 mW ✓ (checked write)",
            "layout": "slot=0x1234 table+0x5678 root_init=0x9abc",
            "service": "PMXDRV_KMD",
            "steps": [
                "service PMXDRV_KMD registered+started ✓",
                "UPPER 115000 → 190000 ✓ readback match",
                "lease channel0 = 190000 mW ✓ (checked write)",
                "recheck wall field=190000 ✓",
                "service PMXDRV_KMD stopped+deleted ✓",
            ],
            "note": "wall field write is volatile (resets on reboot); repeat per boot. Load test before trusting the new wall",
        });
        let lines = super::format_set_power_command(&value);
        assert!(
            lines[0].contains(
                "Kernel wall patched via pmxdrv (root arm): 115000 mW → 190000 mW (190.0 W)"
            ),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("UPPER 115000 → 190000 ✓ readback match")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("recheck wall field=190000 ✓")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("KERNEL PATCH APPLIED") && l.contains("pmxdrv kernel driver")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("volatile (resets on reboot)")),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("ECHO LAYER ONLY")),
            "{lines:?}"
        );
    }

    #[test]
    fn get_power_command_request_carries_echo_note() {
        let value = serde_json::json!({
            "channel": 0, "command": "request", "value": 250000,
            "value_mW": 250000, "value_W": 250.0,
        });
        let lines = super::format_power_command(&value);
        assert!(lines.iter().any(|l| l.contains("echo layer")), "{lines:?}");
        // observed cells stay clean
        let value = serde_json::json!({ "channel": 0, "command": "observed", "value": 7 });
        let lines = super::format_power_command(&value);
        assert!(!lines.iter().any(|l| l.contains("echo layer")), "{lines:?}");
    }
    use super::*;
    use crate::{Command, TargetResult, all_commands};
    use serde_json::json;

    #[test]
    fn human_output_formats_objects_without_json_dump() {
        nvoc_cli_common::color::init(true);
        let execution = Execution {
            function: "get-public-power-limit",
            command: Command::GetPublicPowerLimit,
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

    /// SI casing on current/power units: keys carry the unit as a suffix
    /// (`raw_mA`, `driver_window_mW`), and the generic label formatter must
    /// read them back as mA/mW — an unlisted token fell through to plain
    /// "uppercase the first letter" and rendered "Raw MA" (user report,
    /// 40-series set-pwr-cur-limit run).
    #[test]
    fn si_unit_case_is_preserved_for_ma_and_mw() {
        nvoc_cli_common::color::init(true);
        let execution = Execution {
            function: "set-pwr-cur-limit",
            command: Command::SetPwrCurLimit,
            backend: "nvapi".to_string(),
            warnings: Vec::new(),
            results: vec![TargetResult {
                gpu_id: Some(7),
                backend: "nvapi",
                ok: true,
                output: Some(json!({
                    "applied": true,
                    "target": "nvvdd",
                    "index": 0,
                    "identity": "(13,19)",
                    "amperes": 135.0,
                    "raw_mA": 135000,
                    "driver_window_mA": [1, 135000],
                    "requested_mA": 140000,
                    "clamped_to_driver_window": true,
                })),
                error: None,
            }],
        };

        let rendered = format_human(&execution);

        assert!(rendered.contains("Raw mA: 135000"), "{rendered}");
        assert!(rendered.contains("Driver Window mA"), "{rendered}");
        assert!(rendered.contains("Requested mA: 140000"), "{rendered}");
        assert!(!rendered.contains("MA"), "{rendered}");

        // Board-power row: the same formatter path with the mW suffix.
        let rendered_mw = format_label("raw_mW") + &format_label("driver_window_mW");
        assert_eq!(rendered_mw, "Raw mWDriver Window mW");

        // Acronyms keep their canonical spelling in labels too.
        assert_eq!(format_label("is_ocp_current"), "Is OCP Current");
        assert_eq!(format_label("nvvdd_ocp"), "NVVDD OCP");
        assert_eq!(format_label("tgp_range"), "TGP Range");
    }

    #[test]
    fn vp_ladder_formats_id_freq_table_without_voltage() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "source": "vbios-vp-ladder",
            "domain": "graphics",
            "generation": "Pascal",
            "table_count": 1,
            "mem_clock_ddr_mhz": 4004,
            "profiles": [
                {"id": "0x07", "pstate": "P8", "limit1_mhz": 1320.0, "limit3_mhz": 1240.0},
                {"id": "0x0f", "pstate": "P0", "limit1_mhz": 1911.0, "limit3_mhz": 1811.0}
            ],
            "footers": [
                {"id": "0x0f", "pstate": "P0", "mem_clock_mhz": 2002, "mem_clock_ddr_mhz": 4004}
            ],
            "power": [
                {"platform": "desktop", "target_watt": 180.0, "limit_watt": 200.0,
                 "slider": {"enabled": true}}
            ],
            "thermal_fan": {
                "present": false,
                "note": "BIT 'P' thermal pointer is zero (Pascal)"
            },
            "dcb": {
                "pads": [
                    {"pad": "DP_A", "roles": ["DVI/HDMI"], "connectors": []},
                    {"pad": "DP_B", "roles": ["DP"], "connectors": []}
                ]
            },
            "points": [
                {"index": 0, "frequency_khz": 324000, "frequency_mhz": 324.0},
                {"index": 1, "frequency_khz": 1911000, "frequency_mhz": 1911.0}
            ],
            "warnings": []
        });

        let lines = format_maxwell_vftable_output(&output);
        let text = lines.join("\n");

        assert!(text.contains("VP ladder (Pascal, 2 points)"), "{text}");
        assert!(text.contains("Freq"), "{text}");
        assert!(!text.contains("V_min"), "{text}");
        assert!(text.contains("1911.0"), "{text}");
        assert!(text.contains("profiles (limit1/limit3 MHz)"), "{text}");
        assert!(text.contains("0x0f(P0) 1911/1811"), "{text}");
        assert!(text.contains("mem clock (DDR): 4004 MHz"), "{text}");
        assert!(
            text.contains("footers (DRAM/DDR MHz): 0x0f(P0) 2002/4004"),
            "{text}"
        );
        assert!(
            text.contains("power (desktop): target 180 W, limit 200 W, slider enabled"),
            "{text}"
        );
        assert!(
            text.contains("thermal/fan: BIT 'P' thermal pointer is zero"),
            "{text}"
        );
        assert!(text.contains("display: DP_A=DVI/HDMI DP_B=DP"), "{text}");
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
        // WDDM 2.x minor is a full one-decimal nibble (0x2300 = 2.3, NOT 2.0 —
        // the legacy "2.x == 2.0" special case was wrong).
        let legacy =
            format_value_block(&json!({ "driver_model": { "value": 0x2300_u32 } }), 0).join("\n");
        assert!(legacy.contains("Value: 0x00002300 (WDDM 2.3)"));
        // Sensor range bounds carry a C unit.
        assert!(rendered.contains("Range: Max 139 C, Min -35 C"));
    }

    #[test]
    fn sensors_array_renders_compact_primaries_only() {
        nvoc_cli_common::color::init(true);
        // Mirrors get-status `sensors`: [descriptor, temp] tuples. Classified
        // primaries collapse onto one line; unclassified research channels are
        // dropped from the human view (full detail lives in
        // get-thermal-channels), and legacy descriptor objects keep the block
        // rendering.
        let output = json!({
            "sensors": [
                [{"target": "Gpu", "channel_num": 0, "channel_type": 0, "offset_sw": 0, "offset_hw": 0, "scaling": 256}, 44.53125],
                [{"target": "Gpu", "channel_num": 1, "channel_type": 1, "offset_sw": 0, "offset_hw": 0, "scaling": 256}, 60.75],
                [{"target": "Gpu", "channel_num": 2, "channel_type": 255, "offset_sw": 0, "offset_hw": 3736, "scaling": 256}, 44.59375],
                {"name": "Core", "range": {"max": 139, "min": -35}}
            ]
        });
        let rendered = format_value_block(&output, 0).join("\n");
        // Classified primaries collapse onto one line (sub-degree temps go
        // through the house measurement formatter); ANSI coloring sits between
        // the segments, so assert the parts individually.
        assert!(rendered.contains("GPU_AVG: 44.531 C"), "{rendered}");
        assert!(rendered.contains("HOT_SPOT: 60.75 C"), "{rendered}");
        // Unclassified channel dropped; research fields no longer rendered here.
        assert!(!rendered.contains("Channel Type"));
        assert!(!rendered.contains("44.59375"));
        assert!(!rendered.contains("Offset Hw"));
        // Legacy object entries keep the range line.
        assert!(rendered.contains("Range: Max 139 C, Min -35 C"));
    }

    #[test]
    fn thermal_channels_formatter_renders_primaries_and_metadata() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "channel_mask": "0x000000ff",
            "primary": [
                {"type": "GPU_AVG", "channel": 0, "temp_c": 44.5},
                {"type": "GPU_MAX(hotspot)", "channel": 1, "temp_c": 60.8},
                {"type": "BOARD", "channel": null, "temp_c": null}
            ],
            "channels": [
                {"channel": 0, "ch_type": 0, "therm_dev": [0, 0], "same_sensor_as": null,
                 "offset_sw": 0, "offset_hw": 0, "scaling": 256, "temp_c": 44.5},
                {"channel": 3, "ch_type": 255, "therm_dev": [0, 1], "same_sensor_as": 2,
                 "offset_sw": 0, "offset_hw": 3736, "scaling": 256,
                 "range_c": [-40, 150], "temp_c": 59.2}
            ],
            "status_supported": true
        });
        let rendered = super::format_thermal_channels(&output).join("\n");
        assert!(rendered.contains("GPU_AVG: ch0 = 44.5 C"), "{rendered}");
        assert!(rendered.contains("HOT_SPOT: ch1 = 60.8 C"), "{rendered}");
        assert!(
            rendered.contains("ch3: unclassified = 59.2 C"),
            "{rendered}"
        );
        // Pairing annotation + research fields + decoded range (integer
        // degrees render without decimals through format_measurement).
        assert!(
            rendered.contains("same sensor as ch2 (+offset_hw)"),
            "{rendered}"
        );
        assert!(rendered.contains("offset_hw 3736"), "{rendered}");
        assert!(rendered.contains("scaling 256"), "{rendered}");
        assert!(rendered.contains("range -40 C..150 C"), "{rendered}");
    }

    #[test]
    fn thermal_channels_formatter_renders_register_views() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "channel_mask": "0x000000ff",
            "primary": [],
            "channels": [],
            "status_supported": true,
            "register_views": {
                "ram_type": "GDDR7",
                "vram_chips": [
                    {"label": "A0 (Front)", "temp_c": 45},
                    {"label": "A0 (Back)", "temp_c": 41}
                ],
                "vram_chip_error": null,
                "blackwell_hotspot_register_c": 52
            }
        });
        let rendered = super::format_thermal_channels(&output).join("\n");
        assert!(rendered.contains("Register views"), "{rendered}");
        assert!(rendered.contains("ram_type: GDDR7"), "{rendered}");
        assert!(
            rendered.contains("VRAM chip A0 (Front): 45 C"),
            "{rendered}"
        );
        assert!(rendered.contains("VRAM chip A0 (Back): 41 C"), "{rendered}");
        assert!(
            rendered.contains("Blackwell hotspot register: 52 C"),
            "{rendered}"
        );

        // Gated-out card: the probe set note carries the RAM type…
        let gddr6 = json!({
            "primary": [],
            "channels": [],
            "register_views": {
                "ram_type": "GDDR6",
                "vram_chips": [],
                "vram_chip_error": null,
                "blackwell_hotspot_register_c": null
            }
        });
        let rendered = super::format_thermal_channels(&gddr6).join("\n");
        assert!(
            rendered.contains("VRAM chips: none (probe set is GDDR6x/GDDR7; this is GDDR6)"),
            "{rendered}"
        );

        // …and an errored op degrades to the N/A line with the reason.
        let failed = json!({
            "primary": [],
            "channels": [],
            "register_views": {"supported": false, "error": "RegisterOp refused"}
        });
        let rendered = super::format_thermal_channels(&failed).join("\n");
        assert!(
            rendered.contains("Register views: N/A (RegisterOp refused)"),
            "{rendered}"
        );
    }

    #[test]
    fn human_output_formats_vfp_points_as_rows() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "domain": "all",
            "indexed": true,
            "infer_missing_default": true,
            "segments": [
                {"domain": "graphics", "count": 1, "first_index": 12, "last_index": 12},
                {"domain": "memory", "count": 1, "first_index": 127, "last_index": 127},
            ],
            "points": [
                {
                    "domain": "graphics",
                    "index": 12,
                    "point_type": "programmable",
                    "voltage_mv": 900.0,
                    "frequency_mhz": 1800.0,
                    "delta_mhz": 15.0,
                    "default_frequency_mhz": 1785.0,
                },
                {
                    "domain": "memory",
                    "index": 127,
                    "point_type": "fixed",
                    "voltage_mv": 600.0,
                    "frequency_mhz": 405.0,
                    "delta_mhz": 0.0,
                    "default_frequency_mhz": 405.0,
                }
            ],
        });

        let rendered = render_command_output(Command::GetPublicVftable, &output).join("\n");

        assert!(rendered.contains("V-F Points"));
        assert!(rendered.contains("#12 [programmable]: 900.0 mV, 1800.0 MHz, delta 15.0 MHz"));
        // Segment separators make the graphics/memory boundary explicit.
        assert!(rendered.contains("--- graphics (index 12..12) ---"));
        assert!(rendered.contains("--- memory (index 127..127) ---"));
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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

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
        // Sensors: compact primary line (the research metadata — Channel
        // Num/Type, Range, offsets — moved to get-thermal-channels);
        // temperature goes through the house measurement formatter.
        assert!(!rendered.contains("Target"));
        assert!(!rendered.contains("Name:"));
        assert!(!rendered.contains("Channel Type"));
        assert!(!rendered.contains("Offset"));
        assert!(rendered.contains("GPU_AVG: 52.582 C"));
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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

        // The tuple path no longer renders ranges at all (research metadata
        // lives in get-thermal-channels); the primary collapses to one line.
        assert!(rendered.contains("HOT_SPOT: 78.5 C"));
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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

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

        let rendered = render_command_output(Command::GetStatus, &output).join("\n");

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
        let rendered_obj = render_command_output(
            Command::GetStatus,
            &json!({ "perf": { "unknown": 1, "limits": { "bits": 3 } } }),
        )
        .join("\n");
        assert!(rendered_obj.contains("Limits: Power, Temperature"));
        assert!(rendered_obj.contains("Load Level: Load"));

        let rendered_raw = render_command_output(
            Command::GetStatus,
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
        let rendered = render_command_output(Command::GetStatus, &output).join("\n");
        assert!(rendered.contains("Performance Decrease: Thermal Protection, Power Control"));

        // bits = 0 (idle GPU) -> None.
        let rendered_none = render_command_output(
            Command::GetStatus,
            &json!({ "performance_decrease": { "bits": 0 } }),
        )
        .join("\n");
        assert!(rendered_none.contains("Performance Decrease: None"));
    }

    #[test]
    fn human_output_promotes_nvapi_interface_version_under_ok() {
        nvoc_cli_common::color::init(true);
        let output = json!({
            "name": "NVIDIA GeForce RTX 5060 Laptop GPU",
            "nvapi_interface_version": "R580",
            "bios_version": "90.16.34.00.60",
        });

        let lines = render_command_output(Command::GetInfo, &output);

        // Banner lands first, i.e. directly under the "ok" marker.
        assert_eq!(
            lines.first().map(String::as_str),
            Some("NVAPI Interface Version: R580")
        );
        // Rendered exactly once — not repeated inside the identity block.
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Interface Version"))
                .count(),
            1
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Bios Version: 90.16.34.00.60"))
        );
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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");

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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");

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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");

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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");

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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");

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

        let rendered = render_command_output(Command::GetSettings, &output).join("\n");

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

        let rendered = render_command_output(Command::GetInfo, &output).join("\n");
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

        let rendered = render_command_output(Command::GetThrottleReasons, &output).join("\n");

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

        let rendered = render_command_output(Command::GetThrottleReasons, &output).join("\n");

        assert!(rendered.contains("Reason GPU Idle | Active yes"));
        assert!(!rendered.contains("Violation Status"));
    }

    #[test]
    fn human_output_renders_every_function_without_json_dump() {
        nvoc_cli_common::color::init(true);

        for command in all_commands() {
            let rendered = render_command_output(command, &sample_output(command)).join("\n");
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
            function: "get-public-power-limit",
            command: Command::GetPublicPowerLimit,
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
        assert!(rendered.contains("\"function\":\"get-public-power-limit\""));
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
                "source": "nvml",
                "min_watt": 70,
                "current_watt": 100,
                "max_watt": 100,
            }),
            Command::GetPublicTempLimit => json!({
                "min_temp_c": 65,
                "default_temp_c": 83,
                "max_temp_c": 91,
                "curve": "Default",
            }),
            Command::GetLegacyGpcRailVoltRange => {
                json!([{"pstate": "P0", "min_uv": 0, "current_uv": 0, "max_uv": 100000}])
            }
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
            Command::SetPwrCurLimit => json!({"applied": true, "tgp_watt": 140, "tgp_mw": 140000}),
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
                "rail_mask": "0x00000003",
                "p0_rails": [
                    {
                        "rail_bit": 0,
                        "current_uV": 700000,
                        "target_wall_uV": 750000,
                        "effective_wall_uV": 750000,
                        "vbios_wall_uV": 0,
                        "vrm_max_wall_uV": 1200000,
                        "min_hold_uV": 600000,
                        "offset_ceiling_uV": 450000,
                    },
                    {
                        "rail_bit": 1,
                        "current_uV": 710000,
                        "target_wall_uV": 760000,
                        "effective_wall_uV": 760000,
                        "vbios_wall_uV": 0,
                        "vrm_max_wall_uV": 1200000,
                        "min_hold_uV": 610000,
                        "offset_ceiling_uV": 440000,
                    },
                ],
                "rail_descriptors": [{
                    "rail_bit": 0,
                    "type": 1,
                    "class": 1,
                    "uv_a": 750000,
                    "uv_b": 820000,
                }, {
                    "rail_bit": 1,
                    "type": 3,
                    "class": 3,
                    "uv_a": 700000,
                    "uv_b": 700000,
                }],
                "control": [{
                    "rail_bit": 0, "type": 3, "values_uV": [0, 0, 0, 0, 0, 0],
                }, {
                    "rail_bit": 1, "type": 3, "values_uV": [0, 0, 0, 0, 0, 0],
                }],
                "status": [{
                    "rail_bit": 0, "type": 1, "values_uV": [700000, 750000, 0, 1200000, 750000, 600000],
                }, {
                    "rail_bit": 1, "type": 3, "values_uV": [710000, 760000, 0, 1200000, 760000, 610000],
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
                "previous_mHz": 0, "applied_mHz": -60, "temporary_restored": true,
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

    #[test]
    fn ext_currents_legend_names_the_resolved_roster_not_the_pool() {
        nvoc_cli_common::color::init(true);
        let payload = |relations: Option<Value>| {
            let mut value = json!({
                "segments": [{
                    "bank": 0, "domain": "xbar", "kind": "vf_curve", "type": 5,
                    "start_index": 0, "end_index": 1, "count": 2,
                    "voltage_uV_min": 450000, "voltage_uV_max": 1240000,
                    "freq_default_mhz_min": 225, "freq_default_mhz_max": 2370,
                }],
                "points": [
                    {"bank": 0, "index": 0, "type": 5, "voltage_uV": 450000,
                     "volt_current_mv": 450, "freq_current_mhz": 210,
                     "freq_default_mhz": 210,
                     "domain_currents": {"0": [210.0, 450000.0], "1": [225.0, 450000.0]}},
                    {"bank": 0, "index": 1, "type": 5, "voltage_uV": 1240000,
                     "volt_current_mv": 1240, "freq_current_mhz": 2370,
                     "freq_default_mhz": 2370,
                     "domain_currents": {"0": [2370.0, 1240000.0], "1": [1350.0, 1240000.0]}},
                ],
            });
            if let Some(relations) = relations {
                value["fabric_relations"] = relations;
            }
            value
        };

        // Ada: MSD owns a main vf_curve block, so the xbar record packs
        // SYS/HOST — the pool's MSD must not be named anywhere in the view
        let ada = payload(Some(json!({
            "source": "vftable-ext", "table_available": true,
            "roster": ["sys", "host"], "edges": [],
        })));
        let text = format_private_vfp_output(&ada).join("\n");
        assert!(text.contains("roster [SYS,HOST]"), "{text}");
        assert!(text.contains("xbar ext0 (SYS)"), "{text}");
        assert!(text.contains("xbar ext1 (HOST)"), "{text}");
        assert!(!text.contains("MSD"), "{text}");

        // no relation block at all (older build, or the read failed): the
        // relation is unknown, so neither the summary nor the legend names
        // a domain — the slots stay `?` rather than take the pool guess
        let legacy = payload(None);
        let text = format_private_vfp_output(&legacy).join("\n");
        assert!(
            text.contains("roster [unknown: no fabric relation in this payload]"),
            "{text}"
        );
        assert!(text.contains("ext: ext0=? ext1=?"), "{text}");
        assert!(!text.contains("(SYS)"), "{text}");
        assert!(!text.contains("(HOST)"), "{text}");
        assert!(!text.contains("MSD"), "{text}");
    }
}
