from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Input, Label, Static, TabPane

from ..models import AppConfig


def compose_dashboard(config: AppConfig) -> ComposeResult:
    with TabPane("Dashboard", id="dashboard"):
        with Vertical(classes="section"):
            with Horizontal(classes="row", id="dashboard-controls"):
                yield Label("Refresh (s): ")
                yield Input(
                    value=f"{config.dashboard.refresh_interval:.1f}",
                    id="dashboard-interval",
                    compact=True,
                )
                yield Button("Apply", id="dashboard-interval-apply", compact=True)
                yield Button("Pause", id="dashboard-pause", compact=True)
                yield Button("Now", id="dashboard-now", compact=True)
                yield Button("Info", id="dashboard-info", compact=True)
                yield Button("Status", id="dashboard-status", compact=True)
                yield Button("Get", id="dashboard-get", compact=True)
            yield Static("Waiting for first refresh.", id="metrics")
