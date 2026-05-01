"""
CLI Runner - Subprocess wrapper for NVOC-AutoOptimizer CLI.
Runs commands in background threads and streams output to callbacks.
"""

import subprocess
import threading
import os
import sys
from typing import Callable, Dict, Optional, Sequence, Tuple


class CLIRunner:
    """Wraps nvoc-auto-optimizer.exe subprocess execution."""

    def __init__(
        self,
        exe_path: str,
        on_output: Callable[[str], None],
        on_finished: Optional[Callable[[int], None]] = None,
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
        self._process = None  # type: Optional[subprocess.Popen]
        self._thread = None  # type: Optional[threading.Thread]
        self._cancelled = False

    @staticmethod
    def _no_window_kwargs() -> Dict[str, int]:
        """Return subprocess kwargs for hiding the console on Windows only."""
        if sys.platform == "win32" and hasattr(subprocess, "CREATE_NO_WINDOW"):
            return {"creationflags": subprocess.CREATE_NO_WINDOW}
        return {}

    @property
    def is_running(self) -> bool:
        return self._process is not None and self._process.poll() is None

    def run(self, args: Sequence[str], cwd: Optional[str] = None) -> None:
        """
        Run the CLI with given arguments in a background thread.

        Args:
            args: Command-line arguments (without the exe path)
            cwd: Working directory (defaults to exe parent directory)
        """
        if self.is_running:
            self.on_output(
                "[GUI] A process is already running. Please wait or cancel it.\n"
            )
            return

        self._cancelled = False

        if cwd is None:
            cwd = os.path.dirname(self.exe_path) or "."

        def _worker() -> None:
            cmd = [self.exe_path] + args
            self.on_output(f"[GUI] > {' '.join(cmd)}\n")
            try:
                self._process = subprocess.Popen(
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
                if self._process.stdout is not None:
                    for line in iter(self._process.stdout.readline, ""):
                        if self._cancelled:
                            break
                        self.on_output(line)
                    self._process.stdout.close()
                retcode = self._process.wait()
                if self._cancelled:
                    self.on_output("[GUI] Process cancelled.\n")
                else:
                    self.on_output(f"[GUI] Process exited with code {retcode}\n")
                if self.on_finished:
                    self.on_finished(retcode)
            except FileNotFoundError:
                self.on_output(
                    f"[GUI] ERROR: CLI executable not found: {self.exe_path}\n"
                )
                if self.on_finished:
                    self.on_finished(-1)
            except Exception as e:
                self.on_output(f"[GUI] ERROR: {e}\n")
                if self.on_finished:
                    self.on_finished(-1)
            finally:
                self._process = None

        self._thread = threading.Thread(target=_worker, daemon=True)
        self._thread.start()

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
        self._cancelled = True
        if self._process is not None:
            try:
                self._process.terminate()
            except OSError:
                pass
