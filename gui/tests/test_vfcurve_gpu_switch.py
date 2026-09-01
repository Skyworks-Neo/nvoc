"""Tests for the GPU-switch / unsupported-GPU clearing of the VF Curve tab.

Regression: with a VF-curve-capable card and an incapable card installed,
switching to the incapable one left the previous GPU's curve on the chart —
nothing cleared the display and (without auto-refresh) nothing re-queried.

Covers:
- ``on_gpu_changed``: the previous GPU's curve leaves the chart immediately
  and a reload is fired for the new GPU (stale workers are epoch-dropped).
- ``_on_multi_query_done``: a "not supported" verdict clears the chart with
  an explanatory message; a transient error keeps the current display.
- The epoch-mismatch path consumes a pending refresh (a GPU switch that
  arrived while a refresh was inflight must not be swallowed).
"""

from __future__ import annotations

import matplotlib

matplotlib.use("Agg")  # headless: no Tk display required

import matplotlib.pyplot as plt  # noqa: E402

from src.tabs.vfcurve.tab import VFCurveTab, _CurveData  # noqa: E402


class _FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class _UnsupportedBackend:
    """Backend of a GPU with no V/F-curve interface at all (e.g. Fermi)."""

    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def query_public_vftable(self, gpu: str):
        self.calls.append(("public", gpu))
        raise RuntimeError("NvAPI function not supported on this device")

    def query_private_vftable(self, gpu: str):
        self.calls.append(("private", gpu))
        return None

    def query_volt_rails(self, gpu: str) -> dict:
        self.calls.append(("volt_rails", gpu))
        return {"p0": {}}


class _FakeApp:
    def __init__(self, backend) -> None:
        self.console = _FakeConsole()
        self.backend = backend

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def after(self, _delay, callback):
        # Run immediately so worker completions land synchronously.
        callback()

    def run_background(self, _name, task):
        task()


def _make_tab(backend=None) -> VFCurveTab:
    """A populated VF Curve tab (previous GPU's curve loaded) on a real Agg
    figure, wired to an unsupported-GPU backend by default."""
    tab = VFCurveTab.__new__(VFCurveTab)
    tab.app = _FakeApp(backend if backend is not None else _UnsupportedBackend())
    tab._cleaned_up = False
    # Previous GPU's curve loaded (multi-curve state + legacy single view).
    curve = _CurveData("gpc")
    curve.voltages = [700.0, 800.0, 900.0]
    curve.frequencies = [1400.0, 1500.0, 1600.0]
    curve.defaults = [1400.0, 1500.0, 1600.0]
    tab._curves = {"gpc": curve}
    tab._curve_visible = {"gpc": True}
    tab._active_curve = "gpc"
    tab._voltages = curve.voltages
    tab._frequencies = curve.frequencies
    tab._defaults = curve.defaults
    tab._sel_start = None
    tab._sel_end = None
    tab._drag_orig_freqs = None
    tab._locked_points = {1}
    tab._freq_core_lock = None
    tab._freq_mem_lock = None
    # Refresh-chain state.
    tab._curve_query_epoch = 0
    tab._refresh_curve_inflight = False
    tab._refresh_curve_pending = False
    tab._auto_refreshing = False
    tab._last_load_ts = 0.0
    # Live-point state.
    tab._live_volt = 800.0
    tab._live_freq = 1500.0
    tab._live_pending = (800.0, 1500.0)
    tab._pending_live_point = None
    tab._live_elements = []
    tab._live_hline = None
    tab._live_vline = None
    tab._live_marker = None
    tab._live_text = None
    # P0 wall state (per-GPU).
    tab._p0_bounds = {"min_hold_uV": 600_000}
    tab._p0_bounds_gpu = "GPU0"
    tab._p0_rails = []
    tab._p0_bounds_by_rail = {0: {"min_hold_uV": 600_000}}
    tab._p0_effective_by_rail = {0: 1000.0}
    tab._p0_effective_wall_mv = 1000.0
    tab._pending_wall_mv = 990.0
    tab._dragging_wall = False
    tab._p0_rail_bit = 0
    tab._pending_wall_line = None
    tab._wall_handle = None
    tab._dragging = False
    tab._drag_start_y = None
    # _redraw guards / lazy imports referenced along the draw path.
    tab._volt_unit_tick = None
    tab._is_resize_active = False
    tab._mouse_pressed = False
    tab._blit_bg = None
    # Real figure so _redraw actually renders the empty-chart message.
    tab.fig, tab.ax = plt.subplots()
    tab.fig.patch.set_facecolor("#2b2b2b")
    tab.canvas = matplotlib.backends.backend_agg.FigureCanvasAgg(tab.fig)
    return tab


def _ax_texts(ax) -> list[str]:
    return [t.get_text() for t in ax.texts]


def test_on_gpu_changed_clears_previous_curve_and_reloads() -> None:
    tab = _make_tab()

    tab.on_gpu_changed()

    # Previous GPU's curve (and every piece of per-GPU state) is gone…
    assert tab._curves == {}
    assert tab._voltages == []
    assert tab._active_curve == "gpc"
    assert tab._locked_points == set()
    assert tab._p0_bounds is None
    assert tab._p0_effective_wall_mv is None
    assert tab._pending_wall_mv is None
    # …the reload was fired for the new GPU, and its "not supported"
    # verdict replaced the chart content (not the old curve).
    assert ("public", "GPU0") in tab.app.backend.calls
    assert "No VF curve on this GPU" in _ax_texts(tab.ax)
    assert any("not supported" in m for m in tab.app.console.messages)


def test_multi_query_done_unsupported_verdict_clears_display() -> None:
    tab = _make_tab()

    tab._on_multi_query_done(
        tab._curve_query_epoch, "GPU0", None, "NvAPI not supported", None
    )

    assert tab._curves == {}
    assert tab._voltages == []
    assert "No VF curve on this GPU" in _ax_texts(tab.ax)
    assert any("not supported" in m for m in tab.app.console.messages)


def test_multi_query_done_transient_error_keeps_display() -> None:
    tab = _make_tab()

    tab._on_multi_query_done(tab._curve_query_epoch, "GPU0", None, "read timeout", None)

    # A transient failure keeps the current (same-GPU) curve on the chart.
    assert tab._voltages == [700.0, 800.0, 900.0]
    assert any("VFP query failed" in m for m in tab.app.console.messages)
    assert "No VF curve on this GPU" not in _ax_texts(tab.ax)


def test_stale_epoch_consumes_pending_refresh() -> None:
    """A refresh requested while another was inflight (the GPU-switch reload
    racing an auto-refresh tick) must run after the stale one lands."""
    tab = _make_tab()
    tab._refresh_curve_pending = True

    tab._on_multi_query_done(
        tab._curve_query_epoch - 1, "GPU0", None, "read timeout", None
    )

    # The re-armed reload ran (force=True bypasses the dedup gate) and the
    # pending flag was consumed, not leaked.
    assert tab._refresh_curve_pending is False
    assert ("public", "GPU0") in tab.app.backend.calls


def test_on_gpu_changed_drops_inflight_old_gpu_result() -> None:
    """A worker for the previous GPU that lands after the switch must not
    repopulate the cleared chart (epoch guard)."""
    tab = _make_tab()

    tab.on_gpu_changed()
    # A late worker from the pre-switch chain (old epoch) lands now.
    tab._on_multi_query_done(0, "GPU0", None, "NvAPI not supported", None)

    assert tab._curves == {}
    assert tab._voltages == []
    assert "No VF curve on this GPU" in _ax_texts(tab.ax)
