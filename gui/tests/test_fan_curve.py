"""Fan-curve feature tests: policy routing (Apply with policy=curve), the
Curve-button popup wiring, the curve editor's pure geometry helpers, and
the NativeBackend activate path (editor state → driver write)."""

from __future__ import annotations

from types import SimpleNamespace

from src.backend.base import FanSettings
from src.backend.native import NativeBackend
from src.tabs.dashboard.sections.fan import FanControlController
from src.widgets.fan_curve_editor import (
    FanCurveEditor,
    clamp_curve_points,
    percent_to_rpm,
    rpm_to_percent,
)
from tests.test_fan_control import FakeBackend, FakePane


class _CurveFakeBackend(FakeBackend):
    """FakeBackend + the curve-activation surface the controller calls."""

    def __init__(self, app=None) -> None:
        super().__init__()
        self.app = app
        self.activated = 0

    def activate_fan_curve(self) -> None:
        self.activated += 1


def _controller_with_curve(policy: str = "curve"):
    pane = FakePane(api="NVAPI", policy=policy)
    backend = _CurveFakeBackend()
    return FanControlController(pane, backend), pane, backend


def test_modern_nvapi_policy_values_include_curve() -> None:
    controller, pane, _ = _controller_with_curve(policy="continuous")

    controller.on_backend_change()

    assert pane.policy_values == ["continuous", "curve"]
    assert pane.policy == "continuous"


def test_legacy_nvapi_policy_values_exclude_curve() -> None:
    controller, pane, _ = _controller_with_curve(policy="continuous")

    controller.set_legacy_nvapi(True)

    assert pane.policy_values == ["default", "manual"]
    assert "curve" not in pane.policy_values


def test_apply_with_curve_policy_activates_editor_curve() -> None:
    """policy=curve Apply = activate the editor's slot (write + policy
    switch), never the level pin."""
    controller, _, backend = _controller_with_curve(policy="curve")

    controller.apply()

    assert backend.activated == 1
    assert backend.applied == []


def test_apply_with_continuous_policy_still_pins_level() -> None:
    controller, pane, backend = _controller_with_curve(policy="continuous")

    controller.apply()

    assert backend.activated == 0
    assert backend.applied == [
        FanSettings(backend="nvapi-cooler", fan_id=None, policy="continuous", level=60)
    ]


def test_curve_button_opens_editor_proxy() -> None:
    opened: list[bool] = []

    class _Proxy:
        def open(self) -> None:
            opened.append(True)

    app = SimpleNamespace(curve_editor=_Proxy())
    pane = FakePane(api="NVAPI")
    backend = _CurveFakeBackend(app=app)

    FanControlController(pane, backend).open_curve_editor()

    assert opened == [True]


# ── editor geometry helpers (pure, no Tk) ─────────────────────────────────


def test_rpm_percent_conversion_needs_max_rpm() -> None:
    assert rpm_to_percent(800, 4000) == 20
    assert percent_to_rpm(50, 4000) == 2000
    assert rpm_to_percent(800, None) is None
    assert percent_to_rpm(50, 0) is None


def test_rpm_percent_conversion_clamps() -> None:
    assert rpm_to_percent(5000, 4000) == 100
    assert percent_to_rpm(150, 4000) == 4000


def test_clamp_curve_points_passthrough() -> None:
    points = [(40, 800), (60, 1600), (75, 2400)]
    assert clamp_curve_points(points, 4000) == points


def test_clamp_curve_points_pulls_dragged_point_into_range() -> None:
    # A drag past the axes ceiling clamps onto it; the last point still
    # nudges forward to keep both lanes strictly increasing.
    assert clamp_curve_points([(40, 800), (100, 1600), (75, 9999)], 4000) == [
        (40, 800),
        (100, 1600),
        (101, 4000),
    ]


def test_clamp_curve_points_enforces_monotonic_lanes() -> None:
    # Dragged point 0 past point 1: the later points nudge forward instead.
    assert clamp_curve_points([(80, 2000), (50, 1000), (90, 2500)], 4000) == [
        (80, 2000),
        (81, 2001),
        (90, 2500),
    ]


def test_clamp_curve_points_infeasible_returns_none() -> None:
    # Point 0 at the temperature ceiling leaves no room for two more
    # strictly-increasing points.
    assert clamp_curve_points([(110, 4000), (50, 1000), (90, 2500)], 4000) is None
    assert clamp_curve_points([(40, "abc"), (60, 1600), (75, 2400)], 4000) is None


# ── editor model logic via __new__ (no Tk/matplotlib) ──────────────────────


class _StubVar:
    def __init__(self, value: str = "") -> None:
        self._value = value

    def get(self) -> str:
        return self._value

    def set(self, value: str) -> None:
        self._value = str(value)

    def trace_add(self, *_args) -> None:
        pass


def _bare_editor(points, max_rpm=4000) -> FanCurveEditor:
    editor = FanCurveEditor.__new__(FanCurveEditor)
    editor._slot = 0
    editor._syncing = False
    editor._points = list(points)
    editor._max_rpm = max_rpm
    editor._entry_vars = [(_StubVar(str(t)), _StubVar(str(r))) for t, r in points]
    editor._pct_labels = [SimpleNamespace(configure=lambda **kw: None)] * 3
    editor.ax = object()  # chart identity check in _on_press/_on_motion
    editor._redraw = lambda: None  # type: ignore[method-assign]
    return editor


def test_editor_current_state_returns_copy() -> None:
    editor = _bare_editor([(40, 800), (60, 1600), (75, 2400)])

    slot, points = editor.current_state()

    assert slot == 0
    assert points == [(40, 800), (60, 1600), (75, 2400)]
    assert points is not editor._points


def test_editor_entry_edit_updates_model_and_mirror() -> None:
    editor = _bare_editor([(40, 800), (60, 1600), (75, 2400)])
    editor._entry_vars[1][1].set("1700")

    editor._on_entry_write(1, 1)

    assert editor._points == [(40, 800), (60, 1700), (75, 2400)]
    # The % label mirror follows the RPM edit.
    assert editor._entry_vars[1][0].get() == "60"


def test_editor_entry_drag_race_snaps_back() -> None:
    """A single-point edit that breaks lane monotonicity snaps the entry
    back to the held model instead of silently corrupting the table."""
    editor = _bare_editor([(40, 800), (60, 1600), (75, 2400)])
    editor._entry_vars[2][1].set("100")  # below point 1's RPM

    editor._on_entry_write(2, 1)

    # Forward-nudge repairs it (75,2400)→(76,1601) keeps lanes increasing.
    assert editor._points[1] == (60, 1600)
    assert editor._points[2][1] == 1601


def test_editor_drag_motion_updates_points() -> None:
    editor = _bare_editor([(40, 800), (60, 1600), (75, 2400)])
    editor._drag_index = 1
    axes = editor.ax  # _on_motion checks event.inaxes is self.ax
    event = SimpleNamespace(inaxes=axes, xdata=55.0, ydata=1800.0)

    editor._on_motion(event)

    assert editor._points == [(40, 800), (55, 1800), (75, 2400)]
    assert editor._entry_vars[1][0].get() == "55"
    editor._on_release(event)
    assert editor._drag_index is None


# ── NativeBackend.activate_fan_curve ───────────────────────────────────────


class _CurveNativeStub:
    def __init__(self) -> None:
        self.set_calls: list[tuple] = []
        self.query_calls: list[str] = []
        self.table = {
            "curves": [
                {
                    "index": 0,
                    "points": [
                        {"temp_c": 40, "rpm": 800},
                        {"temp_c": 60, "rpm": 1600},
                        {"temp_c": 75, "rpm": 2400},
                    ],
                }
            ]
        }

    def query_fan_curve(self, gpu: str) -> dict:
        self.query_calls.append(gpu)
        return self.table

    def set_fan_curve(self, gpu, curve_index, points, activate=True, fan_id=None):
        self.set_calls.append((gpu, curve_index, list(points), activate, fan_id))
        return {
            "applied": True,
            "policy_switched_to_continuous": True,
            "pin_released": True,
        }


class _ActivateFakeApp:
    def __init__(self, state, gpu="0x0000") -> None:
        self._state = state
        self._gpu = gpu
        self.native = _CurveNativeStub()
        self.outputs: list[str | None] = []
        self.console = SimpleNamespace(append=lambda text: None)

    def selected_gpu_target(self):
        return self._gpu

    def curve_editor_state(self):
        return self._state

    def run_native_action(self, description, action, on_finished=None):
        self.outputs.append(action(self.native))
        return True


def test_activate_fan_curve_uses_editor_state() -> None:
    app = _ActivateFakeApp(
        (2, [(35, 500), (55, 1500), (80, 3000)]),
    )
    backend = NativeBackend(app)

    backend.activate_fan_curve()

    # Editor points win — no driver read needed.
    assert app.native.query_calls == []
    assert app.native.set_calls == [
        ("0x0000", 2, [(35, 500), (55, 1500), (80, 3000)], True, None)
    ]
    assert "Activated fan curve 2" in (app.outputs[-1] or "")


def test_activate_fan_curve_falls_back_to_driver_table() -> None:
    """Editor never opened → read the driver table and write the slot's own
    points back before activating (SetFanCurve is RMW anyway)."""
    app = _ActivateFakeApp((0, None))
    backend = NativeBackend(app)

    backend.activate_fan_curve()

    assert app.native.query_calls == ["0x0000"]
    assert app.native.set_calls == [
        ("0x0000", 0, [(40, 800), (60, 1600), (75, 2400)], True, None)
    ]


def test_activate_fan_curve_without_table_reports_unreadable() -> None:
    app = _ActivateFakeApp((3, None))
    app.native.table = {"curves": []}
    backend = NativeBackend(app)

    backend.activate_fan_curve()

    assert app.native.set_calls == []
    assert "unreadable" in (app.outputs[-1] or "")
