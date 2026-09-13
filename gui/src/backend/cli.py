"""Temporary CLI-backed GUI backend.

The GUI controllers call semantic backend methods; this adapter is the only
place in the new GUI refactor layer that translates those methods to CLI args.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from src.backend.base import FanSettings

if TYPE_CHECKING:
    from src.app import App


class CliBackend:
    def __init__(self, app: "App") -> None:
        self.app = app

    def apply_fan_settings(self, settings: FanSettings) -> None:
        # The CLI arg translation for fan settings was never wired (the class
        # has no instantiation site yet); fan control goes through the native
        # backend in the meantime.
        raise NotImplementedError(
            "CliBackend fan-settings translation is not implemented; "
            "use the native backend"
        )

    def reset_fan_settings(self, settings: FanSettings) -> None:
        raise NotImplementedError(
            "CliBackend fan-settings translation is not implemented; "
            "use the native backend"
        )
