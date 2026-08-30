from __future__ import annotations

from textual.widgets import Select

from ..models import GpuDescriptor
from .base import PaneController


def _is_discovery_offline_error(output: str) -> bool:
    """True when a failed GPU discovery's error indicates the dGPU is gone.

    Discovery re-runs on the offline-backoff cadence (after the user disabled
    the dGPU), and `NvAPI_EnumPhysicalGPUs` returns ApiNotInitialized /
    NoImplementation / NvidiaDeviceNotFound while the dGPU is off. Suppressing
    these per-tick errors keeps the console quiet during backoff; the dashboard
    already logged a single "dGPU probably offline" hint.
    """
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


class HeaderController(PaneController):
    def selected_gpu_idx(self) -> int | None:
        try:
            value = self.app.query_one("#gpu-select", Select).value
        except Exception:
            return None
        if value in (None, Select.BLANK):
            return None
        return int(value)

    def gpu_args(self) -> list[str]:
        idx = self.selected_gpu_idx()
        return [f"--gpu={idx}"] if idx is not None and idx >= 0 else []

    def current_gpu(self) -> GpuDescriptor | None:
        idx = self.selected_gpu_idx()
        for gpu in self.app.gpus:
            if gpu.index == idx:
                return gpu
        return None

    def focus_gpu_select(self) -> None:
        self.app.query_one("#gpu-select", Select).focus()

    def on_gpu_selected(self, value: object) -> None:
        if value not in (None, Select.BLANK):
            self.app.config_data.last_gpu_idx = int(value)
            self.app.save_config()
            self.app.refresh_all_state()
            # GPU switch: reload (or clear, when the new part has no V/F
            # interface) the VF curve — without this the previous GPU's
            # curve lingers on the plot.
            self.app.vfcurve_controller.on_gpu_changed()

    def on_gpu_list_loaded(
        self, code: int, output: str, gpus: list[GpuDescriptor]
    ) -> None:
        # Suppress per-tick discovery-error spam during offline backoff: the
        # dashboard logged one "dGPU probably offline" hint and is re-probing
        # on a slow cadence. Logging every failed re-probe (each producing
        # "NvAPI_EnumPhysicalGPUs failed: API_NOT_INITIALIZED") floods the
        # console. A successful detection always logs "GPU detection finished."
        if code != 0 and _is_discovery_offline_error(output):
            pass  # silent — backoff hint already logged
        else:
            self.app.write_log(output or "GPU detection finished.")
        self.app.gpus = gpus
        select = self.app.query_one("#gpu-select", Select)
        if not gpus:
            select.set_options([("(no GPUs found)", "-1")])
            select.value = "-1"
            # No GPU detected (dGPU disabled at launch or just removed) —
            # re-probe in the background until one appears.
            self.app.start_gpu_reprobe()
            return
        # A GPU landed — stop the background re-probe.
        self.app._stop_gpu_reprobe()
        select.set_options([(gpu.long_label, str(gpu.index)) for gpu in gpus])
        target = self.app.config_data.last_gpu_idx
        if target is None or all(gpu.index != target for gpu in gpus):
            target = gpus[0].index
        select.value = str(target)
        self.app.config_data.last_gpu_idx = target
        self.app.save_config()
        if code == 0:
            self.app.focus_dashboard_tab_switcher()
        self.app.refresh_all_state()

    def handle_button(self, button_id: str) -> bool:
        if button_id == "detect-gpus":
            self.app.refresh_gpu_list()
            return True
        if button_id == "refresh-all":
            self.app.refresh_all_state()
            return True
        return False
