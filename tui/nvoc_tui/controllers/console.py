from __future__ import annotations

from textual.containers import Vertical
from textual.widgets import Button, Log

from .base import PaneController


class ConsoleController(PaneController):
    HIDE_LABEL = "Hide (^t)"
    SHOW_LABEL = "Show (^t)"

    def write_log(self, text: str) -> None:
        log = self.app.query_one("#output-log", Log)
        for line in text.rstrip("\n").splitlines() or [""]:
            log.write_line(line)
            log.scroll_end()

    def focus_output(self) -> None:
        panel = self.app.query_one("#log-panel", Vertical)
        if panel.has_class("hidden"):
            panel.remove_class("hidden")
            self.app.query_one("#toggle-log", Button).label = self.HIDE_LABEL
            self.app.config_data.ui.log_expanded = True
            self.app.save_config()
        self.app.query_one("#output-log", Log).focus()

    def toggle_output(self) -> None:
        panel = self.app.query_one("#log-panel", Vertical)
        button = self.app.query_one("#toggle-log", Button)
        hidden = panel.has_class("hidden")
        if hidden:
            panel.remove_class("hidden")
            button.label = self.HIDE_LABEL
        else:
            panel.add_class("hidden")
            button.label = self.SHOW_LABEL
        self.app.config_data.ui.log_expanded = hidden
        self.app.save_config()

    def clear_output(self) -> None:
        self.app.query_one("#output-log", Log).clear()

    def handle_button(self, button: Button, button_id: str) -> bool:
        if button_id == "toggle-log":
            self.toggle_output()
            return True
        if button_id == "clear-log":
            self.clear_output()
            return True
        return False
