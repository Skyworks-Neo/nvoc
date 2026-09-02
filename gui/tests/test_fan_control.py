from __future__ import annotations

from src.backend.base import FanSettings
from src.tabs.dashboard.sections.fan import FanControlController


class FakePane:
    def __init__(
        self,
        *,
        api: str = "NVAPI",
        fan_id: str = "All",
        policy: str = "continuous",
        level: int = 60,
    ) -> None:
        self.api = api
        self.fan_id = fan_id
        self.policy = policy
        self.level = level
        self.level_entry = str(level)
        self.policy_values: list[str] = []
        self.supported = True

    def selected_api(self) -> str:
        return self.api

    def selected_fan_id(self) -> str:
        return self.fan_id

    def selected_policy(self) -> str:
        return self.policy

    def fan_level(self) -> int:
        return self.level

    def fan_level_text(self) -> str:
        return self.level_entry

    def set_policy_values(self, values) -> None:
        self.policy_values = list(values)

    def set_policy(self, policy: str) -> None:
        self.policy = policy

    def set_level(self, level: int) -> None:
        self.level = level
        self.level_entry = str(level)

    def set_supported_state(self, supported: bool) -> None:
        self.supported = supported


class FakeBackend:
    def __init__(self) -> None:
        self.applied: list[FanSettings] = []
        self.reset: list[FanSettings] = []

    def apply_fan_settings(self, settings: FanSettings) -> None:
        self.applied.append(settings)

    def reset_fan_settings(self, settings: FanSettings) -> None:
        self.reset.append(settings)


def test_fan_apply_uses_nvapi_all_fans() -> None:
    pane = FakePane(api="NVAPI", fan_id="All", policy="continuous", level=70)
    backend = FakeBackend()

    FanControlController(pane, backend).apply()

    assert backend.applied == [
        FanSettings(
            backend="nvapi-cooler",
            fan_id=None,
            policy="continuous",
            level=70,
        )
    ]


def test_fan_apply_uses_nvml_specific_fan() -> None:
    pane = FakePane(api="NVML", fan_id="Fan 2", policy="manual", level=45)
    backend = FakeBackend()

    FanControlController(pane, backend).apply()

    assert backend.applied == [
        FanSettings(
            backend="nvml-cooler",
            fan_id="2",
            policy="manual",
            level=45,
        )
    ]


def test_fan_reset_uses_auto_policy_and_zero_level() -> None:
    pane = FakePane(api="NVML", fan_id="Fan 1", policy="manual", level=45)
    backend = FakeBackend()

    FanControlController(pane, backend).reset()

    assert backend.reset == [
        FanSettings(
            backend="nvml-cooler",
            fan_id="1",
            policy="auto",
            level=0,
        )
    ]


def test_backend_change_normalizes_invalid_policy() -> None:
    pane = FakePane(api="NVML", policy="perf")

    FanControlController(pane, FakeBackend()).on_backend_change()

    assert pane.policy_values == ["continuous", "manual"]
    assert pane.policy == "continuous"


def test_preset_sets_level_and_applies() -> None:
    pane = FakePane(policy="manual", level=60)
    backend = FakeBackend()

    FanControlController(pane, backend).set_preset(30)

    assert pane.policy == "continuous"
    assert pane.level == 30
    assert backend.applied == [
        FanSettings(
            backend="nvapi-cooler",
            fan_id=None,
            policy="continuous",
            level=30,
        )
    ]


def test_entry_change_updates_applied_level() -> None:
    pane = FakePane(level=60)
    pane.level_entry = "80"
    backend = FakeBackend()
    controller = FanControlController(pane, backend)

    controller.on_entry_change()
    controller.apply()

    assert pane.level == 80
    assert backend.applied == [
        FanSettings(
            backend="nvapi-cooler",
            fan_id=None,
            policy="continuous",
            level=80,
        )
    ]


def test_entry_change_clamps_level() -> None:
    pane = FakePane(level=60)
    pane.level_entry = "125"

    FanControlController(pane, FakeBackend()).on_entry_change()

    assert pane.level == 100


def test_modern_nvapi_policy_list_is_continuous_only() -> None:
    # Live A/B: on modern GPUs only `continuous` (the TemperatureContinuous SW
    # curve policy) actually applies the manual % level via NVAPI — `manual`
    # no-ops on the modern cooler paths, so it is not offered at all. The old
    # 8-entry enum list (default/perf/... / no-op or rejected) must not
    # reappear.
    pane = FakePane(api="NVAPI", policy="default")
    controller = FanControlController(pane, FakeBackend())

    controller.on_backend_change()

    assert pane.policy_values == ["continuous"]
    assert pane.policy == "continuous"
    assert controller.settings().policy == "continuous"


def test_modern_nvapi_normalizes_manual_back_to_continuous() -> None:
    # A selection carried over from the legacy list (or NVML) normalizes to
    # continuous — the only policy that applies on modern NVAPI coolers.
    pane = FakePane(api="NVAPI", policy="manual")
    controller = FanControlController(pane, FakeBackend())

    controller.on_backend_change()

    assert pane.policy == "continuous"
    assert controller.settings().policy == "continuous"


def test_legacy_nvapi_restricts_policy_to_default_manual() -> None:
    # ≤ Kepler: the modern CoolerPolicy types are rejected by the old driver;
    # manual % lands on `manual` there, NOT continuous.
    pane = FakePane(api="NVAPI", policy="continuous")
    controller = FanControlController(pane, FakeBackend())

    controller.set_legacy_nvapi(True)

    assert pane.policy_values == ["default", "manual"]
    assert pane.policy == "manual"
    assert controller.settings().policy == "manual"
