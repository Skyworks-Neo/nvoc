"""Pre-mainloop geometry auto-pin: ANY bare ``app.geometry(...)`` call made
while the window is unmapped must survive CTk's map-time scaling re-issue —
the "test window ignores the code's startup size" regression. Runs the real
App.geometry override on a lightweight App subclass (CTk init only, no NVAPI
discovery / tab construction). Skips cleanly without a display."""

from __future__ import annotations

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


class _AppShim(App):
    def __init__(self) -> None:
        ctk.CTk.__init__(self)
        self._startup_geometry_reapply = None
        self._exiting = False


def _pump(app: App, seconds: float) -> None:
    """Run the event loop for `seconds` (map events + after timers)."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        app.update()


def _expected(app: App) -> tuple[float, float]:
    eff = ctk.ScalingTracker.get_widget_scaling(app)
    return 1400 * eff, 900 * eff


def test_bare_pre_map_geometry_survives_map() -> None:
    app = _AppShim()
    try:
        app.geometry("1400x900+60+40")  # plain call — the auto-pin contract
        assert app._startup_geometry_reapply is not None  # pinned while unmapped
        _pump(app, 0.8)  # map + CTk's map-time re-issue + our after-Map re-apply

        w, h = app.winfo_width(), app.winfo_height()
        ew, eh = _expected(app)
        assert abs(w - ew) < 24 and abs(h - eh) < 24, (
            f"geometry clobbered: {w}x{h}, expected ~{round(ew)}x{round(eh)}"
        )
        _pump(app, 0.3)  # still pinned one beat later
        assert abs(app.winfo_width() - ew) < 24
    finally:
        app.destroy()


def test_mapped_geometry_calls_do_not_arm_the_pin() -> None:
    app = _AppShim()
    try:
        app.geometry("1400x900+60+40")
        _pump(app, 0.8)  # mapped now, pin retired
        assert app._startup_geometry_reapply is None
        app.geometry("1000x700+10+10")  # runtime resize — no pin machinery
        assert app._startup_geometry_reapply is None
    finally:
        app.destroy()
