"""Fan-curve editor popup (ClientFanPolicies table).

A console-style standalone window (proxy in app.py) hosting a matplotlib
chart of one curve slot's 3 (Tj °C → RPM) points. Points drag on the
chart; the Tj/RPM entries mirror the chart both ways. The RPM↔PWM (%)
conversion keys off the cooler family's max-RPM readout (right axis).

The driver enforces strict monotonicity on both lanes (violations answer
a generic -5), so every edit path funnels through ``clamp_curve_points``.
Set Curve writes the slot + activates it (same-transaction
TemperatureContinuous policy switch + best-effort percent-pin release —
the CLI ``set-fan-curve --activate`` semantics); Reset Curve restores
the factory slot; Fan Stop toggles the arbiter zero-RPM flag. The curve
surface itself is per-GPU (no per-fan selector), so Apply Section with
policy=curve activates whatever slot/points this editor currently holds.
"""

from __future__ import annotations

from typing import List, Optional, Tuple

import customtkinter as ctk

# De-CTk'd palette (matches fan.py / overclock.py dark panels)
_PANE_BG = "#2b2b2b"
_CHART_BG = "#232323"
_TEXT_FG = "#e5e5e5"
_TEXT_FG_DIM = "#b3b3b3"
_GRID_COLOR = "#3a3a3a"
_ACCENT = "#59b0ff"
_LINE_COLOR = "#44cc88"
_POINT_COLOR = "#f5f7fb"
_FONT_BODY = ("Segoe UI", 11)

# Editor bounds: temperature lane ceiling (driver stores Q8.8 °C) and the
# RPM ceiling used when the cooler max-RPM readout is unavailable.
MAX_TEMP_C = 110
_RPM_FALLBACK_MAX = 30000
_POINT_COUNT = 3
_SLOT_COUNT = 4


def rpm_to_percent(rpm: int, max_rpm: Optional[int]) -> Optional[int]:
    """Driver RPM → duty % via the cooler's max-RPM readout."""
    if not max_rpm or max_rpm <= 0:
        return None
    return max(0, min(100, round(rpm * 100 / max_rpm)))


def percent_to_rpm(percent: int, max_rpm: Optional[int]) -> Optional[int]:
    """Duty % → driver RPM (inverse of :func:`rpm_to_percent`)."""
    if not max_rpm or max_rpm <= 0:
        return None
    return max(0, min(max_rpm, round(percent * max_rpm / 100)))


def clamp_curve_points(
    points: List[Tuple[float, float]],
    max_rpm: Optional[int],
    max_temp: int = MAX_TEMP_C,
) -> Optional[List[Tuple[int, int]]]:
    """Repair dragged/typed points into what the driver will accept.

    Clamps each point into range, then enforces the strict monotonicity of
    both lanes by nudging later points forward (the driver's Set handler
    rejects any non-increasing lane with a generic -5). Returns ``None``
    when the points cannot fit (a later point would be pushed past the
    ceiling — undoing a drag towards the wall).
    """
    rpm_ceiling = max_rpm if max_rpm and max_rpm > 0 else _RPM_FALLBACK_MAX
    repaired: List[Tuple[int, int]] = []
    prev_t: Optional[int] = None
    prev_r: Optional[int] = None
    for raw_t, raw_r in points:
        try:
            t = int(max(0, min(max_temp, round(float(raw_t)))))
            r = int(max(0, min(rpm_ceiling, round(float(raw_r)))))
        except (TypeError, ValueError):
            return None
        if prev_t is not None:
            t = max(t, prev_t + 1)
            r = max(r, prev_r + 1)
            if t > max_temp or r > rpm_ceiling:
                return None
        repaired.append((t, r))
        prev_t, prev_r = t, r
    return repaired


class FanCurveEditor(ctk.CTkFrame):
    """Curve slot table + draggable matplotlib chart (one GPU at a time)."""

    def __init__(self, master, app) -> None:
        super().__init__(master, fg_color=_PANE_BG, corner_radius=10)
        self.app = app
        self._slot = 0
        self._points: Optional[List[Tuple[int, int]]] = None
        self._max_rpm: Optional[int] = None
        self._loaded_gpu: Optional[str] = None
        self._drag_index: Optional[int] = None
        self._syncing = False
        self._entry_vars: List[Tuple[ctk.StringVar, ctk.StringVar]] = []
        self._pct_labels: List[ctk.CTkLabel] = []
        self._build()

    # ── UI construction ────────────────────────────────────────────────
    def _build(self) -> None:
        # Top bar: GPU label · slot selector · fan-stop switch
        top = ctk.CTkFrame(self, fg_color="transparent")
        top.pack(fill="x", padx=10, pady=(8, 2))
        self.gpu_label = ctk.CTkLabel(
            top, text="GPU —", font=("Segoe UI", 12, "bold"), text_color=_TEXT_FG
        )
        self.gpu_label.pack(side="left")
        self.slot_var = ctk.StringVar(value="Curve 0")
        self.slot_menu = ctk.CTkOptionMenu(
            top,
            variable=self.slot_var,
            values=[f"Curve {i}" for i in range(_SLOT_COUNT)],
            width=110,
            command=self._on_slot_change,
        )
        self.slot_menu.pack(side="right")
        self.fan_stop_switch = ctk.CTkSwitch(
            top, text="Fan Stop", command=self._on_fan_stop_toggle, width=90
        )
        self.fan_stop_switch.pack(side="right", padx=(0, 12))

        # Chart (lazy matplotlib embed, VFCurveTab pattern)
        chart_host = ctk.CTkFrame(self, fg_color=_CHART_BG, corner_radius=8)
        chart_host.pack(fill="both", expand=True, padx=10, pady=(4, 4))
        self._build_chart(chart_host)

        # Point table: rows = Point 0..2, columns = Tj / RPM / %
        table = ctk.CTkFrame(self, fg_color="transparent")
        table.pack(fill="x", padx=10, pady=(2, 2))
        for col in range(4):
            table.grid_columnconfigure(col, weight=0 if col == 0 else 1)
        for col, title in enumerate(("", "Tj (°C)", "RPM", "PWM %")):
            ctk.CTkLabel(
                table, text=title, text_color=_TEXT_FG_DIM, font=_FONT_BODY
            ).grid(row=0, column=col, pady=(0, 2), sticky="ew")
        for i in range(_POINT_COUNT):
            ctk.CTkLabel(
                table,
                text=f"Point {i}",
                text_color=_TEXT_FG,
                font=_FONT_BODY,
            ).grid(row=i + 1, column=0, sticky="w", padx=(0, 6))
            t_var = ctk.StringVar(value="")
            r_var = ctk.StringVar(value="")
            t_var.trace_add("write", lambda *_a, i=i, w=0: self._on_entry_write(i, w))
            r_var.trace_add("write", lambda *_a, i=i, w=1: self._on_entry_write(i, w))
            self._entry_vars.append((t_var, r_var))
            ctk.CTkEntry(table, textvariable=t_var, width=70, justify="center").grid(
                row=i + 1, column=1, sticky="ew", padx=3, pady=2
            )
            ctk.CTkEntry(table, textvariable=r_var, width=70, justify="center").grid(
                row=i + 1, column=2, sticky="ew", padx=3, pady=2
            )
            pct = ctk.CTkLabel(table, text="—", text_color=_TEXT_FG_DIM)
            pct.grid(row=i + 1, column=3, sticky="ew", padx=3)
            self._pct_labels.append(pct)

        # Bottom bar: status + action buttons
        bottom = ctk.CTkFrame(self, fg_color="transparent")
        bottom.pack(fill="x", padx=10, pady=(2, 10))
        self.status_label = ctk.CTkLabel(
            bottom, text="", text_color=_TEXT_FG_DIM, font=_FONT_BODY
        )
        self.status_label.pack(side="left", padx=(0, 8))
        ctk.CTkButton(
            bottom,
            text="Set Curve",
            width=100,
            fg_color="#1a6b2a",
            hover_color="#145220",
            command=self._on_set_curve,
        ).pack(side="right", padx=(6, 0))
        ctk.CTkButton(
            bottom,
            text="Reset Curve",
            width=100,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._on_reset_curve,
        ).pack(side="right")

    def _build_chart(self, parent) -> None:
        import matplotlib

        matplotlib.use("Agg")  # non-interactive; blitted to Tk (VFCurveTab)
        from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg
        from matplotlib.figure import Figure

        self.fig = Figure(figsize=(5.2, 2.4), dpi=96)
        self.fig.patch.set_facecolor(_CHART_BG)
        self.ax = self.fig.add_subplot(111)
        self.ax2 = self.ax.twinx()
        self.fig.subplots_adjust(left=0.1, right=0.9, top=0.94, bottom=0.14)
        self.canvas = FigureCanvasTkAgg(self.fig, master=parent)
        self.canvas.get_tk_widget().pack(fill="both", expand=True, padx=4, pady=4)
        self.canvas.mpl_connect("button_press_event", self._on_press)
        self.canvas.mpl_connect("motion_notify_event", self._on_motion)
        self.canvas.mpl_connect("button_release_event", self._on_release)
        self._redraw()

    # ── data flow ──────────────────────────────────────────────────────
    def current_state(self) -> Tuple[int, Optional[List[Tuple[int, int]]]]:
        """(slot, points) the editor holds — points None when no readable
        table is loaded (Apply-with-curve then writes the driver's own
        points back before activating)."""
        return self._slot, list(self._points) if self._points else None

    def load(self) -> None:
        """Reload the table + max-RPM anchor for the currently selected GPU."""
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self._set_status("No GPU selected.")
            return
        self._loaded_gpu = gpu
        self.gpu_label.configure(text=f"GPU {gpu}")
        self._set_status("Loading fan curve …")
        self.app.backend.query_fan_curve(gpu, self._on_curves_loaded)
        self.app.backend.query_cooler_max_rpm(gpu, self._on_max_rpm_loaded)

    def _on_curves_loaded(self, gpu: str, payload: Optional[dict]) -> None:
        if gpu != self._loaded_gpu:
            return  # GPU switched mid-flight
        curves = payload.get("curves") if isinstance(payload, dict) else None
        points: Optional[List[Tuple[int, int]]] = None
        if isinstance(curves, list):
            for curve in curves:
                if not isinstance(curve, dict) or curve.get("index") != self._slot:
                    continue
                raw = curve.get("points")
                if isinstance(raw, list) and len(raw) == _POINT_COUNT:
                    try:
                        points = [
                            (int(p["temp_c"]), int(p["rpm"]))
                            for p in raw
                            if isinstance(p, dict)
                        ]
                    except (KeyError, TypeError, ValueError):
                        points = None
                break
        if points is not None and len(points) == _POINT_COUNT:
            self._points = points
            self._set_status(f"Curve {self._slot} loaded (driver table).")
        else:
            self._points = None
            self._set_status(
                f"Curve {self._slot}: table unavailable on this GPU (N/A)."
            )
        self._sync_entries()
        self._redraw()

    def _on_max_rpm_loaded(self, gpu: str, max_rpm: Optional[int]) -> None:
        if gpu != self._loaded_gpu:
            return
        self._max_rpm = max_rpm
        self._sync_entries()
        self._redraw()

    def _set_status(self, text: str) -> None:
        self.status_label.configure(text=text)

    # ── entries ↔ model ────────────────────────────────────────────────
    def _sync_entries(self) -> None:
        self._syncing = True
        try:
            for i in range(_POINT_COUNT):
                t_var, r_var = self._entry_vars[i]
                if self._points is not None:
                    temp_c, rpm = self._points[i]
                    t_var.set(str(temp_c))
                    r_var.set(str(rpm))
                    percent = rpm_to_percent(rpm, self._max_rpm)
                    self._pct_labels[i].configure(
                        text="—" if percent is None else f"{percent}%"
                    )
                else:
                    t_var.set("")
                    r_var.set("")
                    self._pct_labels[i].configure(text="—")
        finally:
            self._syncing = False

    def _on_entry_write(self, index: int, which: int) -> None:
        if self._syncing or self._points is None:
            return
        var = self._entry_vars[index][which]
        try:
            value = int(float(var.get().strip()))
        except ValueError:
            return
        pts = list(self._points)
        temp_c, rpm = pts[index]
        pts[index] = (value, rpm) if which == 0 else (temp_c, value)
        repaired = clamp_curve_points(pts, self._max_rpm)
        if repaired is None:
            # Out-of-lane edit — snap the entries back to the held model.
            self._syncing = True
            try:
                var.set(str(self._points[index][which]))
            finally:
                self._syncing = False
            return
        self._points = repaired
        if repaired != pts:
            self._sync_entries()
        self._redraw()

    # ── chart interaction ──────────────────────────────────────────────
    def _rpm_span(self) -> float:
        return float(self._max_rpm or 3000)

    def _on_press(self, event) -> None:
        if self._points is None or event.inaxes is not self.ax:
            return
        if event.xdata is None or event.ydata is None:
            return
        span_t, span_r = float(MAX_TEMP_C), self._rpm_span()
        best: Optional[int] = None
        best_d = 0.09  # ≈10 °C / ≈270 RPM at 3000 — generous pick radius
        for i, (temp_c, rpm) in enumerate(self._points):
            d = ((temp_c - event.xdata) / span_t) ** 2 + (
                (rpm - event.ydata) / span_r
            ) ** 2
            d = d**0.5
            if d <= best_d:
                best, best_d = i, d
        self._drag_index = best

    def _on_motion(self, event) -> None:
        if self._drag_index is None or self._points is None:
            return
        if event.inaxes is not self.ax or event.xdata is None or event.ydata is None:
            return
        pts = list(self._points)
        pts[self._drag_index] = (event.xdata, event.ydata)
        repaired = clamp_curve_points(pts, self._max_rpm)
        if repaired is None:
            return  # cannot fit against the walls — hold position
        self._points = repaired
        self._sync_entries()
        self._redraw()

    def _on_release(self, _event) -> None:
        self._drag_index = None

    def _redraw(self) -> None:
        ax, ax2 = self.ax, self.ax2
        ax.clear()
        ax2.clear()
        max_rpm = self._max_rpm or 3000
        ax.set_facecolor(_CHART_BG)
        ax.set_xlim(0, MAX_TEMP_C)
        ax.set_ylim(0, max_rpm * 1.08)
        ax2.set_ylim(0, 110)
        ax.set_xlabel("Temperature (°C)", color=_TEXT_FG_DIM, fontsize=9)
        ax.set_ylabel("Fan Speed (RPM)", color=_TEXT_FG_DIM, fontsize=9)
        ax2.set_ylabel("Duty (%)", color=_TEXT_FG_DIM, fontsize=9)
        ax.grid(True, color=_GRID_COLOR, linewidth=0.6)
        for axis in (ax, ax2):
            axis.tick_params(colors=_TEXT_FG_DIM, labelsize=8)
            for spine in axis.spines.values():
                spine.set_color(_GRID_COLOR)
        if self._points is None:
            ax.text(
                0.5,
                0.5,
                "Curve table unavailable (N/A)",
                transform=ax.transAxes,
                ha="center",
                va="center",
                color="#888888",
                fontsize=10,
            )
        else:
            temps = [p[0] for p in self._points]
            rpms = [p[1] for p in self._points]
            ax.plot(
                temps,
                rpms,
                color=_LINE_COLOR,
                linewidth=2,
                marker="o",
                markersize=8,
                markerfacecolor=_POINT_COLOR,
                markeredgecolor=_ACCENT,
                markeredgewidth=2,
            )
            for i, (temp_c, rpm) in enumerate(self._points):
                percent = rpm_to_percent(rpm, self._max_rpm)
                ax.annotate(
                    f"P{i}" + (f" {percent}%" if percent is not None else ""),
                    (temp_c, rpm),
                    textcoords="offset points",
                    xytext=(8, 6),
                    color=_TEXT_FG_DIM,
                    fontsize=8,
                )
        self.fig.subplots_adjust(left=0.1, right=0.9, top=0.94, bottom=0.14)
        self.canvas.draw_idle()

    # ── actions ────────────────────────────────────────────────────────
    def _on_slot_change(self, value: str) -> None:
        try:
            slot = int(str(value).split()[-1])
        except ValueError:
            return
        if slot == self._slot:
            return
        self._slot = slot
        # Fan-stop state is per slot and unread here — the switch resets.
        self.fan_stop_switch.deselect()
        self.load()

    def _on_fan_stop_toggle(self) -> None:
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self._set_status("No GPU selected.")
            return
        enable = bool(self.fan_stop_switch.get())
        self.app.backend.set_fanstop_status(gpu, enable, self._slot)
        self._set_status(
            f"Fan stop {'on' if enable else 'off'} for curve {self._slot}."
        )

    def _on_set_curve(self) -> None:
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self._set_status("No GPU selected.")
            return
        try:
            pts = [
                (
                    float(self._entry_vars[i][0].get().strip()),
                    float(self._entry_vars[i][1].get().strip()),
                )
                for i in range(_POINT_COUNT)
            ]
        except ValueError:
            self._set_status("Tj and RPM must be numbers.")
            return
        repaired = clamp_curve_points(pts, self._max_rpm)
        if repaired is None:
            self._set_status("Points must be strictly increasing in both Tj and RPM.")
            return
        self._points = repaired
        self._sync_entries()
        self.app.backend.set_fan_curve(gpu, self._slot, repaired, on_done=self.load)

    def _on_reset_curve(self) -> None:
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self._set_status("No GPU selected.")
            return
        self._set_status(f"Curve {self._slot} reset to factory — reloading …")
        # Reload in the action's on_finished — a concurrent load would race
        # the reset RMW and show the pre-reset table.
        self.app.backend.reset_fan_curve(gpu, self._slot, on_done=self.load)
