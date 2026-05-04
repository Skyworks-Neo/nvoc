from __future__ import annotations

from textual.containers import Vertical
from textual.widgets import Button, Log

from .base import PaneController


class ConsoleController(PaneController):
    def write_log(self, text: str) -> None:
        log = self.app.query_one("#output-log", Log)
        for line in text.rstrip("\n").splitlines() or [""]:
            log.write_line(line)
            log.scroll_end()

    def focus_output(self) -> None:
        panel = self.app.query_one("#log-panel", Vertical)
        if panel.has_class("hidden"):
            panel.remove_class("hidden")
            self.app.query_one("#toggle-log", Button).label = "Hide"
            self.app.config_data.ui.log_expanded = True
            self.app.save_config()
        self.app.query_one("#output-log", Log).focus()

    def handle_button(self, button: Button, button_id: str) -> bool:
        if button_id == "toggle-log":
            panel = self.app.query_one("#log-panel", Vertical)
            hidden = panel.has_class("hidden")
            if hidden:
                panel.remove_class("hidden")
                button.label = "Hide"
            else:
                panel.add_class("hidden")
                button.label = "Show"
            self.app.config_data.ui.log_expanded = hidden
            self.app.save_config()
            return True
        if button_id == "clear-log":
            self.app.query_one("#output-log", Log).clear()
            return True
        return False
