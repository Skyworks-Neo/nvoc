from __future__ import annotations

import os


os.environ.setdefault("PYSTRAY_BACKEND", "dummy")

from src.app import App, _is_discovery_offline_error
from src.tabs.dashboard import DashboardTab


class FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, message: str) -> None:
        self.messages.append(message)


class FakeApp:
    def __init__(self) -> None:
        self.console = FakeConsole()
        self.refreshes = 0
        self.scheduled: list[int] = []

    def _refresh_gpu_list(self) -> None:
        self.refreshes += 1

    def after(self, delay: int, _callback) -> str:
        self.scheduled.append(delay)
        return f"after-{len(self.scheduled)}"


def make_dashboard() -> tuple[DashboardTab, FakeApp]:
    app = FakeApp()
    dashboard = DashboardTab.__new__(DashboardTab)
    dashboard.app = app
    dashboard._polling = True
    dashboard._poll_job = None
    dashboard._interval_ms = 1000
    dashboard._is_resize_active = False
    dashboard._consecutive_offline = 0
    dashboard._in_offline_backoff = False
    dashboard._offline_hint_logged = False
    return dashboard, app


def test_offline_error_classification_excludes_unsupported_features() -> None:
    assert _is_discovery_offline_error("NvAPI: API_NOT_INITIALIZED")
    assert _is_discovery_offline_error("NVIDIA_DEVICE_NOT_FOUND")
    assert DashboardTab._looks_like_offline_error("GPU is lost")
    assert not _is_discovery_offline_error("NVAPI_NOT_SUPPORTED")
    assert not DashboardTab._looks_like_offline_error("function not supported")


def test_dashboard_enters_bounded_backoff_after_three_failures() -> None:
    dashboard, app = make_dashboard()

    for _ in range(3):
        dashboard._record_offline_failure()

    assert dashboard._in_offline_backoff is True
    assert app.refreshes == 1
    assert app.console.messages == [
        "[GUI] dGPU probably offline — polling paused, "
        "re-probing for the GPU to come back.\n"
    ]
    assert dashboard._current_poll_interval_ms() == 5000

    dashboard._consecutive_offline = 10
    assert dashboard._current_poll_interval_ms() == 15000


def test_dashboard_backoff_tick_reprobes_instead_of_querying() -> None:
    dashboard, app = make_dashboard()
    dashboard._in_offline_backoff = True
    dashboard._consecutive_offline = 3

    dashboard._poll_tick()

    assert app.refreshes == 1
    assert app.scheduled == [5000]


def test_dashboard_success_restores_normal_polling_state() -> None:
    dashboard, app = make_dashboard()
    dashboard._in_offline_backoff = True
    dashboard._consecutive_offline = 6
    dashboard._offline_hint_logged = True

    dashboard._exit_offline_backoff()

    assert dashboard._in_offline_backoff is False
    assert dashboard._consecutive_offline == 0
    assert dashboard._offline_hint_logged is False
    assert app.console.messages == ["[GUI] dGPU back online — resuming polling.\n"]


def test_app_reprobe_is_single_shot_until_discovery_finishes() -> None:
    app = App.__new__(App)
    app._exiting = False
    app._gpu_reprobe_after_id = None
    scheduled: list[int] = []
    app.after = lambda delay, _callback: scheduled.append(delay) or "after-1"

    app._start_gpu_reprobe()
    app._start_gpu_reprobe()

    assert scheduled == [5000]
    assert app._gpu_reprobe_after_id == "after-1"
