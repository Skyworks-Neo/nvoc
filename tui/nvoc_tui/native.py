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
        self._query_queue: queue.Queue[Callable[[], None] | None] = queue.Queue()
        self._query_worker = threading.Thread(
            target=self._query_loop,
            daemon=True,
            name="nvoc-tui-query",
        )
        self._query_worker.start()

    def _query_loop(self) -> None:
        while True:
            job = self._query_queue.get()
            try:
                if job is None:
                    return
                job()
            except Exception:
                # Query jobs marshal their own errors to the UI. Keep the
                # shared worker alive if a callback itself unexpectedly fails.
                pass
            finally:
                self._query_queue.task_done()

    def submit_query(self, job: Callable[[], None]) -> None:
        """Run a read-only frontend query on the shared serial worker."""
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

    def _force_wake(self, gpu: str) -> bool:
        """Native GC6 wake (force_gc6_exit) via pynvoc.

        Mobile dGPUs drop to GCOFF after a few idle seconds; NVAPI reads then
        fail and get misreported as 'unsupported'. Best-effort: desktop GPUs
        return False, and an older pynvoc without force_wake is tolerated.
        """
        try:
            wake = getattr(self._pynvoc(), "force_wake", None)
            return bool(wake(gpu)) if callable(wake) else False
        except Exception:
            return False

    def run_query(
        self, gpu: str, command_name: str, *, allow_wake: bool = True
    ) -> tuple[int, str, dict]:
        retcode, output, parsed = self._run_query_once(gpu, command_name)
        if retcode != 0 and allow_wake:
            # A failed read on a mobile GPU is often just GCOFF (idle dGPU
            # powered down) — wake it and retry once before giving up. Only
            # the first sample / manual buttons opt in; steady-state polls
            # pass allow_wake=False so monitoring never blocks GC6 sleep.
            self._force_wake(gpu)
            retcode, output, parsed = self._run_query_once(gpu, command_name)
        return retcode, output, parsed

    def _run_query_once(self, gpu: str, command_name: str) -> tuple[int, str, dict]:
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

    def query_public_vftable(self, gpu: str, domain: str = "graphics") -> list[dict]:
        try:
            return self._pynvoc().query_public_vftable(gpu, domain, True)
        except Exception:
            self._force_wake(gpu)
            return self._pynvoc().query_public_vftable(gpu, domain, True)

    def query_private_vftable(self, gpu: str) -> dict | None:
        """Private ClockClient V/F-POINTS table (segments + points).

        Returns ``None`` when the private family is absent (the open VFP
        interface is the only source for that GPU). Best-effort wake like
        the public read. Mirrors the GUI backend adapter.
        """
        try:
            return self._pynvoc().query_private_vftable(gpu)
        except Exception:
            self._force_wake(gpu)
            try:
                return self._pynvoc().query_private_vftable(gpu)
            except Exception:
                return None

    def query_private_freq_domain_status(
        self, gpu: str, domain_bit: int
    ) -> dict | None:
        """Direct physical clock for one domain (green-curve MEASURE 0x527FC458).

        Returns ``{"domain_bit", "freq_khz"}`` (``freq_khz == 0`` ⇒ driver
        refused / not measurable — caller should not draw a live point),
        ``{"supported": false}`` when the family is absent, or ``None`` on a
        transient error. Preferred over the counter-based read for XBAR/HOST
        live-point polling: one call, no 50 ms sleep.
        """
        try:
            return self._pynvoc().query_private_freq_domain_status(gpu, int(domain_bit))
        except Exception:
            self._force_wake(gpu)
            try:
                return self._pynvoc().query_private_freq_domain_status(
                    gpu, int(domain_bit)
                )
            except Exception:
                return None

    def query_volt_rails(self, gpu: str) -> dict | None:
        """Private VoltRails family (rail mask + P0 voltage bounds).

        Returns the pynvoc ``query_volt_rails`` dict (with a ``p0`` sub-dict
        of floor/ceiling/effective walls) or ``None`` on a transient error.
        Best-effort wake like the other private reads. Mirrors the GUI backend.
        """
        try:
            return self._pynvoc().query_volt_rails(gpu)
        except Exception:
            self._force_wake(gpu)
            try:
                return self._pynvoc().query_volt_rails(gpu)
            except Exception:
                return None

    def query_mobile_limits(self, gpu: str) -> dict:
        """Fetch the mobile power/thermal control surface (all NVAPI).

        Returns ``{"tgp": dict|None, "dnotifier": dict|None,
        "temp_policies": list, "volt_rail": dict|None}``; ``None`` sub-dicts
        mean the private interface isn't exposed by this driver.
        """
        data = self._query_mobile_limits_once(gpu)
        if (
            data["tgp"] is None
            and data["dnotifier"] is None
            and not data["temp_policies"]
            and data["volt_rail"] is None
        ):
            # All three failed at once is the GCOFF signature — wake and retry.
            self._force_wake(gpu)
            data = self._query_mobile_limits_once(gpu)
        return data

    def _query_mobile_limits_once(self, gpu: str) -> dict:
        native = self._pynvoc()
        tgp = None
        dnotifier = None
        policies: Any = []
        volt_rail = None
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
        try:
            volt_rail = native.query_volt_rails(gpu)
        except Exception:
            volt_rail = None
        if not isinstance(volt_rail, dict):
            volt_rail = None
        return {
            "tgp": tgp,
            "dnotifier": dnotifier,
            "temp_policies": policies,
            "volt_rail": volt_rail,
        }

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
