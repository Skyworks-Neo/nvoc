from __future__ import annotations

from textual.widgets import Input, Select

from .base import PaneController


class OverclockController(PaneController):
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

    def oc_args(self) -> list[str]:
        backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
        args = self.app.gpu_args() + ["set", backend]
        args += ["--core-offset", str(self.get_int("#core-offset"))]
        args += ["--mem-offset", str(self.get_int("#mem-offset"))]
        pstart = self.app.query_one("#pstate-start", Input).value.strip().lower()
        pend = self.app.query_one("#pstate-end", Input).value.strip().lower()
        if pstart and pend:
            args += ["--pstate-lock", pstart, pend]
        return args

    def limit_args(self) -> list[str]:
        backend = str(self.app.query_one("#power-api", Select).value or "nvapi")
        args = self.app.gpu_args() + ["set", backend]
        args += ["--power-limit", str(self.get_int("#power-limit"))]
        if backend == "nvapi":
            args += ["--thermal-limit", str(self.get_int("#thermal-limit"))]
            args += ["--voltage-boost", str(self.get_int("#voltage-boost"))]
        return args

    def fan_args(self, reset: bool) -> list[str]:
        backend = (
            "nvml-cooler"
            if str(self.app.query_one("#fan-api", Select).value or "nvapi") == "nvml"
            else "nvapi-cooler"
        )
        args = self.app.gpu_args() + ["set", backend]
        fan_id = str(self.app.query_one("#fan-id", Select).value or "all")
        if fan_id != "all":
            args += ["--id", fan_id]
        if reset:
            args += ["--policy", "auto", "--level", "0"]
        else:
            args += [
                "--policy",
                str(self.app.query_one("#fan-policy", Select).value or "continuous"),
                "--level",
                str(self.get_int("#fan-level", 60)),
            ]
        return args

    def handle_button(self, button_id: str) -> bool:
        if button_id == "oc-apply":
            self.app.run_cli_action(self.oc_args())
            return True
        if button_id == "oc-reset":
            backend = self.app.query_one("#oc-api", Select).value or "nvapi"
            self.app.run_action_chain(
                [
                    self.app.gpu_args() + ["set", str(backend), "--core-offset", "0"],
                    self.app.gpu_args() + ["set", str(backend), "--mem-offset", "0"],
                ]
            )
            return True
        if button_id == "limits-apply":
            self.app.run_cli_action(self.limit_args())
            return True
        if button_id == "reset-all":
            self.app.run_cli_action(self.app.gpu_args() + ["reset"])
            return True
        if button_id == "fan-apply":
            self.app.run_cli_action(self.fan_args(reset=False))
            return True
        if button_id == "fan-reset":
            self.app.run_cli_action(self.fan_args(reset=True))
            return True
        return False
