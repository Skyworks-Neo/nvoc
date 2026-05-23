from __future__ import annotations

import importlib
import json
import threading
from pathlib import Path
from typing import Any, Callable

from .models import ActionState, GpuDescriptor


OutputCallback = Callable[[str, str], None]
FinishCallback = Callable[[int], None]
ActionCallback = Callable[[Any], str | None]


class NativeService:
    def __init__(self, repo_root: Path) -> None:
        self.repo_root = repo_root
        self._native: Any | None = None
        self._lock = threading.Lock()
        self.action_state = ActionState()

    def _pynvoc(self) -> Any:
        if self._native is None:
            self._native = importlib.import_module("pynvoc")
        return self._native

    def list_gpus(self) -> tuple[int, str, list[GpuDescriptor]]:
        try:
            items = self._pynvoc().discover_gpus("both")
        except Exception as exc:
            return -1, f"pynvoc GPU discovery failed: {exc}", []
        gpus = [
            GpuDescriptor(
                index=int(item.get("index", idx)),
                name=str(item.get("name") or f"GPU {item.get('index', idx)}"),
                gpu_id_hex=str(item.get("gpu_id_hex") or "") or None,
            )
            for idx, item in enumerate(items)
            if isinstance(item, dict)
        ]
        return 0, f"Detected {len(gpus)} GPU(s) via pynvoc.", gpus

    def run_query(self, gpu: str, command_name: str) -> tuple[int, str, dict]:
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
            if self.action_state.running:
                return False
            self.action_state.running = True
            self.action_state.description = description

        def worker() -> None:
            code = -1
            try:
                on_output(f"> {description}\n", "command")
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
                    self.action_state.running = False
                    self.action_state.description = ""
                on_finished(code)

        threading.Thread(
            target=worker, daemon=True, name="nvoc-tui-native-action"
        ).start()
        return True

    def _query_output(self, command_name: str, gpu: str, parsed: dict) -> str:
        body = json.dumps(parsed, indent=2, sort_keys=True, default=str)
        return f"> native {command_name} --gpu={gpu}\n{body}"
