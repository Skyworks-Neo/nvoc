from __future__ import annotations

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Label, Static, TabPane

from ..models import AppConfig
from ..widgets import ShortcutInput


def mnemonic_label(letter: str, rest: str) -> Text:
    return Text.assemble((letter, "underline"), rest)


def compose_dashboard(config: AppConfig) -> ComposeResult:
    with TabPane("Dashboard", id="dashboard"):
        with Vertical(classes="section"):
            with Horizontal(classes="row", id="dashboard-controls"):
                yield Label("Refresh (s): ")
                yield ShortcutInput(
                    value=f"{config.dashboard.refresh_interval:.1f}",
                    id="dashboard-interval",
                    compact=True,
                )
                yield Button(
                    mnemonic_label("A", "pply"),
                    id="dashboard-interval-apply",
                    compact=True,
                )
                yield Button(
                    mnemonic_label("P", "ause"), id="dashboard-pause", compact=True
                )
                yield Button(
                    mnemonic_label("N", "ow"), id="dashboard-now", compact=True
                )
                yield Button(
                    mnemonic_label("I", "nfo"), id="dashboard-info", compact=True
                )
                yield Button(
                    mnemonic_label("S", "tatus"),
                    id="dashboard-status",
                    compact=True,
                )
                yield Button(
                    mnemonic_label("G", "et"), id="dashboard-get", compact=True
                )
            yield Static("Waiting for first refresh.", id="metrics")
