from __future__ import annotations

import threading
from pathlib import Path

from rich.text import Text
from textual.widgets import Button, Checkbox, Input, Select
from textual_plotext import PlotextPlot

from ..parsing import compute_vf_plot_bounds, find_curve_point_for_voltage, load_vf_curve
from .base import PaneController


class VFCurveController(PaneController):
    def __init__(self, app) -> None:
        super().__init__(app)
        self.poll_timer = None
        self.refresh_inflight = False

    def auto_refresh_label(self) -> Text:
        state = "On" if self.app.config_data.vfcurve.auto_refresh else "Off"
        return Text.assemble(("A", "underline"), f"uto Refresh: {state}")

    def set_poll_timer(self, enabled: bool) -> None:
        self.app.config_data.vfcurve.auto_refresh = enabled
        self.app.save_config()
        if self.poll_timer is not None:
            self.poll_timer.stop()
            self.poll_timer = None
        if enabled:
            self.poll_timer = self.app.set_interval(2.0, self.tick, pause=False)
        try:
            self.app.query_one("#vf-auto-refresh", Button).label = (
                self.auto_refresh_label()
            )
        except Exception:
            pass
        if (
            enabled
            and not self.app.cli_service.action_state.running
            and not self.refresh_inflight
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
        if target_id == "vf-quick-export":
            checkbox = self.app.query_one("#vf-quick-export", Checkbox)
            checkbox.value = not checkbox.value
            self.sync_from_ui()
            return True
        return self.handle_button(target_id)

    def tick(self) -> None:
        if self.app.cli_service.action_state.running or self.refresh_inflight:
            return
        self.refresh_curve()

    def cache_path(self) -> Path:
        cache_dir = self.app.root_dir / "vfp_cache"
        cache_dir.mkdir(exist_ok=True)
        gpu = self.app.current_gpu()
        if gpu and gpu.uuid:
            return cache_dir / f"{gpu.uuid}.csv"
        idx = self.app.selected_gpu_idx() or 0
        return cache_dir / f"gpu_{idx}.csv"

    def sync_from_ui(self) -> None:
        self.app.config_data.vfcurve.default_path = self.app.query_one(
            "#vf-path", Input
        ).value.strip()
        self.app.config_data.vfcurve.quick_export = self.app.query_one(
            "#vf-quick-export", Checkbox
        ).value
        self.app.save_config()

    def refresh_curve(self) -> None:
        if self.refresh_inflight:
            return
        cache_path = self.cache_path()
        self.refresh_inflight = True

        def worker() -> None:
            code, output, _ = self.app.cli_service.run_query(
                self.app.config_data.cli,
                self.app.gpu_args() + ["set", "vfp", "export", "-q", str(cache_path)],
                "",
            )
            self.app.call_from_thread(
                self.on_curve_loaded, output, str(cache_path), code
            )

        threading.Thread(target=worker, daemon=True, name="vf-refresh").start()

    def on_curve_loaded(self, output: str, path: str, code: int) -> None:
        self.refresh_inflight = False
        if output:
            self.app.write_log(output)
        self.app.cache.vf_curve_path = path
        if code == 0:
            self.render_plot()
        else:
            self.clear_plot("VF curve export failed.")

    def clear_plot(self, title: str) -> None:
        widget = self.app.query_one("#vf-plot", PlotextPlot)
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        plt.title(title)
        plt.xlabel("mV")
        plt.ylabel("MHz")
        widget.refresh()

    def render_plot(self) -> None:
        if not self.app.cache.vf_curve_path:
            self.clear_plot("No VF curve cache loaded.")
            return
        voltages, freqs, defaults = load_vf_curve(self.app.cache.vf_curve_path)
        if not voltages:
            self.clear_plot("VF curve cache is empty.")
            return
        widget = self.app.query_one("#vf-plot", PlotextPlot)
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        plt.plot(voltages, freqs, marker="braille", color="cyan+", label="Current")
        plt.scatter(voltages, defaults, marker="braille", color="white", label="Default")
        live_voltage = self.app.cache.status.get("voltage_mv")
        live_clock = self.app.cache.status.get("gpu_clock_mhz")
        lock_voltage = self.app.cache.status.get("vfp_lock_mv")
        live_point: tuple[float, float] | None = None
        lock_point: tuple[float, float] | None = None
        if isinstance(live_voltage, (int, float)) and isinstance(
            live_clock, (int, float)
        ):
            live_point = (float(live_voltage), float(live_clock))
            plt.scatter(
                [live_point[0]],
                [live_point[1]],
                marker="braille",
                color="yellow+",
                label="Live Point",
            )
            plt.vline(live_point[0], color="yellow+")
            plt.hline(live_point[1], color="yellow+")
        lock_voltage_mv: float | None = None
        if isinstance(lock_voltage, (int, float)):
            lock_voltage_mv = float(lock_voltage)
        if lock_voltage_mv is not None:
            lock_curve_point = find_curve_point_for_voltage(
                voltages, freqs, lock_voltage_mv
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
        working_point = find_curve_point_for_voltage(
            voltages,
            freqs,
            float(live_voltage) if isinstance(live_voltage, (int, float)) else None,
        )
        if working_point is not None:
            plt.hline(working_point[1], color="green+")
        bounds = compute_vf_plot_bounds(
            voltages,
            freqs,
            defaults,
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
            args = self.app.gpu_args() + ["set", "vfp", "export", path]
            if self.app.query_one("#vf-quick-export", Checkbox).value:
                args.append("-q")
            self.app.run_cli_action(args)
            return True
        if button_id == "vf-import":
            self.sync_from_ui()
            path = self.app.query_one("#vf-path", Input).value.strip()
            self.app.run_cli_action(self.app.gpu_args() + ["set", "vfp", "import", path])
            return True
        if button_id == "vf-reset":
            self.app.run_cli_action(self.app.gpu_args() + ["reset", "vfp"])
            return True
        if button_id == "vf-unlock":
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", "nvapi", "--reset-volt-locks"]
            )
            return True
        if button_id == "vf-apply-adj":
            start = self.get_int("#vf-range-start")
            end = self.get_int("#vf-range-end")
            delta = self.get_int("#vf-delta")
            if start > end:
                start, end = end, start
            self.app.run_cli_action(
                self.app.gpu_args()
                + ["set", "vfp", "pointwiseoc", f"{start}-{end}", f"{delta * 1000:+d}"]
            )
            return True
        if button_id == "vf-lock-voltage":
            value = self.app.query_one("#vf-lock-value", Input).value.strip()
            if self.app.query_one("#vf-lock-as-mv", Checkbox).value:
                value = f"{value}mV"
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", "nvapi", "--locked-voltage", value]
            )
            return True
        if button_id == "vf-lock-core":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            self.app.run_cli_action(
                self.app.gpu_args()
                + [
                    "set",
                    backend,
                    "--locked-core-clocks",
                    str(self.get_int("#vf-core-min")),
                    str(self.get_int("#vf-core-max")),
                ]
            )
            return True
        if button_id == "vf-reset-core":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", backend, "--reset-core-clocks"]
            )
            return True
        if button_id == "vf-lock-mem":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            self.app.run_cli_action(
                self.app.gpu_args()
                + [
                    "set",
                    backend,
                    "--locked-mem-clocks",
                    str(self.get_int("#vf-mem-min")),
                    str(self.get_int("#vf-mem-max")),
                ]
            )
            return True
        if button_id == "vf-reset-mem":
            backend = str(self.app.query_one("#vf-freq-api", Select).value or "nvml")
            self.app.run_cli_action(
                self.app.gpu_args() + ["set", backend, "--reset-mem-clocks"]
            )
            return True
        return False
