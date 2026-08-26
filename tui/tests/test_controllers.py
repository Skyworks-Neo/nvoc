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
from nvoc_tui.models import AppConfig, CurveData, GpuCache, GpuDescriptor


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

    def call_after_refresh(self, callback, *args) -> None:
        callback(*args)

    def run_native_action(self, description: str, action) -> None:
        self.actions.append(description)
        output = action(self.native)
        self.action_outputs.append(output)
        if output:
            self.write_log(output)

    def run_action_chain(self, commands) -> None:
        for description, action in commands:
            self.actions.append(description)
            output = action(self.native)
            self.action_outputs.append(output)
            if output:
                self.write_log(output)

    def run_query(
        self,
        command_name: str,
        callback,
        *,
        log_output: bool = True,
        allow_wake: bool = True,
    ) -> None:
        self.query_calls.append((
            command_name,
            callback,
            {"log_output": log_output, "allow_wake": allow_wake},
        ))

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
        self.raise_on_set_vfp_point_private: Exception | None = None
        self.direct_freq_khz: int = 0

    def query_public_vftable(self, gpu, domain, infer_missing_default):
        self.calls.append((
            "query_public_vftable",
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

    def query_private_vftable(self, gpu):
        self.calls.append(("query_private_vftable", gpu))
        return None

    def query_private_freq_domain_status(self, gpu, domain_bit):
        self.calls.append(("query_private_freq_domain_status", gpu, domain_bit))
        return {"domain_bit": domain_bit, "freq_khz": self.direct_freq_khz}

    def set_vfp_point_private(self, gpu, bank, index, delta_khz, freq_mode):
        self.calls.append((
            "set_vfp_point_private",
            gpu,
            bank,
            index,
            delta_khz,
            freq_mode,
        ))
        if self.raise_on_set_vfp_point_private is not None:
            raise self.raise_on_set_vfp_point_private
        return {"applied": True}

    def set_vfp_range_per_point_private(self, gpu, bank, start, end, deltas):
        self.calls.append((
            "set_vfp_range_per_point_private",
            gpu,
            bank,
            start,
            end,
            list(deltas),
        ))
        return {"applied": True}

    def clk_vf_delta_for_target_mhz(self, def_mhz, delta_mhz, class_name):
        # Mirrors the call semantics used by the GUI/TUI raw-converted path:
        # the 2nd argument is the desired MHz offset (not an absolute
        # target); the raw f-offset scales it 10×.
        self.calls.append((
            "clk_vf_delta_for_target_mhz",
            def_mhz,
            delta_mhz,
            class_name,
        ))
        return {"delta": int(delta_mhz * 10)}

    def set_power_limit(self, gpu, backend, value):
        self.calls.append(("set_power_limit", gpu, backend, value))

    def set_thermal_limit(self, gpu, value):
        self.calls.append(("set_thermal_limit", gpu, value))

    def set_voltage_boost(self, gpu, value):
        self.calls.append(("set_voltage_boost", gpu, value))

    def set_legacy_voltage_delta(self, gpu, delta_uv, pstate):
        self.calls.append(("set_legacy_voltage_delta", gpu, delta_uv, pstate))

    def set_clk_domain_offset(self, gpu, domain_bit, offset_khz, flags, unknown):
        self.calls.append((
            "set_clk_domain_offset",
            gpu,
            domain_bit,
            offset_khz,
            flags,
            unknown,
        ))
        return {"applied": True, "applied_kHz": offset_khz}

    def set_volt_rail_target(self, gpu, rail_bit, target_mv, unknown):
        self.calls.append(("set_volt_rail_target", gpu, rail_bit, target_mv, unknown))
        return {"applied": True, "effective_wall_uV": int(target_mv * 1000)}

    def set_ppab_status(self, gpu, enabled):
        self.calls.append(("set_ppab_status", gpu, enabled))

    def set_dnotifier(self, gpu, level):
        self.calls.append(("set_dnotifier", gpu, level))

    def set_tgp_watt(self, gpu, watts, policy_index):
        self.calls.append(("set_tgp_watt", gpu, watts, policy_index))

    def set_target_temp(self, gpu, celsius, policy_index):
        self.calls.append(("set_target_temp", gpu, celsius, policy_index))

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
    command_name, callback, kwargs = app.query_calls[0]
    assert command_name == "status"
    assert callback.__name__ == "on_status_loaded"
    assert kwargs["log_output"] is False
    assert kwargs["allow_wake"] is True  # first sample may wake the GPU


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
    app.native_service.query_public_vftable = lambda _gpu: points
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
    controller = VFCurveController(app)
    controller._curves = {
        "gpc": CurveData(
            "gpc",
            write_mode="public",
            seg_start=0,
            seg_end=1,
            frequencies=[1800.0, 1900.0],
            defaults=[1785.0, 1890.0],
        )
    }

    assert controller.handle_button("vf-reset") is True

    assert app.actions == ["reset VFP deltas"]
    assert app.action_outputs == [
        "Successfully reset GPC curve to default (0-1, public)."
    ]
    assert app.logs == ["Successfully reset GPC curve to default (0-1, public)."]
    assert app.native.calls == [("set_vfp_range_delta", "0x0000", 0, 1, 0)]


def test_vfcurve_reset_private_curve_uses_mode0_clear() -> None:
    app = FakeApp()
    controller = VFCurveController(app)
    controller._curves = {
        "xbar": CurveData(
            "xbar",
            source="private",
            write_mode="private",
            bank=1,
            seg_start=2,
            seg_end=4,
            frequencies=[1000.0, 1100.0, 1200.0],
            defaults=[1000.0, 1100.0, 1200.0],
        )
    }
    controller._active_curve = "xbar"

    assert controller.handle_button("vf-reset") is True

    assert app.actions == ["reset VFP deltas"]
    assert app.native.calls == [
        ("set_vfp_point_private", "0x0000", 1, 2, 0, True),
        ("set_vfp_point_private", "0x0000", 1, 3, 0, True),
        ("set_vfp_point_private", "0x0000", 1, 4, 0, True),
    ]
    assert "Successfully reset XBAR (private mode-0, 2-4)." in app.action_outputs


def test_vfcurve_apply_adjustment_reports_success() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-range-start": SimpleNamespace(value="10"),
        "#vf-range-end": SimpleNamespace(value="5"),
        "#vf-delta": SimpleNamespace(value="125"),
    }
    controller = VFCurveController(app)
    controller._curves = {
        "gpc": CurveData(
            "gpc",
            write_mode="public",
            frequencies=[1800.0] * 11,
            defaults=[1785.0] * 11,
        )
    }

    assert controller.handle_button("vf-apply-adj") is True

    assert app.actions == ["apply VFP range delta"]
    assert app.action_outputs == [
        "Successfully applied 125 MHz VFP delta to points 5-10."
    ]
    assert app.logs == ["Successfully applied 125 MHz VFP delta to points 5-10."]
    assert app.native.calls == [("set_vfp_range_delta", "0x0000", 5, 10, 125000)]


def test_vfcurve_apply_private_mode0_then_raw_fallback() -> None:
    app = FakeApp()
    app.native.raise_on_set_vfp_point_private = RuntimeError("Argument range")
    app.widgets = {
        "#vf-range-start": SimpleNamespace(value="1"),
        "#vf-range-end": SimpleNamespace(value="2"),
        "#vf-delta": SimpleNamespace(value="100"),
    }
    controller = VFCurveController(app)
    controller._curves = {
        "xbar": CurveData(
            "xbar",
            source="private",
            write_mode="private",
            bank=1,
            seg_start=5,
            frequencies=[1000.0, 1100.0, 1200.0, 1300.0],
            defaults=[1000.0, 1100.0, 1200.0, 1300.0],
        )
    }
    controller._active_curve = "xbar"

    assert controller.handle_button("vf-apply-adj") is True

    # mode-0 attempted on the first point, rejected, then the whole range is
    # re-applied via the raw-converted path.
    assert app.native.calls[0] == (
        "set_vfp_point_private",
        "0x0000",
        1,
        6,
        100000,
        True,
    )
    assert ("clk_vf_delta_for_target_mhz", 1100, 100.0, "fabric") in app.native.calls
    assert ("clk_vf_delta_for_target_mhz", 1200, 100.0, "fabric") in app.native.calls
    range_call = app.native.calls[-1]
    assert range_call[0] == "set_vfp_range_per_point_private"
    assert range_call[1:5] == ("0x0000", 1, 6, 7)
    assert range_call[5] == [1000, 1000]  # (100 MHz target) * 10 per FakeNative
    assert (
        "Successfully applied private raw-converted offsets to XBAR (2 pts)."
        in app.action_outputs
    )


class _FakePlt:
    def clear_figure(self) -> None:
        pass

    def clear_data(self) -> None:
        pass

    def clear_color(self) -> None:
        pass

    def plot(self, *args, **kwargs) -> None:
        pass

    def scatter(self, *args, **kwargs) -> None:
        pass

    def vline(self, *args, **kwargs) -> None:
        pass

    def hline(self, *args, **kwargs) -> None:
        pass

    def text(self, *args, **kwargs) -> None:
        pass

    def xlim(self, *args, **kwargs) -> None:
        pass

    def ylim(self, *args, **kwargs) -> None:
        pass

    def title(self, *args, **kwargs) -> None:
        pass

    def xlabel(self, *args, **kwargs) -> None:
        pass

    def ylabel(self, *args, **kwargs) -> None:
        pass


def _plot_widget() -> SimpleNamespace:
    return SimpleNamespace(plt=_FakePlt(), refresh=lambda: None)


def _selector_widgets() -> dict[str, SimpleNamespace]:
    return {
        "#vf-plot": _plot_widget(),
        "#vf-active-curve": SimpleNamespace(
            value="gpc", set_options=lambda options: None, disabled=False
        ),
        "#vf-curve-gpc": SimpleNamespace(value=True, disabled=False),
        "#vf-curve-xbar": SimpleNamespace(value=True, disabled=False),
        "#vf-curve-host": SimpleNamespace(value=True, disabled=False),
    }


def test_vfcurve_toggle_visibility_guards_last_visible_curve() -> None:
    app = FakeApp()
    app.widgets = _selector_widgets()
    controller = VFCurveController(app)
    controller._curves = {
        "gpc": CurveData("gpc", frequencies=[1800.0], defaults=[1785.0]),
        "xbar": CurveData("xbar", frequencies=[1200.0], defaults=[1200.0]),
    }
    controller._curve_visible = {"gpc": True, "xbar": True}
    gpc_checkbox = app.widgets["#vf-curve-gpc"]
    # Active falls back to xbar on hide → direct-read kick needs a queue.
    app.native_service.submit_query = lambda job: None

    # Hide GPC (active): allowed, active falls back to the other visible curve.
    controller._toggle_curve_visible("gpc", gpc_checkbox)
    assert controller._curve_visible == {"gpc": False, "xbar": True}
    assert controller._active_curve == "xbar"

    # Hide XBAR too (now the only visible curve): vetoed, checkbox snaps back.
    xbar_checkbox = app.widgets["#vf-curve-xbar"]
    controller._toggle_curve_visible("xbar", xbar_checkbox)
    assert controller._curve_visible == {"gpc": False, "xbar": True}
    assert xbar_checkbox.value is True


def test_vfcurve_on_curve_loaded_builds_multi_curves() -> None:
    app = FakeApp()
    app.widgets = _selector_widgets()
    controller = VFCurveController(app)
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "default_frequency_khz": 1785000,
        }
    ]
    clk_data = {
        "segments": [
            {
                "kind": "vf_curve",
                "domain": "xbar",
                "bank": 1,
                "start_index": 2,
                "end_index": 3,
            },
            {
                "kind": "pstate_bins",
                "domain": "unknown",
                "bank": 9,
                "start_index": 0,
                "end_index": 5,
            },
        ],
        "points": [
            {
                "bank": 1,
                "index": 2,
                "voltage_uV": 700000,
                "freq_current_mhz": 1200.0,
                "freq_default_mhz": 1200.0,
            },
            {
                "bank": 1,
                "index": 3,
                "voltage_uV": 750000,
                "freq_current_mhz": 1300.0,
                "freq_default_mhz": 1300.0,
            },
        ],
    }

    controller.on_curve_loaded(gpc_points, None, clk_data)

    assert set(controller._curves) == {"gpc", "xbar"}
    assert controller._curves["gpc"].write_mode == "public"
    assert controller._curves["xbar"].bank == 1
    assert controller._curves["xbar"].seg_start == 2
    assert controller._curves["xbar"].seg_end == 3
    assert app.cache.vf_curves is controller._curves
    assert app.cache.curve_visible == {"gpc": True, "xbar": True}


def test_vfcurve_direct_read_updates_live_point() -> None:
    app = FakeApp()
    app.widgets = _selector_widgets()
    controller = VFCurveController(app)
    controller._curves = {
        "xbar": CurveData(
            "xbar",
            voltages=[700.0, 750.0, 800.0],
            frequencies=[1200.0, 1300.0, 1400.0],
            defaults=[1200.0, 1300.0, 1400.0],
        )
    }
    controller._curve_visible = {"xbar": True}
    controller._active_curve = "xbar"
    scheduled: list[object] = []
    app.native_service.submit_query = lambda job: scheduled.append(job)
    app.native_service.query_private_freq_domain_status = (
        app.native.query_private_freq_domain_status
    )
    app.native.direct_freq_khz = 1350000

    controller._kick_direct_read("xbar")
    assert controller._direct_read_inflight is True
    assert len(scheduled) == 1

    # 1350 MHz → halfway between the 1300/1400 points → 775 mV.
    scheduled[0]()
    assert controller._direct_read_inflight is False
    assert app.cache.vf_live_point == (775.0, 1350.0)


def _oc_app(**info: object) -> FakeApp:
    app = FakeApp()
    app.cache.info = dict(info)
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#power-api": SimpleNamespace(value="nvapi"),
        "#core-offset": SimpleNamespace(value="100"),
        "#mem-offset": SimpleNamespace(value="200"),
        "#xbar-offset": SimpleNamespace(value="60"),
        "#power-limit": SimpleNamespace(value="110"),
        "#thermal-limit": SimpleNamespace(value="88"),
        "#voltage-boost": SimpleNamespace(value="25"),
        "#mobile-ppab": SimpleNamespace(value="on"),
        "#mobile-dnotifier": SimpleNamespace(value=3),
        "#mobile-tgp": SimpleNamespace(value="100"),
        "#mobile-target-temp": SimpleNamespace(value="85"),
        "#mobile-volt-limit": SimpleNamespace(value="1050"),
    }
    return app


def test_overclock_apply_oc_includes_xbar_when_supported() -> None:
    app = _oc_app(xbar_supported=True)

    assert OverclockController(app).handle_button("oc-apply") is True

    assert app.actions == ["apply overclock"]
    calls = app.native.calls
    assert calls == [
        ("set_clock_offset", "0x0000", "nvapi", "core", 100, "P0"),
        ("set_clock_offset", "0x0000", "nvapi", "memory", 200, "P0"),
        ("set_clk_domain_offset", "0x0000", 1, 60000, None, None),
    ]
    assert "Successfully applied nvapi overclock." in app.action_outputs[0]
    assert "Successfully applied Xbar offset +60 MHz" in app.action_outputs[0]


def test_overclock_apply_oc_skips_xbar_when_unsupported() -> None:
    app = _oc_app(xbar_supported=False)

    OverclockController(app).handle_button("oc-apply")

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_overclock_apply_oc_skips_xbar_under_nvml_backend() -> None:
    app = _oc_app(xbar_supported=True)
    app.widgets["#oc-api"] = SimpleNamespace(value="nvml")

    OverclockController(app).handle_button("oc-apply")

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_overclock_xbar_supported_arch_heuristic() -> None:
    app = _oc_app(codename="AD107-B", gpu_architecture="Unknown:400:7:161")
    controller = OverclockController(app)
    assert controller.xbar_supported() is True

    app2 = _oc_app(gpu_name="NVIDIA GeForce GTX 1080")
    assert OverclockController(app2).xbar_supported() is False

    app3 = _oc_app(gpu_name="NVIDIA GeForce RTX 4060 Laptop GPU")
    assert OverclockController(app3).xbar_supported() is True


def test_overclock_reset_oc_chain_resets_xbar_when_supported() -> None:
    app = _oc_app(xbar_supported=True)

    assert OverclockController(app).handle_button("oc-reset") is True

    assert app.actions == [
        "reset core offset",
        "reset memory offset",
        "reset xbar offset",
    ]
    assert ("set_clk_domain_offset", "0x0000", 1, 0, None, None) in app.native.calls
    assert "Successfully applied Xbar offset +0 MHz" in "\n".join(app.logs)


def test_overclock_reset_oc_chain_skips_xbar_when_unsupported() -> None:
    app = _oc_app(xbar_supported=False)

    OverclockController(app).handle_button("oc-reset")

    assert app.actions == ["reset core offset", "reset memory offset"]
    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_overclock_apply_limits_routes_legacy_overvolt() -> None:
    app = _oc_app(is_legacy_voltage=True)

    assert OverclockController(app).handle_button("limits-apply") is True

    assert app.native.calls == [
        ("set_power_limit", "0x0000", "nvapi", 110),
        ("set_thermal_limit", "0x0000", 88),
        ("set_legacy_voltage_delta", "0x0000", 25000, "P0"),
    ]


def test_overclock_is_legacy_voltage_heuristic() -> None:
    app = _oc_app(gpu_name="NVIDIA GeForce GTX 970")
    assert OverclockController(app).is_legacy_voltage() is True

    app2 = _oc_app(gpu_name="NVIDIA GeForce RTX 4060 Laptop GPU")
    assert OverclockController(app2).is_legacy_voltage() is False

    app3 = _oc_app(codename="GM204")
    assert OverclockController(app3).is_legacy_voltage() is True


def test_overclock_mobile_apply_includes_volt_limit() -> None:
    app = _oc_app(is_mobile=True)
    controller = OverclockController(app)
    controller._volt_limit_supported = True
    controller._volt_limit_range = (300.0, 1100.0)
    controller._volt_rail_bit = 0

    assert controller.handle_button("mobile-apply") is True

    assert ("set_volt_rail_target", "0x0000", 0, 1050.0, None) in app.native.calls
    assert "Successfully applied Volt Limit 1050 mV" in app.action_outputs[0]


def test_overclock_mobile_apply_clamps_volt_limit_to_walls() -> None:
    app = _oc_app(is_mobile=True)
    app.widgets["#mobile-volt-limit"] = SimpleNamespace(value="1500")
    controller = OverclockController(app)
    controller._volt_limit_supported = True
    controller._volt_limit_range = (300.0, 1100.0)

    controller.handle_button("mobile-apply")

    assert ("set_volt_rail_target", "0x0000", 0, 1100.0, None) in app.native.calls


def test_overclock_mobile_apply_skips_volt_limit_when_unavailable() -> None:
    app = _oc_app(is_mobile=True)
    controller = OverclockController(app)

    controller.handle_button("mobile-apply")

    assert not any(c[0] == "set_volt_rail_target" for c in app.native.calls)


def test_overclock_volt_limit_bounds_from_p0_walls() -> None:
    bounds = OverclockController._volt_limit_bounds_from_p0({
        "vbios_wall_uV": 1_085_000,
        "vrm_max_wall_uV": 1_100_000,
        "effective_wall_uV": 1_060_000,
    })
    # min(VBIOS, VRM) = 1085 mV snapped down to the 2.5 mV grid; position
    # 1060 mV is already on-grid.
    assert bounds == (300.0, 1085.0, 1060.0)

    # No walls reported → 1200 mV fallback; no effective wall → sit at max.
    assert OverclockController._volt_limit_bounds_from_p0({}) == (
        300.0,
        1200.0,
        1200.0,
    )


def test_overclock_resolve_volt_rail_bit() -> None:
    assert (
        OverclockController._resolve_volt_rail_bit({
            "rail_descriptors": [{"rail_bit": 2}],
        })
        == 2
    )
    assert OverclockController._resolve_volt_rail_bit({"rail_mask": "0x5"}) == 0
    assert OverclockController._resolve_volt_rail_bit({"rail_mask": "0x8"}) == 3
    assert OverclockController._resolve_volt_rail_bit({}) == 0


def test_overclock_format_volt_rail_result_messages() -> None:
    assert (
        OverclockController._format_volt_rail_result(
            1050.0, {"applied": True, "effective_wall_uV": 1_045_000}
        )
        == "Successfully applied Volt Limit 1050 mV (effective wall 1045 mV)."
    )
    assert (
        OverclockController._format_volt_rail_result(1050.0, {"supported": False})
        == "Volt-rail target not supported by this driver."
    )
    assert (
        OverclockController._format_volt_rail_result(1050.0, {"applied": True})
        == "Successfully applied Volt Limit 1050 mV."
    )
