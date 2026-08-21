from __future__ import annotations

import threading

from textual.widgets import Input, Select

from .base import PaneController


class OverclockController(PaneController):
    def __init__(self, app) -> None:
        super().__init__(app)
        self._mobile_limits_gpu: str | None = None
        self._mobile_load_lock = threading.Lock()
        self._tgp_policy_index = 2
        self._tgp_range = (5, 140)
        self._target_temp_range = (75, 87)

    def available_pstates(self) -> list[str]:
        pstates = self.app.cache.settings.get("supported_pstates", [])
        if not isinstance(pstates, list):
            return []
        normalized: list[str] = []
        for pstate in pstates:
            value = self.normalize_pstate(str(pstate))
            if value and value not in normalized:
                normalized.append(value)
        return normalized

    def normalize_pstate(self, value: str) -> str:
        stripped = value.strip().upper()
        if stripped.isdigit():
            return f"P{int(stripped)}"
        if len(stripped) > 1 and stripped.startswith("P") and stripped[1:].isdigit():
            return f"P{int(stripped[1:])}"
        return stripped

    def pstate_error(self, pstate: str) -> str:
        available = self.available_pstates()
        if available:
            return (
                f"Unknown pstate {pstate}. Available pstates: {', '.join(available)}."
            )
        return (
            f"Unknown pstate {pstate}. Available pstates are not loaded; run Get first."
        )

    def validate_pstates(self, *pstates: str) -> str | None:
        available = self.available_pstates()
        if not available:
            return None
        available_set = set(available)
        for pstate in pstates:
            if pstate and pstate not in available_set:
                return self.pstate_error(pstate)
        return None

    def enrich_pstate_exception(self, exc: Exception) -> Exception:
        message = str(exc)
        if "unknown pstate" not in message.lower():
            return exc
        available = self.available_pstates()
        if available:
            return RuntimeError(
                f"{message}. Available pstates: {', '.join(available)}."
            )
        return exc

    def activate_shortcut(self, target_id: str) -> bool:
        try:
            self.app.query_one(f"#{target_id}").focus()
            return True
        except Exception:
            return False

    def prime_inputs(self) -> None:
        fields = {
            "#core-offset": str(
                self.app.cache.settings.get(
                    "core_clock_current", self.app.cache.info.get("core_clock_min", 0)
                )
            ),
            "#mem-offset": str(
                self.app.cache.settings.get(
                    "mem_clock_current", self.app.cache.info.get("mem_clock_min", 0)
                )
            ),
            "#power-limit": str(
                self.app.cache.settings.get(
                    "power_limit_current",
                    self.app.cache.info.get("power_limit_default", 100),
                )
            ),
            "#thermal-limit": str(self.app.cache.info.get("thermal_limit_default", 83)),
            "#voltage-boost": str(
                self.app.cache.settings.get("voltage_boost_current", 0)
            ),
        }
        for selector, value in fields.items():
            try:
                self.app.query_one(selector, Input).value = value
            except Exception:
                pass
        self.load_mobile_limits()

    def apply_oc(
        self,
        native,
        gpu: str,
        backend: str,
        core_offset: int,
        mem_offset: int,
    ) -> str:
        native.set_clock_offset(gpu, backend, "core", core_offset, "P0")
        native.set_clock_offset(gpu, backend, "memory", mem_offset, "P0")
        return f"Successfully applied {backend} overclock."

    def apply_pstate_limits(
        self,
        native,
        gpu: str,
        backend: str,
        pstart: str,
        pend: str,
    ) -> str:
        try:
            if backend == "nvml":
                native.set_nvml_pstate_lock(gpu, pstart, pend)
            else:
                native.set_nvapi_pstate_lock(gpu, pstart, pend)
        except Exception as exc:
            raise self.enrich_pstate_exception(exc) from exc
        return f"Successfully applied {backend} PState limits {pstart}-{pend}."

    def reset_pstate_limits(self, native, gpu: str, backend: str) -> str:
        if backend == "nvml":
            native.reset_locked_clocks(gpu, backend, "memory")
        else:
            native.reset_vfp_frequency_lock(gpu, "memory")
        return f"Successfully reset {backend} PState limits."

    def apply_limits(
        self,
        native,
        gpu: str,
        backend: str,
        power_limit: int,
        thermal_limit: int,
        voltage_boost: int,
    ) -> str:
        native.set_power_limit(gpu, backend, power_limit)
        if backend == "nvapi":
            native.set_thermal_limit(gpu, thermal_limit)
            native.set_voltage_boost(gpu, voltage_boost)
        return f"Successfully applied {backend} limits."

    def is_mobile(self) -> bool:
        """Mobile-GPU verdict for the Mobile Power pane.

        Primary signal: the query_info payload's ``is_mobile`` flag computed
        in Rust by core's gpu_type.rs detect_gpu_type (name + codename — the
        single source of truth). Fallback: the name-keyword heuristic for
        payloads without the flag (older pynvoc, CLI-parsed info).
        """
        flag = self.app.cache.info.get("is_mobile")
        if isinstance(flag, bool):
            return flag
        gpu_name = str(self.app.cache.info.get("gpu_name", "")).lower()
        return (
            "mobile" in gpu_name
            or "laptop" in gpu_name
            or " m " in gpu_name
            or gpu_name.endswith(" m")
            or " mx " in gpu_name
            or gpu_name.endswith(" mx")
        )

    def load_mobile_limits(self, force: bool = False) -> None:
        """Background-load the mobile control surface via pynvoc (NVAPI)."""
        gpu = self.app.selected_gpu_target()
        if gpu is None or not self.is_mobile():
            return
        if not force and gpu == self._mobile_limits_gpu:
            return
        if not self._mobile_load_lock.acquire(blocking=False):
            return

        def worker() -> None:
            try:
                data = self.app.native_service.query_mobile_limits(gpu)
            except Exception as exc:
                data = {"error": str(exc)}
            finally:
                self._mobile_load_lock.release()
            try:
                self.app.call_from_thread(self._on_mobile_limits, gpu, data)
            except Exception:
                pass

        threading.Thread(
            target=worker, daemon=True, name="nvoc-tui-mobile-limits"
        ).start()

    def _on_mobile_limits(self, gpu: str, data: dict) -> None:
        first_load = self._mobile_limits_gpu != gpu
        self._mobile_limits_gpu = gpu
        tgp = data.get("tgp") if isinstance(data.get("tgp"), dict) else None
        dnotifier = (
            data.get("dnotifier") if isinstance(data.get("dnotifier"), dict) else None
        )
        policies = data.get("temp_policies") or []
        notes: list[str] = []

        if tgp and tgp.get("min_watt") is not None and tgp.get("max_watt") is not None:
            self._tgp_policy_index = int(tgp.get("policy_index", 2))
            self._tgp_range = (
                int(round(float(tgp["min_watt"]))),
                int(round(float(tgp["max_watt"]))),
            )
            default = int(round(float(tgp.get("default_watt") or tgp["min_watt"])))
            self.set_input("#mobile-tgp", str(default))
        else:
            notes.append("TGP range unavailable")

        if dnotifier and dnotifier.get("levels"):
            options = []
            for item in dnotifier["levels"]:
                label = str(item.get("level", "")).upper()
                try:
                    level_num = int(label.lstrip("D"))
                except ValueError:
                    continue
                watts = item.get("watts")
                display = (
                    f"{label} · {float(watts):.0f}W" if watts is not None else label
                )
                options.append((display, level_num))
            select = self.app.query_one("#mobile-dnotifier", Select)
            select.set_options(options)
            active = dnotifier.get("active")
            if active:
                try:
                    select.value = int(str(active).upper().lstrip("D"))
                except ValueError:
                    pass
        else:
            notes.append("D-Notifier unavailable")

        target = None
        for policy in policies:
            if (
                isinstance(policy, dict)
                and policy.get("min") is not None
                and policy.get("max") is not None
                and float(policy["max"]) > float(policy["min"])
            ):
                target = policy
                break
        if target is not None:
            self._target_temp_range = (
                int(round(float(target["min"]))),
                int(round(float(target["max"]))),
            )
            self.set_input(
                "#mobile-target-temp", str(int(round(float(target.get("celsius", 87)))))
            )
        else:
            notes.append("Target Temp range unavailable")

        if notes:
            self.app.write_log("Mobile power: " + ", ".join(notes) + ".")

        # PPAB has no read-back API; enable it once per GPU on load.
        # Only attempt when the private NVAPI surface actually resolved —
        # on Linux (libnvidia-api stub) / older drivers the setter is
        # NO_IMPLEMENTATION and auto-enabling would just log an error.
        if first_load and (tgp or dnotifier):
            self.app.run_native_action(
                "enable dynamic boost",
                lambda native, gpu=gpu: (
                    native.set_dynamic_boost(gpu, True)
                    or "Dynamic Boost (PPAB) enabled."
                ),
            )

    def set_input(self, selector: str, value: str) -> None:
        try:
            self.app.query_one(selector, Input).value = value
        except Exception:
            pass

    def apply_mobile(
        self,
        native,
        gpu: str,
        ppab: bool,
        d_level: int,
        tgp_watts: int,
        target_temp: int,
    ) -> str:
        native.set_dynamic_boost(gpu, ppab)
        native.set_dnotifier(gpu, d_level)
        native.set_tgp_watt(gpu, tgp_watts, self._tgp_policy_index)
        native.set_target_temp(gpu, float(target_temp), 2)
        return (
            f"Successfully applied mobile power: PPAB {'on' if ppab else 'off'}, "
            f"D{d_level}, TGP {tgp_watts} W, target {target_temp} C."
        )

    def reset_mobile(self, native, gpu: str) -> str:
        native.reset_tgp_watt(gpu, self._tgp_policy_index)
        return "Successfully reset TGP to default."

    def apply_fan(
        self,
        native,
        gpu: str,
        backend: str,
        fan_id: str,
        reset: bool,
        policy: str,
        level: int,
    ) -> str:
        if reset:
            native.set_fan(gpu, backend, fan_id, "auto", 0)
            return "Successfully reset fan control."
        else:
            native.set_fan(gpu, backend, fan_id, policy, level)
            return f"Successfully applied fan {fan_id} {policy} level {level}%."

    def handle_button(self, button_id: str) -> bool:
        if button_id == "oc-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
            core_offset = self.get_int("#core-offset")
            mem_offset = self.get_int("#mem-offset")

            def apply_oc(
                native,
                gpu=gpu,
                backend=backend,
                core_offset=core_offset,
                mem_offset=mem_offset,
            ) -> str:
                return self.apply_oc(native, gpu, backend, core_offset, mem_offset)

            self.app.run_native_action(
                "apply overclock",
                apply_oc,
            )
            return True
        if button_id == "pstate-limits-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
            pstart = (
                self.normalize_pstate(self.app.query_one("#pstate-start", Input).value)
                or "P0"
            )
            pend = (
                self.normalize_pstate(self.app.query_one("#pstate-end", Input).value)
                or pstart
            )

            pstate_error = self.validate_pstates(pstart, pend)
            if pstate_error:
                self.app.write_log(pstate_error)
                return True

            def apply_pstate_limits(
                native, gpu=gpu, backend=backend, pstart=pstart, pend=pend
            ) -> str:
                return self.apply_pstate_limits(native, gpu, backend, pstart, pend)

            self.app.run_native_action(
                "apply PState limits",
                apply_pstate_limits,
            )
            return True
        if button_id == "pstate-limits-reset":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")

            def reset_pstate_limits(native, gpu=gpu, backend=backend) -> str:
                return self.reset_pstate_limits(native, gpu, backend)

            self.app.run_native_action(
                "reset PState limits",
                reset_pstate_limits,
            )
            return True
        if button_id == "oc-reset":
            backend = self.app.query_one("#oc-api", Select).value or "nvapi"
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True
            self.app.run_action_chain([
                (
                    "reset core offset",
                    lambda native, gpu=gpu, backend=str(backend): (
                        native.set_clock_offset(gpu, backend, "core", 0, "P0")
                        or "Successfully reset core offset."
                    ),
                ),
                (
                    "reset memory offset",
                    lambda native, gpu=gpu, backend=str(backend): (
                        native.set_clock_offset(gpu, backend, "memory", 0, "P0")
                        or "Successfully reset memory offset."
                    ),
                ),
            ])
            return True
        if button_id == "limits-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#power-api", Select).value or "nvapi")
            power_limit = self.get_int("#power-limit")
            thermal_limit = self.get_int("#thermal-limit")
            voltage_boost = self.get_int("#voltage-boost")

            def apply_limits(
                native,
                gpu=gpu,
                backend=backend,
                power_limit=power_limit,
                thermal_limit=thermal_limit,
                voltage_boost=voltage_boost,
            ) -> str:
                return self.apply_limits(
                    native,
                    gpu,
                    backend,
                    power_limit,
                    thermal_limit,
                    voltage_boost,
                )

            self.app.run_native_action(
                "apply limits",
                apply_limits,
            )
            return True
        if button_id == "reset-limits":
            gpu = self.app.selected_gpu_target()

            def reset_limits(native, gpu=gpu) -> str:
                native.reset_all(gpu, None)
                return "Successfully reset all limits."

            self.app.run_native_action(
                "reset all limits",
                reset_limits,
            )
            return True
        if button_id == "fan-apply":
            gpu = self.app.selected_gpu_target()
            backend = (
                "nvml-cooler"
                if str(self.app.query_one("#fan-api", Select).value or "nvapi")
                == "nvml"
                else "nvapi-cooler"
            )
            fan_id = str(self.app.query_one("#fan-id", Select).value or "all")
            policy = str(
                self.app.query_one("#fan-policy", Select).value or "continuous"
            )
            level = self.get_int("#fan-level", 60)

            def apply_fan(
                native,
                gpu=gpu,
                backend=backend,
                fan_id=fan_id,
                policy=policy,
                level=level,
            ) -> str:
                return self.apply_fan(
                    native, gpu, backend, fan_id, False, policy, level
                )

            self.app.run_native_action(
                "apply fan",
                apply_fan,
            )
            return True
        if button_id == "fan-reset":
            gpu = self.app.selected_gpu_target()
            backend = (
                "nvml-cooler"
                if str(self.app.query_one("#fan-api", Select).value or "nvapi")
                == "nvml"
                else "nvapi-cooler"
            )
            fan_id = str(self.app.query_one("#fan-id", Select).value or "all")

            def reset_fan(native, gpu=gpu, backend=backend, fan_id=fan_id) -> str:
                return self.apply_fan(native, gpu, backend, fan_id, True, "auto", 0)

            self.app.run_native_action(
                "reset fan",
                reset_fan,
            )
            return True
        if button_id == "mobile-apply":
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True
            ppab = str(self.app.query_one("#mobile-ppab", Select).value or "on") == "on"
            try:
                d_level = int(
                    self.app.query_one("#mobile-dnotifier", Select).value or 1
                )
            except (TypeError, ValueError):
                d_level = 1
            if not 1 <= d_level <= 5:
                self.app.write_log("D-Notifier level must be D1-D5.")
                return True
            tgp_watts = self.get_int("#mobile-tgp", 100)
            lo, hi = self._tgp_range
            tgp_watts = max(lo, min(hi, tgp_watts))
            target_temp = self.get_int("#mobile-target-temp", 87)
            tlo, thi = self._target_temp_range
            target_temp = max(tlo, min(thi, target_temp))

            def apply_mobile(
                native,
                gpu=gpu,
                ppab=ppab,
                d_level=d_level,
                tgp_watts=tgp_watts,
                target_temp=target_temp,
            ) -> str:
                return self.apply_mobile(
                    native, gpu, ppab, d_level, tgp_watts, target_temp
                )

            self.app.run_native_action(
                "apply mobile power",
                apply_mobile,
            )
            return True
        if button_id == "mobile-reset":
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True

            def reset_mobile(native, gpu=gpu) -> str:
                return self.reset_mobile(native, gpu)

            self.app.run_native_action(
                "reset mobile power",
                reset_mobile,
            )
            self.load_mobile_limits(force=True)
            return True
        return False
