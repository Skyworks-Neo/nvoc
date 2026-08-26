from __future__ import annotations

from rich.text import Text
from textual.widgets import Button, Input, Static

from ..metrics_format import _format_metric_lines
from ..widgets import mnemonic_text
from .base import PaneController


class DashboardController(PaneController):
    # First sample may wake the GPU; later polls never block GC6 sleep.
    _first_success = False
    # dGPU-offline backoff: consecutive failed polls whose error looks like a
    # dead/disappearing GPU (ApiNotInitialized after the user disabled the
    # dGPU, NoImplementation when enumeration finds no GPU) trigger a slow
    # re-probe cadence. Each backoff tick re-runs GPU discovery so the dGPU is
    # auto-detected when it returns; one successful poll exits backoff.
    _OFFLINE_FAIL_THRESHOLD = 3
    _OFFLINE_BACKOFF_S = 5.0
    _OFFLINE_BACKOFF_CAP_S = 15.0

    def __init__(self, app) -> None:
        super().__init__(app)
        self.poll_timer = None
        self._timer_paused = False
        self._user_interval = 1.0
        self._consecutive_offline = 0
        self._in_offline_backoff = False
        self._offline_hint_logged = False

    def set_poll_timer(self, interval: float) -> None:
        interval = max(0.2, min(interval, 60.0))
        self._user_interval = interval
        self.app.config_data.dashboard.refresh_interval = interval
        self.app.save_config()
        # In backoff we ignore the user interval and use the slow re-probe rate.
        effective = self._effective_interval()
        if self.poll_timer is not None:
            self.poll_timer.stop()
        self._timer_paused = False
        self.poll_timer = self.app.set_interval(effective, self.tick, pause=False)

    def _effective_interval(self) -> float:
        if not self._in_offline_backoff:
            return self._user_interval
        step = max(1, self._consecutive_offline - self._OFFLINE_FAIL_THRESHOLD + 1)
        return min(self._OFFLINE_BACKOFF_S * step, self._OFFLINE_BACKOFF_CAP_S)

    def tick(self) -> None:
        if self.app.native_service.action_state.running:
            return
        # In offline backoff, re-probe GPU discovery instead of a doomed NVAPI
        # status sweep. discover re-enumerates from the driver, so the dGPU is
        # picked up the moment it returns; the next status poll then succeeds
        # and exits backoff.
        if self._in_offline_backoff:
            self.app.refresh_gpu_list()
            return
        self.app.run_query(
            "status",
            self.on_status_loaded,
            log_output=False,
            allow_wake=not self._first_success,
        )

    @staticmethod
    def _looks_like_offline_error(output: str) -> bool:
        if not output:
            return False
        lowered = output.lower()
        return (
            "not_initialized" in lowered
            or "api_not_initialized" in lowered
            or "noimplementation" in lowered
            or "nvidia_device_not_found" in lowered
            or "novidevicefound" in lowered
            or "gpu is lost" in lowered
        )

    def _record_offline_failure(self) -> None:
        self._consecutive_offline += 1
        if (
            not self._in_offline_backoff
            and self._consecutive_offline >= self._OFFLINE_FAIL_THRESHOLD
        ):
            self._enter_offline_backoff()

    def _enter_offline_backoff(self) -> None:
        self._in_offline_backoff = True
        if not self._offline_hint_logged:
            self._offline_hint_logged = True
            try:
                self.app.write_log(
                    "dGPU probably offline — polling paused, re-probing for the "
                    "GPU to come back."
                )
            except Exception:
                pass
        # Re-arm the timer at the backoff cadence and kick a rediscovery now.
        self.set_poll_timer(self._user_interval)
        try:
            self.app.refresh_gpu_list()
        except Exception:
            pass

    def _exit_offline_backoff(self) -> None:
        if self._in_offline_backoff:
            self._in_offline_backoff = False
            try:
                self.app.write_log("dGPU back online — resuming polling.")
            except Exception:
                pass
            self.set_poll_timer(self._user_interval)
        self._consecutive_offline = 0
        self._offline_hint_logged = False

    def on_info_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0 and not parsed:
            return
        self.app.cache.info = parsed
        self.update_metrics()
        self.app.overclock_controller.prime_inputs()

    def on_status_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code == 0:
            self._first_success = True
            self._exit_offline_backoff()
        elif self._looks_like_offline_error(output):
            # Suppress the per-second error spam; backoff handles re-probing.
            self._record_offline_failure()
            return
        if code != 0 and not parsed:
            return
        self.app.cache.status = parsed
        self.update_metrics()
        if self.app.cache.vf_curve_points or self.app.cache.vf_curves:
            # Re-render the GPC live point (status feed) and kick the
            # direct-read poll when the active curve is xbar/host.
            self.app.vfcurve_controller.poll_live()

    def on_get_loaded(self, code: int, output: str, parsed: dict) -> None:
        if code != 0:
            return
        self.app.cache.settings = parsed
        self.app.overclock_controller.prime_inputs()

    def update_metrics(self) -> None:
        info = self.app.cache.info
        status = self.app.cache.status
        architecture = info.get("arch") or info.get("codename") or "---"
        lines = _format_metric_lines(status, architecture)
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
            if self.poll_timer and self._timer_paused:
                self.poll_timer.resume()
                self._timer_paused = False
                button.label = self.pause_label()
            elif self.poll_timer:
                self.poll_timer.pause()
                self._timer_paused = True
                button.label = "Resume"
            return True
        if button_id == "dashboard-now":
            self.tick()
            return True
        if button_id == "dashboard-info":
            self.app.run_query("info", self.on_info_loaded)
            return True
        if button_id == "dashboard-status":
            self.app.run_query("status", self.on_status_loaded, allow_wake=False)
            return True
        if button_id == "dashboard-get":
            self.app.run_query("get", self.on_get_loaded, allow_wake=False)
            return True
        return False
