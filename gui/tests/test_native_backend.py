from __future__ import annotations

import threading

from src.backend.base import FanSettings
from src.backend.native import NativeBackend


class FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class FakeApp:
    def __init__(self) -> None:
        self.console = FakeConsole()
        self.actions: list[tuple[str, object]] = []
        self.native = FakeNative()

    def selected_gpu_target(self) -> str:
        return "0x0000"

    def run_native_action(self, description: str, action, on_finished=None) -> None:
        self.actions.append((description, action(self.native)))
        if on_finished is not None:
            on_finished(0)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def discover_gpus(self, backends: str):
        self.calls.append(("discover_gpus", backends))
        return [{"index": 0, "name": "GPU", "gpu_id_hex": "0x0000"}]

    def query_info(self, gpu: str, backends: str):
        self.calls.append(("query_info", gpu, backends))
        return {"gpu_id_hex": gpu, "name": "GPU"}

    def set_fan(self, gpu, backend, fan_id, policy, level):
        self.calls.append(("set_fan", gpu, backend, fan_id, policy, level))


def test_list_gpus_uses_pynvoc_discovery() -> None:
    app = FakeApp()
    backend = NativeBackend(app)
    backend._native = app.native

    code, output, gpus = backend.list_gpus()

    assert code == 0
    assert "pynvoc" in output
    assert gpus == [{"index": 0, "name": "GPU", "gpu_id_hex": "0x0000"}]
    assert app.native.calls == [("discover_gpus", "both")]


def test_run_query_returns_json_output() -> None:
    app = FakeApp()
    backend = NativeBackend(app)
    backend._native = app.native

    code, output, parsed = backend.run_query("0x0000", "info")

    assert code == 0
    assert parsed == {"gpu_id_hex": "0x0000", "name": "GPU"}
    assert '"name": "GPU"' in output
    assert app.native.calls == [("query_info", "0x0000", "both")]


def test_fan_settings_call_native_action() -> None:
    app = FakeApp()
    backend = NativeBackend(app)

    backend.apply_fan_settings(
        FanSettings(
            backend="nvml-cooler",
            fan_id="1",
            policy="manual",
            level=55,
        )
    )

    assert app.actions == [("apply fan settings", "Successfully applied fan settings.")]
    assert app.native.calls == [("set_fan", "0x0000", "nvml-cooler", "1", "manual", 55)]


def test_run_action_rejects_overlapping_action() -> None:
    app = FakeApp()
    backend = NativeBackend(app)
    backend._native = app.native
    first_started = threading.Event()
    release = threading.Event()

    def slow_action(_native):
        first_started.set()
        release.wait(timeout=1)
        return "done"

    assert backend.run_action("slow", slow_action, lambda *_: None, lambda _code: None)
    first_started.wait(timeout=1)
    assert not backend.run_action(
        "second", slow_action, lambda *_: None, lambda _code: None
    )
    release.set()
