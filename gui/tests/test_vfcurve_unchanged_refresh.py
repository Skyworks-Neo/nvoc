from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import Mock

from src.tabs.vfcurve import VFCurveTab


def make_tab() -> VFCurveTab:
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._voltages = [800.0]
    tab._frequencies = [1500.0]
    tab._defaults = [1400.0]
    tab._sel_start = None
    tab._sel_end = None
    tab._pending_lock_mv = None
    tab._redraw = Mock()
    tab.app = SimpleNamespace(
        console=Mock(),
        _analyze_vfp_offsets=Mock(return_value=(False, None)),
        _apply_vfp_offset_state=Mock(),
        tab_overclock=None,
    )
    return tab


def write_curve(path, frequency_khz: int) -> None:
    path.write_text(
        "voltage,frequency,delta,default_frequency\n"
        f"800000,{frequency_khz},0,1400000\n",
        encoding="utf-8",
    )


def test_load_csv_skips_redraw_when_curve_is_unchanged(tmp_path) -> None:
    tab = make_tab()
    csv_path = tmp_path / "curve.csv"
    write_curve(csv_path, 1_500_000)

    tab._load_csv(str(csv_path))

    tab._redraw.assert_not_called()
    tab.app.console.append.assert_not_called()


def test_load_csv_redraws_when_curve_changes(tmp_path) -> None:
    tab = make_tab()
    csv_path = tmp_path / "curve.csv"
    write_curve(csv_path, 1_515_000)

    tab._load_csv(str(csv_path))

    assert tab._frequencies == [1515.0]
    tab._redraw.assert_called_once_with()
