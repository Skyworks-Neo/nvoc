from __future__ import annotations

from src.task_runner import GuiTaskRunner


def test_task_runner_executes_background_work() -> None:
    runner = GuiTaskRunner(max_workers=1)
    try:
        future = runner.submit("unit", lambda: 42)
        assert future.result(timeout=1) == 42
    finally:
        runner.shutdown()
