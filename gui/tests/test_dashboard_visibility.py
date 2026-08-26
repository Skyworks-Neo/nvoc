from __future__ import annotations

from types import SimpleNamespace

from src.tabs.dashboard import DashboardTab


class FakeApp:
    def __init__(self, state: str, tab: str = "📊 Dashboard") -> None:
        self._state = state
        self.scheduled: list[int] = []
        self.tabview = SimpleNamespace(get=lambda: tab)

    def state(self) -> str:
        return self._state

    def after(self, delay: int, _callback) -> str:
        self.scheduled.append(delay)
        return "poll-job"


def make_dashboard(
    state: str, tab: str = "📊 Dashboard"
) -> tuple[DashboardTab, FakeApp]:
    app = FakeApp(state, tab)
    dashboard = DashboardTab.__new__(DashboardTab)
    dashboard.app = app
    dashboard._polling = True
    dashboard._poll_job = None
    dashboard._interval_ms = 1000
    dashboard._is_resize_active = False
    dashboard._consecutive_offline = 0
    dashboard._in_offline_backoff = False
    return dashboard, app


def test_poll_tick_defers_query_while_window_is_withdrawn() -> None:
    dashboard, app = make_dashboard("withdrawn")
    fetches: list[bool] = []
    dashboard._fetch_once = lambda: fetches.append(True)

    dashboard._poll_tick()

    assert fetches == []
    assert app.scheduled == [1000]


def test_poll_tick_queries_when_window_is_visible() -> None:
    dashboard, app = make_dashboard("normal")
    fetches: list[bool] = []
    dashboard._fetch_once = lambda: fetches.append(True)

    dashboard._poll_tick()

    assert fetches == [True]
    assert app.scheduled == []


def test_poll_tick_defers_query_while_another_tab_is_visible() -> None:
    dashboard, app = make_dashboard("normal", "⚡ Overclock")
    fetches: list[bool] = []
    dashboard._fetch_once = lambda: fetches.append(True)

    dashboard._poll_tick()

    assert fetches == []
    assert app.scheduled == [1000]
