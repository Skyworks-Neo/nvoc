from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Grid, Horizontal, Vertical
from textual.widgets import Button, Label, Select, TabPane

from ..widgets import ShortcutInput, mnemonic_text


def compose_overclock() -> ComposeResult:
    with TabPane("Overclock", id="overclock"):
        with Vertical(classes="section"):
            with Grid(id="overclock-groups"):
                with Vertical(classes="subpane") as clock_pane:
                    clock_pane.border_title = mnemonic_text("C", "lock")
                    with Grid(id="clock-controls"):
                        with Horizontal(classes="row"):
                            yield Label("API")
                            yield Select(
                                options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                                value="nvapi",
                                classes="nvapi-nvml-select",
                                id="oc-api",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("PState Start")
                            yield ShortcutInput(
                                value="", id="pstate-start", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("PState End")
                            yield ShortcutInput(value="", id="pstate-end", compact=True)
                        with Horizontal(classes="row"):
                            yield Label("Core Offset")
                            yield ShortcutInput(
                                value="0", id="core-offset", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("Mem Offset")
                            yield ShortcutInput(
                                value="0", id="mem-offset", compact=True
                            )
                    with Grid(id="clock-actions"):
                        yield Button(
                            "Apply OC", id="oc-apply", classes="red", compact=True
                        )
                        yield Button(
                            "Reset OC", id="oc-reset", classes="green", compact=True
                        )

                with Vertical(classes="subpane") as power_pane:
                    power_pane.border_title = mnemonic_text("P", "ower")
                    with Grid(id="power-controls"):
                        with Horizontal(classes="row"):
                            yield Label("API")
                            yield Select(
                                options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                                value="nvapi",
                                classes="nvapi-nvml-select",
                                id="power-api",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("Power Limit")
                            yield ShortcutInput(
                                value="100", id="power-limit", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("Thermal Limit")
                            yield ShortcutInput(
                                value="83", id="thermal-limit", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("Voltage Boost")
                            yield ShortcutInput(
                                value="0", id="voltage-boost", compact=True
                            )
                    with Grid(id="power-actions"):
                        yield Button(
                            "Apply Limits",
                            id="limits-apply",
                            classes="red",
                            compact=True,
                        )
                        yield Button(
                            "Reset Limits",
                            id="reset-limits",
                            classes="green",
                            compact=True,
                        )

                with Vertical(classes="subpane") as fan_pane:
                    fan_pane.border_title = mnemonic_text("a", "n", "F")
                    with Grid(id="fan-controls"):
                        with Horizontal(classes="row"):
                            yield Label("Target")
                            yield Select(
                                options=[
                                    ("All", "all"),
                                    ("Fan 1", "1"),
                                    ("Fan 2", "2"),
                                ],
                                value="all",
                                id="fan-id",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("API")
                            yield Select(
                                options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                                value="nvapi",
                                classes="nvapi-nvml-select",
                                id="fan-api",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("Policy")
                            yield Select(
                                options=[
                                    ("contin.", "continuous"),
                                    ("manual", "manual"),
                                    ("default", "default"),
                                    ("auto", "auto"),
                                ],
                                value="continuous",
                                id="fan-policy",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("Level")
                            yield ShortcutInput(
                                value="60", id="fan-level", compact=True
                            )
                    with Grid(id="fan-actions"):
                        yield Button(
                            "Apply Fan", id="fan-apply", classes="red", compact=True
                        )
                        yield Button(
                            "Reset Fan", id="fan-reset", classes="green", compact=True
                        )
