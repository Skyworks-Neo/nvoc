"""Temporary CLI-backed GUI backend.

The GUI controllers call semantic backend methods; this adapter is the only
place in the new GUI refactor layer that translates those methods to CLI args.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from src.backend.base import FanSettings
from src.controllers.fan_control import fan_settings_to_cli_args

if TYPE_CHECKING:
    from src.app import App


class CliBackend:
    def __init__(self, app: "App") -> None:
        self.app = app

    def apply_fan_settings(self, settings: FanSettings) -> None:
        self.app.run_cli_display(
            fan_settings_to_cli_args(self.app.get_gpu_args(), settings)
        )

    def reset_fan_settings(self, settings: FanSettings) -> None:
        self.app.run_cli_display(
            fan_settings_to_cli_args(self.app.get_gpu_args(), settings)
        )
