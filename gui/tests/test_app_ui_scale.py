"""Chart-scale floor and Linux DPI propagation.

100%-scaling displays must not render the VF-curve chart flat and small —
App raises the CHART's effective scale to 1.25 on OS DPI factors below that.
The floor itself never touches CTk's global widget/window multipliers.

Separately, CustomTkinter 6.0.0 returns 1.0 from its ScalingTracker on
Linux, while Tk's Xft backend renders point-size text at Xft.dpi. The real
X11 DPI factor is therefore mirrored into CTk and Tk scaling so geometry
and fonts agree; Windows keeps CTk's own per-monitor DPI tracking.
"""

from __future__ import annotations

import sys

import pytest

import customtkinter as ctk

from src.app import App  # noqa: E402


def _bare_app() -> App:
    """App instance without __init__ (no CTk window / NVAPI discovery).

    Every instance attribute ``_apply_min_ui_scale`` reads must exist up
    front: a CTk subclass falls back to tkinter's ``__getattr__`` for
    missing names, which recurses to death without a live Tcl interpreter.
    """
    app = App.__new__(App)
    app._exiting = False
    app._ui_scale_multiplier_applied = None
    app._ui_scale_logged = False
    app._ui_scale_os_scale_applied = None
    app._linux_ui_scale_applied = None
    app.console = None
    return app


def test_100_percent_screen_raised_to_floor() -> None:
    assert App._min_ui_scale_multiplier(1.0) == pytest.approx(1.25)


def test_factors_at_or_above_floor_untouched() -> None:
    assert App._min_ui_scale_multiplier(1.25) == 1.0
    assert App._min_ui_scale_multiplier(1.5) == 1.0
    assert App._min_ui_scale_multiplier(2.0) == 1.0


def test_sub_floor_factor_scales_proportionally() -> None:
    assert App._min_ui_scale_multiplier(0.8) == pytest.approx(1.25 / 0.8)


def test_degenerate_factor_does_not_explode() -> None:
    assert App._min_ui_scale_multiplier(0.0) == pytest.approx(1.25 / 0.5)


def test_ui_scale_is_clamped_and_degenerate_safe() -> None:
    assert App._clamp_ui_scale(0.0) == 1.0
    assert App._clamp_ui_scale(-3.0) == 1.0
    assert App._clamp_ui_scale(2.0) == 2.0
    assert App._clamp_ui_scale(99.0) == App._MAX_UI_SCALE


def test_windows_floor_does_not_touch_ctk_global_scaling(monkeypatch) -> None:
    # Regression: the floor used to drive ctk.set_widget_scaling /
    # set_window_scaling — inflated chrome around unchanged point fonts.
    # It must only record the multiplier for the chart package on Windows.
    calls: list[tuple[str, float]] = []
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.setattr(
        ctk, "set_widget_scaling", lambda v: calls.append(("widget", v))
    )
    monkeypatch.setattr(
        ctk, "set_window_scaling", lambda v: calls.append(("window", v))
    )
    monkeypatch.setattr(App, "_os_dpi_factor", lambda self: 1.0)

    app = _bare_app()
    app._apply_min_ui_scale()

    assert calls == []
    assert app._ui_scale_multiplier_applied == pytest.approx(1.25)


def test_linux_ui_scaling_syncs_ctk_and_tk(monkeypatch) -> None:
    # CustomTkinter hardcodes 1.0 on Linux while Xft renders points at
    # Xft.dpi: both CTk's geometry/font multipliers and Tk's pixel<->point
    # scaling must move to the Xft factor together.
    calls: list[tuple[str, float]] = []
    tk_calls: list[tuple] = []

    class _FakeTk:
        def call(self, *args):
            tk_calls.append(args)
            return 0

    def _exists() -> bool:
        return True

    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(
        ctk, "set_widget_scaling", lambda v: calls.append(("widget", v))
    )
    monkeypatch.setattr(
        ctk, "set_window_scaling", lambda v: calls.append(("window", v))
    )

    app = _bare_app()
    app.tk = _FakeTk()
    app.winfo_exists = _exists
    app._apply_linux_ui_scaling(2.0)

    assert calls == [("widget", 2.0), ("window", 2.0)]
    assert tk_calls == [("tk", "scaling", pytest.approx(2.6666666666666665))]
    assert app._linux_ui_scale_applied == 2.0

    # Idempotent: a second call with the same factor changes nothing.
    app._apply_linux_ui_scaling(2.0)
    assert len(calls) == 2 and len(tk_calls) == 1


def test_linux_ui_scaling_ignores_non_linux(monkeypatch) -> None:
    calls: list[tuple[str, float]] = []
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.setattr(
        ctk, "set_widget_scaling", lambda v: calls.append(("widget", v))
    )
    app = _bare_app()
    app._apply_linux_ui_scaling(2.0)
    assert calls == []


def test_effective_ui_scale_is_os_times_multiplier(monkeypatch) -> None:
    # The quantity the chart package renders at: native 150% passes
    # through; a 100% screen rides the 1.25 floor.
    app = _bare_app()
    for os_factor, expected in ((1.5, 1.5), (1.25, 1.25), (1.0, 1.25), (2.0, 2.0)):
        monkeypatch.setattr(App, "_os_dpi_factor", lambda self, f=os_factor: f)
        assert app._effective_ui_scale() == pytest.approx(expected)
