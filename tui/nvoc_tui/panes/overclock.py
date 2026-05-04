from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Input, Label, Select, Static, TabPane


def compose_overclock() -> ComposeResult:
    with TabPane("Overclock", id="overclock"):
        with Vertical(classes="section"):
            with Horizontal(classes="row"):
                yield Label("Clock API")
                yield Select(
                    options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                    value="nvapi",
                    classes="nvapi-nvml-select",
                    id="oc-api",
                    allow_blank=False,
                    compact=True,
                )
                yield Label("PState Start")
                yield Input(value="", id="pstate-start", compact=True)
                yield Label("PState End")
                yield Input(value="", id="pstate-end", compact=True)
            with Horizontal(classes="row"):
                yield Label("Core Offset")
                yield Input(value="0", id="core-offset", compact=True)
                yield Label("Mem Offset")
                yield Input(value="0", id="mem-offset", compact=True)
            with Horizontal(classes="row"):
                yield Label("Power API")
                yield Select(
                    options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                    value="nvapi",
                    classes="nvapi-nvml-select",
                    id="power-api",
                    allow_blank=False,
                    compact=True,
                )
                yield Label("Power Limit")
                yield Input(value="100", id="power-limit", compact=True)
                yield Label("Thermal Limit")
                yield Input(value="83", id="thermal-limit", compact=True)
                yield Label("Voltage Boost")
                yield Input(value="0", id="voltage-boost", compact=True)
            with Horizontal(classes="row"):
                yield Button("Apply OC", id="oc-apply", classes="red", compact=True)
                yield Button("Reset OC", id="oc-reset", classes="green", compact=True)
                yield Button("Apply Limits", id="limits-apply", classes="red", compact=True)
                yield Button("Reset All", id="reset-all", classes="green", compact=True)
            yield Static("Fan Control", classes="row")
            with Horizontal(classes="row"):
                yield Label("Fan Target")
                yield Select(
                    options=[("All", "all"), ("Fan 1", "1"), ("Fan 2", "2")],
                    value="all",
                    id="fan-id",
                    allow_blank=False,
                    compact=True,
                )
                yield Label("Fan API")
                yield Select(
                    options=[("NVAPI", "nvapi"), ("NVML", "nvml")],
                    value="nvapi",
                    classes="nvapi-nvml-select",
                    id="fan-api",
                    allow_blank=False,
                    compact=True,
                )
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
                yield Label("Level")
                yield Input(value="60", id="fan-level", compact=True)
                yield Button("Apply Fan", id="fan-apply", classes="red", compact=True)
                yield Button("Reset Fan", id="fan-reset", classes="green", compact=True)
