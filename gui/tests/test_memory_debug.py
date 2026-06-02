from __future__ import annotations

import tracemalloc

from src import memory_debug


def test_configure_memory_debug_from_env_enables_tracing(monkeypatch) -> None:
    if tracemalloc.is_tracing():
        tracemalloc.stop()
    monkeypatch.setenv("NVOC_GUI_MEMORY_LOG_INTERVAL_SEC", "1")
    monkeypatch.setenv("NVOC_GUI_MEMORY_TOP", "2")

    config = memory_debug.configure_memory_debug_from_env()

    assert config is not None
    assert config.interval_seconds == 10.0
    assert config.top_stats == 2
    assert tracemalloc.is_tracing()
    tracemalloc.stop()


def test_format_memory_report_includes_requested_number_of_top_stats(
    monkeypatch,
) -> None:
    if not tracemalloc.is_tracing():
        tracemalloc.start(25)
    monkeypatch.setattr(memory_debug, "current_rss_bytes", lambda: 1024)

    report = memory_debug.format_memory_report(top_stats=1)

    assert report.startswith("[GUI][mem] RSS=1.0 KiB")
    assert report.count("[GUI][mem] #") <= 1
