from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Grid, Horizontal, Vertical
from textual.widgets import Button, Label, Select, TabPane

from ..models import AppConfig
from ..widgets import ShortcutInput, mnemonic_text


def compose_autoscan(config: AppConfig) -> ComposeResult:
    with TabPane("Autoscan", id="autoscan"):
        with Vertical(classes="section"):
            with Grid(id="autoscan-groups"):
                with Vertical(classes="subpane") as mode_pane:
                    mode_pane.border_title = "Mode"
                    with Grid(id="autoscan-mode-controls"):
                        with Horizontal(classes="row"):
                            yield Label(mnemonic_text("M", "ode"))
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

                with Vertical(classes="subpane") as config_pane:
                    config_pane.border_title = "Config"
                    with Grid(id="autoscan-config-controls"):
                        with Horizontal(classes="row"):
                            yield Label(mnemonic_text("D", " Recovery", "BSO"))
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
                            (
                                mnemonic_text("T", "est Executable"),
                                config.autoscan.test_exe,
                                "autoscan-test-exe",
                            ),
                            (
                                "Score XML Path",
                                config.autoscan.score_path,
                                "autoscan-score-path",
                            ),
                            (
                                "Score Threshold",
                                config.autoscan.score_threshold,
                                "autoscan-score",
                            ),
                            (
                                mnemonic_text("L", "oops", "Timeout "),
                                config.autoscan.timeout_loops,
                                "autoscan-timeout",
                            ),
                            (
                                mnemonic_text("g", " File", "Lo"),
                                config.autoscan.log_file,
                                "autoscan-log",
                            ),
                            (
                                mnemonic_text("O", "utput CSV"),
                                config.autoscan.output_csv,
                                "autoscan-output",
                            ),
                            (
                                mnemonic_text("n", "it CSV", "I"),
                                config.autoscan.init_csv,
                                "autoscan-init",
                            ),
                        ]:
                            with Horizontal(classes="row"):
                                yield Label(label)
                                yield ShortcutInput(
                                    value=value,
                                    id=widget_id,
                                    classes="grow",
                                    compact=True,
                                )

                with Vertical(classes="subpane") as action_pane:
                    action_pane.border_title = "Action"
                    with Grid(id="autoscan-actions"):
                        yield Button(
                            mnemonic_text("V", "FP", "Export Init "),
                            id="autoscan-export-init",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("R", "eset & Unlock"),
                            id="autoscan-reset-unlock",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("A", "utoscan", "Start "),
                            id="autoscan-start",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("S", "top"),
                            id="autoscan-stop",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("x", " Results", "Fi"),
                            id="autoscan-fix",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("I", "mport Final"),
                            id="autoscan-import-final",
                            compact=True,
                        )
                        yield Button(
                            mnemonic_text("E", "xport Final"),
                            id="autoscan-export-final",
                            compact=True,
                        )
