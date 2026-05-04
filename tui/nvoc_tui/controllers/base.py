from __future__ import annotations

from typing import TYPE_CHECKING

from textual.widgets import Input

if TYPE_CHECKING:
    from ..app import NVOCApp


class PaneController:
    def __init__(self, app: "NVOCApp") -> None:
        self.app = app

    def get_int(self, widget_id: str, default: int = 0) -> int:
        try:
            return int(self.app.query_one(widget_id, Input).value.strip())
        except ValueError:
            return default
