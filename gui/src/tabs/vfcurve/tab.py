"""
VF Curve Tab - Interactive voltage-frequency curve chart with matplotlib,
plus VFP export/import, lock/unlock, and point adjustment controls.
"""

import csv
import os
import time as _time

import tkinter as tk
import customtkinter as ctk
from tkinter import filedialog
from typing import TYPE_CHECKING, Dict, List, Optional, Tuple


if TYPE_CHECKING:
    from src.app import App

from src.widgets.lightweight_controls import (
    ct_button_font,
    LiteButton,
    LiteEntry,
)
from src.parsing import analyze_vfp_offsets, load_vfp_deltas, write_vfp_points

# ── De-CTk'd panel palette (matches overclock.py / fan_control.py) ──
_PANEL_BG = "#2b2b2b"  # CTk dark frame/scroll background
_TEXT_FG = "#e5e5e5"  # default label text
_TEXT_FG_DIM = "#b3b3b3"  # 'gray70' hints
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")
_CARD_KW = dict(border_width=1, border_color="#1f4e79", corner_radius=10)

# ── Multi-curve palette ──
# Per-curve current/default colors. The current/default line styles
# (solid+marker / dashed) are inherited from the original single-curve look.
_CURVE_COLORS = {
    "gpc": {"current": "#00ccff", "default": "#1f4e79"},  # cyan / deep blue
    "xbar": {"current": "#FF8C00", "default": "#7a3d00"},  # bright orange / dark orange
    "sys": {"current": "#B026FF", "default": "#4B0082"},  # bright purple / dark purple
}
# Display label + private-domain class for the raw-converted prior.
# The third curve was initially mislabeled HOST until a voltage-lock A/B
# proved it tracks SYS (0.89 V → curve 1980 MHz vs live SYS 1994 MHz; the
# Host clock never exceeds 1350 MHz) — see ClkVfSegment::domain_hint in
# nvapi-rs. domain_bit is the ClkDomains record whose offset shifts this
# curve (bit 5's record, labeled Host by the RTSS-derived table).
_CURVE_META = {
    "gpc": {"label": "GPC", "class": "graphics", "domain_bit": 0},
    "xbar": {"label": "XBAR", "class": "fabric", "domain_bit": 1},
    "sys": {"label": "SYS", "class": "fabric", "domain_bit": 5},
}


class _CurveData:
    """One VF curve (GPC/XBAR/SYS) loaded from public or private NVAPI.

    ``voltages``/``frequencies``/``defaults`` are in display units (mV / MHz).
    ``source`` is "public" (GPC via the open VFP interface) or "private"
    (any segment from ``query_private_vftable``). ``seg_start``/``seg_end`` are
    the inclusive private point indices within ``bank`` (used for private
    apply/reset); for the public GPC curve they mirror the public index range.
    ``write_mode`` decides how ``_apply_adj`` / reset reach the GPU.
    """

    __slots__ = (
        "curve_id",
        "voltages",
        "frequencies",
        "defaults",
        "source",
        "bank",
        "seg_start",
        "seg_end",
        "write_mode",
        "has_fixed",
    )

    def __init__(self, curve_id: str):
        self.curve_id = curve_id
        self.voltages: List[float] = []
        self.frequencies: List[float] = []
        self.defaults: List[float] = []
        self.source = "public"
        self.bank = 0
        self.seg_start = 0
        self.seg_end = 0
        # "public" (open VFP) | "private_mode0" (private freq-kHz offset) |
        # "private_raw_converted" (private mode-1 raw f-offset via g(def))
        self.write_mode = "public"
        self.has_fixed = False


class VFCurveTab:
    """VF Curve management tab with interactive chart."""

    # ── Chart export directory (relative to GUI project root) ──
    _EXPORT_DIR = "vfp_cache"
    _DEFAULT_AUTO_REFRESH_INTERVAL_MS = 1000

    @staticmethod
    def _np():
        """Lazy numpy: only the chart interaction paths need it (~34ms import)."""
        import numpy as np

        return np

    def __init__(self, parent: ctk.CTkFrame, app: "App"):
        self.app = app
        self.frame = parent

        # VF data (in display units: mV and MHz)
        self._voltages: List[float] = []
        self._frequencies: List[float] = []  # current
        self._defaults: List[float] = []  # default_frequency

        # ── Multi-curve state ──
        # One _CurveData per discovered curve (gpc/xbar/sys). _voltages /
        # _frequencies / _defaults above are kept as a live view of the active
        # curve's lists (same object references) so every existing single-curve
        # call site (drag, keyboard, space-key, lock, dashboard poll, tests)
        # continues to operate on the active curve unchanged.
        self._curves: Dict[str, _CurveData] = {}
        self._active_curve: str = "gpc"
        self._curve_visible: Dict[str, bool] = {}
        self._curve_lines: Dict[str, dict] = {}  # curve_id -> {"current","default"}
        self._curve_selector_row: Optional[tk.Frame] = None
        self._curve_selector_btns: Dict[str, tk.Frame] = {}
        # per-GPU probe of which private write mode each curve supports, so a
        # mode-0 capability probe isn't repeated every refresh.
        self._curve_probe_cache: Dict[str, Dict[str, str]] = {}
        # Monotonic guard so a stale async query doesn't overwrite a newer load.
        self._curve_query_epoch: int = 0

        # Selection state  (indices into the data arrays)
        self._sel_start: Optional[int] = None
        self._sel_end: Optional[int] = None

        # Single-point lock state: set of locked point indices
        self._locked_points = set()  # type: Set[int]

        # Frequency lock state (core/memory): (min_mhz, max_mhz)
        self._freq_core_lock = None  # type: Optional[Tuple[int, int]]
        self._freq_mem_lock = None  # type: Optional[Tuple[int, int]]
        self._freq_core_lock_backend = None  # type: Optional[str]
        self._freq_mem_lock_backend = None  # type: Optional[str]

        # Drag state
        self._dragging = False
        self._drag_start_y: Optional[float] = None
        self._drag_orig_freqs = None  # numpy array, created lazily

        # Live point state
        self._live_volt: Optional[float] = None
        self._live_freq: Optional[float] = None
        self._live_elements: list = []
        self._live_hline = None
        self._live_vline = None
        self._live_marker = None
        self._live_text = None
        self._cleaned_up = False
        # x-tick value whose label renders as "Volt/V" (set after each
        # curve load; consumed by the x-axis FuncFormatter in _style_axes)
        self._volt_unit_tick: Optional[float] = None
        self._chart_build_after_id: Optional[str] = None
        self._chart_resize_after_id = None
        self._chart_configure_bind_id: Optional[str] = None
        self._mpl_connection_ids: list[int] = []
        self._last_chart_event_width: Optional[int] = None
        self._last_chart_event_height: Optional[int] = None
        self._last_chart_resize_width: Optional[int] = None
        self._last_chart_resize_height: Optional[int] = None
        self._pending_chart_resize_wh: Optional[Tuple[int, int]] = None
        self._is_resize_active = False
        # True while a mouse button is held on the chart (point drag /
        # selection drag). The dashboard poll's live-point update is deferred
        # to pending during interaction — its per-second matplotlib blit was
        # contending with the drag's own blit over the cached background,
        # making point drags stutter once per second.
        self._mouse_pressed = False
        self._pending_live_point: Optional[Tuple[Optional[float], Optional[float]]] = (
            None
        )
        self._pending_full_redraw = False
        self._refresh_curve_inflight = False
        self._refresh_curve_pending = False
        self._last_load_ts = 0.0  # dedupes back-to-back refresh chains
        self._auto_refresh_job: Optional[str] = None
        self._auto_refreshing = False
        self._auto_refresh_interval_ms = self._DEFAULT_AUTO_REFRESH_INTERVAL_MS
        self._auto_interval_var = ctk.StringVar(value="1.0")
        self._auto_toggle_btn = None
        # Live crosshair poller — independent of the dashboard poll so its
        # after(0,_on_done) cannot interpose a blit ahead of a mouse-press
        # event in the Tcl queue. The worker writes volt/freq into
        # _live_pending (GIL-safe scalar); this timer picks them up and
        # blits at a low cadence, skipping while the user is interacting.
        self._live_poll_job: Optional[str] = None
        self._live_pending: Tuple[Optional[float], Optional[float]] = (None, None)
        self._live_poll_inflight = False
        # Direct-read inflight guard for xbar/sys live-point polling.
        self._direct_read_inflight = False

        # ── P0 voltage-boundary vertical lines ──
        # Hardware walls (floor = min_hold, ceiling = min(vbios,vrm)) are
        # immutable per-GPU → cached once on first load, never re-queried on
        # subsequent refreshes. The effective wall (light red) is pushed in by
        # the overclock panel after a Volt Limit apply, since it only changes
        # on SET. None = not available / not drawn. All three lines fall
        # inside the curve's voltage range by design — no axis adjustment.
        self._p0_bounds: Optional[dict] = None
        self._p0_bounds_gpu: Optional[str] = None
        self._p0_effective_wall_mv: Optional[float] = None
        # The light-red effective wall is draggable: a drag moves a *pending*
        # dashed copy (clamp to [floor, ceiling]); on "Apply to GPU" the
        # pending value is written via set_volt_rail_target and the returned
        # effective wall updates the solid line. _p0_rail_bit is the rail the
        # setter targets (resolved once from the volt_rails descriptor/mask).
        self._pending_wall_mv: Optional[float] = None
        self._dragging_wall: bool = False
        self._p0_rail_bit: int = 0
        self._pending_wall_line = None  # animated dashed axvline (blit overlay)
        # Wall-drag handle: a triangle in the top figure margin (above the
        # axes) whose tip points down at the wall line. Clicks on it have
        # inaxes=None (it's outside the axes), so they never reach the
        # point-select logic — zero interference with curve editing.
        self._wall_handle = None  # animated Polygon patch (blit overlay)

        # ── Top: chart area (controls row + plot) ──
        self._chart_area = tk.Frame(self.frame, bg=_PANEL_BG)
        self._chart_area.pack(fill="x", expand=False, padx=10, pady=(10, 5))

        chart_top = tk.Frame(self._chart_area, bg=_PANEL_BG)
        chart_top.pack(fill="x", pady=(0, 4))
        tk.Label(
            chart_top,
            text="📈 VF Curve",
            font=_FONT_HEADER,
            bg=_PANEL_BG,
            fg="#aaccff",
        ).pack(side="left", padx=8)

        io_row = tk.Frame(chart_top, bg=_PANEL_BG)
        io_row.pack(side="left", padx=(12, 0))
        LiteButton(io_row, text="📤Export", width=75, command=self._export_vfp).pack(
            side="left", padx=(0, 4)
        )
        LiteButton(io_row, text="📥Import", width=75, command=self._import_vfp).pack(
            side="left", padx=(0, 4)
        )
        LiteButton(
            io_row,
            text="🔁Reset",
            width=75,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_vfp,
        ).pack(side="left")

        auto_row = tk.Frame(chart_top, bg=_PANEL_BG)
        auto_row.pack(side="right")
        self._auto_toggle_btn = LiteButton(
            auto_row, text="▶ Auto", width=82, command=self._toggle_auto_refresh
        )
        self._auto_toggle_btn.pack(side="left", padx=(0, 8))
        tk.Label(
            auto_row, text="Refresh:", font=_FONT_BODY, bg=_PANEL_BG, fg=_TEXT_FG
        ).pack(side="left", padx=(0, 4))
        auto_interval_entry = LiteEntry(
            auto_row,
            textvariable=self._auto_interval_var,
            width=5,
            min_px=52,
            justify="right",
        )
        auto_interval_entry.pack(side="left", padx=(0, 2))
        tk.Label(
            auto_row, text="s", font=_FONT_BODY, bg=_PANEL_BG, fg=_TEXT_FG_DIM
        ).pack(side="left")
        auto_interval_entry.bind("<Return>", self._on_auto_interval_changed)
        auto_interval_entry.bind("<FocusOut>", self._on_auto_interval_changed)

        self._chart_frame = ctk.CTkFrame(self._chart_area)
        self._chart_frame.pack(fill="both", expand=True)

        # ── Per-curve selector row (below the chart, above the toolbar) ──
        # Only packed when more than one curve is discovered (see _rebuild_selector).
        self._curve_selector_host = tk.Frame(self.frame, bg=_PANEL_BG)

        # Schedule heavy chart init (and matplotlib import) to occur after UI starts
        self._chart_build_after_id = self.app.after(50, self._build_chart_if_alive)

        # ── Chart toolbar ──
        toolbar = tk.Frame(self.frame, bg=_PANEL_BG)
        toolbar.pack(fill="x", padx=10, pady=(0, 5))
        LiteButton(
            toolbar, text="🔄 Refresh Curve", width=140, command=self._refresh_curve
        ).pack(side="left", padx=5)
        LiteButton(
            toolbar, text="↩ Undo Drag Edit", width=140, command=self._undo_drag
        ).pack(side="left", padx=5)
        LiteButton(
            toolbar, text="🗑 Clear Selection", width=130, command=self._clear_selection
        ).pack(side="left", padx=5)
        LiteButton(
            toolbar,
            text="✅ Apply to GPU",
            width=130,
            fg_color="#1a6b2a",
            hover_color="#145220",
            command=self._apply_adj,
        ).pack(side="left", padx=5)
        tk.Label(toolbar, text="API:", font=_FONT_BODY, bg=_PANEL_BG, fg=_TEXT_FG).pack(
            side="left", padx=(10, 2)
        )
        self.freq_lock_api_var = ctk.StringVar(value="NVAPI")
        self.freq_lock_api_menu = ctk.CTkOptionMenu(
            toolbar,
            values=["NVAPI", "NVML"],
            variable=self.freq_lock_api_var,
            width=84,
            height=28,
            anchor="center",
            font=ct_button_font(toolbar),
        )
        self.freq_lock_api_menu.pack(side="left", padx=(0, 5))

        # ── Bottom half: host for the autoscan section ──
        self.autoscan_host = tk.Frame(self.frame, bg=_PANEL_BG)
        self.autoscan_host.pack(fill="both", expand=True, padx=10, pady=(0, 10))

        # ── State vars kept for logic compatibility (Point-Adj / lock UIs
        # removed: chart drag/wheel/space covers those operations) ──
        self.adj_start_var = ctk.StringVar(value="0")
        self.adj_end_var = ctk.StringVar(value="0")
        self.adj_delta_var = ctk.StringVar(value="0")
        self.lock_point_var = ctk.StringVar(value="55")
        self.lock_voltage_var = ctk.BooleanVar(value=False)
        self.core_lock_min_var = ctk.StringVar(value="0")
        self.core_lock_max_var = ctk.StringVar(value="0")
        self.mem_lock_min_var = ctk.StringVar(value="0")
        self.mem_lock_max_var = ctk.StringVar(value="0")

    # ────────────────────────────────────────────
    # Chart setup
    # ────────────────────────────────────────────
    def _build_chart_if_alive(self):
        self._chart_build_after_id = None
        if self._cleaned_up:
            return
        try:
            if not self._chart_frame.winfo_exists():
                return
        except Exception:
            return
        # Don't import matplotlib on the UI thread while the background warm-up
        # (font cache build) is still running — retry shortly instead.
        mpl_ready = getattr(self.app, "_mpl_ready", None)
        if mpl_ready is not None and not mpl_ready.is_set():
            self._chart_build_after_id = self.app.after(100, self._build_chart_if_alive)
            return
        self._build_chart(self._chart_frame)

    @staticmethod
    def _get_screen_dpi_scale(widget) -> float:
        """Return the effective DPI scaling factor of the screen hosting *widget*.

        On Windows at 150% scaling the physical DPI reported by Tk is ~144.
        We normalise against 96 (100% baseline) so the chart figure dpi
        grows proportionally and the canvas always fills its allocated space.
        """
        try:
            screen_dpi = widget.winfo_fpixels("1i")
            if screen_dpi < 90:
                screen_dpi = 96.0
            return screen_dpi / 96.0
        except Exception:
            return 1.0

    def _build_chart(self, parent: ctk.CTkFrame):
        """Create the matplotlib figure embedded in customtkinter."""
        if self._cleaned_up or getattr(self, "canvas", None) is not None:
            return

        # Lazy import matplotlib to avoid blocking GUI startup
        import matplotlib

        matplotlib.use("Agg")  # non-interactive backend; we blit to Tk manually
        from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg
        from matplotlib.figure import Figure

        try:
            scale = self._get_screen_dpi_scale(self.app)
        except Exception:
            scale = 1.0
        fig_dpi = max(72, round(100 * scale))

        self.fig = Figure(figsize=(9, 1.7), dpi=fig_dpi)
        self.fig.patch.set_facecolor("#2b2b2b")
        self.ax = self.fig.add_subplot(111)
        # y tick labels are back OUTSIDE the spine — left margin fits the
        # GHz numbers ("0.5", "1.0") plus a little headroom so the digits
        # never kiss the frame edge
        self.fig.subplots_adjust(left=0.062, right=0.995, top=0.92, bottom=0.12)
        self._style_axes()

        # Placeholder text
        self.ax.text(
            0.5,
            0.5,
            'Click  "Refresh Curve"  to load VF data',
            transform=self.ax.transAxes,
            ha="center",
            va="center",
            color="#888888",
            fontsize=9,
        )

        self.canvas = FigureCanvasTkAgg(self.fig, master=parent)
        self.canvas.get_tk_widget().pack(fill="both", expand=True)

        # Allow the canvas widget to receive keyboard events
        tk_widget = self.canvas.get_tk_widget()
        tk_widget.configure(takefocus=True)
        tk_widget.bind("<Enter>", lambda e: tk_widget.focus_set())
        tk_widget.bind("<space>", self._on_space_key)

        # ── Keyboard navigation bindings ──
        tk_widget.bind("<Left>", self._on_key_left)
        tk_widget.bind("<Right>", self._on_key_right)
        tk_widget.bind("<Up>", self._on_key_up)
        tk_widget.bind("<Down>", self._on_key_down)
        # Tab / Shift-Tab  (return "break" to prevent focus from leaving canvas)
        tk_widget.bind("<Tab>", self._on_key_tab)
        tk_widget.bind("<Shift-Tab>", self._on_key_shift_tab)
        # ── Mouse wheel frequency adjustment ──
        tk_widget.bind("<MouseWheel>", self._on_mousewheel)
        tk_widget.bind("<Button-4>", self._on_mousewheel)
        tk_widget.bind("<Button-5>", self._on_mousewheel)

        # Resize figure width when the parent frame width changes.
        # Height is kept fixed (3.5 in) so controls below are never squeezed out.
        self._chart_configure_bind_id = parent.bind(
            "<Configure>", self._on_chart_resize, add="+"
        )

        # Plot line references (created on first data load)
        self._line_current = None
        self._line_default = None
        self._sel_rect = None  # selection highlight
        self._sel_points = None  # selected point markers
        self._key_redraw_after_id = None  # deferred full redraw after key/wheel edits

        # Connect mouse events
        self._mpl_connection_ids = [
            self.canvas.mpl_connect("button_press_event", self._on_mouse_press),
            self.canvas.mpl_connect("button_release_event", self._on_mouse_release),
            self.canvas.mpl_connect("motion_notify_event", self._on_mouse_move),
        ]

        # Blitting: cache the static background after every full draw so
        # per-second live-point updates and fast curve edits only repaint
        # their own artists (~2x cheaper than a full-figure Agg render).
        self._blit_bg = None
        self._mpl_connection_ids.append(
            self.canvas.mpl_connect("draw_event", self._on_canvas_draw)
        )

        # Data may have loaded before the chart existed — draw it now.
        if self._pending_full_redraw or self._voltages:
            self._pending_full_redraw = False
            self._redraw()

        # The selector row couldn't pack before the chart existed (its host
        # uses after=self._chart_area, which needs the chart area mapped).
        self._rebuild_selector()

    def _animated_artists(self):
        """Artists managed outside the static background (blit overlay)."""
        artists = []
        if self._line_current is not None:
            artists.append(self._line_current)
        if self._sel_rect is not None:
            artists.append(self._sel_rect)
        if self._sel_points is not None:
            artists.append(self._sel_points)
        if self._pending_wall_line is not None:
            artists.append(self._pending_wall_line)
        # NOTE: _wall_handle is NOT animated — it lives in the figure margin
        # above ax.bbox, which the blit overlay cannot repaint. It is static,
        # repainted on each full _redraw.
        for el in self._live_elements:
            artists.append(el)
        return artists

    def _on_canvas_draw(self, _event):
        if self._cleaned_up or self.ax is None:
            return
        # Skip the post-draw overlay blit while a mouse button is held: it
        # contends with the drag's own blit over the cached background and, if
        # it lands in the Tcl queue ahead of a pending press, delays the click.
        # The drag/release path re-establishes the overlay as needed.
        if self._mouse_pressed or self._dragging:
            return
        try:
            # Full draw finished: static content is in the buffer; cache it,
            # then paint the animated artists on top (they were skipped).
            self._blit_bg = self.canvas.copy_from_bbox(self.ax.bbox)
            overlay_changed = False
            for artist in self._animated_artists():
                if getattr(artist, "get_visible", lambda: True)():
                    self.ax.draw_artist(artist)
                    overlay_changed = True
            if overlay_changed:
                self.canvas.blit(self.ax.bbox)
        except Exception:
            self._blit_bg = None

    def _blit_animated(self):
        """Repaint only the animated artists over the cached background."""
        if self._blit_bg is None or self._cleaned_up:
            self.canvas.draw_idle()  # no valid background: full redraw
            return
        try:
            self.canvas.restore_region(self._blit_bg)
            for artist in self._animated_artists():
                if getattr(artist, "get_visible", lambda: True)():
                    self.ax.draw_artist(artist)
            self.canvas.blit(self.ax.bbox)
        except Exception:
            self._blit_bg = None
            self.canvas.draw_idle()

    def _on_chart_resize(self, event):
        """Debounce figure width updates to avoid geometry thrash during live resize."""
        if not hasattr(self, "fig") or not hasattr(self, "canvas"):
            return
        if not self._chart_frame.winfo_ismapped():
            return
        w_px = max(1, int(event.width))
        if self._last_chart_event_width == w_px:
            return
        self._last_chart_event_width = w_px
        self._pending_chart_resize_width = w_px

        if self._is_resize_active:
            return

        if self._chart_resize_after_id is not None:
            try:
                self.app.after_cancel(self._chart_resize_after_id)
            except Exception:
                pass

        self._chart_resize_after_id = self.app.after(
            60, lambda width=w_px: self._apply_chart_resize(width)
        )

    def _apply_chart_resize(self, width_px: int):
        self._chart_resize_after_id = None
        self._blit_bg = None  # stale background: size changed
        if not hasattr(self, "fig") or not hasattr(self, "canvas"):
            return
        if width_px <= 0 or not self._chart_frame.winfo_ismapped():
            return
        if (
            self._last_chart_resize_width is not None
            and abs(width_px - self._last_chart_resize_width) < 8
        ):
            return

        dpi = self.fig.get_dpi()
        new_w = max(1.0, width_px / dpi)
        cur_w, cur_h = self.fig.get_size_inches()
        if abs(new_w - cur_w) * dpi < 2:
            return

        self._last_chart_resize_width = width_px
        self.fig.set_size_inches(new_w, cur_h)
        self.canvas.draw_idle()

    def _style_axes(self):
        from matplotlib.ticker import FuncFormatter

        ax = self.ax
        ax.set_facecolor("#1e1e1e")
        # axis labels one size smaller; ticks relabeled in V / GHz so the
        # numbers stay short (0.5 / 0.6 / ... and 0.5 / 1 / 1.5 / ...) —
        # plot data itself stays in mV / MHz
        # y-axis unit caption hugs the spine's RIGHT side at the axis top
        # (extending left would push it out of the figure)
        ax.text(
            0.0,
            1.02,
            "f/GHz",
            transform=ax.transAxes,
            ha="left",
            va="bottom",
            color="#e08020",
            fontsize=7,
        )

        # one decimal on BOTH axes (0.5 / 1.0 / 1.5 ...) — uniform columns.
        # NOTE: with a FuncFormatter attached, matplotlib regenerates tick
        # TEXT on every draw, so the "Volt/V" unit caption must come FROM
        # the formatter itself — a later set_text() on the label would be
        # silently overwritten (color survives, text doesn't).
        def x_format(v, _pos):
            if self._volt_unit_tick is not None and v == self._volt_unit_tick:
                return "Volt/V"
            return f"{v / 1000.0:.1f}"

        ax.xaxis.set_major_formatter(FuncFormatter(x_format))
        ax.yaxis.set_major_formatter(FuncFormatter(lambda v, _pos: f"{v / 1000.0:.1f}"))
        # tick marks grow INWARD from the spines (direction="in"); the
        # numbers stay outside, so each axis reads number + inward bar
        ax.tick_params(colors="#cccccc", labelsize=6, direction="in")
        for spine in ax.spines.values():
            spine.set_color("#555555")
        ax.grid(True, color="#333333", linewidth=0.5, alpha=0.7)

    # ────────────────────────────────────────────
    # Data loading
    # ────────────────────────────────────────────

    def _start_auto_refresh(self) -> None:
        if self._auto_refreshing:
            return
        self._auto_refreshing = True
        if self._auto_toggle_btn is not None:
            self._auto_toggle_btn.configure(text="⏸ Pause")
        self._schedule_next_auto_refresh()

    def _stop_auto_refresh(self) -> None:
        self._auto_refreshing = False
        if self._auto_toggle_btn is not None:
            self._auto_toggle_btn.configure(text="▶ Auto")
        if self._auto_refresh_job:
            try:
                self.app.after_cancel(self._auto_refresh_job)
            except Exception:
                pass
            self._auto_refresh_job = None

    def cleanup(self) -> None:
        """Release matplotlib resources."""
        self._cleaned_up = True

        if self._chart_build_after_id is not None:
            try:
                self.app.after_cancel(self._chart_build_after_id)
            except Exception:
                # Best-effort cleanup: callback may already be canceled or app may be shutting down.
                pass
            self._chart_build_after_id = None

        if self._chart_resize_after_id is not None:
            try:
                self.app.after_cancel(self._chart_resize_after_id)
            except Exception:
                # Best-effort cleanup: timer may already be canceled/destroyed during teardown.
                pass
            self._chart_resize_after_id = None

        if self._key_redraw_after_id is not None:
            try:
                self.app.after_cancel(self._key_redraw_after_id)
            except Exception:
                pass
            self._key_redraw_after_id = None

        self._stop_auto_refresh()
        self.stop_live_poll()

        if self._chart_configure_bind_id is not None:
            try:
                self._chart_frame.unbind("<Configure>", self._chart_configure_bind_id)
            except Exception:
                # Best-effort teardown: widget/bind may already be gone during shutdown.
                pass
            self._chart_configure_bind_id = None

        canvas = getattr(self, "canvas", None)
        for cid in self._mpl_connection_ids:
            try:
                if canvas is not None:
                    canvas.mpl_disconnect(cid)
            except Exception:
                # Best-effort cleanup: callback may already be disconnected or canvas invalid during shutdown.
                pass
        self._mpl_connection_ids.clear()

        self._live_elements.clear()
        self._live_hline = None
        self._live_vline = None
        self._live_marker = None
        self._live_text = None

        self._line_current = None
        self._line_default = None
        self._sel_rect = None
        self._sel_points = None
        self._pending_wall_line = None
        self._wall_handle = None

        if canvas is not None:
            try:
                canvas.get_tk_widget().destroy()
            except Exception:
                # Best-effort cleanup: widget may already be destroyed during shutdown.
                pass
            try:
                canvas._tkphoto = None
            except Exception:
                # Best-effort teardown: ignore backend-specific cleanup failures.
                pass
            self.canvas = None

        fig = getattr(self, "fig", None)
        if fig is not None:
            try:
                fig.clear()
            except Exception:
                # Best-effort cleanup: figure may already be disposed during shutdown.
                pass
            self.fig = None
        self.ax = None

    def _toggle_auto_refresh(self) -> None:
        if self._auto_refreshing:
            self._stop_auto_refresh()
        else:
            self._start_auto_refresh()

    def _on_auto_interval_changed(self, _event: object = None) -> None:
        try:
            secs = float(self._auto_interval_var.get())
            secs = max(0.2, min(60.0, secs))
            self._auto_refresh_interval_ms = int(secs * 1000)
            self._auto_interval_var.set(f"{secs:.1f}")
        except ValueError:
            self._auto_interval_var.set(f"{self._auto_refresh_interval_ms / 1000:.1f}")

        if self._auto_refreshing:
            self._schedule_next_auto_refresh()

    def _schedule_next_auto_refresh(self) -> None:
        if not self._auto_refreshing:
            return
        if self._auto_refresh_job:
            try:
                self.app.after_cancel(self._auto_refresh_job)
            except Exception:
                pass
        self._auto_refresh_job = self.app.after(
            self._auto_refresh_interval_ms, self._auto_refresh_tick
        )

    def _auto_refresh_tick(self) -> None:
        self._auto_refresh_job = None
        if not self._auto_refreshing:
            return

        # Skip the VFP RM-escape query during a resize drag: it is a heavier
        # sweep than the status poll and also contends with DWM at the driver
        # level. Re-arm; resize end re-renders from cached points.
        if self._is_resize_active:
            self._schedule_next_auto_refresh()
            return

        if self.app.selected_gpu_target() is None:
            self._schedule_next_auto_refresh()
            return

        if self._refresh_curve_inflight:
            self._schedule_next_auto_refresh()
            return

        try:
            current_tab = self.app.tabview.get()
            if not str(current_tab).endswith("VF Curve"):
                self._schedule_next_auto_refresh()
                return
        except Exception:
            pass

        self._refresh_curve(force=True)

    def _refresh_curve(self, force: bool = False):
        """Query VFP points (public GPC + private XBAR/HOST) then load+plot.

        ``force`` bypasses the 2.5 s back-to-back dedup gate — the auto-refresh
        timer uses it so a sub-2.5 s interval (the default is 1.0 s) keeps
        ticking instead of stalling on the gate and never rescheduling.
        """
        # Teardown guard: after cleanup() the matplotlib canvas/figure are
        # gone. An inflight worker may still complete and re-arm this chain
        # via after(0,_refresh_curve) — short-circuit instead of touching
        # freed resources or submitting new work to a shutting-down runner.
        if self._cleaned_up:
            self._refresh_curve_inflight = False
            self._refresh_curve_pending = False
            return
        if self._refresh_curve_inflight:
            self._refresh_curve_pending = True
            return
        # A just-loaded curve is still current: skip the duplicate query
        # when two refresh chains fire back-to-back (e.g. tab entry +
        # post-PPAB-enable refresh). Manual/auto timer passes force=True.
        if not force and _time.monotonic() - self._last_load_ts < 2.5:
            return

        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self.app.console.append("[GUI] No GPU selected.\n")
            return

        self._refresh_curve_inflight = True
        if not self._auto_refreshing:
            self.app.console.append("[GUI] Querying VF curve via pynvoc...\n")

        # Epoch guard: a newer refresh must not be overwritten by a stale one.
        epoch = self._curve_query_epoch + 1
        self._curve_query_epoch = epoch

        def _worker():
            gpc_points = None
            gpc_err = None
            clk_data = None
            try:
                gpc_points = self.app.backend.query_public_vftable(gpu)
            except Exception as exc:
                gpc_err = str(exc)
            try:
                clk_data = self.app.backend.query_private_vftable(gpu)
            except Exception:
                clk_data = None
            self.app.after(
                0,
                lambda: self._on_multi_query_done(
                    epoch, gpu, gpc_points, gpc_err, clk_data
                ),
            )

        self.app.run_background("vfcurve-refresh", _worker)

    def _on_multi_query_done(self, epoch, gpu, gpc_points, gpc_err, clk_data):
        self._refresh_curve_inflight = False
        # After cleanup() the figure/canvas are gone — drop the result and
        # do NOT re-arm the refresh chain, or a late worker would re-submit
        # into a shutting-down GuiTaskRunner (RuntimeError / join extension).
        if self._cleaned_up:
            self._refresh_curve_pending = False
            return
        # Stale worker (a newer refresh superseded this one): drop silently.
        if epoch != self._curve_query_epoch:
            return

        public_unsupported = gpc_err is not None and (
            "not supported" in gpc_err.lower() or "no implementation" in gpc_err.lower()
        )
        built = self._build_curves(
            gpu, gpc_points, gpc_err, public_unsupported, clk_data
        )
        if not built:
            if gpc_err and not public_unsupported:
                self.app.console.append(f"[GUI] VFP query failed: {gpc_err}\n")
            else:
                self.app.console.append("[GUI] VFP query failed.\n")
        else:
            self._last_load_ts = _time.monotonic()
            if not self._auto_refreshing:
                cur = self._curves.get(self._active_curve)
                n = len(cur.voltages) if cur else 0
                self.app.console.append(
                    f"[GUI] VF curve loaded ({n} points on {self._active_curve.upper()}).\n"
                )
            self._load_active_curve()
            # P0 voltage-boundary lines: hardware walls are queried once per
            # GPU (cache hit short-circuits here on subsequent refreshes), so
            # this never adds a per-second NVAPI read to the auto-refresh.
            self.ensure_p0_bounds(gpu)

        if self._refresh_curve_pending:
            self._refresh_curve_pending = False
            self.app.after(0, self._refresh_curve)
        elif self._auto_refreshing:
            self._schedule_next_auto_refresh()

    def _build_curves(
        self, gpu, gpc_points, gpc_err, public_unsupported, clk_data
    ) -> bool:
        """Populate ``self._curves`` from public + private reads.

        Returns False when no curve could be built at all. Per-curve
        ``write_mode`` is set so ``_apply_adj`` / reset know how to reach the
        GPU:

        * GPC via the open VFP interface and no Fixed points  → ``"public"``
        * any private segment, or GPC with Fixed points / public unsupported
          → ``"private"`` (apply dynamically tries mode-0 then falls back to
          raw-converted)

        Point-id ranges (seg_start/seg_end, bank) come straight from the
        private segment structure — never hardcoded.
        """
        curves: Dict[str, _CurveData] = {}
        prev_visible = self._curve_visible or {}

        # ── GPC: prefer the open interface; fall back to the private GPC
        # segment when the public one is explicitly unsupported. ──
        gpc_curve = None
        if gpc_points:
            gpc_curve = _CurveData("gpc")
            gpc_curve.source = "public"
            gpc_curve.voltages = [p["voltage_uv"] / 1000.0 for p in gpc_points]
            gpc_curve.frequencies = [p["frequency_khz"] / 1000.0 for p in gpc_points]
            gpc_curve.defaults = [
                (p.get("default_frequency_khz") or p["frequency_khz"]) / 1000.0
                for p in gpc_points
            ]
            gpc_curve.has_fixed = any(
                p.get("point_type") == "fixed" for p in gpc_points
            )
            gpc_curve.seg_start = 0
            gpc_curve.seg_end = len(gpc_points) - 1 if gpc_points else 0
            # Public family present: traditional OC unless a point is Fixed.
            gpc_curve.write_mode = "private" if gpc_curve.has_fixed else "public"
        elif public_unsupported:
            # Open family rejected — the private GPC segment (if any) is the
            # only GPC source. Located below from clk_data.
            pass

        # ── Private segments: GPC (fallback), XBAR, HOST. ──
        private_gpc: Optional[_CurveData] = None
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
                    if int(p.get("bank", 0)) == bank
                    and s <= int(p.get("index", -1)) <= e
                ]
                if not seg_pts:
                    continue
                cd = _CurveData(hint if hint in _CURVE_COLORS else "gpc")
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
                elif cd.curve_id in _CURVE_COLORS:
                    curves[cd.curve_id] = cd
                # unknown domains are skipped (only gpc/xbar/host displayed)

        # Resolve GPC source: public preferred, private fallback.
        if gpc_curve is not None:
            curves["gpc"] = gpc_curve
        elif private_gpc is not None:
            curves["gpc"] = private_gpc

        if not curves:
            self._curves = {}
            self._curve_visible = {}
            return False

        # Carry over visibility (default: every discovered curve visible), and
        # keep the active curve valid (fallback to first visible).
        self._curves = curves
        self._curve_visible = {cid: prev_visible.get(cid, True) for cid in curves}
        if (
            not self._curve_visible.get(self._active_curve)
            or self._active_curve not in curves
        ):
            self._active_curve = next(
                (cid for cid in curves if self._curve_visible.get(cid)), "gpc"
            )
        return True

    def _load_active_curve(self):
        """灌 active curve 数据进 _voltages/_frequencies/_defaults 并重绘。

        Keeps the legacy single-curve attributes as a live view of the active
        curve so all existing call sites (drag/keyboard/lock/dashboard/tests)
        keep working. Also rebuilds the per-curve selector row.
        """
        curve = self._curves.get(self._active_curve)
        if curve is None:
            return
        # Refresh only the active curve's data identity check (auto-refresh
        # short-circuit) before assigning.
        if (
            curve.voltages == self._voltages
            and curve.frequencies == self._frequencies
            and curve.defaults == self._defaults
            and not getattr(self, "_pending_lock_mv", None)
            and len(self._curves) <= 1
        ):
            # Single-curve no-op fast path (auto-refresh identity tick).
            pass
        self._voltages = curve.voltages
        self._frequencies = curve.frequencies
        self._defaults = curve.defaults
        self._drag_orig_freqs = None
        # Sync the curve's mutated frequencies back (same list refs, no-op).
        self._rebuild_selector()
        self._apply_curve_data(curve.voltages, curve.frequencies, curve.defaults)

    # ────────────────────────────────────────────
    # P0 voltage-boundary lines (deep red walls + light red effective)
    # ────────────────────────────────────────────
    def ensure_p0_bounds(self, gpu: str) -> None:
        """Query P0 voltage bounds once per GPU (hardware walls don't move).

        Called the first time the VF curve loads for a given GPU. The full
        ``p0`` dict is cached and the effective-wall line seeded; subsequent
        refreshes short-circuit on the gpu key (no per-second NVAPI read).
        The overclock panel's post-apply path calls
        :meth:`update_p0_effective_wall` to move just the light-red line.
        """
        if self._p0_bounds_gpu == gpu and self._p0_bounds is not None:
            return
        # GPU changed (or first load): clear any stale line so the old GPU's
        # effective wall isn't briefly shown over the new curve.
        if self._p0_bounds_gpu != gpu:
            self._p0_bounds = None
            self._p0_effective_wall_mv = None
            self._pending_wall_mv = None
        self._p0_bounds_gpu = gpu

        def _worker():
            try:
                vr = self.app.backend.query_volt_rails(gpu)
            except Exception:
                vr = None
            self.app.after(0, lambda: self._on_p0_bounds_loaded(gpu, vr))

        self.app.run_background("vfcurve-p0-bounds", _worker)

    @staticmethod
    def _resolve_rail_bit(volt_rail: dict) -> int:
        """Pick the VoltRails rail bit for set_volt_rail_target.

        Mirrors the overclock panel's ``_resolve_volt_rail_bit``: first rail
        descriptor's ``rail_bit`` when exposed, else the lowest set bit of
        ``rail_mask``. Single-rail mobile GPUs (4060 Laptop, mask 0x1) → 0.
        """
        descs = volt_rail.get("rail_descriptors")
        if isinstance(descs, list) and descs:
            first = descs[0]
            if isinstance(first, dict) and first.get("rail_bit") is not None:
                try:
                    return int(first["rail_bit"])
                except (TypeError, ValueError):
                    pass
        mask = volt_rail.get("rail_mask")
        if isinstance(mask, str) and mask:
            try:
                value = int(mask, 16)
                return (value & -value).bit_length() - 1
            except ValueError:
                pass
        return 0

    def _on_p0_bounds_loaded(self, gpu: str, vr) -> None:
        if self._cleaned_up:
            return
        # A newer GPU switch may have superseded this query — drop it.
        if self._p0_bounds_gpu != gpu:
            return
        if not isinstance(vr, dict):
            self._p0_bounds = None
            self._p0_effective_wall_mv = None
            self._redraw()
            return
        p0 = vr.get("p0") if isinstance(vr.get("p0"), dict) else None
        if not isinstance(p0, dict):
            self._p0_bounds = None
            self._p0_effective_wall_mv = None
            self._redraw()
            return
        self._p0_bounds = p0
        self._p0_rail_bit = self._resolve_rail_bit(vr)
        eff = int(p0.get("effective_wall_uV", 0) or 0)
        self._p0_effective_wall_mv = (eff / 1000.0) if eff > 0 else None
        self._redraw()

    def update_p0_effective_wall(self, effective_wall_uV: int) -> None:
        """Push a new effective wall (light-red line) after a Volt Limit apply.

        Hardware walls (floor/ceiling) are untouched — they don't move. Only
        the effective wall tracks a SET; the overclock panel calls this from
        its ``update_mobile_limits`` tail once the refreshed status lands.
        Any pending drag value is cleared — the applied value supersedes it.
        """
        if self._cleaned_up:
            return
        eff = int(effective_wall_uV or 0)
        mv = (eff / 1000.0) if eff > 0 else None
        if mv == self._p0_effective_wall_mv and self._pending_wall_mv is None:
            return
        self._p0_effective_wall_mv = mv
        self._pending_wall_mv = None
        self._redraw()

    def _switch_active_curve(self, curve_id: str):
        """Select which curve drag/keyboard/apply target — point on rect btn."""
        if curve_id not in self._curves or curve_id == self._active_curve:
            return
        if not self._curve_visible.get(curve_id):
            return  # activating a hidden curve makes no sense
        self._active_curve = curve_id
        curve = self._curves[curve_id]
        self._sel_start = None
        self._sel_end = None
        self._drag_orig_freqs = None
        self._voltages = curve.voltages
        self._frequencies = curve.frequencies
        self._defaults = curve.defaults
        self._rebuild_selector()
        self._redraw()
        # Live-point handoff: when leaving GPC the dashboard-fed (volt,freq)
        # is stale; clear it and let the direct-read path repopulate for
        # xbar/host. When switching back to GPC, clear so the dashboard poll's
        # next tick refills it (don't show an xbar freq on the GPC curve).
        self._live_pending = (None, None)
        self._live_volt = None
        self._live_freq = None
        self._hide_live_point()
        if curve_id in ("xbar", "sys") and not self._direct_read_inflight:
            self._kick_direct_read(curve_id)
        self.app.console.append(
            f"[GUI] Active curve: {curve_id.upper()} "
            f"({curve.source}, {curve.write_mode}).\n"
        )

    def _toggle_curve_visible(self, curve_id: str):
        """Toggle a curve's visibility (checkbox). Hidden curves are not drawn
        and not queried on refresh. Always keeps at least the active curve
        visible."""
        if curve_id not in self._curves:
            return
        if self._curve_visible.get(curve_id) and curve_id == self._active_curve:
            # Don't allow hiding the only visible / active curve.
            visible_count = sum(1 for v in self._curve_visible.values() if v)
            if visible_count <= 1:
                return
        self._curve_visible[curve_id] = not self._curve_visible.get(curve_id, True)
        # If the hidden curve was feeding the live point (GPC via dashboard, or
        # xbar/host via direct read), clear the crosshair so a hidden curve is
        # never polled/drawn — including GPC.
        if not self._curve_visible.get(curve_id):
            if curve_id == "gpc":
                # Stop accepting the dashboard feed until GPC is active again.
                self._live_pending = (None, None)
            if curve_id == self._active_curve:
                self._live_volt = None
                self._live_freq = None
                self._pending_live_point = None
        if not self._curve_visible.get(self._active_curve):
            self._active_curve = next(
                (c for c, v in self._curve_visible.items() if v), self._active_curve
            )
            curve = self._curves.get(self._active_curve)
            if curve is not None:
                self._voltages = curve.voltages
                self._frequencies = curve.frequencies
                self._defaults = curve.defaults
                self._sel_start = None
                self._sel_end = None
                self._drag_orig_freqs = None
            # New active curve's live point starts fresh; kick a direct read
            # immediately if it's xbar/host (GPC will be refed by dashboard).
            self._live_pending = (None, None)
            self._live_volt = None
            self._live_freq = None
            self._hide_live_point()
            if (
                self._active_curve in ("xbar", "sys")
                and not self._direct_read_inflight
            ):
                self._kick_direct_read(self._active_curve)
        self._rebuild_selector()
        self._redraw()

    def _rebuild_selector(self):
        """Rebuild the per-curve selector row (rect buttons + checkboxes).

        Only shown when more than one curve was discovered. Each curve gets a
        rectangular button: clicking the box toggles visibility, clicking the
        button body activates that curve for drag/keyboard/apply.
        """
        if getattr(self, "_chart_frame", None) is None or self._cleaned_up:
            return
        host = getattr(self, "_curve_selector_host", None)
        if host is None:
            return  # UI not built yet (selector host created in _build_chart)
        for child in host.winfo_children():
            child.destroy()
        self._curve_selector_btns = {}
        curves = list(self._curves.items())
        if len(curves) <= 1:
            host.pack_forget()
            return
        host.pack(fill="x", padx=10, pady=(2, 6), after=self._chart_area)
        for cid, curve in curves:
            btn = tk.Frame(
                host,
                bg=_PANEL_BG,
                highlightthickness=2,
                highlightcolor=_CURVE_COLORS[cid]["current"],
                highlightbackground="#444444",
                bd=0,
            )
            btn.pack(side="left", padx=6)
            # Checkbox (left): toggles visibility. Stops propagation so the
            # outer button's <Button-1> (activate) doesn't also fire.
            var = tk.BooleanVar(value=self._curve_visible.get(cid, True))
            ckb = ctk.CTkCheckBox(
                btn,
                text="",
                variable=var,
                width=20,
                height=20,
                command=lambda c=cid, v=var: self._on_selector_checkbox(c, v),
            )
            ckb.pack(side="left", padx=(4, 2), pady=4)
            # Label (right): curve name + active indicator.
            colors = _CURVE_COLORS[cid]
            active = cid == self._active_curve
            label_text = _CURVE_META[cid]["label"]
            lbl = tk.Label(
                btn,
                text=label_text,
                font=_FONT_BODY,
                bg=_PANEL_BG,
                fg=colors["current"],
            )
            lbl.pack(side="left", padx=(0, 6))
            if active:
                lbl.config(font=("Segoe UI", 11, "bold"))
                btn.config(highlightbackground=colors["current"])
            # Click anywhere on the button body (not the checkbox) activates.
            btn.bind("<Button-1>", lambda e, c=cid: self._switch_active_curve(c))
            lbl.bind("<Button-1>", lambda e, c=cid: self._switch_active_curve(c))
            self._curve_selector_btns[cid] = btn

    def _on_selector_checkbox(self, curve_id: str, var: tk.BooleanVar):
        # The checkbox already toggled `var`. Route through the canonical
        # toggle path so the "can't hide the only visible curve" guard and
        # live-point clearing (no crosshair for hidden curves, incl. GPC) are
        # applied consistently. If the guard vetoes, restore the checkbox.
        before = self._curve_visible.get(curve_id, True)
        self._toggle_curve_visible(curve_id)
        if self._curve_visible.get(curve_id, True) == before:
            var.set(before)  # vetoed (only-visible guard) — snap checkbox back

    @staticmethod
    def _write_vfp_points(path: str, points: List[dict]) -> None:
        write_vfp_points(path, points)

    @staticmethod
    def _load_vfp_deltas(
        path: str, reference_points: List[dict]
    ) -> List[Tuple[int, int]]:
        return load_vfp_deltas(path, reference_points)

    def _load_points(self, points: List[dict]):
        """Load VFP points (pynvoc dicts, µV/kHz) and redraw the chart.

        Legacy single-curve entry — used by tests and any caller that hands a
        flat public point list. Rebuilds a GPC-only curve set.
        """
        gpc = _CurveData("gpc")
        gpc.source = "public"
        for p in points:
            gpc.voltages.append(p["voltage_uv"] / 1000.0)
            freq_mhz = p["frequency_khz"] / 1000.0
            gpc.frequencies.append(freq_mhz)
            default_khz = p.get("default_frequency_khz")
            gpc.defaults.append(
                freq_mhz if default_khz is None else default_khz / 1000.0
            )
            if p.get("point_type") == "fixed":
                gpc.has_fixed = True
        gpc.write_mode = "private" if gpc.has_fixed else "public"
        gpc.seg_end = len(points) - 1 if points else 0
        self._curves = {"gpc": gpc}
        self._curve_visible = {"gpc": True}
        self._active_curve = "gpc"
        self._voltages = gpc.voltages
        self._frequencies = gpc.frequencies
        self._defaults = gpc.defaults
        self._apply_curve_data(gpc.voltages, gpc.frequencies, gpc.defaults)

    def _load_csv(self, path: str):
        """Parse CSV (Import button) and redraw chart."""
        if not os.path.isfile(path):
            self.app.console.append(f"[GUI] CSV not found: {path}\n")
            return

        voltages = []
        frequencies = []
        defaults = []

        try:
            with open(path, newline="", encoding="utf-8-sig") as f:
                reader = csv.reader(f)
                for row in reader:
                    if not row or row[0].startswith("#"):
                        continue
                    # Detect header row
                    if row[0].strip().lower() == "voltage":
                        continue
                    try:
                        v = float(row[0])  # µV
                        freq = float(row[1])  # kHz
                        # delta in row[2]
                        default = float(row[3]) if len(row) > 3 else freq
                    except (ValueError, IndexError):
                        continue
                    # Convert: CSV values are in µV / kHz → display as mV / MHz
                    voltages.append(v / 1000.0)  # µV → mV
                    frequencies.append(freq / 1000.0)  # kHz → MHz
                    defaults.append(default / 1000.0)
        except Exception as e:
            self.app.console.append(f"[GUI] Error reading CSV: {e}\n")
            return

        if (
            voltages == self._voltages
            and frequencies == self._frequencies
            and defaults == self._defaults
            and getattr(self, "_pending_lock_mv", None) is None
        ):
            return

        self._apply_curve_data(voltages, frequencies, defaults)

    def _apply_curve_data(
        self,
        voltages: List[float],
        frequencies: List[float],
        defaults: List[float],
    ):
        """Store curve data (mV/MHz), resolve pending lock, and redraw."""
        previous_selection = (
            (self._sel_start, self._sel_end)
            if self._sel_start is not None and self._sel_end is not None
            else None
        )

        self._voltages = voltages
        self._frequencies = frequencies
        self._defaults = defaults
        self._sel_start = None
        self._sel_end = None
        self._drag_orig_freqs = None
        if previous_selection is not None and voltages:
            max_idx = len(voltages) - 1
            start, end = previous_selection
            self._sel_start = max(0, min(start, max_idx))
            self._sel_end = max(0, min(end, max_idx))
            self._sync_selection_to_adj()

        # Apply any pending lock set before data was loaded
        pending_mv = getattr(self, "_pending_lock_mv", None)
        if pending_mv is not None:
            self._pending_lock_mv = None
            idx = self._find_closest_voltage_idx(pending_mv)
            if idx is not None:
                self._locked_points.clear()
                self._locked_points.add(idx)
                self.app.console.append(
                    f"[GUI] Lock synced → point {idx} ({self._voltages[idx]:.1f} mV).\n"
                )

        # Check whether VF offsets are present and whether all points share one uniform offset.
        has_vfp_offset, uniform_core_offset_mhz = analyze_vfp_offsets(
            frequencies, defaults
        )
        apply_vfp_state = getattr(self.app, "_apply_vfp_offset_state", None)
        if callable(apply_vfp_state):
            apply_vfp_state(has_vfp_offset, uniform_core_offset_mhz)
        elif getattr(self.app, "tab_overclock", None):
            self.app.tab_overclock.set_vfp_state(
                has_vfp_offset, uniform_core_offset_mhz
            )

        self.app.console.append(f"[GUI] Loaded {len(voltages)} VF points.\n")
        self._redraw()

    def _redraw(self):
        """Redraw the chart with current data."""
        if self._is_resize_active:
            self._pending_full_redraw = True
            return
        if getattr(self, "ax", None) is None:
            # Chart not built yet — retry once it is (build flushes pending).
            self._pending_full_redraw = True
            return

        ax = self.ax
        ax.clear()
        self._style_axes()

        if not self._voltages:
            ax.text(
                0.5,
                0.5,
                "No data",
                transform=ax.transAxes,
                ha="center",
                va="center",
                color="#888888",
                fontsize=12,
            )
            self.canvas.draw_idle()
            return

        v = self._voltages
        f = self._frequencies
        d = self._defaults

        # ── Draw every visible curve ──
        # Non-active curves are static (baked into the cached background);
        # the active curve's current line is animated (animated=True) so the
        # drag/keyboard/wheel fast-edit path can blit just it.
        self._curve_lines = {}
        # ax.clear() above destroyed the static artists/patches; drop the
        # stale references so the _draw_*_handle calls below recreate them.
        self._pending_wall_line = None
        self._wall_handle = None
        active_colors = _CURVE_COLORS.get(self._active_curve, _CURVE_COLORS["gpc"])
        active_label = _CURVE_META.get(self._active_curve, _CURVE_META["gpc"])["label"]

        # Non-active visible curves first (lower zorder, static).
        for cid, curve in self._curves.items():
            if cid == self._active_curve:
                continue
            if not self._curve_visible.get(cid):
                continue
            colors = _CURVE_COLORS.get(cid, _CURVE_COLORS["gpc"])
            lbl = _CURVE_META.get(cid, {"label": cid.upper()})["label"]
            ax.plot(
                curve.voltages,
                curve.defaults,
                color=colors["default"],
                linestyle="--",
                linewidth=0.9,
                label=f"{lbl} Default",
                zorder=2,
            )
            (line_cur,) = ax.plot(
                curve.voltages,
                curve.frequencies,
                color=colors["current"],
                linestyle="-",
                linewidth=1.1,
                marker="s",
                markersize=0.9,
                markerfacecolor=colors["current"],
                markeredgecolor=colors["current"],
                label=f"{lbl} Current",
                zorder=3,
            )
            self._curve_lines[cid] = {"current": line_cur, "default": None}

        # Active curve (default dashed + current solid+marker, animated).
        (self._line_default,) = ax.plot(
            v,
            d,
            color=active_colors["default"],
            linestyle="--",
            linewidth=0.9,
            label=f"{active_label} Default",
            zorder=2,
        )
        (self._line_current,) = ax.plot(
            v,
            f,
            color=active_colors["current"],
            linestyle="-",
            linewidth=1.1,
            marker="s",
            markersize=0.9,
            markerfacecolor=active_colors["current"],
            markeredgecolor=active_colors["current"],
            label=f"{active_label} Current",
            zorder=4,
            animated=True,
        )
        self._curve_lines[self._active_curve] = {
            "current": self._line_current,
            "default": self._line_default,
        }

        # Selection highlight
        if self._sel_start is not None and self._sel_end is not None:
            s = min(self._sel_start, self._sel_end)
            e = max(self._sel_start, self._sel_end)
            sel_v = v[s : e + 1]
            sel_f = f[s : e + 1]

            # Shaded region — persistent animated artist so selection-range
            # drags can update just the span (voltage-axis only, independent
            # of the moving curve points) via _update_selection_span without
            # a full _redraw. Re-created here on each full redraw.
            self._sel_rect = ax.axvspan(
                v[s], v[e], alpha=0.15, color="#ffcc00", zorder=1, animated=True
            )

            # Highlighted points
            self._sel_points = ax.scatter(
                sel_v,
                sel_f,
                color="#ffcc00",
                s=14,
                zorder=5,
                edgecolors="#ff8800",
                linewidths=0.6,
                animated=True,
            )

            # ── Info popup (right side of axes) ──
            if s == e:
                # Single point: show V / default F / current F / ΔF vs default
                cur_f = f[s]
                ref_f = d[s]
                delta = cur_f - ref_f
                sign = "+" if delta >= 0 else ""
                info = (
                    f"  idx : {s}\n"
                    f"  V   : {v[s]:.1f} mV\n"
                    f"  F   : {cur_f:.1f} MHz\n"
                    f"  dF  : {ref_f:.1f} MHz (default)\n"
                    f"  ΔF  : {sign}{delta:.1f} MHz  "
                )
            else:
                # Range: show idx range / V range / ΔF avg vs default
                deltas = [f[i] - d[i] for i in range(s, e + 1)]
                avg_delta = sum(deltas) / len(deltas)
                sign = "+" if avg_delta >= 0 else ""
                info = (
                    f"  idx : {s} – {e}  ({e - s + 1} pts)\n"
                    f"  V   : {v[s]:.1f} ~ {v[e]:.1f} mV\n"
                    f"  ΔF  : {sign}{avg_delta:.1f} MHz (avg vs default)  "
                )

            ax.text(
                0.99,
                0.03,
                info,
                transform=ax.transAxes,
                ha="right",
                va="bottom",
                fontsize=6.5,
                fontfamily="monospace",
                color="#ffe066",
                zorder=10,
                bbox=dict(
                    boxstyle="round,pad=0.4",
                    facecolor="#1a1a1a",
                    edgecolor="#ffcc00",
                    alpha=0.88,
                    linewidth=0.8,
                ),
            )
        else:
            self._sel_points = None

        ax.legend(
            loc="upper left",
            fontsize=6,
            framealpha=0.5,
            facecolor="#2b2b2b",
            edgecolor="#555555",
            labelcolor="#cccccc",
        )

        # Draw crosshairs for locked points
        for idx in self._locked_points:
            if 0 <= idx < len(v):
                lv = v[idx]
                lf = f[idx]
                crosshair_kw = dict(
                    color="#ff4444", linewidth=1.0, linestyle="--", alpha=0.85
                )
                ax.axvline(x=lv, zorder=6.5, **crosshair_kw)
                ax.axhline(y=lf, zorder=5.5, **crosshair_kw)
                # Center marker
                ax.plot(
                    lv,
                    lf,
                    marker="+",
                    markersize=8,
                    color="#ff4444",
                    markeredgewidth=1.2,
                    zorder=7.5,
                    linestyle="none",
                )
                # Label - Using ASCII [L] instead of emoji to avoid UserWarning/Glyph missing issues
                ax.annotate(
                    f"Locked: {idx}",
                    xy=(lv, lf),
                    xytext=(6, 6),
                    textcoords="offset points",
                    color="#ff8888",
                    fontsize=5,
                    zorder=8,
                )

        # Axis range with some padding — span every visible curve so all fit.
        v_min, v_max = min(v), max(v)
        f_vals = list(f) + list(d)
        for cid, curve in self._curves.items():
            if cid == self._active_curve or not self._curve_visible.get(cid):
                continue
            if curve.voltages:
                v_min = min(v_min, min(curve.voltages))
                v_max = max(v_max, max(curve.voltages))
            f_vals += list(curve.frequencies) + list(curve.defaults)
        f_min, f_max = min(f_vals), max(f_vals) if f_vals else (0, 1)

        # Adjust Y limits if freq lock is outside default range
        if self._freq_core_lock is not None:
            cmin, cmax = self._freq_core_lock
            f_min = min(f_min, cmin)
            f_max = max(f_max, cmax)

        f_pad = max(150, (f_max - f_min) * 0.18)
        ax.set_xlim(v_min - 1, v_max + 1)
        ax.set_ylim(f_min - f_pad, f_max + f_pad)
        # the LAST VISIBLE voltage tick number gives its slot to the unit
        # caption — the raw tick list can end beyond xlim (e.g. a 1.3 tick
        # past a 1.24 V max), so pick the last tick inside the view. Only
        # stash the tick VALUE here: the formatter (see _style_axes) turns
        # it into "Volt/V" at draw time; set_text would be overwritten.
        self._volt_unit_tick = None
        xlim = ax.get_xlim()
        for tick in reversed(ax.get_xticks()):
            if xlim[0] <= tick <= xlim[1]:
                self._volt_unit_tick = tick
                for label in ax.get_xticklabels():
                    if label.get_position()[0] == tick:
                        # match the f/GHz caption: orange, one size up from
                        # the plain tick digits (survives formatter redraws
                        # — only the TEXT is regenerated, not style)
                        label.set_color("#e08020")
                        label.set_fontsize(7)
                break

        # Draw frequency lock visualization (after limits are known)
        if self._freq_core_lock is not None:
            cmin, cmax = self._freq_core_lock
            if cmin == cmax:
                ax.axhline(
                    y=cmin,
                    color="#ffff00",
                    linewidth=1.5,
                    linestyle="-",
                    alpha=0.6,
                    zorder=4.5,
                )
                ax.text(
                    v_min,
                    cmin + (f_max - f_min) * 0.015,
                    f" Freq Lock: {cmin} MHz",
                    color="#ffff00",
                    fontsize=7,
                    alpha=0.8,
                    zorder=5,
                )
            else:
                ax.axhspan(cmin, cmax, color="#ffff00", alpha=0.15, zorder=1.5)
                ax.axhline(
                    y=cmin,
                    color="#ffff00",
                    linewidth=1.0,
                    linestyle="--",
                    alpha=0.5,
                    zorder=4.5,
                )
                ax.axhline(
                    y=cmax,
                    color="#ffff00",
                    linewidth=1.0,
                    linestyle="--",
                    alpha=0.5,
                    zorder=4.5,
                )
                ax.text(
                    v_min,
                    cmax + (f_max - f_min) * 0.015,
                    f" Freq Lock: {cmin}-{cmax} MHz",
                    color="#ffff00",
                    fontsize=7,
                    alpha=0.8,
                    zorder=5,
                )

        # ── P0 voltage-boundary vertical lines ──
        # Deep red (solid): hardware floor (min_hold) + ceiling
        # min(vbios_wall, vrm_max_wall) — immutable per-GPU, cached once.
        # Light red (solid): effective wall — live, pushed by the overclock
        # panel after a Volt Limit apply. All three fall inside the curve's
        # voltage range by design, so no axis adjustment is needed. Labels
        # are rotated 90° to survive narrow charts; zorder sits above the
        # curves (2-4) but below the locked crosshair (5.5-7.5).
        p0 = self._p0_bounds
        if isinstance(p0, dict):
            floor_uv = int(p0.get("min_hold_uV", 0) or 0)
            if floor_uv > 0:
                floor_mv = floor_uv / 1000.0
                ax.axvline(
                    x=floor_mv,
                    color="#8b0000",
                    linewidth=1.2,
                    linestyle="-",
                    alpha=0.8,
                    zorder=4.2,
                )
                ax.text(
                    floor_mv,
                    f_max - (f_max - f_min) * 0.02,
                    " P0 floor",
                    color="#d96666",
                    fontsize=6,
                    alpha=0.85,
                    ha="left",
                    va="top",
                    zorder=5,
                    rotation=90,
                )
            vbios_uv = int(p0.get("vbios_wall_uV", 0) or 0)
            vrm_uv = int(p0.get("vrm_max_wall_uV", 0) or 0)
            walls = [w for w in (vbios_uv, vrm_uv) if w > 0]
            if walls:
                ceil_mv = min(walls) / 1000.0
                ax.axvline(
                    x=ceil_mv,
                    color="#8b0000",
                    linewidth=1.2,
                    linestyle="-",
                    alpha=0.8,
                    zorder=4.2,
                )
                ax.text(
                    ceil_mv,
                    f_max - (f_max - f_min) * 0.02,
                    " P0 ceiling ",
                    color="#d96666",
                    fontsize=6,
                    alpha=0.85,
                    ha="right",
                    va="top",
                    zorder=5,
                    rotation=90,
                )
        if self._p0_effective_wall_mv is not None:
            eff_mv = self._p0_effective_wall_mv
            ax.axvline(
                x=eff_mv,
                color="#ff6b6b",
                linewidth=1.0,
                linestyle="-",
                alpha=0.75,
                zorder=4.1,
            )
            ax.text(
                eff_mv,
                f_min + (f_max - f_min) * 0.02,
                " P0 eff volt lim ",
                color="#ff9999",
                fontsize=6,
                alpha=0.8,
                ha="right",
                va="bottom",
                zorder=5,
                rotation=90,
            )

        # Keep margins in sync with _create_chart (see the note there) —
        # re-applying the old 0.13 here would re-create the left gutter
        self.fig.subplots_adjust(left=0.062, right=0.995, top=0.92, bottom=0.12)

        self._live_elements.clear()
        self._live_hline = None
        self._live_vline = None
        self._live_marker = None
        self._live_text = None
        self._draw_live_point(call_draw_idle=False)
        self._draw_pending_wall(call_draw_idle=False)
        self._draw_wall_handle(call_draw_idle=False)
        self.canvas.draw_idle()

    def _draw_pending_wall(self, call_draw_idle: bool = True):
        """Draw (or hide) the pending wall — the dashed light-red vline a
        drag moves before "Apply to GPU" writes it. It lives in the blit
        overlay (animated) so a drag can move it without a full _redraw,
        mirroring the live-point crosshair pattern.
        """
        if self._pending_wall_mv is None:
            self._hide_pending_wall()
            if call_draw_idle:
                self._blit_animated()
            return
        mv = self._pending_wall_mv
        if self._pending_wall_line is not None:
            self._pending_wall_line.set_xdata([mv, mv])
            self._pending_wall_line.set_visible(True)
        else:
            self._pending_wall_line = self.ax.axvline(
                x=mv,
                color="#ff6b6b",
                linewidth=1.2,
                linestyle="--",
                alpha=0.85,
                zorder=4.15,
                animated=True,
            )
        if call_draw_idle:
            self._blit_animated()

    def _hide_pending_wall(self) -> None:
        if self._pending_wall_line is not None:
            try:
                self._pending_wall_line.set_visible(False)
            except Exception:
                # Artist may be stale/removed during redraw churn.
                pass

    def _draw_wall_handle(self, call_draw_idle: bool = True):
        """Position the wall-drag triangle in the top figure margin.

        A downward-pointing triangle sits just above the axes top spine, its
        tip at the wall's x. Clicks on it land outside the axes (inaxes=None),
        so they never enter the point-select logic — the triangle is the sole
        drag affordance for the light-red wall. Tracks ``_pending_wall_mv``
        when a drag is in progress, else the applied effective wall.

        NOT animated: it lives in the figure margin above the axes, outside
        ``ax.bbox``, so the blit overlay (which only repaints ax.bbox) cannot
        reach it. It is a static artist repainted on each full ``_redraw``,
        which is fine — it only moves when the wall moves, and a wall move
        already triggers a redraw.
        """
        mv = self._pending_wall_mv
        if mv is None:
            mv = self._p0_effective_wall_mv
        if mv is None or self.ax is None or self.fig is None:
            self._hide_wall_handle()
            if call_draw_idle:
                self._blit_animated()
            return
        import matplotlib.patches as mpatches
        from matplotlib.transforms import blended_transform_factory

        trans = blended_transform_factory(self.ax.transData, self.fig.transFigure)
        # Half-width in DATA mV so the triangle keeps a stable visual width
        # as the x-axis zooms (~6‰ of the curve's voltage span, ≥4 mV).
        if self._voltages and len(self._voltages) >= 2:
            span = self._voltages[-1] - self._voltages[0]
            hw = max(4.0, span * 0.006)
        else:
            hw = 8.0
        # Tip down at the axes top (fig fraction 0.925); base at the figure
        # top margin (0.995). The triangle straddles the axes top spine.
        verts = [(mv, 0.925), (mv - hw, 0.995), (mv + hw, 0.995)]
        if self._wall_handle is not None:
            self._wall_handle.set_xy(verts)
            self._wall_handle.set_visible(True)
        else:
            self._wall_handle = mpatches.Polygon(
                verts,
                closed=True,
                transform=trans,
                facecolor="#ff6b6b",
                edgecolor="white",
                linewidth=0.8,
                alpha=0.9,
                zorder=15,
            )
            # The triangle lives in the figure margin ABOVE the axes bbox;
            # the default axes clip would erase it. Disable clipping so it
            # renders outside the axes.
            self._wall_handle.set_clip_on(False)
            self.ax.add_patch(self._wall_handle)
        # Static artist: a full _redraw already calls canvas.draw_idle() which
        # repaints it into the figure buffer. No blit needed here.
        if call_draw_idle:
            self.canvas.draw_idle()

    def _hide_wall_handle(self) -> None:
        if self._wall_handle is not None:
            try:
                self._wall_handle.set_visible(False)
            except Exception:
                # Artist may be stale/removed during redraw churn.
                pass

    # ────────────────────────────────────────────
    # Mouse interaction
    # ────────────────────────────────────────────
    def _find_nearest_index(self, x_data: float) -> Optional[int]:
        """Find the index of the VF point closest to x_data (mV)."""
        if not self._voltages:
            return None
        arr = self._np().array(self._voltages)
        idx = int(self._np().argmin(self._np().abs(arr - x_data)))
        return idx

    def _hit_wall_handle(self, event) -> bool:
        """True when a click lands on the wall-drag triangle (the handle in
        the top figure margin). The handle lives outside the axes, so its
        clicks have ``inaxes=None`` and never reach the point-select logic —
        this is the sole entry point for a wall drag. Uses the handle's
        display bbox, so the hit area tracks the triangle exactly.
        """
        if self._wall_handle is None or event.x is None or event.y is None:
            return False
        try:
            bbox = self._wall_handle.get_window_extent()
        except Exception:
            return False
        if bbox is None or bbox.width <= 0 or bbox.height <= 0:
            return False
        return bbox.x0 <= event.x <= bbox.x1 and bbox.y0 <= event.y <= bbox.y1

    def _wall_clamp_bounds(self) -> Tuple[Optional[float], Optional[float]]:
        """(lo_mV, hi_mV) the pending wall drag is clamped to.

        The effective wall may be dragged BELOW the P0 min_hold floor (the
        driver allows undervolting below it), so the lower bound is the plot's
        left x-edge (or a hard 450 mV floor as a safety backstop), NOT the P0
        floor. The upper bound stays the hardware ceiling
        ``min(vbios_wall, vrm_max_wall)`` — the driver would clamp a SET
        above it anyway. Returns (None, None) when no p0 bounds are cached
        (best-effort unclamped; the driver clamps on SET).
        """
        p0 = self._p0_bounds
        if not isinstance(p0, dict):
            return None, None
        # Lower bound: the plot's left x-edge, but never below 450 mV.
        lo = 450.0
        try:
            xlim_lo = self.ax.get_xlim()[0]
            if xlim_lo is not None:
                lo = max(450.0, float(xlim_lo))
        except Exception:
            pass
        vbios_uv = int(p0.get("vbios_wall_uV", 0) or 0)
        vrm_uv = int(p0.get("vrm_max_wall_uV", 0) or 0)
        walls = [w for w in (vbios_uv, vrm_uv) if w > 0]
        hi = (min(walls) / 1000.0) if walls else None
        return lo, hi

    def _on_mouse_press(self, event):
        # Wall-drag handle lives ABOVE the axes (in the top figure margin),
        # so a click on it has inaxes=None. Handle it before the inaxes guard
        # below so the point-select logic is never entered for a handle grab.
        if event.button == 1 and self._hit_wall_handle(event):
            self._mouse_pressed = True
            self._dragging_wall = True
            wall_mv = self._pending_wall_mv
            if wall_mv is None:
                wall_mv = self._p0_effective_wall_mv
            if wall_mv is not None:
                self._pending_wall_mv = wall_mv
                self._draw_pending_wall()
            return

        if event.inaxes != self.ax or not self._voltages:
            return

        if event.button == 1:  # Left click = start selection / drag
            self._mouse_pressed = True
            idx = self._find_nearest_index(event.xdata)
            if idx is None:
                return

            # If click inside existing selection → start drag
            if self._sel_start is not None and self._sel_end is not None:
                s = min(self._sel_start, self._sel_end)
                e = max(self._sel_start, self._sel_end)
                if s <= idx <= e:
                    self._dragging = True
                    self._drag_start_y = event.ydata
                    self._drag_orig_freqs = self._np().array(
                        self._frequencies, dtype=float
                    )
                    return

            # Otherwise start new selection
            self._sel_start = idx
            self._sel_end = idx
            self._dragging = False
            self._redraw()

        elif event.button == 3:  # Right click = clear selection
            self._clear_selection()

    def _on_mouse_release(self, event):
        if event.button == 1:
            if self._mouse_pressed:
                self._mouse_pressed = False
                # Apply any live-point update deferred during the interaction
                # (the dashboard poll's crosshair blit would otherwise contend
                # with the drag's blit over the cached background).
                self._flush_pending_live_point()
            if self._dragging_wall:
                self._dragging_wall = False
                # Keep _pending_wall_mv — it stays as a dashed line until the
                # user clicks "Apply to GPU", which writes it via
                # set_volt_rail_target and clears the pending state.
                if self._pending_wall_mv is not None:
                    self.app.console.append(
                        f"[GUI] P0 wall pending: {self._pending_wall_mv:.1f} mV "
                        f"(press Apply to GPU to write).\n"
                    )
                return
            if self._dragging:
                self._dragging = False
                self._drag_start_y = None
                # Keep drag_orig_freqs for undo
                self._redraw()
                self._sync_selection_to_adj()
                return

            if event.inaxes != self.ax or not self._voltages:
                return

            idx = self._find_nearest_index(event.xdata)
            if idx is not None and self._sel_start is not None:
                self._sel_end = idx
                self._redraw()
                self._sync_selection_to_adj()

    def _on_mouse_move(self, event):
        if not self._voltages:
            return

        if self._dragging_wall and event.x is not None:
            # The drag handle is above the axes (inaxes=None during the drag),
            # so recover the data-x from the display pixel rather than using
            # event.xdata (which is None outside the axes).
            try:
                mv = float(self.ax.transData.inverted().transform((event.x, 0.0))[0])
            except Exception:
                return
            lo, hi = self._wall_clamp_bounds()
            if lo is not None:
                mv = max(lo, min(hi, mv))
            if mv != self._pending_wall_mv:
                self._pending_wall_mv = mv
                self._draw_pending_wall()
                self._draw_wall_handle()
            return

        if (
            self._dragging
            and event.inaxes == self.ax
            and self._drag_start_y is not None
        ):
            # Drag selected points up/down
            dy = event.ydata - self._drag_start_y  # MHz
            s = min(self._sel_start, self._sel_end)
            e = max(self._sel_start, self._sel_end)

            for i in range(s, e + 1):
                self._frequencies[i] = float(self._drag_orig_freqs[i]) + dy

            # Update only the line data for performance
            if self._line_current is not None:
                self._line_current.set_ydata(self._frequencies)
            if self._sel_points is not None:
                sel_f = self._frequencies[s : e + 1]
                offsets = self._np().column_stack([self._voltages[s : e + 1], sel_f])
                self._sel_points.set_offsets(offsets)
            self._blit_animated()
            return

        # Selection drag (extending selection while mouse button held). The
        # range is voltage-axis only, so update just the span overlay + blit —
        # no full _redraw, keeping the selection smooth and immune to the
        # per-second live-point blit. Point markers update on release.
        if (
            event.button == 1
            and event.inaxes == self.ax
            and self._sel_start is not None
            and not self._dragging
        ):
            idx = self._find_nearest_index(event.xdata)
            if idx is not None and idx != self._sel_end:
                self._sel_end = idx
                self._update_selection_span()

    def sync_lock_from_voltage(self, voltage_mv: Optional[float]):
        """Called at startup: sync VFP lock state from CLI into _locked_points.

        Args:
            voltage_mv: locked voltage in mV, or None if not locked.
        """
        self._locked_points.clear()
        self._pending_lock_mv: Optional[float] = None

        if voltage_mv is None:
            return

        if self._voltages:
            # Data already loaded — find closest point immediately
            idx = self._find_closest_voltage_idx(voltage_mv)
            if idx is not None:
                self._locked_points.add(idx)
                self.app.console.append(
                    f"[GUI] Lock synced → point {idx} ({self._voltages[idx]:.1f} mV).\n"
                )
                self._redraw()
        else:
            # Data not yet loaded — store pending voltage, applied in _load_csv
            self._pending_lock_mv = voltage_mv

    def sync_freq_locks_from_cache(self, limits: Optional[dict]):
        """Sync frequency core/memory lock values from the app cache into UI controls."""
        limits = limits or {}

        def _to_int(value: object) -> Optional[int]:
            try:
                return int(str(value).strip())
            except (TypeError, ValueError):
                return None

        core_min = _to_int(limits.get("vfp_lock_gpu_core_lowerbound_mhz"))
        core_max = _to_int(limits.get("vfp_lock_gpu_core_upperbound_mhz"))
        mem_min = _to_int(limits.get("vfp_lock_memory_lowerbound_mhz"))
        mem_max = _to_int(limits.get("vfp_lock_memory_upperbound_mhz"))

        if core_min is not None and core_max is not None and core_min > core_max:
            core_min, core_max = core_max, core_min
        if mem_min is not None and mem_max is not None and mem_min > mem_max:
            mem_min, mem_max = mem_max, mem_min

        new_core_lock = (
            (core_min, core_max)
            if core_min is not None and core_max is not None
            else None
        )
        new_mem_lock = (
            (mem_min, mem_max) if mem_min is not None and mem_max is not None else None
        )
        new_core_backend = "nvapi" if new_core_lock is not None else None
        new_mem_backend = "nvapi" if new_mem_lock is not None else None
        changed = (
            new_core_lock != self._freq_core_lock
            or new_core_backend != self._freq_core_lock_backend
            or new_mem_lock != self._freq_mem_lock
            or new_mem_backend != self._freq_mem_lock_backend
        )

        self._freq_core_lock = new_core_lock
        self._freq_mem_lock = new_mem_lock
        self._freq_core_lock_backend = new_core_backend
        self._freq_mem_lock_backend = new_mem_backend
        self.app._dashboard_gpu_lock_active = new_core_lock is not None
        self.app._dashboard_mem_lock_active = new_mem_lock is not None
        self.core_lock_min_var.set(str(core_min if core_min is not None else 0))
        self.core_lock_max_var.set(str(core_max if core_max is not None else 0))
        self.mem_lock_min_var.set(str(mem_min if mem_min is not None else 0))
        self.mem_lock_max_var.set(str(mem_max if mem_max is not None else 0))

        if changed and hasattr(self, "ax") and hasattr(self, "canvas"):
            self._redraw()

    def _find_closest_voltage_idx(self, voltage_mv: float) -> Optional[int]:
        """Return the index of the VF point closest to the given voltage (mV)."""
        if not self._voltages:
            return None
        best_idx = 0
        best_dist = abs(self._voltages[0] - voltage_mv)
        for i, v in enumerate(self._voltages):
            d = abs(v - voltage_mv)
            if d < best_dist:
                best_dist = d
                best_idx = i
        return best_idx

    def _resolve_vfp_lock_idx_from_input(self) -> Optional[int]:
        """Resolve the lock panel input to a point index for UI updates."""
        raw_value = self.lock_point_var.get().strip()
        if not raw_value:
            self.app.console.append("[GUI] No lock point specified.\n")
            return None

        try:
            if self.lock_voltage_var.get():
                voltage_mv = float(raw_value)
                idx = self._find_closest_voltage_idx(voltage_mv)
                if idx is None:
                    self.app.console.append(
                        "[GUI] No VF points loaded to resolve lock voltage.\n"
                    )
                    return None
                return idx

            idx = int(raw_value)
        except ValueError:
            mode = "voltage" if self.lock_voltage_var.get() else "index"
            self.app.console.append(f"[GUI] Invalid lock {mode} value: {raw_value}\n")
            return None

        if not self._voltages:
            return idx
        if 0 <= idx < len(self._voltages):
            return idx

        self.app.console.append(f"[GUI] Lock point index out of range: {idx}\n")
        return None

    def _apply_vfp_lock_ui(self, idx: Optional[int]):
        """Update UI state after a successful VFP point lock."""
        self._clear_core_freq_lock_ui()
        self._locked_points.clear()
        if idx is not None:
            self._locked_points.add(idx)
        self._redraw()

    def _apply_vfp_unlock_ui(self):
        """Update UI state after a successful VFP unlock."""
        self._locked_points.clear()
        self._redraw()

    def update_live_point(self, volt_mv: Optional[float], freq_mhz: Optional[float]):
        """Update the real-time crosshair overlay for the current operating point."""
        if self._cleaned_up:
            return
        self._live_volt = volt_mv
        self._live_freq = freq_mhz
        if (
            self._is_resize_active
            or self._mouse_pressed
            or not self._chart_should_draw()
        ):
            self._pending_live_point = (volt_mv, freq_mhz)
            return
        self._draw_live_point()

    def _flush_pending_live_point(self):
        """Apply a deferred live-point update after resize/interaction ends."""
        if self._pending_live_point is None:
            return
        self._live_volt, self._live_freq = self._pending_live_point
        self._pending_live_point = None
        if self._chart_should_draw():
            self._draw_live_point()

    # ── Live crosshair poller (independent of the dashboard poll) ──
    _LIVE_POLL_MS = 1000

    def set_live_pending(
        self, volt_mv: Optional[float], freq_mhz: Optional[float]
    ) -> None:
        """Thread-safe sink for volt/freq from a background poll worker.

        The dashboard poll feeds the crosshair through this instead of
        scheduling an after(0) blit, so its completion cannot interpose a
        blit ahead of a mouse-press in the Tcl event queue (which delayed
        the first click on the curve). The live timer below drains it.

        This feed is GPC-only (the dashboard polls gpu_clock / voltage for the
        graphics domain). Accept it only when GPC is the active AND visible
        curve — otherwise it would pollute the live point with GPC freq while
        the user is looking at an XBAR/HOST curve (whose freq comes from the
        direct-read path), and it would keep "polling" a GPC crosshair the user
        has unchecked. XBAR/HOST live data is pushed here by the direct-read
        completion callback instead.
        """
        if self._active_curve != "gpc" or not self._curve_visible.get("gpc"):
            return
        self._live_pending = (volt_mv, freq_mhz)

    def start_live_poll(self) -> None:
        """Begin the low-cadence crosshair refresh (VF Curve tab active)."""
        if self._live_poll_job is None:
            self._live_poll_job = self.app.after(
                self._LIVE_POLL_MS, self._live_poll_tick
            )

    def stop_live_poll(self) -> None:
        if self._live_poll_job is not None:
            try:
                self.app.after_cancel(self._live_poll_job)
            except Exception:
                pass
            self._live_poll_job = None

    def _live_poll_tick(self) -> None:
        self._live_poll_job = None
        if self._cleaned_up:
            return
        # Only the active+visible curve gets a live-point crosshair. A curve
        # that's unchecked (not visible) is never polled — including GPC, whose
        # dashboard feed is gated in set_live_pending. The active curve is
        # always visible by construction, but guard anyway.
        if not self._curve_visible.get(self._active_curve):
            self._live_volt = None
            self._live_freq = None
            self._hide_live_point()
            self._live_poll_job = self.app.after(
                self._LIVE_POLL_MS, self._live_poll_tick
            )
            return

        # When the active curve is XBAR/HOST, the operating point's frequency
        # comes from the green-curve direct read (0x527FC458), not the GPC
        # gpu_clock the dashboard poll feeds. Kick an async direct read each
        # tick; the result lands in _live_pending and is drawn next tick (or
        # immediately if idle and the read already completed).
        curve = self._curves.get(self._active_curve)
        if (
            curve is not None
            and self._active_curve in ("xbar", "sys")
            and not self._direct_read_inflight
        ):
            self._kick_direct_read(self._active_curve)

        # Drain the latest volt/freq pushed by the background poll worker and
        # blit — but only when idle. During a drag/press the value is held and
        # flushed on release via _flush_pending_live_point.
        volt, freq = self._live_pending
        self._live_volt = volt
        self._live_freq = freq
        if (
            not (self._is_resize_active or self._mouse_pressed)
            and self._chart_should_draw()
        ):
            self._draw_live_point()
        elif self._mouse_pressed or self._is_resize_active:
            # hold for the release/resize-end flush
            self._pending_live_point = (volt, freq)
        self._live_poll_job = self.app.after(self._LIVE_POLL_MS, self._live_poll_tick)

    def _kick_direct_read(self, curve_id: str) -> None:
        """Async direct-read of the active xbar/host domain's physical clock.

        On completion the worker reverse-looks-up the voltage on the active
        curve (freq → voltage interpolation) and pushes (volt, freq_mhz) into
        ``_live_pending`` for the next tick to blit. Direct read only gives
        frequency; the voltage is recovered from the curve itself (xbar/host
        have no pstate-off-curve excursion, so the reverse lookup is faithful).
        """
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        domain_bit = _CURVE_META[curve_id]["domain_bit"]
        curve = self._curves.get(curve_id)
        if curve is None or not curve.voltages:
            return
        # Snapshot the curve points for the main-thread callback (the dict may
        # be replaced by a refresh between submit and completion).
        volts = list(curve.voltages)
        freqs = list(curve.frequencies)
        self._direct_read_inflight = True

        def _worker():
            result = self.app.backend.query_private_freq_domain_status(gpu, domain_bit)
            self.app.after(
                0, lambda: self._on_direct_read_done(result, curve_id, volts, freqs)
            )

        self.app.run_background("vfcurve-direct-read", _worker)

    def _on_direct_read_done(self, result, curve_id, volts, freqs):
        self._direct_read_inflight = False
        if self._cleaned_up:
            return
        # Stale result for a different active curve than we're now showing,
        # or the curve got unchecked (hidden) while the read was in flight —
        # a hidden curve gets no crosshair, so drop the result.
        if curve_id != self._active_curve or not self._curve_visible.get(curve_id):
            return
        if not isinstance(result, dict):
            return
        if result.get("supported") is False:
            # Family absent — no live point for this domain.
            self._live_pending = (None, None)
            return
        freq_khz = result.get("freq_khz")
        if not freq_khz:
            # 0 ⇒ driver refused / not measurable through this interface.
            return
        freq_mhz = freq_khz / 1000.0
        volt = self._reverse_lookup_voltage(volts, freqs, freq_mhz)
        if volt is None:
            return
        self._live_pending = (volt, freq_mhz)
        # If idle, blit immediately rather than waiting for the next tick.
        if (
            not (self._is_resize_active or self._mouse_pressed)
            and self._chart_should_draw()
        ):
            self._live_volt = volt
            self._live_freq = freq_mhz
            self._draw_live_point()

    @staticmethod
    def _reverse_lookup_voltage(
        volts: List[float], freqs: List[float], target_freq: float
    ) -> Optional[float]:
        """Find the voltage on the curve whose frequency is closest to
        ``target_freq``, linearly interpolating between the two nearest points.

        xbar/host curves are monotonic in frequency vs voltage (no pstate
        off-curve excursion), so the reverse lookup is single-valued. Returns
        ``None`` when the curve is empty.
        """
        n = len(freqs)
        if n == 0:
            return None
        if n == 1:
            return volts[0]
        # Find the segment [i, i+1] whose freq span contains target_freq, or
        # the nearest end if target is outside the range (clamp to the closer
        # end so the crosshair still marks the working point at the edge).
        # Curve should be freq-ascending with voltage; handle either direction.
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

    @property
    def is_interacting(self) -> bool:
        """True while the user is actively dragging points or extending a
        selection on the curve. The dashboard poll pauses during interaction
        so its per-second NVAPI sweep + main-thread completion callback
        (parse/rows/snapshot/live-point) cannot interrupt the drag."""
        return bool(self._mouse_pressed or self._dragging)

    def _chart_should_draw(self) -> bool:
        if not hasattr(self, "canvas") or not hasattr(self, "_chart_frame"):
            return False
        try:
            if not self._chart_frame.winfo_ismapped():
                return False
            return str(self.app.tabview.get()).endswith("VF Curve")
        except Exception:
            return False

    def on_resize_state_changed(self, resizing: bool, force_flush: bool = False):
        self._is_resize_active = resizing
        if resizing:
            return

        pending_w = self._pending_chart_resize_width
        self._pending_chart_resize_width = None
        if pending_w is not None:
            self._apply_chart_resize(pending_w)

        if self._pending_full_redraw:
            self._pending_full_redraw = False
            self._redraw()

        self._flush_pending_live_point()

    def _draw_live_point(self, call_draw_idle: bool = True):
        if self._live_volt is None or self._live_freq is None or not self._voltages:
            self._hide_live_point()
            if call_draw_idle:
                self._blit_animated()
            return

        lv = self._live_volt
        lf = self._live_freq
        ax = self.ax

        if self._live_hline is not None:
            self._live_hline.set_ydata([lf, lf])
            self._live_vline.set_xdata([lv, lv])
            self._live_marker.set_data([lv], [lf])
            self._live_text.xy = (lv, lf)
            self._live_text.set_text(f"Live: {lv:.1f} mV, {lf:.0f} MHz")
            self._set_live_point_visible(True)
            if call_draw_idle:
                self._blit_animated()
            return

        crosshair_kw = dict(
            color="#22cc44", linewidth=1.0, linestyle="--", alpha=0.85, animated=True
        )
        hline = ax.axhline(y=lf, zorder=6.0, **crosshair_kw)
        vline = ax.axvline(x=lv, zorder=5.0, **crosshair_kw)

        # Center marker
        (marker,) = ax.plot(
            lv,
            lf,
            marker="+",
            markersize=8,
            color="#22cc44",
            markeredgewidth=1.2,
            zorder=7.0,
            linestyle="none",
            animated=True,
        )

        # Label (placed slightly below to avoid overlapping with default lock markers)
        # Using [Live] instead of emoji to avoid UserWarning/Glyph missing issues
        text = ax.annotate(
            f"Live: {lv:.1f} mV, {lf:.0f} MHz",
            xy=(lv, lf),
            xytext=(-70, 3),
            textcoords="offset points",
            color="#88ffaa",
            fontsize=5,
            zorder=8,
            animated=True,
        )

        self._live_elements.extend([hline, vline, marker, text])
        self._live_hline = hline
        self._live_vline = vline
        self._live_marker = marker
        self._live_text = text

        if call_draw_idle:
            self._blit_animated()

    def _hide_live_point(self) -> None:
        self._set_live_point_visible(False)

    def _update_selection_span(self) -> None:
        """Lightweight selection-range repaint (no full _redraw).

        The selection span (yellow band) is purely a voltage-axis range — it
        does not depend on the curve points' frequencies, which only move
        during a point drag, not a selection drag. So a selection-range drag
        can rebuild just the span + the selected-point markers and blit,
        instead of clearing the axes and rebuilding every artist. This keeps
        the selection visual perfectly smooth and immune to the per-second
        live-point blit (which restores the static background that no longer
        contains the span or the markers — both are animated overlays now).

        axvspan's Polygon uses a blended transform (x in data coords, y in
        axes coords), so in-place set_xy can't be used; remove+recreate is
        cheap and correct. The selected-point scatter's offsets are updated
        in place from the current frequencies so the markers track the band.
        """
        if self.ax is None or self._cleaned_up:
            return
        if not self._voltages:
            return
        if self._sel_start is None or self._sel_end is None:
            # Clear the span + markers if they exist.
            changed = False
            if self._sel_rect is not None:
                self._sel_rect.set_visible(False)
                changed = True
            if self._sel_points is not None:
                self._sel_points.set_visible(False)
                changed = True
            if changed:
                self._blit_animated()
            return
        s = min(self._sel_start, self._sel_end)
        e = max(self._sel_start, self._sel_end)
        v = self._voltages
        f = self._frequencies
        x0, x1 = v[s], v[e]

        # Recreate the span (blended transform makes set_xy unsafe).
        if self._sel_rect is not None:
            try:
                self._sel_rect.remove()
            except Exception:
                pass
        self._sel_rect = self.ax.axvspan(
            x0, x1, alpha=0.15, color="#ffcc00", zorder=1, animated=True
        )

        # Update selected-point markers in place to track the band.
        import numpy as _np

        sel_v = v[s : e + 1]
        sel_f = f[s : e + 1]
        if self._sel_points is None:
            self._sel_points = self.ax.scatter(
                sel_v,
                sel_f,
                color="#ffcc00",
                s=14,
                zorder=5,
                edgecolors="#ff8800",
                linewidths=0.6,
                animated=True,
            )
        else:
            self._sel_points.set_offsets(_np.column_stack([sel_v, sel_f]))
            self._sel_points.set_visible(True)

        self._blit_animated()

    def _set_live_point_visible(self, visible: bool) -> None:
        for el in self._live_elements:
            try:
                el.set_visible(visible)
            except Exception:
                # Matplotlib artists can be stale or removed during redraw churn.
                pass

    # ────────────────────────────────────────────
    # Keyboard navigation
    # ────────────────────────────────────────────

    # How many MHz one Up/Down key press shifts the selected point(s)
    _KEY_FREQ_STEP_MHZ = 2.5  # one step ≈ 2.5 MHz (1 VF table row in kHz × 2.5)

    def _is_single_point_sel(self) -> bool:
        return (
            self._sel_start is not None
            and self._sel_end is not None
            and self._sel_start == self._sel_end
        )

    def _is_range_sel(self) -> bool:
        return (
            self._sel_start is not None
            and self._sel_end is not None
            and self._sel_start != self._sel_end
        )

    def _selected_freq_lock_backend(self) -> str:
        selected = self.freq_lock_api_var.get().strip().upper()
        return "nvapi" if selected == "NVAPI" else "nvml"

    def _selected_freq_lock_backend_label(self) -> str:
        return self._selected_freq_lock_backend().upper()

    def _backend_label(self, backend: str) -> str:
        return backend.upper()

    def _set_core_freq_lock_ui(self, min_mhz: int, max_mhz: int, backend: str) -> None:
        self._freq_core_lock = (min_mhz, max_mhz)
        self._freq_core_lock_backend = backend
        self.core_lock_min_var.set(str(min_mhz))
        self.core_lock_max_var.set(str(max_mhz))

    def _clear_core_freq_lock_ui(self) -> None:
        self._freq_core_lock = None
        self._freq_core_lock_backend = None
        self.core_lock_min_var.set("0")
        self.core_lock_max_var.set("0")

    def _set_mem_freq_lock_ui(self, min_mhz: int, max_mhz: int, backend: str) -> None:
        self._freq_mem_lock = (min_mhz, max_mhz)
        self._freq_mem_lock_backend = backend
        self.mem_lock_min_var.set(str(min_mhz))
        self.mem_lock_max_var.set(str(max_mhz))

    def _clear_mem_freq_lock_ui(self) -> None:
        self._freq_mem_lock = None
        self._freq_mem_lock_backend = None
        self.mem_lock_min_var.set("0")
        self.mem_lock_max_var.set("0")

    def _core_reset_backend(self, default_backend: str) -> str:
        if self._freq_core_lock is not None and self._freq_core_lock_backend:
            return self._freq_core_lock_backend
        return default_backend

    def _mem_reset_backend(self, default_backend: str) -> str:
        if self._freq_mem_lock is not None and self._freq_mem_lock_backend:
            return self._freq_mem_lock_backend
        return default_backend

    def _lock_core_native(
        self, native, gpu: str, backend: str, min_mhz: int, max_mhz: int
    ) -> None:
        if backend == "nvapi":
            native.set_vfp_frequency_lock(gpu, "core", max_mhz * 1000, min_mhz * 1000)
        else:
            native.set_locked_clocks(gpu, backend, "core", min_mhz, max_mhz)

    def _reset_core_native(self, native, gpu: str, backend: str) -> None:
        if backend == "nvapi":
            native.reset_vfp_frequency_lock(gpu, "core")
        else:
            native.reset_core_clocks(gpu, backend)

    def _lock_mem_native(
        self, native, gpu: str, backend: str, min_mhz: int, max_mhz: int
    ) -> None:
        if backend == "nvapi":
            native.set_vfp_frequency_lock(gpu, "memory", max_mhz * 1000, min_mhz * 1000)
        else:
            native.set_locked_clocks(gpu, backend, "memory", min_mhz, max_mhz)

    def _reset_mem_native(self, native, gpu: str, backend: str) -> None:
        if backend == "nvapi":
            native.reset_vfp_frequency_lock(gpu, "memory")
        else:
            native.reset_mem_clocks(gpu, backend)

    # ── Left / Shift-Tab : move selection left (lower index) ──
    def _on_key_left(self, event=None):
        if not self._voltages or self._sel_start is None:
            return "break"
        _n = len(self._voltages)
        if self._is_single_point_sel():
            new = max(0, self._sel_start - 1)
            self._sel_start = self._sel_end = new
        else:
            # shift range left by 1, clamp at 0
            s = min(self._sel_start, self._sel_end)
            e = max(self._sel_start, self._sel_end)
            span = e - s
            new_s = max(0, s - 1)
            self._sel_start = new_s
            self._sel_end = new_s + span
        self._sync_selection_to_adj()
        self._redraw()
        return "break"

    def _on_key_shift_tab(self, event=None):
        return self._on_key_left(event)

    # ── Right / Tab : move selection right (higher index) ──
    def _on_key_right(self, event=None):
        if not self._voltages or self._sel_start is None:
            return "break"
        n = len(self._voltages)
        if self._is_single_point_sel():
            new = min(n - 1, self._sel_start + 1)
            self._sel_start = self._sel_end = new
        else:
            s = min(self._sel_start, self._sel_end)
            e = max(self._sel_start, self._sel_end)
            span = e - s
            new_e = min(n - 1, e + 1)
            self._sel_end = new_e
            self._sel_start = new_e - span
        self._sync_selection_to_adj()
        self._redraw()
        return "break"

    def _on_key_tab(self, event=None):
        return self._on_key_right(event)

    # ── Up : increase frequency of selected point(s) ──
    def _on_key_up(self, event=None):
        if not self._voltages or self._sel_start is None:
            return "break"
        self._key_shift_freq(+self._KEY_FREQ_STEP_MHZ)
        return "break"

    # ── Down : decrease frequency of selected point(s) ──
    def _on_key_down(self, event=None):
        if not self._voltages or self._sel_start is None:
            return "break"
        self._key_shift_freq(-self._KEY_FREQ_STEP_MHZ)
        return "break"

    # ── Mouse wheel : equivalent to Up / Down key ──
    def _on_mousewheel(self, event):
        """Handle mouse wheel to adjust frequency of selected point(s)."""
        if not self._voltages or self._sel_start is None:
            return "break"

        event_num = getattr(event, "num", None)
        if event_num == 4:
            delta_mhz = +self._KEY_FREQ_STEP_MHZ  # Linux scroll up = increase freq
        elif event_num == 5:
            delta_mhz = -self._KEY_FREQ_STEP_MHZ  # Linux scroll down = decrease freq
        else:
            delta = int(getattr(event, "delta", 0) or 0)
            if delta == 0:
                return "break"
            # Windows: positive delta = scroll up = increase, negative = scroll down = decrease
            delta_mhz = (
                self._KEY_FREQ_STEP_MHZ if delta > 0 else -self._KEY_FREQ_STEP_MHZ
            )

        self._key_shift_freq(delta_mhz)
        return "break"

    def _key_shift_freq(self, delta_mhz: float):
        """Shift the frequency of the currently selected point(s) by delta_mhz."""
        if self._sel_start is None or self._sel_end is None:
            return
        s = min(self._sel_start, self._sel_end)
        e = max(self._sel_start, self._sel_end)

        # Save undo snapshot before first edit in a batch
        if self._drag_orig_freqs is None:
            self._drag_orig_freqs = self._np().array(self._frequencies, dtype=float)

        for i in range(s, e + 1):
            self._frequencies[i] = round(self._frequencies[i] + delta_mhz, 3)

        self._sync_selection_to_adj()
        # Fast in-place update per event (drag path pattern); the full
        # redraw (axes/limits/info text) is deferred until events settle.
        self._fast_update_current_curve()
        if self._key_redraw_after_id is not None:
            self.app.after_cancel(self._key_redraw_after_id)
        self._key_redraw_after_id = self.app.after(150, self._deferred_key_redraw)

    def _deferred_key_redraw(self):
        self._key_redraw_after_id = None
        self._redraw()

    def _fast_update_current_curve(self):
        """Update the current-curve artist in place (no figure rebuild)."""
        line = getattr(self, "_line_current", None)
        if line is None or self.ax is None:
            return
        line.set_ydata(self._frequencies)
        self._blit_animated()

    def _on_space_key(self, event=None):
        """Toggle lock state based on selection.
        - Single point: Cycle Unlock -> VFP Lock -> Freq Lock -> Unlock.
        - Range: Toggle Unlock <-> Freq Range Lock.
        """
        if getattr(self, "_is_toggling_lock", False):
            self.app.console.append("[GUI] Operation in progress. Please wait...\n")
            return "break"

        if not self._voltages:
            return "break"
        if self._sel_start is None or self._sel_end is None:
            return "break"

        s = min(self._sel_start, self._sel_end)
        e = max(self._sel_start, self._sel_end)
        gpu = self.app.selected_gpu_target()
        lock_backend = self._selected_freq_lock_backend()
        lock_backend_label = self._selected_freq_lock_backend_label()

        self._is_toggling_lock = True

        # Capture current variables to pass into thread
        idx = s
        cur_f = int(round(self._frequencies[idx]))
        min_f = int(round(min(self._frequencies[s : e + 1])))
        max_f = int(round(max(self._frequencies[s : e + 1])))
        vol = self._voltages[idx]

        is_vfp_locked = idx in self._locked_points
        is_freq_locked_single = (
            self._freq_core_lock is not None and self._freq_core_lock == (cur_f, cur_f)
        )
        is_freq_locked_range = (
            self._freq_core_lock is not None and self._freq_core_lock == (min_f, max_f)
        )
        has_vfp_locks = len(self._locked_points) > 0
        active_freq_backend = self._core_reset_backend(lock_backend)
        active_freq_backend_label = self._backend_label(active_freq_backend)

        if s == e and is_vfp_locked:
            description = "convert VFP lock to frequency lock"

            def action(native) -> str:
                native.reset_vfp_lock(gpu)
                self._lock_core_native(native, gpu, lock_backend, cur_f, cur_f)
                return (
                    f"Successfully applied {lock_backend_label} lock for point {idx}."
                )

            def done(rc: int, local_f=cur_f, backend=lock_backend) -> None:
                self._locked_points.clear()
                if rc == 0:
                    self._set_core_freq_lock_ui(local_f, local_f, backend)
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        elif s == e and is_freq_locked_single:
            description = "reset frequency lock"

            def action(native) -> str:
                self._reset_core_native(native, gpu, active_freq_backend)
                return f"Successfully reset {active_freq_backend_label} lock."

            def done(rc: int) -> None:
                if rc == 0:
                    self._clear_core_freq_lock_ui()
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        elif s == e and lock_backend == "nvml":
            description = "lock NVML frequency point"

            def action(native) -> str:
                if has_vfp_locks:
                    native.reset_vfp_lock(gpu)
                self._lock_core_native(native, gpu, lock_backend, cur_f, cur_f)
                return (
                    f"Successfully applied {lock_backend_label} lock for point {idx}."
                )

            def done(rc: int, local_f=cur_f, backend=lock_backend) -> None:
                self._locked_points.clear()
                if rc == 0:
                    self._set_core_freq_lock_ui(local_f, local_f, backend)
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        elif s == e:
            description = "lock VFP point"

            def action(native) -> str:
                self._reset_core_native(native, gpu, lock_backend)
                native.set_vfp_voltage_lock(gpu, idx, None, False)
                return f"Locked VFP point {idx}."

            def done(rc: int, local_idx=idx) -> None:
                self._clear_core_freq_lock_ui()
                if rc == 0:
                    self._locked_points.clear()
                    self._locked_points.add(local_idx)
                    self.app.console.append(
                        f"[GUI] VFP Lock applied ({vol:.1f} mV / {cur_f} MHz).\n"
                    )
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        elif is_freq_locked_range:
            description = "reset frequency range lock"

            def action(native) -> str:
                self._reset_core_native(native, gpu, active_freq_backend)
                return f"Successfully reset {active_freq_backend_label} range lock."

            def done(rc: int) -> None:
                if rc == 0:
                    self._clear_core_freq_lock_ui()
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        else:
            description = "apply frequency range lock"

            def action(native) -> str:
                if has_vfp_locks:
                    native.reset_vfp_lock(gpu)
                self._lock_core_native(native, gpu, lock_backend, min_f, max_f)
                return (
                    f"Successfully applied {lock_backend_label} lock for range {s}-{e} "
                    f"({min_f}-{max_f} MHz)."
                )

            def done(rc: int, lf_min=min_f, lf_max=max_f, backend=lock_backend) -> None:
                self._locked_points.clear()
                if rc == 0:
                    self._set_core_freq_lock_ui(lf_min, lf_max, backend)
                self._redraw()
                self.canvas.draw()
                self._is_toggling_lock = False

        self.app.run_native_action(description, action, on_finished=done)
        return "break"

    def _clear_selection(self):
        self._sel_start = None
        self._sel_end = None
        self.adj_start_var.set("0")
        self.adj_end_var.set("0")
        self._redraw()

    def _undo_drag(self):
        """Undo the last drag edit by restoring original frequencies."""
        if self._drag_orig_freqs is not None and len(self._drag_orig_freqs) == len(
            self._frequencies
        ):
            self._frequencies = self._drag_orig_freqs.tolist()
            self._drag_orig_freqs = None
            self._redraw()
            self.app.console.append("[GUI] Drag edit undone.\n")
        else:
            self.app.console.append("[GUI] Nothing to undo.\n")

    def _sync_selection_to_adj(self):
        """Sync chart selection range (and current avg delta) to the adjustment fields."""
        if self._sel_start is None or self._sel_end is None:
            return
        s = min(self._sel_start, self._sel_end)
        e = max(self._sel_start, self._sel_end)
        self.adj_start_var.set(str(s))
        self.adj_end_var.set(str(e))
        # Show avg delta vs default in the delta field (for reference only)
        if self._frequencies and self._defaults:
            deltas = [
                self._frequencies[i] - self._defaults[i]
                for i in range(s, min(e + 1, len(self._frequencies)))
            ]
            if deltas:
                avg = sum(deltas) / len(deltas)
                self.adj_delta_var.set(f"{avg:+.1f}")

    # ────────────────────────────────────────────
    # Actions (native pynvoc calls)
    # ────────────────────────────────────────────
    def _export_vfp(self):
        gpu = self.app.selected_gpu_target()

        path = filedialog.asksaveasfilename(
            title="Export VF Curve",
            defaultextension=".csv",
            filetypes=[("CSV Files", "*.csv"), ("All Files", "*.*")],
        )
        if not path:
            return

        def export(native, gpu=gpu, path=path) -> str:
            points = native.query_public_vftable(gpu, "graphics", True)
            self._write_vfp_points(path, points)
            return f"Exported {len(points)} VFP point(s) to {path}."

        self.app.run_native_action("export VFP curve", export)

    def _import_vfp(self):
        gpu = self.app.selected_gpu_target()

        path = filedialog.askopenfilename(
            title="Import VF Curve",
            defaultextension=".csv",
            filetypes=[("CSV Files", "*.csv"), ("All Files", "*.*")],
        )
        if not path:
            return

        def import_curve(native, gpu=gpu, path=path) -> str:
            points = native.query_public_vftable(gpu, "graphics", True)
            deltas = self._load_vfp_deltas(path, points)
            native.set_domain_vfp_deltas(gpu, "graphics", deltas)
            return f"Imported {len(deltas)} VFP point delta(s) from {path}."

        self.app.run_native_action(
            "import VFP curve",
            import_curve,
            on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
        )

    def _on_core_lock_done(
        self, rc: int, min_clk: int, max_clk: int, backend: str, backend_label: str
    ):
        def _update_ui():
            if rc == 0:
                self._set_core_freq_lock_ui(min_clk, max_clk, backend)
                self.app.console.append(
                    f"[GUI] {backend_label} core clock locked successfully.\n"
                )
            else:
                self.app.console.append(
                    f"[GUI] {backend_label} core clock lock failed.\n"
                )
            self._redraw()
            self._is_toggling_lock = False

        self.app.after(0, _update_ui)

    def _on_core_reset_done(self, rc: int, backend_label: str):
        def _update_ui():
            if rc == 0:
                self._clear_core_freq_lock_ui()
                self.app.console.append(
                    f"[GUI] {backend_label} core clock reset successfully.\n"
                )
            else:
                self.app.console.append(
                    f"[GUI] {backend_label} core clock reset failed.\n"
                )
            self._redraw()
            self._is_toggling_lock = False

        self.app.after(0, _update_ui)

    def _on_mem_lock_done(
        self, rc: int, min_clk: int, max_clk: int, backend: str, backend_label: str
    ):
        def _update_ui():
            if rc == 0:
                self._set_mem_freq_lock_ui(min_clk, max_clk, backend)
                self.app.console.append(
                    f"[GUI] {backend_label} memory clock locked successfully.\n"
                )
            else:
                self.app.console.append(
                    f"[GUI] {backend_label} memory clock lock failed.\n"
                )
            self._redraw()

        self.app.after(0, _update_ui)

    def _on_mem_reset_done(self, rc: int, backend_label: str):
        def _update_ui():
            if rc == 0:
                self._clear_mem_freq_lock_ui()
                self.app.console.append(
                    f"[GUI] {backend_label} memory clock reset successfully.\n"
                )
            else:
                self.app.console.append(
                    f"[GUI] {backend_label} memory clock reset failed.\n"
                )
            self._redraw()

        self.app.after(0, _update_ui)

    def _apply_adj(self):
        """Apply the current frequency edits for the selected range to the GPU.

        Routes by the active curve's ``write_mode``:

        * ``public``  — open VFP ``set_vfp_range_delta`` (grouped, unchanged).
        * ``private`` — try private mode-0 (kHz offset) per point; on
          ArgumentRange (mode-0 rejected, e.g. CMP170HX / Fixed points) fall
          back to raw-converted mode-1 via ``clk_vf_delta_for_target`` +
          ``set_vfp_range_per_point_private``. Console-logs which path ran.
        """
        gpu = self.app.selected_gpu_target()
        try:
            start = int(self.adj_start_var.get())
            end = int(self.adj_end_var.get())
        except ValueError:
            self.app.console.append("[GUI] Invalid start/end point values.\n")
            return

        if start > end:
            start, end = end, start

        if not self._frequencies or not self._defaults:
            self.app.console.append("[GUI] No VF data loaded.\n")
            return

        try:
            target_delta_mhz = float(self.adj_delta_var.get().strip())
        except ValueError:
            self.app.console.append("[GUI] Invalid Delta (MHz) value.\n")
            return

        # Consume a pending wall drag (if any). A wall apply shares the
        # "Apply to GPU" button with VFP point deltas: when there is no VFP
        # edit (delta == 0), the wall is applied alone; otherwise it is
        # prepended to the VFP apply lambda so both write in one action.
        pending_wall = self._pending_wall_mv
        self._pending_wall_mv = None
        wall_only = pending_wall is not None and target_delta_mhz == 0
        if wall_only:
            self._apply_wall_target(pending_wall)
            self._redraw()
            return

        n = len(self._frequencies)
        start = max(0, min(start, n - 1))
        end = max(0, min(end, n - 1))

        if self._drag_orig_freqs is None or len(self._drag_orig_freqs) != len(
            self._frequencies
        ):
            self._drag_orig_freqs = self._np().array(self._frequencies, dtype=float)

        for i in range(start, end + 1):
            self._frequencies[i] = round(self._defaults[i] + target_delta_mhz, 3)

        self._redraw()

        curve = self._curves.get(self._active_curve)
        # Build per-point delta list (kHz, integer) vs default.
        deltas_khz = [
            round((self._frequencies[i] - self._defaults[i]) * 1000)
            for i in range(start, end + 1)
        ]

        # ── Public path: GPC via the open VFP interface (unchanged). ──
        if curve is not None and curve.write_mode == "public":
            groups = []  # type: List[Tuple[int,int,int]]
            g_start = start
            g_delta = deltas_khz[0]
            for offset, dkz in enumerate(deltas_khz[1:], start=1):
                if dkz != g_delta:
                    groups.append((g_start, start + offset - 1, g_delta))
                    g_start = start + offset
                    g_delta = dkz
            groups.append((g_start, end, g_delta))

            self.app.console.append(
                f"[GUI] Applying {len(groups)} public VFP group(s) "
                f"to {self._active_curve.upper()} {start}–{end}…\n"
            )

            def apply_groups(
                native,
                gpu=gpu,
                groups=groups,
                pending_wall=pending_wall,
                rail_bit=self._p0_rail_bit,
            ) -> str:
                applied = 0
                failed = 0
                messages = []
                if pending_wall is not None:
                    messages.append(
                        self._apply_wall_inline(native, gpu, pending_wall, rail_bit)
                    )
                for frm, to, dkz in groups:
                    try:
                        native.set_vfp_range_delta(gpu, frm, to, dkz)
                    except Exception as exc:
                        failed += 1
                        messages.append(
                            f"Warning: failed VFP delta group {frm}-{to} ({dkz} kHz): {exc}"
                        )
                        continue
                    applied += 1
                messages.append(
                    f"Applied {applied} VFP delta group(s); {failed} failed."
                )
                return "\n".join(messages)

            self.app.run_native_action(
                "apply VFP point deltas",
                apply_groups,
                on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
            )
            return

        # ── Private path: mode-0 first, raw-converted fallback. ──
        if curve is None:
            self.app.console.append("[GUI] Active curve missing — cannot apply.\n")
            return
        bank = curve.bank
        base = curve.seg_start + start  # absolute private index of `start`
        class_name = _CURVE_META[curve.curve_id]["class"]
        defaults_mhz = list(self._defaults)
        self.app.console.append(
            f"[GUI] Applying private VFP to {curve.curve_id.upper()} "
            f"{start}–{end} (bank {bank}, mode-0 → raw-converted fallback)…\n"
        )

        def apply_private(
            native,
            gpu=gpu,
            bank=bank,
            base=base,
            class_name=class_name,
            defaults_mhz=defaults_mhz,
            deltas_khz=deltas_khz,
            start=start,
            curve_id=curve.curve_id,
            pending_wall=pending_wall,
            rail_bit=self._p0_rail_bit,
        ) -> str:
            wall_msg = ""
            if pending_wall is not None:
                wall_msg = (
                    self._apply_wall_inline(native, gpu, pending_wall, rail_bit) + "\n"
                )
            # 1) Try mode-0 (kHz frequency offset) per point.
            try:
                for offset, dkz in enumerate(deltas_khz):
                    r = native.set_vfp_point_private(
                        gpu, bank, base + offset, dkz, True
                    )
                    if isinstance(r, dict) and r.get("supported") is False:
                        raise RuntimeError("private VFP family unsupported")
                return wall_msg + (
                    f"Successfully applied private mode-0 offsets to {curve_id.upper()} "
                    f"({len(deltas_khz)} pts)."
                )
            except Exception as exc:
                msg = str(exc).lower()
                if "argument" not in msg and "unsupported" not in msg:
                    raise
                # mode-0 rejected at readback → fall through to raw-converted.
            # 2) Raw-converted: translate each MHz offset to a raw mode-1
            # f-offset control value via the universal g(def) prior.
            raw_deltas = []
            for offset in range(len(deltas_khz)):
                def_mhz = int(round(defaults_mhz[start + offset]))
                tgt_mhz = deltas_khz[offset] / 1000.0
                r = native.clk_vf_delta_for_target_mhz(def_mhz, tgt_mhz, class_name)
                d = r.get("delta") if isinstance(r, dict) else None
                if d is None:
                    return wall_msg + (
                        f"raw-converted translation failed at def={def_mhz} MHz "
                        f"({curve_id.upper()}); apply aborted."
                    )
                raw_deltas.append(int(d))
            last = base + len(deltas_khz) - 1
            r2 = native.set_vfp_range_per_point_private(
                gpu, bank, base, last, raw_deltas
            )
            if isinstance(r2, dict) and r2.get("supported") is False:
                return (
                    wall_msg + f"private VFP write unsupported on {curve_id.upper()}."
                )
            return wall_msg + (
                f"Successfully applied private raw-converted offsets to {curve_id.upper()} "
                f"({len(raw_deltas)} pts)."
            )

        self.app.run_native_action(
            "apply VFP point deltas",
            apply_private,
            on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
        )

    # ── P0 wall apply (drag → Apply to GPU) ──
    _WALL_STEP_MV = 2.5  # LCM of 5 mV (30/40-series) and 12.5 mV (10/20-series)

    def _snap_wall_mv(self, mv: float) -> float:
        """Snap a free-continuous drag value to the 2.5 mV rail grid."""
        return round(mv / self._WALL_STEP_MV) * self._WALL_STEP_MV

    @staticmethod
    def _format_wall_result(target_mv: float, result: object) -> str:
        """Console message from a set_volt_rail_target result dict."""
        if not isinstance(result, dict):
            return f"P0 wall target {target_mv:g} mV: applied (no readback)."
        if result.get("supported") is False:
            return f"P0 wall target {target_mv:g} mV: unsupported."
        eff = result.get("effective_wall_uV", 0)
        eff_mv = (int(eff) / 1000.0) if eff else target_mv
        return (
            f"P0 wall target {target_mv:g} mV → effective {eff_mv:g} mV "
            f"(clamped to min(target, vbios_wall, vrm_max_wall))."
        )

    def _apply_wall_inline(
        self, native, gpu: str, pending_mv: float, rail_bit: int
    ) -> str:
        """Write a pending wall target from inside an apply lambda (worker
        thread). Returns a console message and schedules the effective-line
        update on the UI thread. Best-effort: a failure is logged, not
        raised, so a combined wall+VFP apply still completes the VFP half.
        """
        target_mv = self._snap_wall_mv(pending_mv)
        try:
            r = native.set_volt_rail_target(gpu, rail_bit, target_mv, None)
        except Exception as exc:
            return f"Warning: P0 wall target {target_mv:g} mV failed: {exc}"
        eff = 0
        if isinstance(r, dict):
            eff = int(r.get("effective_wall_uV", 0) or 0)
        eff_mv = (eff / 1000.0) if eff > 0 else target_mv
        self.app.after(0, lambda em=eff_mv: self._on_wall_applied(em))
        return self._format_wall_result(target_mv, r)

    def _apply_wall_target(self, mv: float) -> None:
        """Apply ONLY a pending wall (no VFP edit) via set_volt_rail_target."""
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self.app.console.append("[GUI] No GPU selected.\n")
            return
        target_mv = self._snap_wall_mv(mv)
        rail_bit = self._p0_rail_bit

        def apply_wall(native, gpu=gpu, rail_bit=rail_bit, target_mv=target_mv) -> str:
            return self._apply_wall_inline(native, gpu, target_mv, rail_bit)

        self.app.console.append(f"[GUI] Applying P0 wall target {target_mv:g} mV…\n")
        self.app.run_native_action("apply P0 volt-rail target", apply_wall)

    def _on_wall_applied(self, eff_mv: float) -> None:
        """Update the solid effective line after a wall SET lands."""
        if self._cleaned_up:
            return
        self._p0_effective_wall_mv = eff_mv
        self._pending_wall_mv = None
        self._redraw()

    def _reset_vfp(self):
        """Reset the active curve to default (selected-curve semantics).

        Public GPC → open ``set_vfp_range_delta`` 0 over the segment. Private
        (XBAR/HOST, or GPC when public is unsupported) → mode-0 clear per
        point, raw-converted clear fallback (delta 0 → raw f-offset that
        zeroes the effect). Never touches other curves' segments.
        """
        curve = self._curves.get(self._active_curve)
        if curve is None:
            self.app.console.append("[GUI] No active curve to reset.\n")
            return
        gpu = self.app.selected_gpu_target()
        cid = curve.curve_id.upper()

        if curve.write_mode == "public":
            s, e = curve.seg_start, curve.seg_end

            def reset_public(native, gpu=gpu, s=s, e=e, cid=cid) -> str:
                native.set_vfp_range_delta(gpu, s, e, 0)
                return f"Successfully reset {cid} curve to default ({s}–{e}, public)."

            self.app.run_native_action(
                "reset VFP deltas",
                reset_public,
                on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
            )
            return

        bank = curve.bank
        base = curve.seg_start
        end_idx = curve.seg_end
        class_name = _CURVE_META[curve.curve_id]["class"]
        defaults_mhz = list(curve.defaults)

        def reset_private(
            native,
            gpu=gpu,
            bank=bank,
            base=base,
            end_idx=end_idx,
            class_name=class_name,
            defaults_mhz=defaults_mhz,
            cid=cid,
        ) -> str:
            # 1) mode-0 clear (value 0) per point in the segment.
            try:
                for idx in range(base, end_idx + 1):
                    r = native.set_vfp_point_private(gpu, bank, idx, 0, True)
                    if isinstance(r, dict) and r.get("supported") is False:
                        raise RuntimeError("private VFP family unsupported")
                return f"Successfully reset {cid} (private mode-0, {base}–{end_idx})."
            except Exception as exc:
                msg = str(exc).lower()
                if "argument" not in msg and "unsupported" not in msg:
                    raise
            # 2) raw-converted clear: delta 0 → the raw f-offset that zeroes
            # the effect (≈ D0 per the prior).
            raw_deltas = []
            for idx in range(base, end_idx + 1):
                local = idx - base
                def_mhz = (
                    int(round(defaults_mhz[local])) if local < len(defaults_mhz) else 0
                )
                r = native.clk_vf_delta_for_target_mhz(def_mhz, 0.0, class_name)
                d = r.get("delta") if isinstance(r, dict) else None
                raw_deltas.append(int(d) if d is not None else 0)
            r2 = native.set_vfp_range_per_point_private(
                gpu, bank, base, end_idx, raw_deltas
            )
            if isinstance(r2, dict) and r2.get("supported") is False:
                return f"private reset unsupported on {cid}."
            return f"Successfully reset {cid} (private raw, {base}–{end_idx})."

        self.app.run_native_action(
            "reset VFP deltas",
            reset_private,
            on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
        )
