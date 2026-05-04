from __future__ import annotations

from pathlib import Path

from textual.widgets import Input, Select

from ..cli import CliService
from ..models import GpuDescriptor
from .base import PaneController


class HeaderController(PaneController):
    def selected_gpu_idx(self) -> int | None:
        try:
            value = self.app.query_one("#gpu-select", Select).value
        except Exception:
            return None
        if value in (None, Select.BLANK):
            return None
        return int(value)

    def gpu_args(self) -> list[str]:
        idx = self.selected_gpu_idx()
        return [f"--gpu={idx}"] if idx is not None and idx >= 0 else []

    def current_gpu(self) -> GpuDescriptor | None:
        idx = self.selected_gpu_idx()
        for gpu in self.app.gpus:
            if gpu.index == idx:
                return gpu
        return None

    def save_cli_path(self) -> None:
        path = self.app.query_one("#cli-path", Input).value.strip()
        discovered = CliService.discover_cli(path)
        self.app.config_data.cli = (
            discovered
            if discovered.exe_path
            else self.app.config_data.cli.__class__(exe_path=path)
        )
        if self.app.config_data.cli.exe_path and not self.app.config_data.cli.cwd:
            self.app.config_data.cli.cwd = str(
                Path(self.app.config_data.cli.exe_path).resolve().parent
            )
        self.app.save_config()
        self.app.write_log(
            f"CLI path set to: {self.app.config_data.cli.exe_path or path}"
        )

    def on_gpu_selected(self, value: object) -> None:
        if value not in (None, Select.BLANK):
            self.app.config_data.last_gpu_idx = int(value)
            self.app.save_config()
            self.app.refresh_all_state()

    def on_gpu_list_loaded(
        self, code: int, output: str, gpus: list[GpuDescriptor]
    ) -> None:
        self.app.write_log(output or "GPU detection finished.")
        self.app.gpus = gpus
        select = self.app.query_one("#gpu-select", Select)
        if not gpus:
            select.set_options([("(no GPUs found)", "-1")])
            select.value = "-1"
            return
        select.set_options([(gpu.long_label, str(gpu.index)) for gpu in gpus])
        target = self.app.config_data.last_gpu_idx
        if target is None or all(gpu.index != target for gpu in gpus):
            target = gpus[0].index
        select.value = str(target)
        self.app.config_data.last_gpu_idx = target
        self.app.save_config()
        if code == 0 and self.app.config_data.cli.exe_path:
            self.app.focus_dashboard_tab_switcher()
        self.app.refresh_all_state()

    def handle_button(self, button_id: str) -> bool:
        if button_id == "detect-gpus":
            self.app.refresh_gpu_list()
            return True
        if button_id == "save-cli":
            self.save_cli_path()
            return True
        if button_id == "refresh-all":
            self.app.refresh_all_state()
            return True
        return False
