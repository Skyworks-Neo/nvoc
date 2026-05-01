"""
Autoscan Tab - VFP auto-scanning workflow.
"""

import customtkinter as ctk
import os
from tkinter import filedialog
from typing import TYPE_CHECKING, Optional, Tuple
from src.widgets.lightweight_controls import (
    LiteButton,
    LiteEntry,
    install_mousewheel_support,
)

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
        mode_frame = ctk.CTkFrame(scroll)
        mode_frame.pack(fill="x", pady=(0, 10))
        ctk.CTkLabel(mode_frame, text="Scan Mode", font=("", 14, "bold")).pack(
            anchor="w", padx=10, pady=(10, 5)
        )

        self.mode_var = ctk.StringVar(value="standard")
        mode_row = ctk.CTkFrame(mode_frame, fg_color="transparent")
        mode_row.pack(fill="x", padx=10, pady=(0, 10))
        for val, label in [
            ("standard", "Standard"),
            ("ultrafast", "Ultrafast"),
            ("legacy", "Legacy (Maxwell/9xx)"),
        ]:
            ctk.CTkRadioButton(
                mode_row, text=label, variable=self.mode_var, value=val
            ).pack(side="left", padx=15)

        # === Parameters ===
        param_frame = ctk.CTkFrame(scroll)
        param_frame.pack(fill="x", pady=(0, 10))
        ctk.CTkLabel(param_frame, text="Parameters", font=("", 14, "bold")).pack(
            anchor="w", padx=10, pady=(10, 5)
        )

        params_grid = ctk.CTkFrame(param_frame, fg_color="transparent")
        params_grid.pack(fill="x", padx=10, pady=(0, 10))
        params_grid.columnconfigure(1, weight=0)

        row = 0
        # Test exe
        ctk.CTkLabel(params_grid, text="Test Executable:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        default_test_exe = (
            r".\test\test_cuda_windows.bat"
            if os.name == "nt"
            else "./test/test_opencl_linux.sh"
        )
        self.test_exe_var = ctk.StringVar(value=default_test_exe)
        test_exe_row = ctk.CTkFrame(params_grid, fg_color="transparent")
        test_exe_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        test_exe_entry = LiteEntry(
            test_exe_row,
            textvariable=self.test_exe_var,
            width=52,
            min_px=420,
            justify="left",
        )
        test_exe_entry.pack(side="left")
        LiteButton(
            test_exe_row,
            text="...",
            width=34,
            command=lambda: self._browse_file(self.test_exe_var),
        ).pack(side="left", padx=(5, 0))

        row += 1
        # Timeout loops
        ctk.CTkLabel(params_grid, text="Timeout Loops:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.timeout_var = ctk.StringVar(value="30")
        LiteEntry(
            params_grid,
            textvariable=self.timeout_var,
            width=10,
            min_px=86,
            justify="right",
        ).grid(row=row, column=1, sticky="w", padx=5, pady=3)

        row += 1
        # Log file
        ctk.CTkLabel(params_grid, text="Log File:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.log_var = ctk.StringVar(value=r".\ws\vfp.log")
        log_row = ctk.CTkFrame(params_grid, fg_color="transparent")
        log_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        log_entry = LiteEntry(
            log_row, textvariable=self.log_var, width=52, min_px=420, justify="left"
        )
        log_entry.pack(side="left")
        LiteButton(
            log_row,
            text="...",
            width=34,
            command=lambda: self._browse_file(self.log_var),
        ).pack(side="left", padx=(5, 0))

        row += 1
        # Output CSV
        ctk.CTkLabel(params_grid, text="Output CSV:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.output_csv_var = ctk.StringVar(value=r".\ws\vfp-tem.csv")
        out_row = ctk.CTkFrame(params_grid, fg_color="transparent")
        out_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        out_entry = LiteEntry(
            out_row,
            textvariable=self.output_csv_var,
            width=52,
            min_px=420,
            justify="left",
        )
        out_entry.pack(side="left")
        LiteButton(
            out_row,
            text="...",
            width=34,
            command=lambda: self._browse_save(self.output_csv_var),
        ).pack(side="left", padx=(5, 0))

        row += 1
        # Init CSV
        ctk.CTkLabel(params_grid, text="Init CSV:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.init_csv_var = ctk.StringVar(value=r".\ws\vfp-init.csv")
        init_row = ctk.CTkFrame(params_grid, fg_color="transparent")
        init_row.grid(row=row, column=1, sticky="ew", padx=5, pady=3)
        init_entry = LiteEntry(
            init_row,
            textvariable=self.init_csv_var,
            width=52,
            min_px=420,
            justify="left",
        )
        init_entry.pack(side="left")
        LiteButton(
            init_row,
            text="...",
            width=34,
            command=lambda: self._browse_file(self.init_csv_var),
        ).pack(side="left", padx=(5, 0))

        row += 1
        # BSOD Recovery
        ctk.CTkLabel(params_grid, text="BSOD Recovery:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.bsod_var = ctk.StringVar(value="(auto)")
        ctk.CTkOptionMenu(
            params_grid,
            variable=self.bsod_var,
            values=["(auto)", "aggressive", "traditional"],
            width=150,
        ).grid(row=row, column=1, sticky="w", padx=5, pady=3)

        # === Action Buttons ===
        btn_frame = ctk.CTkFrame(scroll)
        btn_frame.pack(fill="x", pady=(0, 10))
        ctk.CTkLabel(btn_frame, text="Actions", font=("", 14, "bold")).pack(
            anchor="w", padx=10, pady=(10, 5)
        )

        btn_row = ctk.CTkFrame(btn_frame, fg_color="transparent")
        btn_row.pack(fill="x", padx=10, pady=(0, 10))

        LiteButton(
            btn_row, text="📤 Export Init VFP", width=150, command=self._export_init
        ).pack(side="left", padx=5)
        LiteButton(
            btn_row, text="🔓 Reset & Unlock VFP", width=170, command=self._reset_unlock
        ).pack(side="left", padx=5)

        btn_row2 = ctk.CTkFrame(btn_frame, fg_color="transparent")
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

        btn_row3 = ctk.CTkFrame(btn_frame, fg_color="transparent")
        btn_row3.pack(fill="x", padx=10, pady=(0, 10))

        LiteButton(
            btn_row3, text="🔧 Fix Results", width=130, command=self._fix_result
        ).pack(side="left", padx=5)
        LiteButton(
            btn_row3, text="📥 Import Final VFP", width=160, command=self._import_final
        ).pack(side="left", padx=5)
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
        self.app.console.append("[GUI] Resetting core offset/curve...\n")
        self.app.run_cli_display(gpu_args + ["set", "nvml", "--core-offset", "0"])
        # Chain: after OC reset, export
        self.app.run_cli_display(gpu_args + ["set", "vfp", "export", "-q", "-"])

    def _reset_unlock(self) -> None:
        """Reset VF curve explicitly and unlock NVAPI VFP states, then auto refresh."""
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli_display(gpu_args + ["set", "nvapi", "--reset-volt-locks"])
        self.app.run_cli_display(gpu_args + ["reset", "vfp"])
        if getattr(self.app, "tab_vfcurve", None):
            self.app.tab_vfcurve._refresh_curve()

    def _start_scan(self) -> None:
        gpu_args = self.app.get_gpu_args()
        mode = self.mode_var.get()

        if mode == "legacy":
            args = gpu_args + ["set", "vfp", "autoscan_legacy"]
            args += ["-w", self.test_exe_var.get()]
            args += ["-l", self.log_var.get()]
            args += ["-t", self.timeout_var.get()]
            bsod = self.bsod_var.get()
            if bsod != "(auto)":
                args += ["-b", bsod]
        else:
            args = gpu_args + ["set", "vfp", "autoscan"]
            if mode == "ultrafast":
                args.append("-u")
            args += ["-w", self.test_exe_var.get()]
            args += ["-l", self.log_var.get()]
            args += ["-t", self.timeout_var.get()]
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
        mode = self.mode_var.get()
        args = gpu_args + ["set", "vfp", "fix_result", "-m", "1"]
        if mode == "ultrafast":
            args.append("-u")
        self.app.run_cli_display(args)

    def _import_final(self) -> None:
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli_display(gpu_args + ["set", "vfp", "import", r".\ws\vfp.csv"])

    def _export_final(self) -> None:
        gpu_args = self.app.get_gpu_args()
        self.app.run_cli_display(
            gpu_args + ["set", "vfp", "export", r".\ws\vfp-final.csv"]
        )
