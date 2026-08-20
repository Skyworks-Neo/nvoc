"""
Autoscan Tab - VFP auto-scanning workflow.
"""

import tkinter as tk
import customtkinter as ctk
from tkinter import filedialog
from typing import TYPE_CHECKING, Optional, Tuple
from src.widgets.lightweight_controls import (
    ct_button_font,
    LiteButton,
    LiteEntry,
    install_mousewheel_support,
)

# De-CTk'd palette (matches overclock.py / fan_control.py)
_PANE_BG = "#2b2b2b"
_TEXT_FG = "#e5e5e5"
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")
_SECTION_BORDER = "#1f4e79"

if TYPE_CHECKING:
    from src.app import App


class AutoscanTab:
    """Autoscan tab for VFP curve auto-optimization."""

    def __init__(self, parent: ctk.CTkFrame, app: "App") -> None:
        self.app = app
        self.frame = parent
        self._is_resize_active = False
        self._pending_scan_button_state: Optional[Tuple[bool, bool]] = None

        # Scrollable content
        scroll = ctk.CTkScrollableFrame(self.frame)
        scroll.pack(fill="both", expand=True, padx=10, pady=10)
        install_mousewheel_support(scroll)

        # === Mode Selection ===
        mode_frame = ctk.CTkFrame(
            scroll, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        mode_frame.pack(fill="x", pady=(0, 10))
        tk.Label(
            mode_frame, text="🔍 Scan Mode", font=_FONT_HEADER, bg=_PANE_BG, fg=_TEXT_FG
        ).pack(anchor="w", padx=10, pady=(10, 5))

        self.mode_var = ctk.StringVar(value="Standard")
        mode_row = tk.Frame(mode_frame, bg=_PANE_BG)
        mode_row.pack(fill="x", padx=10, pady=(0, 10))
        tk.Label(
            mode_row, text="Mode:", font=_FONT_BODY, bg=_PANE_BG, fg=_TEXT_FG
        ).pack(side="left", padx=(0, 6))
        self.mode_menu = ctk.CTkOptionMenu(
            mode_row,
            variable=self.mode_var,
            values=["Standard", "Ultrafast", "Legacy"],
            width=140,
            anchor="center",
            font=ct_button_font(mode_row),
        )
        self.mode_menu.pack(side="left")
        tk.Label(
            mode_row, text="BSOD:", font=_FONT_BODY, bg=_PANE_BG, fg=_TEXT_FG
        ).pack(side="left", padx=(16, 6))
        self.bsod_var = ctk.StringVar(value="(auto)")
        self.bsod_menu = ctk.CTkOptionMenu(
            mode_row,
            variable=self.bsod_var,
            values=["(auto)", "aggressive", "traditional"],
            width=130,
            anchor="center",
            font=ct_button_font(mode_row),
        )
        self.bsod_menu.pack(side="left")

        # === Parameters (left half) | Actions (right half) ===
        split = tk.Frame(scroll, bg=_PANE_BG)
        split.pack(fill="x", pady=(0, 10))
        split.grid_columnconfigure(0, weight=1, uniform="scan_split")
        split.grid_columnconfigure(1, weight=1, uniform="scan_split")

        param_frame = ctk.CTkFrame(
            split, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        param_frame.grid(row=0, column=0, sticky="new", padx=(0, 5))
        tk.Label(
            param_frame, text="⚙ Parameters", font=_FONT_HEADER, bg=_PANE_BG, fg=_TEXT_FG
        ).pack(anchor="w", padx=10, pady=(10, 5))

        params_grid = tk.Frame(param_frame, bg=_PANE_BG)
        params_grid.pack(fill="x", padx=10, pady=(0, 10))
        params_grid.columnconfigure(1, weight=0)

        row = 0
        # Output CSV
        tk.Label(params_grid, text="Output CSV:", font=_FONT_BODY, bg=_PANE_BG, fg=_TEXT_FG).grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.output_csv_var = ctk.StringVar(value=r".\ws\vfp-tem.csv")
        out_row = tk.Frame(params_grid, bg=_PANE_BG)
        out_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        out_entry = LiteEntry(
            out_row,
            textvariable=self.output_csv_var,
            width=20,
            min_px=140,
            justify="left",
        )
        out_entry.pack(side="left", fill="x", expand=True)
        LiteButton(
            out_row,
            text="...",
            width=34,
            command=lambda: self._browse_save(self.output_csv_var),
        ).pack(side="left", padx=(5, 0))

        row += 1
        # Init CSV
        tk.Label(params_grid, text="Init CSV:", font=_FONT_BODY, bg=_PANE_BG, fg=_TEXT_FG).grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.init_csv_var = ctk.StringVar(value=r".\ws\vfp-init.csv")
        init_row = tk.Frame(params_grid, bg=_PANE_BG)
        init_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        init_entry = LiteEntry(
            init_row,
            textvariable=self.init_csv_var,
            width=20,
            min_px=140,
            justify="left",
        )
        init_entry.pack(side="left", fill="x", expand=True)
        LiteButton(
            init_row,
            text="...",
            width=34,
            command=lambda: self._browse_file(self.init_csv_var),
        ).pack(side="left", padx=(5, 0))

        # === Action Buttons (right half) ===
        btn_frame = ctk.CTkFrame(
            split, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        btn_frame.grid(row=0, column=1, sticky="new", padx=(5, 0))
        tk.Label(
            btn_frame, text="▶ Actions", font=_FONT_HEADER, bg=_PANE_BG, fg=_TEXT_FG
        ).pack(anchor="w", padx=10, pady=(10, 5))

        btn_row = tk.Frame(btn_frame, bg=_PANE_BG)
        btn_row.pack(fill="x", padx=10, pady=(0, 10))

        LiteButton(
            btn_row, text="📤 Export Init VFP", width=10, command=self._export_init
        ).pack(side="left", fill="x", expand=True, padx=5)
        LiteButton(
            btn_row,
            text="🔓 Reset VFP",
            width=10,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_unlock,
        ).pack(side="left", fill="x", expand=True, padx=5)
        LiteButton(
            btn_row, text="🔧 Fix Results", width=10, command=self._fix_result
        ).pack(side="left", fill="x", expand=True, padx=5)

        btn_row2 = tk.Frame(btn_frame, bg=_PANE_BG)
        btn_row2.pack(fill="x", padx=10, pady=(0, 10))

        self.start_btn = LiteButton(
            btn_row2,
            text="▶ Start Autoscan",
            width=160,
            fg_color="#2d8a4e",
            hover_color="#236b3c",
            command=self._start_scan,
        )
        self.start_btn.pack(side="left", padx=5)

        self.stop_btn = LiteButton(
            btn_row2,
            text="⏹ Stop",
            width=100,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._stop_scan,
        )
        self.stop_btn.configure(state="disabled")
        self.stop_btn.pack(side="left", padx=5)

        btn_row3 = tk.Frame(btn_frame, bg=_PANE_BG)
        btn_row3.pack(fill="x", padx=10, pady=(0, 10))

        LiteButton(
            btn_row3, text="📥 Import Final VFP", width=10, command=self._import_final
        ).pack(side="left", fill="x", expand=True, padx=5)
        LiteButton(
            btn_row3, text="📤 Export Final VFP", width=160, command=self._export_final
        ).pack(side="left", padx=5)

    def _browse_file(self, var: ctk.StringVar) -> None:
        path = filedialog.askopenfilename()
        if path:
            var.set(path)

    def _browse_save(self, var: ctk.StringVar) -> None:
        path = filedialog.asksaveasfilename(
            defaultextension=".csv", filetypes=[("CSV", "*.csv"), ("All", "*.*")]
        )
        if path:
            var.set(path)

    def _set_scan_buttons(self, start_enabled: bool, stop_enabled: bool):
        if self._is_resize_active:
            self._pending_scan_button_state = (start_enabled, stop_enabled)
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
            and self._pending_scan_button_state is not None
        ):
            start_enabled, stop_enabled = self._pending_scan_button_state
            self._pending_scan_button_state = None
            self._set_scan_buttons(start_enabled, stop_enabled)

    def _export_init(self) -> None:
        gpu_args = self.app.get_gpu_args()
        gpu = self.app.selected_gpu_target()
        self.app.console.append("[GUI] Resetting core offset/curve...\n")

        def export_after_reset(code: int) -> None:
            if code == 0:
                self.app.run_cli_display(gpu_args + ["export-vfp", "-q", "-"])

        self.app.run_native_action(
            "reset core offset",
            lambda native, gpu=gpu: (
                native.set_clock_offset(gpu, "nvml", "core", 0, "P0")
                or "Successfully reset core offset."
            ),
            on_finished=export_after_reset,
        )

    def _reset_unlock(self) -> None:
        """Reset VF curve explicitly and unlock NVAPI VFP states, then auto refresh."""
        gpu_args = self.app.get_gpu_args()
        gpu = self.app.selected_gpu_target()

        def refresh_curve(_code: int) -> None:
            if getattr(self.app, "tab_vfcurve", None):
                self.app.tab_vfcurve._refresh_curve()

        def reset_curve_after_unlock(code: int) -> None:
            if code == 0:
                self.app.run_cli_display(
                    gpu_args + ["reset-vfp"],
                    on_finished=refresh_curve,
                )

        self.app.run_native_action(
            "reset VFP lock",
            lambda native, gpu=gpu: (
                native.reset_vfp_lock(gpu) or "Successfully reset VFP lock."
            ),
            on_finished=reset_curve_after_unlock,
        )

    def _start_scan(self) -> None:
        gpu_args = self.app.get_gpu_args()
        mode = {
            "Ultrafast": "ultrafast",
            "Legacy": "legacy",
        }.get(self.mode_var.get(), "standard")

        if mode == "legacy":
            args = gpu_args + ["autoscan-vfp-legacy"]
            bsod = self.bsod_var.get()
            if bsod != "(auto)":
                args += ["-b", bsod]
        else:
            args = gpu_args + ["autoscan-vfp"]
            if mode == "ultrafast":
                args.append("-u")
            args += ["-o", self.output_csv_var.get()]
            args += ["-i", self.init_csv_var.get()]
            bsod = self.bsod_var.get()
            if bsod != "(auto)":
                args += ["-b", bsod]

        self._set_scan_buttons(start_enabled=False, stop_enabled=True)

        def on_finished(retcode: int) -> None:
            self.frame.after(
                0,
                lambda: self._set_scan_buttons(start_enabled=True, stop_enabled=False),
            )

        self.app.run_cli(args, on_finished=on_finished)

    def _stop_scan(self) -> None:
        self.app.cancel_cli()
        self._set_scan_buttons(start_enabled=True, stop_enabled=False)

    def _fix_result(self) -> None:
        gpu_args = self.app.get_gpu_args()
        mode = {"Ultrafast": "ultrafast"}.get(self.mode_var.get(), "standard")
        args = gpu_args + ["fix-vfp-result", "-m", "1"]
        if mode == "ultrafast":
            args.append("-u")
        self.app.run_cli_display(args)

    def _import_final(self) -> None:
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli_display(gpu_args + ["import-vfp", r".\ws\vfp.csv"])

    def _export_final(self) -> None:
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli_display(gpu_args + ["export-vfp", r".\ws\vfp-final.csv"])
