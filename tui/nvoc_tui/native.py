from __future__ import annotations

import importlib
import json
import queue
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
        # 单一常驻查询线程：轮询（dashboard 1 Hz + vfcurve 0.5 Hz）原来
        # 每 tick 新开一个 threading.Thread，线程本身会退出，但每次都
        # 并发进入 Rust 的后端发现路径；改为串行队列后也顺带消除了
        # 重叠 tick 造成的并发 NVAPI/NVML 初始化。
        self._query_queue: queue.Queue = queue.Queue()
        self._query_worker = threading.Thread(
            target=self._query_loop, daemon=True, name="nvoc-tui-query"
        )
        self._query_worker.start()

    def _query_loop(self) -> None:
        while True:
            job = self._query_queue.get()
            if job is None:
                break
            try:
                job()
            except Exception:
                pass

    def submit_query(self, job: Callable[[], None]) -> None:
        """Run `job` on the shared query thread (serializes poll work)."""
        self._query_queue.put(job)

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

    def query_mobile_limits(self, gpu: str) -> dict:
        """Fetch the mobile power/thermal control surface (all NVAPI).

        Returns ``{"tgp": dict|None, "dnotifier": dict|None,
        "temp_policies": list}``; ``None`` sub-dicts mean the private
        interface isn't exposed by this driver.
        """
        native = self._pynvoc()
        tgp = None
        dnotifier = None
        policies: Any = []
        try:
            tgp = native.query_tgp_watt_range(gpu)
        except Exception:
            tgp = None
        try:
            dnotifier = native.query_dnotifier(gpu)
        except Exception:
            dnotifier = None
        try:
            policies = native.query_target_temp_policies(gpu)
        except Exception:
            policies = []
        if not isinstance(policies, list):
            policies = []
        return {"tgp": tgp, "dnotifier": dnotifier, "temp_policies": policies}

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
