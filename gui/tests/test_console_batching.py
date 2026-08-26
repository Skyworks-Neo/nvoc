from __future__ import annotations

import os
import threading
from unittest.mock import Mock, call

os.environ.setdefault("PYSTRAY_BACKEND", "dummy")

from src.app import App
from src.widgets.output_console import OutputConsole


def test_output_console_appends_batch_with_one_widget_update() -> None:
    console = OutputConsole.__new__(OutputConsole)
    console._lock = threading.Lock()
    console._expanded = True
    console.textbox = Mock()
    console.textbox.index.side_effect = ["1.0", "2.0", "3.0", "3.0"]

    console.append_batch(["Successfully applied\n", "Command failed\n"])

    assert console.textbox.configure.call_args_list == [
        call(state="normal"),
        call(state="disabled"),
    ]
    assert console.textbox.insert.call_args_list == [
        call("end", "Successfully applied\n"),
        call("end", "Command failed\n"),
    ]
    assert console.textbox.tag_add.call_args_list == [
        call("lime", "1.0", "2.0"),
        call("red", "2.0", "3.0"),
    ]
    console.textbox.see.assert_called_once_with("end")


def test_app_coalesces_cli_output_into_one_scheduled_flush() -> None:
    app = Mock()
    app._cli_output_buffer = []
    app._cli_output_flush_id = None
    app._cli_output_lock = threading.Lock()
    scheduled = []

    def after(delay_ms: int, callback) -> str:
        scheduled.append((delay_ms, callback))
        return "flush-1"

    app.after.side_effect = after
    app._flush_cli_output = lambda: App._flush_cli_output(app)

    App._on_cli_output(app, "first\n")
    App._on_cli_output(app, "second\n")

    assert len(scheduled) == 1
    assert scheduled[0][0] == 100
    app.console.append_batch.assert_not_called()

    scheduled[0][1]()

    app.console.append_batch.assert_called_once_with(["first\n", "second\n"])
    assert app._cli_output_buffer == []
    assert app._cli_output_flush_id is None
