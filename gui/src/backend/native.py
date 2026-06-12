"""Native ``pynvoc`` GUI backend.

Short direct GUI operations use this adapter. The auto-optimize workflow stays
on the CLI runner so it can keep streamed output and cancellation semantics.
"""

from __future__ import annotations

import importlib
import json
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import TYPE_CHECKING, Any, Callable

from src.backend.base import FanSettings

if TYPE_CHECKING:
    from src.app import App


OutputCallback = Callable[[str, str], None]
FinishCallback = Callable[[int], None]
ActionCallback = Callable[[Any], str | None]


class NativeBackend:
    def __init__(self, app: "App") -> None:
        self.app = app
        self._native: Any | None = None
        self._lock = threading.Lock()
        self._action_running = False
        self._fallback_executor: ThreadPoolExecutor | None = None

    def _pynvoc(self) -> Any:
        if self._native is None:
            self._native = importlib.import_module("pynvoc")
        return self._native

    def list_gpus(self) -> tuple[int, str, list[dict[str, Any]]]:
        try:
            items = self._pynvoc().discover_gpus("both")
        except Exception as exc:
            return -1, f"pynvoc GPU discovery failed: {exc}", []
        gpus = [item for item in items if isinstance(item, dict)]
        return 0, f"Detected {len(gpus)} GPU(s) via pynvoc.", gpus

    def run_query(self, gpu: str, command_name: str) -> tuple[int, str, dict[str, Any]]:
        try:
            native = self._pynvoc()
            if command_name == "info":
                parsed = native.query_info(gpu, "both")
            elif command_name == "status":
                parsed = native.query_status(gpu, "both")
            elif command_name == "get":
                parsed = native.query_settings(gpu, "both")
            else:
                return -1, f"Unsupported native query: {command_name}", {}
            if not isinstance(parsed, dict):
                return -1, f"pynvoc {command_name} query returned non-dict data.", {}
            return 0, self._query_output(command_name, gpu, parsed), parsed
        except Exception as exc:
            return -1, f"pynvoc {command_name} query failed: {exc}", {}

    def query_domain_vfp_points(self, gpu: str, domain: str = "graphics") -> list[dict]:
        return self._pynvoc().query_domain_vfp_points(gpu, domain, True)

    def run_action(
        self,
        description: str,
        action: ActionCallback,
        on_output: OutputCallback,
        on_finished: FinishCallback,
    ) -> bool:
        with self._lock:
            already_running = self._action_running
            if not already_running:
                self._action_running = True
        if already_running:
            on_output("Another native action is already running.\n", "error")
            on_finished(-1)
            return False

        def worker() -> None:
            code = -1
            try:
                on_output(f"> native {description}\n", "command")
                output = action(self._pynvoc())
                if output:
                    on_output(
                        output if output.endswith("\n") else f"{output}\n", "info"
                    )
                code = 0
                on_output("Native action completed.\n", "success")
            except Exception as exc:
                on_output(f"{exc}\n", "error")
            finally:
                with self._lock:
                    self._action_running = False
                on_finished(code)

        submit = getattr(self.app, "run_background", None)
        try:
            if callable(submit):
                submit("native-action", worker)
            else:
                if self._fallback_executor is None:
                    self._fallback_executor = ThreadPoolExecutor(
                        max_workers=2, thread_name_prefix="nvoc-gui-native"
                    )
                self._fallback_executor.submit(worker)
        except Exception as exc:
            with self._lock:
                self._action_running = False
            on_output(f"Failed to schedule native action: {exc}\n", "error")
            on_finished(-1)
            return False
        return True

    def shutdown(self) -> None:
        """Release fallback worker threads when NativeBackend is used standalone."""
        if self._fallback_executor is not None:
            self._fallback_executor.shutdown(wait=True, cancel_futures=True)
            self._fallback_executor = None

    def apply_fan_settings(self, settings: FanSettings) -> None:
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self.app.console.append("[GUI] No GPU selected.\n")
            return

        def apply(native: Any, gpu: str = gpu, settings: FanSettings = settings) -> str:
            native.set_fan(
                gpu, settings.backend, settings.fan_id, settings.policy, settings.level
            )
            return "Successfully applied fan settings."

        self.app.run_native_action("apply fan settings", apply)

    def reset_fan_settings(self, settings: FanSettings) -> None:
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            self.app.console.append("[GUI] No GPU selected.\n")
            return

        def reset(native: Any, gpu: str = gpu, settings: FanSettings = settings) -> str:
            native.set_fan(gpu, settings.backend, settings.fan_id, "auto", 0)
            return "Successfully reset fan settings."

        self.app.run_native_action("reset fan settings", reset)

    @staticmethod
    def _query_output(command_name: str, gpu: str, parsed: dict[str, Any]) -> str:
        body = json.dumps(parsed, indent=2, sort_keys=True, default=str)
        return f"> native {command_name} --gpu={gpu}\n{body}"
