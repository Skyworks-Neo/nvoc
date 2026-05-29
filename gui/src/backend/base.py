"""Backend operation models shared by GUI controllers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Protocol


@dataclass(frozen=True)
class FanSettings:
    backend: str
    policy: str
    level: int
    fan_id: Optional[str] = None


class GuiBackend(Protocol):
    def apply_fan_settings(self, settings: FanSettings) -> None: ...
    def reset_fan_settings(self, settings: FanSettings) -> None: ...
