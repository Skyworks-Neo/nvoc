"""Real-Tk construction smoke tests for ``OverclockTab._make_slider_row``.

The behavioural tests (test_overclock.py) build tabs with ``__new__`` and
stub widgets — they never execute the row-construction closure chain, which
is exactly how a bare-name reference to a class staticmethod (a NameError
fired on the first Core row at construction, leaving a half-built tab —
the "rendering exploded" GUI) survived a fully green suite. These tests
build the row with REAL widgets so name-resolution and wiring bugs die here.

Tk-in-pytest needs a display; this suite runs on the dev machine (Windows,
interactive session). Skips cleanly when no display is available.
"""

from __future__ import annotations

import sys
import types

import pytest

try:
    import tkinter as tk

    tk.Tk()
    tk_available = True
except Exception:
    tk_available = False

pytestmark = pytest.mark.skipif(not tk_available, reason="no display for real Tk")


fan_control_stub = types.ModuleType("src.tabs.dashboard.sections.fan")


class FanControlPane:
    pass


fan_control_stub.FanControlPane = FanControlPane
sys.modules.setdefault("src.tabs.dashboard.sections.fan", fan_control_stub)

from src.tabs.dashboard.sections.overclock import OverclockTab  # noqa: E402


class _App:
    """Minimal app surface _make_slider_row + the toggle touch."""

    class _Backend:
        @staticmethod
        def query_private_freq_domain_info(_gpu: str) -> dict:
            return {"entries": [{"bit": 0, "values_kHz": [0, -12500]}]}

    def __init__(self) -> None:
        self.backend = self._Backend()

    @staticmethod
    def selected_gpu_target() -> str:
        return "GPU0"


def _make_tab(root: tk.Tk) -> OverclockTab:
    tab = OverclockTab.__new__(OverclockTab)
    tab.app = _App()
    tab._syncing = False
    return tab


def test_slider_row_constructs_with_volt_bit_without_nameerror() -> None:
    """The full construction closure chain runs clean on a volt-toggled row:
    _fmt resolves through the nested scope, the chip grids in column 3, and
    every per-plane attribute lands on the slider."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, entry, var, btn = tab._make_slider_row(
            frame,
            "Core:",
            -200,
            200,
            0,
            step=2.5,
            signed=True,
            unit="MHz",
            decimals=1,
            entry_width=8,
            volt_bit=0,
        )
        assert var.get() == "+0.0"
        assert slider._oc_volt_bit == 0
        assert slider._oc_volt_mode is False
        assert slider._oc_unit_var is var
        assert slider._oc_freq_min == -200
        assert slider._oc_freq_max == 200
        assert slider._oc_freq_step == 2.5
        assert slider._oc_freq_decimals == 1
        assert slider._oc_volt_min == -300
        assert slider._oc_volt_max == 300
        # the chip is a real gridded tk.Label in column 3
        toggle = slider._oc_unit_toggle
        assert isinstance(toggle, tk.Label)
        assert toggle.cget("text") == "MHz"
        assert toggle.grid_info()["column"] == 3
        assert isinstance(entry, object)
        assert isinstance(btn, object)
    finally:
        root.destroy()


def test_toggle_round_trip_on_real_widgets() -> None:
    """Chip text flips MHz → mV → MHz on the real label; the slider/entry
    reconfigure onto ±300/1-decimal and back onto the construction plane;
    the mV anchor comes from the backend's slot1 (−12500 µV → −12.5 mV)."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, _entry, var, _btn = tab._make_slider_row(
            frame,
            "Core:",
            -200,
            200,
            0,
            step=2.5,
            signed=True,
            unit="MHz",
            decimals=1,
            entry_width=8,
            volt_bit=0,
        )

        tab._toggle_row_unit(slider)
        assert slider._oc_unit_toggle.cget("text") == "mV"
        assert slider._oc_min == -300
        assert slider._oc_max == 300
        assert slider._oc_step == 0.1
        assert slider._oc_decimals == 1
        # slot1 = −12500 µV → −12.5 mV (0.1-grid float fuzz ≈ 1e-13)
        assert slider.get() == pytest.approx(-12.5, abs=1e-6)
        assert var.get() == "-12.5"

        tab._toggle_row_unit(slider)
        assert slider._oc_unit_toggle.cget("text") == "MHz"
        assert slider._oc_min == -200
        assert slider._oc_max == 200
        assert slider._oc_step == 2.5
        assert slider._oc_decimals == 1  # Core's construction decimals
        assert slider.get() == 0
        assert var.get() == "+0.0"
    finally:
        root.destroy()


def test_unit_toggle_visibility_round_trip_on_real_widget() -> None:
    """grid_remove/grid show-hide preserves the chip's grid cell (the
    remembered-options idiom) — the page-0 rows rely on it."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, *_ = tab._make_slider_row(
            frame,
            "Core:",
            -200,
            200,
            0,
            step=2.5,
            signed=True,
            unit="MHz",
            volt_bit=0,
        )
        toggle = slider._oc_unit_toggle

        tab._set_unit_toggle_visible(slider, False)
        assert toggle.grid_info() == {}  # grid_remove releases the slot
        tab._set_unit_toggle_visible(slider, True)
        info = toggle.grid_info()
        assert info["column"] == 3  # remembered options restored
        assert info["row"] == 0
    finally:
        root.destroy()


def _rendered_thumb_value(slider) -> float:
    """Read the thumb position from the CANVAS ITEMS, not the state.

    The stale-render bug this pins was invisible to state assertions: the
    slider's _value/_from/_to were always correct while the canvas kept
    drawing the construction range. Measure the drawn oval instead.
    """
    ovals = slider._canvas.find_withtag("all")
    coords = None
    for item in ovals:
        if slider._canvas.type(item) == "oval":
            coords = slider._canvas.coords(item)
    assert coords is not None, "no thumb oval rendered"
    x_center = (coords[0] + coords[2]) / 2
    width = max(1, slider._canvas.winfo_width())
    return slider._x_to_value(x_center, width)


def test_range_reconfigure_with_same_value_repaints() -> None:
    """The startup stale-render regression: 'get' lands first and pins the
    value (100) on the construction range (50..150), then 'info' lands and
    reconfigures to the real range (70..100) with the SAME value. Both
    redraws used to be skipped as no-ops and the canvas rendered the
    construction range forever — the thumb visibly stuck at 33% with the
    state reading 70..100@100. configure() must mark the geometry dirty so
    the no-op set() still repaints."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, _entry, var, _btn = tab._make_slider_row(
            frame, "Pwr Limit:", 50, 150, 100, step=1
        )
        root.update_idletasks()

        # get lands first: power_limit_current=100 on the construction range
        tab._set_slider_value(slider, var, 100)
        root.update_idletasks()
        assert slider.get() == 100.0

        # info lands: the real TDP range with the same default/current
        tab._reconfigure_slider(slider, var, 70, 100, 100, step=1)
        root.update_idletasks()

        assert (slider.cget("from_"), slider.cget("to")) == (70.0, 100.0)
        assert slider.get() == 100.0
        # THE assertion: the drawn thumb sits at the far right (value 100),
        # not at the stale 50..150@100 position (33%).
        assert _rendered_thumb_value(slider) == pytest.approx(100.0)
    finally:
        root.destroy()


def test_range_reconfigure_reverse_order_repaints() -> None:
    """Same fix, info-before-get ordering: reconfigure onto 70..100 then a
    same-value set(100) must not regress the canvas to a stale render."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, _entry, var, _btn = tab._make_slider_row(
            frame, "Pwr Limit:", 50, 150, 100, step=1
        )
        root.update_idletasks()

        tab._reconfigure_slider(slider, var, 70, 100, 100, step=1)
        root.update_idletasks()
        tab._set_slider_value(slider, var, 100)
        root.update_idletasks()

        assert slider.get() == 100.0
        assert _rendered_thumb_value(slider) == pytest.approx(100.0)
    finally:
        root.destroy()


def test_value_move_on_unchanged_range_repaints() -> None:
    """A real value change still repaints (the no-op fast path stays)."""
    root = tk.Tk()
    root.withdraw()
    try:
        tab = _make_tab(root)
        frame = tk.Frame(root)
        slider, _entry, var, _btn = tab._make_slider_row(
            frame, "Pwr Limit:", 70, 100, 85, step=1
        )
        root.update_idletasks()
        tab._set_slider_value(slider, var, 100)
        root.update_idletasks()
        assert _rendered_thumb_value(slider) == pytest.approx(100.0)
        tab._set_slider_value(slider, var, 70)
        root.update_idletasks()
        assert _rendered_thumb_value(slider) == pytest.approx(70.0)
    finally:
        root.destroy()
