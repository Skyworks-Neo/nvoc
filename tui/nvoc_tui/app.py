# Copyright (C) 2026 Ajax Dong
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
from __future__ import annotations

import threading

from textual import events
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Container
from textual.widgets import Button, Label, Select, TabbedContent

from .cli import CliService
from .config import ConfigStore
from .controllers.autoscan import AutoscanController
from .controllers.console import ConsoleController
from .controllers.dashboard import DashboardController
from .controllers.header import HeaderController
from .controllers.overclock import OverclockController
from .controllers.vfcurve import VFCurveController
from .models import AppConfig, GpuCache, GpuDescriptor, repo_root
from .panes.autoscan import compose_autoscan
from .panes.console import compose_console
from .panes.dashboard import compose_dashboard
from .panes.header import compose_header
from .panes.overclock import compose_overclock
from .panes.vfcurve import compose_vfcurve


class NVOCApp(App[None]):
    TITLE = "NVOC-TUI"
    MIN_WIDTH = 55
    MIN_HEIGHT = 24
    TAB_IDS = ("dashboard", "autoscan", "overclock", "vfcurve")
    TAB_FIRST_FOCUS = {
        "dashboard": "#dashboard-interval",
        "autoscan": "#autoscan-mode",
        "overclock": "#oc-api",
        "vfcurve": "#vf-path",
    }
    CSS_PATH = [
        "styles/base.tcss",
        "styles/header.tcss",
        "styles/dashboard.tcss",
        "styles/autoscan.tcss",
        "styles/overclock.tcss",
        "styles/vfcurve.tcss",
        "styles/console.tcss",
    ]
    BINDINGS = [
        ("ctrl+c", "quit", "Quit"),
        ("ctrl+g", "focus_gpu_select", "GPU"),
        ("ctrl+o", "focus_output", "Output"),
        Binding("ctrl+t", "toggle_output", show=False),
        Binding("ctrl+e", "clear_output", show=False),
        Binding("alt+a", "pane_shortcut('a')", show=False),
        Binding("alt+b", "pane_shortcut('b')", show=False),
        Binding("alt+d", "pane_shortcut('d')", show=False),
        Binding("alt+p", "pane_shortcut('p')", show=False),
        Binding("alt+n", "pane_shortcut('n')", show=False),
        Binding("alt+i", "pane_shortcut('i')", show=False),
        Binding("alt+s", "pane_shortcut('s')", show=False),
        Binding("alt+t", "pane_shortcut('t')", show=False),
        Binding("alt+x", "pane_shortcut('x')", show=False),
        Binding("alt+g", "pane_shortcut('g')", show=False),
        Binding("alt+o", "pane_shortcut('o')", show=False),
        Binding("alt+c", "pane_shortcut('c')", show=False),
        Binding("alt+q", "pane_shortcut('q')", show=False),
        Binding("alt+r", "pane_shortcut('r')", show=False),
        Binding("alt+e", "pane_shortcut('e')", show=False),
        Binding("alt+v", "pane_shortcut('v')", show=False),
        Binding("alt+l", "pane_shortcut('l')", show=False),
        Binding("alt+u", "pane_shortcut('u')", show=False),
        Binding("alt+m", "pane_shortcut('m')", show=False),
        ("f1", "switch_tab(0)", "Dashboard"),
        ("f2", "switch_tab(1)", "Autoscan"),
        ("f3", "switch_tab(2)", "Overclock"),
        ("f4", "switch_tab(3)", "VF Curve"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self.animation_level = "none"
        self.root_dir = repo_root()
        self.config_store = ConfigStore(self.root_dir)
        self.config_data: AppConfig = self.config_store.load()
        discovered = CliService.discover_cli(self.config_data.cli.exe_path)
        if discovered.exe_path:
            self.config_data.cli = discovered
            self.save_config()
        self.cli_service = CliService(self.root_dir)
        self.gpus: list[GpuDescriptor] = []
        self.cache = GpuCache()

        self.header_controller = HeaderController(self)
        self.dashboard_controller = DashboardController(self)
        self.autoscan_controller = AutoscanController(self)
        self.overclock_controller = OverclockController(self)
        self.vfcurve_controller = VFCurveController(self)
        self.console_controller = ConsoleController(self)

    def compose(self) -> ComposeResult:
        yield from compose_header(self.config_data)
        with TabbedContent(
            initial=self.config_data.ui.active_tab or "dashboard", id="main-tabs"
        ):
            yield from compose_dashboard(self.config_data)
            yield from compose_autoscan(self.config_data)
            yield from compose_overclock()
            yield from compose_vfcurve(
                self.config_data, self.vfcurve_controller.auto_refresh_label()
            )
        yield from compose_console()
        with Container(id="small-terminal-layer"):
            yield Label(
                f"Please enlarge the terminal\nto at least {self.MIN_WIDTH}x{self.MIN_HEIGHT}.",
                id="small-terminal-message",
            )

    def on_mount(self) -> None:
        self.write_log("NVOC-TUI started.")
        self.dashboard_controller.update_metrics()
        self.vfcurve_controller.clear_plot("No VF curve cache loaded.")
        self.update_responsive_layout()
        self.refresh_gpu_list()
        self.dashboard_controller.set_poll_timer(
            self.config_data.dashboard.refresh_interval
        )
        self.vfcurve_controller.set_poll_timer(self.config_data.vfcurve.auto_refresh)

    def save_config(self) -> None:
        self.config_store.data = self.config_data
        self.config_store.save()

    def selected_gpu_idx(self) -> int | None:
        return self.header_controller.selected_gpu_idx()

    def gpu_args(self) -> list[str]:
        return self.header_controller.gpu_args()

    def current_gpu(self) -> GpuDescriptor | None:
        return self.header_controller.current_gpu()

    def write_log(self, text: str) -> None:
        self.console_controller.write_log(text)

    def append_threadsafe(self, text: str, _level: str = "info") -> None:
        self.call_from_thread(self.write_log, text)

    def action_finished(self, code: int) -> None:
        self.call_from_thread(self.after_action, code)

    def after_action(self, code: int) -> None:
        if code >= 0:
            self.refresh_all_state()

    def run_cli_action(self, args: list[str]) -> None:
        if not self.config_data.cli.exe_path:
            self.write_log("CLI executable not configured.")
            return
        started = self.cli_service.run_action(
            self.config_data.cli, args, self.append_threadsafe, self.action_finished
        )
        if not started:
            self.write_log("Another action is already running.")

    def run_action_chain(self, commands: list[list[str]]) -> None:
        queue = list(commands)

        def start_next(_code: int = 0) -> None:
            if not queue:
                self.refresh_all_state()
                return
            next_args = queue.pop(0)
            started = self.cli_service.run_action(
                self.config_data.cli,
                next_args,
                self.append_threadsafe,
                lambda code: self.call_from_thread(start_next, code),
            )
            if not started:
                self.write_log("Another action is already running.")

        start_next()

    def run_query(self, command_name: str, args: list[str], callback) -> None:
        def worker() -> None:
            code, output, parsed = self.cli_service.run_query(
                self.config_data.cli, args, command_name
            )
            self.call_from_thread(callback, code, output, parsed)

        threading.Thread(
            target=worker, daemon=True, name=f"query-{command_name}"
        ).start()

    def refresh_gpu_list(self) -> None:
        def worker() -> None:
            code, output, gpus = self.cli_service.list_gpus(self.config_data.cli)
            self.call_from_thread(
                self.header_controller.on_gpu_list_loaded, code, output, gpus
            )

        threading.Thread(target=worker, daemon=True, name="gpu-list").start()

    def focus_dashboard_tab_switcher(self) -> None:
        self.switch_to_tab("dashboard")
        try:
            self.query_one("#main-tabs Tabs").focus()
        except Exception:
            self.query_one("#dashboard-now", Button).focus()

    def action_switch_tab(self, index: int) -> None:
        if 0 <= index < len(self.TAB_IDS):
            self.switch_to_tab(self.TAB_IDS[index], focus_first=True)

    def action_focus_gpu_select(self) -> None:
        self.header_controller.focus_gpu_select()

    def action_focus_output(self) -> None:
        self.console_controller.focus_output()

    def action_toggle_output(self) -> None:
        self.console_controller.toggle_output()

    def action_clear_output(self) -> None:
        self.console_controller.clear_output()

    def action_pane_shortcut(self, key: str) -> bool:
        tabs = self.query_one("#main-tabs", TabbedContent)
        if tabs.active == "dashboard":
            dashboard_shortcuts = {
                "a": "dashboard-interval-apply",
                "p": "dashboard-pause",
                "r": "dashboard-now",
                "i": "dashboard-info",
                "s": "dashboard-status",
                "g": "dashboard-get",
            }
            if key in dashboard_shortcuts:
                self.dashboard_controller.activate_button(dashboard_shortcuts[key])
                return True
        elif tabs.active == "autoscan":
            autoscan_shortcuts = {
                "m": "autoscan-mode",
                "d": "autoscan-bsod",
                "v": "autoscan-export-init",
                "i": "autoscan-import-final",
                "e": "autoscan-export-final",
                "t": "autoscan-test-exe",
                "l": "autoscan-timeout",
                "g": "autoscan-log",
                "o": "autoscan-output",
                "n": "autoscan-init",
                "r": "autoscan-reset-unlock",
                "a": "autoscan-start",
                "s": "autoscan-stop",
                "x": "autoscan-fix",
            }
            if key in autoscan_shortcuts:
                self.autoscan_controller.activate_shortcut(autoscan_shortcuts[key])
                return True
        elif tabs.active == "overclock":
            overclock_shortcuts = {
                "c": "oc-api",
                "p": "power-api",
                "a": "fan-id",
            }
            if key in overclock_shortcuts:
                self.overclock_controller.activate_shortcut(overclock_shortcuts[key])
                return True
        elif tabs.active == "vfcurve":
            vfcurve_shortcuts = {
                "c": "vf-path",
                "q": "vf-quick-export",
                "s": "vf-refresh",
                "a": "vf-auto-refresh",
                "e": "vf-export",
                "i": "vf-import",
                "r": "vf-reset",
                "v": "vf-range-start",
                "l": "vf-lock-value",
                "u": "vf-freq-api",
                "m": "vf-mem-min",
            }
            if key in vfcurve_shortcuts:
                self.vfcurve_controller.activate_shortcut(vfcurve_shortcuts[key])
                return True
        return False

    def on_key(self, event: events.Key) -> None:
        if self.consume_alt_prefix_key(event.key):
            event.stop()
            event.prevent_default()

    def consume_alt_prefix_key(self, key: str) -> bool:
        if key.startswith("alt+"):
            shortcut_key = key.rpartition("+")[2].lower()
            if len(shortcut_key) == 1:
                return self.action_pane_shortcut(shortcut_key)
        return False

    def switch_to_tab(self, tab_id: str, *, focus_first: bool = False) -> None:
        tabs = self.query_one("#main-tabs", TabbedContent)
        tabs.active = tab_id
        self.config_data.ui.active_tab = tab_id
        self.save_config()
        if focus_first:
            self.focus_first_in_tab(tab_id)

    def focus_first_in_tab(self, tab_id: str) -> None:
        selector = self.TAB_FIRST_FOCUS.get(tab_id)
        if selector is None:
            return
        try:
            self.query_one(selector).focus()
        except Exception:
            self.query_one(f"#{tab_id}").focus()

    def refresh_all_state(self) -> None:
        if not self.gpu_args():
            self.dashboard_controller.update_metrics()
            return
        self.run_query(
            "info",
            self.gpu_args() + ["-O", "json", "info"],
            self.dashboard_controller.on_info_loaded,
        )
        self.run_query(
            "status",
            self.gpu_args() + ["-O", "json", "status", "-a"],
            self.dashboard_controller.on_status_loaded,
        )
        self.run_query(
            "get",
            self.gpu_args() + ["-O", "json", "get"],
            self.dashboard_controller.on_get_loaded,
        )

    def on_select_changed(self, event: Select.Changed) -> None:
        if event.select.id == "gpu-select":
            self.header_controller.on_gpu_selected(event.value)

    def on_resize(self, event) -> None:
        del event
        self.update_responsive_layout()
        self.vfcurve_controller.render_plot()

    def update_responsive_layout(self) -> None:
        self.set_class(self.size.width < 80, "compact")
        self.set_class(self.size.width < 100, "narrow")
        self.set_class(self.size.width >= 160, "wide")
        self.set_class(
            self.size.width < self.MIN_WIDTH or self.size.height < self.MIN_HEIGHT,
            "too-small",
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        button_id = event.button.id or ""
        if self.header_controller.handle_button(button_id):
            return
        if self.dashboard_controller.handle_button(event.button, button_id):
            return
        if self.autoscan_controller.handle_button(button_id):
            return
        if self.overclock_controller.handle_button(button_id):
            return
        if self.vfcurve_controller.handle_button(button_id):
            return
        self.console_controller.handle_button(event.button, button_id)
