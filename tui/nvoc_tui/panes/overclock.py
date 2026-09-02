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
                    with Grid(id="pstate-controls"):
                        with Horizontal(classes="row"):
                            yield Label("PState From")
                            yield ShortcutInput(
                                value="", id="pstate-start", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("PState To")
                            yield ShortcutInput(value="", id="pstate-end", compact=True)
                    with Grid(id="pstate-actions"):
                        yield Button(
                            "Apply PState",
                            id="pstate-limits-apply",
                            classes="red",
                            compact=True,
                        )
                        yield Button(
                            "Reset PState",
                            id="pstate-limits-reset",
                            classes="green",
                            compact=True,
                        )
                    with Grid(id="clock-controls"):
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
                        with Horizontal(classes="row"):
                            # Fabric-clock offset (NVAPI-only ClockClient path,
                            # xbar = domain bit 1). Arch-gated: the controller
                            # disables this row for pre-Pascal GPUs.
                            yield Label("Xbar Offset")
                            yield ShortcutInput(
                                value="0", id="xbar-offset", compact=True
                            )
                        with Horizontal(classes="row"):
                            # Sys = ClkDomains bit3 (pure SYS). RMW: read bit3
                            # current offset, +f, write back. 30系+ bit1 couples
                            # SYS so an Xbar write also drags bit3 — the Sys RMW
                            # stacks on top rather than overwriting the -f cancel.
                            yield Label("Sys Offset")
                            yield ShortcutInput(
                                value="0", id="sys-offset", compact=True
                            )
                        with Horizontal(classes="row"):
                            # Msd = ClkDomains bit5. Pascal: bit5 SET N/A →
                            # controller disables this row on Pascal.
                            yield Label("Msd Offset")
                            yield ShortcutInput(
                                value="0", id="msd-offset", compact=True
                            )
                        with Horizontal(classes="row"):
                            # Host = ClkDomains bit9 (presence via controllable
                            # mask: 0x3FF has bit9, 0xFF does not).
                            yield Label("Host Offset")
                            yield ShortcutInput(
                                value="0", id="host-offset", compact=True
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

                with Vertical(classes="subpane", id="mobile-power-pane") as mobile_pane:
                    mobile_pane.border_title = mnemonic_text("M", "obile Power")
                    with Grid(id="mobile-controls"):
                        with Horizontal(classes="row"):
                            yield Label("PPAB")
                            yield Select(
                                options=[("On", "on"), ("Off", "off")],
                                value="on",
                                id="mobile-ppab",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("D-Notifier")
                            yield Select(
                                options=[
                                    ("D1", 1),
                                    ("D2", 2),
                                    ("D3", 3),
                                    ("D4", 4),
                                    ("D5", 5),
                                ],
                                value=1,
                                id="mobile-dnotifier",
                                allow_blank=False,
                                compact=True,
                            )
                        with Horizontal(classes="row"):
                            yield Label("TGP (W)")
                            yield ShortcutInput(
                                value="100", id="mobile-tgp", compact=True
                            )
                        with Horizontal(classes="row"):
                            yield Label("Target Temp (C)")
                            yield ShortcutInput(
                                value="87", id="mobile-target-temp", compact=True
                            )
                        with Horizontal(classes="row"):
                            # Absolute voltage target on the private VoltRails
                            # path (mobile-only, NVAPI). Bounds + starting
                            # position come from the volt-rail P0 walls in
                            # _on_mobile_limits; disabled until they load.
                            yield Label("Volt Limit (mV)")
                            yield ShortcutInput(
                                value="", id="mobile-volt-limit", compact=True
                            )
                    with Grid(id="mobile-actions"):
                        yield Button(
                            "Apply Mobile",
                            id="mobile-apply",
                            classes="red",
                            compact=True,
                        )
                        yield Button(
                            "Reset Mobile",
                            id="mobile-reset",
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
                            # NVAPI cooler policies (modern GPUs): only
                            # `continuous` actually applies the manual %
                            # level (live A/B); `manual` is the explicit
                            # fallback. Legacy GPUs (≤ Kepler) get the
                            # default/manual dropdown at discovery time —
                            # manual % lands on `manual` there.
                            yield Select(
                                options=[
                                    ("contin.", "continuous"),
                                    ("manual", "manual"),
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
