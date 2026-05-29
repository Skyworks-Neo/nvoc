"""Fan/cooler pane behavior and command construction."""

from __future__ import annotations

from typing import Optional, Protocol, Sequence

from src.backend.base import FanSettings, GuiBackend


NVAPI_POLICIES = [
    "default",
    "manual",
    "perf",
    "discrete",
    "continuous",
    "hybrid",
    "software",
    "default32",
]
NVML_POLICIES = ["continuous", "manual"]


class FanControlPaneProtocol(Protocol):
    def selected_api(self) -> str: ...
    def selected_fan_id(self) -> str: ...
    def selected_policy(self) -> str: ...
    def fan_level(self) -> int: ...
    def fan_level_text(self) -> str: ...
    def set_policy_values(self, values: Sequence[str]) -> None: ...
    def set_policy(self, policy: str) -> None: ...
    def set_level(self, level: int) -> None: ...
    def set_supported_state(self, supported: bool) -> None: ...


def fan_settings_to_cli_args(
    gpu_args: Sequence[str], settings: FanSettings
) -> list[str]:
    args = list(gpu_args) + ["set", settings.backend]
    if settings.fan_id:
        args.extend(["--id", settings.fan_id])
    args.extend(["--policy", settings.policy, "--level", str(settings.level)])
    return args


class FanControlController:
    def __init__(self, pane: FanControlPaneProtocol, backend: GuiBackend) -> None:
        self.pane = pane
        self.backend = backend

    def selected_backend(self) -> str:
        selected = self.pane.selected_api().strip().upper()
        return "nvml-cooler" if selected == "NVML" else "nvapi-cooler"

    def allowed_policies(self) -> list[str]:
        if self.selected_backend() == "nvml-cooler":
            return list(NVML_POLICIES)
        return list(NVAPI_POLICIES)

    def normalize_policy(self) -> str:
        policy = self.pane.selected_policy().lower().strip()
        allowed = self.allowed_policies()
        if policy not in allowed:
            policy = "continuous" if "continuous" in allowed else allowed[0]
            self.pane.set_policy(policy)
        return policy

    def fan_id(self) -> Optional[str]:
        selected = self.pane.selected_fan_id().strip()
        if not selected.startswith("Fan "):
            return None
        parts = selected.split()
        return parts[1] if len(parts) > 1 else None

    def settings(self, *, reset: bool = False) -> FanSettings:
        return FanSettings(
            backend=self.selected_backend(),
            fan_id=self.fan_id(),
            policy="auto" if reset else self.normalize_policy(),
            level=0 if reset else self.pane.fan_level(),
        )

    def on_backend_change(self) -> None:
        self.pane.set_policy_values(self.allowed_policies())
        self.normalize_policy()

    def on_slider_change(self, value: float) -> None:
        self.pane.set_level(int(value))

    def on_entry_change(self) -> None:
        text = self.pane.fan_level_text().strip()
        if not text:
            return
        try:
            level = int(text)
        except ValueError:
            return
        self.pane.set_level(max(0, min(100, level)))

    def set_preset(self, level: int) -> None:
        self.pane.set_policy("continuous")
        self.pane.set_level(level)
        self.apply()

    def apply(self) -> None:
        self.backend.apply_fan_settings(self.settings())

    def reset(self) -> None:
        self.backend.reset_fan_settings(self.settings(reset=True))

    def set_supported(self, supported: bool) -> None:
        self.pane.set_supported_state(supported)
