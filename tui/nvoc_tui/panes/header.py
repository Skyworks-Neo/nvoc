from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Header, Input, Label, Select, Static

from ..models import AppConfig


def compose_header(config: AppConfig) -> ComposeResult:
    yield Header()
    with Vertical(id="topbar"):
        with Horizontal(classes="toprow"):
            yield Label("GPU: ")
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
        with Horizontal(classes="toprow"):
            yield Label("CLI: ")
            yield Input(
                value=config.cli.exe_path,
                placeholder="CLI path",
                id="cli-path",
                classes="grow",
                compact=True,
            )
            yield Button("Save CLI", id="save-cli", compact=True)
    yield Static(classes="hsplit")
