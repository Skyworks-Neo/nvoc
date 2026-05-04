from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Footer, Label, Log


def compose_console() -> ComposeResult:
    with Horizontal(id="log-header"):
        yield Label("  Output")
        yield Button("Hide", id="toggle-log", compact=True)
        yield Button("Clear", id="clear-log", compact=True)
    with Vertical(id="log-panel"):
        yield Log(id="output-log", highlight=True, auto_scroll=True, max_lines=100)
    yield Footer()
