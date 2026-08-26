from __future__ import annotations

import json
import threading
import time

from src.config import Config


def test_set_defers_write_until_flush(tmp_path) -> None:
    config = Config(str(tmp_path))
    writes: list[dict] = []
    config._write_snapshot = lambda snapshot: writes.append(snapshot)

    config.set("first", 1)
    config.set("second", 2)

    assert writes == []

    config.flush()

    assert writes == [{**config.data, "first": 1, "second": 2}]
    config.close()
    assert not config._flusher.is_alive()


def test_background_flusher_coalesces_burst(tmp_path) -> None:
    original_delay = Config._FLUSH_DELAY_S
    Config._FLUSH_DELAY_S = 0.01
    try:
        config = Config(str(tmp_path))
        writes: list[dict] = []
        config._write_snapshot = lambda snapshot: writes.append(snapshot)

        config.set("value", 1)
        config.set("value", 2)

        deadline = time.monotonic() + 1.0
        while not writes and time.monotonic() < deadline:
            time.sleep(0.01)

        assert len(writes) == 1
        assert writes[0]["value"] == 2
        config.close()
    finally:
        Config._FLUSH_DELAY_S = original_delay


def test_close_persists_pending_update_and_stops_thread(tmp_path) -> None:
    config = Config(str(tmp_path))
    config.set("last_gpu_idx", 4)

    config.close()

    saved = json.loads((tmp_path / "nvoc_gui_config.json").read_text())
    assert saved["last_gpu_idx"] == 4
    assert not config._flusher.is_alive()


def test_close_cannot_be_overwritten_by_older_background_snapshot(tmp_path) -> None:
    original_delay = Config._FLUSH_DELAY_S
    Config._FLUSH_DELAY_S = 0
    try:
        config = Config(str(tmp_path))
        first_write_started = threading.Event()
        release_first_write = threading.Event()
        writes: list[int] = []

        def write_snapshot(snapshot) -> None:
            if not writes:
                first_write_started.set()
                assert release_first_write.wait(timeout=1)
            writes.append(snapshot["value"])

        config._write_snapshot = write_snapshot
        config.set("value", 1)
        assert first_write_started.wait(timeout=1)

        config.set("value", 2)
        close_thread = threading.Thread(target=config.close)
        close_thread.start()
        release_first_write.set()
        close_thread.join(timeout=1)

        assert not close_thread.is_alive()
        assert writes == [1, 2]
        assert not config._flusher.is_alive()
    finally:
        Config._FLUSH_DELAY_S = original_delay
