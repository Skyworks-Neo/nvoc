from __future__ import annotations

import threading

from rich.text import Text
from textual.widgets import Button, Checkbox, Input, Select
from textual_plotext import PlotextPlot

from ..models import CurveData
from ..parsing import (
    CURVE_META,
    build_vf_curves,
    compute_vf_plot_bounds_multi,
    find_curve_point_for_voltage,
    load_vf_curve_deltas,
    public_vfp_unsupported,
    reverse_lookup_voltage,
    write_vf_curve_points,
)
from ..widgets import mnemonic_text
from .base import PaneController

# Per-curve plotext colors: (current line, default scatter).
_CURVE_COLORS = {
    "gpc": ("cyan+", "white"),
    "xbar": ("orange+", "gray+"),
    "host": ("magenta+", "gray+"),
}
_CURVE_ORDER = ("gpc", "xbar", "host")


class VFCurveController(PaneController):
    def __init__(self, app) -> None:
        super().__init__(app)
        self.poll_timer = None
        self.refresh_inflight = False
        self._refresh_lock = threading.Lock()
        # Multi-curve state (TUI port of the GUI selector, minus editing).
        self._curves: dict[str, CurveData] = {}
        self._active_curve = "gpc"
        self._curve_visible: dict[str, bool] = {}
        self._direct_read_inflight = False
        # Last option list pushed to the curve Select (suppresses no-op
        # set_options calls — each one posts Changed events).
        self._synced_options: list | None = None
        # True while (and shortly after) _sync_curve_widgets writes widget
        # state programmatically. Select/Checkbox Changed events posted by
        # those writes are processed while this is still set (queue order:
        # they precede the call_after_refresh that clears it), so the app
        # handlers can drop them as echoes. Without this, a stale echo can
        # be mistaken for a real switch and re-write the widgets, resonating
        # into an unbounded switch ping-pong (each hop = full plot render +
        # a Log thread worker that never completes under load).
        self._syncing = False

    def auto_refresh_label(self) -> Text:
        state = "On" if self.app.config_data.vfcurve.auto_refresh else "Off"
        return mnemonic_text("A", f"uto Refresh: {state}")

    def set_poll_timer(self, enabled: bool) -> None:
        self.app.config_data.vfcurve.auto_refresh = enabled
        self.app.save_config()
        if self.poll_timer is not None:
            self.poll_timer.stop()
            self.poll_timer = None
        if enabled:
            self.poll_timer = self.app.set_interval(2.0, self.tick, pause=False)
        try:
            self.app.query_one(
                "#vf-auto-refresh", Button
            ).label = self.auto_refresh_label()
        except Exception:
            pass
        if (
            enabled
            and not self.app.native_service.action_state.running
            and not self.is_refresh_inflight()
        ):
            self.refresh_curve()

    def activate_shortcut(self, target_id: str) -> bool:
        if target_id in {
            "vf-path",
            "vf-range-start",
            "vf-lock-value",
            "vf-mem-min",
        }:
            self.app.query_one(f"#{target_id}", Input).focus()
            return True
        if target_id == "vf-freq-api":
            self.app.query_one("#vf-freq-api", Select).focus()
            return True
        return self.handle_button(target_id)

    def tick(self) -> None:
        if self.app.native_service.action_state.running or self.is_refresh_inflight():
            return
        self.refresh_curve()

    def is_refresh_inflight(self) -> bool:
        with self._refresh_lock:
            return self.refresh_inflight

    def _begin_refresh(self) -> bool:
        with self._refresh_lock:
            if self.refresh_inflight:
                return False
            self.refresh_inflight = True
            return True

    def _end_refresh(self) -> None:
        with self._refresh_lock:
            self.refresh_inflight = False

    def sync_from_ui(self) -> None:
        self.app.config_data.vfcurve.default_path = self.app.query_one(
            "#vf-path", Input
        ).value.strip()
        self.app.save_config()

    def refresh_curve(self) -> None:
        if not self._begin_refresh():
            return
        try:
            gpu = self.app.selected_gpu_target()
        except Exception:
            self._end_refresh()
            raise
        if gpu is None:
            self._end_refresh()
            self.clear_plot("No GPU selected.")
            return

        def worker() -> None:
            gpc_points: list[dict] | None = None
            gpc_err: str | None = None
            clk_data: dict | None = None
            try:
                gpc_points = self.app.native_service.query_public_vftable(gpu)
            except Exception as exc:
                gpc_err = str(exc)
            try:
                clk_data = self.app.native_service.query_private_vftable(gpu)
            except Exception:
                clk_data = None
            try:
                self.app.call_from_thread(
                    self.on_curve_loaded, gpc_points, gpc_err, clk_data
                )
            except Exception:
                self._end_refresh()
                raise

        try:
            self.app.native_service.submit_query(worker)
        except Exception:
            self._end_refresh()
            raise

    def on_curve_loaded(
        self,
        gpc_points: "list[dict] | None",
        gpc_err: str | None,
        clk_data: dict | None,
    ) -> None:
        self._end_refresh()
        curves = build_vf_curves(gpc_points, gpc_err, clk_data)
        self.app.cache.vf_curve_points = gpc_points if curves else None
        self.app.cache.vf_curves = curves
        if curves is None:
            self._curves = {}
            self._curve_visible = {}
            self.app.cache.curve_visible = {}
            self.app.cache.vf_live_point = None
            if gpc_err and not public_vfp_unsupported(gpc_err):
                self.app.write_log(f"pynvoc VFP curve query failed: {gpc_err}")
            self.clear_plot("VF curve query failed.")
            self._sync_curve_widgets()
            return
        # Carry over visibility (default: every discovered curve visible) and
        # keep the active curve valid (must exist and be visible).
        prev_visible = self._curve_visible or {}
        self._curves = curves
        self._curve_visible = {cid: prev_visible.get(cid, True) for cid in curves}
        if self._active_curve not in curves or not self._curve_visible.get(
            self._active_curve
        ):
            self._active_curve = next(
                (cid for cid in curves if self._curve_visible.get(cid)), "gpc"
            )
        self.app.cache.curve_visible = dict(self._curve_visible)
        self.app.cache.active_curve = self._active_curve
        self.app.cache.vf_live_point = None
        self.render_plot()
        self._sync_curve_widgets()
        if self._active_curve in ("xbar", "host") and not self._direct_read_inflight:
            self._kick_direct_read(self._active_curve)

    def clear_plot(self, title: str) -> None:
        try:
            widget = self.app.query_one("#vf-plot", PlotextPlot)
        except Exception:
            return  # pane not composed / being torn down
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        plt.title(title)
        plt.xlabel("mV")
        plt.ylabel("MHz")
        widget.refresh()

    def render_plot(self) -> None:
        curves = self._curves or {}
        if not curves:
            self.clear_plot("No VF curve loaded.")
            return
        visible = [
            curves[cid]
            for cid in _CURVE_ORDER
            if cid in curves and self._curve_visible.get(cid, True)
        ]
        if not visible:
            self.clear_plot("No VF curve visible.")
            return
        active_id = self._active_curve if self._active_curve in curves else "gpc"
        active = curves.get(active_id) if self._curve_visible.get(active_id) else None
        try:
            widget = self.app.query_one("#vf-plot", PlotextPlot)
        except Exception:
            return  # pane not composed / being torn down
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        for curve in visible:
            current_color, default_color = _CURVE_COLORS.get(
                curve.curve_id, _CURVE_COLORS["gpc"]
            )
            label = CURVE_META.get(curve.curve_id, CURVE_META["gpc"])["label"]
            plt.plot(
                curve.voltages,
                curve.frequencies,
                marker="braille",
                color=current_color,
                label=label,
            )
            plt.scatter(
                curve.voltages, curve.defaults, marker="braille", color=default_color
            )
        # Live crosshair only on the active curve: GPC from the dashboard
        # status feed, XBAR/HOST from the direct-read poll path. Hidden
        # curves are neither plotted nor polled.
        live_point: tuple[float, float] | None = None
        live_color = "yellow+"
        live_voltage: float | None = None
        if active is not None:
            if active_id == "gpc":
                live_voltage = self.app.cache.status.get("voltage_mv")
                live_clock = self.app.cache.status.get("gpu_clock_mhz")
                if isinstance(live_voltage, (int, float)) and isinstance(
                    live_clock, (int, float)
                ):
                    live_point = (float(live_voltage), float(live_clock))
            else:
                live_color = "green+"
                cached_point = self.app.cache.vf_live_point
                if cached_point is not None:
                    live_point = cached_point
        if live_point is not None:
            plt.scatter(
                [live_point[0]],
                [live_point[1]],
                marker="braille",
                color=live_color,
                label="Live Point",
            )
            plt.vline(live_point[0], color=live_color)
            plt.hline(live_point[1], color=live_color)
        # Lock + working-point markers stay on the active curve (the public
        # lock concepts only apply to one curve at a time).
        lock_point: tuple[float, float] | None = None
        lock_voltage_mv: float | None = None
        lock_voltage = self.app.cache.status.get("vfp_lock_mv")
        if active is not None and isinstance(lock_voltage, (int, float)):
            lock_voltage_mv = float(lock_voltage)
            lock_curve_point = find_curve_point_for_voltage(
                active.voltages, active.frequencies, lock_voltage_mv
            )
            if lock_curve_point is not None:
                lock_point = (lock_voltage_mv, lock_curve_point[1])
        if lock_point is not None:
            plt.vline(lock_point[0], color="orange+")
            plt.hline(lock_point[1], color="orange+")
            plt.text(
                "Locked at {} mV".format(lock_voltage_mv),
                lock_point[0],
                0,
                color="orange+",
                alignment="right",
            )
        working_point = None
        if active is not None:
            working_point = find_curve_point_for_voltage(
                active.voltages,
                active.frequencies,
                float(live_voltage) if isinstance(live_voltage, (int, float)) else None,
            )
        if working_point is not None:
            plt.hline(working_point[1], color="green+")
        bounds = compute_vf_plot_bounds_multi(
            visible,
            live_point=live_point,
            lock_point=lock_point,
            working_point=working_point,
        )
        if bounds is not None:
            (x_min, x_max), (y_min, y_max) = bounds
            plt.xlim(x_min, x_max)
            plt.ylim(y_min, y_max)
        plt.title("VF Curve")
        plt.xlabel("mV")
        plt.ylabel("MHz")
        widget.refresh()

    # ── Curve selector: active switch + visibility (TUI subtraction) ──

    def poll_live(self) -> None:
        """Dashboard 1 Hz hook: re-render + kick a direct read for xbar/host."""
        if not self._curves:
            return
        self.render_plot()
        if self._active_curve in ("xbar", "host") and not self._direct_read_inflight:
            self._kick_direct_read(self._active_curve)

    def _sync_curve_widgets(self) -> None:
        """Reflect curve discovery/active/visibility into the selector row.

        Tolerates a missing selector (bare-app tests, pre-compose) like
        ``set_poll_timer`` does. Every programmatic ``set_options``/
        ``value`` write posts Changed events back into the app handlers —
        mutating reactives from inside those same handlers can resonate
        into an unbounded event echo loop (pegged core + runaway memory).
        So each write happens only when the widget state actually differs
        from the target, and ``set_options`` (which resets the Select to
        blank — another Changed) only when the discovered set changed.
        """
        try:
            select = self.app.query_one("#vf-active-curve", Select)
        except Exception:
            return
        discovered = [cid for cid in _CURVE_ORDER if cid in self._curves]
        options = [(CURVE_META[cid]["label"], cid) for cid in discovered] or [
            ("GPC", "gpc")
        ]
        if self._synced_options != options:
            self._synced_options = options
            select.set_options(options)
        self._syncing = True
        if select.value != self._active_curve:
            select.value = self._active_curve
        for cid in _CURVE_ORDER:
            try:
                checkbox = self.app.query_one(f"#vf-curve-{cid}", Checkbox)
            except Exception:
                continue
            checkbox.disabled = cid not in self._curves
            want = cid in self._curves and self._curve_visible.get(cid, True)
            if checkbox.value != want:
                checkbox.value = want
        # Echoes posted above are drained before this runs (queue order),
        # so the sync window closes only after they have been ignored.
        self.app.call_after_refresh(self._end_widget_sync)

    def _end_widget_sync(self) -> None:
        self._syncing = False

    def _switch_active_curve(self, curve_id: str) -> None:
        """Select which curve apply/reset and the live crosshair target."""
        if curve_id not in self._curves or curve_id == self._active_curve:
            return
        if not self._curve_visible.get(curve_id, True):
            return  # activating a hidden curve makes no sense
        self._active_curve = curve_id
        self.app.cache.active_curve = curve_id
        self.app.cache.vf_live_point = None
        self.render_plot()
        self._sync_curve_widgets()
        if curve_id in ("xbar", "host") and not self._direct_read_inflight:
            self._kick_direct_read(curve_id)
        curve = self._curves[curve_id]
        self.app.write_log(
            f"Active curve: {CURVE_META[curve_id]['label']} "
            f"({curve.source}, {curve.write_mode})."
        )

    def _toggle_curve_visible(self, curve_id: str, checkbox=None) -> None:
        """Toggle a curve's visibility. Hidden curves are not drawn and not
        polled. Never leaves zero visible curves; vetoes snap the checkbox
        back."""
        if curve_id not in self._curves:
            return
        currently = self._curve_visible.get(curve_id, True)
        if currently and curve_id == self._active_curve:
            if sum(1 for v in self._curve_visible.values() if v) <= 1:
                if checkbox is not None:
                    # Vetoed — snap the checkbox back OUTSIDE this Changed
                    # handler: mutating the reactive reentrantly from inside
                    # its own callback is what can resonate into an event
                    # echo loop (pegged core + runaway memory).
                    self.app.call_after_refresh(setattr, checkbox, "value", True)
                return
        self._curve_visible[curve_id] = not currently
        if not self._curve_visible.get(curve_id):
            if curve_id == self._active_curve:
                self.app.cache.vf_live_point = None
        if not self._curve_visible.get(self._active_curve, True):
            self._active_curve = next(
                (c for c, v in self._curve_visible.items() if v), self._active_curve
            )
            self.app.cache.vf_live_point = None
        self.app.cache.curve_visible = dict(self._curve_visible)
        self.app.cache.active_curve = self._active_curve
        self.render_plot()
        self._sync_curve_widgets()
        if self._active_curve in ("xbar", "host") and not self._direct_read_inflight:
            self._kick_direct_read(self._active_curve)

    def _kick_direct_read(self, curve_id: str) -> None:
        """Direct physical clock read (0x527FC458) for the live crosshair."""
        curve = self._curves.get(curve_id)
        if curve is None or not self._curve_visible.get(curve_id, True):
            return
        if curve_id != self._active_curve or self._direct_read_inflight:
            return
        try:
            gpu = self.app.selected_gpu_target()
        except Exception:
            return
        if gpu is None:
            return
        domain_bit = CURVE_META[curve_id]["domain_bit"]
        # Snapshot for the completion callback (a refresh may replace the
        # curve dicts while the read is in flight).
        volts = list(curve.voltages)
        freqs = list(curve.frequencies)
        self._direct_read_inflight = True

        def worker() -> None:
            try:
                result = self.app.native_service.query_private_freq_domain_status(
                    gpu, domain_bit
                )
            except Exception:
                # _on_direct_read_done resets the inflight flag; without this
                # guard a raised escape leaves it stuck True forever and the
                # live crosshair silently dies (or retry logic spins).
                result = None
            try:
                self.app.call_from_thread(
                    self._on_direct_read_done, result, curve_id, volts, freqs
                )
            except Exception:
                self._direct_read_inflight = False
                raise

        try:
            self.app.native_service.submit_query(worker)
        except Exception:
            self._direct_read_inflight = False
            raise

    def _on_direct_read_done(
        self,
        result: dict | None,
        curve_id: str,
        volts: list[float],
        freqs: list[float],
    ) -> None:
        self._direct_read_inflight = False
        # Stale (different active curve now) or hidden while in flight — a
        # hidden curve gets no crosshair, so drop the result.
        if curve_id != self._active_curve or not self._curve_visible.get(curve_id):
            return
        if not isinstance(result, dict):
            return
        if result.get("supported") is False:
            self.app.cache.vf_live_point = None
            return
        freq_khz = result.get("freq_khz")
        if not freq_khz:
            # 0 ⇒ driver refused / not measurable through this interface.
            return
        freq_mhz = freq_khz / 1000.0
        volt = reverse_lookup_voltage(volts, freqs, freq_mhz)
        if volt is None:
            return
        self.app.cache.vf_live_point = (volt, freq_mhz)
        self.render_plot()

    def handle_button(self, button_id: str) -> bool:
        if button_id == "vf-refresh":
            self.sync_from_ui()
            self.refresh_curve()
            return True
        if button_id == "vf-auto-refresh":
            self.sync_from_ui()
            self.set_poll_timer(not self.app.config_data.vfcurve.auto_refresh)
            return True
        if button_id == "vf-export":
            self.sync_from_ui()
            path = self.app.query_one("#vf-path", Input).value.strip()
            if not path:
                self.app.write_log("VFP export path is empty.")
                return True
            gpu = self.app.selected_gpu_target()

            def export(native, gpu=gpu, path=path) -> str:
                points = native.query_public_vftable(gpu, "graphics", True)
                write_vf_curve_points(path, points)
                return f"Exported {len(points)} VFP point(s) to {path}."

            self.app.run_native_action("export VFP curve", export)
            return True
        if button_id == "vf-import":
            self.sync_from_ui()
            path = self.app.query_one("#vf-path", Input).value.strip()
            if not path:
                self.app.write_log("VFP import path is empty.")
                return True
            gpu = self.app.selected_gpu_target()

            def import_curve(native, gpu=gpu, path=path) -> str:
                points = native.query_public_vftable(gpu, "graphics", True)
                deltas = load_vf_curve_deltas(path, points)
                native.set_domain_vfp_deltas(gpu, "graphics", deltas)
                return f"Imported {len(deltas)} VFP point delta(s) from {path}."

            self.app.run_native_action("import VFP curve", import_curve)
            return True
        if button_id == "vf-reset":
            # Active-curve semantics: public GPC resets its own segment via
            # the open interface; private curves (XBAR/HOST, or GPC when the
            # public family is unsupported) clear per point with a
            # raw-converted fallback. Never touches other curves' segments.
            curve = self._curves.get(self._active_curve)
            if curve is None:
                self.app.write_log("No active curve to reset.")
                return True
            gpu = self.app.selected_gpu_target()
            cid = curve.curve_id.upper()

            if curve.write_mode == "public":
                seg_start, seg_end = curve.seg_start, curve.seg_end

                def reset_public(
                    native, gpu=gpu, seg_start=seg_start, seg_end=seg_end, cid=cid
                ) -> str:
                    native.set_vfp_range_delta(gpu, seg_start, seg_end, 0)
                    return (
                        f"Successfully reset {cid} curve to default "
                        f"({seg_start}-{seg_end}, public)."
                    )

                self.app.run_native_action("reset VFP deltas", reset_public)
                return True

            bank = curve.bank
            base = curve.seg_start
            end_idx = curve.seg_end
            class_name = CURVE_META[curve.curve_id]["class"]
            defaults_mhz = list(curve.defaults)

            def reset_private(
                native,
                gpu=gpu,
                bank=bank,
                base=base,
                end_idx=end_idx,
                class_name=class_name,
                defaults_mhz=defaults_mhz,
                cid=cid,
            ) -> str:
                # 1) mode-0 clear (value 0) per point in the segment.
                try:
                    for idx in range(base, end_idx + 1):
                        r = native.set_vfp_point_private(gpu, bank, idx, 0, True)
                        if isinstance(r, dict) and r.get("supported") is False:
                            raise RuntimeError("private VFP family unsupported")
                    return (
                        f"Successfully reset {cid} (private mode-0, {base}-{end_idx})."
                    )
                except Exception as exc:
                    msg = str(exc).lower()
                    if "argument" not in msg and "unsupported" not in msg:
                        raise
                # 2) raw-converted clear: delta 0 → the raw f-offset that
                # zeroes the effect (≈ D0 per the g(def) prior).
                raw_deltas = []
                for idx in range(base, end_idx + 1):
                    local = idx - base
                    def_mhz = (
                        int(round(defaults_mhz[local]))
                        if local < len(defaults_mhz)
                        else 0
                    )
                    r = native.clk_vf_delta_for_target_mhz(def_mhz, 0.0, class_name)
                    d = r.get("delta") if isinstance(r, dict) else None
                    raw_deltas.append(int(d) if d is not None else 0)
                r2 = native.set_vfp_range_per_point_private(
                    gpu, bank, base, end_idx, raw_deltas
                )
                if isinstance(r2, dict) and r2.get("supported") is False:
                    return f"private reset unsupported on {cid}."
                return f"Successfully reset {cid} (private raw, {base}-{end_idx})."

            self.app.run_native_action("reset VFP deltas", reset_private)
            return True
        if button_id == "vf-unlock":
            gpu = self.app.selected_gpu_target()

            def reset_vfp_lock(native, gpu=gpu) -> str:
                native.reset_vfp_lock(gpu)
                return "Successfully reset VFP lock."

            self.app.run_native_action(
                "reset VFP lock",
                reset_vfp_lock,
            )
            return True
        if button_id == "vf-apply-adj":
            start = self.get_int("#vf-range-start")
            end = self.get_int("#vf-range-end")
            delta = self.get_int("#vf-delta")
            if start > end:
                start, end = end, start
            curve = self._curves.get(self._active_curve)
            if curve is None or not curve.frequencies or not curve.defaults:
                self.app.write_log("No VF data loaded for the active curve.")
                return True
            n = len(curve.frequencies)
            start = max(0, min(start, n - 1))
            end = max(0, min(end, n - 1))
            gpu = self.app.selected_gpu_target()

            if curve.write_mode == "public":
                # Open VFP interface (public GPC, unchanged).
                def apply_vfp_delta(
                    native, gpu=gpu, start=start, end=end, delta=delta
                ) -> str:
                    native.set_vfp_range_delta(gpu, start, end, delta * 1000)
                    return (
                        f"Successfully applied {delta} MHz VFP delta "
                        f"to points {start}-{end}."
                    )

                self.app.run_native_action(
                    "apply VFP range delta",
                    apply_vfp_delta,
                )
                return True

            # Private path: mode-0 first, raw-converted fallback (port of the
            # GUI apply_private closure).
            bank = curve.bank
            base = curve.seg_start + start  # absolute private index of `start`
            class_name = CURVE_META[curve.curve_id]["class"]
            defaults_mhz = list(curve.defaults)
            cid = curve.curve_id.upper()
            # TUI range inputs apply a uniform delta over the range.
            deltas_khz = [delta * 1000] * (end - start + 1)

            def apply_private(
                native,
                gpu=gpu,
                bank=bank,
                base=base,
                class_name=class_name,
                defaults_mhz=defaults_mhz,
                deltas_khz=deltas_khz,
                start=start,
                cid=cid,
            ) -> str:
                # 1) mode-0 (kHz frequency offset) per point.
                try:
                    for offset, dkz in enumerate(deltas_khz):
                        r = native.set_vfp_point_private(
                            gpu, bank, base + offset, dkz, True
                        )
                        if isinstance(r, dict) and r.get("supported") is False:
                            raise RuntimeError("private VFP family unsupported")
                    return (
                        f"Successfully applied private mode-0 offsets to {cid} "
                        f"({len(deltas_khz)} pts)."
                    )
                except Exception as exc:
                    msg = str(exc).lower()
                    if "argument" not in msg and "unsupported" not in msg:
                        raise
                # 2) raw-converted: translate each MHz offset to a raw
                # mode-1 f-offset via the g(def) prior.
                raw_deltas = []
                for offset in range(len(deltas_khz)):
                    def_mhz = int(round(defaults_mhz[start + offset]))
                    tgt_mhz = deltas_khz[offset] / 1000.0
                    r = native.clk_vf_delta_for_target_mhz(def_mhz, tgt_mhz, class_name)
                    d = r.get("delta") if isinstance(r, dict) else None
                    if d is None:
                        return (
                            f"raw-converted translation failed at "
                            f"def={def_mhz} MHz ({cid}); apply aborted."
                        )
                    raw_deltas.append(int(d))
                last = base + len(deltas_khz) - 1
                r2 = native.set_vfp_range_per_point_private(
                    gpu, bank, base, last, raw_deltas
                )
                if isinstance(r2, dict) and r2.get("supported") is False:
                    return f"private VFP write unsupported on {cid}."
                return (
                    f"Successfully applied private raw-converted offsets "
                    f"to {cid} ({len(raw_deltas)} pts)."
                )

            self.app.run_native_action(
                "apply VFP point deltas",
                apply_private,
            )
            return True
        if button_id == "vf-lock-voltage":
            value = self.app.query_one("#vf-lock-value", Input).value.strip()
            if self.app.query_one("#vf-lock-as-mv", Checkbox).value:
                try:
                    voltage_uv = int(float(value) * 1000)
                except (OverflowError, ValueError):
                    self.app.write_log(
                        "Invalid VFP lock voltage: enter a numeric mV value."
                    )
                    return True
                point = None
            else:
                voltage_uv = None
                try:
                    point = int(value)
                except ValueError:
                    self.app.write_log(
                        "Invalid VFP lock point: enter a numeric point index."
                    )
                    return True
            gpu = self.app.selected_gpu_target()

            def lock_vfp_voltage(
                native, gpu=gpu, point=point, voltage_uv=voltage_uv
            ) -> str:
                native.set_vfp_voltage_lock(gpu, point, voltage_uv, False)
                if voltage_uv is not None:
                    return (
                        f"Successfully locked VFP voltage to {voltage_uv / 1000:g} mV."
                    )
                return f"Successfully locked VFP voltage to point {point}."

            self.app.run_native_action(
                "lock VFP voltage",
                lock_vfp_voltage,
            )
            return True
        if button_id == "vf-lock-core":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            gpu = self.app.selected_gpu_target()
            min_mhz = self.get_int("#vf-core-min")
            max_mhz = self.get_int("#vf-core-max")

            def lock_core(
                native, gpu=gpu, backend=backend, min_mhz=min_mhz, max_mhz=max_mhz
            ) -> str:
                if backend == "nvapi":
                    native.set_vfp_frequency_lock(
                        gpu, "core", max_mhz * 1000, min_mhz * 1000
                    )
                else:
                    native.set_locked_clocks(gpu, backend, "core", min_mhz, max_mhz)
                return f"Successfully locked core clocks to {min_mhz}-{max_mhz} MHz."

            self.app.run_native_action(
                "lock core clocks",
                lock_core,
            )
            return True
        if button_id == "vf-reset-core":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            gpu = self.app.selected_gpu_target()

            def reset_core(native, gpu=gpu, backend=backend) -> str:
                if backend == "nvapi":
                    native.reset_vfp_frequency_lock(gpu, "core")
                else:
                    native.reset_core_clocks(gpu, backend)
                return "Successfully reset core clocks."

            self.app.run_native_action(
                "reset core clocks",
                reset_core,
            )
            return True
        if button_id == "vf-lock-mem":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            gpu = self.app.selected_gpu_target()
            min_mhz = self.get_int("#vf-mem-min")
            max_mhz = self.get_int("#vf-mem-max")

            def lock_mem(
                native, gpu=gpu, backend=backend, min_mhz=min_mhz, max_mhz=max_mhz
            ) -> str:
                if backend == "nvapi":
                    native.set_vfp_frequency_lock(
                        gpu, "memory", max_mhz * 1000, min_mhz * 1000
                    )
                else:
                    native.set_locked_clocks(gpu, backend, "memory", min_mhz, max_mhz)
                return f"Successfully locked memory clocks to {min_mhz}-{max_mhz} MHz."

            self.app.run_native_action(
                "lock memory clocks",
                lock_mem,
            )
            return True
        if button_id == "vf-reset-mem":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            gpu = self.app.selected_gpu_target()

            def reset_mem(native, gpu=gpu, backend=backend) -> str:
                if backend == "nvapi":
                    native.reset_vfp_frequency_lock(gpu, "memory")
                else:
                    native.reset_mem_clocks(gpu, backend)
                return "Successfully reset memory clocks."

            self.app.run_native_action(
                "reset memory clocks",
                reset_mem,
            )
            return True
        return False
