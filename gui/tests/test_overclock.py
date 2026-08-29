from __future__ import annotations

import sys
import types


fan_control_stub = types.ModuleType("src.tabs.dashboard.sections.fan")


class FanControlPane:
    pass


fan_control_stub.FanControlPane = FanControlPane
sys.modules.setdefault("src.tabs.dashboard.sections.fan", fan_control_stub)

from src.tabs.dashboard.sections.overclock import OverclockTab  # noqa: E402


class FakeVar:
    def __init__(self, value: str) -> None:
        self.value = value

    def get(self) -> str:
        return self.value

    def set(self, value: str) -> None:
        self.value = value


class FakeConsole:
    def __init__(self) -> None:
        self.messages: list[str] = []

    def append(self, text: str) -> None:
        self.messages.append(text)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def set_clock_offset(
        self, gpu: str, backend: str, domain: str, offset: int, pstate: str
    ) -> None:
        self.calls.append(("set_clock_offset", gpu, backend, domain, offset, pstate))

    def set_volt_rail_target(
        self, gpu: str, rail_bit: int, target_mv: float, expect_type
    ) -> dict:
        # pynvoc's set_volt_rail_target always returns a dict (NOT None like
        # the other setters), so the apply path must format the message from
        # it rather than use `setter() or "msg"`. Mirror the real payload
        # (target_uV is an i32 µV integer in the real binding).
        self.calls.append(
            (
                "set_volt_rail_target",
                gpu,
                rail_bit,
                target_mv,
                expect_type,
            )
        )
        return {"applied": True, "effective_wall_uV": int(target_mv * 1000)}

    def set_tgp_watt(self, gpu: str, watts: int, policy_index: int) -> None:
        self.calls.append(("set_tgp_watt", gpu, watts, policy_index))

    def set_clk_domain_offset(
        self, gpu: str, domain_bit: int, offset_khz: int, slot, temporary
    ) -> dict:
        # Like set_volt_rail_target: always returns a dict (never None), so
        # the apply paths must format the message from it. Mirror the real
        # payload (applied_mHz is the driver readback, MHz).
        self.calls.append(
            (
                "set_clk_domain_offset",
                gpu,
                domain_bit,
                offset_khz,
                slot,
                temporary,
            )
        )
        return {"applied": True, "bit": domain_bit, "applied_mHz": offset_khz / 1000.0}

    def set_target_temp(self, gpu: str, tlimit: float, policy_index: int) -> None:
        self.calls.append(("set_target_temp", gpu, tlimit, policy_index))

    def set_pstate_native_lock(self, gpu: str, pstate: str) -> None:
        self.calls.append(("set_pstate_native_lock", gpu, pstate))

    def reset_pstate_native_lock(self, gpu: str) -> None:
        self.calls.append(("reset_pstate_native_lock", gpu))

    def set_nvml_pstate_lock(self, gpu: str, first: str, second: str) -> None:
        self.calls.append(("set_nvml_pstate_lock", gpu, first, second))

    def set_nvapi_pstate_lock(self, gpu: str, first: str, second: str) -> None:
        self.calls.append(("set_nvapi_pstate_lock", gpu, first, second))

    def reset_mem_clocks(self, gpu: str, backend: str) -> None:
        self.calls.append(("reset_mem_clocks", gpu, backend))


class FakeApp:
    def __init__(self, *, legacy_voltage: bool | None = False) -> None:
        self.console = FakeConsole()
        self.native = FakeNative()
        self.actions: list[str] = []
        # Per-GPU flags surfaced from get-gpu-list (see app.py
        # _populate_gpu_dropdown). None → unknown.
        self._gpu_flags_by_idx = {0: {"is_legacy_voltage": legacy_voltage}}

    def get_current_gpu_index(self) -> int | None:
        return 0

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def run_native_action(self, description: str, action, on_finished=None) -> bool:
        self.actions.append(description)
        action(self.native)
        if on_finished is not None:
            on_finished(0)
        return True

    def run_native_action_chain(self, commands) -> None:
        for description, action in commands:
            self.actions.append(description)
            action(self.native)


def make_tab(
    xbar_supported: bool = False,
    xbar_value: str = "10",
    legacy_voltage: bool | None = False,
) -> tuple[OverclockTab, FakeApp]:
    app = FakeApp(legacy_voltage=legacy_voltage)
    tab = OverclockTab.__new__(OverclockTab)
    tab.app = app
    tab._syncing = False
    tab._is_resize_active = False
    tab._pending_vfp_state = None
    tab._is_vfp_mode = False
    tab._vfp_uniform_offset_mhz = None
    tab.core_var = FakeVar("125")
    tab.mem_var = FakeVar("600")
    tab.core_slider = FakeSlider("normal")
    tab.mem_slider = FakeSlider("normal")
    tab.oc_api_var = FakeVar("NVAPI")
    tab._xbar_supported = xbar_supported
    tab.xbar_var = FakeVar(xbar_value)
    tab.xbar_slider = FakeSlider("normal")
    tab._supported_pstates = []
    return tab, app


def test_vfp_state_does_not_replace_core_offset_display() -> None:
    tab, _app = make_tab()

    tab.set_vfp_state(True, 50)

    assert tab._is_vfp_mode is True
    assert tab._vfp_uniform_offset_mhz == 50
    assert tab.core_var.get() == "125"


def test_core_apply_still_runs_while_vfp_offset_exists() -> None:
    tab, app = make_tab()
    tab.set_vfp_state(True, None)

    tab._apply_core_only()

    assert app.actions == ["apply core offset"]
    assert app.native.calls == [
        ("set_clock_offset", "GPU0", "nvapi", "core", 125, "P0")
    ]


def test_apply_offset_runs_core_and_memory_while_vfp_offset_exists() -> None:
    tab, app = make_tab()
    tab.set_vfp_state(True, 50)

    tab._apply_oc()

    assert app.actions == ["apply core offset", "apply memory offset"]
    assert app.native.calls == [
        ("set_clock_offset", "GPU0", "nvapi", "core", 125, "P0"),
        ("set_clock_offset", "GPU0", "nvapi", "memory", 600, "P0"),
    ]


# ── Volt Limit (mobile-only VoltRails target) ──────────────────────────────


class FakeSlider:
    """Minimal slider stub: only ``cget("state")`` is exercised by the
    mobile apply paths."""

    def __init__(self, state: str = "normal") -> None:
        self._state = state

    def cget(self, key: str) -> str:
        if key == "state":
            return self._state
        return ""

    def set(self, _value) -> None:
        pass


def make_mobile_tab(
    vlimit_value: str = "1085",
    vlimit_state: str = "normal",
    tlimit_value: str = "85",
    plimit_value: str = "30",
) -> tuple[OverclockTab, FakeApp]:
    """A mobile-mode tab with only the attrs the volt-rail apply paths touch."""
    app = FakeApp()
    tab = OverclockTab.__new__(OverclockTab)
    tab.app = app
    tab._syncing = False
    tab._mobile_mode = True
    tab._mobile_load_in_flight = (
        True  # short-circuit _load_mobile_limits() in on_finished
    )
    tab._volt_rail_bit = 0
    tab.vlimit_var = FakeVar(vlimit_value)
    tab.vlimit_slider = FakeSlider(vlimit_state)
    tab.tlimit_var = FakeVar(tlimit_value)
    tab.tlimit_slider = FakeSlider("normal")
    tab.plimit_var = FakeVar(plimit_value)
    tab.plimit_slider = FakeSlider("normal")
    tab._tgp_policy_index = 2
    return tab, app


def test_volt_limit_bounds_takes_min_of_walls() -> None:
    p0 = {
        "vbios_wall_uV": 1_210_000,
        "vrm_max_wall_uV": 1_180_000,
        "effective_wall_uV": 1_080_000,
    }
    assert OverclockTab._volt_limit_bounds_from_p0(p0) == (300.0, 1180.0, 1080.0)


def test_volt_limit_bounds_ignores_zero_wall() -> None:
    # VRM not reported (0) → ceiling is the VBIOS wall alone.
    p0 = {
        "vbios_wall_uV": 1_210_000,
        "vrm_max_wall_uV": 0,
        "effective_wall_uV": 1_080_000,
    }
    assert OverclockTab._volt_limit_bounds_from_p0(p0) == (300.0, 1210.0, 1080.0)


def test_volt_limit_bounds_falls_back_to_1200mv_when_both_zero() -> None:
    p0 = {"vbios_wall_uV": 0, "vrm_max_wall_uV": 0, "effective_wall_uV": 0}
    assert OverclockTab._volt_limit_bounds_from_p0(p0) == (300.0, 1200.0, 1200.0)


def test_volt_limit_bounds_clamps_position_into_range() -> None:
    # effective wall above the ceiling → clamped down to max.
    p0 = {
        "vbios_wall_uV": 1_100_000,
        "vrm_max_wall_uV": 1_100_000,
        "effective_wall_uV": 1_250_000,
    }
    assert OverclockTab._volt_limit_bounds_from_p0(p0) == (300.0, 1100.0, 1100.0)


def test_volt_limit_bounds_snaps_position_to_2_5mv_grid() -> None:
    # effective wall not on the 2.5 mV slider grid → snapped to nearest so
    # the one-decimal entry text and the canvas thumb agree (1083 mV → 1082.5).
    p0 = {
        "vbios_wall_uV": 1_210_000,
        "vrm_max_wall_uV": 1_180_000,
        "effective_wall_uV": 1_083_000,
    }  # 1083 mV → snaps to 1082.5
    assert OverclockTab._volt_limit_bounds_from_p0(p0) == (300, 1180, 1082.5)


def test_volt_limit_bounds_snaps_ceiling_down_to_grid() -> None:
    # A ceiling that is not on the 2.5 mV grid snaps DOWN so no offered slider
    # position exceeds the actual wall (1186 mV → 1185, not 1187.5).
    p0 = {
        "vbios_wall_uV": 1_186_000,
        "vrm_max_wall_uV": 0,
        "effective_wall_uV": 1_080_000,
    }
    min_mv, max_mv, _pos = OverclockTab._volt_limit_bounds_from_p0(p0)
    assert (min_mv, max_mv) == (300, 1185)


def test_resolve_volt_rail_bit_from_descriptors() -> None:
    vr = {"rail_descriptors": [{"rail_bit": 1, "type": 3}]}
    assert (
        OverclockTab._resolve_volt_rail_bit(OverclockTab.__new__(OverclockTab), vr) == 1
    )


def test_resolve_volt_rail_bit_falls_back_to_mask_lsb() -> None:
    vr = {"rail_mask": "0x00000001"}
    tab = OverclockTab.__new__(OverclockTab)
    assert tab._resolve_volt_rail_bit(vr) == 0


def test_apply_vlimit_only_calls_set_volt_rail_target() -> None:
    tab, app = make_mobile_tab(vlimit_value="1085")

    tab._apply_vlimit_only()

    assert app.actions == ["apply volt-rail target"]
    assert app.native.calls == [("set_volt_rail_target", "GPU0", 0, 1085.0, None)]


def test_apply_vlimit_only_accepts_decimal_mv() -> None:
    # 10/20-series rail step is 12.5 mV → one decimal (2.5 mV grid) must flow
    # through to the pynvoc setter as a float.
    tab, app = make_mobile_tab(vlimit_value="1082.5")

    tab._apply_vlimit_only()

    assert app.actions == ["apply volt-rail target"]
    assert app.native.calls == [("set_volt_rail_target", "GPU0", 0, 1082.5, None)]


def test_apply_vlimit_action_returns_str_not_dict() -> None:
    # Regression: set_volt_rail_target returns a dict (not None), so the
    # `setter() or "msg"` pattern yielded the dict and crashed the native
    # worker at output.endswith(). The action MUST format a string.
    tab, app = make_mobile_tab(vlimit_value="1085")
    captured = {}

    class CapturingApp(FakeApp):
        def run_native_action(self, description, action, on_finished=None):
            captured["output"] = action(self.native)
            super().run_native_action(description, action, on_finished)

    tab.app = CapturingApp()
    tab.app.console = app.console
    tab.app.native = app.native
    tab.app.actions = []

    tab._apply_vlimit_only()

    assert isinstance(captured["output"], str)


def test_apply_vlimit_only_skips_non_numeric() -> None:
    tab, app = make_mobile_tab(vlimit_value="abc")

    tab._apply_vlimit_only()

    assert app.actions == []
    assert app.native.calls == []


def test_format_volt_rail_target_result_surfaces_effective_wall() -> None:
    result = {"applied": True, "effective_wall_uV": 1_080_000}
    msg = OverclockTab._format_volt_rail_target_result(1085.0, result)
    assert "1085 mV" in msg
    assert "effective wall 1080 mV" in msg


def test_format_volt_rail_target_result_decimal_mv() -> None:
    # A half-mV target and a half-mV effective wall both render with the
    # decimal intact (not truncated to 1082 / 1083).
    result = {"applied": True, "effective_wall_uV": 1_082_500}
    msg = OverclockTab._format_volt_rail_target_result(1082.5, result)
    assert "1082.5 mV" in msg
    assert "effective wall 1082.5 mV" in msg


def test_format_volt_rail_target_result_unsupported() -> None:
    # set_volt_rail_target returns {"supported": False} when the driver
    # exposes no VoltRails path. This MUST yield a string (not the dict),
    # or the native worker's output.endswith() crashes.
    msg = OverclockTab._format_volt_rail_target_result(1085.0, {"supported": False})
    assert isinstance(msg, str)
    assert "not supported" in msg


def test_apply_limits_mobile_includes_volt_rail() -> None:
    tab, app = make_mobile_tab(vlimit_value="1085")

    tab._apply_limits()

    # All three mobile limit actions are chained.
    assert "apply volt-rail target" in app.actions
    assert app.native.calls[-1] == ("set_volt_rail_target", "GPU0", 0, 1085.0, None)


def test_apply_limits_mobile_skips_disabled_volt_slider() -> None:
    tab, app = make_mobile_tab(vlimit_value="1085", vlimit_state="disabled")

    tab._apply_limits()

    assert not any(call[0] == "set_volt_rail_target" for call in app.native.calls)


# ── Xbar (ClockClient domain offset, NVAPI-only) ───────────────────────────


def test_xbar_supported_arch_chip_codes() -> None:
    # pynvoc ArchInfo Display: chip codes, optionally ":rev"-suffixed.
    f = OverclockTab._xbar_supported_arch
    assert f("tu106") is True  # Turing: GTX 16系 + RTX 20系
    assert f("tu117") is True
    assert f("ad107m") is True  # Ada (40系 laptop)
    assert f("AD107M:A1") is True
    assert f("ga102") is True  # Ampere (30系)
    assert f("gb202") is True  # Blackwell (50系)
    assert f("gp104") is False  # Pascal (10系) — too old
    assert f("gv100") is False  # Volta — too old
    assert f("gm204") is False
    assert f("") is False


def test_xbar_supported_arch_friendly_names() -> None:
    # CLI human output: friendly architecture names.
    f = OverclockTab._xbar_supported_arch
    assert f("Turing") is True
    assert f("Ampere") is True
    assert f("Ada") is True
    assert f("Blackwell") is True
    assert f("Pascal") is False
    assert f("Maxwell") is False


def test_apply_xbar_only_converts_mhz_to_khz() -> None:
    # GUI speaks MHz; pynvoc takes kHz. xbar = ClockClient domain bit 1.
    tab, app = make_tab(xbar_value="10")

    tab._apply_xbar_only()

    assert app.actions == ["apply xbar offset"]
    assert app.native.calls == [
        ("set_clk_domain_offset", "GPU0", 1, 10_000, None, None)
    ]


def test_apply_xbar_action_returns_str_not_dict() -> None:
    # Regression: set_clk_domain_offset returns a dict (not None), so the
    # action MUST format a string or the native worker's output.endswith()
    # crashes (same class of bug as the volt-rail one).
    tab, app = make_tab(xbar_value="10")
    captured = {}

    class CapturingApp(FakeApp):
        def run_native_action(self, description, action, on_finished=None):
            captured["output"] = action(self.native)
            super().run_native_action(description, action, on_finished)

    tab.app = CapturingApp()
    tab.app.console = app.console
    tab.app.native = app.native
    tab.app.actions = []

    tab._apply_xbar_only()

    assert isinstance(captured["output"], str)
    assert "10 MHz" in captured["output"]


def test_format_xbar_offset_result_surfaces_readback() -> None:
    result = {"applied": True, "applied_kHz": 15_000}
    msg = OverclockTab._format_xbar_offset_result(15, result)
    assert "+15 MHz" in msg
    assert "readback +15 MHz" in msg


def test_format_xbar_offset_result_unsupported() -> None:
    msg = OverclockTab._format_xbar_offset_result(10, {"supported": False})
    assert isinstance(msg, str)
    assert "not supported" in msg


def test_apply_oc_includes_xbar_when_supported() -> None:
    tab, app = make_tab(xbar_supported=True, xbar_value="10")

    tab._apply_oc()

    assert "apply xbar offset" in app.actions
    assert ("set_clk_domain_offset", "GPU0", 1, 10_000, None, None) in [
        c for c in app.native.calls if c[0] == "set_clk_domain_offset"
    ]


def test_apply_oc_skips_xbar_when_unsupported() -> None:
    tab, app = make_tab(xbar_supported=False)

    tab._apply_oc()

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_apply_oc_skips_xbar_when_disabled() -> None:
    # NVML selection greys the row out -> Apply Section must skip it.
    tab, app = make_tab(xbar_supported=True, xbar_value="10")
    tab.xbar_slider = FakeSlider("disabled")

    tab._apply_oc()

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_reset_oc_resets_xbar_when_supported() -> None:
    tab, app = make_tab(xbar_supported=True, xbar_value="10")

    tab._reset_oc()

    assert tab.xbar_var.get() == "0"
    assert ("set_clk_domain_offset", "GPU0", 1, 0, None, None) in [
        c for c in app.native.calls if c[0] == "set_clk_domain_offset"
    ]


def test_xbar_supported_from_info_prefers_payload_flag() -> None:
    # The pynvoc query_info payload carries xbar_supported computed by core's
    # detect_gpu_type — the single source of truth. It wins even when the
    # arch string would confuse a heuristic (Ada reports 'Unknown:...').
    f = OverclockTab._xbar_supported_from_info
    info = {
        "xbar_supported": True,
        "gpu_architecture": "Unknown:400:7:161",
        "codename": "AD107-B",
        "gpu_name": "NVIDIA GeForce RTX 4060 Laptop GPU",
    }
    assert f(info) is True
    # Flag False wins too (e.g. Pascal with a misleading name).
    assert f({**info, "xbar_supported": False}) is False


def test_xbar_supported_from_info_falls_back_without_flag() -> None:
    # CLI-parsed / NVML-only / older-pynvoc payloads lack the flag → the
    # local heuristic (codename first, then arch, then marketing name).
    f = OverclockTab._xbar_supported_from_info
    assert f({"codename": "AD107-B", "gpu_name": "RTX 4060 Laptop"}) is True
    assert f({"gpu_architecture": "tu116"}) is True
    assert f({"gpu_name": "NVIDIA GeForce GTX 1080"}) is False
    assert f({}) is False


def test_core_apply_accepts_decimal_mhz() -> None:
    # 2.5 MHz grid: one decimal flows through to pynvoc as a float
    # (7.5/12.5 MHz hardware steps both divide the grid).
    tab, app = make_tab()
    tab.core_var = FakeVar("122.5")

    tab._apply_core_only()

    assert app.actions == ["apply core offset"]
    assert app.native.calls == [
        ("set_clock_offset", "GPU0", "nvapi", "core", 122.5, "P0")
    ]


def test_apply_oc_core_decimal_survives_section_apply() -> None:
    tab, app = make_tab()
    tab.core_var = FakeVar("122.5")

    tab._apply_oc()

    assert ("set_clock_offset", "GPU0", "nvapi", "core", 122.5, "P0") in (
        app.native.calls
    )


def test_fmt_slider_value_signs_decimals_rows_and_zero() -> None:
    # Core offset row: signed + decimals=1 → explicit sign on positives AND
    # zero (+0.0), matching the integer signed rows' +0.
    class SignedDecimalSlider:
        _oc_decimals = 1
        _oc_signed = True

    class SignedIntSlider:
        _oc_decimals = 0
        _oc_signed = True

    class PlainSlider:
        _oc_decimals = 0
        _oc_signed = False

    f = OverclockTab._fmt_slider_value
    assert f(SignedDecimalSlider(), 122.5) == "+122.5"
    assert f(SignedDecimalSlider(), 0) == "+0.0"
    assert f(SignedDecimalSlider(), -25.0) == "-25.0"
    assert f(SignedIntSlider(), 0) == "+0"
    assert f(PlainSlider(), 0) == "0"


class _FakePstateSelector:
    """Minimal SegmentRangeSelector stub: holds the selected (start, end)."""

    def __init__(self, selection: tuple[str, str], point_mode: bool = False) -> None:
        self._selection = selection
        self.point_mode = point_mode

    def get_selection(self) -> tuple[str, str] | None:
        return self._selection

    def set_point_mode(self, enabled: bool) -> None:
        self.point_mode = bool(enabled)

    def set_values(self, _values) -> None:  # pragma: no cover - not exercised
        pass


def test_pstate_lock_legacy_uses_native_point_lock() -> None:
    # Legacy GPU (Maxwell/Kepler/Fermi): must call the native single-P-State
    # pin, NOT the mem-range lock — the mem-range path needs NVML P-State
    # memory-clock ranges that are Not Supported on e.g. Fermi.
    tab, app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P8", "P8"))

    tab._apply_pstate_lock()

    assert app.actions == ["apply P-State lock"]
    assert app.native.calls == [("set_pstate_native_lock", "GPU0", "P8")]


def test_pstate_lock_legacy_reset_uses_native_reset() -> None:
    tab, app = make_tab(legacy_voltage=True)

    tab._unlock_pstate_lock()

    assert app.actions == ["reset P-State lock"]
    assert app.native.calls == [("reset_pstate_native_lock", "GPU0")]


def test_pstate_lock_legacy_with_nvml_available_still_native() -> None:
    # Regression: the fallback is gated on legacy GENERATION, not NVML
    # availability — after the NVSMI path fix made NVML load on the GT730,
    # a backend_nvml-based gate flipped this back to the mem-range lock.
    tab, app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P8", "P8"))

    tab._apply_pstate_lock()

    assert app.native.calls == [("set_pstate_native_lock", "GPU0", "P8")]


def test_pstate_lock_modern_uses_mem_range() -> None:
    # Modern GPU: original range-lock path (point-mode OFF, range slider).
    tab, app = make_tab(legacy_voltage=False)
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))

    tab._apply_pstate_lock()

    assert app.actions == ["apply P-State lock"]
    # NVAPI backend selected → set_nvapi_pstate_lock (mem-range derivation).
    assert app.native.calls == [("set_nvapi_pstate_lock", "GPU0", "P0", "P2")]


def test_pstate_point_mode_set_on_legacy() -> None:
    # When the P-State list updates on a legacy GPU, the selector must be
    # fused into point-mode (single-P-State pin has no range form).
    tab, _app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P8", "P8"))
    # set_supported_pstates also pokes the apply/unlock buttons.
    tab.btn_apply_pstate = FakeSlider("normal")
    tab.btn_unlock_pstate = FakeSlider("normal")

    tab.set_supported_pstates(["P0", "P8", "P12"])

    assert tab.pstate_selector.point_mode is True


def test_pstate_range_mode_preserved_on_modern_gpu() -> None:
    tab, _app = make_tab(legacy_voltage=False)
    tab.pstate_selector = _FakePstateSelector(("P0", "P8"))
    tab.btn_apply_pstate = FakeSlider("normal")
    tab.btn_unlock_pstate = FakeSlider("normal")

    tab.set_supported_pstates(["P0", "P8", "P12"])

    assert tab.pstate_selector.point_mode is False
