"""Optional memory diagnostics for long-running NVOC-GUI sessions."""

from __future__ import annotations

import ctypes
import gc
import os
import sys
import tracemalloc
from dataclasses import dataclass
from typing import Callable, Optional


_DEFAULT_TOP_STATS = 5
_MIN_INTERVAL_SECONDS = 10.0


@dataclass(frozen=True)
class MemoryDebugConfig:
    """Runtime settings for GUI memory diagnostics."""

    interval_seconds: float
    top_stats: int = _DEFAULT_TOP_STATS


def configure_memory_debug_from_env() -> Optional[MemoryDebugConfig]:
    """Enable tracemalloc when requested through environment variables.

    Set ``NVOC_GUI_MEMORY_LOG_INTERVAL_SEC`` to a positive interval to emit
    periodic memory samples to the GUI console. ``NVOC_GUI_MEMORY_TOP`` controls
    how many allocation sites are included in each sample.
    """

    raw_interval = os.environ.get("NVOC_GUI_MEMORY_LOG_INTERVAL_SEC", "").strip()
    if not raw_interval:
        return None
    try:
        interval_seconds = float(raw_interval)
    except ValueError:
        return None
    if interval_seconds <= 0:
        return None

    interval_seconds = max(_MIN_INTERVAL_SECONDS, interval_seconds)
    top_stats = _DEFAULT_TOP_STATS
    raw_top = os.environ.get("NVOC_GUI_MEMORY_TOP", "").strip()
    if raw_top:
        try:
            top_stats = max(1, min(20, int(raw_top)))
        except ValueError:
            top_stats = _DEFAULT_TOP_STATS

    if not tracemalloc.is_tracing():
        tracemalloc.start(25)
    return MemoryDebugConfig(interval_seconds=interval_seconds, top_stats=top_stats)


def current_rss_bytes() -> Optional[int]:
    """Return current resident set size in bytes when the platform exposes it."""

    if sys.platform.startswith("linux"):
        try:
            with open("/proc/self/status", encoding="utf-8") as status_file:
                for line in status_file:
                    if line.startswith("VmRSS:"):
                        parts = line.split()
                        return int(parts[1]) * 1024
        except (OSError, ValueError, IndexError):
            return None
    if sys.platform == "win32":
        return _windows_current_rss_bytes()
    return None


def _windows_current_rss_bytes() -> Optional[int]:
    # ``ctypes.wintypes`` only exists on Windows; import it lazily here so the
    # module remains importable on Linux/macOS where this helper is never called.
    import ctypes.wintypes as wintypes

    class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    counters = PROCESS_MEMORY_COUNTERS()
    counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
    try:
        handle = ctypes.windll.kernel32.GetCurrentProcess()
        ok = ctypes.windll.psapi.GetProcessMemoryInfo(
            handle, ctypes.byref(counters), counters.cb
        )
    except (AttributeError, OSError):
        return None
    if not ok:
        return None
    return int(counters.WorkingSetSize)


def format_memory_report(top_stats: int = _DEFAULT_TOP_STATS) -> str:
    """Build a compact tracemalloc/RSS report for console output."""

    gc.collect()
    rss = current_rss_bytes()
    current, peak = (
        tracemalloc.get_traced_memory() if tracemalloc.is_tracing() else (0, 0)
    )
    lines = [
        "[GUI][mem] RSS={} Python={} peak={} objects={}".format(
            _format_bytes(rss),
            _format_bytes(current),
            _format_bytes(peak),
            len(gc.get_objects()),
        )
    ]
    if tracemalloc.is_tracing():
        snapshot = tracemalloc.take_snapshot()
        stats = snapshot.statistics("lineno")[:top_stats]
        for index, stat in enumerate(stats, start=1):
            frame = stat.traceback[0]
            lines.append(
                f"[GUI][mem] #{index} {stat.size / 1024:.1f} KiB in "
                f"{stat.count} blocks at {frame.filename}:{frame.lineno}"
            )
    return "\n".join(lines) + "\n"


def schedule_memory_reports(
    after: Callable[[int, Callable[[], None]], object],
    append: Callable[[str], None],
    config: MemoryDebugConfig,
) -> None:
    """Schedule periodic memory reports on a Tk-compatible event loop."""

    interval_ms = int(config.interval_seconds * 1000)

    def emit() -> None:
        append(format_memory_report(config.top_stats))
        after(interval_ms, emit)

    after(interval_ms, emit)


def _format_bytes(value: Optional[int]) -> str:
    if value is None:
        return "n/a"
    units = ["B", "KiB", "MiB", "GiB"]
    amount = float(value)
    for unit in units:
        if amount < 1024 or unit == units[-1]:
            return f"{amount:.1f} {unit}"
        amount /= 1024
    return f"{amount:.1f} GiB"
