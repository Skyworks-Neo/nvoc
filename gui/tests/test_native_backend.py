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


class RejectingSubmitApp(FakeApp):
    def run_background(self, _name: str, _task) -> None:
        raise RuntimeError("runner stopped")


def test_native_backend_exposes_query_volt_rails() -> None:
    """The VF Curve tab's P0 boundary lines call backend.query_volt_rails
    directly (not just via query_mobile_limits). Guard against the method
    going missing — without it the P0 lines silently never load."""
    assert hasattr(NativeBackend, "query_volt_rails")
    assert callable(getattr(NativeBackend, "query_volt_rails"))


class MobileLimitsFakeNative:
    """pynvoc stand-in covering the mobile-limits fan-out: the PPAB ceiling
    query plus the NVML enforced fallback."""

    def __init__(self, ceiling: dict | None, enforced: float | None = None) -> None:
        self.ceiling = ceiling
        self.enforced = enforced
        self.calls: list[str] = []

    def query_tgp_watt_range(self, gpu: str):
        self.calls.append("tgp")
        return {
            "policy_index": 2,
            "min_watt": 5.0,
            "default_watt": 100.0,
            "max_watt": 140.0,
        }

    def query_dnotifier(self, gpu: str):
        self.calls.append("dnotifier")
        return {"active": "D2", "levels": []}

    def query_target_temp_policies(self, gpu: str):
        self.calls.append("temp")
        return []

    def query_power_ceiling(self, gpu: str):
        self.calls.append("ceiling")
        return self.ceiling

    def query_status(self, gpu: str, backends: str):
        self.calls.append("status")
        return {"power_limit_w": self.enforced}

    def query_volt_rails(self, gpu: str):
        self.calls.append("volt_rail")
        return None


def _backend_with_mobile_fake(fake: MobileLimitsFakeNative) -> NativeBackend:
    app = FakeApp()
    backend = NativeBackend(app)
    backend._native = fake
    return backend


def test_mobile_limits_power_wall_prefers_ceiling() -> None:
    """power_limit_w must be the PPAB ceiling (min of requested TGP and the
    active D-Notifier cap) when the private query answers — the value the
    TGP slider anchors to after a D-Notifier apply."""
    fake = MobileLimitsFakeNative(
        ceiling={
            "policy_index": 2,
            "default_watt": 100.0,
            "requested_watt": 100.0,
            "dnotify_watt": 55.0,
            "ceiling_watt": 55.0,
        },
        enforced=123.0,
    )
    backend = _backend_with_mobile_fake(fake)

    data = backend.query_mobile_limits("0x0000")

    assert data["power_limit_w"] == 55.0
    assert "ceiling" in fake.calls


def test_mobile_limits_power_wall_falls_back_to_nvml() -> None:
    """Where the private power-policy family is unavailable (ceiling None),
    keep the NVML enforced power limit as the wall — the pre-PPAB behavior."""
    fake = MobileLimitsFakeNative(ceiling=None, enforced=170.0)
    backend = _backend_with_mobile_fake(fake)

    data = backend.query_mobile_limits("0x0000")

    assert data["power_limit_w"] == 170.0


def test_mobile_limits_power_wall_none_when_both_unavailable() -> None:
    fake = MobileLimitsFakeNative(ceiling=None, enforced=None)
    backend = _backend_with_mobile_fake(fake)

    data = backend.query_mobile_limits("0x0000")

    assert data["power_limit_w"] is None


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


def test_run_action_resets_running_flag_when_submit_fails() -> None:
    app = RejectingSubmitApp()
    backend = NativeBackend(app)
    backend._native = app.native
    output: list[str] = []
    finished: list[int] = []

    assert not backend.run_action(
        "fail",
        lambda _native: "done",
        lambda text, _level: output.append(text),
        finished.append,
    )

    assert finished == [-1]
    assert any("Failed to schedule native action" in message for message in output)
    assert not backend._action_running
