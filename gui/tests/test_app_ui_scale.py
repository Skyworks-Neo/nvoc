"""Chart-scale floor: 100%-scaling displays must not render the VF-curve
chart flat and small — App raises the CHART's effective scale to 1.25 on
OS DPI factors below that. The widget/window half of the old floor was
reverted (de-CTk'd panels draw point-sized fonts CTk scaling can't move,
so the widget inflation only made big buttons around small text): the
floor must never touch CTk's global multipliers again."""

from __future__ import annotations

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


def test_floor_does_not_touch_ctk_global_scaling(monkeypatch) -> None:
    # Regression: the floor used to drive ctk.set_widget_scaling /
    # set_window_scaling — inflated chrome around unchanged point fonts.
    # It must only record the multiplier for the chart package now.
    calls: list[tuple[str, float]] = []
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


def test_effective_ui_scale_is_os_times_multiplier(monkeypatch) -> None:
    # The quantity the chart package renders at: native 150% passes
    # through; a 100% screen rides the 1.25 floor.
    app = _bare_app()
    for os_factor, expected in ((1.5, 1.5), (1.25, 1.25), (1.0, 1.25)):
        monkeypatch.setattr(App, "_os_dpi_factor", lambda self, f=os_factor: f)
        assert app._effective_ui_scale() == pytest.approx(expected)
