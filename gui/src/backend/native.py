"""Native ``pynvoc`` GUI backend.

Short direct GUI operations use this adapter. The auto-optimize workflow stays
on the CLI runner so it can keep streamed output and cancellation semantics.
"""

from __future__ import annotations

import importlib
import json
import threading
import time
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

    def _force_wake(self, gpu: str) -> bool:
        """Native GC6 wake (force_gc6_exit) via pynvoc.

        Mobile dGPUs drop to GCOFF after a few idle seconds; NVAPI reads then
        fail and get misreported as 'unsupported' (mobile controls, P-States,
        missing live-point data). Best-effort: desktop GPUs return False.
        """
        try:
            wake = getattr(self._pynvoc(), "force_wake", None)
            return bool(wake(gpu)) if callable(wake) else False
        except Exception:
            return False

    def force_wake(self, gpu: str) -> bool:
        """Public alias for the GC6 wake (used by the GUI re-probe path)."""
        return self._force_wake(gpu)

    def run_query(
        self, gpu: str, command_name: str, allow_wake: bool = True
    ) -> tuple[int, str, dict[str, Any]]:
        retcode, output, parsed = self._run_query_once(gpu, command_name)
        if retcode != 0 and allow_wake:
            # A failed read on a mobile GPU is often just GCOFF (idle dGPU
            # powered down) — wake it, give the D0 transition a moment, and
            # retry once before giving up. Monitoring polls pass
            # allow_wake=False so they never block GC6 sleep.
            self._force_wake(gpu)
            time.sleep(0.3)
            retcode, output, parsed = self._run_query_once(gpu, command_name)
        return retcode, output, parsed

    def _run_query_once(
        self, gpu: str, command_name: str
    ) -> tuple[int, str, dict[str, Any]]:
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
        try:
            return self._pynvoc().query_domain_vfp_points(gpu, domain, True)
        except Exception:
            self._force_wake(gpu)
            return self._pynvoc().query_domain_vfp_points(gpu, domain, True)

    def query_clk_vf_points(self, gpu: str) -> dict | None:
        """Read the private ClockClient V/F-POINTS table (segments + points).

        Returns ``None`` when the private family is absent (the open VFP
        interface is the only source for that GPU). Best-effort wake like the
        public read.
        """
        try:
            return self._pynvoc().query_clk_vf_points(gpu)
        except Exception:
            self._force_wake(gpu)
            try:
                return self._pynvoc().query_clk_vf_points(gpu)
            except Exception:
                return None

    def query_clk_domain_freq_direct(self, gpu: str, domain_bit: int) -> dict | None:
        """Direct physical clock for one domain (green-curve MEASURE 0x527FC458).

        Returns ``{"domain_bit", "freq_khz"}`` (``freq_khz == 0`` ⇒ driver
        refused / not measurable — caller should not draw a live point), or
        ``{"supported": false}`` when the family is absent, or ``None`` on a
        transient error. Preferred over the counter-based read for XBAR/HOST
        live-point polling: one call, no 50 ms sleep.
        """
        try:
            return self._pynvoc().query_clk_domain_freq_direct(gpu, int(domain_bit))
        except Exception:
            self._force_wake(gpu)
            try:
                return self._pynvoc().query_clk_domain_freq_direct(gpu, int(domain_bit))
            except Exception:
                return None

    def query_mobile_limits(self, gpu: str) -> dict[str, Any]:
        """Fetch the mobile power/thermal control surface (all NVAPI).

        Returns ``{"tgp": dict|None, "dnotifier": dict|None,
        "temp_policies": list, "volt_rail": dict|None}``;
        ``None`` sub-dicts mean the private interface isn't exposed by this
        driver.
        """
        data = self._query_mobile_limits_once(gpu)
        attempts = 0
        while (
            data["tgp"] is None
            and data["dnotifier"] is None
            and not data["temp_policies"]
            and data.get("volt_rail") is None
            and data.get("power_limit_w") is None
            and attempts < 3
        ):
            # All three failed at once is the GCOFF signature, not three
            # coincidental 'unsupported' verdicts. The wake call returns
            # before the dGPU actually reaches D0, so allow a few
            # wake->settle->retry rounds before concluding 'unsupported'.
            self._force_wake(gpu)
            time.sleep(0.4)
            data = self._query_mobile_limits_once(gpu)
            attempts += 1
        return data

    def _query_mobile_limits_once(self, gpu: str) -> dict[str, Any]:
        native = self._pynvoc()
        # All five sub-queries are independent NVAPI reads and pynvoc runs
        # them lock-free against the Arc'd inventory snapshot — fan them out
        # so the mobile control surface loads in ~max(query) instead of the
        # sum (each sweep is 100-500ms on a woken dGPU).
        from concurrent.futures import ThreadPoolExecutor

        def _safe(fn, default):
            try:
                return fn()
            except Exception:
                return default

        with ThreadPoolExecutor(
            max_workers=5, thread_name_prefix="nvoc-mobile"
        ) as pool:
            tgp_f = pool.submit(_safe, lambda: native.query_tgp_watt_range(gpu), None)
            dnotifier_f = pool.submit(_safe, lambda: native.query_dnotifier(gpu), None)
            policies_f = pool.submit(
                _safe, lambda: native.query_target_temp_policies(gpu), []
            )
            enforced_f = pool.submit(
                _safe,
                # NVML enforced power limit: the actually-active power wall
                # (post D-Notifier/load clamp) — the TGP policy itself exposes
                # no current-value read, so this is the closest real position.
                lambda: native.query_status(gpu, "both").get("power_limit_w"),
                None,
            )
            # Private VoltRails P0 bounds: VBIOS/VRM voltage ceilings + the
            # currently-effective voltage wall (the limit slider position).
            volt_rail_f = pool.submit(_safe, lambda: native.query_volt_rails(gpu), None)

        tgp = tgp_f.result()
        dnotifier = dnotifier_f.result()
        policies = policies_f.result()
        enforced_w = enforced_f.result()
        volt_rail = volt_rail_f.result()
        if not isinstance(policies, list):
            policies = []
        if not isinstance(volt_rail, dict):
            volt_rail = None
        return {
            "tgp": tgp,
            "dnotifier": dnotifier,
            "temp_policies": policies,
            "volt_rail": volt_rail,
            "power_limit_w": enforced_w,
        }

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
