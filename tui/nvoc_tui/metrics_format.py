from __future__ import annotations

# Pure formatting helpers for the dashboard metrics panel. Kept separate from
# the Textual controller so they can be unit-tested without a widget app.


def _temp_c_str(value) -> str:
    """Format a temperature for display as a rounded integer (°C)."""
    if value is None:
        return "---"
    try:
        return f"{round(float(value))}"
    except (TypeError, ValueError):
        return "---"


def _effective_clocks_text(status: dict) -> str:
    """Format the effective (actually-running) clocks line.

    Falls back to ``---`` when the GPU/driver doesn't expose the GetAllClocks
    V2 layout.
    """
    gpu = status.get("eff_gpu_clock_mhz")
    mem = status.get("eff_mem_clock_mhz")
    if not isinstance(gpu, (int, float)) and not isinstance(mem, (int, float)):
        return "---"
    parts = []
    if isinstance(gpu, (int, float)):
        parts.append(f"GPU {round(float(gpu))}")
    if isinstance(mem, (int, float)):
        parts.append(f"MEM {round(float(mem))}")
    return " | ".join(parts) + " MHz"


def _format_mibps(mibps: float) -> str:
    """Format a MiB/s value with a sensible unit (MiB/s or GiB/s)."""
    if mibps >= 1024.0:
        return f"{mibps / 1024.0:.1f} GiB/s"
    if mibps >= 10.0:
        return f"{mibps:.0f} MiB/s"
    return f"{mibps:.1f} MiB/s"


# Internal fabric clock domains (from GetAllClocks V2 `all_clocks_mhz`) to surface
# on a dedicated FCLK line — the "crossbar clock" GPU-Z shows plus the other
# structurally-interesting fabric clocks. Ordered for stable display.
_FABRIC_CLOCK_DOMAINS = ["Xbar", "Sys", "Hub", "Host", "Gpc", "Disp", "Hotclk"]


def _fabric_clocks_text(status: dict) -> str:
    """Format the internal-fabric clocks line (Xbar/crossbar, Sys, Hub, ...).

    Reads the per-domain MHz from ``all_clocks_mhz`` (GetAllClocks V2's full
    32-domain breakdown). Returns ``""`` when no fabric domains are present so
    the line is omitted entirely.
    """
    all_clocks = status.get("all_clocks_mhz")
    if not isinstance(all_clocks, dict):
        return ""
    parts = []
    for domain in _FABRIC_CLOCK_DOMAINS:
        mhz = all_clocks.get(domain)
        if isinstance(mhz, (int, float)) and float(mhz) > 0:
            parts.append(f"{domain.upper()} {round(float(mhz))}")
    return " | ".join(parts) + " MHz" if parts else ""


def _power_rails_text(status: dict) -> str:
    """Per-rail power breakdown (NVAPI PowerMonitor, descriptor-driven).

    Reads ``power_rails_w`` — a { "<RailName>": <watts> } map keyed by the
    descriptor's rail identity (correct on every GPU: a laptop shows
    InputTotalBoard/InputNvvdd/..., a desktop shows InputPex12v1/PCIe slot/
    InputExt12v8pin*/InputTotalBoard). Renders each rail as ``<short> <W>``;
    board total is omitted (it's on the PWR line via NVML). Returns ``""`` when
    no rails are present so the line is omitted.
    """
    rails = status.get("power_rails_w")
    if not isinstance(rails, dict):
        return ""
    # Short labels for common rails; others use the full rail name. Map keys may
    # carry a confidence suffix (`~` Inferred, `?` Ambiguous) — strip it before
    # the lookup and re-append so the marker survives in the rendered label.
    short = {
        "InputTotalBoard": "BOARD",
        "InputNvvdd": "CHIP",
        "InputFbvdd": "MEM",
        "InputPwrSrcPp": "SRC",
        "InputPex12v1": "PCIE",
        "InputPex12v": "PCIE12V",
        "InputPex3v3": "PEX3V3",
    }
    parts = []
    for name, val in rails.items():
        if not isinstance(val, (int, float)):
            continue
        marker = ""
        base = name
        # Strip a trailing confidence marker (~ inferred, ? ambiguous).
        if base and base[-1] in "~?":
            marker = base[-1]
            base = base[:-1]
        label = short.get(base, base)
        # Skip the board-total duplicate (already on the PWR line).
        if label == "BOARD" and not marker:
            continue
        parts.append(f"{label}{marker} {val:.1f}")
    return " | ".join(parts) + " W" if parts else ""


def _pcie_bandwidth_text(status: dict) -> str:
    """Bidirectional real-time PCIe bandwidth, nvitop-style (``↑Tx ↓Rx``).

    Reads ``pcie_tx_mibps`` / ``pcie_rx_mibps`` (NVML
    ``nvmlDeviceGetPcieThroughput``, KB/s over a ~20ms interval → MiB/s). Empty
    string when the GPU/driver doesn't expose it.
    """
    tx = status.get("pcie_tx_mibps")
    rx = status.get("pcie_rx_mibps")
    has_tx = isinstance(tx, (int, float))
    has_rx = isinstance(rx, (int, float))
    if not has_tx and not has_rx:
        return ""
    parts = []
    if has_tx:
        parts.append(f"↑{_format_mibps(float(tx))}")
    if has_rx:
        parts.append(f"↓{_format_mibps(float(rx))}")
    return " ".join(parts)


# NVAPI PerfFlags bit -> reason name. Bit semantics mirror nvapi-rs
# (`sys/src/gpu/power.rs`, NV_GPU_PERF_FLAGS + its display table). Ascending
# bit order so the decoded list reads consistently regardless of active set.
_PERF_LIMIT_BITS = [
    (1, "Power"),
    (2, "Temperature"),
    (4, "Reliability Voltage"),
    (8, "Operating Voltage"),
    (16, "No Load"),
    (32, "Unknown32"),
]


def _perf_limits_text(perf) -> str:
    """Decode the NVAPI perf-policy limit bitmask (``perf.limits``) to a
    comma-separated reason list. ``0`` -> ``none``; missing/non-numeric -> ``---``.
    """
    if not isinstance(perf, dict):
        return "---"
    limits = perf.get("limits")
    if not isinstance(limits, (int, float)):
        return "---"
    bits = int(limits)
    reasons = [name for bit, name in _PERF_LIMIT_BITS if bits & bit]
    return ", ".join(reasons) if reasons else "none"


def _format_metric_lines(status: dict, architecture: str) -> list[str]:
    """Build the dashboard metric lines from a normalized status dict.

    `status` is the pynvoc `query_status` output (see ``normalize_status`` in
    ``nvoc-python/src/lib.rs``). Missing fields render as ``---``.
    """
    if status.get("vfp_locked"):
        lock_mv = status.get("vfp_lock_mv")
        if isinstance(lock_mv, (int, float)):
            vfp_lock_text = f"ON ({lock_mv} mV)"
        else:
            vfp_lock_text = "ON"
    else:
        vfp_lock_text = "OFF"

    util = status.get("utilization") or {}

    def _pct(key: str) -> str:
        value = util.get(key)
        return f"{round(float(value))}%" if isinstance(value, (int, float)) else "---"

    load_text = " | ".join(
        [
            f"GPU {_pct('Graphics')}",
            # FrameBuffer is NVAPI's name for the memory-controller utilization domain.
            f"MC {_pct('FrameBuffer')}",
            f"VEN {_pct('VideoEngine')}",
            f"BUS {_pct('BusInterface')}",
        ]
    )

    vram = status.get("vram") or {}

    def _vram_gb(key: str) -> float | None:
        value = vram.get(key)
        if not isinstance(value, (int, float)):
            return None
        return float(value) / (1024.0 * 1024.0)

    used_gb = _vram_gb("used_kib")
    total_gb = _vram_gb("total_kib")
    vram_text = (
        f"{used_gb:.1f} / {total_gb:.1f} GB"
        if used_gb is not None and total_gb is not None
        else "---"
    )

    coolers = status.get("coolers") or {}
    valid_coolers = [
        (cid, c) for cid, c in sorted(coolers.items()) if isinstance(c, dict)
    ]
    fan_parts: list[str] = []
    for idx, (_, cooler) in enumerate(valid_coolers, start=1):
        rpm = cooler.get("current_tach")
        level = cooler.get("current_level")
        rpm_s = f"{round(float(rpm))}" if isinstance(rpm, (int, float)) else "---"
        level_s = (
            f"{round(float(level))}%" if isinstance(level, (int, float)) else "---"
        )
        label = "FAN" if len(valid_coolers) == 1 else f"FAN{idx}"
        fan_parts.append(f"{label}: {rpm_s} RPM @ {level_s}")
    fan_text = " | ".join(fan_parts) if fan_parts else "---"

    lanes = status.get("pcie_lanes")
    pcie_text = f"x{int(lanes)}" if isinstance(lanes, (int, float)) else "---"
    # Prepend the PCIe link generation as "Gen<cur>/<max>" (NVML
    # nvmlDeviceGetCurr/MaxPcieLinkGeneration) when exposed, e.g. "Gen4/4".
    cur_gen = status.get("pcie_link_gen")
    max_gen = status.get("pcie_max_link_gen")
    gen_text = ""
    if isinstance(cur_gen, (int, float)) and isinstance(max_gen, (int, float)):
        gen_text = f"Gen{int(float(cur_gen))}/{int(float(max_gen))} "
    elif isinstance(max_gen, (int, float)):
        gen_text = f"Gen?/{int(float(max_gen))} "
    # Append bidirectional real-time PCIe bandwidth (nvitop-style) when the GPU
    # exposes it via NVML nvmlDeviceGetPcieThroughput. ↑ = TX (GPU->host),
    # ↓ = RX (host->GPU). Omitted entirely on unsupported GPUs.
    bw = _pcie_bandwidth_text(status)
    if bw:
        pcie_text = f"{pcie_text} {bw}" if pcie_text != "---" else bw
    # Append the PCIe replay counter (NVML nvmlDeviceGetPcieReplayCounter) when
    # it is non-zero — a rising count indicates link-quality problems. Zero is
    # the normal steady state, so it is omitted to keep the line quiet.
    replay = status.get("pcie_replay_counter")
    if isinstance(replay, (int, float)) and float(replay) > 0:
        pcie_text = f"{pcie_text} ⚠replay {int(float(replay))}".strip()
    # Prepend the PCIe generation once everything else is assembled.
    if gen_text and pcie_text != "---":
        pcie_text = f"{gen_text}{pcie_text}"
    elif gen_text:
        pcie_text = gen_text.strip()

    perf = status.get("perf") or {}
    perf_text = _perf_limits_text(perf)

    # Thermal sensors: core (GPU_AVG) is always shown; hot spot (GPU_MAX) and
    # memory/VRAM are appended only when the GPU exposes them, so the line
    # adapts to whatever channels are available.
    temp_parts = [f"CORE {_temp_c_str(status.get('temperature_c'))} C"]
    if isinstance(status.get("temp_hotspot"), (int, float)):
        temp_parts.append(f"HOTSPOT {_temp_c_str(status['temp_hotspot'])} C")
    if isinstance(status.get("temp_memory"), (int, float)):
        temp_parts.append(f"MEM {_temp_c_str(status['temp_memory'])} C")
    temp_text = " | ".join(temp_parts)

    return [
        f"GPU: {status.get('gpu_clock_mhz', '---')} MHz",
        f"MEM: {status.get('mem_clock_mhz', '---')} MHz",
        f"ECLK: {_effective_clocks_text(status)}",
        f"FCLK: {_fabric_clocks_text(status) or '---'}",
        f"VOLT: {status.get('voltage_mv', '---')} mV",
        f"VFP LOCK: {vfp_lock_text}",
        f"TEMP: {temp_text}",
        f"PWR: {status.get('power_w', '---')} W",
        f"RAILS: {_power_rails_text(status) or '---'}",
        f"LOAD: {load_text}",
        f"VRAM: {vram_text}",
        f"FAN: {fan_text}",
        f"PCIE: {pcie_text}",
        f"PSTATE: {status.get('pstate', '---')}",
        f"PERF LIMIT: {perf_text}",
        f"ARCH: {architecture}",
    ]
