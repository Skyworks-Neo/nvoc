from __future__ import annotations

from types import SimpleNamespace

from nvoc_tui.controllers.autoscan import AutoscanController
from nvoc_tui.controllers.overclock import OverclockController
from nvoc_tui.controllers.vfcurve import VFCurveController
from nvoc_tui.models import AppConfig, GpuCache


class FakeApp:
    def __init__(self) -> None:
        self.config_data = AppConfig()
        self.cache = GpuCache()
        self.widgets: dict[str, object] = {}
        self.actions: list[list[str]] = []

    def query_one(self, selector: str, _widget_type=None):
        return self.widgets[selector]

    def gpu_args(self) -> list[str]:
        return ["--gpu=0"]

    def save_config(self) -> None:
        pass

    def run_action(self, args: list[str]) -> None:
        self.actions.append(args)


def test_autoscan_args_uses_ultrafast_mode() -> None:
    app = FakeApp()
    app.config_data.autoscan.mode = "ultrafast"
    app.config_data.autoscan.output_csv = "out.csv"
    app.config_data.autoscan.init_csv = "init.csv"
    app.config_data.autoscan.bsod_recovery = "aggressive"

    args = AutoscanController(app).autoscan_args()

    assert args[:5] == ["--gpu=0", "set", "vfp", "autoscan", "-u"]
    assert ["-o", "out.csv", "-i", "init.csv"] == args[5:9]
    assert args[-2:] == ["-b", "aggressive"]


def test_overclock_limit_args_for_nvapi_include_extra_limits() -> None:
    app = FakeApp()
    app.widgets = {
        "#power-api": SimpleNamespace(value="nvapi"),
        "#power-limit": SimpleNamespace(value="110"),
        "#thermal-limit": SimpleNamespace(value="88"),
        "#voltage-boost": SimpleNamespace(value="25"),
    }

    args = OverclockController(app).limit_args()

    assert args == [
        "--gpu=0",
        "set",
        "nvapi",
        "--power-limit",
        "110",
        "--thermal-limit",
        "88",
        "--voltage-boost",
        "25",
    ]


def test_overclock_fan_reset_args_preserve_target() -> None:
    app = FakeApp()
    app.widgets = {
        "#fan-api": SimpleNamespace(value="nvml"),
        "#fan-id": SimpleNamespace(value="2"),
    }

    args = OverclockController(app).fan_args(reset=True)

    assert args == [
        "--gpu=0",
        "set",
        "nvml-cooler",
        "--id",
        "2",
        "--policy",
        "auto",
        "--level",
        "0",
    ]


def test_vfcurve_export_action_appends_quick_flag() -> None:
    app = FakeApp()
    app.widgets = {
        "#vf-path": SimpleNamespace(value="curve.csv"),
        "#vf-quick-export": SimpleNamespace(value=True),
    }

    assert VFCurveController(app).handle_button("vf-export") is True

    assert app.config_data.vfcurve.default_path == "curve.csv"
    assert app.config_data.vfcurve.quick_export is True
    assert app.actions == [
        ["--gpu=0", "set", "vfp", "export", "curve.csv", "-q"]
    ]
