"""Tray-menu quit must actually terminate the process.

Regression: _quit_app used to schedule _do_shutdown through after() from the
pystray thread — but the thread-safe after() override drops deliveries once
_exiting is set, and _quit_app sets that very flag first. The shutdown request
was silently dropped: tray icon vanished, process survived as a hidden zombie.

Contract now: a foreign-thread quit sets _quit_requested (BEFORE _exiting, so
the poller's re-arm cannot race it) and never touches Tk; the always-running
main-thread poller drains the flag and runs the shutdown. Runs the real
methods on a lightweight App shim (CTk init only, no NVAPI discovery).
Skips cleanly without a display."""

from __future__ import annotations

import threading
import time

import pytest

try:
    import customtkinter as ctk

    ctk.CTk()
    tk_available = True
except Exception:
    tk_available = False

pytestmark = pytest.mark.skipif(not tk_available, reason="no display for real Tk")

from src.app import App  # noqa: E402


class _StubTrayIcon:
    def __init__(self) -> None:
        self.stop_calls = 0

    def stop(self) -> None:
        self.stop_calls += 1


class _QuitShim(App):
    def __init__(self) -> None:
        ctk.CTk.__init__(self)
        self._exiting = False
        self._quit_requested = False
        self._tray_icon: object | None = None
        self.shutdown_calls = 0

    def _do_shutdown(self) -> None:
        self.shutdown_calls += 1


def _pump(app: App, seconds: float) -> None:
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        app.update()


def test_foreign_thread_quit_never_calls_after_and_defers_shutdown() -> None:
    app = _QuitShim()
    try:
        icon = _StubTrayIcon()
        app._tray_icon = icon
        after_calls: list[tuple] = []
        original_after = app.after

        def spy_after(ms, func=None, *args):  # noqa: ANN001
            after_calls.append(func)
            return original_after(ms, func, *args)

        app.after = spy_after  # type: ignore[method-assign]

        worker = threading.Thread(target=app._quit_app)
        worker.start()
        worker.join(timeout=5)

        assert not worker.is_alive()
        assert app._quit_requested is True
        assert app._exiting is True
        assert icon.stop_calls == 1  # tray icon gone (user-visible)
        assert app._tray_icon is None
        assert after_calls == [None] * 0 or all(
            f is None or f.__name__ != "_do_shutdown" for f in after_calls
        ), "shutdown must not be scheduled through after() from a foreign thread"
        assert app.shutdown_calls == 0  # not on the tray thread
    finally:
        app.destroy()


def test_poller_drains_quit_request_on_main_thread() -> None:
    app = _QuitShim()
    try:
        # Simulate the tray-thread quit.
        app._quit_requested = True
        app._exiting = True

        app._poll_single_instance_signal()  # main-thread tick

        assert app.shutdown_calls == 1
    finally:
        app.destroy()


def test_poller_drain_is_single_shot() -> None:
    app = _QuitShim()
    try:
        app._quit_requested = True
        app._exiting = True

        app._poll_single_instance_signal()
        app._poll_single_instance_signal()  # re-fired tick after drain

        assert app.shutdown_calls == 1
    finally:
        app.destroy()


def test_main_thread_quit_runs_shutdown_immediately() -> None:
    app = _QuitShim()
    try:
        app._quit_app()
        assert app.shutdown_calls == 1
        assert app._exiting is True
        assert app._quit_requested is False

        app._quit_app()  # re-entry guard
        assert app.shutdown_calls == 1
    finally:
        app.destroy()


def test_end_to_end_tray_quit_through_the_real_poller() -> None:
    app = _QuitShim()
    try:
        app._tray_icon = _StubTrayIcon()

        worker = threading.Thread(target=app._quit_app)
        worker.start()
        worker.join(timeout=5)

        # Arm the real poller the way __init__ does, then pump the Tk loop:
        # the drain must happen on the main thread within one tick.
        app.after(200, app._poll_single_instance_signal)
        deadline = time.monotonic() + 5.0
        while app.shutdown_calls == 0 and time.monotonic() < deadline:
            _pump(app, 0.1)

        assert app.shutdown_calls == 1, "poller never drained the quit request"
    finally:
        app.destroy()
