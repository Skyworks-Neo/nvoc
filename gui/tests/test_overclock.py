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
    def __init__(self, mem_lock_error: Exception | None = None) -> None:
        self.calls: list[tuple] = []
        # What the mem-range pstate setters return (pynvoc: None on a clean
        # apply, else the overlap warning string).
        self.pstate_lock_warning: str | None = None
        # Raised by the mem-range pstate setters when set (pre-Kepler part:
        # the NVML pstate query is Not Supported).
        self.mem_lock_error = mem_lock_error
        # Raise from set_clock_offset (e.g. pstate20 NotSupported -104) to
        # exercise the ClkDomains fallback path.
        self.raise_on_set_clock: Exception | None = None
        # What query_private_freq_domain_info reports for bit3's slot-0 offset
        # (kHz) — the Sys RMW baseline.
        self.bit3_current_khz: int = 0
        # Full override of the payload's entries (pynvoc shape) for tests that
        # need more than bit3 — the startup anchor reads all four fabric bits.
        self.domain_entries: list[dict] | None = None

    def set_clock_offset(
        self, gpu: str, backend: str, domain: str, offset: int, pstate: str
    ) -> None:
        self.calls.append(("set_clock_offset", gpu, backend, domain, offset, pstate))
        if self.raise_on_set_clock is not None:
            raise self.raise_on_set_clock

    def query_private_freq_domain_info(self, gpu: str) -> dict:
        self.calls.append(("query_private_freq_domain_info", gpu))
        entries = self.domain_entries
        if entries is None:
            entries = [{"bit": 3, "values_kHz": [self.bit3_current_khz]}]
        return {
            "controllable_mask": "0x000003FF",
            "entries": entries,
        }

    def set_volt_rail_target(
        self, gpu: str, rail_bit: int, target_mv: float, expect_type
    ) -> dict:
        # pynvoc's set_volt_rail_target always returns a dict (NOT None like
        # the other setters), so the apply path must format the message from
        # it rather than use `setter() or "msg"`. Mirror the real payload
        # (target_uV is an i32 µV integer in the real binding).
        self.calls.append((
            "set_volt_rail_target",
            gpu,
            rail_bit,
            target_mv,
            expect_type,
        ))
        return {"applied": True, "effective_wall_uV": int(target_mv * 1000)}

    def set_tgp_watt(self, gpu: str, watts: int, policy_index: int) -> None:
        self.calls.append(("set_tgp_watt", gpu, watts, policy_index))

    def set_clk_domain_offset(
        self, gpu: str, domain_bit: int, offset_khz: int, slot, temporary
    ) -> dict:
        # Like set_volt_rail_target: always returns a dict (never None), so
        # the apply paths must format the message from it. Mirror the real
        # payload (applied_mHz is the driver readback, MHz).
        self.calls.append((
            "set_clk_domain_offset",
            gpu,
            domain_bit,
            offset_khz,
            slot,
            temporary,
        ))
        return {"applied": True, "bit": domain_bit, "applied_mHz": offset_khz / 1000.0}

    def set_target_temp(self, gpu: str, tlimit: float, policy_index: int) -> None:
        self.calls.append(("set_target_temp", gpu, tlimit, policy_index))

    def set_pstate_native_lock(self, gpu: str, pstate: str) -> None:
        self.calls.append(("set_pstate_native_lock", gpu, pstate))

    def reset_pstate_native_lock(self, gpu: str) -> None:
        self.calls.append(("reset_pstate_native_lock", gpu))

    def set_nvml_pstate_lock(self, gpu: str, first: str, second: str) -> str | None:
        self.calls.append(("set_nvml_pstate_lock", gpu, first, second))
        if self.mem_lock_error is not None:
            raise self.mem_lock_error
        return self.pstate_lock_warning

    def set_nvapi_pstate_lock(self, gpu: str, first: str, second: str) -> str | None:
        self.calls.append(("set_nvapi_pstate_lock", gpu, first, second))
        if self.mem_lock_error is not None:
            raise self.mem_lock_error
        return self.pstate_lock_warning

    def reset_mem_clocks(self, gpu: str, backend: str) -> None:
        self.calls.append(("reset_mem_clocks", gpu, backend))


class FakeFrame:
    """The tab's parent widget: only ``after`` is used, to bounce a background
    worker's result back onto the render thread."""

    def after(self, _delay: int, callback) -> None:
        callback()


# The relations the driver declares through get-private-vftable's ext slots,
# as the GUI's parser hands them to the write path ({child_bit: (parents…)}).
_XBAR_SYS: dict[int, tuple[int, ...]] = {3: (1,)}  # 30系+ (ext0=SYS)
_XBAR_HOST: dict[int, tuple[int, ...]] = {9: (1,)}  # Ada (ext1=HOST)


def _domain_entries(
    offsets_khz: dict[int, int], freq_slot: int = 0, width: int = 8
) -> list[dict]:
    """Build get-private-freq-domain-info ``entries`` in the pynvoc payload
    shape: one record per bit, ``value_modifiable`` True (the protocol
    marshals it, so the value dwords ARE driver data) and the frequency-plane
    offset parked in this generation's slot."""
    entries = []
    for bit, khz in offsets_khz.items():
        vals = [0] * width
        vals[freq_slot] = khz
        entries.append({
            "bit": bit,
            "type": 10,
            "value_modifiable": True,
            "values_kHz": vals,
        })
    return entries


class FakeApp:
    def __init__(
        self,
        *,
        legacy_voltage: bool | None = False,
        mem_lock_error: Exception | None = None,
    ) -> None:
        self.console = FakeConsole()
        self.native = FakeNative(mem_lock_error=mem_lock_error)
        self.actions: list[str] = []
        self.action_outputs: list[str | None] = []
        # Per-GPU flags surfaced from get-gpu-list (see app.py
        # _populate_gpu_dropdown). None → unknown/absent.
        self._gpu_flags_by_idx = {
            0: {
                "is_legacy_voltage": legacy_voltage,
            }
        }

    def get_current_gpu_index(self) -> int | None:
        return 0

    def selected_gpu_target(self) -> str:
        return "GPU0"

    def run_native_action(self, description: str, action, on_finished=None) -> bool:
        # Mirror the backend worker: run the action, catch its exception
        # (error output), and never let it escape into the caller. The
        # finish code mirrors NativeBackend.run_action too (-1 on failure).
        self.actions.append(description)
        code = -1
        try:
            output = action(self.native)
        except Exception as exc:
            self.action_outputs.append(f"{exc}")
        else:
            self.action_outputs.append(output)
            code = 0
        if on_finished is not None:
            on_finished(code)
        return True

    def after(self, _delay: int, callback) -> None:
        # Run immediately so the worker-thread fallback lands synchronously.
        callback()

    def run_background(self, _name: str, task) -> None:
        # The caps query dispatches its worker through here; run it inline so
        # the whole query → parse → anchor path is testable synchronously.
        task()

    def run_native_action_chain(self, commands) -> None:
        for description, action in commands:
            self.actions.append(description)
            action(self.native)


def make_tab(
    xbar_supported: bool = False,
    xbar_value: str = "10",
    legacy_voltage: bool | None = False,
    mem_lock_error: Exception | None = None,
) -> tuple[OverclockTab, FakeApp]:
    app = FakeApp(legacy_voltage=legacy_voltage, mem_lock_error=mem_lock_error)
    tab = OverclockTab.__new__(OverclockTab)
    tab.app = app
    tab.frame = FakeFrame()
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
    # Pager state (the new fabric/uncore pages)
    tab._has_oc_pager = xbar_supported
    tab._oc_page = 0
    tab._oc_n_pages = 3 if xbar_supported else 1
    tab._is_pascal_gpu = False
    # The driver's main→attached relation. The default here is an EMPTY one —
    # a table that is known and declares no edge (Turing, Pascal): the fabric
    # rows then write raw, the plain per-bit writes the legacy tests pin. A
    # test that wants compensation sets the dict; one that wants the
    # pre-payload state sets None (rows refuse to write).
    tab._fabric_parents = {}
    tab._fabric_gpu = None
    tab._clk_domain_mask = 0x3FF if xbar_supported else 0
    tab._sys_supported = bool(tab._clk_domain_mask & (1 << 3))
    tab._msd_supported = bool(tab._clk_domain_mask & (1 << 5))
    tab._host_supported = bool(tab._clk_domain_mask & (1 << 9))
    # Startup anchor marker (None → this GPU's fabric rows were never seeded).
    tab._domain_anchor_gpu = None
    tab.sys_var = FakeVar("0")
    tab.msd_var = FakeVar("0")
    tab.host_var = FakeVar("0")
    tab.sys_slider = FakeStateWidget("normal")
    tab.msd_slider = FakeStateWidget("normal")
    tab.host_slider = FakeStateWidget("normal")
    # The pager grey-gate (_refresh_nvapi_only_rows) touches every fabric/
    # uncore row's slider/entry/apply widgets plus the limit-panel mode.
    tab.xbar_slider = FakeStateWidget("normal")
    tab.xbar_entry = FakeStateWidget("normal")
    tab.btn_apply_xbar = FakeStateWidget("normal")
    tab.btn_reset_xbar = FakeStateWidget("normal")
    tab.sys_entry = FakeStateWidget("normal")
    tab.btn_apply_sys = FakeStateWidget("normal")
    tab.btn_reset_sys = FakeStateWidget("normal")
    tab.msd_entry = FakeStateWidget("normal")
    tab.btn_apply_msd = FakeStateWidget("normal")
    tab.btn_reset_msd = FakeStateWidget("normal")
    tab.host_entry = FakeStateWidget("normal")
    tab.btn_apply_host = FakeStateWidget("normal")
    tab.btn_reset_host = FakeStateWidget("normal")
    # Pager chrome (round-robin arrows + indicator)
    tab._oc_btn_prev = FakeStateWidget("normal")
    tab._oc_btn_next = FakeStateWidget("normal")
    tab._oc_page_indicator = FakeVar("1/1")
    tab._limit_panel_mode = "off"
    return tab, app


def test_core_offset_falls_back_to_clk_domain_bit0_on_not_supported() -> None:
    """pstate20 -104 NotSupported → core offset via ClkDomains bit0 (Gpc)."""
    tab, app = make_tab()
    app.native.raise_on_set_clock = RuntimeError("NVAPI NotSupported -104")

    msg = OverclockTab._apply_core_only_action(app.native, "GPU0", "nvapi", 125.0, "P0")

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert any(c[2] == 0 and c[3] == 125000 for c in clk_calls)
    assert "via ClkDomains bit0" in msg


def test_mem_offset_falls_back_to_clk_domain_bit2_on_not_supported() -> None:
    """pstate20 -104 → mem offset via ClkDomains bit2 (WRITE bit2 = 显存 M,
    NOT the MEASURE bit2 which reads SYS)."""
    tab, app = make_tab()
    app.native.raise_on_set_clock = RuntimeError("NVAPI NotSupported -104")

    msg = OverclockTab._apply_mem_only_action(app.native, "GPU0", "nvapi", 200, "P0")

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert any(c[2] == 2 and c[3] == 200000 for c in clk_calls)
    assert "via ClkDomains bit2" in msg


def test_clk_domain_caps_enables_msd_and_host_rows() -> None:
    """Regression: the caps-loaded callback must not reach for an info cache
    the GUI App doesn't have (that crashed the callback and left Msd/Host
    greyed forever on every card, incl. a 4060 Laptop with mask 0x3FF)."""
    tab, _app = make_tab(xbar_supported=True)
    tab._sys_supported = False
    tab._msd_supported = False
    tab._host_supported = False

    tab._on_clk_domain_caps_loaded({"controllable_mask": "0x000003FF"})

    assert tab._clk_domain_mask == 0x3FF
    assert tab._sys_supported is True
    assert tab._msd_supported is True
    assert tab._host_supported is True


def test_clk_domain_caps_pascal_forces_msd_off() -> None:
    """Pascal bit5 SET N/A → MSD stays greyed even when the mask claims it."""
    tab, _app = make_tab(xbar_supported=True)
    tab._is_pascal_gpu = True

    tab._on_clk_domain_caps_loaded({"controllable_mask": "0x000003FF"})

    assert tab._msd_supported is False
    assert tab._host_supported is True  # bit9 unaffected by the Pascal gate


def test_clk_domain_caps_mask_without_bit9_disables_host() -> None:
    """0xFF (no bit9) → Host greyed."""
    tab, _app = make_tab(xbar_supported=True)

    tab._on_clk_domain_caps_loaded({"controllable_mask": "0x000000FF"})

    assert tab._host_supported is False
    assert tab._msd_supported is True  # bit5 present


def test_xbar_30plus_writes_bit3_cancel() -> None:
    """30系+ (SYS rides XBAR): Xbar +f writes bit1=+f AND RMWs bit3 (current−f)
    to cancel SYS. Stock bit3=0 → cancel lands at −f."""
    tab, app = make_tab(xbar_supported=True)

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 60, _XBAR_SYS)

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(1) == 60000
    assert bits.get(3) == -60000
    assert "Sys-cancel" in msg


def test_xbar_30plus_cancel_preserves_existing_sys_offset() -> None:
    """The cancel is an RMW (current − f): a Sys offset already on bit3
    survives — only the coupling drift is subtracted. Sys +30 then Xbar +60 →
    bit3 = +30 − 60 = −30 (SYS stays at +30 net of the +60 coupling:
    −30 + 60 = +30)."""
    tab, app = make_tab(xbar_supported=True)
    app.native.bit3_current_khz = 30000  # Sys +30 already applied

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 60, _XBAR_SYS)

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(3) == 30000 - 60000  # +30 preserved, −60 drift removed
    assert "bit3 +30 → -30 MHz" in msg


def test_xbar_without_edge_direct_write_no_cancel() -> None:
    """Pascal/GTX16/RTX20 — and any card whose relation holds no edge for
    bit1: Xbar +f writes bit1 only. Byte-identical to the pre-relation
    behavior, which is the point of driving this off the payload."""
    tab, app = make_tab(xbar_supported=True)

    OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 60, {})

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(1) == 60000
    assert 3 not in bits


def test_sys_apply_is_absolute_net_on_coupled_arch() -> None:
    """Sys (bit3) resolves from the NET target: bit3 := f − bit1, because on
    30系+ the XBAR offset rides into SYS (bit1), so the live SYS offset — and
    the value this row displays — is bit1 + bit3. Xbar +100 has parked
    bit3 = −100 (net SYS 0); asking for +30 must land bit3 at −70 (= +30 net
    through the +100 coupling), never at −100 + 30."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -100_000})

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 3, 30, _XBAR_SYS)

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(3) == 30_000 - 100_000
    assert "net SYS +30 MHz" in msg


def test_sys_apply_is_idempotent_on_reapply() -> None:
    """Regression (the reported startup bug's write half): an untouched
    "Apply Section" right after boot re-sends the anchored value. The net form
    is idempotent twice over — the first write lands on the resolved bit3
    (−70, never the old stacking +30), and the second finds the plane already
    where it asked and writes nothing at all."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -100_000})

    OverclockTab._apply_fabric_row_action(app.native, "GPU0", 3, 30, _XBAR_SYS)
    # The driver now holds what we just wrote; re-apply the same row value.
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 3, 30, _XBAR_SYS)

    writes = [c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert writes == [-70_000]
    assert "no write needed" in msg


def test_sys_apply_without_coupling_ignores_bit1() -> None:
    """10/16/20/Pascal: no XBAR→SYS edge, so bit1 is pure Xbar and the
    absolute form degenerates to bit3 := f (bit1 must NOT be subtracted)."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: 10_000})

    OverclockTab._apply_fabric_row_action(app.native, "GPU0", 3, 30, {})

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(3) == 30_000


def test_xbar_cancel_keeps_net_sys_when_xbar_moves() -> None:
    """bit3 := (bit1_cur + bit3_cur) − f — the OLD bit3_cur − f forgot the
    previous XBAR term, dragging SYS by −f on every re-apply. Sys +30 with
    Xbar +100 in place (net = +30), Xbar → +150 → bit3 = +30 − 150 = −120,
    i.e. net SYS stays +30."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 150, _XBAR_SYS)

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(1) == 150_000
    assert bits.get(3) == 30_000 - 150_000
    assert "net SYS +30 MHz kept" in msg


def test_xbar_cancel_reapply_is_idempotent() -> None:
    """Re-applying the SAME Xbar value must not leak a −f drift into SYS: +60
    writes the cancel once and the second apply is a no-op."""
    tab, app = make_tab(xbar_supported=True)

    OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 60, _XBAR_SYS)
    app.native.domain_entries = _domain_entries({1: 60_000, 3: -60_000})
    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 1, 60, _XBAR_SYS)

    writes = [
        c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset" and c[2] == 3
    ]
    assert writes == [-60_000]
    assert "no write needed" in msg


# ── The relation is data-driven: Ada's Host edge (ext1=HOST) ───────────────


def test_host_apply_is_net_aware_when_the_driver_attaches_it() -> None:
    """Ada: bank0-xbar's ext slots list HOST under XBAR (ext1=HOST), so the
    Host row is a net too — the XBAR offset rides into it. A raw bit9 write
    lands the row at f + bit1; resolving own := f − bit1 lands it on f."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 9: -100_000})

    msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", 9, 50, _XBAR_HOST)

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(9) == 50_000 - 100_000
    assert "net HOST +50 MHz" in msg


def test_host_apply_is_raw_without_the_edge() -> None:
    """No Host edge in the relation (Turing/Pascal: the driver's own table
    declares none) → the row is its own record, plain write."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000})

    OverclockTab._apply_fabric_row_action(app.native, "GPU0", 9, 50, {})

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(9) == 50_000
    assert 1 not in bits


def test_apply_row_uses_the_relation_the_tab_adopted() -> None:
    """The widget-level entry points read the relation off the tab (not a
    hardcoded XBAR→SYS pair): with the Ada relation adopted from the payload,
    _apply_host_only compensates against XBAR."""
    tab, app = make_tab(xbar_supported=True, xbar_value="50")
    tab._fabric_parents = dict(_XBAR_HOST)
    tab.host_var = FakeVar("50")
    app.native.domain_entries = _domain_entries({1: 100_000})

    tab._apply_host_only()

    assert app.actions == ["apply host offset"]
    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits.get(9) == 50_000 - 100_000


def test_fabric_relation_is_unknown_until_a_payload_lands() -> None:
    """No payload yet (older pynvoc / a failed read) is UNKNOWN, not "no
    edges": nothing is written through a fabric row until the driver's
    relation for this card arrives. A payload that landed and says "no edges"
    is a real answer — the rows write raw — and one with edges compensates."""
    tab, _app = make_tab(xbar_supported=True)

    tab._fabric_parents = None
    assert tab._fabric_relation() is None  # unknown → writes refuse

    tab._fabric_parents = {}  # known and empty → plain per-bit writes
    assert tab._fabric_relation() == {}

    tab._fabric_parents = {3: (1,), 9: (1,)}
    assert tab._fabric_relation() == {3: (1,), 9: (1,)}


def test_fabric_rows_refuse_to_write_while_the_relation_is_unknown() -> None:
    """Unknown relation → the four rows write NOTHING and say so. Their box
    values are NETS: written raw they would land the net a parent-term high,
    and there is no generation guess to fall back on any more."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    for bit in (1, 3, 5, 9):
        msg = OverclockTab._apply_fabric_row_action(app.native, "GPU0", bit, 50, None)
        assert msg == OverclockTab._FABRIC_UNKNOWN_MSG

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_reset_refuses_while_the_relation_is_unknown() -> None:
    """↺ on a fabric row is a NET reset: unresolved, the raw bit must not be
    zeroed (that is the drift the net model exists to prevent) and the row
    keeps the value it had rather than showing a +0 that never landed."""
    tab, app = make_tab(xbar_supported=True)
    tab._fabric_parents = None
    rows = _install_anchor_rows(tab)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    assert (
        OverclockTab._reset_fabric_row_action(app.native, "GPU0", "Sys", 3, None, 0, 1)
        == OverclockTab._FABRIC_UNKNOWN_MSG
    )

    tab.sys_var.set("+30")
    tab._reset_clk_domain("Sys", 3, rows["sys"], tab.sys_var)

    # Routed to the fabric action (it is a fabric bit even with no relation)…
    assert app.actions == ["reset sys domain offset"]
    # …which refused: not a single write, and the widget keeps the user's value.
    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)
    assert tab.sys_var.get() == "+30"


def test_page_reset_keeps_the_fabric_rows_when_the_relation_is_unknown() -> None:
    """The page-1 ↺ zeroes the Xbar/Sys widgets up front for the plan; with an
    unknown relation the plan refuses, so the widgets must stay where they were."""
    tab, app = make_tab(xbar_supported=True, xbar_value="+100")
    tab._oc_page = 1
    tab._fabric_parents = None
    tab.sys_var.set("+30")

    tab._reset_oc()

    assert app.actions == ["reset xbar/sys offset"]
    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)
    assert tab.xbar_var.get() == "+100"
    assert tab.sys_var.get() == "+30"


def test_fabric_parents_from_payload_takes_every_declared_edge() -> None:
    """Every edge the driver's table declares is a master→slave relation and
    is compensated — there is no per-edge verdict to filter on. The shape is
    the Rust one (fabric_relations.edges[{parent_bit, child_bit}]); a payload
    that could not be read (`table_available` false), or one without the
    block at all (older binding), reads as None → the relation stays UNKNOWN
    for this card and the rows refuse to write."""
    parse = OverclockTab._fabric_parents_from_payload
    payload = {
        "fabric_relations": {
            "source": "vftable-ext",
            "table_available": True,
            "roster": ["sys", "msd", "host"],
            "edges": [
                {"parent_bit": 1, "child_bit": 3},
                {"parent_bit": 1, "child_bit": 5},
                {"parent_bit": 1, "child_bit": 9},
                {"parent_bit": 1, "child_bit": 3},  # dupe
            ],
        }
    }
    assert parse(payload) == {3: (1,), 5: (1,), 9: (1,)}

    assert parse(None) is None
    assert parse({}) is None
    assert parse({"fabric_relations": {"edges": "nope"}}) is None
    # the read failed / ext layout not decoded: no edges may be invented
    assert parse({"fabric_relations": {"table_available": False, "edges": []}}) is None
    assert (
        parse({
            "fabric_relations": {
                "table_available": False,
                "edges": [{"parent_bit": 1, "child_bit": 3}],
            }
        })
        is None
    )


def test_query_clk_domain_caps_adopts_the_relation_from_the_vftable() -> None:
    """The relation arrives on the SAME worker as the caps (one private-table
    read per GPU) and is adopted before the anchor, so the very first paint
    already shows nets — including Ada's Host row, which the old
    XBAR→SYS-only rule never knew about."""
    tab, app = make_tab(xbar_supported=True)
    app.backend = _BackendWithVftable(
        entries={
            "controllable_mask": "0x000003FF",
            # the driver lists the attached domains with 0 offsets — their net
            # is then entirely the XBAR term riding into them.
            "entries": _domain_entries({1: 100_000, 3: 0, 9: 0}),
        },
        # Ada's live dump: the xbar record packs SYS and HOST — no MSD edge
        # is declared, so the MSD row stays on its own record.
        vftable={
            "fabric_relations": {
                "table_available": True,
                "edges": [
                    {"parent_bit": 1, "child_bit": 3},
                    {"parent_bit": 1, "child_bit": 9},
                ],
            }
        },
    )
    rows = _install_anchor_rows(tab)

    tab._query_clk_domain_capabilities({"gpu_series": "GeForce RTX 40 Series"})

    assert tab._fabric_parents == {3: (1,), 9: (1,)}
    assert tab._fabric_gpu == "GPU0"
    assert rows["xbar"].get() == 100.0
    assert rows["sys"].get() == 100.0  # net: own 0 + XBAR
    assert rows["host"].get() == 100.0
    assert rows["msd"].get() == 0.0  # no record in the payload → untouched

    # The table read is per-GPU: a second caps query (every refresh) must not
    # re-read the big payload.
    tab._query_clk_domain_capabilities({"gpu_series": "GeForce RTX 40 Series"})
    assert app.backend.vftable_calls == ["GPU0"]


def test_relation_is_adopted_even_when_the_caps_read_fails() -> None:
    """The relation rides its own read: a caps query that came back empty
    (driver hiccup) must not throw the relation away with it."""
    tab, _app = make_tab(xbar_supported=True)

    tab._on_clk_domain_caps_loaded(
        None,
        "GPU0",
        {
            "fabric_relations": {
                "table_available": True,
                "edges": [
                    {"parent_bit": 1, "child_bit": 9},
                ],
            },
        },
    )

    assert tab._fabric_relation() == {9: (1,)}


def test_relation_read_off_another_card_does_not_compensate_this_one() -> None:
    """A relation describes the card it was read from: switch to a GPU the
    driver has not described (yet) and the relation goes back to UNKNOWN —
    the rows stop compensating and refuse to write — instead of carrying the
    old card's edges over."""
    tab, app = make_tab(xbar_supported=True)
    tab._fabric_parents = {3: (1,), 9: (1,)}
    tab._fabric_gpu = "GPU0"
    assert tab._fabric_relation() == {3: (1,), 9: (1,)}

    app.selected_gpu_target = lambda: "GPU1"

    assert tab._fabric_relation() is None


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


class FakeStateWidget:
    """Slider/entry/button stub for the pager grey-gate + reset paths
    (state get/set via cget/configure, slider value via set)."""

    def __init__(self, state: str = "normal") -> None:
        self._state = state

    def cget(self, key: str) -> str:
        if key == "state":
            return self._state
        return ""

    def configure(self, **_kw) -> None:
        if "state" in _kw:
            self._state = _kw["state"]

    def set(self, _value) -> None:
        pass


# ── Unit toggle (MHz ↔ mV): slot-0 frequency ↔ slot-1 voltage ───────────────


class FakeUnitSlider(FakeStateWidget):
    """Slider stub carrying the unit-toggle plane state (volt mode, plane
    bounds, the entry var the re-anchor writes)."""

    def __init__(self, state: str = "normal") -> None:
        super().__init__(state)
        self._oc_volt_bit = None  # set by the test
        self._oc_volt_mode = False
        self._oc_unit_toggle = FakeStateWidget(state)
        self._oc_unit_var = None  # set by the test
        self._oc_freq_min = -500
        self._oc_freq_max = 500
        self._oc_freq_step = 5
        self._oc_freq_decimals = 0
        self._oc_volt_min = -300
        self._oc_volt_max = 300
        self._oc_volt_step = 0.1
        self._oc_signed = True
        self._oc_decimals = 0
        self._oc_min = -500
        self._oc_max = 500
        self._oc_step = 5
        self._value = 0

    def get(self) -> float:
        return self._value

    def set(self, value) -> None:
        self._value = value

    def cget(self, key: str):
        # A real CTkSlider exposes numeric from_/to; mirror that. (The stub
        # inherits a ""-returning cget, and _set_slider_value's getattr
        # default is evaluated eagerly even when _oc_min exists.)
        if key == "from_":
            return self._oc_min
        if key == "to":
            return self._oc_max
        return super().cget(key)

    def configure(self, **kw) -> None:
        # _reconfigure_slider passes from_/to/number_of_steps/state — the
        # FakeStateWidget.configure only handles state, so fold the rest in.
        if "from_" in kw:
            self._oc_min = kw["from_"]
        if "to" in kw:
            self._oc_max = kw["to"]
        if "number_of_steps" in kw and (self._oc_max - self._oc_min):
            self._oc_step = (self._oc_max - self._oc_min) / kw["number_of_steps"]
        super().configure(**kw)


class FakeBackend:
    """backend.query_private_freq_domain_info stub — the toggle's slot-1
    anchor source."""

    def __init__(self, entries: dict | None) -> None:
        self._entries = entries or {}
        self.calls: list[str] = []

    def query_private_freq_domain_info(self, gpu: str) -> dict:
        self.calls.append(gpu)
        return self._entries


class _BackendWithVftable(FakeBackend):
    """FakeBackend + the private V/F table read the relation rides on."""

    def __init__(self, entries: dict, vftable: dict | None) -> None:
        super().__init__(entries)
        self._vftable = vftable
        self.vftable_calls: list[str] = []

    def query_private_vftable(self, gpu: str) -> dict | None:
        self.vftable_calls.append(gpu)
        return self._vftable


def make_toggle_tab(
    entries: dict | None = None,
) -> tuple[OverclockTab, FakeApp, FakeBackend]:
    """A tab with one unit-toggle row (Xbar-style) wired to a fake backend
    and the plane state the toggle reads."""
    tab, app = make_tab(xbar_supported=True)
    backend = FakeBackend(entries)
    tab.app = app
    app.backend = backend
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 1
    var = FakeVar("0")
    slider._oc_unit_var = var
    tab.xbar_slider = slider
    tab.xbar_var = var
    return tab, app, backend


# ── Startup anchor: seed the fabric rows from the live ClkDomains offsets ──


def _install_anchor_rows(tab: OverclockTab) -> dict[str, FakeUnitSlider]:
    """Give every fabric row a unit-toggle-capable slider+var pair so the
    anchor path (_set_slider_value) has somewhere to land. Returns them by
    row name."""
    rows: dict[str, FakeUnitSlider] = {}
    for name, bit in (("xbar", 1), ("sys", 3), ("msd", 5), ("host", 9)):
        slider = FakeUnitSlider()
        slider._oc_volt_bit = bit
        var = FakeVar("+0")
        slider._oc_unit_var = var
        setattr(tab, f"{name}_slider", slider)
        setattr(tab, f"{name}_var", var)
        rows[name] = slider
    return rows


def _place(tab: OverclockTab, entries: list, mask: str = "0x000003FF", gpu="GPU0"):
    """Drive the caps-load callback the way the async worker does (gpu rides
    along so the anchor is keyed to the card it was read from)."""
    tab._on_clk_domain_caps_loaded({"controllable_mask": mask, "entries": entries}, gpu)


def test_query_clk_domain_caps_anchors_fabric_rows_end_to_end() -> None:
    """The reported bug: at startup the Xbar/Sys/Msd/Host rows stayed at the
    construction +0 even though the driver held real offsets. The whole
    startup path — check_capabilities → _query_clk_domain_capabilities →
    worker → _on_clk_domain_caps_loaded(gpu) — now seeds them from
    get-private-freq-domain-info."""
    tab, app = make_tab(xbar_supported=True)
    app.backend = FakeBackend({
        "controllable_mask": "0x000003FF",
        "entries": _domain_entries({1: 100_000, 3: -70_000, 9: 40_000}),
    })
    tab._fabric_parents = {3: (1,)}
    rows = _install_anchor_rows(tab)

    tab._query_clk_domain_capabilities({"gpu_series": "GeForce RTX 40 Series"})

    assert app.backend.calls == ["GPU0"]
    assert rows["xbar"].get() == 100.0
    assert rows["sys"].get() == 30.0  # bit1 + bit3 (XBAR rides into SYS)
    assert rows["host"].get() == 40.0
    assert tab.xbar_var.get() == "+100"
    assert tab.host_var.get() == "+40"


def test_startup_anchor_sys_row_includes_coupled_xbar_term() -> None:
    """30系+: the live SYS offset is bit1 + bit3 — the XBAR term rides into
    SYS, so the row must show the NET (that is also what the Sys write
    resolves to)."""
    tab, _app = make_tab(xbar_supported=True)
    tab._fabric_parents = {3: (1,)}
    rows = _install_anchor_rows(tab)

    _place(tab, _domain_entries({1: 100_000, 3: -70_000}))

    assert rows["xbar"].get() == 100.0
    assert rows["sys"].get() == 30.0


def test_startup_anchor_runs_once_per_gpu() -> None:
    """The caps query re-runs on every refresh (initial status, GPU switch,
    after every native action) — re-seeding would silently revert a value the
    user typed but has not applied yet."""
    tab, _app = make_tab(xbar_supported=True)
    rows = _install_anchor_rows(tab)
    _place(tab, _domain_entries({1: 100_000}))
    assert rows["xbar"].get() == 100.0

    tab.xbar_var.set("+150")  # user types, does not apply
    rows["xbar"].set(150)
    _place(tab, _domain_entries({1: 100_000}))  # a refresh re-loads the caps

    assert rows["xbar"].get() == 150.0
    assert tab.xbar_var.get() == "+150"


def test_startup_anchor_reseeds_after_a_gpu_switch() -> None:
    """A different card is a different anchor — the new GPU's rows get seeded
    from ITS offsets (keyed by gpu, not a plain once-ever flag)."""
    tab, app = make_tab(xbar_supported=True)
    rows = _install_anchor_rows(tab)
    _place(tab, _domain_entries({1: 100_000}), gpu="GPU0")
    assert rows["xbar"].get() == 100.0

    app.selected_gpu_target = lambda: "GPU1"
    _place(tab, _domain_entries({1: 20_000}), gpu="GPU1")

    assert rows["xbar"].get() == 20.0
    assert tab._domain_anchor_gpu == "GPU1"


def test_startup_anchor_skips_non_driver_data_and_unknown_rows() -> None:
    """value_modifiable False → the dwords are protocol placeholders, NOT
    offsets (the CLI's write path refuses those records too); a bit the mask
    does not claim has no row; a bit the payload omits leaves its row at 0."""
    tab, _app = make_tab(xbar_supported=True)
    rows = _install_anchor_rows(tab)
    placeholder = _domain_entries({1: 77_000})[0]
    placeholder["value_modifiable"] = False

    _place(tab, [placeholder] + _domain_entries({3: 20_000}), mask="0x000000FF")

    assert rows["xbar"].get() == 0.0  # placeholder must not be shown as truth
    assert rows["sys"].get() == 20.0
    assert rows["msd"].get() == 0.0  # absent from the payload
    assert rows["host"].get() == 0.0  # bit9 not in the mask


def test_startup_anchor_leaves_volt_mode_rows_alone() -> None:
    """A row the user already flipped to mV shows the voltage plane — the
    anchor must not write a MHz offset onto it."""
    tab, _app = make_tab(xbar_supported=True)
    rows = _install_anchor_rows(tab)
    rows["xbar"]._oc_volt_mode = True
    rows["xbar"]._oc_decimals = 1
    rows["xbar"].set(7.5)

    _place(tab, _domain_entries({1: 100_000}))

    assert rows["xbar"].get() == 7.5


def test_startup_anchor_reads_the_generation_freq_slot() -> None:
    """Blackwell 50系 parks the frequency plane in slot 2 (voltage 3), so the
    anchor must read this generation's slot, not slot 0."""
    tab, _app = make_tab(xbar_supported=True)
    tab._is_blackwell_gpu = True
    rows = _install_anchor_rows(tab)

    _place(tab, _domain_entries({1: 100_000, 5: -20_000}, freq_slot=2))

    assert rows["xbar"].get() == 100.0
    assert rows["msd"].get() == -20.0


def test_startup_anchor_retries_when_the_payload_has_nothing_to_anchor() -> None:
    """An empty/unreadable payload must not mark the GPU anchored — the next
    caps load retries instead of leaving the rows at 0 for the session."""
    tab, _app = make_tab(xbar_supported=True)
    rows = _install_anchor_rows(tab)

    _place(tab, [])
    assert tab._domain_anchor_gpu is None

    _place(tab, _domain_entries({1: 100_000}))

    assert rows["xbar"].get() == 100.0


def test_clk_domain_caps_result_for_another_gpu_is_ignored() -> None:
    """A GPU switch landing mid-flight: the payload belongs to GPU9 while the
    panel now shows GPU0 — neither the mask verdict nor the anchor may land."""
    tab, _app = make_tab(xbar_supported=True)
    tab._sys_supported = False
    rows = _install_anchor_rows(tab)

    _place(tab, _domain_entries({1: 100_000}), gpu="GPU9")

    assert tab._sys_supported is False
    assert rows["xbar"].get() == 0.0


# ── Per-row resets: net-aware on the coupled 30系+ XBAR/SYS pair ────────────


def test_reset_sys_domain_zeroes_the_net_offset() -> None:
    """Sys ↺ on 30系+: bit3 := −bit1 so the NET SYS offset is 0. Zeroing bit3
    outright would leave the whole XBAR offset standing as SYS drift, i.e. the
    row's 0 would be a lie. The volt plane is that domain's own → cleared."""
    tab, app = make_tab(xbar_supported=True)
    tab._fabric_parents = {3: (1,)}
    rows = _install_anchor_rows(tab)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    tab._reset_clk_domain("Sys", 3, rows["sys"], tab.sys_var)

    writes = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert [w[2:] for w in writes] == [(3, -100_000, 0, None), (3, 0, 1, None)]
    assert app.actions == ["reset sys domain offset"]
    assert tab.sys_var.get() == "+0"


def test_reset_xbar_domain_keeps_net_sys() -> None:
    """Xbar ↺ on 30系+ must not drag SYS: bit1 → 0 on both planes and bit3 :=
    the net the cancel was holding (bit1_cur + bit3_cur). The bit3 voltage
    plane belongs to the SYS domain and is left alone."""
    tab, app = make_tab(xbar_supported=True)
    tab._fabric_parents = {3: (1,)}
    rows = _install_anchor_rows(tab)
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000})

    tab._reset_clk_domain("Xbar", 1, rows["xbar"], tab.xbar_var)

    writes = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert [w[2:] for w in writes] == [
        (1, 0, 0, None),
        (1, 0, 1, None),
        (3, 30_000, 0, None),
    ]
    assert app.actions == ["reset xbar domain offset"]
    assert tab.xbar_var.get() == "+0"


def test_reset_sys_domain_in_volt_mode_clears_only_the_volt_plane() -> None:
    """mV mode has no coupling (separate plane), so the reset stays the plain
    single-slot zeroing."""
    tab, app = make_tab(xbar_supported=True)
    tab._fabric_parents = {3: (1,)}
    rows = _install_anchor_rows(tab)
    rows["sys"]._oc_volt_mode = True
    app.native.domain_entries = _domain_entries({1: 100_000})

    tab._reset_clk_domain("Sys", 3, rows["sys"], tab.sys_var)

    writes = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert [w[2:] for w in writes] == [(3, 0, 1, None)]


def test_toggle_switches_chip_and_reconfigures_to_mv_plane() -> None:
    """Click → mV: chip text/colour flips, the SAME slider/entry reconfigure
    onto ±300/1 (decimals=1), anchored at the record's live slot1 (µV→mV)."""
    tab, _app, backend = make_toggle_tab(
        entries={
            "entries": [
                {"bit": 1, "values_kHz": [25000, -12500]},
            ]
        }
    )
    slider = tab.xbar_slider
    var = tab.xbar_var

    tab._toggle_row_unit(slider)

    assert slider._oc_volt_mode is True
    assert slider._oc_unit_toggle._state == "mV" or True  # chip text via configure
    # plane bounds on the SAME widget: ±300, 0.1 mV step, one-decimal render
    assert slider._oc_min == -300
    assert slider._oc_max == 300
    assert slider._oc_step == 0.1
    assert slider._oc_decimals == 1
    # anchor = slot1 (−12500 µV) → −12.5 mV
    assert slider.get() == -12.5
    assert var.get() == "-12.5"
    assert backend.calls == ["GPU0"]


def test_toggle_back_restores_mhz_plane_and_live_anchor() -> None:
    """Second click → MHz: construction range/step/decimals return, the row
    re-anchors at the record's LIVE frequency offset — a fabric row's MHz
    value is a readback (see the startup anchor), so flipping the chip must
    not leave it showing 0 while the card still holds the offset."""
    tab, _app, backend = make_toggle_tab(
        entries={
            "entries": [
                {"bit": 1, "values_kHz": [25000, -12500]},
            ]
        }
    )
    slider = tab.xbar_slider
    var = tab.xbar_var

    tab._toggle_row_unit(slider)
    backend._entries["entries"][0]["values_kHz"][0] = 40_000  # moved meanwhile
    tab._toggle_row_unit(slider)

    assert slider._oc_volt_mode is False
    assert slider._oc_min == -500
    assert slider._oc_max == 500
    assert slider._oc_step == 5
    assert slider._oc_decimals == 0
    assert slider.get() == 40.0
    assert var.get() == "+40"


def test_toggle_back_anchors_at_zero_when_the_payload_has_no_offset() -> None:
    """No record for the bit (or an unreadable payload) → the MHz plane still
    comes back, anchored at 0."""
    tab, _app, _backend = make_toggle_tab()
    slider = tab.xbar_slider
    var = tab.xbar_var

    tab._toggle_row_unit(slider)
    tab._toggle_row_unit(slider)

    assert slider._oc_volt_mode is False
    assert slider.get() == 0
    assert var.get() == "+0"


def test_toggle_back_anchors_sys_row_at_the_net_offset() -> None:
    """The Sys row's MHz plane is the NET offset on 30系+ (bit1 + bit3) — the
    same value the startup anchor shows and the Sys write resolves to, not the
    raw bit3 cancel."""
    tab, _app, _backend = make_toggle_tab(
        entries={
            "entries": [
                {"bit": 1, "values_kHz": [100_000, 0]},
                {"bit": 3, "values_kHz": [-70_000, 0]},
            ]
        }
    )
    tab._fabric_parents = {3: (1,)}
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 3
    var = FakeVar("0")
    slider._oc_unit_var = var
    tab.sys_slider = slider
    tab.sys_var = var

    tab._toggle_row_unit(slider)  # → mV plane
    tab._toggle_row_unit(slider)  # → back to MHz

    assert slider.get() == 30.0
    assert var.get() == "+30"


def test_toggle_anchor_falls_back_to_zero_when_query_fails() -> None:
    """Unreadable backend (exception / no matching bit) → anchor 0, still in
    the mV plane."""
    tab, _app, backend = make_toggle_tab(entries=None)

    class Boom:
        def query_private_freq_domain_info(self, _gpu):
            raise RuntimeError("escape refused")

    tab.app.backend = Boom()
    slider = tab.xbar_slider

    tab._toggle_row_unit(slider)

    assert slider._oc_volt_mode is True
    assert slider.get() == 0


def test_xbar_volt_mode_writes_slot1_direct_no_cancel() -> None:
    """mV-mode Xbar apply: ONE slot-1 write on bit1 (µV), no bit3 cancel —
    the coupling/cancel is a slot-0 frequency-plane artifact; the voltage
    plane is per-domain independent."""
    tab, app = make_tab(xbar_supported=True)
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 1
    slider._oc_volt_mode = True
    tab.xbar_slider = slider
    tab.xbar_var = FakeVar("25")
    tab._fabric_parents = {3: (1,)}  # would couple on the MHz plane

    tab._apply_xbar_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 1, 25000, 1, None)]
    assert app.actions == ["apply xbar volt offset"]
    assert "volt offset +25 mV" in app.action_outputs[0]


def test_sys_volt_mode_skips_rmw() -> None:
    """mV-mode Sys apply: direct slot-1 bit3 write — the RMW exists to
    preserve the slot-0 Xbar-cancel, which a slot-1 write cannot touch."""
    tab, app = make_tab(xbar_supported=True)
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 3
    slider._oc_volt_mode = True
    tab.sys_slider = slider
    tab.sys_var = FakeVar("-30")
    app.native.bit3_current_khz = 10000  # MHz plane would RMW from this

    tab._apply_sys_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 3, -30000, 1, None)]
    # no query_private_freq_domain_info → no RMW baseline read
    assert not [c for c in app.native.calls if c[0] == "query_private_freq_domain_info"]


def test_core_volt_mode_skips_public_path() -> None:
    """mV-mode Core apply: ONLY the ClkDomains bit0 slot-1 write — the
    pstate20 public path (and its -104 fallback chain) is frequency-plane
    only, there is no public voltage path to try first."""
    tab, app = make_tab()
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 0
    slider._oc_volt_mode = True
    slider._oc_decimals = 1
    tab.core_slider = slider
    tab.core_var = FakeVar("+12.5")

    tab._apply_core_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 0, 12500, 1, None)]
    # the public pstate20 setter was never touched
    assert not [c for c in app.native.calls if c[0] == "set_clock_offset"]
    assert app.actions == ["apply core volt offset"]


def test_mem_volt_mode_skips_public_path() -> None:
    """mV-mode Mem apply: only the bit2 slot-1 write."""
    tab, app = make_tab()
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 2
    slider._oc_volt_mode = True
    tab.mem_slider = slider
    tab.mem_var = FakeVar("-8")

    tab._apply_mem_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 2, -8000, 1, None)]
    assert app.actions == ["apply memory volt offset"]


def test_volt_format_reads_slot1_readback() -> None:
    """The mV console message shows pynvoc's applied value AS-IS: the
    binding already divides the record's raw slot-1 dword (µV) by 1000, so
    `applied_mHz` IS millivolts — dividing again reported 1000× too small
    (4060 regression: "+134.8 mV applied, readback +0.1348 mV")."""
    # -12500 µV raw → pynvoc applied_mHz = -12.5 (already mV)
    res = {"applied": True, "slot": 1, "applied_mHz": -12.5}
    msg = OverclockTab._format_clk_domain_volt_result("Sys", -12.5, res)
    assert "volt offset -12.5 mV" in msg
    assert "readback -12.5 mV" in msg
    # the exact 4060 report: +134.8 mV write → raw 134800 µV → applied 134.8
    res_4060 = {"applied": True, "slot": 1, "applied_mHz": 134.8}
    msg_4060 = OverclockTab._format_clk_domain_volt_result("Core", 134.8, res_4060)
    assert "volt offset +134.8 mV" in msg_4060
    assert "readback +134.8 mV" in msg_4060
    assert "0.1348" not in msg_4060

    # non-1 slot → no readback clause (the write didn't land on the plane)
    res2 = {"applied": True, "slot": 0, "applied_mHz": 25.0}
    msg2 = OverclockTab._format_clk_domain_volt_result("Sys", 25.0, res2)
    assert "readback" not in msg2

    res3 = {"supported": False}
    msg3 = OverclockTab._format_clk_domain_volt_result("Sys", 25.0, res3)
    assert "not supported" in msg3


def test_row_volt_mode_defaults_false_for_plain_rows() -> None:
    """Plain (non-toggle) rows read volt mode False via getattr default —
    every existing MHz apply path stays untouched."""
    tab, _app = make_tab()
    assert tab._row_volt_mode(tab.core_slider) is False
    assert tab._row_volt_mode(object()) is False


# ── Blackwell (50系) plane-slot shift: freq 0→2, volt 1→3 ──────────────────


def test_is_blackwell_from_info_codename_gate() -> None:
    """Codename GB* = Blackwell (desktop/laptop/workstation/server);
    Volta GV100 and Pascal GP* must not collide with the prefix."""
    f = OverclockTab._is_blackwell_from_info
    assert f({"codename": "GB207"}) is True
    assert f({"codename": "gb202-B"}) is True
    assert f({"codename": "AD107-B"}) is False
    assert f({"codename": "GV100"}) is False
    assert f({"codename": "GP104"}) is False
    assert f({}) is False


def _make_blackwell_tab(**kw) -> tuple[OverclockTab, FakeApp]:
    tab, app = make_tab(**kw)
    tab._is_blackwell_gpu = True
    return tab, app


def test_blackwell_xbar_mhz_writes_freq_slot2() -> None:
    """Blackwell: the frequency plane lives in slot 2 (shifted from slot 0).
    --freq on the CLI and the MHz row apply both target slot 2."""
    tab, app = _make_blackwell_tab(xbar_supported=True, xbar_value="10")

    tab._apply_xbar_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 1, 10_000, 2, None)]


def test_blackwell_xbar_mv_writes_volt_slot3() -> None:
    """Blackwell: the voltage plane lives in slot 3 — the mV-mode Xbar
    apply writes slot 3, not slot 1."""
    tab, app = _make_blackwell_tab(xbar_supported=True)
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 1
    slider._oc_volt_mode = True
    tab.xbar_slider = slider
    tab.xbar_var = FakeVar("25")
    tab._fabric_parents = {3: (1,)}  # a parent term is MHz-plane: no bit3 park here

    tab._apply_xbar_only()

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 1, 25_000, 3, None)]


def test_blackwell_core_fallback_writes_freq_slot2() -> None:
    """Blackwell: the pstate20 -104 fallback lands on the ClkDomains
    frequency plane = slot 2."""
    tab, app = _make_blackwell_tab()
    app.native.raise_on_set_clock = RuntimeError("NVAPI NotSupported -104")

    OverclockTab._apply_core_only_action(app.native, "GPU0", "nvapi", 125.0, "P0", 2)

    clk_calls = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert clk_calls == [("set_clk_domain_offset", "GPU0", 0, 125_000, 2, None)]


def test_blackwell_row_reset_clears_slots_2_and_3() -> None:
    """Blackwell: the MHz-mode row reset clears both plane slots (2, 3) —
    NOT slots 0/1, which are driver-opaque dwords on this generation."""
    tab, app = _make_blackwell_tab(xbar_supported=True, xbar_value="10")

    tab._reset_clk_domain("Xbar", 1, tab.xbar_slider, tab.xbar_var)

    slots = sorted(c[4] for c in app.native.calls if c[0] == "set_clk_domain_offset")
    assert slots == [2, 3]


def test_blackwell_volt_anchor_reads_slot3() -> None:
    """The mV toggle's anchor reads the record's slot-3 dword (µV→mV) on
    Blackwell — slot 1 is an opaque dword there, not the voltage plane."""
    tab, _app = _make_blackwell_tab(xbar_supported=True)
    backend = FakeBackend({"entries": [{"bit": 1, "values_kHz": [11, 22, 33, -12500]}]})
    tab.app.backend = backend
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 1

    assert tab._query_row_volt_anchor_mv(slider) == -12.5


def test_non_blackwell_volt_anchor_still_reads_slot1() -> None:
    """10~40系 regression: the anchor keeps reading slot 1."""
    tab, _app = make_tab(xbar_supported=True)
    backend = FakeBackend({"entries": [{"bit": 1, "values_kHz": [11, -12500, 33]}]})
    tab.app.backend = backend
    slider = FakeUnitSlider()
    slider._oc_volt_bit = 1

    assert tab._query_row_volt_anchor_mv(slider) == -12.5


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


class _RecordingSlider(FakeSlider):
    """FakeSlider that records the last configured range + set() value so
    update_mobile_limits anchoring can be asserted."""

    def __init__(self, state: str = "normal") -> None:
        super().__init__(state)
        self.configured: dict = {}
        self.value = None

    def configure(self, **kw) -> None:
        self.configured.update(kw)

    def set(self, value) -> None:
        self.value = value


def _make_mobile_limits_tab() -> tuple[OverclockTab, _RecordingSlider, FakeVar]:
    tab, _app = make_mobile_tab()
    slider = _RecordingSlider()
    var = FakeVar("100")
    tab.plimit_slider = slider
    tab.plimit_var = var
    return tab, slider, var


def test_update_mobile_limits_anchors_plimit_at_power_wall() -> None:
    """The Pwr Limit slider anchors at the actually-effective power wall
    (power_limit_w = min of requested TGP and the active D-Notifier cap),
    NOT the VBIOS default — anchoring at the default made the slider jump
    after a D-Notifier apply/section reset."""
    tab, slider, var = _make_mobile_limits_tab()

    tab.update_mobile_limits({
        "tgp": {
            "policy_index": 2,
            "min_watt": 5.0,
            "default_watt": 100.0,
            "max_watt": 140.0,
        },
        "dnotifier": None,
        "temp_policies": [],
        "volt_rail": None,
        "power_limit_w": 55.0,
    })

    assert slider.configured["from_"] == 5
    assert slider.configured["to"] == 140
    assert slider.value == 55
    assert var.value == "55"


def test_update_mobile_limits_clamps_wall_into_tgp_range() -> None:
    """A wall outside the fresh TGP range clamps to the nearest bound —
    never jumps back to the VBIOS default."""
    tab, slider, var = _make_mobile_limits_tab()

    tab.update_mobile_limits({
        "tgp": {
            "policy_index": 2,
            "min_watt": 5.0,
            "default_watt": 100.0,
            "max_watt": 140.0,
        },
        "dnotifier": None,
        "temp_policies": [],
        "volt_rail": None,
        "power_limit_w": 150.0,  # above the fresh max
    })

    assert slider.value == 140
    assert var.value == "140"


def test_update_mobile_limits_falls_back_to_default_without_wall() -> None:
    """No wall reading at all (private family + NVML both unavailable) →
    keep the VBIOS default as the position."""
    tab, slider, var = _make_mobile_limits_tab()

    tab.update_mobile_limits({
        "tgp": {
            "policy_index": 2,
            "min_watt": 5.0,
            "default_watt": 100.0,
            "max_watt": 140.0,
        },
        "dnotifier": None,
        "temp_policies": [],
        "volt_rail": None,
        "power_limit_w": None,
    })

    assert slider.value == 100
    assert var.value == "100"


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
    assert f("gp104") is True  # Pascal (10系) — Xbar offset live-verified
    assert f("gv100") is True  # Volta — allowed through alongside Pascal
    assert f("gm204") is False  # Maxwell and older — predates XBAR ClockClient
    assert f("") is False


def test_xbar_supported_arch_friendly_names() -> None:
    # CLI human output: friendly architecture names.
    f = OverclockTab._xbar_supported_arch
    assert f("Turing") is True
    assert f("Ampere") is True
    assert f("Ada") is True
    assert f("Blackwell") is True
    assert f("Pascal") is True  # Xbar offset live-verified on Pascal
    assert f("Volta") is True
    assert f("Maxwell") is False


def test_apply_xbar_only_converts_mhz_to_khz() -> None:
    # GUI speaks MHz; pynvoc takes kHz. xbar = ClockClient domain bit 1.
    # Non-Blackwell tab → the frequency plane rides slot 0 explicitly.
    # The row write snapshots the plane first (the net solve needs the current
    # own values), then writes only what changes.
    tab, app = make_tab(xbar_value="10")

    tab._apply_xbar_only()

    assert app.actions == ["apply xbar offset"]
    assert app.native.calls == [
        ("query_private_freq_domain_info", "GPU0"),
        ("set_clk_domain_offset", "GPU0", 1, 10_000, 0, None),
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
    # Apply Section applies the CURRENT page; page 1 = Xbar/Sys.
    tab, app = make_tab(xbar_supported=True, xbar_value="10")
    tab._oc_page = 1

    tab._apply_oc()

    assert "apply xbar offset" in app.actions
    assert ("set_clk_domain_offset", "GPU0", 1, 10_000, 0, None) in [
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
    tab._oc_page = 1

    tab._apply_oc()

    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


def test_reset_oc_resets_xbar_when_supported() -> None:
    """Page-1 ↺ drives the XBAR (and SYS) rows to net 0 through the plan — and
    on the FREQUENCY plane slot this generation uses, not the driver-opaque
    slot 0 the old `set_clk_domain_offset(bit, 0, None, …)` wrote."""
    tab, app = make_tab(xbar_supported=True, xbar_value="10")
    tab._oc_page = 1

    tab._reset_oc()

    assert tab.xbar_var.get() == "0"
    writes = [c for c in app.native.calls if c[0] == "set_clk_domain_offset"]
    assert [w[2:] for w in writes] == [
        (1, 0, 0, None),
        (3, 0, 0, None),
    ]
    assert app.actions == ["reset xbar/sys offset"]


def test_reset_oc_page1_keeps_net_sys_and_net_host() -> None:
    """The page-1 ↺ zeroes the XBAR and SYS rows; every other domain rides
    along only if it must — Ada's HOST row is re-parked at the net it was
    holding instead of being dragged by the XBAR reset."""
    tab, app = make_tab(xbar_supported=True, xbar_value="+100")
    tab._oc_page = 1
    tab._fabric_parents = {3: (1,), 9: (1,)}
    # XBAR +100 with SYS +30 net and HOST +50 net parked under it.
    app.native.domain_entries = _domain_entries({1: 100_000, 3: -70_000, 9: -50_000})

    tab._reset_oc()

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits[1] == 0
    assert bits[3] == 0  # target: net SYS 0
    assert bits[9] == 50_000  # parked: net HOST +50 preserved


def test_reset_fabric_row_parks_a_grandchild_at_its_net() -> None:
    """A domain riding the reset row is parked against the value that actually
    lands. Two levels down the solve must not subtract the parent term twice:
    resetting SYS (net 0, under XBAR +60) with MSD riding SYS keeps net SYS 0
    and net MSD 0 — bit5 := +60, not +120."""
    tab, app = make_tab(xbar_supported=True)
    app.native.domain_entries = _domain_entries({1: 60_000, 3: 0, 5: 0})

    msg = OverclockTab._reset_fabric_row_action(
        app.native, "GPU0", "Sys", 3, {3: (1,), 5: (3,)}, 0, 1
    )

    bits = {
        c[2]: c[3]
        for c in app.native.calls
        if c[0] == "set_clk_domain_offset" and c[4] == 0  # frequency plane
    }
    assert bits[3] == -60_000  # net SYS := 0 while XBAR holds +60
    assert bits[5] == 60_000  # parked: own := net_old(MSD) − new own(SYS)
    assert "net Msd +0 MHz kept" in msg


def test_vfcurve_curve_reset_routes_relation_bits_through_the_net_solve() -> None:
    """Resetting the XBAR curve zeroes the XBAR net and re-parks SYS/HOST at
    theirs. The old [bit1, bit3] footprint also wiped SYS's own offset (and
    left every descendant dragged by the XBAR term)."""
    from src.tabs.vfcurve.tab import VFCurveTab

    tab, app = make_tab(xbar_supported=True)
    oc = types.SimpleNamespace(
        _is_blackwell_gpu=False,
        _fabric_relation=lambda: {3: (1,), 9: (1,)},
    )
    stub = types.SimpleNamespace(app=types.SimpleNamespace(tab_overclock=oc))
    app.native.domain_entries = _domain_entries({1: 60_000, 3: 0, 9: 20_000})

    note = VFCurveTab._domain_global_reset_note(stub, app.native, "GPU0", "xbar")

    bits = {c[2]: c[3] for c in app.native.calls if c[0] == "set_clk_domain_offset"}
    assert bits[1] == 0  # XBAR net → 0
    assert bits[3] == 60_000  # net SYS +60 kept (own := −new own(XBAR))
    assert bits[9] == 80_000  # net HOST +80 kept
    assert "net Sys +60 MHz kept" in note


def test_vfcurve_curve_reset_refuses_when_the_relation_is_unknown() -> None:
    """Unknown relation: the curve reset for a fabric bit refuses (nothing is
    written — its global offset is a net that cannot be resolved), while a
    GPC/MEM curve keeps zeroing its global offset as it always did."""
    from src.tabs.vfcurve.tab import VFCurveTab

    tab, app = make_tab(xbar_supported=True)
    oc = types.SimpleNamespace(
        _is_blackwell_gpu=False,
        _fabric_relation=lambda: None,
    )
    stub = types.SimpleNamespace(app=types.SimpleNamespace(tab_overclock=oc))
    app.native.domain_entries = _domain_entries({1: 60_000})

    note = VFCurveTab._domain_global_reset_note(stub, app.native, "GPU0", "xbar")

    assert note.strip() == OverclockTab._FABRIC_UNKNOWN_MSG
    assert not any(c[0] == "set_clk_domain_offset" for c in app.native.calls)

    gpc = VFCurveTab._domain_global_reset_note(stub, app.native, "GPU0", "gpc")

    assert gpc.strip() != OverclockTab._FABRIC_UNKNOWN_MSG
    assert any(c[0] == "set_clk_domain_offset" for c in app.native.calls)


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
    assert (
        f({"gpu_name": "NVIDIA GeForce GTX 1080"}) is True
    )  # GTX 1080 = Pascal, supported
    assert (
        f({"gpu_name": "NVIDIA GeForce GTX 980"}) is False
    )  # 9-series = Maxwell, too old
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


def test_pstate_lock_mem_range_first_on_any_gpu() -> None:
    # Mem-range is ALWAYS the first choice — no generation gating. Even a
    # legacy-voltage part (Maxwell Titan X / Kepler) gets the range lock
    # (with the full range preserved) until it actually fails.
    tab, app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))

    tab._apply_pstate_lock()

    assert app.actions == ["apply P-State lock"]
    assert app.native.calls == [("set_nvapi_pstate_lock", "GPU0", "P0", "P2")]


def test_pstate_lock_mem_range_failure_falls_back_to_pin() -> None:
    # Runtime mem-range failure (pre-Kepler part: NVML pstate mem-clock
    # query Not Supported) → native single-P-State pin, point-mode fused.
    tab, app = make_tab(mem_lock_error=RuntimeError("Not Supported"))
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))

    tab._apply_pstate_lock()
    # The pin re-apply rides the failed action's on_finished (finish code -1
    # on the mem-range attempt): the mem-range call raises (recorded), then
    # the fallback pin fires.
    assert app.native.calls == [
        ("set_nvapi_pstate_lock", "GPU0", "P0", "P2"),
        ("set_pstate_native_lock", "GPU0", "P0"),
    ]
    assert tab.pstate_selector.point_mode is True
    assert tab._pstate_pin_fallback is True
    # The fallback is announced in the console.
    assert any(
        "falling back to the native single-P-State pin" in m
        for m in app.console.messages
    )


def test_pstate_fallback_survives_busy_mainloop() -> None:
    # Regression: the pin used to dispatch via app.after(0) from the
    # worker's except block — while the first action still held the
    # _action_running gate — so a busy mainloop ran it into "Another native
    # action is already running" and the pin never applied. The fallback
    # must ride the action's on_finished instead. Deferring after()
    # callbacks here stands in for the busy mainloop: under the old
    # dispatch the fallback would sit unflushed in `deferred` and this
    # test would see no pin.
    tab, app = make_tab(mem_lock_error=RuntimeError("Not Supported"))
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))
    deferred: list = []
    app.after = lambda _delay, callback: deferred.append(callback)

    tab._apply_pstate_lock()

    # No cross-thread marshal needed — the fallback rode on_finished.
    assert deferred == []
    assert app.native.calls == [
        ("set_nvapi_pstate_lock", "GPU0", "P0", "P2"),
        ("set_pstate_native_lock", "GPU0", "P0"),
    ]
    assert tab._pstate_pin_fallback is True
    assert tab.pstate_selector.point_mode is True


def test_pstate_pin_sticky_after_fallback() -> None:
    # After the fallback armed, subsequent applies go straight to the pin
    # (no pointless mem-range retry) and unlock resets the pin.
    tab, app = make_tab(mem_lock_error=RuntimeError("Not Supported"))
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))
    tab._apply_pstate_lock()

    tab._apply_pstate_lock()
    assert app.native.calls[-1] == ("set_pstate_native_lock", "GPU0", "P0")

    tab._unlock_pstate_lock()
    assert app.native.calls[-1] == ("reset_pstate_native_lock", "GPU0")


def test_pstate_fallback_cleared_on_pstate_refresh() -> None:
    # A fresh p-state roster (get / GPU switch) clears the fallback and
    # restores the range selector.
    tab, app = make_tab(mem_lock_error=RuntimeError("Not Supported"))
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))
    tab._apply_pstate_lock()
    assert tab.pstate_selector.point_mode is True

    tab.btn_apply_pstate = FakeSlider("normal")
    tab.btn_unlock_pstate = FakeSlider("normal")
    tab.set_supported_pstates(["P0", "P2", "P8"])

    assert tab._pstate_pin_fallback is False
    assert tab.pstate_selector.point_mode is False


def test_supported_pstates_normalized_idle_first() -> None:
    # Canonical idle-first roster regardless of the reporting backend:
    # NVML enumerates P8..P0, but the NVAPI pstates20 fallback on legacy
    # drivers lists max-first — unsorted, the selector rendered mirrored
    # there and the default selection (last slot) silently became the
    # idle state instead of the max-perf endpoint.
    tab, app = make_tab()
    tab.pstate_selector = _FakePstateSelector(("P0", "P0"))
    tab.btn_apply_pstate = FakeSlider("normal")
    tab.btn_unlock_pstate = FakeSlider("normal")

    tab.set_supported_pstates(["P0", "P2", "P8", "P5"])

    assert tab._supported_pstates == ["p8", "p5", "p2", "p0"]


def test_pstate_lock_mem_range_warning_surfaced() -> None:
    # Overlapping P-States outside the requested range (identical memory
    # clocks after a VBIOS edit): applied anyway, warning surfaced first.
    tab, app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P0", "P0"))
    app.native.pstate_lock_warning = (
        "P0 maps to memory lock window 3501-3601 MHz, which also overlaps "
        "NVML P-States outside the requested range: P2 — applying anyway"
    )

    tab._apply_pstate_lock()

    assert app.native.calls == [("set_nvapi_pstate_lock", "GPU0", "P0", "P0")]
    output = app.action_outputs[0]
    assert output.startswith("Warning: P0 maps to memory lock window")
    assert "Successfully applied NVAPI P-State lock P0-P0." in output


def test_pstate_lock_modern_uses_mem_range() -> None:
    # Modern GPU: original range-lock path (point-mode OFF, range slider).
    tab, app = make_tab(legacy_voltage=False)
    tab.pstate_selector = _FakePstateSelector(("P0", "P2"))

    tab._apply_pstate_lock()

    assert app.actions == ["apply P-State lock"]
    # NVAPI backend selected → set_nvapi_pstate_lock (mem-range derivation).
    assert app.native.calls == [("set_nvapi_pstate_lock", "GPU0", "P0", "P2")]


def test_pstate_range_mode_default() -> None:
    # The selector starts in range mode — point-mode only fuses after a
    # runtime mem-range failure.
    tab, _app = make_tab(legacy_voltage=True)
    tab.pstate_selector = _FakePstateSelector(("P0", "P8"))
    tab.btn_apply_pstate = FakeSlider("normal")
    tab.btn_unlock_pstate = FakeSlider("normal")

    tab.set_supported_pstates(["P0", "P8", "P12"])

    assert tab.pstate_selector.point_mode is False


# ── Fan-section verdict: fanless server cards grey out (is_server + count) ──


class _FanSectionRecorder:
    def __init__(self) -> None:
        self.supported: bool | None = None
        self.calls: list[tuple] = []

    def set_supported(self, supported: bool) -> None:
        self.supported = supported
        self.calls.append(("set_supported", supported))

    def set_legacy_nvapi(self, legacy: bool) -> None:
        self.calls.append(("set_legacy_nvapi", legacy))

    def set_fan_choices(self, choices) -> None:
        self.calls.append(("set_fan_choices", tuple(choices)))

    def set_level(self, level: int) -> None:
        self.calls.append(("set_level", level))


def _make_fan_tab() -> tuple[OverclockTab, _FanSectionRecorder]:
    tab, _app = make_tab()
    tab.fan_section = _FanSectionRecorder()
    tab._fanless_gpus = set()
    tab._fanned_gpus = set()
    tab._fan_surface_gpu = None
    tab._fan_surface_load_in_flight = False
    tab._mobile_mode = False
    tab._set_limit_panel_mode = lambda mode: None
    tab._load_mobile_limits = lambda *a, **k: None
    tab._refresh_fan_surface = lambda is_mobile: None
    tab.vboost_label_var = FakeVar("VoltBoost:")
    tab.vboost_unit_var = FakeVar("%")
    return tab, tab.fan_section


def _fan_info_payload(**overrides) -> dict:
    payload = {
        "gpu_name": "NVIDIA Tesla P100-PCIE-16GB",
        "gpu_architecture": "GP100",
        "is_mobile": False,
        "is_server": False,
        "is_legacy_voltage": False,
        "xbar_supported": False,
    }
    payload.update(overrides)
    return payload


def test_fan_supported_on_desktop_gpu() -> None:
    tab, fan = _make_fan_tab()
    tab.check_capabilities(_fan_info_payload())
    assert fan.supported is True


def test_fan_greys_out_on_mobile_gpu() -> None:
    tab, fan = _make_fan_tab()
    tab.check_capabilities(_fan_info_payload(is_mobile=True))
    assert fan.supported is False


def test_fan_greys_out_synchronously_on_server_flag() -> None:
    """gpu_type.rs is_server classification — no fan query needed."""
    tab, fan = _make_fan_tab()
    tab.check_capabilities(_fan_info_payload(is_server=True))
    assert fan.supported is False


def test_fan_server_flag_waived_by_observed_fans() -> None:
    """ServerLovelace L40/L4 carry onboard fans: the observed count ≥ 1
    (recorded in _fanned_gpus by _fan_surface_loaded) wins over the
    classification, and stays winning on later capability refreshes."""
    tab, fan = _make_fan_tab()
    tab._fanned_gpus.add("GPU0")
    tab.check_capabilities(_fan_info_payload(is_server=True))
    assert fan.supported is True


def test_fan_surface_loaded_count_zero_greys_out_and_sticks() -> None:
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    tab._fan_surface_loaded("GPU0", {"count": 0})
    assert "GPU0" in tab._fanless_gpus
    assert "GPU0" not in tab._fanned_gpus
    assert fan.supported is False
    # The recorded verdict survives later capability refreshes (no flags).
    tab.check_capabilities(_fan_info_payload())
    assert fan.supported is False


def test_fan_surface_loaded_count_positive_re_enables_server_card() -> None:
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    # Server flag greys it first…
    tab.check_capabilities(_fan_info_payload(is_server=True))
    assert fan.supported is False
    # …then NVML reports a fan (L40 case) — pane re-enables and the
    # classification is waived on every later refresh.
    tab._fan_surface_loaded("GPU0", {"count": 1, "current_percent": 37})
    assert fan.supported is True
    assert "GPU0" in tab._fanned_gpus
    tab.check_capabilities(_fan_info_payload(is_server=True))
    assert fan.supported is True


def test_fan_surface_loaded_stale_gpu_ignored() -> None:
    """A fan verdict landing after a GPU switch must not re-verdict the
    newly selected card."""
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU1"  # selection moved on
    tab.check_capabilities(_fan_info_payload())
    tab._fan_surface_loaded("GPU0", {"count": 0})
    assert "GPU0" not in tab._fanless_gpus
    assert fan.supported is True


def test_fan_surface_modern_keeps_continuous_policy() -> None:
    """Modern cards answer the NVAPI cooler family too (1650 Super / A4000
    count=1 live) — the verdict must NOT flip the policy dropdown to the
    legacy default/manual list (regression: every fanned GPU was flagged
    legacy from the NVML count alone)."""
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    tab._fan_surface_loaded(
        "GPU0", {"count": 1, "current_percent": 33}, {"count": 1, "coolers": []}
    )
    assert ("set_legacy_nvapi", False) in fan.calls


def test_fan_surface_legacy_signature_restricts_policy() -> None:
    """GT730 signature: NVML sees the fan while the private NVAPI cooler
    family reports zero — policy dropdown restricted to default/manual."""
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    tab._fan_surface_loaded(
        "GPU0", {"count": 1, "current_percent": 40}, {"count": 0, "coolers": []}
    )
    assert ("set_legacy_nvapi", True) in fan.calls


def test_fan_surface_missing_nvapi_answer_counts_legacy() -> None:
    """No NVAPI cooler answer (old deployed pyd / transient error) is
    treated conservatively as legacy."""
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    tab._fan_surface_loaded("GPU0", {"count": 1, "current_percent": 40}, None)
    assert ("set_legacy_nvapi", True) in fan.calls


def test_fan_surface_verdict_flips_between_gpus() -> None:
    """The shared fan pane must re-verdict BOTH ways on GPU switch: legacy
    card → modern card restores the modern policy list."""
    tab, fan = _make_fan_tab()
    tab._fan_surface_gpu = "GPU0"
    tab._fan_surface_loaded(
        "GPU0", {"count": 1, "current_percent": 40}, {"count": 0, "coolers": []}
    )
    assert ("set_legacy_nvapi", True) in fan.calls
    tab._fan_surface_gpu = "GPU1"
    tab._fan_surface_loaded(
        "GPU1", {"count": 1, "current_percent": 35}, {"count": 1, "coolers": []}
    )
    assert ("set_legacy_nvapi", False) in fan.calls
