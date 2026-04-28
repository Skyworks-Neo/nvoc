# Copyright (C) 2026 Ajax Dong
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program. If not, see <https://www.gnu.org/licenses/>.
from __future__ import annotations

import os
import shutil
import subprocess
import threading
from pathlib import Path
from typing import Callable

from .models import ActionState, CliLocation, GpuDescriptor
from .parsing import normalize_query_output, parse_gpu_list


OutputCallback = Callable[[str, str], None]
FinishCallback = Callable[[int], None]


class CliService:
    def __init__(self, repo_root: Path) -> None:
        self.repo_root = repo_root
        self._process: subprocess.Popen[str] | None = None
        self._lock = threading.Lock()
        self.action_state = ActionState()

    @staticmethod
    def discover_cli(saved_path: str = "") -> CliLocation:
        candidates = [saved_path, shutil.which("nvoc-autooptimizer") or "", shutil.which("nvoc-auto-optimizer") or ""]
        sibling = Path(__file__).resolve().parent.parent.parent / "NVOC-AutoOptimizer" / "target" / "release" / "nvoc-auto-optimizer"
        candidates.append(str(sibling))
        if os.name == "nt":
            candidates.append(str(sibling.with_suffix(".exe")))

        for candidate in candidates:
            if candidate and Path(candidate).is_file():
                cwd = str(Path(candidate).resolve().parent.parent)
                if Path(cwd).name == "target":
                    cwd = str(Path(cwd).parent)
                return CliLocation(exe_path=str(Path(candidate).resolve()), cwd=cwd)
        return CliLocation(exe_path="", cwd=None)

    def run_query(self, cli: CliLocation, args: list[str], command_name: str) -> tuple[int, str, dict]:
        query_args = list(args)
        if command_name in {"info", "status", "get", "list"} and "-O" not in query_args:
            query_args = ["-O", "json"] + query_args
        cmd = [cli.exe_path] + query_args
        if not cli.exe_path:
            return -1, "CLI executable not configured.", {}
        try:
            completed = subprocess.run(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                cwd=cli.cwd,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
            )
        except FileNotFoundError:
            return -1, f"CLI executable not found: {cli.exe_path}", {}
        except subprocess.TimeoutExpired:
            return -1, "Command timed out.", {}
        except Exception as exc:
            return -1, str(exc), {}
        return completed.returncode, completed.stdout, normalize_query_output(command_name, completed.stdout)

    def list_gpus(self, cli: CliLocation) -> tuple[int, str, list[GpuDescriptor]]:
        if not cli.exe_path:
            return -1, "CLI executable not configured.", []
        try:
            completed = subprocess.run(
                [cli.exe_path, "list"],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                cwd=cli.cwd,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
            )
        except Exception as exc:
            return -1, str(exc), []
        return completed.returncode, completed.stdout, parse_gpu_list(completed.stdout)

    def run_action(
        self,
        cli: CliLocation,
        args: list[str],
        on_output: OutputCallback,
        on_finished: FinishCallback,
    ) -> bool:
        with self._lock:
            if self.action_state.running:
                return False
            self.action_state.running = True
            self.action_state.description = " ".join(args)

        def worker() -> None:
            cmd = [cli.exe_path] + args
            on_output(f"> {' '.join(cmd)}\n", "command")
            code = -1
            try:
                self._process = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    cwd=cli.cwd,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    bufsize=1,
                )
                if self._process.stdout is not None:
                    for line in iter(self._process.stdout.readline, ""):
                        on_output(line, "info")
                code = self._process.wait()
                on_output(f"Process exited with code {code}\n", "success" if code == 0 else "error")
            except FileNotFoundError:
                on_output(f"CLI executable not found: {cli.exe_path}\n", "error")
            except Exception as exc:
                on_output(f"{exc}\n", "error")
            finally:
                with self._lock:
                    self._process = None
                    self.action_state.running = False
                    self.action_state.description = ""
                on_finished(code)

        threading.Thread(target=worker, daemon=True, name="nvoc-tui-action").start()
        return True

    def cancel_action(self) -> None:
        with self._lock:
            if self._process is not None:
                try:
                    self._process.terminate()
                except OSError:
                    pass
