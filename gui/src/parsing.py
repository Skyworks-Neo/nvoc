"""GUI-local parsers and VFP CSV helpers."""

from __future__ import annotations

import csv
import json
import os
import re
from typing import Any, Optional, Tuple


def voltage_text_to_mv(value_text: str, unit_text: str) -> int:
    value = float(value_text)
    unit = unit_text.lower()
    if unit in {"uv", "µv", "μv"}:
        return int(round(value / 1000.0))
    return int(round(value))


def native_query_payload(output: str) -> Optional[dict[str, Any]]:
    text = output.strip()
    start = text.find("{")
    if start < 0:
        return None
    try:
        payload = json.loads(text[start:])
    except json.JSONDecodeError:
        return None
    return payload if isinstance(payload, dict) else None


def as_float(value: object) -> Optional[float]:
    if isinstance(value, (int, float)):
        return float(value)
    return None


def parse_gpu_list_output(
    output: str,
) -> tuple[dict[int, str], dict[int, str], dict[int, str], dict[int, str]]:
    gpu_pattern = re.compile(r"^GPU\s+(\d+)\s*:\s*(.+)$")
    uuid_pattern = re.compile(r"^UUID=(GPU-[\w-]+)")
    short_labels: dict[int, str] = {}
    gpu_names: dict[int, str] = {}
    uuid_map: dict[int, str] = {}
    last_gpu_idx: Optional[int] = None

    for line in output.strip().split("\n"):
        line = line.strip()
        match = gpu_pattern.match(line)
        if match:
            idx = int(match.group(1))
            name = match.group(2).strip()
            inline_uuid = re.search(r"(?i)\buuid\s*[:=]\s*(GPU-[\w-]+)", name)
            if inline_uuid:
                uuid_map[idx] = inline_uuid.group(1)
            name = re.split(r"(?i)\buuid\s*[:=]\s*gpu-[\w-]+", name, maxsplit=1)[
                0
            ].strip()
            name = re.sub(r"\s*\[\s*GPU-[\w-]+\s*\].*$", "", name, flags=re.IGNORECASE)
            last_gpu_idx = idx
            if idx not in short_labels:
                short_labels[idx] = f"GPU {idx}: {name}"
                gpu_names[idx] = name
            continue
        uuid_match = uuid_pattern.match(line)
        if uuid_match and last_gpu_idx is not None:
            uuid_map[last_gpu_idx] = uuid_match.group(1)

    long_labels = {
        idx: f"{short}  [{uuid_map[idx]}]" if idx in uuid_map else short
        for idx, short in short_labels.items()
    }
    return short_labels, long_labels, gpu_names, uuid_map


def parse_info_limits(output: str) -> dict[str, Any]:
    native_payload = native_query_payload(output)
    if native_payload is not None:
        limits = dict(native_payload)
        if "arch" in limits and "gpu_architecture" not in limits:
            limits["gpu_architecture"] = limits["arch"]
        return limits

    limits: dict[str, Any] = {}
    for line in output.split("\n"):
        line = line.strip()
        if line.startswith("Architecture"):
            parts = line.split(":", 1)
            if len(parts) == 2 and parts[1].strip():
                limits["gpu_architecture"] = parts[1].strip()
        elif line.startswith("VFP (Graphics)"):
            match = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
            if match:
                limits["core_clock_min"] = int(match.group(1))
                limits["core_clock_max"] = int(match.group(2))
        elif line.startswith("VFP (Memory)"):
            match = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
            if match:
                limits["mem_clock_min"] = int(match.group(1))
                limits["mem_clock_max"] = int(match.group(2))
        elif line.startswith("Power Limit"):
            match = re.search(r"(\d+)%\s*~\s*(\d+)%\s*\((\d+)%\s*default\)", line)
            if match:
                limits["power_limit_min"] = int(match.group(1))
                limits["power_limit_max"] = int(match.group(2))
                limits["power_limit_default"] = int(match.group(3))
            current = re.search(r"\|\s*(\d+)%\s*current", line)
            if current:
                limits["power_limit_current"] = int(current.group(1))
            watts = re.search(
                r"(\d+)W\s*min\s*/\s*(\d+)W\s*current\s*/\s*(\d+)W\s*max", line
            )
            if watts:
                limits["power_watt_min"] = int(watts.group(1))
                limits["power_watt_current"] = int(watts.group(2))
                limits["power_watt_max"] = int(watts.group(3))
        elif line.startswith("Thermal Limit"):
            match = re.search(
                r"(\d+)\s*C\s*~\s*(\d+)\s*C\s*\((\d+)\s*C\s*default\)", line
            )
            if match:
                limits["thermal_limit_min"] = int(match.group(1))
                limits["thermal_limit_max"] = int(match.group(2))
                limits["thermal_limit_default"] = int(match.group(3))
            current = re.search(r"\|\s*(\d+)\s*C\s*current", line)
            if current:
                limits["thermal_limit_current"] = int(current.group(1))
        elif line.startswith("Overvolt"):
            match = re.search(
                r"^Overvolt\s+(P\d+).*?:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\(\s*range\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*-\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\)",
                line,
                re.IGNORECASE,
            )
            if match:
                pstate = match.group(1).upper()
                if limits.get("legacy_overvolt_pstate") == "P0" and pstate != "P0":
                    continue
                limits["legacy_overvolt_pstate"] = pstate
                limits["legacy_overvolt_current_mv"] = voltage_text_to_mv(
                    match.group(2), match.group(3)
                )
                limits["legacy_overvolt_min_mv"] = voltage_text_to_mv(
                    match.group(4), match.group(5)
                )
                limits["legacy_overvolt_max_mv"] = voltage_text_to_mv(
                    match.group(6), match.group(7)
                )
    return limits


def parse_status_current_values(output: str) -> Tuple[Optional[float], dict[str, Any]]:
    locked_voltage_mv: Optional[float] = None
    limits_update: dict[str, Any] = {}

    native_payload = native_query_payload(output)
    if native_payload is not None:
        lock_value = native_payload.get("vfp_lock_mv")
        if isinstance(lock_value, (int, float)):
            locked_voltage_mv = float(lock_value)
        for key in (
            "core_clock_current",
            "mem_clock_current",
            "power_limit_current",
            "thermal_limit_current",
            "voltage_boost_current",
        ):
            value = native_payload.get(key)
            if isinstance(value, (int, float)):
                limits_update[key] = int(value)
        return locked_voltage_mv, limits_update

    for line in output.splitlines():
        line_s = line.strip()
        if re.search(r"vfp lock", line_s, re.IGNORECASE):
            match = re.search(
                r"voltage\s*:\s*(\d+(?:\.\d+)?)\s*mv", line_s, re.IGNORECASE
            )
            if match:
                locked_voltage_mv = float(match.group(1))
        elif re.search(r"Graphics.*Offset|OC\s*\(Graphics\)", line_s, re.IGNORECASE):
            match = re.search(r"([+-]?\d+)\s*MHz", line_s)
            if match:
                limits_update["core_clock_current"] = int(match.group(1))
        elif re.search(r"Memory.*Offset|OC\s*\(Memory\)", line_s, re.IGNORECASE):
            match = re.search(r"([+-]?\d+)\s*MHz", line_s)
            if match:
                limits_update["mem_clock_current"] = int(match.group(1))
        elif re.search(r"power limit", line_s, re.IGNORECASE):
            match = re.search(r"([+-]?\d+)\s*%", line_s)
            if match:
                limits_update["power_limit_current"] = int(match.group(1))
        elif re.search(r"thermal limit", line_s, re.IGNORECASE):
            match = re.search(r"(\d+)\s*[Cc]", line_s)
            if match:
                limits_update["thermal_limit_current"] = int(match.group(1))
        elif re.search(r"voltage boost", line_s, re.IGNORECASE):
            match = re.search(r"([+-]?\d+)\s*%", line_s)
            if match:
                limits_update["voltage_boost_current"] = int(match.group(1))

    return locked_voltage_mv, limits_update


def parse_supported_pstates(output: str) -> list[str]:
    match = re.search(
        r"Supported P-States:\s*(.*?)(?:\n\s*Supported Applications Clocks:|\Z)",
        output,
        re.IGNORECASE | re.DOTALL,
    )
    if not match:
        return []

    seen: set[str] = set()
    pstates: list[str] = []
    for raw in match.group(1).splitlines():
        state_match = re.match(r"^P\s*(\d+)\s*:", raw.strip(), re.IGNORECASE)
        if not state_match:
            continue
        label = f"P{int(state_match.group(1))}"
        if label not in seen:
            seen.add(label)
            pstates.append(label)
    return pstates


def parse_nvml_power_limits_from_get(output: str) -> dict[str, int]:
    match = re.search(
        r"^\s*Power\s+Limit\s*:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*\(\s*Min:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*-\s*Max:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*\)",
        output,
        re.IGNORECASE | re.MULTILINE,
    )
    if not match:
        return {}
    return {
        "power_limit_nvml_current_w": int(round(float(match.group(1)))),
        "power_limit_nvml_min_w": int(round(float(match.group(2)))),
        "power_limit_nvml_max_w": int(round(float(match.group(3)))),
    }


def parse_nvapi_power_current_from_get(output: str) -> dict[str, int]:
    match = re.search(
        r"^\s*Power\s+Limit\.*\s*:\s*([+-]?\d+)\s*%",
        output,
        re.IGNORECASE | re.MULTILINE,
    )
    return {"power_limit_current": int(match.group(1))} if match else {}


def parse_vfp_lock_bounds(output: str) -> dict[str, int]:
    bounds: dict[str, int] = {}
    patterns = {
        "vfp_lock_gpu_core_upperbound_mhz": r"^\s*VFP\s+Lock\s+GPU\s+Core\s+Upperbound\s*:\s*([+-]?\d+)\s*MHz\b",
        "vfp_lock_gpu_core_lowerbound_mhz": r"^\s*VFP\s+Lock\s+GPU\s+Core\s+Lowerbound\s*:\s*([+-]?\d+)\s*MHz\b",
        "vfp_lock_memory_upperbound_mhz": r"^\s*VFP\s+Lock\s+Memory\s+Upperbound\s*:\s*([+-]?\d+)\s*MHz\b",
        "vfp_lock_memory_lowerbound_mhz": r"^\s*VFP\s+Lock\s+Memory\s+Lowerbound\s*:\s*([+-]?\d+)\s*MHz\b",
    }
    for key, pattern in patterns.items():
        match = re.search(pattern, output, re.IGNORECASE | re.MULTILINE)
        if match:
            bounds[key] = int(match.group(1))
    return bounds


def _frequency_value_to_mhz(value: object) -> Optional[int]:
    if isinstance(value, (int, float)):
        return int(round(value / 1000.0 if abs(value) >= 10000 else value))
    if not isinstance(value, str):
        return None

    match = re.search(r"([+-]?\d+(?:\.\d+)?)\s*([kmg]?hz)?", value, re.IGNORECASE)
    if not match:
        return None
    raw = float(match.group(1))
    unit = (match.group(2) or "").lower()
    if unit == "khz":
        raw /= 1000.0
    elif unit == "ghz":
        raw *= 1000.0
    elif not unit and abs(raw) >= 10000:
        raw /= 1000.0
    return int(round(raw))


def normalize_native_vfp_lock_bounds(payload: dict[str, Any]) -> dict[str, int]:
    """Map pynvoc VFP lock payloads onto the GUI's legacy lock-bound keys."""
    locks = payload.get("vfp_locks")
    if not isinstance(locks, dict):
        return {}

    bounds: dict[str, int] = {}
    for raw_key, raw_value in locks.items():
        value = _frequency_value_to_mhz(raw_value)
        if value is None:
            continue

        key = re.sub(r"[^a-z0-9]+", "", str(raw_key).lower())
        if "memory" in key or key.startswith("mem"):
            prefix = "vfp_lock_memory"
        elif "gpu" in key or "graphics" in key or "core" in key:
            prefix = "vfp_lock_gpu_core"
        else:
            continue

        suffix = (
            "lowerbound_mhz"
            if ("lower" in key or "min" in key or "unknown" in key)
            else "upperbound_mhz"
        )
        bounds[f"{prefix}_{suffix}"] = value
    return bounds


def parse_legacy_overvolt_bounds(output: str) -> dict[str, int | str]:
    pattern = re.compile(
        r"^\s*Overvolt\s+(P\d+)\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\(\s*range\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*-\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\)",
        re.IGNORECASE,
    )
    rows: list[tuple[str, int, int, int]] = []
    for line in output.splitlines():
        match = pattern.match(line.strip())
        if match:
            rows.append((
                match.group(1).upper(),
                voltage_text_to_mv(match.group(2), match.group(3)),
                voltage_text_to_mv(match.group(4), match.group(5)),
                voltage_text_to_mv(match.group(6), match.group(7)),
            ))
    if not rows:
        return {}
    selected = next((row for row in rows if row[0] == "P0"), rows[0])
    return {
        "legacy_overvolt_pstate": selected[0],
        "legacy_overvolt_current_mv": selected[1],
        "legacy_overvolt_min_mv": selected[2],
        "legacy_overvolt_max_mv": selected[3],
    }


def parse_dashboard_status(output: str) -> dict[str, Any]:
    parsed = {
        "gpu_clock_mhz": None,
        "mem_clock_mhz": None,
        "voltage_mv": None,
        "temperature_c": None,
        "power_w": None,
        "vfp_locked": None,
    }
    native_payload = native_query_payload(output)
    if native_payload is not None:
        parsed["gpu_clock_mhz"] = as_float(native_payload.get("gpu_clock_mhz"))
        parsed["mem_clock_mhz"] = as_float(native_payload.get("mem_clock_mhz"))
        parsed["voltage_mv"] = as_float(native_payload.get("voltage_mv"))
        parsed["temperature_c"] = as_float(native_payload.get("temperature_c"))
        parsed["power_w"] = as_float(native_payload.get("power_w"))
        locked_value = native_payload.get("vfp_locked")
        parsed["vfp_locked"] = (
            bool(locked_value) if isinstance(locked_value, bool) else None
        )
        return parsed

    for raw in output.splitlines():
        line = raw.strip()
        low = line.lower()
        if parsed["gpu_clock_mhz"] is None and re.search(
            r"graphics.clock|core.clock|gpu.clock", low
        ):
            match = re.search(r"(\d+(?:\.\d+)?)\s*mhz", low)
            if match:
                parsed["gpu_clock_mhz"] = float(match.group(1))
        if parsed["mem_clock_mhz"] is None and re.search(r"mem(?:ory)?.clock", low):
            match = re.search(r"(\d+(?:\.\d+)?)\s*mhz", low)
            if match:
                parsed["mem_clock_mhz"] = float(match.group(1))
        if parsed["voltage_mv"] is None and re.search(
            r"(?:core|gpu).volt(?:age)?", low
        ):
            match = re.search(r"(\d+(?:\.\d+)?)\s*mv", low)
            if match:
                parsed["voltage_mv"] = float(match.group(1))
            if re.search(r"\(locked\)", low):
                parsed["vfp_locked"] = True
            elif parsed["vfp_locked"] is None:
                parsed["vfp_locked"] = False
        if re.search(r"vfp.lock", low) and re.search(
            r"voltage:(\d+(?:\.\d+)?)\s*mv", low
        ):
            parsed["vfp_locked"] = True
        if parsed["temperature_c"] is None and "sensor" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*c\b", low)
            if match:
                parsed["temperature_c"] = float(match.group(1))
        if parsed["temperature_c"] is None and "temp" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*(?:°?c\b|celsius)", low)
            if match:
                parsed["temperature_c"] = float(match.group(1))
        if parsed["power_w"] is None and re.search(r"power.usage", low):
            match = re.search(r"(\d+(?:\.\d+)?)\s*%\s*\(normalized", low)
            if not match:
                match = re.search(r"(\d+(?:\.\d+)?)\s*%\s*\(total", low)
            if match:
                parsed["power_w"] = float(match.group(1))
        if parsed["power_w"] is None and re.search(
            r"power.(?:draw|consumption)|power\s*:", low
        ):
            match = re.search(r"(\d+(?:\.\d+)?)\s*w\b", low)
            if match:
                parsed["power_w"] = float(match.group(1))
    return parsed


def analyze_vfp_offsets(
    frequencies: list[float], defaults: list[float]
) -> tuple[bool, Optional[int]]:
    if (not frequencies) or (len(frequencies) != len(defaults)):
        return False, None

    eps = 1e-4
    offsets = [freq - default for freq, default in zip(frequencies, defaults)]
    if all(abs(offset - offsets[0]) <= eps for offset in offsets):
        if abs(offsets[0]) <= eps:
            return False, None
        return True, int(round(offsets[0]))
    return any(abs(offset) > eps for offset in offsets), None


def get_vfp_offset_state_from_csv(
    csv_path: str,
) -> Optional[tuple[bool, Optional[int]]]:
    if not os.path.isfile(csv_path):
        return None
    frequencies: list[float] = []
    defaults: list[float] = []
    try:
        with open(csv_path, newline="", encoding="utf-8-sig") as file:
            reader = csv.reader(file)
            for row in reader:
                if not row or row[0].startswith("#"):
                    continue
                if row[0].strip().lower() in {"voltage_uv", "voltage", "uv"}:
                    continue
                try:
                    freq = float(row[1]) / 1000.0
                    default = float(row[3]) / 1000.0 if len(row) > 3 else freq
                except (ValueError, IndexError):
                    continue
                frequencies.append(freq)
                defaults.append(default)
    except Exception:
        return None
    return analyze_vfp_offsets(frequencies, defaults)


def write_vfp_points(path: str, points: list[dict[str, Any]]) -> None:
    with open(path, "w", newline="", encoding="utf-8") as file:
        writer = csv.writer(file)
        writer.writerow(["voltage", "frequency", "delta", "default_frequency"])
        for point in points:
            writer.writerow([
                point.get("voltage_uv", 0),
                point.get("frequency_khz", 0),
                point.get("delta_khz", 0),
                point.get("default_frequency_khz", 0),
            ])


def load_vfp_deltas(
    path: str, reference_points: list[dict[str, Any]]
) -> list[tuple[int, int]]:
    reference_by_voltage = {
        int(point.get("voltage_uv", -1)): point for point in reference_points
    }
    deltas: list[tuple[int, int]] = []
    with open(path, newline="", encoding="utf-8-sig") as file:
        reader = csv.reader(file)
        for row_index, row in enumerate(reader):
            if not row or row[0].startswith("#"):
                continue
            if row[0].strip().lower() in {"voltage", "voltage_uv", "uv"}:
                continue
            try:
                voltage_uv = int(float(row[0]))
                frequency_khz = int(round(float(row[1])))
            except (IndexError, ValueError):
                continue
            reference = reference_by_voltage.get(voltage_uv)
            if reference is None:
                continue
            point_index = int(reference.get("index", row_index))
            default_khz = int(reference.get("default_frequency_khz", frequency_khz))
            deltas.append((point_index, frequency_khz - default_khz))
    return deltas
