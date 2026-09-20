"""Chart scaling decision: height and font compensation belong to the
100%-display floor package only — native ≥125% screens keep the approved
1.7in band and unboosted point fonts."""

from __future__ import annotations

from src.tabs.vfcurve.tab import VFCurveTab  # noqa: E402


def test_chart_height_compensated_only_on_floored_displays() -> None:
    assert VFCurveTab._chart_height_in(1.0) == 1.7
    assert VFCurveTab._chart_height_in(1.25) == 2.4
    assert VFCurveTab._chart_height_in(1.5) == 2.4


def test_fs_boosts_only_when_floor_package_active() -> None:
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._font_boost = 1.25
    assert tab._fs(6) == 7.5
    assert tab._fs(9) == 11.25
    tab._font_boost = 1.0
    assert tab._fs(6) == 6.0
    # pre-build safety: missing boost attr reads as no-op
    bare = VFCurveTab.__new__(VFCurveTab)
    assert bare._fs(6) == 6.0
