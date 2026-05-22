from __future__ import annotations

from textual.widgets import Input, Select

from .base import PaneController


class OverclockController(PaneController):
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
    ) -> None:
        native.set_clock_offset(gpu, backend, "core", core_offset, pstart)
        native.set_clock_offset(gpu, backend, "memory", mem_offset, pstart)
        if pend:
            if backend == "nvml":
                native.set_nvml_pstate_lock(gpu, pstart, pend)
            else:
                native.set_nvapi_pstate_lock(gpu, pstart, pend)

    def apply_limits(
        self,
        native,
        gpu: str,
        backend: str,
        power_limit: int,
        thermal_limit: int,
        voltage_boost: int,
    ) -> None:
        native.set_power_limit(gpu, backend, power_limit)
        if backend == "nvapi":
            native.set_thermal_limit(gpu, thermal_limit)
            native.set_voltage_boost(gpu, voltage_boost)

    def apply_fan(
        self,
        native,
        gpu: str,
        backend: str,
        fan_id: str,
        reset: bool,
        policy: str,
        level: int,
    ) -> None:
        if reset:
            native.set_fan(gpu, backend, fan_id, "auto", 0)
        else:
            native.set_fan(gpu, backend, fan_id, policy, level)

    def handle_button(self, button_id: str) -> bool:
        if button_id == "oc-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
            core_offset = self.get_int("#core-offset")
            mem_offset = self.get_int("#mem-offset")
            pstart = self.app.query_one("#pstate-start", Input).value.strip() or "P0"
            pend = self.app.query_one("#pstate-end", Input).value.strip()

            def apply_oc(
                native,
                gpu=gpu,
                backend=backend,
                core_offset=core_offset,
                mem_offset=mem_offset,
                pstart=pstart,
                pend=pend,
            ) -> None:
                self.apply_oc(
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
                        ),
                    ),
                    (
                        "reset memory offset",
                        lambda native, gpu=gpu, backend=str(backend): (
                            native.set_clock_offset(gpu, backend, "memory", 0, "P0")
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
            ) -> None:
                self.apply_limits(
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
            self.app.run_native_action(
                "reset all limits",
                lambda native, gpu=gpu: native.reset_all(gpu, None),
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
            ) -> None:
                self.apply_fan(native, gpu, backend, fan_id, False, policy, level)

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

            def reset_fan(native, gpu=gpu, backend=backend, fan_id=fan_id) -> None:
                self.apply_fan(native, gpu, backend, fan_id, True, "auto", 0)

            self.app.run_native_action(
                "reset fan",
                reset_fan,
            )
            return True
        return False
