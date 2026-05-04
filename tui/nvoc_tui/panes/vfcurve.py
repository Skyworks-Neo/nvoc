from __future__ import annotations

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Grid, Horizontal, Vertical
from textual.widgets import Button, Checkbox, Label, Select, TabPane
from textual_plotext import PlotextPlot

from ..models import AppConfig
from ..widgets import ShortcutInput, mnemonic_text


def compose_vfcurve(config: AppConfig, auto_refresh_label: Text) -> ComposeResult:
    with TabPane("VF Curve", id="vfcurve"):
        with Vertical(classes="section"):
            # First split the pane into two columns
            with Grid(id="vfcurve-controls-main"):
                with Vertical(classes="column"):
                    # VFP Import / Export subpane
                    with Vertical(classes="subpane") as vfp_subpane:
                        vfp_subpane.border_title = "VFP Import/Export"
                        with Horizontal(classes="row"):
                            yield Label(mnemonic_text("C", "SV Path"))
                            yield ShortcutInput(
                                value=config.vfcurve.default_path,
                                placeholder="CSV path for import/export",
                                id="vf-path",
                                classes="grow",
                                compact=True,
                            )
                        with Grid(id="vf-import-export-actions"):
                            yield Checkbox(
                                mnemonic_text("Q", "uick export"),
                                value=config.vfcurve.quick_export,
                                id="vf-quick-export",
                                compact=True,
                            )
                            yield Button(
                                mnemonic_text("R", "eset VFP"),
                                id="vf-reset",
                                classes="green",
                                compact=True,
                            )
                            yield Button(
                                mnemonic_text("E", "xport VFP"),
                                id="vf-export",
                                compact=True,
                            )
                            yield Button(
                                mnemonic_text("I", "mport VFP"),
                                id="vf-import",
                                compact=True,
                            )

                    # VFP Adjustments subpane
                    with Vertical(classes="subpane") as vf_adj_subpane:
                        vf_adj_subpane.border_title = "VFP Adjustments (Alt-V)"
                        with Grid(id="vf-range-actions"):
                            yield ShortcutInput(
                                value="0", id="vf-range-start", compact=True
                            )
                            yield Label("to")
                            yield ShortcutInput(
                                value="0", id="vf-range-end", compact=True
                            )
                            yield Label("Delta MHz")
                            yield ShortcutInput(value="0", id="vf-delta", compact=True)
                            yield Button(
                                "Apply Adj",
                                id="vf-apply-adj",
                                classes="red",
                                compact=True,
                            )

                    # Plot settings subpane
                    with Vertical(classes="subpane") as plot_settings_subpane:
                        plot_settings_subpane.border_title = "Plot Settings"
                        with Grid(id="vf-plot-settings"):
                            yield Button(
                                mnemonic_text("s", "h Curve", before="Refre"),
                                id="vf-refresh",
                                classes="blue",
                                compact=True,
                            )
                            yield Button(
                                auto_refresh_label, id="vf-auto-refresh", compact=True
                            )

                # The right column
                with Vertical(classes="column"):
                    # Voltage locking subpane
                    with Vertical(classes="subpane") as volt_lock_subpane:
                        volt_lock_subpane.border_title = "Voltage Locking (Alt-L)"
                        with Grid(id="vf-lock-actions"):
                            yield ShortcutInput(
                                value="55", id="vf-lock-value", compact=True
                            )
                            yield Checkbox(
                                "As mV", value=False, id="vf-lock-as-mv", compact=True
                            )
                            yield Button(
                                "Lock Voltage",
                                id="vf-lock-voltage",
                                classes="red",
                                compact=True,
                            )
                            yield Button(
                                "Reset Volt Lock",
                                id="vf-unlock",
                                classes="green",
                                compact=True,
                            )

                    # Frequency adjustments subpane
                    with Vertical(classes="subpane") as freq_adj_subpane:
                        freq_adj_subpane.border_title = (
                            "Core Frequency Adjustments (Alt-U)"
                        )
                        with Grid(id="vf-freq-actions"):
                            with Horizontal(classes="row"):
                                yield Label("API")
                                yield Select(
                                    options=[("NVML", "nvml"), ("NVAPI", "nvapi")],
                                    value="nvml",
                                    classes="nvapi-nvml-select",
                                    id="vf-freq-api",
                                    allow_blank=False,
                                    compact=True,
                                )
                            with Horizontal(classes="row"):
                                yield Label("Min")
                                yield ShortcutInput(
                                    value="0", id="vf-core-min", compact=True
                                )
                                yield Label("Max")
                                yield ShortcutInput(
                                    value="0", id="vf-core-max", compact=True
                                )
                            with Horizontal(classes="row"):
                                yield Button(
                                    "Lock Core",
                                    id="vf-lock-core",
                                    classes="red",
                                    compact=True,
                                )
                            with Horizontal(classes="row"):
                                yield Button(
                                    "Reset Core",
                                    id="vf-reset-core",
                                    classes="green",
                                    compact=True,
                                )

                    # Memory frequency adjustments subpane
                    with Vertical(classes="subpane") as mem_adj_subpane:
                        mem_adj_subpane.border_title = (
                            "Memory Frequency Adjustments (Alt-M)"
                        )
                        with Grid(id="vf-mem-actions"):
                            yield Label("Min")
                            yield ShortcutInput(
                                value="0", id="vf-mem-min", compact=True
                            )
                            yield Label("Max")
                            yield ShortcutInput(
                                value="0", id="vf-mem-max", compact=True
                            )
                            yield Button(
                                "Lock Mem",
                                id="vf-lock-mem",
                                classes="red",
                                compact=True,
                            )
                            yield Button(
                                "Reset Mem",
                                id="vf-reset-mem",
                                classes="green",
                                compact=True,
                            )
            yield PlotextPlot(id="vf-plot")
