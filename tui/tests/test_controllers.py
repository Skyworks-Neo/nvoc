from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

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
        self.logs: list[str] = []
        self.native = FakeNative()

    def query_one(self, selector: str, _widget_type=None):
        return self.widgets[selector]

    def gpu_args(self) -> list[str]:
        return ["--gpu=0"]

    def save_config(self) -> None:
        pass

    def selected_gpu_target(self) -> str:
        return "0x0000"

    def run_native_action(self, description: str, action) -> None:
        self.actions.append(description)
        self.action_outputs.append(action(self.native))

    def write_log(self, text: str) -> None:
        self.logs.append(text)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

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

    def set_vfp_voltage_lock(self, gpu, point, voltage_uv, immediate):
        self.calls.append(("set_vfp_voltage_lock", gpu, point, voltage_uv, immediate))


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
    assert app.native.calls == [
        ("set_power_limit", "0x0000", "nvapi", 110),
        ("set_thermal_limit", "0x0000", 88),
        ("set_voltage_boost", "0x0000", 25),
    ]


def test_overclock_fan_reset_preserves_target() -> None:
    app = FakeApp()
    app.widgets = {
        "#fan-api": SimpleNamespace(value="nvml"),
        "#fan-id": SimpleNamespace(value="2"),
    }

    assert OverclockController(app).handle_button("fan-reset") is True

    assert app.actions == ["reset fan"]
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
    assert app.native.calls == [
        ("set_vfp_voltage_lock", "0x0000", None, 875500, False)
    ]
