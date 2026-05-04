from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Label, Static, TabPane

from ..models import AppConfig
from ..widgets import ShortcutInput, mnemonic_text


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
                    mnemonic_text("A", "pply"),
                    id="dashboard-interval-apply",
                    compact=True,
                )
                yield Button(
                    mnemonic_text("P", "ause"), id="dashboard-pause", compact=True
                )
                yield Button(
                    mnemonic_text("N", "ow"), id="dashboard-now", compact=True
                )
                yield Button(
                    mnemonic_text("I", "nfo"), id="dashboard-info", compact=True
                )
                yield Button(
                    mnemonic_text("S", "tatus"),
                    id="dashboard-status",
                    compact=True,
                )
                yield Button(
                    mnemonic_text("G", "et"), id="dashboard-get", compact=True
                )
            yield Static("Waiting for first refresh.", id="metrics")
