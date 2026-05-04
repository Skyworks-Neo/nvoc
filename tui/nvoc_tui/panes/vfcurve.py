from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Button, Checkbox, Input, Label, Select, TabPane
from textual_plotext import PlotextPlot

from ..models import AppConfig


def compose_vfcurve(config: AppConfig, auto_refresh_label: str) -> ComposeResult:
    with TabPane("VF Curve", id="vfcurve"):
        with Vertical(classes="section"):
            with Horizontal(classes="row"):
                yield Input(
                    value=config.vfcurve.default_path,
                    placeholder="CSV path for import/export",
                    id="vf-path",
                    classes="grow",
                    compact=True,
                )
                yield Checkbox(
                    "Quick export",
                    value=config.vfcurve.quick_export,
                    id="vf-quick-export",
                    compact=True,
                )
            with Horizontal(classes="row", id="vf-actions"):
                yield Button("Refresh Curve", id="vf-refresh", classes="blue", compact=True)
                yield Button(auto_refresh_label, id="vf-auto-refresh", compact=True)
                yield Button("Export VFP", id="vf-export", compact=True)
                yield Button("Import VFP", id="vf-import", compact=True)
                yield Button("Reset VFP", id="vf-reset", classes="green", compact=True)
            with Horizontal(classes="row", id="vf-range-actions"):
                yield Label("VF Adj: Range")
                yield Input(value="0", id="vf-range-start", compact=True)
                yield Label("to")
                yield Input(value="0", id="vf-range-end", compact=True)
                yield Label("Delta MHz")
                yield Input(value="0", id="vf-delta", compact=True)
                yield Button("Apply Adj", id="vf-apply-adj", classes="red", compact=True)
            with Horizontal(classes="row", id="vf-lock-actions"):
                yield Label("Lock Value")
                yield Input(value="55", id="vf-lock-value", compact=True)
                yield Checkbox("As mV", value=False, id="vf-lock-as-mv", compact=True)
                yield Button("Lock Voltage", id="vf-lock-voltage", classes="red", compact=True)
                yield Button("Reset Volt Lock", id="vf-unlock", classes="green", compact=True)
            with Horizontal(classes="row", id="vf-mem-actions"):
                yield Label("Freq API")
                yield Select(
                    options=[("NVML", "nvml"), ("NVAPI", "nvapi")],
                    value="nvml",
                    classes="nvapi-nvml-select",
                    id="vf-freq-api",
                    allow_blank=False,
                    compact=True,
                )
                yield Label("Core Freq Min")
                yield Input(value="0", id="vf-core-min", compact=True)
                yield Label("Max")
                yield Input(value="0", id="vf-core-max", compact=True)
                yield Button("Lock Core", id="vf-lock-core", classes="red", compact=True)
                yield Button("Reset Core", id="vf-reset-core", classes="green", compact=True)
            with Horizontal(classes="row"):
                yield Label("Mem Freq Min")
                yield Input(value="0", id="vf-mem-min", compact=True)
                yield Label("Max")
                yield Input(value="0", id="vf-mem-max", compact=True)
                yield Button("Lock Mem", id="vf-lock-mem", classes="red", compact=True)
                yield Button("Reset Mem", id="vf-reset-mem", classes="green", compact=True)
            yield PlotextPlot(id="vf-plot")
