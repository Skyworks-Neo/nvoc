from __future__ import annotations

from textual.widgets import Input, Select

from .base import PaneController


class AutoscanController(PaneController):
    def activate_shortcut(self, target_id: str) -> bool:
        if target_id in {"autoscan-mode", "autoscan-bsod"}:
            self.app.query_one(f"#{target_id}", Select).focus()
            return True
        if target_id in {
            "autoscan-test-exe",
            "autoscan-timeout",
            "autoscan-log",
            "autoscan-output",
            "autoscan-init",
        }:
            self.app.query_one(f"#{target_id}", Input).focus()
            return True
        return self.handle_button(target_id)

    def sync_from_ui(self) -> None:
        self.app.config_data.autoscan.mode = str(
            self.app.query_one("#autoscan-mode", Select).value or "standard"
        )
        self.app.config_data.autoscan.bsod_recovery = str(
            self.app.query_one("#autoscan-bsod", Select).value or ""
        )
        mapping = {
            "test_exe": "#autoscan-test-exe",
            "score_path": "#autoscan-score-path",
            "score_threshold": "#autoscan-score",
            "timeout_loops": "#autoscan-timeout",
            "log_file": "#autoscan-log",
            "output_csv": "#autoscan-output",
            "init_csv": "#autoscan-init",
        }
        for field, selector in mapping.items():
            setattr(
                self.app.config_data.autoscan,
                field,
                self.app.query_one(selector, Input).value.strip(),
            )
        self.app.save_config()

    def autoscan_args(self) -> list[str]:
        data = self.app.config_data.autoscan
        if data.mode == "legacy":
            args = self.app.gpu_args() + ["set", "vfp", "autoscan_legacy"]
        else:
            args = self.app.gpu_args() + ["set", "vfp", "autoscan"]
            if data.mode == "ultrafast":
                args.append("-u")
            args += ["-o", data.output_csv, "-i", data.init_csv]
        args += [
            "-w",
            data.test_exe,
            "-l",
            data.log_file,
            "-x",
            data.score_path,
            "-z",
            data.score_threshold,
            "-t",
            data.timeout_loops,
        ]
        if data.bsod_recovery:
            args += ["-b", data.bsod_recovery]
        return args

    def handle_button(self, button_id: str) -> bool:
        if button_id == "autoscan-export-init":
            self.sync_from_ui()
            self.app.run_action_chain(
                [
                    self.app.gpu_args() + ["set", "nvml", "--core-offset", "0"],
                    self.app.gpu_args() + ["set", "vfp", "export", "-q", "-"],
                ]
            )
            return True
        if button_id == "autoscan-reset-unlock":
            self.sync_from_ui()
            self.app.run_action_chain(
                [
                    self.app.gpu_args() + ["set", "nvapi", "--reset-volt-locks"],
                    self.app.gpu_args() + ["reset", "vfp"],
                ]
            )
            return True
        if button_id == "autoscan-start":
            self.sync_from_ui()
            self.app.run_cli_action(self.autoscan_args())
            return True
        if button_id == "autoscan-stop":
            self.app.cli_service.cancel_action()
            return True
        if button_id == "autoscan-fix":
            self.sync_from_ui()
            args = self.app.gpu_args() + ["set", "vfp", "fix_result", "-m", "1"]
            if self.app.config_data.autoscan.mode == "ultrafast":
                args.append("-u")
            self.app.run_cli_action(args)
            return True
        if button_id == "autoscan-import-final":
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", "vfp", "import", r".\ws\vfp.csv"]
            )
            return True
        if button_id == "autoscan-export-final":
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", "vfp", "export", r".\ws\vfp-final.csv"]
            )
            return True
        return False
