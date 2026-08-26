from __future__ import annotations

import os
from unittest.mock import Mock

os.environ.setdefault("PYSTRAY_BACKEND", "dummy")

from src.app import App


class RefreshHarness:
    refresh_after_native_action = App.refresh_after_native_action
    _refresh_after_native_action_now = App._refresh_after_native_action_now

    def __init__(self) -> None:
        self._refresh_chain_after_id = None
        self.scheduled: list[tuple[int, object, str]] = []
        self.cancelled: list[str] = []
        self._invalidate_query_cache = Mock()
        self._query_gpu_get = Mock()
        self._query_overclock_status = Mock()
        self.tab_dashboard = Mock()
        self.tab_vfcurve = Mock()

    def after(self, delay_ms: int, callback) -> str:
        timer_id = f"refresh-{len(self.scheduled) + 1}"
        self.scheduled.append((delay_ms, callback, timer_id))
        return timer_id

    def after_cancel(self, timer_id: str) -> None:
        self.cancelled.append(timer_id)


def test_native_refresh_burst_keeps_only_latest_timer() -> None:
    app = RefreshHarness()

    app.refresh_after_native_action()
    app.refresh_after_native_action()

    assert [delay for delay, _callback, _id in app.scheduled] == [300, 300]
    assert app.cancelled == ["refresh-1"]
    assert app._refresh_chain_after_id == "refresh-2"


def test_deferred_native_refresh_runs_each_query_once() -> None:
    app = RefreshHarness()
    app.refresh_after_native_action(curve_affecting=True)

    app.scheduled[-1][1]()

    assert app._refresh_chain_after_id is None
    app._invalidate_query_cache.assert_called_once_with()
    app._query_gpu_get.assert_called_once_with()
    app._query_overclock_status.assert_called_once_with()
    app.tab_dashboard._fetch_once.assert_called_once_with()
    app.tab_vfcurve._refresh_curve.assert_called_once_with()
