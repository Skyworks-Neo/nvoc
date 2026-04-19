from __future__ import annotations

import threading
from pathlib import Path

from textual.app import App, ComposeResult
from textual.containers import Horizontal, HorizontalScroll, Vertical
from textual.widgets import Button, Checkbox, Footer, Header, Input, Label, Log, Select, Static, TabbedContent, TabPane
from textual_plotext import PlotextPlot

from .cli import CliService
from .config import ConfigStore
from .models import AppConfig, GpuCache, GpuDescriptor, repo_root
from .parsing import find_curve_point_for_voltage, load_vf_curve


class NVOCApp(App[None]):
    TITLE = "NVOC-TUI"
    CSS = """
    # Main Screen & common widgets
    Screen {
        layout: vertical;
        overflow: hidden;
    }
    .hsplit {
        height: 1;
        border-top: solid $surface;
        margin: 0 1;
    }
    .row, .toprow {
        height: auto;
        margin: 0;
        width: 1fr;
        align-horizontal: center;
    }
    .grow {
        width: 1fr !important;
    }
    .row > *, .toprow > * {
        margin: 0 1;
    }
    .row > Input {
        min-width: 6;
        width: 6;
    }
    .green {
        background: green;
    }
    .red {
        background: red;
    }
    .blue {
        background: blue;
    }
    .nvapi-nvml-select {
        width: 8;
    }

    # Top Bar
    #topbar {
        height: auto;
        margin-left: 1;
    }
    .toprow {
        height: 1;
    }
    #gpu-actions {
        width: auto;
    }
    #gpu-actions > * {
        margin-left: 1;
    }

    # Dashboard Controls
    #dashboard-controls {
        width: auto;
        height: auto;
    }
    #dashboard-controls Button {
        min-width: 8;
        width: auto;
    }
    #dashboard-interval {
        width: 8;
        padding: 0 1;
    }
    .section {
        overflow: auto;
    }
    #metrics {
        height: auto;
        border: round $surface;
        padding: 0 2;
    }
    
    # VF Curve tab
    #vf-plot {
        min-height: 20;
        height: 1fr;
        border: round $accent;
        overflow: auto;
        content-align: left top;
    }
    #vfcurve #vf-plot {
        margin: 0 2;
    }
    #log-header {
        height: auto;
        padding: 0;
        background: $surface;
    }
    #log-panel {
        height: 5;
    }
    #log-panel.hidden {
        display: none;
    }
    Log {
        height: 14;
        border: round $surface;
    }
    .section {
        margin: 0 2;
    }
    TabbedContent {
        height: 1fr;
        overflow: hidden;
    }
    TabbedContent > ContentSwitcher {
        height: 1fr;
        overflow: hidden;
    }
    TabPane {
        height: 1fr;
    }
    TabPane > .section {
        height: 1fr;
    }
    """
    BINDINGS = [("ctrl+c", "quit", "Quit")]

    def __init__(self) -> None:
        super().__init__()
        self.root_dir = repo_root()
        self.config_store = ConfigStore(self.root_dir)
        self.config_data: AppConfig = self.config_store.load()
        discovered = CliService.discover_cli(self.config_data.cli.exe_path)
        if discovered.exe_path:
            self.config_data.cli = discovered
            self.config_store.data = self.config_data
            self.config_store.save()
        self.cli_service = CliService(self.root_dir)
        self.gpus: list[GpuDescriptor] = []
        self.cache = GpuCache()
        self.poll_timer = None
        self.vf_poll_timer = None
        self.vf_refresh_inflight = False

    def compose(self) -> ComposeResult:
        yield Header()
        with Vertical(id="topbar"):
            with Horizontal(classes="toprow"):
                yield Label("GPU: ")
                yield Select(options=[("Detecting...", "-1")], id="gpu-select", allow_blank=False, compact=True, classes="grow")
                with Horizontal(id="gpu-actions"):
                    yield Button("Detect", id="detect-gpus", compact=True)
                    yield Button("Refresh All", id="refresh-all", compact=True)
            with Horizontal(classes="toprow"):
                yield Label("CLI: ")
                yield Input(value=self.config_data.cli.exe_path, placeholder="CLI path", id="cli-path", classes="grow", compact=True)
                yield Button("Save CLI", id="save-cli", compact=True)
        yield Static(classes="hsplit")
        with TabbedContent(initial=self.config_data.ui.active_tab or "dashboard"):
            with TabPane("Dashboard", id="dashboard"):
                with Vertical(classes="section"):
                    with Horizontal(classes="row", id="dashboard-controls"):
                        yield Label("Refresh (s): ")
                        yield Input(value=f"{self.config_data.dashboard.refresh_interval:.1f}", id="dashboard-interval", compact=True)
                        yield Button("Apply", id="dashboard-interval-apply", compact=True)
                        yield Button("Pause", id="dashboard-pause", compact=True)
                        yield Button("Now", id="dashboard-now", compact=True)
                        yield Button("Info", id="dashboard-info", compact=True)
                        yield Button("Status", id="dashboard-status", compact=True)
                        yield Button("Get", id="dashboard-get", compact=True)
                    yield Static("Waiting for first refresh.", id="metrics")
            with TabPane("Autoscan", id="autoscan"):
                with Vertical(classes="section"):
                    with Horizontal(classes="row"):
                        yield Label("Mode")
                        yield Select(
                            options=[
                                ("Standard", "standard"),
                                ("Ultrafast", "ultrafast"),
                                ("Legacy", "legacy"),
                            ],
                            value=self.config_data.autoscan.mode,
                            id="autoscan-mode",
                            allow_blank=False,
                            compact=True,
                        )
                        yield Label("BSOD")
                        yield Select(
                            options=[("(auto)", ""), ("aggressive", "aggressive"), ("traditional", "traditional")],
                            value=self.config_data.autoscan.bsod_recovery,
                            id="autoscan-bsod",
                            allow_blank=False,
                            compact=True,
                        )
                    for label, value, widget_id in [
                        ("Test Executable", self.config_data.autoscan.test_exe, "autoscan-test-exe"),
                        ("Score XML Path", self.config_data.autoscan.score_path, "autoscan-score-path"),
                        ("Score Threshold", self.config_data.autoscan.score_threshold, "autoscan-score"),
                        ("Timeout Loops", self.config_data.autoscan.timeout_loops, "autoscan-timeout"),
                        ("Log File", self.config_data.autoscan.log_file, "autoscan-log"),
                        ("Output CSV", self.config_data.autoscan.output_csv, "autoscan-output"),
                        ("Init CSV", self.config_data.autoscan.init_csv, "autoscan-init"),
                    ]:
                        with Horizontal(classes="row"):
                            yield Label(label)
                            yield Input(value=value, id=widget_id, classes="grow", compact=True)
                    with Horizontal(classes="row"):
                        yield Button("Export Init VFP", id="autoscan-export-init", compact=True)
                        yield Button("Reset & Unlock", id="autoscan-reset-unlock", compact=True)
                        yield Button("Start Autoscan", id="autoscan-start", compact=True)
                        yield Button("Stop", id="autoscan-stop", compact=True)
                        yield Button("Fix Results", id="autoscan-fix", compact=True)
                        yield Button("Import Final", id="autoscan-import-final", compact=True)
                        yield Button("Export Final", id="autoscan-export-final", compact=True)
            with TabPane("Overclock", id="overclock"):
                with Vertical(classes="section"):
                    with Horizontal(classes="row"):
                        yield Label("Clock API")
                        yield Select(options=[("NVAPI", "nvapi"), ("NVML", "nvml")], value="nvapi", classes="nvapi-nvml-select", id="oc-api", allow_blank=False, compact=True)
                        yield Label("PState Start")
                        yield Input(value="", id="pstate-start", compact=True)
                        yield Label("PState End")
                        yield Input(value="", id="pstate-end", compact=True)
                    with Horizontal(classes="row"):
                        yield Label("Core Offset")
                        yield Input(value="0", id="core-offset", compact=True)
                        yield Label("Mem Offset")
                        yield Input(value="0", id="mem-offset", compact=True)
                    with Horizontal(classes="row"):
                        yield Label("Power API")
                        yield Select(options=[("NVAPI", "nvapi"), ("NVML", "nvml")], value="nvapi", classes="nvapi-nvml-select", id="power-api", allow_blank=False, compact=True)
                        yield Label("Power Limit")
                        yield Input(value="100", id="power-limit", compact=True)
                        yield Label("Thermal Limit")
                        yield Input(value="83", id="thermal-limit", compact=True)
                        yield Label("Voltage Boost")
                        yield Input(value="0", id="voltage-boost", compact=True)
                    with Horizontal(classes="row"):
                        yield Button("Apply OC", id="oc-apply", classes="red", compact=True)
                        yield Button("Reset OC", id="oc-reset", classes="green", compact=True)
                        yield Button("Apply Limits", id="limits-apply", classes="red", compact=True)
                        yield Button("Reset All", id="reset-all", classes="green", compact=True)
                    yield Static("Fan Control", classes="row")
                    with Horizontal(classes="row"):
                        yield Label("Fan Target")
                        yield Select(options=[("All", "all"), ("Fan 1", "1"), ("Fan 2", "2")], value="all", id="fan-id", allow_blank=False, compact=True)
                        yield Label("Fan API")
                        yield Select(options=[("NVAPI", "nvapi"), ("NVML", "nvml")], value="nvapi", classes="nvapi-nvml-select", id="fan-api", allow_blank=False, compact=True)
                        yield Label("Policy")
                        yield Select(
                            options=[("contin.", "continuous"), ("manual", "manual"), ("default", "default"), ("auto", "auto")],
                            value="continuous",
                            id="fan-policy",
                            allow_blank=False,
                            compact=True,
                        )
                        yield Label("Level")
                        yield Input(value="60", id="fan-level", compact=True)
                        yield Button("Apply Fan", id="fan-apply", classes="red", compact=True)
                        yield Button("Reset Fan", id="fan-reset", classes="green", compact=True)
            with TabPane("VF Curve", id="vfcurve"):
                with Vertical(classes="section"):
                    with Horizontal(classes="row"):
                        yield Input(value=self.config_data.vfcurve.default_path, placeholder="CSV path for import/export", id="vf-path", classes="grow", compact=True)
                        yield Checkbox("Quick export", value=self.config_data.vfcurve.quick_export, id="vf-quick-export", compact=True)
                    with Horizontal(classes="row", id="vf-actions"):
                        yield Button("Refresh Curve", id="vf-refresh", classes="blue", compact=True)
                        yield Button(self._vf_auto_refresh_label(), id="vf-auto-refresh", compact=True)
                        yield Button("Export VFP", id="vf-export", compact=True)
                        yield Button("Import VFP", id="vf-import", compact=True)
                        yield Button("Unlock VFP", id="vf-unlock", classes="red", compact=True)
                        yield Button("Reset VFP", id="vf-reset", classes="green", compact=True)
                    with Horizontal(classes="row", id="vf-range-actions"):
                        yield Label("VF Adj: Range")
                        yield Input(value="0", id="vf-range-start", compact=True)
                        yield Label("to")
                        yield Input(value="0", id="vf-range-end", compact=True)
                        yield Label("Delta MHz")
                        yield Input(value="0", id="vf-delta", compact=True)
                        yield Button("Apply Adj", id="vf-apply-adj", classes="red", compact=True)
                    with Horizontal(classes="row", id="vf-lock-actions"):
                        yield Label("Lock Value")
                        yield Input(value="55", id="vf-lock-value", compact=True)
                        yield Checkbox("As mV", value=False, id="vf-lock-as-mv", compact=True)
                        yield Button("Lock Voltage", id="vf-lock-voltage", classes="red", compact=True)
                    with Horizontal(classes="row", id="vf-mem-actions"):
                        yield Label("Freq API")
                        yield Select(options=[("NVML", "nvml"), ("NVAPI", "nvapi")], value="nvml", classes="nvapi-nvml-select", id="vf-freq-api", allow_blank=False, compact=True)
                        yield Label("Core Freq Min")
                        yield Input(value="0", id="vf-core-min", compact=True)
                        yield Label("Max")
                        yield Input(value="0", id="vf-core-max", compact=True)
                        yield Button("Lock Core", id="vf-lock-core", classes="red", compact=True)
                        yield Button("Reset Core", id="vf-reset-core", classes="green", compact=True)
                    with Horizontal(classes="row"):
                        yield Label("Mem Freq Min")
                        yield Input(value="0", id="vf-mem-min", compact=True)
                        yield Label("Max")
                        yield Input(value="0", id="vf-mem-max", compact=True)
                        yield Button("Lock Mem", id="vf-lock-mem", classes="red", compact=True)
                        yield Button("Reset Mem", id="vf-reset-mem", classes="green", compact=True)
                    yield PlotextPlot(id="vf-plot")
        with Horizontal(id="log-header"):
            yield Label("  Output")
            yield Button("Hide", id="toggle-log", compact=True)
            yield Button("Clear", id="clear-log", compact=True)
        with Vertical(id="log-panel"):
            yield Log(id="output-log", highlight=True)
        yield Footer()

    def on_mount(self) -> None:
        self._write_log("NVOC-TUI started.")
        self._update_metrics()
        self._clear_vf_plot("No VF curve cache loaded.")
        self._refresh_gpu_list()
        self._set_poll_timer(self.config_data.dashboard.refresh_interval)
        self._set_vf_poll_timer(self.config_data.vfcurve.auto_refresh)

    def _set_poll_timer(self, interval: float) -> None:
        interval = max(0.2, min(interval, 60.0))
        self.config_data.dashboard.refresh_interval = interval
        self.config_store.data = self.config_data
        self.config_store.save()
        if self.poll_timer is not None:
            self.poll_timer.stop()
        self.poll_timer = self.set_interval(interval, self._dashboard_tick, pause=False)

    def _dashboard_tick(self) -> None:
        if self.cli_service.action_state.running:
            return
        self._run_query("status", self.gpu_args() + ["-O", "json", "status", "-a"], self._on_status_loaded)

    def _vf_auto_refresh_label(self) -> str:
        return "Auto Refresh: On" if self.config_data.vfcurve.auto_refresh else "Auto Refresh: Off"

    def _set_vf_poll_timer(self, enabled: bool) -> None:
        self.config_data.vfcurve.auto_refresh = enabled
        self.config_store.data = self.config_data
        self.config_store.save()
        if self.vf_poll_timer is not None:
            self.vf_poll_timer.stop()
            self.vf_poll_timer = None
        if enabled:
            self.vf_poll_timer = self.set_interval(2.0, self._vf_curve_tick, pause=False)
        try:
            self.query_one("#vf-auto-refresh", Button).label = self._vf_auto_refresh_label()
        except Exception:
            pass
        if enabled and not self.cli_service.action_state.running and not self.vf_refresh_inflight:
            self._refresh_vf_curve()

    def _vf_curve_tick(self) -> None:
        if self.cli_service.action_state.running or self.vf_refresh_inflight:
            return
        self._refresh_vf_curve()

    def _selected_gpu_idx(self) -> int | None:
        try:
            value = self.query_one("#gpu-select", Select).value
        except Exception:
            return None
        if value in (None, Select.BLANK):
            return None
        return int(value)

    def gpu_args(self) -> list[str]:
        idx = self._selected_gpu_idx()
        return [f"--gpu={idx}"] if idx is not None and idx >= 0 else []

    def _current_gpu(self) -> GpuDescriptor | None:
        idx = self._selected_gpu_idx()
        for gpu in self.gpus:
            if gpu.index == idx:
                return gpu
        return None

    def _vf_cache_path(self) -> Path:
        cache_dir = self.root_dir / "vfp_cache"
        cache_dir.mkdir(exist_ok=True)
        gpu = self._current_gpu()
        if gpu and gpu.uuid:
            return cache_dir / f"{gpu.uuid}.csv"
        idx = self._selected_gpu_idx() or 0
        return cache_dir / f"gpu_{idx}.csv"

    def _write_log(self, text: str) -> None:
        log = self.query_one("#output-log", Log)
        for line in text.rstrip("\n").splitlines() or [""]:
            log.write_line(line)

    def _append_threadsafe(self, text: str, _level: str = "info") -> None:
        self.call_from_thread(self._write_log, text)

    def _action_finished(self, code: int) -> None:
        self.call_from_thread(self._after_action, code)

    def _after_action(self, code: int) -> None:
        if code >= 0:
            self._refresh_all_state()

    def _run_action(self, args: list[str]) -> None:
        if not self.config_data.cli.exe_path:
            self._write_log("CLI executable not configured.")
            return
        started = self.cli_service.run_action(self.config_data.cli, args, self._append_threadsafe, self._action_finished)
        if not started:
            self._write_log("Another action is already running.")

    def _run_action_chain(self, commands: list[list[str]]) -> None:
        queue = list(commands)

        def start_next(_code: int = 0) -> None:
            if not queue:
                self._refresh_all_state()
                return
            next_args = queue.pop(0)
            started = self.cli_service.run_action(
                self.config_data.cli,
                next_args,
                self._append_threadsafe,
                lambda code: self.call_from_thread(start_next, code),
            )
            if not started:
                self._write_log("Another action is already running.")

        start_next()

    def _run_query(self, command_name: str, args: list[str], callback) -> None:
        def worker() -> None:
            code, output, parsed = self.cli_service.run_query(self.config_data.cli, args, command_name)
            self.call_from_thread(callback, code, output, parsed)

        threading.Thread(target=worker, daemon=True, name=f"query-{command_name}").start()

    def _refresh_gpu_list(self) -> None:
        def worker() -> None:
            code, output, gpus = self.cli_service.list_gpus(self.config_data.cli)
            self.call_from_thread(self._on_gpu_list_loaded, code, output, gpus)

        threading.Thread(target=worker, daemon=True, name="gpu-list").start()

    def _on_gpu_list_loaded(self, code: int, output: str, gpus: list[GpuDescriptor]) -> None:
        self._write_log(output or "GPU detection finished.")
        self.gpus = gpus
        select = self.query_one("#gpu-select", Select)
        if not gpus:
            select.set_options([("(no GPUs found)", "-1")])
            select.value = "-1"
            return
        select.set_options([(gpu.long_label, str(gpu.index)) for gpu in gpus])
        target = self.config_data.last_gpu_idx
        if target is None or all(gpu.index != target for gpu in gpus):
            target = gpus[0].index
        select.value = str(target)
        self.config_data.last_gpu_idx = target
        self.config_store.data = self.config_data
        self.config_store.save()
        self._refresh_all_state()

    def _refresh_all_state(self) -> None:
        if not self.gpu_args():
            self._update_metrics()
            return
        self._run_query("info", self.gpu_args() + ["-O", "json", "info"], self._on_info_loaded)
        self._run_query("status", self.gpu_args() + ["-O", "json", "status", "-a"], self._on_status_loaded)
        self._run_query("get", self.gpu_args() + ["-O", "json", "get"], self._on_get_loaded)

    def _on_info_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0 and output:
            self._write_log(output)
        if code != 0 and not parsed:
            return
        self.cache.info = parsed
        self._update_metrics()
        self._prime_overclock_inputs()

    def _on_status_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0 and output:
            self._write_log(output)
        if code != 0 and not parsed:
            return
        self.cache.status = parsed
        self._update_metrics()
        if self.cache.vf_curve_path:
            self._render_vf_plot()

    def _on_get_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0:
            self._write_log(output)
            return
        self.cache.settings = parsed
        self._prime_overclock_inputs()

    def _prime_overclock_inputs(self) -> None:
        fields = {
            "#core-offset": str(self.cache.settings.get("core_clock_current", self.cache.info.get("core_clock_min", 0))),
            "#mem-offset": str(self.cache.settings.get("mem_clock_current", self.cache.info.get("mem_clock_min", 0))),
            "#power-limit": str(self.cache.settings.get("power_limit_current", self.cache.info.get("power_limit_default", 100))),
            "#thermal-limit": str(self.cache.info.get("thermal_limit_default", 83)),
            "#voltage-boost": str(self.cache.settings.get("voltage_boost_current", 0)),
        }
        for selector, value in fields.items():
            try:
                self.query_one(selector, Input).value = value
            except Exception:
                pass

    def _update_metrics(self) -> None:
        info = self.cache.info
        status = self.cache.status
        architecture = info.get("arch") or info.get("codename") or "---"
        lines = [
            f"GPU: {status.get('gpu_clock_mhz', '---')} MHz",
            f"MEM: {status.get('mem_clock_mhz', '---')} MHz",
            f"VOLT: {status.get('voltage_mv', '---')} mV",
            f"TEMP: {status.get('temperature_c', '---')} C",
            f"PWR: {status.get('power_w', '---')} W",
            f"ARCH: {architecture}",
        ]
        self.query_one("#metrics", Static).update("\n".join(lines))

    def _save_cli_path(self) -> None:
        path = self.query_one("#cli-path", Input).value.strip()
        discovered = CliService.discover_cli(path)
        self.config_data.cli = discovered if discovered.exe_path else self.config_data.cli.__class__(exe_path=path)
        if self.config_data.cli.exe_path and not self.config_data.cli.cwd:
            self.config_data.cli.cwd = str(Path(self.config_data.cli.exe_path).resolve().parent)
        self.config_store.data = self.config_data
        self.config_store.save()
        self._write_log(f"CLI path set to: {self.config_data.cli.exe_path or path}")

    def _refresh_vf_curve(self) -> None:
        if self.vf_refresh_inflight:
            return
        cache_path = self._vf_cache_path()
        self.vf_refresh_inflight = True

        def worker() -> None:
            code, output, _ = self.cli_service.run_query(
                self.config_data.cli,
                self.gpu_args() + ["set", "vfp", "export", "-q", str(cache_path)],
                "",
            )
            self.call_from_thread(self._on_vf_curve_loaded, output, str(cache_path), code)

        threading.Thread(target=worker, daemon=True, name="vf-refresh").start()

    def _on_vf_curve_loaded(self, output: str, path: str, code: int) -> None:
        self.vf_refresh_inflight = False
        if output:
            self._write_log(output)
        self.cache.vf_curve_path = path
        if code == 0:
            self._render_vf_plot()
        else:
            self._clear_vf_plot("VF curve export failed.")

    def _clear_vf_plot(self, title: str) -> None:
        widget = self.query_one("#vf-plot", PlotextPlot)
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        plt.title(title)
        plt.xlabel("mV")
        plt.ylabel("MHz")
        widget.refresh()

    def _render_vf_plot(self) -> None:
        if not self.cache.vf_curve_path:
            self._clear_vf_plot("No VF curve cache loaded.")
            return
        voltages, freqs, defaults = load_vf_curve(self.cache.vf_curve_path)
        if not voltages:
            self._clear_vf_plot("VF curve cache is empty.")
            return
        widget = self.query_one("#vf-plot", PlotextPlot)
        plt = widget.plt
        plt.clear_figure()
        plt.clear_data()
        plt.clear_color()
        plt.plot(voltages, freqs, marker="braille", color="cyan+", label="Current")
        plt.scatter(voltages, defaults, marker="braille", color="white", label="Default")
        live_voltage = self.cache.status.get("voltage_mv")
        live_clock = self.cache.status.get("gpu_clock_mhz")
        if isinstance(live_voltage, (int, float)) and isinstance(live_clock, (int, float)):
            live_label = "Lock Point" if self.cache.status.get("voltage_locked") else "Live Point"
            plt.vline(float(live_voltage), color="yellow+")
            plt.hline(float(live_clock), color="yellow+")
            plt.scatter([float(live_voltage)], [float(live_clock)], marker="braille", color="yellow+", label=live_label)
        working_point = find_curve_point_for_voltage(
            voltages,
            freqs,
            float(live_voltage) if isinstance(live_voltage, (int, float)) else None,
        )
        if working_point is not None:
            plt.vline(working_point[0], color="green+")
            plt.hline(working_point[1], color="green+")
            plt.scatter(
                [working_point[0]],
                [working_point[1]],
                marker="braille",
                color="green+",
                label="Working VFP",
            )
        plt.ylim(0, max(max(freqs), max(defaults)))
        plt.title("VF Curve")
        plt.xlabel("mV")
        plt.ylabel("MHz")
        widget.refresh()

    def _get_int(self, widget_id: str, default: int = 0) -> int:
        try:
            return int(self.query_one(widget_id, Input).value.strip())
        except ValueError:
            return default

    def on_select_changed(self, event: Select.Changed) -> None:
        if event.select.id == "gpu-select":
            if event.value not in (None, Select.BLANK):
                self.config_data.last_gpu_idx = int(event.value)
                self.config_store.data = self.config_data
                self.config_store.save()
                self._refresh_all_state()

    def on_resize(self, event) -> None:
        del event
        self._render_vf_plot()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        button_id = event.button.id or ""
        if button_id == "detect-gpus":
            self._refresh_gpu_list()
        elif button_id == "save-cli":
            self._save_cli_path()
        elif button_id == "refresh-all":
            self._refresh_all_state()
        elif button_id == "dashboard-interval-apply":
            try:
                value = float(self.query_one("#dashboard-interval", Input).value.strip())
            except ValueError:
                value = 1.0
            self._set_poll_timer(value)
        elif button_id == "dashboard-pause":
            if self.poll_timer and self.poll_timer.pause:
                self.poll_timer.resume()
                event.button.label = "Pause"
            elif self.poll_timer:
                self.poll_timer.pause()
                event.button.label = "Resume"
        elif button_id == "dashboard-now":
            self._dashboard_tick()
        elif button_id == "dashboard-info":
            self._run_action(self.gpu_args() + ["info"])
        elif button_id == "dashboard-status":
            self._run_action(self.gpu_args() + ["status", "-a"])
        elif button_id == "dashboard-get":
            self._run_action(self.gpu_args() + ["get"])
        elif button_id == "autoscan-export-init":
            self._sync_autoscan_from_ui()
            self._run_action_chain(
                [
                    self.gpu_args() + ["set", "nvml", "--core-offset", "0"],
                    self.gpu_args() + ["set", "vfp", "export", "-q", "-"],
                ]
            )
        elif button_id == "autoscan-reset-unlock":
            self._sync_autoscan_from_ui()
            self._run_action_chain(
                [
                    self.gpu_args() + ["set", "nvapi", "--reset-volt-locks"],
                    self.gpu_args() + ["reset", "vfp"],
                ]
            )
        elif button_id == "autoscan-start":
            self._sync_autoscan_from_ui()
            self._run_action(self._autoscan_args())
        elif button_id == "autoscan-stop":
            self.cli_service.cancel_action()
        elif button_id == "autoscan-fix":
            self._sync_autoscan_from_ui()
            args = self.gpu_args() + ["set", "vfp", "fix_result", "-m", "1"]
            if self.config_data.autoscan.mode == "ultrafast":
                args.append("-u")
            self._run_action(args)
        elif button_id == "autoscan-import-final":
            self._run_action(self.gpu_args() + ["set", "vfp", "import", r".\ws\vfp.csv"])
        elif button_id == "autoscan-export-final":
            self._run_action(self.gpu_args() + ["set", "vfp", "export", r".\ws\vfp-final.csv"])
        elif button_id == "oc-apply":
            self._run_action(self._oc_args())
        elif button_id == "oc-reset":
            backend = self.query_one("#oc-api", Select).value or "nvapi"
            self._run_action_chain(
                [
                    self.gpu_args() + ["set", str(backend), "--core-offset", "0"],
                    self.gpu_args() + ["set", str(backend), "--mem-offset", "0"],
                ]
            )
        elif button_id == "limits-apply":
            self._run_action(self._limit_args())
        elif button_id == "reset-all":
            self._run_action(self.gpu_args() + ["reset"])
        elif button_id == "fan-apply":
            self._run_action(self._fan_args(reset=False))
        elif button_id == "fan-reset":
            self._run_action(self._fan_args(reset=True))
        elif button_id == "vf-refresh":
            self._sync_vfcurve_from_ui()
            self._refresh_vf_curve()
        elif button_id == "vf-auto-refresh":
            self._sync_vfcurve_from_ui()
            self._set_vf_poll_timer(not self.config_data.vfcurve.auto_refresh)
        elif button_id == "vf-export":
            self._sync_vfcurve_from_ui()
            path = self.query_one("#vf-path", Input).value.strip()
            args = self.gpu_args() + ["set", "vfp", "export", path]
            if self.query_one("#vf-quick-export", Checkbox).value:
                args.append("-q")
            self._run_action(args)
        elif button_id == "vf-import":
            self._sync_vfcurve_from_ui()
            path = self.query_one("#vf-path", Input).value.strip()
            self._run_action(self.gpu_args() + ["set", "vfp", "import", path])
        elif button_id == "vf-reset":
            self._run_action(self.gpu_args() + ["reset", "vfp"])
        elif button_id == "vf-unlock":
            self._run_action(self.gpu_args() + ["set", "nvapi", "--reset-volt-locks"])
        elif button_id == "vf-apply-adj":
            start = self._get_int("#vf-range-start")
            end = self._get_int("#vf-range-end")
            delta = self._get_int("#vf-delta")
            if start > end:
                start, end = end, start
            self._run_action(
                self.gpu_args()
                + ["set", "vfp", "pointwiseoc", f"{start}-{end}", f"{delta * 1000:+d}"]
            )
        elif button_id == "vf-lock-voltage":
            value = self.query_one("#vf-lock-value", Input).value.strip()
            if self.query_one("#vf-lock-as-mv", Checkbox).value:
                value = f"{value}mV"
            self._run_action(self.gpu_args() + ["set", "nvapi", "--locked-voltage", value])
        elif button_id == "vf-lock-core":
            backend = str(self.query_one("#vf-freq-api", Select).value or "nvml")
            self._run_action(
                self.gpu_args()
                + [
                    "set",
                    backend,
                    "--locked-core-clocks",
                    str(self._get_int("#vf-core-min")),
                    str(self._get_int("#vf-core-max")),
                ]
            )
        elif button_id == "vf-reset-core":
            backend = str(self.query_one("#vf-freq-api", Select).value or "nvml")
            self._run_action(self.gpu_args() + ["set", backend, "--reset-core-clocks"])
        elif button_id == "vf-lock-mem":
            backend = str(self.query_one("#vf-freq-api", Select).value or "nvml")
            self._run_action(
                self.gpu_args()
                + [
                    "set",
                    backend,
                    "--locked-mem-clocks",
                    str(self._get_int("#vf-mem-min")),
                    str(self._get_int("#vf-mem-max")),
                ]
            )
        elif button_id == "vf-reset-mem":
            backend = str(self.query_one("#vf-freq-api", Select).value or "nvml")
            self._run_action(self.gpu_args() + ["set", backend, "--reset-mem-clocks"])
        elif button_id == "toggle-log":
            panel = self.query_one("#log-panel", Vertical)
            hidden = panel.has_class("hidden")
            if hidden:
                panel.remove_class("hidden")
                event.button.label = "Hide"
            else:
                panel.add_class("hidden")
                event.button.label = "Show"
            self.config_data.ui.log_expanded = hidden
            self.config_store.data = self.config_data
            self.config_store.save()
        elif button_id == "clear-log":
            self.query_one("#output-log", Log).clear()

    def _sync_autoscan_from_ui(self) -> None:
        self.config_data.autoscan.mode = str(self.query_one("#autoscan-mode", Select).value or "standard")
        self.config_data.autoscan.bsod_recovery = str(self.query_one("#autoscan-bsod", Select).value or "")
        mapping = {
            "test_exe": "#autoscan-test-exe",
            "score_path": "#autoscan-score-path",
            "score_threshold": "#autoscan-score",
            "timeout_loops": "#autoscan-timeout",
            "log_file": "#autoscan-log",
            "output_csv": "#autoscan-output",
            "init_csv": "#autoscan-init",
        }
        for field, selector in mapping.items():
            setattr(self.config_data.autoscan, field, self.query_one(selector, Input).value.strip())
        self.config_store.data = self.config_data
        self.config_store.save()

    def _sync_vfcurve_from_ui(self) -> None:
        self.config_data.vfcurve.default_path = self.query_one("#vf-path", Input).value.strip()
        self.config_data.vfcurve.quick_export = self.query_one("#vf-quick-export", Checkbox).value
        self.config_store.data = self.config_data
        self.config_store.save()

    def _autoscan_args(self) -> list[str]:
        data = self.config_data.autoscan
        if data.mode == "legacy":
            args = self.gpu_args() + ["set", "vfp", "autoscan_legacy"]
        else:
            args = self.gpu_args() + ["set", "vfp", "autoscan"]
            if data.mode == "ultrafast":
                args.append("-u")
            args += ["-o", data.output_csv, "-i", data.init_csv]
        args += ["-w", data.test_exe, "-l", data.log_file, "-x", data.score_path, "-z", data.score_threshold, "-t", data.timeout_loops]
        if data.bsod_recovery:
            args += ["-b", data.bsod_recovery]
        return args

    def _oc_args(self) -> list[str]:
        backend = str(self.query_one("#oc-api", Select).value or "nvapi")
        args = self.gpu_args() + ["set", backend]
        args += ["--core-offset", str(self._get_int("#core-offset"))]
        args += ["--mem-offset", str(self._get_int("#mem-offset"))]
        pstart = self.query_one("#pstate-start", Input).value.strip().lower()
        pend = self.query_one("#pstate-end", Input).value.strip().lower()
        if pstart and pend:
            args += ["--pstate-lock", pstart, pend]
        return args

    def _limit_args(self) -> list[str]:
        backend = str(self.query_one("#power-api", Select).value or "nvapi")
        args = self.gpu_args() + ["set", backend]
        args += ["--power-limit", str(self._get_int("#power-limit"))]
        if backend == "nvapi":
            args += ["--thermal-limit", str(self._get_int("#thermal-limit"))]
            args += ["--voltage-boost", str(self._get_int("#voltage-boost"))]
        return args

    def _fan_args(self, reset: bool) -> list[str]:
        backend = "nvml-cooler" if str(self.query_one("#fan-api", Select).value or "nvapi") == "nvml" else "nvapi-cooler"
        args = self.gpu_args() + ["set", backend]
        fan_id = str(self.query_one("#fan-id", Select).value or "all")
        if fan_id != "all":
            args += ["--id", fan_id]
        if reset:
            args += ["--policy", "auto", "--level", "0"]
        else:
            args += [
                "--policy",
                str(self.query_one("#fan-policy", Select).value or "continuous"),
                "--level",
                str(self._get_int("#fan-level", 60)),
            ]
        return args
