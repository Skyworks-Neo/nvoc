"""
CLI Runner - Subprocess wrapper for nvoc-auto-optimizer CLI.
Runs commands in background threads and streams output to callbacks.
"""

import subprocess
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from threading import Lock
from typing import Callable, Dict, Optional, Sequence, Tuple


class CLIRunner:
    """Wraps nvoc-auto-optimizer.exe subprocess execution."""

    def __init__(
        self,
        exe_path: str,
        on_output: Callable[[str], None],
        on_finished: Optional[Callable[[int], None]] = None,
        submit: Optional[Callable[[str, Callable[[], None]], object]] = None,
    ):
        """
        Args:
            exe_path: Path to nvoc-auto-optimizer.exe
            on_output: Callback invoked with each line of stdout/stderr
            on_finished: Callback invoked with return code when process ends
        """
        self.exe_path = exe_path
        self.on_output = on_output
        self.on_finished = on_finished
        self._submit = submit
        self._fallback_executor = None  # type: Optional[ThreadPoolExecutor]
        self._process = None  # type: Optional[subprocess.Popen]
        self._thread = None  # type: Optional[object]
        self._cancelled = False
        self._busy = False
        self._current_on_finished = None  # type: Optional[Callable[[int], None]]
        self._lock = Lock()

    @staticmethod
    def _no_window_kwargs() -> Dict[str, int]:
        """Return subprocess kwargs for hiding the console on Windows only."""
        if sys.platform == "win32" and hasattr(subprocess, "CREATE_NO_WINDOW"):
            return {"creationflags": subprocess.CREATE_NO_WINDOW}
        return {}

    @property
    def is_running(self) -> bool:
        with self._lock:
            return self._busy

    def run(
        self,
        args: Sequence[str],
        cwd: Optional[str] = None,
        on_finished: Optional[Callable[[int], None]] = None,
    ) -> None:
        """
        Run the CLI with given arguments in a background thread.

        Args:
            args: Command-line arguments (without the exe path)
            cwd: Working directory (defaults to exe parent directory)
        """
        with self._lock:
            if self._busy:
                self.on_output(
                    "[GUI] A process is already running. Please wait or cancel it.\n"
                )
                return
            self._busy = True
            self._cancelled = False
            self._current_on_finished = (
                on_finished if on_finished is not None else self.on_finished
            )

        if cwd is None:
            cwd = os.path.dirname(self.exe_path) or "."

        def _complete(retcode: int) -> None:
            with self._lock:
                callback = self._current_on_finished
                self._process = None
                self._thread = None
                self._busy = False
                self._current_on_finished = None
            if callback:
                callback(retcode)

        def _worker() -> None:
            cmd = [self.exe_path] + args
            self.on_output(f"[GUI] > {' '.join(cmd)}\n")
            try:
                with self._lock:
                    cancelled_before_start = self._cancelled
                if cancelled_before_start:
                    self.on_output("[GUI] Process cancelled.\n")
                    _complete(-1)
                    return
                process = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    cwd=cwd,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    bufsize=1,
                    **self._no_window_kwargs(),
                )
                with self._lock:
                    self._process = process
                if process.stdout is not None:
                    for line in iter(process.stdout.readline, ""):
                        if self._cancelled:
                            break
                        self.on_output(line)
                    process.stdout.close()
                retcode = process.wait()
                if self._cancelled:
                    self.on_output("[GUI] Process cancelled.\n")
                else:
                    self.on_output(f"[GUI] Process exited with code {retcode}\n")
                _complete(retcode)
            except FileNotFoundError:
                self.on_output(
                    f"[GUI] ERROR: CLI executable not found: {self.exe_path}\n"
                )
                _complete(-1)
            except Exception as e:
                self.on_output(f"[GUI] ERROR: {e}\n")
                _complete(-1)

        if self._submit is not None:
            try:
                self._thread = self._submit("cli-runner", _worker)
            except Exception as exc:
                self.on_output(f"[GUI] ERROR: failed to schedule CLI command: {exc}\n")
                _complete(-1)
        else:
            if self._fallback_executor is None:
                self._fallback_executor = ThreadPoolExecutor(
                    max_workers=1, thread_name_prefix="nvoc-gui-cli"
                )
            self._thread = self._fallback_executor.submit(_worker)

    def run_sync(
        self, args: Sequence[str], cwd: Optional[str] = None
    ) -> Tuple[int, str]:
        """
        Run the CLI synchronously and return (returncode, output).
        """
        if cwd is None:
            cwd = os.path.dirname(self.exe_path) or "."

        cmd = [self.exe_path] + args
        try:
            result = subprocess.run(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                cwd=cwd,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
                **self._no_window_kwargs(),
            )
            return result.returncode, result.stdout
        except FileNotFoundError:
            return -1, f"CLI executable not found: {self.exe_path}"
        except subprocess.TimeoutExpired:
            return -1, "Command timed out"
        except Exception as e:
            return -1, str(e)

    def cancel(self) -> None:
        """Cancel the currently running process."""
        callback = None  # type: Optional[Callable[[int], None]]
        cancelled_pending = False
        with self._lock:
            self._cancelled = True
            process = self._process
            thread = self._thread
            if process is None and thread is not None:
                cancel = getattr(thread, "cancel", None)
                if callable(cancel) and cancel():
                    callback = self._current_on_finished
                    self._thread = None
                    self._busy = False
                    self._current_on_finished = None
                    cancelled_pending = True

        if cancelled_pending:
            self.on_output("[GUI] Process cancelled.\n")
            if callback:
                callback(-1)
            return

        if process is not None:
            try:
                process.terminate()
            except OSError:
                pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    process.kill()
                except OSError:
                    # Best-effort cancellation: process may have already exited.
                    pass

    def shutdown(self) -> None:
        """Stop any active process and release fallback worker threads."""
        self.cancel()
        if self._fallback_executor is not None:
            self._fallback_executor.shutdown(wait=True, cancel_futures=True)
            self._fallback_executor = None
