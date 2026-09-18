from concurrent.futures import Future
from types import SimpleNamespace
from unittest.mock import Mock

from src.tabs.vfcurve.sections.autoscan import AutoscanTab


class Config:
    def __init__(self):
        self.values = {}

    def get(self, key):
        return self.values.get(key)

    def set(self, key, value):
        self.values[key] = value


def panel():
    tab = AutoscanTab.__new__(AutoscanTab)
    tab._acting = False
    tab._refreshing = False
    tab._task_id = None
    tab._offset = 0
    tab._tasks = {}
    tab.app = SimpleNamespace(
        _exiting=False,
        gpu_map={"Selected": "0x100"},
        gpu_var=SimpleNamespace(get=lambda: "Selected"),
        config=Config(),
        console=Mock(),
        run_cli=Mock(side_effect=AssertionError("must not launch a CLI")),
    )
    tab.frame = Mock()
    tab.start_btn = Mock()
    tab.stop_btn = Mock()
    tab.status_var = Mock()
    tab.mode_var = SimpleNamespace(get=lambda: "Ultrafast")
    tab.task_menu = Mock()
    tab.task_var = Mock()
    return tab


def immediate(action, complete, failed=None):
    try:
        result = action()
    except Exception as exc:
        if failed:
            failed(exc)
    else:
        complete(result)


def test_scan_submission_goes_to_service_and_reuses_uncertain_request():
    tab = panel()
    tab._background = immediate
    tab._refresh = Mock()
    client = Mock()
    client.start.side_effect = [OSError("response lost"), {"id": 7}]
    tab._client = lambda: client
    tab._start_scan()
    pending = tab.app.config.get("hosted_scan_pending").copy()
    tab._start_scan()
    assert client.start.call_args_list[0] == client.start.call_args_list[1]
    assert pending["gpu_id"] == 256
    assert pending["mode"] == "ultrafast"
    assert tab._task_id == 7
    assert tab.app.config.get("hosted_scan_pending") is None
    tab.app.run_cli.assert_not_called()


def test_reopen_attaches_to_active_service_task_and_fetches_logs():
    tab = panel()
    tab._background = immediate
    task = {
        "id": 9,
        "origin": "automation",
        "request": {"gpu_id": 256},
        "state": "running",
        "error": None,
    }
    client = Mock()
    client.tasks.return_value = [task]
    client.control.return_value = {"manual_hold": [], "recovery_required": []}
    client.log.return_value = {"text": "progress\n", "next_offset": 9}
    tab._client = lambda: client
    tab._refresh()
    assert tab._task_id == 9
    tab._refresh()
    client.log.assert_called_once_with(9, 0)
    tab.app.console.append.assert_called_once_with("progress\n")
    assert tab._offset == 9


def test_cancel_targets_selected_task_not_current_gpu_selection():
    tab = panel()
    tab._task_id = 12
    tab._background = immediate
    tab._refresh = Mock()
    client = Mock()
    tab._client = lambda: client
    tab._stop_scan()
    client.cancel.assert_called_once_with(12)
    client.takeover.assert_not_called()


def test_close_detaches_without_cancel_and_does_not_touch_destroyed_widgets():
    tab = panel()
    tab.app._exiting = True
    tab._refresh = Mock()
    tab._poll()
    tab._refresh.assert_not_called()
    tab.frame.after.assert_not_called()
    future = Future()
    tab.app.run_background = Mock(return_value=future)
    completed = Mock()
    tab._background(lambda: {}, completed)
    callback = tab.frame.after.call_args.args[1]
    future.set_result({"id": 1})
    callback()
    completed.assert_not_called()
