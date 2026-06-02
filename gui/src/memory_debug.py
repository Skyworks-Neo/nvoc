"""Optional memory diagnostics for long-running GUI sessions."""

from __future__ import annotations

import gc
import os
import sys
import time
import tracemalloc
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.app import App


def _current_rss_kib() -> int | None:
    """Return current process RSS in KiB when supported by the platform."""
    if sys.platform.startswith("linux"):
        try:
            with open("/proc/self/status", encoding="utf-8") as status_file:
                for line in status_file:
                    if line.startswith("VmRSS:"):
                        return int(line.split()[1])
        except OSError:
            return None

    if sys.platform == "win32":
        try:
            import ctypes
            from ctypes import wintypes

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
            counters.cb = ctypes.sizeof(counters)
            handle = ctypes.windll.kernel32.GetCurrentProcess()
            ok = ctypes.windll.psapi.GetProcessMemoryInfo(
                handle, ctypes.byref(counters), counters.cb
            )
            if ok:
                return int(counters.WorkingSetSize // 1024)
        except Exception:
            return None

    return None


class MemoryDebugSampler:
    """Periodically log RSS, tracemalloc totals, GC objects, and Tk image count."""

    def __init__(self, app: "App", interval_ms: int = 300_000) -> None:
        self._app = app
        self._interval_ms = interval_ms
        self._after_id: str | None = None
        app_dir = Path(__file__).resolve().parents[1]
        self._path = Path(os.environ.get("NVOC_GUI_MEMORY_LOG", app_dir / "memory.log"))
        self._started_at = time.monotonic()

    def start(self) -> None:
        if not tracemalloc.is_tracing():
            tracemalloc.start(25)
        self._sample()

    def stop(self) -> None:
        if self._after_id is None:
            return
        try:
            self._app.after_cancel(self._after_id)
        except Exception:
            pass
        self._after_id = None

    def _sample(self) -> None:
        gc.collect()
        rss_kib = _current_rss_kib()
        current, peak = tracemalloc.get_traced_memory()
        elapsed_s = int(time.monotonic() - self._started_at)
        timestamp = datetime.now().astimezone().isoformat(timespec="seconds")
        tk_images = self._tk_image_count()
        gc_objects_list = gc.get_objects()
        gc_objects = len(gc_objects_list)
        matplotlib_stats = self._matplotlib_stats(gc_objects_list)

        lines = [
            (
                f"timestamp={timestamp} elapsed={elapsed_s}s rss_mib={_to_mib(rss_kib)} "
                f"py_current_mib={current / 1024 / 1024:.1f} "
                f"py_peak_mib={peak / 1024 / 1024:.1f} "
                f"gc_objects={gc_objects} tk_images={tk_images} "
                f"{matplotlib_stats}"
            )
        ]
        snapshot = tracemalloc.take_snapshot()
        for stat in snapshot.statistics("filename")[:12]:
            lines.append(f"  {stat}")
        lines.append("")

        try:
            self._path.parent.mkdir(parents=True, exist_ok=True)
            with self._path.open("a", encoding="utf-8") as log_file:
                log_file.write("\n".join(lines))
        except OSError:
            pass

        self._after_id = self._app.after(self._interval_ms, self._sample)

    def _tk_image_count(self) -> int | str:
        try:
            return len(self._app.tk.call("image", "names"))
        except Exception as exc:
            return f"unavailable:{exc}"

    def _matplotlib_stats(self, objects: list[object]) -> str:
        if "matplotlib" not in sys.modules:
            return "mpl_figures=0 mpl_axes=0 mpl_artists=0 vf_axes_children=0"

        figures = 0
        axes = 0
        artists = 0
        try:
            from matplotlib.artist import Artist
        except Exception:
            Artist = None

        for obj in objects:
            cls = type(obj)
            module = getattr(cls, "__module__", "")
            name = getattr(cls, "__name__", "")
            if module == "matplotlib.figure" and name == "Figure":
                figures += 1
            elif module == "matplotlib.axes._axes" and name == "Axes":
                axes += 1
            if Artist is not None and isinstance(obj, Artist):
                artists += 1

        vf_axes_children = 0
        vf_tab = getattr(self._app, "tab_vfcurve", None)
        vf_ax = getattr(vf_tab, "ax", None)
        if vf_ax is not None:
            try:
                vf_axes_children = len(vf_ax.get_children())
            except Exception:
                vf_axes_children = -1
        return (
            f"mpl_figures={figures} mpl_axes={axes} "
            f"mpl_artists={artists} vf_axes_children={vf_axes_children}"
        )


def _to_mib(kib: int | None) -> str:
    if kib is None:
        return "unknown"
    return f"{kib / 1024:.1f}"
