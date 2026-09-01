# Copyright (C) 2026 Ajax Dong
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any, cast

from .models import CurveData, EffectiveCurve, GpuDescriptor


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


def _normalize_status_lock_fields(parsed: dict[str, Any]) -> dict[str, Any]:
    # Status can represent the same lock state using either the modern
    # "vfp_locked" key or the legacy "voltage_locked" key.
    lock_state = parsed.get("vfp_locked")
    if not isinstance(lock_state, bool):
        legacy_lock_state = parsed.get("voltage_locked")
        if isinstance(legacy_lock_state, bool):
            lock_state = legacy_lock_state
    if isinstance(lock_state, bool):
        parsed["vfp_locked"] = lock_state
    parsed.pop("voltage_locked", None)
    return parsed


def _normalize_status_json(value: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(value)

    clocks = value.get("clocks")
    if isinstance(clocks, dict):
        graphics = _as_float(clocks.get("Graphics"))
        memory = _as_float(clocks.get("Memory"))
        video = _as_float(clocks.get("Video"))
        if graphics is not None:
            normalized["gpu_clock_mhz"] = graphics / 1000.0
        if memory is not None:
            normalized["mem_clock_mhz"] = memory / 1000.0
        if video is not None:
            normalized["video_clock_mhz"] = video / 1000.0

    voltage = _as_float(value.get("voltage"))
    if voltage is None:
        # Legacy GPUs (≤ Kepler): the private core_voltage() read yields
        # nothing, but the PUBLIC GetVoltageDomainsStatus value in the same
        # status payload carries the authoritative core-domain voltage (the
        # "Voltage Domains → Voltage: 880000 uV" field). Fall back to it so
        # the dashboard shows a real voltage instead of `---`.
        domains = value.get("voltage_domains")
        if isinstance(domains, dict):
            voltage = _as_float(domains.get("voltage"))
    if voltage is not None:
        normalized["voltage_mv"] = voltage / 1000.0

    sensors = value.get("sensors")
    if isinstance(sensors, list):
        # Thermal sensors are `[descriptor, temp]` pairs. The descriptor's
        # `channel_type` identifies the sensor: 0=GPU_AVG (core), 1=GPU_MAX
        # (hot spot), 3=MEMORY (VRAM). Keep the core temp on `temperature_c`
        # (positional default — first sensor) and also expose the typed trio
        # so the dashboard can render whichever are present.
        for entry in sensors:
            if not (
                isinstance(entry, list)
                and len(entry) >= 2
                and isinstance(entry[1], (int, float))
            ):
                continue
            temp = float(entry[1])
            if "temperature_c" not in normalized:
                normalized["temperature_c"] = temp
            descriptor = entry[0] if isinstance(entry[0], dict) else {}
            ch_type = descriptor.get("channel_type")
            if ch_type == 0 and "temp_core" not in normalized:
                normalized["temp_core"] = temp
            elif ch_type == 1 and "temp_hotspot" not in normalized:
                normalized["temp_hotspot"] = temp
            elif ch_type == 3 and "temp_memory" not in normalized:
                normalized["temp_memory"] = temp

    power = value.get("power")
    if isinstance(power, dict):
        total_gpu_power = _as_float(power.get("TotalGpuPower"))
        if total_gpu_power is not None:
            normalized["power_w"] = total_gpu_power

    # Status JSON exposes VFP lock state as a map of active lock bounds.
    # A non-empty map means some VFP lock is currently active.
    vfp_locks = value.get("vfp_locks")
    if isinstance(vfp_locks, dict):
        normalized["vfp_locked"] = bool(vfp_locks)
        for lock in vfp_locks.values():
            if not isinstance(lock, dict):
                continue
            voltage = _as_float(lock.get("Voltage"))
            if voltage is None:
                voltage = _as_float(lock.get("voltage"))
            if voltage is not None:
                normalized["vfp_lock_mv"] = voltage / 1000.0
                break

    return _normalize_status_lock_fields(normalized)


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
            name = re.split(r"(?i)\buuid\s*[:=]\s*gpu-[\w-]+", name, maxsplit=1)[
                0
            ].strip()
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
            watts = re.search(
                r"(\d+)W\s*min\s*/\s*(\d+)W\s*current\s*/\s*(\d+)W\s*max", line
            )
            if watts:
                parsed["power_limit_nvml_min_w"] = int(watts.group(1))
                parsed["power_limit_nvml_current_w"] = int(watts.group(2))
                parsed["power_limit_nvml_max_w"] = int(watts.group(3))
        elif line.startswith("Thermal Limit"):
            match = re.search(
                r"(\d+)\s*C\s*~\s*(\d+)\s*C\s*\((\d+)\s*C\s*default\)", line
            )
            if match:
                parsed["thermal_limit_min"] = int(match.group(1))
                parsed["thermal_limit_max"] = int(match.group(2))
                parsed["thermal_limit_default"] = int(match.group(3))
    return parsed


def parse_status_output(output: str) -> dict[str, Any]:
    parsed: dict[str, Any] = {}
    vfp_lock_line_seen = False
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
            # The text output sometimes only marks lock state on voltage lines.
            # Prefer explicit VFP lock lines when present.
            if not vfp_lock_line_seen:
                parsed["vfp_locked"] = "(locked)" in low
        elif "sensor" in low or "temp" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*(?:°?c|celsius)", low)
            if match:
                parsed["temperature_c"] = float(match.group(1))
        elif "power" in low:
            match = re.search(r"(\d+(?:\.\d+)?)\s*w\b", low)
            if match:
                parsed["power_w"] = float(match.group(1))
        elif "vfp lock" in low:
            vfp_lock_line_seen = True
            if "none" in low:
                parsed["vfp_locked"] = False
                continue
            parsed["vfp_locked"] = True
            lock_mv = re.search(r"(\d+(?:\.\d+)?)\s*mv", low)
            if lock_mv:
                parsed["vfp_lock_mv"] = float(lock_mv.group(1))
    return _normalize_status_lock_fields(parsed)


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
            match = re.search(
                r"([0-9]+(?:\.[0-9]+)?)\s*W\s*\(Min:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*-\s*Max:\s*([0-9]+(?:\.[0-9]+)?)\s*W",
                line,
            )
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


def vf_curve_points_to_series(
    points: list[dict[str, Any]],
) -> tuple[list[float], list[float], list[float]]:
    """Convert in-memory V/F points from micro-units to plot units."""
    voltages: list[float] = []
    frequencies: list[float] = []
    defaults: list[float] = []
    for point in points:
        voltage_uv = point.get("voltage_uv", 0)
        frequency_khz = point.get("frequency_khz", 0)
        default_khz = point.get("default_frequency_khz")
        voltages.append(float(voltage_uv) / 1000.0)
        frequencies.append(float(frequency_khz) / 1000.0)
        defaults.append(
            frequencies[-1] if default_khz is None else float(default_khz) / 1000.0
        )
    return voltages, frequencies, defaults


def load_vf_curve(path: str) -> tuple[list[float], list[float], list[float]]:
    csv_path = Path(path)
    if not csv_path.is_file():
        return [], [], []

    voltages: list[float] = []
    freqs: list[float] = []
    defaults: list[float] = []
    for raw in csv_path.read_text(encoding="utf-8-sig").splitlines():
        row = [piece.strip() for piece in raw.split(",")]
        if (
            not row
            or row[0].startswith("#")
            or row[0].lower() in {"voltage", "voltage_uv"}
        ):
            continue
        if len(row) < 2:
            continue
        try:
            voltages.append(float(row[0]) / 1000.0)
            freqs.append(float(row[1]) / 1000.0)
            defaults.append(
                float(row[3]) / 1000.0 if len(row) > 3 else float(row[1]) / 1000.0
            )
        except ValueError:
            continue

    return voltages, freqs, defaults


def write_vf_curve_points(path: str, points: list[dict[str, Any]]) -> None:
    rows = ["voltage,frequency,delta,default_frequency"]
    for point in points:
        rows.append(
            "{},{},{},{}".format(
                int(point.get("voltage_uv", 0)),
                int(point.get("frequency_khz", 0)),
                int(point.get("delta_khz", 0)),
                int(point.get("default_frequency_khz", 0)),
            )
        )
    Path(path).write_text("\n".join(rows) + "\n", encoding="utf-8")


def load_vf_curve_deltas(
    path: str, current_points: list[dict[str, Any]]
) -> list[tuple[int, int]]:
    csv_path = Path(path)
    if not csv_path.is_file():
        raise FileNotFoundError(path)

    indices_by_voltage: dict[int, int] = {}
    for point in current_points:
        if "voltage_uv" not in point or "index" not in point:
            continue
        try:
            indices_by_voltage[int(point["voltage_uv"])] = int(point["index"])
        except (TypeError, ValueError):
            continue

    deltas: list[tuple[int, int]] = []
    for raw in csv_path.read_text(encoding="utf-8-sig").splitlines():
        row = [piece.strip() for piece in raw.split(",")]
        if (
            not row
            or row[0].startswith("#")
            or row[0].lower() in {"voltage", "voltage_uv"}
        ):
            continue
        if len(row) < 3:
            continue
        try:
            voltage = int(row[0])
            delta = int(row[2])
        except ValueError:
            continue
        if voltage in indices_by_voltage:
            deltas.append((indices_by_voltage[voltage], delta))
    return deltas


def find_curve_point_for_voltage(
    voltages: list[float],
    freqs: list[float],
    voltage_mv: float | None,
) -> tuple[float, float] | None:
    if voltage_mv is None or not voltages or len(voltages) != len(freqs):
        return None

    target_voltage = float(voltage_mv)
    nearest_index = min(
        range(len(voltages)), key=lambda idx: abs(voltages[idx] - target_voltage)
    )
    return voltages[nearest_index], freqs[nearest_index]


def compute_vf_plot_bounds(
    voltages: list[float],
    freqs: list[float],
    defaults: list[float],
    *,
    live_point: tuple[float, float] | None = None,
    lock_point: tuple[float, float] | None = None,
    working_point: tuple[float, float] | None = None,
) -> tuple[tuple[float, float], tuple[float, float]] | None:
    if not voltages or not freqs or not defaults:
        return None

    x_values = list(voltages)
    y_values = [*freqs, *defaults]
    for point in (live_point, lock_point, working_point):
        if point is None:
            continue
        x_values.append(point[0])
        y_values.append(point[1])

    x_min: float = float(min(x_values))
    x_max: float = float(max(x_values))
    y_min: float = float(min(y_values))
    if y_min > 0.0:
        y_min = 0.0
    y_max: float = float(max(y_values))

    x_padding: float = float(max(1.0, (x_max - x_min) * 0.03) if x_max > x_min else 1.0)
    y_padding: float = float(max(1.0, (y_max - y_min) * 0.05) if y_max > y_min else 1.0)

    return cast(
        tuple[tuple[float, float], tuple[float, float]],
        (
            (x_min - x_padding, x_max + x_padding),
            (y_min, y_max + y_padding),
        ),
    )


# ── Multi-curve helpers (port of the GUI VF-curve logic, minus editing) ──

# Per-curve metadata: plot label, g(def) prior class for raw-converted
# translation, and the ClockDomain bit for the direct-read live crosshair.
# The third curve's attribution moved twice — HOST → SYS (voltage-lock)
# → MSD, pinned by the bit-5 offset A/B (+200 MHz into the ClkDomains
# bit-5 record shifted every point; the Host MEASURE channel stayed in
# its 825–1350 band) — see ClkVfSegment::domain_hint in nvapi-rs.
# NOTE the bits are DIFFERENT records: domain_bit below is the bit whose
# MEASURE_FREQ reads the live clock for the crosshair. For MSD that is
# bit 21 (the Msd channel — the domain this curve IS): the SYS channel
# (bit 2) co-moves in the same uncore band but reads a DIFFERENT value,
# so sampling bit 2 puts the green cross off the curve (live-seen: bit2
# 2095.8 vs bit21 2070.2 MHz at the same instant). The record whose
# offset WRITE shifts this curve is bit 5 (surfaced as MSD by the CLI's
# WRITE map).
CURVE_META: dict[str, dict[str, Any]] = {
    "gpc": {"label": "GPC", "class": "graphics", "domain_bit": 0},
    "xbar": {"label": "XBAR", "class": "fabric", "domain_bit": 1},
    "msd": {"label": "MSD", "class": "fabric", "domain_bit": 21},
    # Pascal-HBM compute cards (GP100/V100): bank 0's 2nd 80-pt curve is
    # the HBM MEM V/F curve (live A/B: the MEM domain offset hits it).
    # class "graphics" = the neutral g(def) prior (no HBM calibration yet).
    "mem": {"label": "MEM", "class": "graphics", "domain_bit": 4},
}


def curve_meta(curve_id: str) -> dict[str, Any]:
    """Meta lookup with a synthesized fallback for unknownN curves.

    50-series (GB10) packs a fourth vf_curve the ordinal hint table can't
    name (domain "unknown"); such curves get ids "unknown1", "unknown2", …
    (in segment order) and display like any other curve — but with no
    domain_bit, so no live crosshair / direct read.
    """
    meta = CURVE_META.get(curve_id)
    if meta is not None:
        return meta
    label = (
        "UNK" + curve_id[len("unknown") :]
        if curve_id.startswith("unknown")
        else curve_id.upper()
    )
    # class "graphics" = neutral g(def) prior; only used for raw conversions.
    return {"label": label, "class": "graphics", "domain_bit": None}


def public_vfp_unsupported(gpc_err: str | None) -> bool:
    """True when the open VFP interface explicitly rejected the query."""
    if not gpc_err:
        return False
    low = gpc_err.lower()
    return "not supported" in low or "no implementation" in low


# ClkDomains WRITE-record bit -> curve id. Deliberately DISTINCT from
# CURVE_META's domain_bit, which is the MEASURE bit: MSD measures on bit
# 21 but its offset WRITE record is bit 5; MEM measures on bit 4 but
# writes on bit 2. Conflating the two tables silently synthesizes from
# the wrong domain's offset.
WRITE_BIT_TO_CURVE: dict[int, str] = {0: "gpc", 1: "xbar", 5: "msd", 2: "mem"}


def normalize_domain_offsets(raw: Any) -> dict[str, dict[str, int]]:
    """pynvoc private-freq-domain info payload -> {curve_id: offsets}.

    The payload entries carry ``bit`` / ``value_modifiable`` /
    ``values_kHz`` — an 8-list whose [0] is the slot-0 kHz frequency
    offset and [1] the slot-1 µV voltage addend DESPITE the field name;
    that unit split is normalized here and nowhere else. Entries whose
    ``value_modifiable`` is False are dropped (their value fields are not
    driver data), as are bits outside WRITE_BIT_TO_CURVE (measure bits,
    unexposed domains).
    """
    if not isinstance(raw, dict) or not raw.get("entries"):
        return {}
    offsets: dict[str, dict[str, int]] = {}
    for entry in raw["entries"]:
        if not isinstance(entry, dict) or not entry.get("value_modifiable"):
            continue
        try:
            bit = int(entry["bit"])
        except (KeyError, TypeError, ValueError):
            continue
        curve_id = WRITE_BIT_TO_CURVE.get(bit)
        if curve_id is None:
            continue
        values = entry.get("values_kHz")
        if not isinstance(values, list) or len(values) < 2:
            continue
        try:
            offsets[curve_id] = {
                "slot0_khz": int(values[0] or 0),
                "slot1_uv": int(values[1] or 0),
            }
        except (TypeError, ValueError):
            continue
    return offsets


def synthesize_effective(
    curve: CurveData, offsets: dict[str, dict[str, int]]
) -> EffectiveCurve:
    """Frontier synthesis: the base curve shifted by its OWN offsets.

    F_eff(v) = F_base(v − slot1): the slot-1 µV addend shifts the voltage
    axis (the grid moves by +slot1 mV), the slot-0 kHz offset adds to the
    frequencies. Other domains' offsets deliberately do NOT enter — they
    are silent on this frontier (XBAR demanding 0.7 V under a 0.8 V GPC
    point means XBAR +50 mV changes nothing observable; only once it
    exceeds the GPC demand does it lift the shared rail's OPERATING
    point, which is a floor/unreachable-region question, not a frontier
    one). Not applicable when the offsets are zero or the base series is
    degenerate (Pascal all-zero voltage axis, GCOFF null-filled segment).
    """
    own = offsets.get(curve.curve_id)
    off_mv = (own["slot1_uv"] / 1000.0) if own else 0.0
    off_mhz = (own["slot0_khz"] / 1000.0) if own else 0.0
    eff = EffectiveCurve(curve.curve_id, offset_mv=off_mv, offset_mhz=off_mhz)
    if off_mv == 0.0 and off_mhz == 0.0:
        return eff
    if not any(v > 0 for v in curve.voltages):
        return eff  # Pascal-style all-zero voltage axis: nothing to shift
    if not curve.frequencies or all(f == 0 for f in curve.frequencies):
        return eff  # GCOFF / null-filled segment
    eff.voltages = [v + off_mv for v in curve.voltages]
    eff.freqs = [f + off_mhz for f in curve.frequencies]
    eff.applicable = True
    return eff


def build_vf_curves(
    gpc_points: list[dict[str, Any]] | None,
    gpc_err: str | None,
    clk_data: dict[str, Any] | None,
    domain_info: Any = None,
) -> dict[str, CurveData] | None:
    """Classify public + private V/F reads into per-domain curves.

    Port of the GUI ``_build_curves``: GPC's DEFAULT axis is authoritative
    from the private GPC segment whenever that segment carries a populated
    voltage axis; the open interface then only donates CURRENT frequencies
    (and only when its voltage grid still matches the default grid).
    XBAR/MSD come from the private ClockClient V/F-POINTS ``vf_curve``
    segments; unnamed domains (the 50-series fourth curve) display as
    unknownN; pstate_bins are skipped. Point-id ranges come straight from
    the segment structure — never hardcoded. Returns ``None`` when no
    curve can be built.

    Why private defaults: the public fill path returns empty entries
    whenever a positive gpc slot1 (µV V/F-curve voltage offset) is active,
    and under negative offsets the public voltage grid is shifted away
    from the default grid. The private STATUS gpc segment always reflects
    the default table (V100/538.78 verified; whether the public breakage
    is V100-, old-driver- or both-specific is open — hence runtime grid
    detection below, never a generation table). Pascal server cards carry
    an all-zero private voltage axis (freq-indexed records) and keep the
    public source via the populated-axis guard.
    """
    curves: dict[str, CurveData] = {}
    unknown_count = 0

    gpc_curve: CurveData | None = None
    if gpc_points:
        gpc_curve = CurveData("gpc")
        gpc_curve.source = "public"
        gpc_curve.voltages = [p["voltage_uv"] / 1000.0 for p in gpc_points]
        gpc_curve.frequencies = [p["frequency_khz"] / 1000.0 for p in gpc_points]
        gpc_curve.defaults = [
            (p.get("default_frequency_khz") or p["frequency_khz"]) / 1000.0
            for p in gpc_points
        ]
        gpc_curve.has_fixed = any(p.get("point_type") == "fixed" for p in gpc_points)
        gpc_curve.seg_start = 0
        gpc_curve.seg_end = len(gpc_points) - 1 if gpc_points else 0
        # Public family present: traditional OC unless a point is Fixed.
        gpc_curve.write_mode = "private" if gpc_curve.has_fixed else "public"
    elif public_vfp_unsupported(gpc_err):
        # Open family rejected — the private GPC segment (if any) is the
        # only GPC source; located below from clk_data.
        pass

    private_gpc: CurveData | None = None
    if clk_data and clk_data.get("segments"):
        segs = clk_data["segments"]
        pts = clk_data.get("points", [])
        for seg in segs:
            if seg.get("kind") != "vf_curve":
                continue  # pstate_bins are not curves — never plotted
            hint = seg.get("domain", "unknown")
            bank = int(seg.get("bank", 0))
            s = int(seg.get("start_index", 0))
            e = int(seg.get("end_index", s))
            seg_pts = [
                p
                for p in pts
                if int(p.get("bank", 0)) == bank and s <= int(p.get("index", -1)) <= e
            ]
            if not seg_pts:
                continue
            if hint in CURVE_META:
                curve_id = hint
            else:
                # Unnamed domain (50-series fourth curve): display as
                # unknownN — and NEVER as the private-GPC fallback.
                unknown_count += 1
                curve_id = f"unknown{unknown_count}"
            cd = CurveData(curve_id)
            cd.source = "private"
            cd.bank = bank
            cd.seg_start = s
            cd.seg_end = e
            cd.voltages = [p["voltage_uV"] / 1000.0 for p in seg_pts]
            cd.frequencies = [p["freq_current_mhz"] for p in seg_pts]
            cd.defaults = [p["freq_default_mhz"] for p in seg_pts]
            cd.write_mode = "private"
            if cd.curve_id == "gpc":
                private_gpc = cd
            else:
                curves[cd.curve_id] = cd

    # Resolve GPC source. The private GPC segment is the DEFAULT-axis
    # authority whenever its voltage axis is populated (Pascal server
    # cards: all-zero axis → keep the public source). The public read
    # donates CURRENT frequencies only when its grid still matches the
    # default grid — under an active slot1 shift it is either shifted
    # (negative offset) or empty (positive offset) and must be ignored.
    private_gpc_usable = private_gpc is not None and any(
        v > 0 for v in private_gpc.voltages
    )
    if private_gpc_usable:
        cd = private_gpc
        # Populated-fraction gate: a HEALTHY public read populates the whole
        # frequency column; the broken positive-slot1 read zeroes every data
        # word but lets the #0 sentinel survive (live V100: #0 450 mV/0.4
        # MHz + 127 zero rows) — so `any(freq>0)` cannot tell them apart.
        nonzero_freq = (
            sum(1 for p in gpc_points if p.get("frequency_khz", 0) > 0)
            if gpc_points
            else 0
        )
        if (
            gpc_points
            and len(gpc_points) == len(cd.voltages)
            and nonzero_freq * 2 >= len(gpc_points)
            and all(
                abs(p["voltage_uv"] / 1000.0 - v) <= 0.01
                for p, v in zip(gpc_points, cd.voltages)
            )
        ):
            # Unshifted public grid: adopt its live CURRENT frequencies
            # (public deltas / OC state); defaults stay private.
            cd.frequencies = [p["frequency_khz"] / 1000.0 for p in gpc_points]
            cd.has_fixed = any(p.get("point_type") == "fixed" for p in gpc_points)
            cd.source = "hybrid"
        else:
            # Shifted or broken public read: private currents are the
            # honest view (== defaults on legacy, live state elsewhere).
            cd.has_fixed = True
        curves["gpc"] = cd
    elif gpc_curve is not None:
        curves["gpc"] = gpc_curve
    elif private_gpc is not None:
        # Public absent AND private voltage axis empty — last resort.
        curves["gpc"] = private_gpc

    if not curves:
        return None
    # Effective-series synthesis is the FALLBACK for the broken-positive-
    # slot1 state and nothing else: it triggers only when the public read
    # came back present-but-corrupt (fill path bails → the frequency column
    # zeroes out EXCEPT the #0 sentinel, live V100: #0 survives with
    # 450 mV/0.4 MHz) AND the private default axis is authoritative. A
    # healthy public read (hybrid) already carries the live currents; a
    # shifted grid (negative slot1) displays fine through the private axis;
    # an absent public family is "not supported", not breakage — none of
    # those synthesize.
    public_broken = (
        bool(gpc_points)
        and len(gpc_points) > 1
        and (sum(1 for p in gpc_points if p.get("frequency_khz", 0) > 0) <= 1)
    )
    offsets = normalize_domain_offsets(domain_info)
    if offsets and public_broken:
        gpc = curves.get("gpc")
        if gpc is not None and gpc.source != "public":
            gpc.effective = synthesize_effective(gpc, offsets)
    # Canonical display order GPC → XBAR → MSD → others (unknownN keep
    # discovery order; stable sort). Consumers (selector, plot draws)
    # iterate this dict — without this, a public-source GPC (inserted
    # last above) would sort after the private segments.
    order = {"gpc": 0, "xbar": 1, "msd": 2, "mem": 3}
    return dict(sorted(curves.items(), key=lambda kv: order.get(kv[0], 4)))


def reverse_lookup_voltage(
    volts: list[float], freqs: list[float], target_freq: float
) -> float | None:
    """Voltage on the curve closest to ``target_freq`` (linear interpolation).

    xbar/msd curves are monotonic in frequency vs voltage (no pstate
    off-curve excursion), so the reverse lookup is single-valued; targets
    outside the range clamp to the nearer end. Port of the GUI helper.
    """
    n = len(freqs)
    if n == 0:
        return None
    if n == 1:
        return volts[0]
    ascending = freqs[-1] >= freqs[0]
    sfreqs = freqs
    svolts = volts
    if not ascending:
        sfreqs = list(reversed(freqs))
        svolts = list(reversed(volts))
    if target_freq <= sfreqs[0]:
        return svolts[0]
    if target_freq >= sfreqs[-1]:
        return svolts[-1]
    for i in range(n - 1):
        f0, f1 = sfreqs[i], sfreqs[i + 1]
        if f0 <= target_freq <= f1:
            v0, v1 = svolts[i], svolts[i + 1]
            if f1 == f0:
                return v0
            t = (target_freq - f0) / (f1 - f0)
            return v0 + (v1 - v0) * t
    return svolts[-1]


def compute_vf_plot_bounds_multi(
    curves: list[CurveData],
    *,
    live_point: tuple[float, float] | None = None,
    lock_point: tuple[float, float] | None = None,
    working_point: tuple[float, float] | None = None,
) -> tuple[tuple[float, float], tuple[float, float]] | None:
    """Union bounds across every visible curve (same padding as single)."""
    voltages: list[float] = []
    freqs: list[float] = []
    defaults: list[float] = []
    for curve in curves:
        voltages.extend(curve.voltages)
        freqs.extend(curve.frequencies)
        defaults.extend(curve.defaults)
        eff = curve.effective
        if eff is not None and eff.applicable:
            voltages.extend(eff.voltages)
            freqs.extend(eff.freqs)
    if not voltages or not freqs or not defaults:
        return None
    return compute_vf_plot_bounds(
        voltages,
        freqs,
        defaults,
        live_point=live_point,
        lock_point=lock_point,
        working_point=working_point,
    )
