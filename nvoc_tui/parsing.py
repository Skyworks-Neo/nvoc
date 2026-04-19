from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from .models import GpuDescriptor


GPU_LINE_RE = re.compile(r"^GPU\s+(\d+)\s*:\s*(.+)$")
UUID_LINE_RE = re.compile(r"UUID=(GPU-[\w-]+)", re.IGNORECASE)


def parse_json_output(output: str) -> Any | None:
    stripped = output.strip()
    if not stripped:
        return None
    decoder = json.JSONDecoder()
    candidate_indexes = [idx for idx, char in enumerate(stripped) if char in "[{"]
    for idx in candidate_indexes:
        try:
            parsed, _ = decoder.raw_decode(stripped[idx:])
        except json.JSONDecodeError:
            continue
        return parsed
    return None


def _as_float(value: Any) -> float | None:
    if isinstance(value, (int, float)):
        return float(value)
    return None


def _normalize_status_json(value: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(value)

    clocks = value.get("clocks")
    if isinstance(clocks, dict):
        graphics = _as_float(clocks.get("Graphics"))
        memory = _as_float(clocks.get("Memory"))
        if graphics is not None:
            normalized["gpu_clock_mhz"] = graphics / 1000.0
        if memory is not None:
            normalized["mem_clock_mhz"] = memory / 1000.0

    voltage = _as_float(value.get("voltage"))
    if voltage is not None:
        normalized["voltage_mv"] = voltage / 1000.0

    sensors = value.get("sensors")
    if isinstance(sensors, list):
        for entry in sensors:
            if (
                isinstance(entry, list)
                and len(entry) >= 2
                and isinstance(entry[1], (int, float))
            ):
                normalized["temperature_c"] = float(entry[1])
                break

    power = value.get("power")
    if isinstance(power, dict):
        total_gpu_power = _as_float(power.get("TotalGpuPower"))
        if total_gpu_power is not None:
            normalized["power_w"] = total_gpu_power

    return normalized


def parse_gpu_list(output: str) -> list[GpuDescriptor]:
    gpus: dict[int, GpuDescriptor] = {}
    last_idx: int | None = None
    for raw in output.splitlines():
        line = raw.strip()
        match = GPU_LINE_RE.match(line)
        if match:
            idx = int(match.group(1))
            name = match.group(2).strip()
            uuid_match = re.search(r"(?i)\buuid\s*[:=]\s*(GPU-[\w-]+)", name)
            uuid = uuid_match.group(1) if uuid_match else None
            name = re.split(r"(?i)\buuid\s*[:=]\s*gpu-[\w-]+", name, maxsplit=1)[0].strip()
            if name.startswith("ID:") and idx in gpus:
                continue
            gpus[idx] = GpuDescriptor(index=idx, name=name, uuid=uuid)
            last_idx = idx
            continue
        uuid_match = UUID_LINE_RE.search(line)
        if uuid_match and last_idx is not None and last_idx in gpus:
            gpus[last_idx].uuid = uuid_match.group(1)
    return [gpus[idx] for idx in sorted(gpus)]


def parse_info_output(output: str) -> dict[str, Any]:
    parsed: dict[str, Any] = {}
    for raw in output.splitlines():
        line = raw.strip()
        if line.startswith("Architecture"):
            value = line.split(":", 1)[1].strip()
            parsed["arch"] = value
        elif line.startswith("VFP (Graphics)"):
            match = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
            if match:
                parsed["core_clock_min"] = int(match.group(1))
                parsed["core_clock_max"] = int(match.group(2))
        elif line.startswith("VFP (Memory)"):
            match = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
            if match:
                parsed["mem_clock_min"] = int(match.group(1))
                parsed["mem_clock_max"] = int(match.group(2))
        elif line.startswith("Power Limit"):
            match = re.search(r"(\d+)%\s*~\s*(\d+)%\s*\((\d+)%\s*default\)", line)
            if match:
                parsed["power_limit_min"] = int(match.group(1))
                parsed["power_limit_max"] = int(match.group(2))
                parsed["power_limit_default"] = int(match.group(3))
            watts = re.search(r"(\d+)W\s*min\s*/\s*(\d+)W\s*current\s*/\s*(\d+)W\s*max", line)
            if watts:
                parsed["power_limit_nvml_min_w"] = int(watts.group(1))
                parsed["power_limit_nvml_current_w"] = int(watts.group(2))
                parsed["power_limit_nvml_max_w"] = int(watts.group(3))
        elif line.startswith("Thermal Limit"):
            match = re.search(r"(\d+)\s*C\s*~\s*(\d+)\s*C\s*\((\d+)\s*C\s*default\)", line)
            if match:
                parsed["thermal_limit_min"] = int(match.group(1))
                parsed["thermal_limit_max"] = int(match.group(2))
                parsed["thermal_limit_default"] = int(match.group(3))
    return parsed


def parse_status_output(output: str) -> dict[str, Any]:
    parsed: dict[str, Any] = {}
    for raw in output.splitlines():
        line = raw.strip()
        low = line.lower()
        if "graphics" in low and "mhz" in low and "gpu_clock_mhz" not in parsed:
            match = re.search(r"(\d+(?:\.\d+)?)\s*mhz", low)
            if match:
                parsed["gpu_clock_mhz"] = float(match.group(1))
        elif "mem" in low and "mhz" in low and "mem_clock_mhz" not in parsed:
            match = re.search(r"(\d+(?:\.\d+)?)\s*mhz", low)
            if match:
                parsed["mem_clock_mhz"] = float(match.group(1))
        elif re.search(r"(?:core|gpu).volt", low):
            match = re.search(r"(\d+(?:\.\d+)?)\s*mv", low)
            if match:
                parsed["voltage_mv"] = float(match.group(1))
            parsed["voltage_locked"] = "(locked)" in low
        elif "sensor" in low or "temp" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*(?:°?c|celsius)", low)
            if match:
                parsed["temperature_c"] = float(match.group(1))
        elif "power" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*w\b", low)
            if match:
                parsed["power_w"] = float(match.group(1))
    return parsed


def parse_get_output(output: str) -> dict[str, Any]:
    parsed: dict[str, Any] = {}
    pstates: list[str] = []
    for raw in output.splitlines():
        line = raw.strip()
        state_match = re.match(r"^P\s*(\d+)\s*:", line, re.IGNORECASE)
        if state_match:
            pstates.append(f"P{int(state_match.group(1))}")
            continue
        if "Core Clock Offset" in line:
            match = re.search(r"([+-]?\d+)\s*MHz", line)
            if match:
                parsed["core_clock_current"] = int(match.group(1))
        elif "Mem Clock Offset" in line or "Memory" in line and "Offset" in line:
            match = re.search(r"([+-]?\d+)\s*MHz", line)
            if match:
                parsed["mem_clock_current"] = int(match.group(1))
        elif "Power Limit" in line and "%" in line:
            match = re.search(r"([+-]?\d+)\s*%", line)
            if match:
                parsed["power_limit_current"] = int(match.group(1))
        elif "Power Limit" in line and "W" in line:
            match = re.search(r"([0-9]+(?:\.[0-9]+)?)\s*W\s*\(Min:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*-\s*Max:\s*([0-9]+(?:\.[0-9]+)?)\s*W", line)
            if match:
                parsed["power_limit_nvml_current_w"] = int(round(float(match.group(1))))
                parsed["power_limit_nvml_min_w"] = int(round(float(match.group(2))))
                parsed["power_limit_nvml_max_w"] = int(round(float(match.group(3))))
    if pstates:
        parsed["supported_pstates"] = pstates
    return parsed


def normalize_query_output(command: str, output: str) -> dict[str, Any]:
    parsed_json = parse_json_output(output)
    if parsed_json is not None:
        if isinstance(parsed_json, list) and parsed_json:
            value = parsed_json[0]
            if isinstance(value, dict):
                if command == "status":
                    return _normalize_status_json(value)
                return value
        if isinstance(parsed_json, dict):
            if command == "status":
                return _normalize_status_json(parsed_json)
            return parsed_json
    if command == "info":
        return parse_info_output(output)
    if command == "status":
        return parse_status_output(output)
    if command == "get":
        return parse_get_output(output)
    return {}


def load_vf_curve(path: str) -> tuple[list[float], list[float], list[float]]:
    csv_path = Path(path)
    if not csv_path.is_file():
        return [], [], []

    voltages: list[float] = []
    freqs: list[float] = []
    defaults: list[float] = []
    for raw in csv_path.read_text(encoding="utf-8-sig").splitlines():
        row = [piece.strip() for piece in raw.split(",")]
        if not row or row[0].startswith("#") or row[0].lower() in {"voltage", "voltage_uv"}:
            continue
        if len(row) < 2:
            continue
        try:
            voltages.append(float(row[0]) / 1000.0)
            freqs.append(float(row[1]) / 1000.0)
            defaults.append(float(row[3]) / 1000.0 if len(row) > 3 else float(row[1]) / 1000.0)
        except ValueError:
            continue

    return voltages, freqs, defaults


def find_curve_point_for_voltage(
    voltages: list[float],
    freqs: list[float],
    voltage_mv: float | None,
) -> tuple[float, float] | None:
    if voltage_mv is None or not voltages or len(voltages) != len(freqs):
        return None

    nearest_index = min(range(len(voltages)), key=lambda idx: abs(voltages[idx] - voltage_mv))
    return voltages[nearest_index], freqs[nearest_index]


def compute_vf_plot_bounds(
    voltages: list[float],
    freqs: list[float],
    defaults: list[float],
    *,
    live_point: tuple[float, float] | None = None,
    working_point: tuple[float, float] | None = None,
) -> tuple[tuple[float, float], tuple[float, float]] | None:
    if not voltages or not freqs or not defaults:
        return None

    x_values = list(voltages)
    y_values = [*freqs, *defaults]
    for point in (live_point, working_point):
        if point is None:
            continue
        x_values.append(point[0])
        y_values.append(point[1])

    x_min = min(x_values)
    x_max = max(x_values)
    y_min = min(0.0, min(y_values))
    y_max = max(y_values)

    x_padding = max(1.0, (x_max - x_min) * 0.03) if x_max > x_min else 1.0
    y_padding = max(1.0, (y_max - y_min) * 0.05) if y_max > y_min else 1.0

    return (
        (x_min - x_padding, x_max + x_padding),
        (y_min, y_max + y_padding),
    )
