from __future__ import annotations

from pathlib import Path
from threading import Event

from nvoc_tui.native import NativeService


class FakeNative:
    def query_info(self, gpu, _backends):
        return {"gpu": gpu, "name": "Test GPU"}

    def query_status(self, gpu, _backends):
        return {"gpu": gpu, "temperature_c": 65}

    def query_settings(self, gpu, _backends):
        return {"gpu": gpu, "core_offset_mhz": 100}


def test_run_query_returns_loggable_native_output() -> None:
    service = NativeService(Path.cwd())
    service._native = FakeNative()

    code, output, parsed = service.run_query("0x1234", "info")

    assert code == 0
    assert parsed == {"gpu": "0x1234", "name": "Test GPU"}
    assert output.startswith("> native info --gpu=0x1234\n")
    assert '"name": "Test GPU"' in output


def test_submit_query_serializes_jobs_and_survives_failures() -> None:
    service = NativeService(Path.cwd())
    first_started = Event()
    release_first = Event()
    second_finished = Event()
    calls: list[str] = []

    def first() -> None:
        calls.append("first-start")
        first_started.set()
        assert release_first.wait(timeout=1)
        calls.append("first-end")

    def second() -> None:
        calls.append("second")
        second_finished.set()

    service.submit_query(first)
    assert first_started.wait(timeout=1)
    service.submit_query(second)
    assert not second_finished.wait(timeout=0.05)
    release_first.set()
    assert second_finished.wait(timeout=1)
    assert calls == ["first-start", "first-end", "second"]

    def fail() -> None:
        raise RuntimeError("query failed")

    recovered = Event()
    service.submit_query(fail)
    service.submit_query(recovered.set)
    assert recovered.wait(timeout=1)
