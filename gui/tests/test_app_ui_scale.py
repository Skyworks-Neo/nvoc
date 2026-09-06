"""UI-scale floor: 100%-scaling displays must not render the whole UI at
1.0x (flat small VF chart, tiny fonts) — App raises any OS DPI factor below
1.25 to 1.25 via CTk's manual multipliers."""

from __future__ import annotations

import pytest

from src.app import App  # noqa: E402


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
