"""
VF Curve Tab - Interactive voltage-frequency curve chart with matplotlib,
plus VFP export/import, lock/unlock, and point adjustment controls.
"""

import csv
import os
import threading
import customtkinter as ctk
from tkinter import filedialog
from typing import TYPE_CHECKING, List, Optional, Tuple

import numpy as np

if TYPE_CHECKING:
    from src.app import App

from src.cli_runner import CLIRunner
from src.widgets.lightweight_controls import (
    LiteButton,
    LiteEntry,
    install_mousewheel_support,
)


class VFCurveTab:
    """VF Curve management tab with interactive chart."""

    # ── Chart export directory (relative to GUI project root) ──
    _EXPORT_DIR = "vfp_cache"
    _DEFAULT_AUTO_REFRESH_INTERVAL_MS = 1000

    def __init__(self, parent: ctk.CTkFrame, app: "App"):
        self.app = app
        self.frame = parent

        # VF data (in display units: mV and MHz)
        self._voltages: List[float] = []
        self._frequencies: List[float] = []  # current
        self._defaults: List[float] = []  # default_frequency

        # Selection state  (indices into the data arrays)
        self._sel_start: Optional[int] = None
        self._sel_end: Optional[int] = None

        # Single-point lock state: set of locked point indices
        self._locked_points = set()  # type: Set[int]

        # Frequency lock state (core/memory): (min_mhz, max_mhz)
        self._freq_core_lock = None  # type: Optional[Tuple[int, int]]
        self._freq_mem_lock = None  # type: Optional[Tuple[int, int]]

        # Drag state
        self._dragging = False
        self._drag_start_y: Optional[float] = None
        self._drag_orig_freqs: Optional[np.ndarray] = None

        # Live point state
        self._live_volt: Optional[float] = None
        self._live_freq: Optional[float] = None
        self._live_elements: list = []
        self._chart_resize_after_id = None
        self._last_chart_event_width: Optional[int] = None
        self._last_chart_resize_width: Optional[int] = None
        self._pending_chart_resize_width: Optional[int] = None
        self._is_resize_active = False
        self._pending_live_point: Optional[Tuple[Optional[float], Optional[float]]] = (
            None
        )
        self._pending_full_redraw = False
        self._refresh_curve_inflight = False
        self._refresh_curve_pending = False
        self._auto_refresh_job: Optional[str] = None
        self._auto_refreshing = False
        self._auto_refresh_interval_ms = self._DEFAULT_AUTO_REFRESH_INTERVAL_MS
        self._auto_interval_var = ctk.StringVar(value="1.0")
        self._auto_toggle_btn = None

        # ── Top: chart area (controls row + plot) ──
        chart_area = ctk.CTkFrame(self.frame, fg_color="transparent")
        chart_area.pack(fill="x", expand=False, padx=10, pady=(10, 5))

        chart_top = ctk.CTkFrame(chart_area, fg_color="transparent")
        chart_top.pack(fill="x", pady=(0, 4))
        ctk.CTkLabel(
            chart_top,
            text="📈 VF Curve Plot",
            font=("", 15, "bold"),
            text_color="#aaccff",
        ).pack(side="left", padx=8)
        auto_row = ctk.CTkFrame(chart_top, fg_color="transparent")
        auto_row.pack(side="right")
        self._auto_toggle_btn = ctk.CTkButton(
            auto_row, text="▶ Auto", width=82, command=self._toggle_auto_refresh
        )
        self._auto_toggle_btn.pack(side="left", padx=(0, 6))
        auto_interval_entry = LiteEntry(
            auto_row,
            textvariable=self._auto_interval_var,
            width=5,
            min_px=52,
            justify="right",
        )
        auto_interval_entry.pack(side="left", padx=(0, 6))
        auto_interval_entry.bind("<Return>", self._on_auto_interval_changed)
        auto_interval_entry.bind("<FocusOut>", self._on_auto_interval_changed)
        ctk.CTkLabel(auto_row, text="Refresh (s):").pack(side="left")

        self._chart_frame = ctk.CTkFrame(chart_area)
        self._chart_frame.pack(fill="x", expand=False)

        # Schedule heavy chart init (and matplotlib import) to occur after UI starts
        self.app.after(50, lambda: self._build_chart(self._chart_frame))

        # ── Chart toolbar ──
        toolbar = ctk.CTkFrame(self.frame, fg_color="transparent")
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
        ctk.CTkLabel(toolbar, text="API:").pack(side="left", padx=(10, 2))
        self.freq_lock_api_var = ctk.StringVar(value="NVML")
        self.freq_lock_api_menu = ctk.CTkOptionMenu(
            toolbar,
            values=["NVAPI", "NVML"],
            variable=self.freq_lock_api_var,
            width=92,
            height=28,
        )
        self.freq_lock_api_menu.pack(side="left", padx=(0, 5))

        # ── Bottom: scrollable controls ──
        scroll = ctk.CTkScrollableFrame(self.frame)
        scroll.pack(fill="both", expand=True, padx=10, pady=(0, 10))
        install_mousewheel_support(scroll)

        # === Main Frame ===
        top_split_frame = ctk.CTkFrame(scroll, fg_color="transparent")
        top_split_frame.pack(fill="x", pady=(0, 10))
        for col in range(4):
            top_split_frame.columnconfigure(col, weight=1, uniform="equal")

        # === Column 1: Point Adjustment ===
        adj_frame = ctk.CTkFrame(top_split_frame)
        adj_frame.grid(row=0, column=0, sticky="nsew", padx=(10, 5), pady=5)
        ctk.CTkLabel(adj_frame, text="📐Point Adj", font=("", 14, "bold")).grid(
            row=0, column=0, columnspan=2, sticky="w", padx=10, pady=(10, 5)
        )

        # Point Adjustment Rows
        ctk.CTkLabel(adj_frame, text="Range:").grid(
            row=1, column=0, sticky="w", padx=10, pady=3
        )
        adj_frame.columnconfigure(0, weight=1)
        adj_frame.columnconfigure(1, weight=1)
        range_row = ctk.CTkFrame(adj_frame, fg_color="transparent")
        range_row.grid(row=1, column=1, sticky="w", padx=6, pady=3)
        self.adj_start_var = ctk.StringVar(value="0")
        range_w = 3
        LiteEntry(
            range_row,
            textvariable=self.adj_start_var,
            width=range_w,
            min_px=36,
            justify="right",
        ).pack(side="left")
        ctk.CTkLabel(range_row, text="~").pack(side="left", padx=0)
        self.adj_end_var = ctk.StringVar(value="0")
        LiteEntry(
            range_row,
            textvariable=self.adj_end_var,
            width=range_w,
            min_px=38,
            justify="right",
        ).pack(side="left")

        ctk.CTkLabel(adj_frame, text="Δf/MHz:").grid(
            row=2, column=0, sticky="w", padx=10, pady=3
        )
        self.adj_delta_var = ctk.StringVar(value="0")
        LiteEntry(
            adj_frame,
            textvariable=self.adj_delta_var,
            width=7,
            min_px=70,
            justify="right",
        ).grid(row=2, column=1, sticky="w", padx=6, pady=3)

        btn_adj = LiteButton(
            adj_frame, text="✏️ Apply Adj", width=160, command=self._apply_adj
        )
        btn_adj.grid(row=3, column=0, columnspan=2, padx=10, pady=(10, 10))

        # === Column 2: Lock Point ===
        lock_frame = ctk.CTkFrame(top_split_frame)
        lock_frame.grid(row=0, column=1, sticky="nsew", padx=(5, 10), pady=5)
        lock_frame.columnconfigure(0, weight=1)
        lock_frame.columnconfigure(1, weight=1)
        ctk.CTkLabel(lock_frame, text="🔒Volt Lock", font=("", 14, "bold")).grid(
            row=0, column=0, columnspan=2, sticky="w", padx=10, pady=(10, 5)
        )

        # Lock Point Rows
        ctk.CTkLabel(lock_frame, text="Index:").grid(
            row=1, column=0, sticky="w", padx=10, pady=3
        )
        self.lock_point_var = ctk.StringVar(value="55")
        LiteEntry(
            lock_frame,
            textvariable=self.lock_point_var,
            width=7,
            min_px=52,
            justify="right",
        ).grid(row=1, column=1, sticky="w", padx=10, pady=3)

        self.lock_voltage_var = ctk.BooleanVar(value=False)
        ctk.CTkCheckBox(
            lock_frame, text="As volt(mV)", variable=self.lock_voltage_var
        ).grid(row=2, column=0, columnspan=2, sticky="w", padx=10, pady=10)

        # Combine Lock and Unlock buttons into a single cell
        button_frame = ctk.CTkFrame(
            lock_frame
        )  # Create a separate frame for the buttons
        button_frame.grid(
            row=3, column=0, columnspan=2, sticky="ew", padx=10, pady=(5, 10)
        )  # Span across both columns

        btn_lock = LiteButton(
            button_frame, text="🔒Lock", command=self._lock_vfp, width=70
        )
        btn_lock.pack(side="left", fill="x", padx=0)

        btn_unlock_all = LiteButton(
            button_frame, text="🔓Unlock", command=self._unlock_vfp, width=75
        )
        btn_unlock_all.pack(side="right", fill="x", padx=0)

        # === Column 3: Core Clock ===
        core_lock_frame = ctk.CTkFrame(top_split_frame)
        core_lock_frame.grid(row=0, column=2, sticky="nsew", padx=(5, 10), pady=5)
        core_lock_frame.columnconfigure(0, weight=1)
        core_lock_frame.columnconfigure(1, weight=1)
        ctk.CTkLabel(
            core_lock_frame, text="⚙Core Freq Lock", font=("", 14, "bold")
        ).grid(row=0, column=0, columnspan=2, sticky="w", padx=10, pady=(10, 5))

        # Core Clock Rows
        ctk.CTkLabel(core_lock_frame, text="Min/MHz:").grid(
            row=1, column=0, sticky="w", padx=10, pady=3
        )
        self.core_lock_min_var = ctk.StringVar(value="0")
        LiteEntry(
            core_lock_frame,
            textvariable=self.core_lock_min_var,
            width=7,
            min_px=52,
            justify="right",
        ).grid(row=1, column=1, sticky="w", padx=10, pady=3)

        ctk.CTkLabel(core_lock_frame, text="Max/MHz:").grid(
            row=2, column=0, sticky="w", padx=10, pady=3
        )
        self.core_lock_max_var = ctk.StringVar(value="0")
        LiteEntry(
            core_lock_frame,
            textvariable=self.core_lock_max_var,
            width=7,
            min_px=52,
            justify="right",
        ).grid(row=2, column=1, sticky="w", padx=10, pady=3)

        # Combine "Lock Core" and "Reset" buttons into a single cell
        button_frame_core = ctk.CTkFrame(core_lock_frame)
        button_frame_core.grid(
            row=3, column=0, columnspan=2, sticky="ew", padx=10, pady=(5, 10)
        )  # Span across both columns

        btn_core_lock = LiteButton(
            button_frame_core, text="🔒Lock", width=70, command=self._lock_core_clocks
        )
        btn_core_lock.pack(side="left", fill="x", padx=0)

        btn_core_reset = LiteButton(
            button_frame_core, text="🔓Reset", width=75, command=self._reset_core_clocks
        )
        btn_core_reset.pack(side="right", fill="x", padx=0)

        # === Column 4: Memory Clock ===
        mem_frame = ctk.CTkFrame(top_split_frame)
        mem_frame.grid(row=0, column=3, sticky="nsew", padx=(5, 10), pady=5)
        mem_frame.columnconfigure(0, weight=1)
        mem_frame.columnconfigure(1, weight=1)
        ctk.CTkLabel(mem_frame, text="⚙Mem Freq Lock", font=("", 14, "bold")).grid(
            row=0, column=0, columnspan=2, sticky="w", padx=10, pady=(10, 5)
        )

        # Memory Clock Rows
        ctk.CTkLabel(mem_frame, text="Min (MHz):").grid(
            row=1, column=0, sticky="w", padx=10, pady=3
        )
        self.mem_lock_min_var = ctk.StringVar(value="0")
        LiteEntry(
            mem_frame,
            textvariable=self.mem_lock_min_var,
            width=7,
            min_px=52,
            justify="right",
        ).grid(row=1, column=1, sticky="w", padx=10, pady=3)

        ctk.CTkLabel(mem_frame, text="Max (MHz):").grid(
            row=2, column=0, sticky="w", padx=10, pady=3
        )
        self.mem_lock_max_var = ctk.StringVar(value="0")
        LiteEntry(
            mem_frame,
            textvariable=self.mem_lock_max_var,
            width=7,
            min_px=52,
            justify="right",
        ).grid(row=2, column=1, sticky="w", padx=10, pady=3)
        button_frame_mem = ctk.CTkFrame(
            mem_frame
        )  # Create a separate frame for the buttons
        button_frame_mem.grid(
            row=3, column=0, columnspan=2, sticky="ew", padx=10, pady=(5, 10)
        )  # Span across both columns

        btn_mem_lock = LiteButton(
            button_frame_mem,
            text="🔒Lock",
            width=70,
            command=self._lock_mem_clocks,
        )
        btn_mem_lock.pack(side="left", fill="x", padx=0)

        btn_mem_reset = LiteButton(
            button_frame_mem,
            text="🔓Reset",
            width=75,
            command=self._reset_mem_clocks,
        )
        btn_mem_reset.pack(side="right", fill="x", padx=0)

        # === Export / Import ===
        ei_frame = ctk.CTkFrame(scroll)
        ei_frame.pack(fill="x", pady=(0, 10))
        ctk.CTkLabel(
            ei_frame, text="📂 Export / Import VF Curve", font=("", 14, "bold")
        ).pack(anchor="w", padx=10, pady=(10, 5))

        grid = ctk.CTkFrame(ei_frame, fg_color="transparent")
        grid.pack(fill="x", padx=10, pady=(0, 10))
        grid.columnconfigure(1, weight=0)

        ctk.CTkLabel(grid, text="File Path:").grid(
            row=0, column=0, sticky="w", padx=5, pady=3
        )
        self.csv_path_var = ctk.StringVar(value="")
        path_row = ctk.CTkFrame(grid, fg_color="transparent")
        path_row.grid(row=0, column=1, sticky="ew", padx=5, pady=3)
        path_entry = LiteEntry(
            path_row,
            textvariable=self.csv_path_var,
            width=52,
            min_px=420,
            justify="left",
        )
        path_entry.pack(side="left")
        LiteButton(path_row, text="...", width=34, command=self._browse_csv).pack(
            side="left", padx=(5, 0)
        )

        self.use_default_path_var = ctk.BooleanVar(value=False)
        ctk.CTkCheckBox(
            path_row, text="Use for I/O", variable=self.use_default_path_var, width=80
        ).pack(side="left", padx=(10, 0))

        self.quick_export_var = ctk.BooleanVar(value=True)
        ctk.CTkCheckBox(
            grid, text="Quick export (skip load curve)", variable=self.quick_export_var
        ).grid(row=1, column=0, columnspan=2, sticky="w", padx=5, pady=3)

        btn_ei = ctk.CTkFrame(ei_frame, fg_color="transparent")
        btn_ei.pack(fill="x", padx=10, pady=(0, 10))
        LiteButton(
            btn_ei, text="📤 Export VFP", width=130, command=self._export_vfp
        ).pack(side="left", padx=5)
        LiteButton(
            btn_ei, text="📥 Import VFP", width=130, command=self._import_vfp
        ).pack(side="left", padx=5)
        LiteButton(
            btn_ei,
            text="🔁 Reset VFP",
            width=140,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_vfp,
        ).pack(side="left", padx=5)

    # ────────────────────────────────────────────
    # Chart setup
    # ────────────────────────────────────────────
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

        self.fig = Figure(figsize=(9, 2.45), dpi=fig_dpi)
        self.fig.patch.set_facecolor("#2b2b2b")
        self.ax = self.fig.add_subplot(111)
        # Reserve enough left margin so the Y-axis label is never clipped
        self.fig.subplots_adjust(left=0.11, right=0.98, top=0.95, bottom=0.18)
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

        # Resize figure width when the parent frame width changes.
        # Height is kept fixed (3.5 in) so controls below are never squeezed out.
        parent.bind("<Configure>", self._on_chart_resize, add="+")

        # Plot line references (created on first data load)
        self._line_current = None
        self._line_default = None
        self._sel_rect = None  # selection highlight
        self._sel_points = None  # selected point markers

        # Connect mouse events
        self.canvas.mpl_connect("button_press_event", self._on_mouse_press)
        self.canvas.mpl_connect("button_release_event", self._on_mouse_release)
        self.canvas.mpl_connect("motion_notify_event", self._on_mouse_move)

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
        ax = self.ax
        ax.set_facecolor("#1e1e1e")
        ax.set_xlabel("Voltage (mV)", color="#e08020", fontsize=8)
        ax.set_ylabel("Frequency (MHz)", color="#e08020", fontsize=8, labelpad=10)
        ax.tick_params(colors="#cccccc", labelsize=6)
        for spine in ax.spines.values():
            spine.set_color("#555555")
        ax.grid(True, color="#333333", linewidth=0.5, alpha=0.7)

    # ────────────────────────────────────────────
    # Data loading
    # ────────────────────────────────────────────
    def _get_csv_path(self) -> str:
        """Return the CSV cache path for the current GPU (by UUID)."""
        app_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        cache_dir = os.path.join(app_dir, self._EXPORT_DIR)
        os.makedirs(cache_dir, exist_ok=True)

        uuid = self.app.get_current_gpu_uuid()
        if uuid:
            fname = f"{uuid}.csv"
        else:
            idx = self.app.get_current_gpu_index()
            fname = f"gpu_{idx if idx is not None else 0}.csv"
        return os.path.join(cache_dir, fname)

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

        gpu_args = self.app.get_gpu_args()
        if not gpu_args:
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

        self._refresh_curve()

    def _refresh_curve(self):
        """Export VFP from GPU then load and plot it."""
        if self._refresh_curve_inflight:
            self._refresh_curve_pending = True
            return

        csv_path = self._get_csv_path()
        gpu_args = self.app.get_gpu_args()
        if not gpu_args:
            self.app.console.append("[GUI] No GPU selected.\n")
            return

        self._refresh_curve_inflight = True
        self.app.console.append("[GUI] Exporting VF curve...\n")

        def _worker():
            runner = CLIRunner(
                self.app.runner.exe_path, on_output=self.app._on_cli_output
            )
            args = gpu_args + ["set", "vfp", "export", "-q", csv_path]
            retcode, _ = runner.run_sync(args, cwd=self.app.cli_cwd)
            self.app.after(0, lambda: self._on_export_done(retcode, csv_path))

        threading.Thread(target=_worker, daemon=True).start()

    def _on_export_done(self, retcode: int, csv_path: str):
        self._refresh_curve_inflight = False
        if retcode != 0:
            self.app.console.append("[GUI] VFP export failed.\n")
        else:
            self.app.console.append(f"[GUI] VFP exported to {csv_path}\n")
            self._load_csv(csv_path)

        if self._refresh_curve_pending:
            self._refresh_curve_pending = False
            self.app.after(0, self._refresh_curve)
        elif self._auto_refreshing:
            self._schedule_next_auto_refresh()

    def _load_csv(self, path: str):
        """Parse CSV and redraw chart."""
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

        self._voltages = voltages
        self._frequencies = frequencies
        self._defaults = defaults
        self._sel_start = None
        self._sel_end = None
        self._drag_orig_freqs = None

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
        analyze_vfp_offsets = getattr(self.app, "_analyze_vfp_offsets", None)
        if callable(analyze_vfp_offsets):
            has_vfp_offset, uniform_core_offset_mhz = analyze_vfp_offsets(
                frequencies, defaults
            )
        else:
            has_vfp_offset = any(
                abs(f - d) > 1e-4 for f, d in zip(frequencies, defaults)
            )
            uniform_core_offset_mhz = None
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

        # Default curve (dashed)
        (self._line_default,) = ax.plot(
            v,
            d,
            color="#888888",
            linestyle="--",
            linewidth=0.9,
            label="Default",
            zorder=2,
        )

        # Current curve (solid with point markers)
        (self._line_current,) = ax.plot(
            v,
            f,
            color="#00ccff",
            linestyle="-",
            linewidth=1.1,
            marker="s",
            markersize=0.9,
            markerfacecolor="#00ccff",
            markeredgecolor="#00ccff",
            label="Current",
            zorder=3,
        )

        # Selection highlight
        if self._sel_start is not None and self._sel_end is not None:
            s = min(self._sel_start, self._sel_end)
            e = max(self._sel_start, self._sel_end)
            sel_v = v[s : e + 1]
            sel_f = f[s : e + 1]

            # Shaded region
            ax.axvspan(v[s], v[e], alpha=0.15, color="#ffcc00", zorder=1)

            # Highlighted points
            self._sel_points = ax.scatter(
                sel_v,
                sel_f,
                color="#ffcc00",
                s=14,
                zorder=5,
                edgecolors="#ff8800",
                linewidths=0.6,
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

        # Axis range with some padding
        v_min, v_max = min(v), max(v)
        all_f = f + d
        f_min, f_max = min(f), max(all_f)

        # Adjust Y limits if freq lock is outside default range
        if self._freq_core_lock is not None:
            cmin, cmax = self._freq_core_lock
            f_min = min(f_min, cmin)
            f_max = max(f_max, cmax)

        f_pad = max(150, (f_max - f_min) * 0.18)
        ax.set_xlim(v_min - 1, v_max + 1)
        ax.set_ylim(f_min - f_pad, f_max + f_pad)

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

        # Keep fixed margins so Y-axis label is never clipped
        self.fig.subplots_adjust(left=0.11, right=0.98, top=0.95, bottom=0.18)

        self._live_elements.clear()
        self._draw_live_point(call_draw_idle=False)
        self.canvas.draw_idle()

    # ────────────────────────────────────────────
    # Mouse interaction
    # ────────────────────────────────────────────
    def _find_nearest_index(self, x_data: float) -> Optional[int]:
        """Find the index of the VF point closest to x_data (mV)."""
        if not self._voltages:
            return None
        arr = np.array(self._voltages)
        idx = int(np.argmin(np.abs(arr - x_data)))
        return idx

    def _on_mouse_press(self, event):
        if event.inaxes != self.ax or not self._voltages:
            return

        if event.button == 1:  # Left click = start selection / drag
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
                    self._drag_orig_freqs = np.array(self._frequencies, dtype=float)
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
                offsets = np.column_stack([self._voltages[s : e + 1], sel_f])
                self._sel_points.set_offsets(offsets)
            self.canvas.draw_idle()
            return

        # Selection drag (extending selection while mouse button held)
        if (
            event.button == 1
            and event.inaxes == self.ax
            and self._sel_start is not None
            and not self._dragging
        ):
            idx = self._find_nearest_index(event.xdata)
            if idx is not None and idx != self._sel_end:
                self._sel_end = idx
                self._redraw()

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
        changed = new_core_lock != self._freq_core_lock

        self._freq_core_lock = new_core_lock
        self._freq_mem_lock = new_mem_lock
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
        self._freq_core_lock = None
        self.core_lock_min_var.set("0")
        self.core_lock_max_var.set("0")
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
        self._live_volt = volt_mv
        self._live_freq = freq_mhz
        if self._is_resize_active:
            self._pending_live_point = (volt_mv, freq_mhz)
            return
        self._draw_live_point()

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

        if self._pending_live_point is not None:
            self._live_volt, self._live_freq = self._pending_live_point
            self._pending_live_point = None
            self._draw_live_point()

    def _draw_live_point(self, call_draw_idle: bool = True):
        # Remove previously drawn live elements
        for el in self._live_elements:
            try:
                el.remove()
            except Exception:
                pass
        self._live_elements.clear()

        if self._live_volt is None or self._live_freq is None or not self._voltages:
            if call_draw_idle:
                self.canvas.draw_idle()
            return

        lv = self._live_volt
        lf = self._live_freq
        ax = self.ax

        crosshair_kw = dict(color="#22cc44", linewidth=1.0, linestyle="--", alpha=0.85)

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
        )

        self._live_elements.extend([hline, vline, marker, text])

        if call_draw_idle:
            self.canvas.draw_idle()

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

    def _key_shift_freq(self, delta_mhz: float):
        """Shift the frequency of the currently selected point(s) by delta_mhz."""
        if self._sel_start is None or self._sel_end is None:
            return
        s = min(self._sel_start, self._sel_end)
        e = max(self._sel_start, self._sel_end)

        # Save undo snapshot before first edit in a batch
        if self._drag_orig_freqs is None:
            self._drag_orig_freqs = np.array(self._frequencies, dtype=float)

        for i in range(s, e + 1):
            self._frequencies[i] = round(self._frequencies[i] + delta_mhz, 3)

        self._sync_selection_to_adj()
        self._redraw()

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
        gpu_args = self.app.get_gpu_args()
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

        def _worker():
            # Use an isolated runner inside the background thread so composite operations execute sequentially
            runner = CLIRunner(
                self.app.runner.exe_path, on_output=self.app._on_cli_output
            )

            try:
                if s == e:
                    # Single point selected
                    if is_vfp_locked:
                        # VFP Lock -> selected frequency lock backend
                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Unlocking VFP point {idx}...\n"
                            ),
                        )
                        runner.run_sync(
                            gpu_args + ["set", "nvapi", "--reset-volt-locks"],
                            cwd=self.app.cli_cwd,
                        )

                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Applying {lock_backend_label} lock for point {idx} ({cur_f} MHz)....\n"
                            ),
                        )
                        rc, _ = runner.run_sync(
                            gpu_args
                            + [
                                "set",
                                lock_backend,
                                "--locked-core-clocks",
                                str(cur_f),
                                str(cur_f),
                            ],
                            cwd=self.app.cli_cwd,
                        )

                        def _update_freq_lock(result_rc=rc, local_f=cur_f):
                            self._locked_points.clear()
                            if result_rc == 0:
                                self._freq_core_lock = (local_f, local_f)
                                self.core_lock_min_var.set(str(local_f))
                                self.core_lock_max_var.set(str(local_f))
                                self.app.console.append(
                                    f"[GUI] ✔ {lock_backend_label} lock applied successfully.\n"
                                )
                            else:
                                self.app.console.append(
                                    f"[GUI] ❌ {lock_backend_label} lock failed (rc={result_rc}), UI falling back.\n"
                                )
                            self._redraw()
                            self.canvas.draw()  # Force immediate UI paint

                        self.app.after(0, _update_freq_lock)

                    elif is_freq_locked_single:
                        # Selected frequency lock backend -> Unlock
                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Resetting {lock_backend_label} lock...\n"
                            ),
                        )
                        rc, _ = runner.run_sync(
                            gpu_args + ["set", lock_backend, "--reset-core-clocks"],
                            cwd=self.app.cli_cwd,
                        )

                        def _update_unlock(result_rc=rc):
                            if result_rc == 0:
                                self._freq_core_lock = None
                                self.core_lock_min_var.set("0")
                                self.core_lock_max_var.set("0")
                                self.app.console.append(
                                    f"[GUI] ✔ {lock_backend_label} lock reset successfully.\n"
                                )
                            else:
                                self.app.console.append(
                                    f"[GUI] ❌ {lock_backend_label} lock reset failed (rc={result_rc}).\n"
                                )
                            self._redraw()
                            self.canvas.draw()  # Force immediate UI paint

                        self.app.after(0, _update_unlock)

                    else:
                        # Unlock -> VFP Lock
                        # Always clear selected freq lock backend via CLI to be safe
                        runner.run_sync(
                            gpu_args + ["set", lock_backend, "--reset-core-clocks"],
                            cwd=self.app.cli_cwd,
                        )

                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Locking point {idx} via VFP...\n"
                            ),
                        )
                        rc, _ = runner.run_sync(
                            gpu_args + ["set", "nvapi", "--locked-voltage", str(idx)],
                            cwd=self.app.cli_cwd,
                        )

                        def _update_vfp(
                            result_rc=rc, local_idx=idx, local_vol=vol, local_f=cur_f
                        ):
                            self._freq_core_lock = None
                            self.core_lock_min_var.set("0")
                            self.core_lock_max_var.set("0")
                            if result_rc == 0:
                                self._locked_points.clear()
                                self._locked_points.add(local_idx)
                                self.app.console.append(
                                    f"[GUI] ✔ VFP Lock applied ({local_vol:.1f} mV / {local_f} MHz).\n"
                                )
                            else:
                                self.app.console.append(
                                    f"[GUI] ❌ VFP Lock failed (rc={result_rc}). UI fallback bypassed.\n"
                                )
                                # If backend success is not reflected in return code, keep UI in sync with user-observed state.
                                self._locked_points.clear()
                                self._locked_points.add(local_idx)
                            self._redraw()
                            self.canvas.draw()  # Force immediate UI paint

                        self.app.after(0, _update_vfp)

                else:
                    # Range selected
                    if is_freq_locked_range:
                        # Unlock selected frequency lock backend
                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Resetting {lock_backend_label} range lock...\n"
                            ),
                        )
                        rc, _ = runner.run_sync(
                            gpu_args + ["set", lock_backend, "--reset-core-clocks"],
                            cwd=self.app.cli_cwd,
                        )

                        def _update_range_unlock(result_rc=rc):
                            if result_rc == 0:
                                self._freq_core_lock = None
                                self.core_lock_min_var.set("0")
                                self.core_lock_max_var.set("0")
                                self.app.console.append(
                                    f"[GUI] ✔ {lock_backend_label} range lock reset.\n"
                                )
                            else:
                                self.app.console.append(
                                    f"[GUI] ❌ {lock_backend_label} range unlock failed (rc={result_rc}).\n"
                                )
                            self._redraw()
                            self.canvas.draw()  # Force immediate UI paint

                        self.app.after(0, _update_range_unlock)
                    else:
                        # Lock selected frequency backend range
                        if has_vfp_locks:
                            runner.run_sync(
                                gpu_args + ["set", "nvapi", "--reset-volt-locks"],
                                cwd=self.app.cli_cwd,
                            )

                        self.app.after(
                            0,
                            lambda: self.app.console.append(
                                f"[GUI] Applying {lock_backend_label} lock for range {s}-{e} ({min_f} - {max_f} MHz)....\n"
                            ),
                        )
                        rc, _ = runner.run_sync(
                            gpu_args
                            + [
                                "set",
                                lock_backend,
                                "--locked-core-clocks",
                                str(min_f),
                                str(max_f),
                            ],
                            cwd=self.app.cli_cwd,
                        )

                        def _update_range_lock(
                            result_rc=rc, lf_min=min_f, lf_max=max_f
                        ):
                            self._locked_points.clear()
                            if result_rc == 0:
                                self._freq_core_lock = (lf_min, lf_max)
                                self.core_lock_min_var.set(str(lf_min))
                                self.core_lock_max_var.set(str(lf_max))
                                self.app.console.append(
                                    f"[GUI] ✔ {lock_backend_label} range lock applied.\n"
                                )
                            else:
                                self.app.console.append(
                                    f"[GUI] ❌ {lock_backend_label} range lock failed (rc={result_rc}).\n"
                                )
                                # Fallback UI for potential false negative returns
                                self._freq_core_lock = (lf_min, lf_max)
                                self.core_lock_min_var.set(str(lf_min))
                                self.core_lock_max_var.set(str(lf_max))
                            self._redraw()
                            self.canvas.draw()  # Force immediate UI paint

                        self.app.after(0, _update_range_lock)
            finally:
                self.app.after(0, lambda: setattr(self, "_is_toggling_lock", False))

        threading.Thread(target=_worker, daemon=True, name="space-toggle").start()
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
    # Actions (CLI calls)
    # ────────────────────────────────────────────
    def _browse_csv(self):
        path = filedialog.asksaveasfilename(
            title="Select CSV File Path",
            defaultextension=".csv",
            filetypes=[("CSV Files", "*.csv"), ("All Files", "*.*")],
        )
        if path:
            self.csv_path_var.set(path)

    def _export_vfp(self):
        gpu_args = self.app.get_gpu_args()

        if self.use_default_path_var.get():
            path = self.csv_path_var.get().strip()
            if not path:
                path = self._get_csv_path()
                self.csv_path_var.set(path)
        else:
            path = filedialog.asksaveasfilename(
                title="Export VF Curve",
                defaultextension=".csv",
                filetypes=[("CSV Files", "*.csv"), ("All Files", "*.*")],
            )
            if not path:
                return

        args = gpu_args + ["set", "vfp", "export", path]
        if self.quick_export_var.get():
            args.append("-q")
        self.app.run_cli_display(args)

    def _import_vfp(self):
        gpu_args = self.app.get_gpu_args()

        if self.use_default_path_var.get():
            path = self.csv_path_var.get().strip()
            if not path:
                self.app.console.append("[GUI] No CSV path specified.\n")
                return
        else:
            path = filedialog.askopenfilename(
                title="Import VF Curve",
                defaultextension=".csv",
                filetypes=[("CSV Files", "*.csv"), ("All Files", "*.*")],
            )
            if not path:
                return

        args = gpu_args + ["set", "vfp", "import", path]
        self.app.run_cli(
            args, on_finished=lambda _rc: self.app.after(0, self._refresh_curve)
        )

    def _lock_vfp(self):
        gpu_args = self.app.get_gpu_args()
        val = self.lock_point_var.get()
        if self.lock_voltage_var.get():
            args = gpu_args + ["set", "nvapi", "--locked-voltage", f"{val}mV"]
        else:
            args = gpu_args + ["set", "nvapi", "--locked-voltage", val]

        lock_idx = self._resolve_vfp_lock_idx_from_input()

        def _on_finished(rc: int, idx=lock_idx):
            def _update_ui():
                self._apply_vfp_lock_ui(idx)

            self.app.after(0, _update_ui)

        self.app.run_cli(args, on_finished=_on_finished)

    def _unlock_vfp(self):
        gpu_args = self.app.get_gpu_args()

        def _on_finished(rc: int):
            def _update_ui():
                if rc == 0:
                    self._apply_vfp_unlock_ui()

            self.app.after(0, _update_ui)

        self.app.run_cli(
            gpu_args + ["set", "nvapi", "--reset-volt-locks"], on_finished=_on_finished
        )

    def _lock_core_clocks(self):
        if getattr(self, "_is_toggling_lock", False):
            self.app.console.append("[GUI] Operation in progress. Please wait...\n")
            return

        try:
            min_clk = int(self.core_lock_min_var.get().strip())
            max_clk = int(self.core_lock_max_var.get().strip())
        except ValueError:
            self.app.console.append("[GUI] Invalid min/max core clock values.\n")
            return

        if min_clk > max_clk:
            min_clk, max_clk = max_clk, min_clk

        self._is_toggling_lock = True
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_freq_lock_backend()
        backend_label = self._selected_freq_lock_backend_label()
        args = gpu_args + [
            "set",
            backend,
            "--locked-core-clocks",
            str(min_clk),
            str(max_clk),
        ]
        self.app.console.append(
            f"[GUI] Locking {backend_label} core clocks to {min_clk} - {max_clk} MHz...\n"
        )
        self.app.run_cli(
            args,
            on_finished=lambda rc, label=backend_label: self._on_core_lock_done(
                rc, min_clk, max_clk, label
            ),
        )

    def _on_core_lock_done(
        self, rc: int, min_clk: int, max_clk: int, backend_label: str
    ):
        def _update_ui():
            if rc == 0:
                self._freq_core_lock = (min_clk, max_clk)
                self.core_lock_min_var.set(str(min_clk))
                self.core_lock_max_var.set(str(max_clk))
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

    def _reset_core_clocks(self):
        if getattr(self, "_is_toggling_lock", False):
            self.app.console.append("[GUI] Operation in progress. Please wait...\n")
            return

        self._is_toggling_lock = True
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_freq_lock_backend()
        backend_label = self._selected_freq_lock_backend_label()
        args = gpu_args + ["set", backend, "--reset-core-clocks"]
        self.app.console.append(f"[GUI] Resetting {backend_label} core clocks...\n")
        self.app.run_cli(
            args,
            on_finished=lambda rc, label=backend_label: self._on_core_reset_done(
                rc, label
            ),
        )

    def _on_core_reset_done(self, rc: int, backend_label: str):
        def _update_ui():
            if rc == 0:
                self._freq_core_lock = None
                self.core_lock_min_var.set("0")
                self.core_lock_max_var.set("0")
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

    def _lock_mem_clocks(self):
        try:
            min_clk = int(self.mem_lock_min_var.get().strip())
            max_clk = int(self.mem_lock_max_var.get().strip())
        except ValueError:
            self.app.console.append("[GUI] Invalid min/max memory clock values.\n")
            return

        if min_clk > max_clk:
            min_clk, max_clk = max_clk, min_clk

        gpu_args = self.app.get_gpu_args()
        backend = self._selected_freq_lock_backend()
        backend_label = self._selected_freq_lock_backend_label()
        args = gpu_args + [
            "set",
            backend,
            "--locked-mem-clocks",
            str(min_clk),
            str(max_clk),
        ]
        self.app.console.append(
            f"[GUI] Locking {backend_label} memory clocks to {min_clk} - {max_clk} MHz...\n"
        )
        self.app.run_cli_display(args)

    def _reset_mem_clocks(self):
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_freq_lock_backend()
        backend_label = self._selected_freq_lock_backend_label()
        args = gpu_args + ["set", backend, "--reset-mem-clocks"]
        self.app.console.append(f"[GUI] Resetting {backend_label} memory clocks...\n")
        self.app.run_cli_display(args)

    def _apply_adj(self):
        """Apply the current frequency edits for the selected range to the GPU.

        Uses the Delta (MHz) field as the target offset vs default for the
        selected range, updates the in-memory curve, then groups consecutive
        equal-delta points and runs pointwiseoc calls sequentially.
        """
        gpu_args = self.app.get_gpu_args()
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

        n = len(self._frequencies)
        start = max(0, min(start, n - 1))
        end = max(0, min(end, n - 1))

        if self._drag_orig_freqs is None or len(self._drag_orig_freqs) != len(
            self._frequencies
        ):
            self._drag_orig_freqs = np.array(self._frequencies, dtype=float)

        for i in range(start, end + 1):
            self._frequencies[i] = round(self._defaults[i] + target_delta_mhz, 3)

        self._redraw()

        # Build per-point delta list (kHz, integer)
        deltas_khz = [
            round((self._frequencies[i] - self._defaults[i]) * 1000)
            for i in range(start, end + 1)
        ]

        # Group consecutive points with identical delta → fewer CLI calls
        groups = []  # type: List[Tuple[int, int, int]]  # (from_idx, to_idx, delta_khz)
        g_start = start
        g_delta = deltas_khz[0]
        for offset, dkz in enumerate(deltas_khz[1:], start=1):
            if dkz != g_delta:
                groups.append((g_start, start + offset - 1, g_delta))
                g_start = start + offset
                g_delta = dkz
        groups.append((g_start, end, g_delta))

        self.app.console.append(
            f"[GUI] Applying {len(groups)} pointwiseoc group(s) "
            f"for range {start}–{end}…\n"
        )

        def _worker():
            runner = CLIRunner(self.app.runner.exe_path, on_output=lambda _: None)
            for frm, to, dkz in groups:
                sign = "+" if dkz >= 0 else ""
                args = gpu_args + [
                    "set",
                    "vfp",
                    "pointwiseoc",
                    f"{frm}-{to}",
                    f"{sign}{dkz}",
                ]
                self.app.console.append(
                    f"[GUI]   pointwiseoc {frm}-{to} {sign}{dkz} kHz\n"
                )
                retcode, _ = runner.run_sync(args, cwd=self.app.cli_cwd)
                if retcode != 0:
                    self.app.console.append(
                        f"[GUI] ⚠ pointwiseoc {frm}-{to} failed (rc={retcode})\n"
                    )
            # Refresh curve on main thread when all groups are done
            self.app.after(0, self._refresh_curve)

        threading.Thread(target=_worker, daemon=True, name="apply-adj").start()

    def _reset_vfp(self):
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli(
            gpu_args + ["reset", "vfp"],
            on_finished=lambda _rc: self.app.after(0, self._refresh_curve),
        )
