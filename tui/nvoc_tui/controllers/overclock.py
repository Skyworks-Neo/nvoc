from __future__ import annotations

from textual.widgets import Input, Select

from .base import PaneController


class OverclockController(PaneController):
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

    def apply_oc(
        self,
        native,
        gpu: str,
        backend: str,
        core_offset: int,
        mem_offset: int,
        pstart: str,
        pend: str,
    ) -> str:
        try:
            native.set_clock_offset(gpu, backend, "core", core_offset, pstart)
            native.set_clock_offset(gpu, backend, "memory", mem_offset, pstart)
            if pend:
                if backend == "nvml":
                    native.set_nvml_pstate_lock(gpu, pstart, pend)
                else:
                    native.set_nvapi_pstate_lock(gpu, pstart, pend)
        except Exception as exc:
            raise self.enrich_pstate_exception(exc) from exc
        return f"Successfully applied {backend} overclock."

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
            pstart = (
                self.normalize_pstate(self.app.query_one("#pstate-start", Input).value)
                or "P0"
            )
            pend = self.normalize_pstate(self.app.query_one("#pstate-end", Input).value)

            pstate_error = self.validate_pstates(pstart, pend)
            if pstate_error:
                self.app.write_log(pstate_error)
                return True

            def apply_oc(
                native,
                gpu=gpu,
                backend=backend,
                core_offset=core_offset,
                mem_offset=mem_offset,
                pstart=pstart,
                pend=pend,
            ) -> str:
                return self.apply_oc(
                    native, gpu, backend, core_offset, mem_offset, pstart, pend
                )

            self.app.run_native_action(
                "apply overclock",
                apply_oc,
            )
            return True
        if button_id == "oc-reset":
            backend = self.app.query_one("#oc-api", Select).value or "nvapi"
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True
            self.app.run_action_chain(
                [
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
                ]
            )
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
        return False
