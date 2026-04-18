from __future__ import annotations

import json
import re
from math import ceil
from pathlib import Path
from typing import Any

from .models import GpuDescriptor


GPU_LINE_RE = re.compile(r"^GPU\s+(\d+)\s*:\s*(.+)$")
UUID_LINE_RE = re.compile(r"UUID=(GPU-[\w-]+)", re.IGNORECASE)


def parse_json_output(output: str) -> Any | None:
    stripped = output.strip()
    if not stripped or not stripped.startswith(("{", "[")):
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        return None


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
            parsed["gpu_architecture"] = value
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
                return value
        if isinstance(parsed_json, dict):
            return parsed_json
    if command == "info":
        return parse_info_output(output)
    if command == "status":
        return parse_status_output(output)
    if command == "get":
        return parse_get_output(output)
    return {}


def vf_curve_plot(path: str, width: int = 90, height: int = 20) -> str:
    csv_path = Path(path)
    if not csv_path.is_file():
        return "No VF curve cache loaded."

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

    if not voltages:
        return "VF curve cache is empty."

    plot_width = max(24, width)
    plot_height = max(8, height)
    left_pad = 8
    inner_width = max(8, plot_width - left_pad - 1)
    inner_height = max(4, plot_height - 4)

    min_v = min(voltages)
    max_v = max(voltages)
    min_f = min(min(freqs), min(defaults))
    max_f = max(max(freqs), max(defaults))
    if max_v == min_v:
        max_v += 1.0
    if max_f == min_f:
        max_f += 1.0

    grid = [[" " for _ in range(inner_width)] for _ in range(inner_height)]

    def place(series_x: list[float], series_y: list[float], marker: str) -> None:
        for x_val, y_val in zip(series_x, series_y):
            x_ratio = (x_val - min_v) / (max_v - min_v)
            y_ratio = (y_val - min_f) / (max_f - min_f)
            x = min(inner_width - 1, max(0, int(round(x_ratio * (inner_width - 1)))))
            y = min(inner_height - 1, max(0, int(round((1.0 - y_ratio) * (inner_height - 1)))))
            current = grid[y][x]
            if current != " " and current != marker:
                grid[y][x] = "*"
            else:
                grid[y][x] = marker

    place(voltages, defaults, ".")
    place(voltages, freqs, "#")

    lines: list[str] = []
    title = "VF Curve (# current, . default)"
    lines.append(title[:plot_width].ljust(plot_width))
    for row_idx, row in enumerate(grid):
        freq_value = max_f - ((max_f - min_f) * row_idx / max(1, inner_height - 1))
        label = f"{int(round(freq_value)):>6} "
        lines.append(f"{label}|{''.join(row)}")

    axis = " " * left_pad + "+" + "-" * inner_width
    lines.append(axis[:plot_width])
    min_label = f"{int(round(min_v))}mV"
    max_label = f"{int(round(max_v))}mV"
    gap = max(1, plot_width - len(min_label) - len(max_label))
    lines.append((min_label + (" " * gap) + max_label)[:plot_width].ljust(plot_width))
    return "\n".join(lines)
