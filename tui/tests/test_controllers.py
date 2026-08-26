from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest
from nvoc_tui.app import NVOCApp, _is_offline_error
from nvoc_tui.controllers.console import ConsoleController
from nvoc_tui.controllers.dashboard import DashboardController
from nvoc_tui.controllers.header import HeaderController
from nvoc_tui.controllers.overclock import OverclockController
from nvoc_tui.controllers.vfcurve import VFCurveController
from nvoc_tui.models import AppConfig, GpuCache, GpuDescriptor


class FakeApp:
    def __init__(self) -> None:
        self.config_data = AppConfig()
        self.cache = GpuCache()
        self.root_dir = Path.cwd()
        self.widgets: dict[str, object] = {}
        self.actions: list[str] = []
        self.action_outputs: list[str | None] = []
        self.query_calls: list[tuple] = []
        self.logs: list[str] = []
        self.native = FakeNative()
        self.native_service = SimpleNamespace(
            action_state=SimpleNamespace(running=False)
        )
        self.classes: set[str] = set()
        self.gpus = []
        self.refreshes = 0
        self.reprobe_starts = 0
        self.reprobe_stops = 0
        self.timers: list[FakeTimer] = []
        self.dashboard_focuses = 0
        self.full_refreshes = 0

    def query_one(self, selector: str, _widget_type=None):
        return self.widgets[selector]

    def has_class(self, class_name: str) -> bool:
        return class_name in self.classes

    def set_class(self, condition: bool, class_name: str) -> None:
        if condition:
            self.classes.add(class_name)
        else:
            self.classes.discard(class_name)

    def gpu_args(self) -> list[str]:
        return ["--gpu=0"]

    def save_config(self) -> None:
        pass

    def selected_gpu_target(self) -> str:
        return "0x0000"

    def selected_gpu_idx(self) -> int:
        return 0

    def current_gpu(self):
        return SimpleNamespace(uuid=None)

    def call_from_thread(self, callback, *args) -> None:
        callback(*args)

    def run_native_action(self, description: str, action) -> None:
        self.actions.append(description)
        output = action(self.native)
        self.action_outputs.append(output)
        if output:
            self.write_log(output)

    def run_query(
        self, command_name: str, callback, *, log_output: bool = True
    ) -> None:
        self.query_calls.append((command_name, callback, log_output))

    def write_log(self, text: str) -> None:
        self.logs.append(text)

    def set_interval(self, interval: float, callback, *, pause: bool = False):
        timer = FakeTimer(interval, callback, pause)
        self.timers.append(timer)
        return timer

    def refresh_gpu_list(self) -> None:
        self.refreshes += 1

    def start_gpu_reprobe(self) -> None:
        self.reprobe_starts += 1

    def _stop_gpu_reprobe(self) -> None:
        self.reprobe_stops += 1

    def focus_dashboard_tab_switcher(self) -> None:
        self.dashboard_focuses += 1

    def refresh_all_state(self) -> None:
        self.full_refreshes += 1


class FakeTimer:
    def __init__(self, interval: float, callback, pause: bool = False) -> None:
        self.interval = interval
        self.callback = callback
        self.paused = pause
        self.stopped = False

    def stop(self) -> None:
        self.stopped = True

    def pause(self) -> None:
        self.paused = True

    def resume(self) -> None:
        self.paused = False


class FakePanel:
    def __init__(self, classes: set[str] | None = None) -> None:
        self.classes = classes or set()

    def has_class(self, class_name: str) -> bool:
        return class_name in self.classes

    def add_class(self, class_name: str) -> None:
        self.classes.add(class_name)

    def remove_class(self, class_name: str) -> None:
        self.classes.discard(class_name)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []
        self.raise_on_set_clock: Exception | None = None

    def query_domain_vfp_points(self, gpu, domain, infer_missing_default):
        self.calls.append((
            "query_domain_vfp_points",
            gpu,
            domain,
            infer_missing_default,
        ))
        return [
            {
                "index": 7,
                "voltage_uv": 800000,
                "frequency_khz": 1800000,
                "delta_khz": 15000,
                "default_frequency_khz": 1785000,
            }
        ]

    def set_power_limit(self, gpu, backend, value):
        self.calls.append(("set_power_limit", gpu, backend, value))

    def set_thermal_limit(self, gpu, value):
        self.calls.append(("set_thermal_limit", gpu, value))

    def set_voltage_boost(self, gpu, value):
        self.calls.append(("set_voltage_boost", gpu, value))

    def set_fan(self, gpu, backend, fan_id, policy, level):
        self.calls.append(("set_fan", gpu, backend, fan_id, policy, level))

    def set_clock_offset(self, gpu, backend, domain, offset, pstate):
        self.calls.append(("set_clock_offset", gpu, backend, domain, offset, pstate))
        if self.raise_on_set_clock is not None:
            raise self.raise_on_set_clock

    def set_nvml_pstate_lock(self, gpu, pstart, pend):
        self.calls.append(("set_nvml_pstate_lock", gpu, pstart, pend))

    def set_nvapi_pstate_lock(self, gpu, pstart, pend):
        self.calls.append(("set_nvapi_pstate_lock", gpu, pstart, pend))

    def reset_locked_clocks(self, gpu, backend, domain):
        self.calls.append(("reset_locked_clocks", gpu, backend, domain))

    def reset_vfp_frequency_lock(self, gpu, domain):
        self.calls.append(("reset_vfp_frequency_lock", gpu, domain))

    def set_vfp_voltage_lock(self, gpu, point, voltage_uv, immediate):
        self.calls.append(("set_vfp_voltage_lock", gpu, point, voltage_uv, immediate))

    def reset_vfp_deltas(self, gpu, domain):
        self.calls.append(("reset_vfp_deltas", gpu, domain))

    def reset_vfp_lock(self, gpu):
        self.calls.append(("reset_vfp_lock", gpu))

    def set_vfp_range_delta(self, gpu, start, end, delta):
        self.calls.append(("set_vfp_range_delta", gpu, start, end, delta))


def test_dashboard_tick_suppresses_status_json_output() -> None:
    app = FakeApp()

    DashboardController(app).tick()

    assert len(app.query_calls) == 1
    command_name, callback, log_output = app.query_calls[0]
    assert command_name == "status"
    assert callback.__name__ == "on_status_loaded"
    assert log_output is False


def test_offline_error_classification_excludes_unsupported_features() -> None:
    assert _is_offline_error("NvAPI: API_NOT_INITIALIZED")
    assert DashboardController._looks_like_offline_error("GPU is lost")
    assert not _is_offline_error("NVAPI_NOT_SUPPORTED")
    assert not DashboardController._looks_like_offline_error("function not supported")


def test_dashboard_enters_backoff_after_three_offline_failures() -> None:
    app = FakeApp()
    controller = DashboardController(app)
    controller.set_poll_timer(1.0)

    for _ in range(3):
        controller.on_status_loaded(-1, "API_NOT_INITIALIZED", {})

    assert controller._in_offline_backoff is True
    assert controller._effective_interval() == 5.0
    assert app.refreshes == 1
    assert app.logs == [
        "dGPU probably offline — polling paused, re-probing for the GPU to come back."
    ]
    assert app.timers[-1].interval == 5.0


def test_dashboard_backoff_tick_reprobes_instead_of_querying() -> None:
    app = FakeApp()
    controller = DashboardController(app)
    controller._in_offline_backoff = True

    controller.tick()

    assert app.refreshes == 1
    assert app.query_calls == []


def test_dashboard_success_restores_user_poll_interval() -> None:
    app = FakeApp()
    app.widgets["#metrics"] = SimpleNamespace(update=lambda _value: None)
    controller = DashboardController(app)
    controller._user_interval = 2.5
    controller._in_offline_backoff = True
    controller._consecutive_offline = 4
    controller._offline_hint_logged = True

    controller.on_status_loaded(0, "", {})

    assert controller._in_offline_backoff is False
    assert controller._consecutive_offline == 0
    assert app.logs == ["dGPU back online — resuming polling."]
    assert app.timers[-1].interval == 2.5


def test_header_reprobes_empty_gpu_list_and_stops_after_gpu_returns() -> None:
    app = FakeApp()
    select = SimpleNamespace(options=[], value=None)
    select.set_options = lambda options: setattr(select, "options", options)
    app.widgets["#gpu-select"] = select
    controller = HeaderController(app)

    controller.on_gpu_list_loaded(-1, "API_NOT_INITIALIZED", [])

    assert app.logs == []
    assert app.reprobe_starts == 1
    assert select.value == "-1"

    controller.on_gpu_list_loaded(
        0, "", [GpuDescriptor(index=2, name="RTX", gpu_id_hex="0x2")]
    )

    assert app.reprobe_stops == 1
    assert select.options == [("GPU 2: RTX [0x2]", "2")]
    assert select.value == "2"
    assert app.dashboard_focuses == 1
    assert app.full_refreshes == 1


def test_console_maximize_toggle_updates_app_class_and_label() -> None:
    app = FakeApp()
    log = SimpleNamespace(focused=False)
    log.focus = lambda: setattr(log, "focused", True)
    app.widgets = {
        "#log-panel": FakePanel(),
        "#toggle-log": SimpleNamespace(label="Hide (^t)"),
        "#maximize-log": SimpleNamespace(label="Max (^x)"),
        "#output-log": log,
    }

    controller = ConsoleController(app)

    controller.toggle_output_maximized()

    assert app.has_class("output-maximized") is True
    assert app.widgets["#maximize-log"].label == "Restore (^x)"
    assert log.focused is True

    controller.toggle_output_maximized()

    assert app.has_class("output-maximized") is False
    assert app.widgets["#maximize-log"].label == "Max (^x)"


def test_console_maximize_from_hidden_shows_and_persists_output() -> None:
    app = FakeApp()
    panel = FakePanel(classes={"hidden"})
    app.widgets = {
        "#log-panel": panel,
        "#toggle-log": SimpleNamespace(label="Show (^t)"),
        "#maximize-log": SimpleNamespace(label="Max (^x)"),
        "#output-log": SimpleNamespace(focus=lambda: None),
    }

    ConsoleController(app).toggle_output_maximized()

    assert panel.has_class("hidden") is False
    assert app.widgets["#toggle-log"].label == "Hide (^t)"
    assert app.config_data.ui.log_expanded is True
    assert app.has_class("output-maximized") is True


def test_console_hide_from_maximized_clears_maximized_state() -> None:
    app = FakeApp()
    app.classes.add("output-maximized")
    panel = FakePanel()
    app.widgets = {
        "#log-panel": panel,
        "#toggle-log": SimpleNamespace(label="Hide (^t)"),
        "#maximize-log": SimpleNamespace(label="Restore (^x)"),
    }

    ConsoleController(app).toggle_output()

    assert panel.has_class("hidden") is True
    assert app.has_class("output-maximized") is False
    assert app.widgets["#maximize-log"].label == "Max (^x)"
    assert app.config_data.ui.log_expanded is False


def test_app_binds_ctrl_x_to_output_maximize_toggle() -> None:
    assert any(
        binding.key == "ctrl+x" and binding.action == "toggle_output_maximized"
        for binding in NVOCApp.BINDINGS
        if hasattr(binding, "key")
    )


def test_overclock_apply_limits_for_nvapi_calls_native_apis() -> None:
    app = FakeApp()
    app.widgets = {
        "#power-api": SimpleNamespace(value="nvapi"),
        "#power-limit": SimpleNamespace(value="110"),
        "#thermal-limit": SimpleNamespace(value="88"),
        "#voltage-boost": SimpleNamespace(value="25"),
    }

    assert OverclockController(app).handle_button("limits-apply") is True

    assert app.actions == ["apply limits"]
    assert app.action_outputs == ["Successfully applied nvapi limits."]
    assert app.logs == ["Successfully applied nvapi limits."]
    assert app.native.calls == [
        ("set_power_limit", "0x0000", "nvapi", 110),
        ("set_thermal_limit", "0x0000", 88),
        ("set_voltage_boost", "0x0000", 25),
    ]


def test_overclock_apply_ignores_pstate_fields() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#core-offset": SimpleNamespace(value="100"),
        "#mem-offset": SimpleNamespace(value="200"),
        "#pstate-start": SimpleNamespace(value="P5"),
        "#pstate-end": SimpleNamespace(value="P2"),
    }

    assert OverclockController(app).handle_button("oc-apply") is True

    assert app.actions == ["apply overclock"]
    assert app.action_outputs == ["Successfully applied nvapi overclock."]
    assert app.native.calls == [
        ("set_clock_offset", "0x0000", "nvapi", "core", 100, "P0"),
        ("set_clock_offset", "0x0000", "nvapi", "memory", 200, "P0"),
    ]


def test_overclock_pstate_limits_rejects_unknown_start_with_available_list() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#pstate-start": SimpleNamespace(value="P5"),
        "#pstate-end": SimpleNamespace(value="P2"),
    }

    assert OverclockController(app).handle_button("pstate-limits-apply") is True

    assert app.actions == []
    assert app.native.calls == []
    assert app.logs == ["Unknown pstate P5. Available pstates: P0, P2."]


def test_overclock_pstate_limits_calls_nvapi() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#pstate-start": SimpleNamespace(value="P0"),
        "#pstate-end": SimpleNamespace(value="P2"),
    }

    assert OverclockController(app).handle_button("pstate-limits-apply") is True

    assert app.actions == ["apply PState limits"]
    assert app.action_outputs == ["Successfully applied nvapi PState limits P0-P2."]
    assert app.native.calls == [("set_nvapi_pstate_lock", "0x0000", "P0", "P2")]


def test_overclock_pstate_limits_defaults_blank_end_to_start_and_calls_nvml() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvml"),
        "#pstate-start": SimpleNamespace(value="P2"),
        "#pstate-end": SimpleNamespace(value=""),
    }

    assert OverclockController(app).handle_button("pstate-limits-apply") is True

    assert app.actions == ["apply PState limits"]
    assert app.action_outputs == ["Successfully applied nvml PState limits P2-P2."]
    assert app.native.calls == [("set_nvml_pstate_lock", "0x0000", "P2", "P2")]


def test_overclock_pstate_limits_enriches_native_unknown_pstate() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.native.raise_on_set_clock = RuntimeError("unknown pstate")
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#pstate-start": SimpleNamespace(value="P0"),
        "#pstate-end": SimpleNamespace(value=""),
    }

    original = app.native.set_nvapi_pstate_lock

    def raise_unknown(*args):
        original(*args)
        raise app.native.raise_on_set_clock

    app.native.set_nvapi_pstate_lock = raise_unknown

    try:
        OverclockController(app).handle_button("pstate-limits-apply")
    except RuntimeError as exc:
        assert str(exc) == "unknown pstate. Available pstates: P0, P2."
    else:
        raise AssertionError("expected RuntimeError")


def test_overclock_pstate_reset_uses_nvapi_memory_vfp_lock() -> None:
    app = FakeApp()
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
    }

    assert OverclockController(app).handle_button("pstate-limits-reset") is True

    assert app.actions == ["reset PState limits"]
    assert app.action_outputs == ["Successfully reset nvapi PState limits."]
    assert app.native.calls == [("reset_vfp_frequency_lock", "0x0000", "memory")]


def test_overclock_pstate_reset_uses_nvml_memory_locked_clocks() -> None:
    app = FakeApp()
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvml"),
    }

    assert OverclockController(app).handle_button("pstate-limits-reset") is True

    assert app.actions == ["reset PState limits"]
    assert app.action_outputs == ["Successfully reset nvml PState limits."]
    assert app.native.calls == [("reset_locked_clocks", "0x0000", "nvml", "memory")]


def test_overclock_fan_reset_preserves_target() -> None:
    app = FakeApp()
    app.widgets = {
        "#fan-api": SimpleNamespace(value="nvml"),
        "#fan-id": SimpleNamespace(value="2"),
    }

    assert OverclockController(app).handle_button("fan-reset") is True

    assert app.actions == ["reset fan"]
    assert app.action_outputs == ["Successfully reset fan control."]
    assert app.logs == ["Successfully reset fan control."]
    assert app.native.calls == [("set_fan", "0x0000", "nvml-cooler", "2", "auto", 0)]


def test_overclock_shortcut_focuses_target_widget() -> None:
    app = FakeApp()
    target = SimpleNamespace(focused=False)
    target.focus = lambda: setattr(target, "focused", True)
    app.widgets = {"#power-api": target}

    assert OverclockController(app).activate_shortcut("power-api") is True

    assert target.focused is True


def test_vfcurve_export_action_writes_static_curve(tmp_path: Path) -> None:
    app = FakeApp()
    curve_path = tmp_path / "curve.csv"
    app.widgets = {
        "#vf-path": SimpleNamespace(value=str(curve_path)),
    }

    assert VFCurveController(app).handle_button("vf-export") is True

    assert app.config_data.vfcurve.default_path == str(curve_path)
    assert app.actions == ["export VFP curve"]
    assert curve_path.read_text(encoding="utf-8").splitlines() == [
        "voltage,frequency,delta,default_frequency",
        "800000,1800000,15000,1785000",
    ]


def test_vfcurve_refresh_suppresses_overlapping_workers(tmp_path: Path) -> None:
    app = FakeApp()
    app.root_dir = tmp_path
    scheduled: list[object] = []

    app.native_service.submit_query = scheduled.append
    controller = VFCurveController(app)

    controller.refresh_curve()
    controller.refresh_curve()

    assert len(scheduled) == 1
    assert controller.is_refresh_inflight() is True


def test_vfcurve_refresh_keeps_points_in_memory(tmp_path: Path) -> None:
    app = FakeApp()
    app.root_dir = tmp_path
    points = [
        {
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "default_frequency_khz": 1750000,
        }
    ]
    app.native_service.query_domain_vfp_points = lambda _gpu: points

    app.native_service.submit_query = lambda job: job()
    controller = VFCurveController(app)
    rendered: list[bool] = []
    controller.render_plot = lambda: rendered.append(True)

    controller.refresh_curve()

    assert app.cache.vf_curve_points == points
    assert rendered == [True]
    assert not (tmp_path / "vfp_cache").exists()
    assert controller.is_refresh_inflight() is False


def test_vfcurve_refresh_clears_inflight_when_thread_start_fails(
    tmp_path: Path,
) -> None:
    app = FakeApp()
    app.root_dir = tmp_path

    def fail_submit(_job) -> None:
        raise RuntimeError("query queue unavailable")

    app.native_service.submit_query = fail_submit
    controller = VFCurveController(app)

    with pytest.raises(RuntimeError):
        controller.refresh_curve()

    assert controller.is_refresh_inflight() is False


def test_vfcurve_lock_voltage_rejects_invalid_point() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-lock-value": SimpleNamespace(value=""),
        "#vf-lock-as-mv": SimpleNamespace(value=False),
    }

    assert VFCurveController(app).handle_button("vf-lock-voltage") is True

    assert app.actions == []
    assert app.native.calls == []
    assert app.logs == ["Invalid VFP lock point: enter a numeric point index."]


def test_vfcurve_lock_voltage_rejects_invalid_mv() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-lock-value": SimpleNamespace(value="not a number"),
        "#vf-lock-as-mv": SimpleNamespace(value=True),
    }

    assert VFCurveController(app).handle_button("vf-lock-voltage") is True

    assert app.actions == []
    assert app.native.calls == []
    assert app.logs == ["Invalid VFP lock voltage: enter a numeric mV value."]


def test_vfcurve_lock_voltage_accepts_mv_value() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-lock-value": SimpleNamespace(value="875.5"),
        "#vf-lock-as-mv": SimpleNamespace(value=True),
    }

    assert VFCurveController(app).handle_button("vf-lock-voltage") is True

    assert app.actions == ["lock VFP voltage"]
    assert app.action_outputs == ["Successfully locked VFP voltage to 875.5 mV."]
    assert app.logs == ["Successfully locked VFP voltage to 875.5 mV."]
    assert app.native.calls == [("set_vfp_voltage_lock", "0x0000", None, 875500, False)]


def test_vfcurve_reset_vfp_reports_success() -> None:
    app = FakeApp()

    assert VFCurveController(app).handle_button("vf-reset") is True

    assert app.actions == ["reset VFP deltas"]
    assert app.action_outputs == ["Successfully reset VFP deltas."]
    assert app.logs == ["Successfully reset VFP deltas."]
    assert app.native.calls == [("reset_vfp_deltas", "0x0000", "all")]


def test_vfcurve_apply_adjustment_reports_success() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-range-start": SimpleNamespace(value="10"),
        "#vf-range-end": SimpleNamespace(value="5"),
        "#vf-delta": SimpleNamespace(value="125"),
    }

    assert VFCurveController(app).handle_button("vf-apply-adj") is True

    assert app.actions == ["apply VFP range delta"]
    assert app.action_outputs == [
        "Successfully applied 125 MHz VFP delta to points 5-10."
    ]
    assert app.logs == ["Successfully applied 125 MHz VFP delta to points 5-10."]
    assert app.native.calls == [("set_vfp_range_delta", "0x0000", 5, 10, 125000)]
