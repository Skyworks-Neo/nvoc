from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical, Grid
from textual.widgets import Button, Label, Static, TabPane

from ..models import AppConfig
from ..widgets import ShortcutInput, mnemonic_text


def compose_dashboard(config: AppConfig) -> ComposeResult:
    with TabPane("Dashboard", id="dashboard"):
        with Vertical(classes="section"):
            with Vertical(classes="subpane") as stats_pane:
                stats_pane.border_title = "GPU Statistics"
                with Grid(id="dashboard-stats"):
                    with Horizontal(classes="row"):
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
                        mnemonic_text("R", "refresh"), id="dashboard-now", compact=True
                    )
                yield Static("Waiting for first refresh.", id="metrics")

            with Vertical(classes="subpane") as actions_pane:
                actions_pane.border_title = "CLI Commands (see logs for output)"
                with Grid(id="dashboard-actions"):
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
