"""
Dashboard Tab - Real-time GPU monitoring display.
Shows: Core Freq, Mem Freq, Core Voltage (lock indicator), Temperature, Power.
Auto-refreshes at a configurable interval via CLI 'status' command.
"""

import tkinter as tk
import customtkinter as ctk
from typing import TYPE_CHECKING, Optional, Dict, Tuple

from src.parsing import as_float, parse_dashboard_status
from src.widgets.lightweight_controls import LiteButton, LiteEntry, ct_button_font

if TYPE_CHECKING:
    from src.app import App

# ── Colour palette ──────────────────────────────────────────────────────────
_BG_CARD = "#1a1a2e"
_BG_ROW = "#16213e"
_BG_BAR = "#0f3460"
_FG_LABEL = "#8899bb"
_FG_VALUE = "#e8e8f0"
_FG_UNIT = "#6688aa"
_FG_LOCK = "#ff4444"
_FG_UNLOCK = "#44cc88"

# Bar gradient colour stops per metric (2 or 3 hex strings)
_BAR_COLORS: Dict[str, tuple] = {
    "GPU": ("#1e88e5", "#00e5ff", "#00bfa5"),
    "MEM": ("#7b1fa2", "#e040fb", "#ff4081"),
    "VOLT": ("#1565c0", "#42a5f5"),
    "TEMP": ("#1b5e20", "#f9a825", "#c62828"),
    "PWR": ("#004d40", "#00bcd4", "#ff6f00"),
}


# ── Helper: colour blend ─────────────────────────────────────────────────────
def _blend_hex(c0: str, c1: str, t: float) -> str:
    def parse(c):
        c = c.lstrip("#")
        return int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16)

    r0, g0, b0 = parse(c0)
    r1, g1, b1 = parse(c1)
    return "#{:02x}{:02x}{:02x}".format(
        int(r0 + (r1 - r0) * t),
        int(g0 + (g1 - g0) * t),
        int(b0 + (b1 - b0) * t),
    )


def _gradient_color(stops: tuple, ratio: float) -> str:
    ratio = max(0.0, min(1.0, ratio))
    if len(stops) == 2:
        return _blend_hex(stops[0], stops[1], ratio)
    # 3-stop
    c0, c1, c2 = stops
    if ratio < 0.5:
        return _blend_hex(c0, c1, ratio * 2)
    return _blend_hex(c1, c2, (ratio - 0.5) * 2)


# ── Metric row widget ─────────────────────────────────────────────────────────
class _MetricRow:
    """One row: LABEL | segmented-gradient bar | VALUE unit [lock]"""

    _BAR_H = 18
    _SEG_W = 18
    _GAP_W = 4
    _RESIZE_DRAW_DELAY_MS = 45
    _WIDTH_DELTA_PX = 6

    def __init__(
        self,
        parent: ctk.CTkFrame,
        key: str,
        label: str,
        unit: str,
        val_min: float,
        val_max: float,
    ):
        self.key = key
        self.unit = unit
        self.val_min = val_min
        self.val_max = val_max
        self._stops = _BAR_COLORS.get(key, ("#1e88e5", "#00e5ff"))

        row = ctk.CTkFrame(parent, fg_color=_BG_ROW, corner_radius=8)
        row.pack(fill="x", padx=16, pady=3)

        # col 0: label
        ctk.CTkLabel(
            row,
            text=label,
            font=("Consolas", 13, "bold"),
            text_color=_FG_LABEL,
            width=56,
            anchor="w",
        ).grid(row=0, column=0, padx=(14, 6), pady=2)

        # col 1: canvas (bar) – expands
        self._canvas = tk.Canvas(
            row, height=self._BAR_H, bg=_BG_BAR, highlightthickness=0
        )
        self._canvas.grid(row=0, column=1, padx=4, pady=2, sticky="ew")
        row.columnconfigure(1, weight=1)
        self._canvas.bind("<Configure>", self._on_resize)
        self._ratio = 0.0
        self._last_draw_width: Optional[int] = None
        self._last_draw_ratio: Optional[float] = None
        self._resize_after_id: Optional[int] = None
        self._is_resize_active = False

        # col 2: numeric value
        self._val_lbl = ctk.CTkLabel(
            row,
            text="---",
            width=86,
            font=("Consolas", 18, "bold"),
            text_color=_FG_VALUE,
            anchor="e",
        )
        self._val_lbl.grid(row=0, column=2, padx=(4, 2), pady=2)

        # col 3: unit
        ctk.CTkLabel(
            row,
            text=unit,
            width=34,
            font=("Consolas", 11),
            text_color=_FG_UNIT,
            anchor="w",
        ).grid(row=0, column=3, padx=(0, 8), pady=2)

        # col 4: lock icon
        self._lock_lbl = ctk.CTkLabel(
            row, text="", width=22, font=("", 13), text_color=_FG_LOCK
        )
        self._lock_lbl.grid(row=0, column=4, padx=(0, 12), pady=2)

    def _on_resize(self, event):
        if self._is_resize_active:
            return
        width = max(1, int(event.width))
        if (
            self._last_draw_width is not None
            and abs(width - self._last_draw_width) < self._WIDTH_DELTA_PX
        ):
            return
        if self._resize_after_id:
            try:
                self._canvas.after_cancel(self._resize_after_id)
            except Exception:
                pass
        self._resize_after_id = self._canvas.after(
            self._RESIZE_DRAW_DELAY_MS,
            lambda r=self._ratio: self._draw_bar(r),
        )

    def set_resize_active(self, active: bool):
        self._is_resize_active = active
        if not active:
            self._draw_bar(self._ratio, force=True)

    def _draw_bar(self, ratio: float, force: bool = False):
        self._resize_after_id = None
        self._ratio = ratio
        c = self._canvas
        w = c.winfo_width()
        h = self._BAR_H
        if w < 2:
            return
        ratio_changed = (
            self._last_draw_ratio is None or abs(ratio - self._last_draw_ratio) >= 1e-3
        )
        if (
            not force
            and self._last_draw_width is not None
            and abs(w - self._last_draw_width) < self._WIDTH_DELTA_PX
            and not ratio_changed
        ):
            return
        self._last_draw_width = w
        self._last_draw_ratio = ratio
        c.delete("all")
        fill_w = max(0, min(w, int(w * ratio)))
        sw, gw = self._SEG_W, self._GAP_W

        # filled segments
        x = 0
        while x < fill_w:
            x1 = min(x + sw, fill_w)
            mid = (x + x1) / 2 / w
            color = _gradient_color(self._stops, mid)
            c.create_rectangle(x, 2, x1, h - 2, fill=color, outline="")
            x += sw + gw

        # dim unfilled segments
        # restart from aligned position
        n_filled = fill_w // (sw + gw)
        x_dim = n_filled * (sw + gw)
        if x_dim < fill_w:
            x_dim = fill_w
        while x_dim < w:
            x1 = min(x_dim + sw, w)
            c.create_rectangle(x_dim, 4, x1, h - 4, fill="#243040", outline="")
            x_dim += sw + gw

    def update(self, value: float, locked: Optional[bool] = None):
        span = self.val_max - self.val_min
        ratio = max(0.0, min(1.0, (value - self.val_min) / span)) if span else 0.0
        if self._is_resize_active:
            self._ratio = ratio
        else:
            self._draw_bar(ratio)

        # Format number
        if self.key == "VOLT":
            txt = f"{value:.1f}"
        elif self.key in ("GPU", "MEM", "TEMP"):
            txt = f"{value:.0f}"
        else:
            txt = f"{value:.1f}"
        self._val_lbl.configure(text=txt)

        if locked is True:
            self._lock_lbl.configure(text="🔒", text_color=_FG_LOCK)
        elif locked is False:
            self._lock_lbl.configure(text="🔓", text_color=_FG_UNLOCK)
        else:
            self._lock_lbl.configure(text="")

    def set_error(self):
        self._val_lbl.configure(text="---")
        self._draw_bar(0.0)
        self._lock_lbl.configure(text="")


# ── Main Tab ──────────────────────────────────────────────────────────────────
class DashboardTab:
    """Dashboard tab with real-time GPU metric bars."""

    _DEFAULT_INTERVAL_MS = 1000

    def __init__(self, parent: ctk.CTkFrame, app: "App") -> None:
        self.app = app
        self.frame = parent

        self._poll_job: Optional[str] = None
        self._polling = False
        self._fetching = False
        # The very first successful sample is mandatory (wake the GPU if
        # needed); afterwards polling never blocks GC6 sleep.
        self._first_success = False
        self._interval_ms = self._DEFAULT_INTERVAL_MS
        self._is_resize_active = False
        self._pending_done_payload: Optional[Tuple[int, str]] = None

        self._build_ui()
        self._restore_snapshot()
        self._sync_lock_state_from_cache()
        # Force one immediate sample so dashboard-only workflows update right away.
        self.app.after(120, self._fetch_once)
        # Start polling automatically after a short delay
        self.app.after(900, self._start_polling)

    # ── UI ────────────────────────────────────────────────────────────────────
    def _build_ui(self) -> None:
        # header
        header = ctk.CTkFrame(self.frame, fg_color="transparent")
        header.pack(fill="x", padx=10, pady=(0, 4))

        tk.Label(
            header,
            text="📊 GPU Live Monitor",
            font=("Segoe UI", 13, "bold"),
            bg="#2b2b2b",
            fg="#aaccff",
        ).pack(side="left", padx=8)

        # interval: Refresh: [entry] s — label styled to match the VF tab
        tk.Label(
            header,
            text="s",
            font=("Segoe UI", 11),
            bg="#2b2b2b",
            fg="#e5e5e5",
        ).pack(side="right", padx=(0, 6))
        self._interval_var = ctk.StringVar(value="1.0")
        ie = LiteEntry(
            header, textvariable=self._interval_var, width=5, justify="right"
        )
        ie.pack(side="right", padx=(0, 4))
        tk.Label(
            header,
            text="Refresh:",
            font=("Segoe UI", 11),
            bg="#2b2b2b",
            fg="#e5e5e5",
        ).pack(side="right", padx=(0, 4))
        ie.bind("<Return>", self._on_interval_changed)
        ie.bind("<FocusOut>", self._on_interval_changed)

        self._toggle_btn = ctk.CTkButton(
            header,
            text="⏸ Pause",
            width=90,
            command=self._toggle_polling,
            font=ct_button_font(header),
        )
        self._toggle_btn.pack(side="right", padx=6)

        ctk.CTkButton(
            header,
            text="🔄 Now",
            width=70,
            command=self._fetch_once,
            font=ct_button_font(header),
        ).pack(side="right", padx=4)

        # metric card
        card = ctk.CTkFrame(self.frame, fg_color=_BG_CARD, corner_radius=12)
        card.pack(fill="x", padx=10, pady=(4, 6))

        self._rows: Dict[str, _MetricRow] = {}
        for key, label, unit, vmin, vmax in [
            ("GPU", "GPU", "MHz", 100, 3500),
            ("MEM", "MEM", "MHz", 100, 12000),
            ("VOLT", "VOLT", "mV", 500, 1250),
            ("TEMP", "TEMP", "°C", 20, 100),
            ("PWR", "PWR", "W", 0, 400),
        ]:
            self._rows[key] = _MetricRow(card, key, label, unit, vmin, vmax)

        # Status line removed (user request); keep a no-op sink so the
        # fetch/resize paths can still call configure(text=...) unconditionally.
        class _NoopLabel:
            def configure(self, **_kw):
                pass

        self._status_lbl = _NoopLabel()

        # Host frame for the overclock tab's top panels (Clock Offsets +
        # Power & Thermal Limits) — the dashboard doubles as the control
        # center. Empty (zero height) until the overclock tab is built.
        self.oc_panels_host = tk.Frame(self.frame, bg="#2b2b2b")
        self.oc_panels_host.pack(fill="x", padx=10, pady=(4, 6))

        # Host frame for the fan-control section (below the two OC cards).
        self.fan_panels_host = tk.Frame(self.frame, bg="#2b2b2b")
        self.fan_panels_host.pack(fill="x", padx=10, pady=(0, 6))

        # Quick-access buttons — four equal slots, LiteButton styling to
        # match the section buttons above.
        controls = tk.Frame(self.frame, bg="#2b2b2b")
        controls.pack(fill="x", padx=10, pady=(4, 6))
        for col in range(4):
            controls.grid_columnconfigure(col, weight=1, uniform="dash_btns")
        LiteButton(
            controls, text="🔄 Refresh Info", width=10, command=self._refresh_info
        ).grid(row=0, column=0, sticky="ew", padx=4)
        LiteButton(
            controls, text="📊 Show Status", width=10, command=self._show_status
        ).grid(row=0, column=1, sticky="ew", padx=4)
        LiteButton(
            controls, text="📋 Show OC Settings", width=10, command=self._show_get
        ).grid(row=0, column=2, sticky="ew", padx=4)
        LiteButton(
            controls,
            text="🖥 Console",
            width=10,
            command=lambda: self.app.console.toggle(),
        ).grid(row=0, column=3, sticky="ew", padx=4)

    _SNAPSHOT_MAX_AGE_S = 24 * 3600

    def _snapshot_path(self):
        import os

        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
        d = os.path.join(base, "nvoc-gui")
        try:
            os.makedirs(d, exist_ok=True)
            return os.path.join(d, "dashboard_snapshot.json")
        except OSError:
            return None

    def _save_snapshot(self, fresh: dict) -> None:
        """Persist the last dashboard sample (background, best-effort)."""

        def _write():
            import json
            import time

            path = self._snapshot_path()
            if not path:
                return
            try:
                payload = {"ts": time.time(), "values": fresh}
                with open(path, "w", encoding="utf-8") as f:
                    json.dump(payload, f)
            except OSError:
                pass

        try:
            self.app.run_background("dash-snapshot", _write)
        except Exception:
            pass

    def _restore_snapshot(self) -> None:
        """Cold start: paint the last saved sample instantly (stale data,
        replaced by the first live poll)."""
        import json
        import os
        import time

        path = self._snapshot_path()
        if not path or not os.path.isfile(path):
            return
        try:
            with open(path, encoding="utf-8") as f:
                payload = json.load(f)
        except (OSError, ValueError):
            return
        if time.time() - payload.get("ts", 0) > self._SNAPSHOT_MAX_AGE_S:
            return
        values = payload.get("values") or {}
        locks = {
            "GPU": self.app._dashboard_gpu_lock_active
            if hasattr(self.app, "_dashboard_gpu_lock_active")
            else False,
        }
        for key, row in self._rows.items():
            val = values.get(key)
            if val is not None:
                try:
                    row.update(float(val), locked=locks.get(key))
                except (TypeError, ValueError):
                    continue

    def _sync_lock_state_from_cache(self) -> None:
        """Refresh dashboard lock flags from app cache even before VF tab is created."""
        cache = getattr(self.app, "_gpu_limits_cache", {}) or {}

        def _has_pair(lo_key: str, hi_key: str) -> bool:
            lo = cache.get(lo_key)
            hi = cache.get(hi_key)
            return (
                lo is not None
                and hi is not None
                and str(lo).strip() != ""
                and str(hi).strip() != ""
            )

        self.app._dashboard_gpu_lock_active = _has_pair(
            "vfp_lock_gpu_core_lowerbound_mhz",
            "vfp_lock_gpu_core_upperbound_mhz",
        )
        self.app._dashboard_mem_lock_active = _has_pair(
            "vfp_lock_memory_lowerbound_mhz",
            "vfp_lock_memory_upperbound_mhz",
        )

    # ── Polling ───────────────────────────────────────────────────────────────
    def _start_polling(self) -> None:
        if self._polling:
            return
        self._polling = True
        self._toggle_btn.configure(text="⏸ Pause")
        self._schedule_next()

    def _stop_polling(self) -> None:
        self._polling = False
        self._toggle_btn.configure(text="▶ Resume")
        if self._poll_job:
            try:
                self.app.after_cancel(self._poll_job)
            except Exception:
                pass
            self._poll_job = None

    def _toggle_polling(self) -> None:
        if self._polling:
            self._stop_polling()
        else:
            self._start_polling()

    def _on_interval_changed(self, _event: object = None) -> None:
        try:
            secs = float(self._interval_var.get())
            secs = max(0.2, min(60.0, secs))
            self._interval_ms = int(secs * 1000)
            self._interval_var.set(f"{secs:.1f}")
        except ValueError:
            self._interval_var.set(f"{self._interval_ms / 1000:.1f}")

    def _schedule_next(self) -> None:
        if self._polling:
            self._poll_job = self.app.after(self._interval_ms, self._poll_tick)

    def _poll_tick(self) -> None:
        if not self._polling:
            return
        # Pause the NVAPI+NVML sweep during a resize drag: the background
        # query contends with DWM at the GPU-driver level (synchronized
        # stutter once per poll interval) and its after(0,...) completion
        # interrupts the high-frequency Configure event burst on the main
        # thread. Resize end flushes the last sample via on_resize_state_changed.
        if self._is_resize_active:
            self._schedule_next()
            return
        # Pause while the VF Curve tab is being interacted with (point drag /
        # selection drag): the per-second NVAPI sweep's main-thread completion
        # (parse + rows + snapshot + live-point) interrupts the drag's event
        # stream. The next tick after the interaction ends resumes polling.
        tab_vfcurve = getattr(self.app, "tab_vfcurve", None)
        if tab_vfcurve is not None and getattr(tab_vfcurve, "is_interacting", False):
            self._schedule_next()
            return
        # Don't burn a full NVAPI+NVML status sweep per second while the
        # window is in the tray. Keep it running on the Dashboard (row
        # refresh) AND on VF Curve: the curve's green live-crosshair
        # reuses this status poll's voltage/frequency instead of issuing
        # its own (heavier) VFP-curve query, so the poll must keep firing
        # while the curve tab is visible or the crosshair freezes.
        try:
            if self.app.state() == "withdrawn":
                self._schedule_next()
                return
            current_tab = str(self.app.tabview.get())
            if not (
                current_tab.endswith("Dashboard") or current_tab.endswith("VF Curve")
            ):
                self._schedule_next()
                return
        except Exception:
            pass
        self._fetch_once()

    def _fetch_once(self) -> None:
        # Sync lock state from cache at the start of every fetch
        self._sync_lock_state_from_cache()

        if self._fetching:
            return
        gpu_args = self.app.get_gpu_args()
        if not gpu_args:
            self._status_lbl.configure(text="⚠ No GPU selected")
            self._schedule_next()
            return
        self._fetching = True
        self._status_lbl.configure(text="⟳ Fetching…")
        self.app.run_gpu_query_async(
            ["status", "-a"],
            self._on_done,
            thread_name="dash-poll",
            use_cache=False,  # polling wants live data, not TTL reuse
            allow_wake=not self._first_success,  # first sample may wake
        )

    def _on_done(self, retcode: int, output: str) -> None:
        self._fetching = False
        if retcode == 0:
            self._first_success = True

        if self._is_resize_active:
            # Keep only latest sample while resize is active, then flush once.
            self._pending_done_payload = (retcode, output)
            self._status_lbl.configure(text="⟳ Resizing… deferring dashboard redraw")
            self._schedule_next()
            return

        # On the VF Curve tab the metric rows / snapshot are invisible — only
        # the live crosshair (voltage/frequency) consumes the poll. Skip the
        # row redraws + snapshot write to keep the main thread free for the
        # curve's drag/blit event stream (a full _parse_and_update every
        # second interrupted point drags).
        live_only = self._vfcurve_active()
        self._apply_done_payload(retcode, output, live_only=live_only)
        self._schedule_next()

    def _vfcurve_active(self) -> bool:
        """True when the VF Curve tab is the currently shown tab."""
        try:
            return str(self.app.tabview.get()).endswith("VF Curve")
        except Exception:
            return False

    def _apply_done_payload(
        self, retcode: int, output: str, live_only: bool = False
    ) -> None:
        """Apply a status poll result to the UI in one batched update.

        ``live_only`` (VF Curve tab) skips the metric-row redraws and the
        snapshot write — only the live crosshair's voltage/frequency are
        forwarded to the curve tab.
        """

        if retcode == 0:
            self._parse_and_update(output, live_only=live_only)
        else:
            if not live_only:
                self._status_lbl.configure(text="⚠ CLI error")
                for row in self._rows.values():
                    row.set_error()

    def on_resize_state_changed(
        self, resizing: bool, force_flush: bool = False
    ) -> None:
        self._is_resize_active = resizing
        for row in self._rows.values():
            row.set_resize_active(resizing)
        should_flush = (not resizing) and (
            force_flush or self._pending_done_payload is not None
        )
        if should_flush and self._pending_done_payload is not None:
            retcode, output = self._pending_done_payload
            self._pending_done_payload = None
            self._apply_done_payload(retcode, output)

    # ── Parsing ───────────────────────────────────────────────────────────────
    def _parse_and_update(self, output: str, live_only: bool = False) -> None:
        parsed_status = parse_dashboard_status(output)
        gpu_mhz = as_float(parsed_status.get("gpu_clock_mhz"))
        mem_mhz = as_float(parsed_status.get("mem_clock_mhz"))
        volt_mv = as_float(parsed_status.get("voltage_mv"))
        temp_c = as_float(parsed_status.get("temperature_c"))
        pwr_w = as_float(parsed_status.get("power_w"))

        # VF Curve tab: only the live crosshair (voltage/frequency) is visible.
        # Forward just those two and return — skip the lock sync, the 5 metric
        # rows, and the snapshot write, so the main thread stays free for the
        # curve's drag/blit event stream.
        if live_only:
            if getattr(self.app, "tab_vfcurve", None) is not None:
                self.app.tab_vfcurve.update_live_point(volt_mv, gpu_mhz)
            return

        locked_value = parsed_status.get("vfp_locked")
        locked: Optional[bool] = (
            bool(locked_value) if isinstance(locked_value, bool) else None
        )

        # Fallback: check vfcurve lock state
        if locked is None and getattr(self.app, "tab_vfcurve", None):
            lpts = getattr(self.app.tab_vfcurve, "_locked_points", set())
            locked = len(lpts) > 0

        self._sync_lock_state_from_cache()

        gpu_bar_locked: Optional[bool] = (
            True if getattr(self.app, "_dashboard_gpu_lock_active", False) else None
        )
        mem_bar_locked: Optional[bool] = (
            True if getattr(self.app, "_dashboard_mem_lock_active", False) else None
        )

        if getattr(self.app, "tab_vfcurve", None):
            if getattr(self.app.tab_vfcurve, "_freq_core_lock", None) is not None:
                gpu_bar_locked = True
            if getattr(self.app.tab_vfcurve, "_freq_mem_lock", None) is not None:
                mem_bar_locked = True

        # update vfcurve live point
        if getattr(self.app, "tab_vfcurve", None):
            self.app.tab_vfcurve.update_live_point(volt_mv, gpu_mhz)

        # ── update rows ──
        updates = [
            ("GPU", gpu_mhz, gpu_bar_locked),
            ("MEM", mem_mhz, mem_bar_locked),
            ("VOLT", volt_mv, locked),
            ("TEMP", temp_c, None),
            ("PWR", pwr_w, None),
        ]
        fresh = {}
        for key, val, lk in updates:
            if val is not None:
                self._rows[key].update(val, locked=lk)
                fresh[key] = val
            else:
                self._rows[key].set_error()
        if fresh:
            self._save_snapshot(fresh)

    # ── Quick-access button handlers ──────────────────────────────────────────
    def _refresh_info(self) -> None:
        self.app.show_gpu_command(["info"])

    def _show_status(self) -> None:
        self.app.show_gpu_command(["status", "-a"])

    def _show_get(self) -> None:
        self.app.show_gpu_command(["get"])
