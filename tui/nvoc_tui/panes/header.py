from __future__ import annotations

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Header, Label, Select, Static

from ..models import AppConfig
from ..widgets import ShortcutInput


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
        with Horizontal(classes="toprow"):
            yield Label("CLI: ")
            yield ShortcutInput(
                value=config.cli.exe_path,
                placeholder="CLI path",
                id="cli-path",
                classes="grow",
                compact=True,
            )
            yield Button("Save CLI", id="save-cli", compact=True)
    yield Static(classes="hsplit")
