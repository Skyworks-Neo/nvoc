from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Input, Label, Select, TabPane

from ..models import AppConfig


def compose_autoscan(config: AppConfig) -> ComposeResult:
    with TabPane("Autoscan", id="autoscan"):
        with Vertical(classes="section"):
            with Horizontal(classes="row"):
                yield Label("Mode")
                yield Select(
                    options=[
                        ("Standard", "standard"),
                        ("Ultrafast", "ultrafast"),
                        ("Legacy", "legacy"),
                    ],
                    value=config.autoscan.mode,
                    id="autoscan-mode",
                    allow_blank=False,
                    compact=True,
                )
                yield Label("BSOD")
                yield Select(
                    options=[
                        ("(auto)", ""),
                        ("aggressive", "aggressive"),
                        ("traditional", "traditional"),
                    ],
                    value=config.autoscan.bsod_recovery,
                    id="autoscan-bsod",
                    allow_blank=False,
                    compact=True,
                )
            for label, value, widget_id in [
                ("Test Executable", config.autoscan.test_exe, "autoscan-test-exe"),
                ("Score XML Path", config.autoscan.score_path, "autoscan-score-path"),
                ("Score Threshold", config.autoscan.score_threshold, "autoscan-score"),
                ("Timeout Loops", config.autoscan.timeout_loops, "autoscan-timeout"),
                ("Log File", config.autoscan.log_file, "autoscan-log"),
                ("Output CSV", config.autoscan.output_csv, "autoscan-output"),
                ("Init CSV", config.autoscan.init_csv, "autoscan-init"),
            ]:
                with Horizontal(classes="row"):
                    yield Label(label)
                    yield Input(value=value, id=widget_id, classes="grow", compact=True)
            with Horizontal(classes="row"):
                yield Button("Export Init VFP", id="autoscan-export-init", compact=True)
                yield Button("Reset & Unlock", id="autoscan-reset-unlock", compact=True)
                yield Button("Start Autoscan", id="autoscan-start", compact=True)
                yield Button("Stop", id="autoscan-stop", compact=True)
                yield Button("Fix Results", id="autoscan-fix", compact=True)
                yield Button("Import Final", id="autoscan-import-final", compact=True)
                yield Button("Export Final", id="autoscan-export-final", compact=True)
