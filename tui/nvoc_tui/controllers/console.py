from __future__ import annotations

from textual.containers import Vertical
from textual.widgets import Button, Log

from .base import PaneController


class ConsoleController(PaneController):
    HIDE_LABEL = "Hide (^t)"
    SHOW_LABEL = "Show (^t)"
    MAXIMIZE_LABEL = "Max (^x)"
    RESTORE_LABEL = "Restore (^x)"

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
            self.set_maximized(False)
            panel.add_class("hidden")
            button.label = self.SHOW_LABEL
        self.app.config_data.ui.log_expanded = hidden
        self.app.save_config()

    def toggle_output_maximized(self) -> None:
        panel = self.app.query_one("#log-panel", Vertical)
        if panel.has_class("hidden"):
            panel.remove_class("hidden")
            self.app.query_one("#toggle-log", Button).label = self.HIDE_LABEL
            self.app.config_data.ui.log_expanded = True
            self.app.save_config()
        self.set_maximized(not self.is_maximized())
        if self.is_maximized():
            self.app.query_one("#output-log", Log).focus()

    def is_maximized(self) -> bool:
        return self.app.has_class("output-maximized")

    def set_maximized(self, maximized: bool) -> None:
        self.app.set_class(maximized, "output-maximized")
        self.app.query_one("#maximize-log", Button).label = (
            self.RESTORE_LABEL if maximized else self.MAXIMIZE_LABEL
        )

    def clear_output(self) -> None:
        self.app.query_one("#output-log", Log).clear()

    def handle_button(self, button: Button, button_id: str) -> bool:
        if button_id == "toggle-log":
            self.toggle_output()
            return True
        if button_id == "maximize-log":
            self.toggle_output_maximized()
            return True
        if button_id == "clear-log":
            self.clear_output()
            return True
        return False
