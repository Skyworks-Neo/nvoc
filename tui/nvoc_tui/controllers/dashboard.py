from __future__ import annotations

from rich.text import Text
from textual.widgets import Button, Input, Static

from ..widgets import mnemonic_text
from .base import PaneController


class DashboardController(PaneController):
    def __init__(self, app) -> None:
        super().__init__(app)
        self.poll_timer = None

    def set_poll_timer(self, interval: float) -> None:
        interval = max(0.2, min(interval, 60.0))
        self.app.config_data.dashboard.refresh_interval = interval
        self.app.save_config()
        if self.poll_timer is not None:
            self.poll_timer.stop()
        self.poll_timer = self.app.set_interval(interval, self.tick, pause=False)

    def tick(self) -> None:
        if self.app.cli_service.action_state.running:
            return
        self.app.run_query(
            "status",
            self.app.gpu_args() + ["-O", "json", "status", "-a"],
            self.on_status_loaded,
        )

    def on_info_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0 and output:
            self.app.write_log(output)
        if code != 0 and not parsed:
            return
        self.app.cache.info = parsed
        self.update_metrics()
        self.app.overclock_controller.prime_inputs()

    def on_status_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0 and output:
            self.app.write_log(output)
        if code != 0 and not parsed:
            return
        self.app.cache.status = parsed
        self.update_metrics()
        if self.app.cache.vf_curve_path:
            self.app.vfcurve_controller.render_plot()

    def on_get_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0:
            self.app.write_log(output)
            return
        self.app.cache.settings = parsed
        self.app.overclock_controller.prime_inputs()

    def update_metrics(self) -> None:
        info = self.app.cache.info
        status = self.app.cache.status
        architecture = info.get("arch") or info.get("codename") or "---"
        if status.get("vfp_locked"):
            lock_mv = status.get("vfp_lock_mv")
            if isinstance(lock_mv, (int, float)):
                vfp_lock_text = f"ON ({lock_mv} mV)"
            else:
                vfp_lock_text = "ON"
        else:
            vfp_lock_text = "OFF"
        lines = [
            f"GPU: {status.get('gpu_clock_mhz', '---')} MHz",
            f"MEM: {status.get('mem_clock_mhz', '---')} MHz",
            f"VOLT: {status.get('voltage_mv', '---')} mV",
            f"VFP LOCK: {vfp_lock_text}",
            f"TEMP: {status.get('temperature_c', '---')} C",
            f"PWR: {status.get('power_w', '---')} W",
            f"ARCH: {architecture}",
        ]
        self.app.query_one("#metrics", Static).update("\n".join(lines))

    def activate_button(self, button_id: str) -> bool:
        button = self.app.query_one(f"#{button_id}", Button)
        return self.handle_button(button, button_id)

    def pause_label(self) -> Text:
        return mnemonic_text("P", "ause")

    def handle_button(self, button: Button, button_id: str) -> bool:
        if button_id == "dashboard-interval-apply":
            try:
                value = float(
                    self.app.query_one("#dashboard-interval", Input).value.strip()
                )
            except ValueError:
                value = 1.0
            self.set_poll_timer(value)
            return True
        if button_id == "dashboard-pause":
            if self.poll_timer and self.poll_timer.pause:
                self.poll_timer.resume()
                button.label = self.pause_label()
            elif self.poll_timer:
                self.poll_timer.pause()
                button.label = "Resume"
            return True
        if button_id == "dashboard-now":
            self.tick()
            return True
        if button_id == "dashboard-info":
            self.app.run_cli_action(self.app.gpu_args() + ["info"])
            return True
        if button_id == "dashboard-status":
            self.app.run_cli_action(self.app.gpu_args() + ["status", "-a"])
            return True
        if button_id == "dashboard-get":
            self.app.run_cli_action(self.app.gpu_args() + ["get"])
            return True
        return False
