from __future__ import annotations

from src import cli_runner
from src.cli_runner import CLIRunner


class QueuedJob:
    def __init__(self, task) -> None:
        self.task = task
        self.cancelled = False

    def cancel(self) -> bool:
        self.cancelled = True
        return True


def test_cli_runner_rejects_second_command_while_first_is_pending() -> None:
    queued: list[QueuedJob] = []
    output: list[str] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    runner = CLIRunner("nvoc-autooptimizer", output.append, submit=submit)

    runner.run(["first"])
    runner.run(["second"])

    assert len(queued) == 1
    assert runner.is_running
    assert any("already running" in message for message in output)


def test_cli_runner_cancel_pending_job_invokes_callback() -> None:
    queued: list[QueuedJob] = []
    finished: list[int] = []
    output: list[str] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    runner = CLIRunner("nvoc-autooptimizer", output.append, submit=submit)

    runner.run(["first"], on_finished=finished.append)
    runner.cancel()

    assert queued[0].cancelled
    assert not runner.is_running
    assert finished == [-1]
    assert any("cancelled" in message for message in output)


def test_cli_runner_captures_per_run_callback(monkeypatch) -> None:
    queued: list[QueuedJob] = []
    finished: list[str] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    class FakeStdout:
        def __init__(self) -> None:
            self._lines = iter(["line\n", ""])

        def readline(self) -> str:
            return next(self._lines)

        def close(self) -> None:
            return

    class FakeProcess:
        def __init__(self, *_args, **_kwargs) -> None:
            self.stdout = FakeStdout()

        def poll(self):
            return None

        def wait(self) -> int:
            return 7

        def terminate(self) -> None:
            return

    monkeypatch.setattr(cli_runner.subprocess, "Popen", FakeProcess)
    runner = CLIRunner("nvoc-autooptimizer", lambda _text: None, submit=submit)

    runner.run(["first"], on_finished=lambda code: finished.append(f"first:{code}"))
    runner.on_finished = lambda code: finished.append(f"default:{code}")
    queued[0].task()

    assert finished == ["first:7"]
    assert not runner.is_running
