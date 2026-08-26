from __future__ import annotations

import inspect
import os

os.environ.setdefault("PYSTRAY_BACKEND", "dummy")

from src.app import App


def test_initial_layout_flush_does_not_dispatch_full_event_loop() -> None:
    source = inspect.getsource(App._build_ui)

    assert "self.update_idletasks()" in source
    assert "self.update()" not in source
