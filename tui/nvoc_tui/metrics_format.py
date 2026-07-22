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

    perf = status.get("perf") or {}
    perf_limits = perf.get("limits") if isinstance(perf, dict) else None
    perf_text = (
        f"0x{int(perf_limits):x}" if isinstance(perf_limits, (int, float)) else "---"
    )

    return [
        f"GPU: {status.get('gpu_clock_mhz', '---')} MHz",
        f"MEM: {status.get('mem_clock_mhz', '---')} MHz",
        f"VOLT: {status.get('voltage_mv', '---')} mV",
        f"VFP LOCK: {vfp_lock_text}",
        f"TEMP: {_temp_c_str(status.get('temperature_c'))} C",
        f"PWR: {status.get('power_w', '---')} W",
        f"LOAD: {load_text}",
        f"VRAM: {vram_text}",
        f"FAN: {fan_text}",
        f"PCIE: {pcie_text}",
        f"PSTATE: {status.get('pstate', '---')}",
        f"PERF: {perf_text}",
        f"ARCH: {architecture}",
    ]
