from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

from nvoc_tui.app import NVOCApp
from nvoc_tui.controllers.console import ConsoleController
from nvoc_tui.controllers.dashboard import DashboardController
from nvoc_tui.controllers.overclock import OverclockController
from nvoc_tui.controllers.vfcurve import VFCurveController
from nvoc_tui.models import AppConfig, GpuCache


class FakeApp:
    def __init__(self) -> None:
        self.config_data = AppConfig()
        self.cache = GpuCache()
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
        self.calls.append(
            ("query_domain_vfp_points", gpu, domain, infer_missing_default)
        )
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


def test_overclock_apply_rejects_unknown_start_pstate_with_available_list() -> None:
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

    assert app.actions == []
    assert app.native.calls == []
    assert app.logs == ["Unknown pstate P5. Available pstates: P0, P2."]


def test_overclock_apply_enriches_native_unknown_pstate_with_available_list() -> None:
    app = FakeApp()
    app.cache.settings["supported_pstates"] = ["P0", "P2"]
    app.native.raise_on_set_clock = RuntimeError("unknown pstate")
    app.widgets = {
        "#oc-api": SimpleNamespace(value="nvapi"),
        "#core-offset": SimpleNamespace(value="100"),
        "#mem-offset": SimpleNamespace(value="200"),
        "#pstate-start": SimpleNamespace(value="P0"),
        "#pstate-end": SimpleNamespace(value=""),
    }

    try:
        OverclockController(app).handle_button("oc-apply")
    except RuntimeError as exc:
        assert str(exc) == "unknown pstate. Available pstates: P0, P2."
    else:
        raise AssertionError("expected RuntimeError")


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
