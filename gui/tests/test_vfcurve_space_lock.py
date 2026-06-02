from __future__ import annotations

from src.tabs.vfcurve import VFCurveTab


class FakeVar:
    def __init__(self, value: str) -> None:
        self.value = value

    def get(self) -> str:
        return self.value

    def set(self, value: str) -> None:
        self.value = value


class FakeCanvas:
    def __init__(self) -> None:
        self.draw_count = 0

    def draw(self) -> None:
        self.draw_count += 1


class FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def reset_vfp_lock(self, gpu: str) -> None:
        self.calls.append(("reset_vfp_lock", gpu))

    def set_vfp_voltage_lock(
        self, gpu: str, point: int | None, voltage_uv: int | None, immediate: bool
    ) -> None:
        self.calls.append(("set_vfp_voltage_lock", gpu, point, voltage_uv, immediate))

    def set_vfp_frequency_lock(
        self, gpu: str, domain: str, upper_khz: int, lower_khz: int
    ) -> None:
        self.calls.append(("set_vfp_frequency_lock", gpu, domain, upper_khz, lower_khz))

    def reset_vfp_frequency_lock(self, gpu: str, domain: str) -> None:
        self.calls.append(("reset_vfp_frequency_lock", gpu, domain))

    def set_locked_clocks(
        self, gpu: str, backend: str, domain: str, min_mhz: int, max_mhz: int
    ) -> None:
        self.calls.append(("set_locked_clocks", gpu, backend, domain, min_mhz, max_mhz))

    def reset_core_clocks(self, gpu: str, backend: str) -> None:
        self.calls.append(("reset_core_clocks", gpu, backend))

    def reset_mem_clocks(self, gpu: str, backend: str) -> None:
        self.calls.append(("reset_mem_clocks", gpu, backend))


class FakeApp:
    def __init__(self) -> None:
        self.console = FakeConsole()
        self.native = FakeNative()
        self.actions: list[tuple[str, str | None]] = []

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def after(self, _delay: int, callback) -> None:
        callback()

    def run_native_action(self, description: str, action, on_finished=None) -> None:
        output = action(self.native)
        self.actions.append((description, output))
        if on_finished is not None:
            on_finished(0)


def make_tab(api: str, start: int = 1, end: int = 1) -> tuple[VFCurveTab, FakeApp]:
    app = FakeApp()
    tab = VFCurveTab.__new__(VFCurveTab)
    tab.app = app
    tab._voltages = [800.0, 900.0, 1000.0, 1100.0]
    tab._frequencies = [1400.0, 1500.0, 1600.0, 1700.0]
    tab._defaults = list(tab._frequencies)
    tab._sel_start = start
    tab._sel_end = end
    tab._locked_points = set()
    tab._freq_core_lock = None
    tab._freq_mem_lock = None
    tab._freq_core_lock_backend = None
    tab._freq_mem_lock_backend = None
    tab.freq_lock_api_var = FakeVar(api)
    tab.adj_start_var = FakeVar("0")
    tab.adj_end_var = FakeVar("0")
    tab.adj_delta_var = FakeVar("0")
    tab.core_lock_min_var = FakeVar("0")
    tab.core_lock_max_var = FakeVar("0")
    tab.mem_lock_min_var = FakeVar("0")
    tab.mem_lock_max_var = FakeVar("0")
    tab.canvas = FakeCanvas()
    tab._redraw = lambda: None
    return tab, app


def press_space(tab: VFCurveTab) -> None:
    assert tab._on_space_key() == "break"


def test_space_nvapi_single_cycles_voltage_freq_unlock() -> None:
    tab, app = make_tab("NVAPI")

    press_space(tab)
    assert app.native.calls[-1] == (
        "set_vfp_voltage_lock",
        "GPU0",
        1,
        None,
        False,
    )
    assert tab._locked_points == {1}
    assert tab._freq_core_lock is None

    press_space(tab)
    assert app.native.calls[-2:] == [
        ("reset_vfp_lock", "GPU0"),
        ("set_vfp_frequency_lock", "GPU0", "core", 1500000, 1500000),
    ]
    assert tab._locked_points == set()
    assert tab._freq_core_lock == (1500, 1500)
    assert tab._freq_core_lock_backend == "nvapi"

    press_space(tab)
    assert app.native.calls[-1] == ("reset_vfp_frequency_lock", "GPU0", "core")
    assert tab._freq_core_lock is None
    assert tab._freq_core_lock_backend is None


def test_space_nvml_single_cycles_freq_unlock_without_voltage_lock() -> None:
    tab, app = make_tab("NVML")

    press_space(tab)
    assert app.native.calls == [
        ("set_locked_clocks", "GPU0", "nvml", "core", 1500, 1500)
    ]
    assert tab._locked_points == set()
    assert tab._freq_core_lock == (1500, 1500)
    assert tab._freq_core_lock_backend == "nvml"

    press_space(tab)
    assert app.native.calls[-1] == ("reset_core_clocks", "GPU0", "nvml")
    assert not any(call[0] == "set_vfp_voltage_lock" for call in app.native.calls)
    assert tab._freq_core_lock is None
    assert tab._freq_core_lock_backend is None


def test_space_nvapi_range_cycles_freq_range_unlock() -> None:
    tab, app = make_tab("NVAPI", start=1, end=3)

    press_space(tab)
    assert app.native.calls == [
        ("set_vfp_frequency_lock", "GPU0", "core", 1700000, 1500000)
    ]
    assert tab._freq_core_lock == (1500, 1700)
    assert tab._freq_core_lock_backend == "nvapi"

    press_space(tab)
    assert app.native.calls[-1] == ("reset_vfp_frequency_lock", "GPU0", "core")
    assert tab._freq_core_lock is None


def test_space_nvml_range_cycles_freq_range_unlock() -> None:
    tab, app = make_tab("NVML", start=1, end=3)

    press_space(tab)
    assert app.native.calls == [
        ("set_locked_clocks", "GPU0", "nvml", "core", 1500, 1700)
    ]
    assert tab._freq_core_lock == (1500, 1700)
    assert tab._freq_core_lock_backend == "nvml"

    press_space(tab)
    assert app.native.calls[-1] == ("reset_core_clocks", "GPU0", "nvml")
    assert tab._freq_core_lock is None


def test_space_reset_uses_lock_backend_after_selector_change() -> None:
    tab, app = make_tab("NVML")

    press_space(tab)
    tab.freq_lock_api_var.set("NVAPI")
    press_space(tab)

    assert app.native.calls[-1] == ("reset_core_clocks", "GPU0", "nvml")
    assert not any(
        call == ("reset_vfp_frequency_lock", "GPU0", "core")
        for call in app.native.calls
    )


def test_load_csv_preserves_selection_for_space_round_robin_refresh(tmp_path) -> None:
    tab, _app = make_tab("NVAPI", start=1, end=1)
    csv_path = tmp_path / "curve.csv"
    csv_path.write_text(
        "\n".join([
            "voltage,frequency,delta,default_frequency",
            "800000,1400000,0,1400000",
            "900000,1500000,0,1500000",
            "1000000,1600000,0,1600000",
        ]),
        encoding="utf-8",
    )

    tab._load_csv(str(csv_path))

    assert tab._sel_start == 1
    assert tab._sel_end == 1
    assert tab.adj_start_var.get() == "1"
    assert tab.adj_end_var.get() == "1"
