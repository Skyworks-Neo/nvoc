from __future__ import annotations

import sys
import types


fan_control_stub = types.ModuleType("src.panes.fan_control")


class FanControlPane:
    pass


fan_control_stub.FanControlPane = FanControlPane
sys.modules.setdefault("src.panes.fan_control", fan_control_stub)

from src.tabs.overclock import OverclockTab  # noqa: E402


class FakeVar:
    def __init__(self, value: str) -> None:
        self.value = value

    def get(self) -> str:
        return self.value

    def set(self, value: str) -> None:
        self.value = value


class FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def set_clock_offset(
        self, gpu: str, backend: str, domain: str, offset: int, pstate: str
    ) -> None:
        self.calls.append(("set_clock_offset", gpu, backend, domain, offset, pstate))


class FakeApp:
    def __init__(self) -> None:
        self.console = FakeConsole()
        self.native = FakeNative()
        self.actions: list[str] = []

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def run_native_action(self, description: str, action, on_finished=None) -> bool:
        self.actions.append(description)
        action(self.native)
        if on_finished is not None:
            on_finished(0)
        return True

    def run_native_action_chain(self, commands) -> None:
        for description, action in commands:
            self.actions.append(description)
            action(self.native)


def make_tab() -> tuple[OverclockTab, FakeApp]:
    app = FakeApp()
    tab = OverclockTab.__new__(OverclockTab)
    tab.app = app
    tab._syncing = False
    tab._is_resize_active = False
    tab._pending_vfp_state = None
    tab._is_vfp_mode = False
    tab._vfp_uniform_offset_mhz = None
    tab.core_var = FakeVar("125")
    tab.mem_var = FakeVar("600")
    tab.oc_api_var = FakeVar("NVAPI")
    return tab, app


def test_vfp_state_does_not_replace_core_offset_display() -> None:
    tab, _app = make_tab()

    tab.set_vfp_state(True, 50)

    assert tab._is_vfp_mode is True
    assert tab._vfp_uniform_offset_mhz == 50
    assert tab.core_var.get() == "125"


def test_core_apply_still_runs_while_vfp_offset_exists() -> None:
    tab, app = make_tab()
    tab.set_vfp_state(True, None)

    tab._apply_core_only()

    assert app.actions == ["apply core offset"]
    assert app.native.calls == [
        ("set_clock_offset", "GPU0", "nvapi", "core", 125, "P0")
    ]


def test_apply_offset_runs_core_and_memory_while_vfp_offset_exists() -> None:
    tab, app = make_tab()
    tab.set_vfp_state(True, 50)

    tab._apply_oc()

    assert app.actions == ["apply core offset", "apply memory offset"]
    assert app.native.calls == [
        ("set_clock_offset", "GPU0", "nvapi", "core", 125, "P0"),
        ("set_clock_offset", "GPU0", "nvapi", "memory", 600, "P0"),
    ]
