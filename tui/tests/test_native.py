from __future__ import annotations

from pathlib import Path

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
