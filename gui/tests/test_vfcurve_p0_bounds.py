"""Tests for the P0 voltage-boundary vertical lines on the VF Curve chart.

Covers:
- ``_redraw`` draws the floor/ceiling (deep red) + effective (light red)
  ``axvline``s when P0 bounds are present, and none when absent.
- ``ensure_p0_bounds`` queries once per GPU and short-circuits on repeat.
- ``update_p0_effective_wall`` moves only the light-red line and redraws.
"""

from __future__ import annotations

import matplotlib

matplotlib.use("Agg")  # headless: no Tk display required

import matplotlib.pyplot as plt  # noqa: E402

from src.tabs.vfcurve.tab import VFCurveTab  # noqa: E402

# Deep-red (floor/ceiling) and light-red (effective) line colors — mirror
# the literals in VFCurveTab._redraw so the assertions track the draw.
_DEEP_RED = "#8b0000"
_LIGHT_RED = "#ff6b6b"


class _FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class _FakeBackend:
    """Records query_volt_rails calls; returns a canned p0 payload."""

    def __init__(self, p0: dict | None) -> None:
        self._p0 = p0
        self.calls: list[str] = []

    def query_volt_rails(self, gpu: str) -> dict:
        self.calls.append(gpu)
        return {"p0": self._p0}


class _FakeApp:
    def __init__(self, backend: _FakeBackend) -> None:
        self.console = _FakeConsole()
        self.backend = backend

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def after(self, _delay: int, callback) -> None:
        # Run immediately so the worker's completion lands synchronously.
        callback()

    def run_background(self, _name: str, task) -> None:
        task()


def _sample_p0() -> dict:
    """A 4060-Laptop-shaped P0 bounds payload (µV)."""
    return {
        "current_uV": 940_000,
        "target_wall_uV": 1_005_000,
        "effective_wall_uV": 1_005_000,
        "vbios_wall_uV": 0,
        "vrm_max_wall_uV": 1_200_000,
        "min_hold_uV": 625_000,
        "offset_ceiling_uV": 195_000,
    }


def _make_tab(p0: dict | None = _sample_p0(), gpu: str = "GPU0") -> VFCurveTab:
    """Build a VFCurveTab wired to a real Agg figure so _redraw draws."""
    tab = VFCurveTab.__new__(VFCurveTab)
    tab.app = _FakeApp(_FakeBackend(p0))
    tab._cleaned_up = False
    tab._voltages = [700.0, 800.0, 900.0, 1000.0, 1100.0]
    tab._frequencies = [1400.0, 1500.0, 1600.0, 1700.0, 1800.0]
    tab._defaults = list(tab._frequencies)
    tab._sel_start = None
    tab._sel_end = None
    tab._locked_points = set()
    tab._freq_core_lock = None
    tab._freq_mem_lock = None
    tab._curve_visible = {}
    tab._curves = {}
    tab._active_curve = "gpc"
    tab._volt_unit_tick = None
    tab._p0_bounds = None
    tab._p0_bounds_gpu = None
    tab._p0_effective_wall_mv = None
    tab._p0_rails = []
    tab._p0_bounds_by_rail = {}
    tab._p0_effective_by_rail = {}
    tab._pending_wall_mv = None
    tab._dragging_wall = False
    tab._p0_rail_bit = 0
    tab._pending_wall_line = None
    tab._wall_handle = None
    tab._dragging = False
    tab._drag_start_y = None
    # _redraw guards / lazy imports referenced along the draw path.
    tab._is_resize_active = False
    tab._mouse_pressed = False
    tab._blit_bg = None  # _blit_animated falls back to draw_idle when None
    tab._pending_live_point = None  # _flush_pending_live_point guard
    tab._live_elements = []
    tab._live_hline = None
    tab._live_vline = None
    tab._live_marker = None
    tab._live_text = None
    tab._live_volt = None
    tab._live_freq = None
    # Real figure so _redraw actually creates axvline artists.
    tab.fig, tab.ax = plt.subplots()
    tab.fig.patch.set_facecolor("#2b2b2b")
    tab.canvas = matplotlib.backends.backend_agg.FigureCanvasAgg(tab.fig)
    return tab


def _axvline_colors(ax) -> list[str]:
    """Colors of every axvline drawn on ax (in draw order).

    axvline(x) produces a 2-point Line2D whose xdata is ``[x, x]`` (constant
    x, spanning the axes' y-limit). We filter to constant-x lines with a
    drawn linestyle — that excludes the curve lines (varying x) and the
    marker-only locked/selection artists (linestyle "none").
    """
    import matplotlib.lines as mlines

    out = []
    for ln in ax.get_lines():
        if not isinstance(ln, mlines.Line2D):
            continue
        if ln.get_linestyle() in (None, "None", "none", ""):
            continue
        xs = ln.get_xdata()  # orig=True: raw data, not display coords
        if len(xs) == 2 and xs[0] == xs[1]:
            out.append(ln.get_color())
    return out


def test_redraw_draws_three_p0_lines_when_bounds_present() -> None:
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")  # populates _p0_bounds + effective
    # ensure_p0_bounds already calls _redraw once; reset for a clean assertion
    tab.ax.clear()

    tab._redraw()

    deep = [c for c in _axvline_colors(tab.ax) if c == _DEEP_RED]
    light = [c for c in _axvline_colors(tab.ax) if c == _LIGHT_RED]
    # floor + ceiling = two deep-red; effective = one light-red.
    assert len(deep) == 2, deep
    assert len(light) == 1, light


def test_redraw_attaches_wall_handle_to_axes() -> None:
    """_redraw must recreate the wall-handle triangle after ax.clear()
    destroys the prior patch (regression: a stale reference left the
    triangle invisible because it was no longer in the axes)."""
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    tab.ax.clear()

    tab._redraw()

    # The handle Polygon must be present in the axes' patches and visible.
    import matplotlib.patches as mpatches

    polys = [p for p in tab.ax.patches if isinstance(p, mpatches.Polygon)]
    assert len(polys) >= 1
    handle = tab._wall_handle
    assert handle is not None
    assert handle in tab.ax.patches
    assert handle.get_visible()
    assert handle.get_clip_on() is False


def test_redraw_draws_no_p0_lines_when_bounds_absent() -> None:
    tab = _make_tab(p0=None)
    tab.ax.clear()

    tab._redraw()

    colors = _axvline_colors(tab.ax)
    assert not any(c == _DEEP_RED for c in colors)
    assert not any(c == _LIGHT_RED for c in colors)


def test_ensure_p0_bounds_queries_once_per_gpu() -> None:
    tab = _make_tab()
    assert tab.app.backend.calls == []

    tab.ensure_p0_bounds("GPU0")
    assert len(tab.app.backend.calls) == 1
    assert tab._p0_bounds is not None

    # Same GPU again — cache hit, no extra query.
    tab.ensure_p0_bounds("GPU0")
    assert len(tab.app.backend.calls) == 1

    # New GPU — re-query and clear the stale effective line.
    tab.ensure_p0_bounds("GPU1")
    assert len(tab.app.backend.calls) == 2


def test_update_p0_effective_wall_moves_only_light_red() -> None:
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    assert tab._p0_effective_wall_mv == 1005.0  # 1_005_000 µV

    # Floor/ceiling come from the cached p0 dict and must not change.
    floor_before = tab._p0_bounds["min_hold_uV"]
    vrm_before = tab._p0_bounds["vrm_max_wall_uV"]

    # Swap _redraw for a counter so we can assert a redraw fired once.
    redraws = {"n": 0}
    _real_redraw = tab._redraw

    def _counting_redraw():
        redraws["n"] += 1

    tab._redraw = _counting_redraw

    tab.update_p0_effective_wall(1_100_000)

    assert tab._p0_effective_wall_mv == 1100.0
    assert redraws["n"] == 1  # value changed → one redraw
    # Hardware walls untouched.
    assert tab._p0_bounds["min_hold_uV"] == floor_before
    assert tab._p0_bounds["vrm_max_wall_uV"] == vrm_before

    # A no-op push (same value) must NOT redraw again.
    tab.update_p0_effective_wall(1_100_000)
    assert redraws["n"] == 1

    tab._redraw = _real_redraw  # restore for teardown


# ── Wall drag + apply (pending → Apply to GPU) ──


class _FakeMouseEvent:
    """Minimal matplotlib MouseEvent stub for hit-test/drag tests.

    ``x``/``y`` are display pixels (mpl origin bottom-left); ``xdata`` is the
    data-space x (None when the click is outside the axes, e.g. on the handle).
    """

    def __init__(self, xdata, x=None, y=None, inaxes=True, button=1):
        self.xdata = xdata
        self.x = x
        self.y = y
        self.inaxes = inaxes if inaxes else None
        self.button = button


def _build_wall_handle(tab) -> None:
    """Force the wall-drag triangle to exist at the current wall position."""
    tab._draw_wall_handle(call_draw_idle=False)


def _handle_center(tab) -> tuple[float, float]:
    """Display-px center of the wall handle triangle."""
    bbox = tab._wall_handle.get_window_extent()
    return ((bbox.x0 + bbox.x1) / 2.0, (bbox.y0 + bbox.y1) / 2.0)


def test_hit_wall_handle_false_outside_triangle() -> None:
    """A click off the triangle (e.g. on a VF point) does not grab it."""
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    _build_wall_handle(tab)
    # Click well to the left of the handle center (on a VF point, in-axes).
    cx, cy = _handle_center(tab)
    ev = _FakeMouseEvent(xdata=700.0, x=cx - 200.0, inaxes=tab.ax)
    ev.y = cy  # display y near the triangle but x off it
    assert bool(tab._hit_wall_handle(ev)) is False


def test_hit_wall_handle_true_on_triangle() -> None:
    """A click on the triangle grabs it (inaxes=None, above the axes)."""
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    _build_wall_handle(tab)
    cx, cy = _handle_center(tab)
    # inaxes=None: the triangle is in the top figure margin, outside the axes.
    ev = _FakeMouseEvent(xdata=None, x=cx, inaxes=None)
    ev.y = cy
    assert bool(tab._hit_wall_handle(ev)) is True


def test_wall_clamp_bounds_uses_p0_walls() -> None:
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    lo, hi = tab._wall_clamp_bounds()
    # Lower bound is the plot's left x-edge (the effective wall may be
    # dragged below the P0 min_hold floor), floored at 450 mV. The test tab's
    # auto xlim left edge is ~680 → lo = 680, not the 625 mV min_hold.
    xlim_lo = tab.ax.get_xlim()[0]
    assert lo == max(450.0, float(xlim_lo))
    # Upper bound stays the hardware ceiling min(vbios, vrm) = 1200 mV.
    assert hi == 1200.0


def test_drag_wall_via_handle_sets_pending_and_clamps() -> None:
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    _build_wall_handle(tab)
    # Press on the triangle center (inaxes=None → handle grab, not point select).
    cx, cy = _handle_center(tab)
    press = _FakeMouseEvent(xdata=None, x=cx, inaxes=None)
    press.y = cy
    tab._on_mouse_press(press)
    assert tab._dragging_wall is True
    assert tab._pending_wall_mv == tab._p0_effective_wall_mv

    # Drag to 1150 mV (inside [625, 1200]). The drag handler recovers data-x
    # from display px (event.xdata is None above the axes), so pass display px.
    px_1150 = tab.ax.transData.transform((1150.0, 0.0))[0]
    move1 = _FakeMouseEvent(xdata=None, x=px_1150, inaxes=None)
    tab._on_mouse_move(move1)
    assert tab._pending_wall_mv == 1150.0

    # Drag beyond ceiling → clamped to 1200.
    px_1300 = tab.ax.transData.transform((1300.0, 0.0))[0]
    move2 = _FakeMouseEvent(xdata=None, x=px_1300, inaxes=None)
    tab._on_mouse_move(move2)
    assert tab._pending_wall_mv == 1200.0

    # Release keeps the pending value (no apply yet).
    release = _FakeMouseEvent(xdata=None, x=px_1300, button=1, inaxes=None)
    tab._on_mouse_release(release)
    assert tab._dragging_wall is False
    assert tab._pending_wall_mv == 1200.0


def test_wall_handle_follows_pending() -> None:
    """The triangle tracks _pending_wall_mv when a drag is in progress."""
    tab = _make_tab()
    tab.ensure_p0_bounds("GPU0")
    _build_wall_handle(tab)
    eff_mv = tab._p0_effective_wall_mv
    # Before a drag, the handle sits at the effective wall.
    assert tab._wall_handle.get_xy()[0][0] == eff_mv

    # Set a pending value and redraw the handle — it should follow.
    tab._pending_wall_mv = 1100.0
    tab._draw_wall_handle(call_draw_idle=False)
    assert tab._wall_handle.get_xy()[0][0] == 1100.0


class _ApplyNative:
    """Records set_volt_rail_target / set_vfp_range_delta calls."""

    def __init__(self):
        self.calls: list = []

    def set_volt_rail_target(self, gpu, rail_bit, target_mv, expected_type):
        self.calls.append(("set_volt_rail_target", gpu, rail_bit, target_mv))
        return {"applied": True, "effective_wall_uV": int(target_mv * 1000)}

    def set_vfp_range_delta(self, gpu, frm, to, dkz):
        self.calls.append(("set_vfp_range_delta", gpu, frm, to, dkz))
        return {}


class _ApplyApp(_FakeApp):
    """App that runs run_native_action synchronously and records."""

    def __init__(self, backend):
        super().__init__(backend)
        self._native = _ApplyNative()
        self.actions: list = []

    def run_native_action(self, description, action, on_finished=None):
        msg = action(self._native)
        self.actions.append((description, msg))
        if on_finished is not None:
            on_finished(0)


def test_apply_wall_only_when_delta_zero() -> None:
    """Pending wall + delta==0 → apply only the wall, no VFP write."""
    tab = _make_tab()
    # Swap in an app that captures native calls.
    tab.app = _ApplyApp(tab.app.backend)
    tab.ensure_p0_bounds("GPU0")
    tab._pending_wall_mv = 1100.0  # simulate a finished drag
    tab.adj_start_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_end_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_delta_var = type("V", (), {"get": lambda self: "0"})()
    tab._drag_orig_freqs = None
    # _apply_adj reads self.app.selected_gpu_target via the apply lambda;
    # _ApplyApp inherits _FakeApp which returns "GPU0".

    tab._apply_adj()

    # Only the wall setter fired, no VFP range delta.
    assert any(c[0] == "set_volt_rail_target" for c in tab.app._native.calls)
    assert not any(c[0] == "set_vfp_range_delta" for c in tab.app._native.calls)
    # target snapped to 2.5 mV grid: 1100.0 is already on-grid.
    wall_call = next(c for c in tab.app._native.calls if c[0] == "set_volt_rail_target")
    assert wall_call[3] == 1100.0
    # Pending consumed.
    assert tab._pending_wall_mv is None


def test_apply_snaps_to_2_5mv_grid() -> None:
    tab = _make_tab()
    tab.app = _ApplyApp(tab.app.backend)
    tab.ensure_p0_bounds("GPU0")
    tab._pending_wall_mv = 1007.3  # off-grid free drag
    tab.adj_start_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_end_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_delta_var = type("V", (), {"get": lambda self: "0"})()
    tab._drag_orig_freqs = None

    tab._apply_adj()

    wall_call = next(c for c in tab.app._native.calls if c[0] == "set_volt_rail_target")
    # round(1007.3 / 2.5) * 2.5 = 1007.5
    assert wall_call[3] == 1007.5


# ── Multi-rail parts: per-curve VoltRails rail selection (P100 / 50-series) ──


def _p100_payload() -> dict:
    """A P100-shaped query_volt_rails payload: rail 0 = core/GPC, rail 1 =
    HBM MEM (status values [current, target, vbios, vrm, effective,
    min_hold] µV, from a live GP100)."""
    rail0 = {
        "rail_bit": 0,
        "current_uV": 675_000,
        "target_wall_uV": 1_131_250,
        "effective_wall_uV": 1_125_000,
        "vbios_wall_uV": 0,
        "vrm_max_wall_uV": 1_125_000,
        "min_hold_uV": 675_000,
        "offset_ceiling_uV": 0,
    }
    rail1 = {
        "rail_bit": 1,
        "current_uV": 681_250,
        "target_wall_uV": 1_018_750,
        "effective_wall_uV": 1_018_750,
        "vbios_wall_uV": 0,
        "vrm_max_wall_uV": 1_125_000,
        "min_hold_uV": 681_250,
        "offset_ceiling_uV": 0,
    }
    return {"p0": rail0, "p0_rails": [rail0, rail1]}


class _MultiRailBackend(_FakeBackend):
    def __init__(self, payload: dict) -> None:
        super().__init__(None)
        self._payload = payload

    def query_volt_rails(self, gpu: str) -> dict:
        self.calls.append(gpu)
        return self._payload


def _make_multirail_tab() -> VFCurveTab:
    tab = _make_tab()
    tab.app = _ApplyApp(_MultiRailBackend(_p100_payload()))
    return tab


def _vline_positions(ax) -> set[float]:
    """Data-x of every drawn (constant-x) axvline on the axes."""
    import matplotlib.lines as mlines

    out = set()
    for ln in ax.get_lines():
        if not isinstance(ln, mlines.Line2D):
            continue
        if ln.get_linestyle() in (None, "None", "none", ""):
            continue
        xs = ln.get_xdata()
        if len(xs) == 2 and xs[0] == xs[1]:
            out.add(float(xs[0]))
    return out


def test_multirail_gpc_active_uses_primary_rail() -> None:
    tab = _make_multirail_tab()
    tab.ensure_p0_bounds("GPU0")
    tab.ax.clear()

    assert tab._active_p0_rail_bit() == 0
    assert tab._p0_effective_wall_mv == 1125.0
    tab._redraw()
    assert _vline_positions(tab.ax) == {675.0, 1125.0}


def test_multirail_mem_active_uses_secondary_rail() -> None:
    tab = _make_multirail_tab()
    tab.ensure_p0_bounds("GPU0")
    tab._active_curve = "mem"
    tab._sync_active_p0_view()
    tab.ax.clear()

    assert tab._active_p0_rail_bit() == 1
    assert tab._p0_effective_wall_mv == 1018.75
    # Clamp ceiling follows the MEM rail's hardware walls.
    assert tab._wall_clamp_bounds()[1] == 1125.0
    tab._redraw()
    # floor 681.25 / ceiling 1125.0 / effective 1018.75 — all rail-1.
    assert _vline_positions(tab.ax) == {681.25, 1125.0, 1018.75}
    # The secondary-rail label tag shows up when the rail isn't primary.
    assert tab._active_p0_rail_tag() == " (MEM)"


def test_multirail_curve_switch_drops_pending_wall() -> None:
    """A pending wall drag is clamped against the OLD rail's walls — it must
    not survive a curve switch to another rail."""
    tab = _make_multirail_tab()
    tab.ensure_p0_bounds("GPU0")
    tab._pending_wall_mv = 1100.0
    tab._dragging_wall = True

    tab._active_curve = "mem"
    tab._sync_active_p0_view()

    assert tab._pending_wall_mv is None
    assert tab._dragging_wall is False
    assert tab._p0_effective_wall_mv == 1018.75


def test_multirail_wall_apply_targets_active_rail() -> None:
    """Applying a wall while the MEM curve is selected must SET rail 1."""
    tab = _make_multirail_tab()
    tab.ensure_p0_bounds("GPU0")
    tab._active_curve = "mem"
    tab._sync_active_p0_view()
    tab._pending_wall_mv = 1000.0
    tab.adj_start_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_end_var = type("V", (), {"get": lambda self: "0"})()
    tab.adj_delta_var = type("V", (), {"get": lambda self: "0"})()
    tab._drag_orig_freqs = None

    tab._apply_adj()

    wall_call = next(c for c in tab.app._native.calls if c[0] == "set_volt_rail_target")
    assert wall_call[2] == 1  # rail_bit 1 (HBM MEM)
    assert wall_call[3] == 1000.0


def test_multirail_primary_apply_keeps_mem_view_intact() -> None:
    """A Volt Limit SET on the PRIMARY rail (overclock panel apply) must not
    move the MEM curve's displayed wall."""
    tab = _make_multirail_tab()
    tab.ensure_p0_bounds("GPU0")
    tab._active_curve = "mem"
    tab._sync_active_p0_view()
    assert tab._p0_effective_wall_mv == 1018.75

    # Overclock panel applies on the primary rail (no rail_bit arg).
    tab.update_p0_effective_wall(1_050_000, 0)

    # Per-rail record updated for rail 0, but the visible MEM wall stands.
    assert tab._p0_effective_by_rail[0] == 1050.0
    assert tab._p0_effective_wall_mv == 1018.75
