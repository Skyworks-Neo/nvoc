from __future__ import annotations

import os
import subprocess

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


def test_cli_runner_cancel_schedules_process_wait_off_caller_thread() -> None:
    queued: list[QueuedJob] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    class FakeProcess:
        def __init__(self) -> None:
            self.terminated = 0
            self.waited = 0

        def terminate(self) -> None:
            self.terminated += 1

        def wait(self, timeout: int) -> int:
            assert timeout == 3
            self.waited += 1
            return 0

    process = FakeProcess()
    runner = CLIRunner("nvoc-autooptimizer", lambda _text: None, submit=submit)
    runner._process = process
    runner._busy = True

    runner.cancel()

    assert process.terminated == 0
    assert process.waited == 0
    assert len(queued) == 1
    assert runner._cancel_inflight is True

    queued[0].task()

    assert process.terminated == 1
    assert process.waited == 1
    assert runner._cancel_inflight is False


def test_cli_runner_reuses_one_cancel_job_per_process() -> None:
    queued: list[QueuedJob] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    process = type(
        "FakeProcess",
        (),
        {"terminate": lambda self: None, "wait": lambda self, timeout: 0},
    )()
    runner = CLIRunner("nvoc-autooptimizer", lambda _text: None, submit=submit)
    runner._process = process
    runner._busy = True

    runner.cancel()
    runner.cancel()

    assert len(queued) == 1


def test_cli_runner_shutdown_cancels_synchronously(monkeypatch) -> None:
    process = type(
        "FakeProcess",
        (),
        {"terminate": lambda self: None, "wait": lambda self, timeout: 0},
    )()
    runner = CLIRunner("nvoc-autooptimizer", lambda _text: None)
    runner._process = process
    waits: list[bool] = []
    original_cancel = runner.cancel

    def record_cancel(wait: bool = False) -> None:
        waits.append(wait)
        original_cancel(wait=wait)

    monkeypatch.setattr(runner, "cancel", record_cancel)

    runner.shutdown()

    assert waits == [True]


def test_cli_runner_exe_override_streams_output_tap(monkeypatch) -> None:
    """Per-run exe override (VF Curve stressor panel) drives cmd and cwd."""
    queued: list[QueuedJob] = []
    console: list[str] = []
    tapped: list[str] = []

    def submit(_name, task):
        job = QueuedJob(task)
        queued.append(job)
        return job

    recorded: dict = {}

    class FakeStdout:
        def __init__(self) -> None:
            self._lines = iter(['VERDICT_JSON: {"result": "pass"}\n', ""])

        def readline(self) -> str:
            return next(self._lines)

        def close(self) -> None:
            return

    class FakeProcess:
        def __init__(self, cmd, **kwargs) -> None:
            recorded["cmd"] = cmd
            recorded["cwd"] = kwargs.get("cwd")
            self.stdout = FakeStdout()

        def poll(self):
            return 0

        def wait(self) -> int:
            return 0

        def terminate(self) -> None:
            return

    monkeypatch.setattr(cli_runner.subprocess, "Popen", FakeProcess)
    runner = CLIRunner("nvoc-autooptimizer", console.append, submit=submit)
    stressor = os.path.join("C:", "tools", "cli-stressor-cuda-rs.exe")

    runner.run(["--duration", "5"], exe=stressor, on_output=tapped.append)
    queued[0].task()

    assert recorded["cmd"] == [stressor, "--duration", "5"]
    assert recorded["cwd"] == os.path.dirname(stressor)
    assert tapped == ['VERDICT_JSON: {"result": "pass"}\n']
    assert any(stressor in message for message in console)
    assert not runner.is_running


def test_cli_runner_reports_missing_override_executable(monkeypatch) -> None:
    console: list[str] = []
    finished: list[int] = []

    def raising_popen(*_args, **_kwargs):
        raise FileNotFoundError()

    monkeypatch.setattr(cli_runner.subprocess, "Popen", raising_popen)
    runner = CLIRunner("nvoc-autooptimizer", console.append)
    stressor = os.path.join("C:", "tools", "cli-stressor-cuda-rs.exe")

    runner.run(["--duration", "5"], exe=stressor, on_finished=finished.append)

    assert any(stressor in message for message in console)
    assert finished == [-1]


def test_cli_runner_cancel_kills_process_after_timeout() -> None:
    class FakeProcess:
        def __init__(self) -> None:
            self.killed = False

        def terminate(self) -> None:
            return

        def wait(self, timeout: int) -> int:
            raise subprocess.TimeoutExpired("nvoc", timeout)

        def kill(self) -> None:
            self.killed = True

    process = FakeProcess()
    runner = CLIRunner("nvoc-autooptimizer", lambda _text: None)
    runner._process = process

    runner.cancel(wait=True)

    assert process.killed is True
