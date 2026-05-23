from __future__ import annotations

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Header, Label, Select, Static

from ..models import AppConfig


def compose_header(config: AppConfig) -> ComposeResult:
    yield Header()
    with Vertical(id="topbar"):
        with Horizontal(classes="toprow"):
            yield Label(Text.assemble(("G", "bold"), "PU: "))
            yield Select(
                options=[("Detecting...", "-1")],
                id="gpu-select",
                allow_blank=False,
                compact=True,
                classes="grow",
            )
            with Horizontal(id="gpu-actions"):
                yield Button("Detect", id="detect-gpus", compact=True)
                yield Button("Refresh All", id="refresh-all", compact=True)
    yield Static(classes="hsplit")
