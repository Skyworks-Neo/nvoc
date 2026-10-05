"""
Stress Tool panel — the VF Curve tab's lower half.

Replaces the former autoscan section with a front end for the standalone
``cli-stressor-cuda-rs`` executable: it builds a stressor command line from
its controls, runs it through the app's shared CLI runner (console window +
single-command guard) and mirrors the verdict line the tool prints.

Every control is gated on the ``--help`` capability probe of the selected
executable (see ``stressor_args``): with a main-lineage build the verify/SDC
and the extra Vulkan families stay greyed out instead of producing clap
errors, with an enhanced build they light up.
"""

import os
import tkinter as tk
from tkinter import filedialog
from typing import TYPE_CHECKING, Any, Dict, List, Optional, Set, Tuple

import customtkinter as ctk

from src.config import DEFAULT_CONFIG
from src.widgets.lightweight_controls import (
    ct_button_font,
    install_mousewheel_support,
    LiteButton,
    LiteCheckbutton,
    LiteEntry,
)
from .stressor_args import (
    build_stressor_args,
    discover_stressor_exe,
    flag_for,
    format_exit_status,
    probe_stressor_flags,
    summarize_line,
)

# De-CTk'd palette (matches overclock.py / fan_control.py)
_PANE_BG = "#2b2b2b"
_TEXT_FG = "#e5e5e5"
_TEXT_FG_DIM = "#8a8a8a"
_TEXT_OK = "#5fd07a"
_TEXT_BAD = "#ef6b5e"
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")
_FONT_SMALL = ("Segoe UI", 10)
_SECTION_BORDER = "#1f4e79"

_DEFAULT_SENTINEL = "(default)"
_PROFILE_VALUES = (_DEFAULT_SENTINEL, "standard", "low-vram", "40-50", "dynamic-export")
_STREAM_VALUES = (_DEFAULT_SENTINEL, "single", "dual", "triple")
_GATED_HINT = (
    "Options the selected executable does not advertise stay greyed out — "
    "the Verify/SDC switches and the extra Vulkan knobs need an enhanced "
    "stressor build."
)

if TYPE_CHECKING:
    from src.app import App


class StressorPanel:
    """Stress-tool front end for cli-stressor-cuda-rs."""

    def __init__(self, parent: ctk.CTkFrame, app: "App") -> None:
        self.app = app
        self.frame = parent
        self._is_resize_active = False
        self._pending_run_button_state: Optional[Tuple[bool, bool]] = None
        self._capabilities: Optional[Set[str]] = None
        self._probe_generation = 0
        self._vars: Dict[str, Any] = {}
        self._gated: List[Tuple[str, Any, Optional[tk.Label]]] = []
        self._stored = self._read_stored()

        self._exe_var = ctk.StringVar(value="")
        self._exe_var.trace_add("write", lambda *_: self._persist())

        self._build_run_bar()
        scroll = ctk.CTkScrollableFrame(self.frame)
        scroll.pack(fill="both", expand=True, padx=10, pady=(10, 6))
        install_mousewheel_support(scroll)
        self._build_target_card(scroll)
        self._build_parameters_card(scroll)
        self._build_verify_card(scroll)
        self._build_vulkan_card(scroll)

        self._seed_exe_path()
        self.app.register_resize_target(self)
        self._refresh_gpu_label()
        gpu_var = getattr(self.app, "gpu_var", None)
        if gpu_var is not None and hasattr(gpu_var, "trace_add"):
            gpu_var.trace_add("write", lambda *_: self._refresh_gpu_label())
        self._schedule_probe()

    # ── configuration plumbing ─────────────────────────────────────────────

    def _read_stored(self) -> Dict[str, Any]:
        try:
            stored = self.app.config.get("stressor", {})
        except Exception:
            stored = {}
        return dict(stored) if isinstance(stored, dict) else {}

    def _default(self, key: str) -> Any:
        return DEFAULT_CONFIG.get("stressor", {}).get(key, "")

    def _value(self, key: str) -> Any:
        return self._stored.get(key, self._default(key))

    def _persist(self) -> None:
        """Merge the panel state over the stored block (last used values)."""
        payload = dict(self._stored)
        payload["exe_path"] = self._exe_path()
        for key, var in self._vars.items():
            try:
                payload[key] = var.get()
            except tk.TclError:
                continue
        self._stored = payload
        try:
            self.app.config.set("stressor", payload)
        except Exception:
            pass

    # ── small widget factories ─────────────────────────────────────────────

    def _label(
        self, parent: Any, text: str, *, dim: bool = False, small: bool = False
    ) -> tk.Label:
        return tk.Label(
            parent,
            text=text,
            font=_FONT_SMALL if small else _FONT_BODY,
            bg=_PANE_BG,
            fg=_TEXT_FG_DIM if dim else _TEXT_FG,
        )

    def _card(self, parent: Any, title: str) -> tk.Frame:
        card = ctk.CTkFrame(
            parent, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        card.pack(fill="x", pady=(0, 10))
        tk.Label(card, text=title, font=_FONT_HEADER, bg=_PANE_BG, fg=_TEXT_FG).pack(
            anchor="w", padx=10, pady=(10, 5)
        )
        body = tk.Frame(card, bg=_PANE_BG)
        body.pack(fill="x", padx=10, pady=(0, 10))
        return body

    def _mk_var(self, key: str) -> Any:
        default = self._default(key)
        stored = self._value(key)
        if isinstance(default, bool):
            var: Any = ctk.BooleanVar(value=bool(stored))
        else:
            # A value-taking option can inherit a stale bool from an older
            # config block (vulkan_particles used to be a checkbox); treat it
            # as unset instead of forwarding "False" to clap.
            if isinstance(stored, bool):
                stored = ""
            var = ctk.StringVar(value="" if stored is None else str(stored))
        self._vars[key] = var
        var.trace_add("write", lambda *_: self._persist())
        return var

    def _register_gated(
        self, key: str, widget: Any, label: Optional[tk.Label] = None
    ) -> None:
        self._gated.append((key, widget, label))

    def _entry_cell(
        self,
        grid: tk.Frame,
        row: int,
        col: int,
        text: str,
        key: str,
        *,
        width: int = 12,
        min_px: int = 96,
        gated: bool = False,
    ) -> LiteEntry:
        label = self._label(grid, text)
        label.grid(row=row, column=col * 2, sticky="w", padx=(5, 4), pady=3)
        entry = LiteEntry(
            grid,
            textvariable=self._mk_var(key),
            width=width,
            min_px=min_px,
            justify="left",
        )
        entry.grid(row=row, column=col * 2 + 1, sticky="w", padx=(0, 14), pady=3)
        if gated:
            self._register_gated(key, entry, label)
        return entry

    def _check_cell(
        self,
        grid: tk.Frame,
        row: int,
        col: int,
        text: str,
        key: str,
        *,
        gated: bool = False,
        columnspan: int = 1,
    ) -> LiteCheckbutton:
        chk = LiteCheckbutton(
            grid,
            text=text,
            variable=self._mk_var(key),
            font=_FONT_BODY,
            bg=_PANE_BG,
            fg=_TEXT_FG,
        )
        chk.grid(
            row=row,
            column=col * 2,
            columnspan=columnspan,
            sticky="w",
            padx=(5, 4),
            pady=3,
        )
        if gated:
            self._register_gated(key, chk)
        return chk

    def _menu_cell(
        self,
        grid: tk.Frame,
        row: int,
        col: int,
        text: str,
        key: str,
        values: Tuple[str, ...],
        *,
        width: int = 140,
    ) -> ctk.CTkOptionMenu:
        label = self._label(grid, text)
        label.grid(row=row, column=col * 2, sticky="w", padx=(5, 4), pady=3)
        menu = ctk.CTkOptionMenu(
            grid,
            variable=self._mk_var(key),
            values=list(values),
            width=width,
            anchor="center",
            font=ct_button_font(grid),
        )
        menu.grid(row=row, column=col * 2 + 1, sticky="w", padx=(0, 14), pady=3)
        return menu

    # ── Run bar (pinned to the bottom, always reachable) ───────────────────

    def _build_run_bar(self) -> None:
        card = ctk.CTkFrame(
            self.frame, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        card.pack(side="bottom", fill="x", padx=10, pady=(0, 10))

        row1 = tk.Frame(card, bg=_PANE_BG)
        row1.pack(fill="x", padx=10, pady=(10, 6))
        self.start_btn = LiteButton(
            row1,
            text="▶ Start Stress",
            width=160,
            fg_color="#2d8a4e",
            hover_color="#236b3c",
            command=self._start_run,
        )
        self.start_btn.pack(side="left")
        self.stop_btn = LiteButton(
            row1,
            text="⏹ Stop",
            width=100,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._stop_run,
        )
        self.stop_btn.configure(state="disabled")
        self.stop_btn.pack(side="left", padx=(6, 0))
        self._status_lbl = self._label(row1, "idle", dim=True)
        self._status_lbl.pack(side="left", padx=(12, 0))

        # Same quick-access row as the dashboard's bottom strip, so the stress
        # console stays one click away from this tab.
        row2 = tk.Frame(card, bg=_PANE_BG)
        row2.pack(fill="x", padx=10, pady=(0, 8))
        for col in range(4):
            row2.grid_columnconfigure(col, weight=1, uniform="stress_quick")
        LiteButton(
            row2,
            text="🔄 Refresh Info",
            width=10,
            command=lambda: self._dashboard_action("_refresh_info"),
        ).grid(row=0, column=0, sticky="ew", padx=4)
        LiteButton(
            row2,
            text="📊 Show Status",
            width=10,
            command=lambda: self._dashboard_action("_show_status"),
        ).grid(row=0, column=1, sticky="ew", padx=4)
        LiteButton(
            row2,
            text="📋 Show OC Settings",
            width=10,
            command=lambda: self._dashboard_action("_show_get"),
        ).grid(row=0, column=2, sticky="ew", padx=4)
        LiteButton(
            row2,
            text="🖥 Console",
            width=10,
            command=lambda: self.app.console.toggle(),
        ).grid(row=0, column=3, sticky="ew", padx=4)

        self._result_lbl = tk.Label(
            card,
            text="",
            font=_FONT_BODY,
            bg=_PANE_BG,
            fg=_TEXT_FG,
            anchor="w",
            justify="left",
        )
        self._result_lbl.pack(fill="x", padx=10, pady=(0, 10))

    # ── Target card ────────────────────────────────────────────────────────

    def _build_target_card(self, scroll: Any) -> None:
        body = self._card(scroll, "🎯 Target")
        grid = tk.Frame(body, bg=_PANE_BG)
        grid.pack(fill="x")
        grid.grid_columnconfigure(1, weight=1)

        self._label(grid, "GPU:").grid(row=0, column=0, sticky="w", padx=(5, 6), pady=3)
        self._gpu_lbl = tk.Label(
            grid,
            text="",
            font=_FONT_BODY,
            bg=_PANE_BG,
            fg=_TEXT_FG,
            anchor="w",
        )
        self._gpu_lbl.grid(row=0, column=1, columnspan=3, sticky="ew", pady=3)

        self._label(grid, "Stressor:").grid(
            row=1, column=0, sticky="w", padx=(5, 6), pady=3
        )
        exe_row = tk.Frame(grid, bg=_PANE_BG)
        exe_row.grid(row=1, column=1, columnspan=3, sticky="ew", pady=3)
        LiteEntry(
            exe_row,
            textvariable=self._exe_var,
            width=28,
            min_px=240,
            justify="left",
        ).pack(side="left", fill="x", expand=True)
        LiteButton(exe_row, text="...", width=34, command=self._browse_exe).pack(
            side="left", padx=(5, 0)
        )

        cuda_label = self._label(grid, "CUDA libs:")
        cuda_label.grid(row=2, column=0, sticky="w", padx=(5, 6), pady=3)
        cuda_row = tk.Frame(grid, bg=_PANE_BG)
        cuda_row.grid(row=2, column=1, columnspan=3, sticky="ew", pady=3)
        cuda_entry = LiteEntry(
            cuda_row,
            textvariable=self._mk_var("cuda_path"),
            width=28,
            min_px=240,
            justify="left",
        )
        cuda_entry.pack(side="left", fill="x", expand=True)
        LiteButton(cuda_row, text="...", width=34, command=self._browse_cuda_path).pack(
            side="left", padx=(5, 0)
        )
        self._register_gated("cuda_path", cuda_entry, cuda_label)

        tools = tk.Frame(body, bg=_PANE_BG)
        tools.pack(fill="x", pady=(8, 0))
        LiteButton(tools, text="🔄 List GPUs", width=110, command=self._list_gpus).pack(
            side="left", padx=(5, 0)
        )
        LiteButton(
            tools, text="🔎 Re-probe", width=100, command=self._schedule_probe
        ).pack(side="left", padx=(6, 0))
        self._probe_lbl = self._label(body, "", dim=True, small=True)
        self._probe_lbl.pack(anchor="w", padx=5, pady=(6, 0))

    # ── Parameters card ────────────────────────────────────────────────────

    def _build_parameters_card(self, scroll: Any) -> None:
        body = self._card(scroll, "⚙ Parameters")
        self._label(
            body,
            "Blank fields fall back to the tool's own defaults.",
            dim=True,
            small=True,
        ).pack(anchor="w", padx=5, pady=(0, 4))
        grid = tk.Frame(body, bg=_PANE_BG)
        grid.pack(fill="x")

        self._menu_cell(grid, 0, 0, "Profile:", "profile", _PROFILE_VALUES)
        self._entry_cell(grid, 0, 1, "Duration (s):", "duration", width=8, min_px=70)
        self._entry_cell(grid, 1, 0, "Precisions:", "precisions", width=16, min_px=130)
        self._entry_cell(
            grid, 1, 1, "Matrix sizes:", "matrix_sizes", width=14, min_px=110
        )
        self._entry_cell(
            grid, 2, 0, "Kernel types:", "kernel_types", width=16, min_px=130
        )
        self._entry_cell(
            grid,
            2,
            1,
            "Validate interval (s):",
            "validate_interval",
            width=8,
            min_px=70,
        )
        self._entry_cell(
            grid, 3, 0, "Validate size:", "validate_size", width=10, min_px=90
        )
        self._entry_cell(grid, 3, 1, "Seed:", "seed", width=8, min_px=70)
        self._menu_cell(grid, 4, 0, "Stream mode:", "stream_mode", _STREAM_VALUES)
        self._check_cell(grid, 4, 1, "Generate buffers on GPU", "gpu_generate")

        extra_row = tk.Frame(body, bg=_PANE_BG)
        extra_row.pack(fill="x", pady=(6, 0))
        self._label(extra_row, "Extra args:").pack(side="left", padx=(5, 4))
        LiteEntry(
            extra_row,
            textvariable=self._mk_var("extra_args"),
            width=32,
            min_px=240,
            justify="left",
        ).pack(side="left", fill="x", expand=True, padx=(0, 14))

    # ── Verify / SDC card ──────────────────────────────────────────────────

    def _build_verify_card(self, scroll: Any) -> None:
        body = self._card(scroll, "🛡 Verify / SDC")
        self._label(body, _GATED_HINT, dim=True, small=True).pack(
            anchor="w", padx=5, pady=(0, 4)
        )
        grid = tk.Frame(body, bg=_PANE_BG)
        grid.pack(fill="x")

        self._check_cell(grid, 0, 0, "Disable verification", "no_verify", gated=True)
        self._check_cell(
            grid,
            0,
            1,
            "Continue on error",
            "verify_continue_on_error",
            gated=True,
        )
        self._entry_cell(
            grid,
            1,
            0,
            "Resident check interval (s):",
            "verify_resident_interval",
            width=8,
            min_px=70,
            gated=True,
        )
        self._check_cell(grid, 1, 1, "Slab pattern mode", "verify_slab", gated=True)
        self._entry_cell(
            grid,
            2,
            0,
            "GEMM full-check max size:",
            "gemm_full_check_max_size",
            width=12,
            min_px=96,
            gated=True,
        )
        self._check_cell(grid, 2, 1, "Skip self-test", "skip_self_test", gated=True)

        json_row = tk.Frame(body, bg=_PANE_BG)
        json_row.pack(fill="x", pady=(6, 0))
        json_label = self._label(json_row, "JSON out:")
        json_label.pack(side="left", padx=(5, 4))
        json_entry = LiteEntry(
            json_row,
            textvariable=self._mk_var("json_out"),
            width=26,
            min_px=200,
            justify="left",
        )
        json_entry.pack(side="left", fill="x", expand=True)
        LiteButton(json_row, text="...", width=34, command=self._browse_json_out).pack(
            side="left", padx=(5, 0)
        )
        self._register_gated("json_out", json_entry, json_label)

    # ── Vulkan card ────────────────────────────────────────────────────────

    def _build_vulkan_card(self, scroll: Any) -> None:
        body = self._card(scroll, "🎮 Vulkan")
        grid = tk.Frame(body, bg=_PANE_BG)
        grid.pack(fill="x")

        self._check_cell(grid, 0, 0, "Enable Vulkan render load", "vulkan", gated=True)
        self._check_cell(grid, 0, 1, "Vulkan only", "vulkan_only", gated=True)
        self._entry_cell(grid, 1, 0, "Width:", "vulkan_width", width=8, gated=True)
        self._entry_cell(grid, 1, 1, "Height:", "vulkan_height", width=8, gated=True)
        self._entry_cell(grid, 2, 0, "MSAA:", "vulkan_msaa", width=8, gated=True)
        self._entry_cell(grid, 2, 1, "Iters:", "vulkan_iters", width=8, gated=True)
        self._entry_cell(grid, 3, 0, "Shells:", "vulkan_shells", width=8, gated=True)
        self._check_cell(grid, 4, 0, "Windowed", "vulkan_window", gated=True)
        self._check_cell(grid, 4, 1, "Rotate", "vulkan_rotate", gated=True)
        self._entry_cell(
            grid, 5, 0, "Particles:", "vulkan_particles", width=10, gated=True
        )
        self._check_cell(grid, 5, 1, "Heavy offscreen", "vulkan_offscreen", gated=True)

    # ── executable discovery / capability probe ────────────────────────────

    def _exe_path(self) -> str:
        try:
            return self._exe_var.get().strip()
        except tk.TclError:
            return ""

    def _seed_exe_path(self) -> None:
        stored = str(self._value("exe_path") or "").strip()
        try:
            optimizer = str(self.app.config.get("cli_exe_path") or "")
        except Exception:
            optimizer = ""
        discovered = discover_stressor_exe(stored, optimizer)
        if discovered:
            self._exe_var.set(discovered)
        elif stored:
            self._exe_var.set(stored)

    def _browse_exe(self) -> None:
        current = self._exe_path()
        path = filedialog.askopenfilename(
            title="Select cli-stressor-cuda-rs executable",
            initialdir=os.path.dirname(current) or None,
            initialfile=os.path.basename(current) or None,
            filetypes=[("Executable", "*.exe"), ("All files", "*.*")],
        )
        if path:
            self._exe_var.set(path)
            self._schedule_probe()

    def _browse_json_out(self) -> None:
        path = filedialog.asksaveasfilename(
            defaultextension=".json",
            filetypes=[("JSON", "*.json"), ("All files", "*.*")],
        )
        if path and "json_out" in self._vars:
            self._vars["json_out"].set(path)

    def _browse_cuda_path(self) -> None:
        current = str(self._value("cuda_path") or "").strip()
        path = filedialog.askdirectory(
            title="Select the directory holding the CUDA runtime libraries",
            initialdir=current or None,
        )
        if path and "cuda_path" in self._vars:
            self._vars["cuda_path"].set(path)

    def _schedule_probe(self) -> None:
        exe = self._exe_path()
        self._probe_generation += 1
        generation = self._probe_generation
        if not exe or not os.path.isfile(exe):
            self._apply_capabilities(None)
            return
        self._probe_lbl.configure(
            text=f"Capabilities: probing {os.path.basename(exe)} …", fg=_TEXT_FG_DIM
        )

        def work() -> None:
            caps = probe_stressor_flags(exe)

            def apply() -> None:
                if generation != self._probe_generation:
                    return
                self._apply_capabilities(caps)

            self.app.after(0, apply)

        self.app.run_background("stressor-probe", work)

    def _apply_capabilities(self, capabilities: Optional[Set[str]]) -> None:
        self._capabilities = capabilities
        for key, widget, label in self._gated:
            supported = flag_for(key, capabilities) is not None
            try:
                widget.configure(state="normal" if supported else "disabled")
            except tk.TclError:
                pass
            if label is not None:
                label.configure(fg=_TEXT_FG if supported else _TEXT_FG_DIM)
        self._update_probe_label()

    def _update_probe_label(self) -> None:
        exe = self._exe_path()
        if not exe or not os.path.isfile(exe):
            self._probe_lbl.configure(
                text="Capabilities: no stressor executable selected", fg=_TEXT_FG_DIM
            )
            return
        caps = self._capabilities
        if caps is None:
            self._probe_lbl.configure(
                text=(
                    "Capabilities: probe failed — every option is forwarded "
                    "as-is (the tool reports unknown flags itself)"
                ),
                fg=_TEXT_FG_DIM,
            )
            return
        verify = (
            "available" if flag_for("no_verify", caps) is not None else "unavailable"
        )
        vulkan_flag = flag_for("vulkan", caps) or "unavailable"
        self._probe_lbl.configure(
            text=(
                f"Capabilities: {len(caps)} flags detected · "
                f"verify/SDC {verify} · vulkan {vulkan_flag}"
            ),
            fg=_TEXT_FG_DIM,
        )

    def _refresh_gpu_label(self) -> None:
        try:
            selected = str(self.app.gpu_var.get() or "").strip()
        except Exception:
            selected = ""
        try:
            index = self.app.get_current_gpu_index()
        except Exception:
            index = None
        suffix = (
            f"  → --gpu-index {index}"
            if index is not None
            else "  → tool default (index 0)"
        )
        self._gpu_lbl.configure(text=(selected or "(none)") + suffix)

    def _list_gpus(self) -> None:
        exe = self._exe_path()
        if not exe:
            self.app.console.append(
                "[GUI] No stressor executable — use '...' to locate "
                "cli-stressor-cuda-rs.\n"
            )
            return
        self.app.run_cli_display(["--list-gpus"], exe=exe)

    def _dashboard_action(self, name: str) -> None:
        tab = getattr(self.app, "tab_dashboard", None)
        action = getattr(tab, name, None) if tab is not None else None
        if callable(action):
            action()
        else:
            self.app.console.append(
                "[GUI] Dashboard actions are available once the Dashboard tab "
                "has been built.\n"
            )

    # ── run control ────────────────────────────────────────────────────────

    def _collect_state(self) -> Dict[str, Any]:
        state: Dict[str, Any] = {}
        for key, var in self._vars.items():
            try:
                value = var.get()
            except tk.TclError:
                continue
            if value == _DEFAULT_SENTINEL:
                continue
            state[key] = value
        return state

    def _set_run_buttons(self, start_enabled: bool, stop_enabled: bool) -> None:
        if self._is_resize_active:
            self._pending_run_button_state = (start_enabled, stop_enabled)
            return
        desired_start = "normal" if start_enabled else "disabled"
        desired_stop = "normal" if stop_enabled else "disabled"
        if self.start_btn.cget("state") != desired_start:
            self.start_btn.configure(state=desired_start)
        if self.stop_btn.cget("state") != desired_stop:
            self.stop_btn.configure(state=desired_stop)

    def on_resize_state_changed(
        self, resizing: bool, force_flush: bool = False
    ) -> None:
        self._is_resize_active = resizing
        if (
            (not resizing)
            and force_flush
            and self._pending_run_button_state is not None
        ):
            start_enabled, stop_enabled = self._pending_run_button_state
            self._pending_run_button_state = None
            self._set_run_buttons(start_enabled, stop_enabled)

    def _set_status(self, text: str, *, ok: bool = False, bad: bool = False) -> None:
        color = _TEXT_FG_DIM
        if ok:
            color = _TEXT_OK
        elif bad:
            color = _TEXT_BAD
        self._status_lbl.configure(text=text, fg=color)

    def _set_result(self, text: str) -> None:
        color = _TEXT_FG
        lowered = text.lower()
        if "verdict: pass" in lowered:
            color = _TEXT_OK
        elif "verdict: fail" in lowered or "verdict: error" in lowered:
            color = _TEXT_BAD
        self._result_lbl.configure(text=text, fg=color)

    def _start_run(self) -> None:
        exe = self._exe_path()
        if not exe or not os.path.isfile(exe):
            self.app.console.append(
                "[GUI] Stressor executable not found — use '...' to locate "
                "cli-stressor-cuda-rs.\n"
            )
            return
        runner = getattr(self.app, "runner", None)
        if runner is not None and getattr(runner, "is_running", False):
            self.app.console.append(
                "[GUI] Another CLI command is running — start the stressor "
                "once it finishes.\n"
            )
            return

        state = self._collect_state()
        try:
            index = self.app.get_current_gpu_index()
        except Exception:
            index = None
        if index is None:
            self.app.console.append(
                "[GUI] No GPU selected — the stressor uses its own default "
                "(PCI-sorted CUDA index 0).\n"
            )
        else:
            state["gpu_index"] = str(index)
            gpu_count = len(getattr(self.app, "gpu_names", {}) or {})
            if gpu_count > 1:
                self.app.console.append(
                    f"[GUI] {gpu_count} GPUs present: --gpu-index {index} counts "
                    "PCI-bus-sorted CUDA devices, which can differ from this "
                    "list's order — cross-check with “🔄 List GPUs”.\n"
                )

        args = build_stressor_args(state, self._capabilities)
        self._persist()
        self._set_result("")
        self._set_run_buttons(start_enabled=False, stop_enabled=True)
        self._set_status("running…")
        try:
            self.app.console.open()
        except Exception:
            pass

        def on_line(line: str) -> None:
            summary = summarize_line(line)
            if summary:
                self.app.after(0, lambda: self._set_result(summary))

        def on_finished(code: int) -> None:
            self.app.after(0, lambda: self._finish_run(code))

        self.app.run_cli(args, on_finished=on_finished, exe=exe, on_output=on_line)

    def _finish_run(self, code: int) -> None:
        self._set_run_buttons(start_enabled=True, stop_enabled=False)
        self._set_status(
            format_exit_status(code), ok=(code == 0), bad=(code not in (0, -1))
        )

    def _stop_run(self) -> None:
        self.app.cancel_cli()
        self._set_run_buttons(start_enabled=True, stop_enabled=False)
        self._set_status("stopping…")
