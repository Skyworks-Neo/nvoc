"""
Overclock Tab - OC offset (slider + entry) and power/thermal limits.
Ranges are queried from GPU hardware via the CLI 'info' command.
"""

from typing import TYPE_CHECKING, Tuple, Dict, Any, Optional, Union

import tkinter as tk

import customtkinter as ctk

from src.tabs.dashboard.sections.fan import FanControlPane
from src.widgets.lightweight_controls import (
    ct_button_font,
    CanvasSlider,
    LiteButton,
    LiteCheckbutton,
    LiteEntry,
    SegmentRangeSelector,
    SegmentToggleSelector,
    install_mousewheel_support,
)
from src.widgets.hover_tooltip import HoverTooltip

# ── De-CTk'd panel palette ──────────────────────────────────────────────
# Inner layout uses plain tk widgets: one bg for every container (matches
# CTk dark-theme frame/scroll bg), plus text colors. Keeping these in one
# place makes the light/dark question a one-spot change.
_PANEL_BG = "#2b2b2b"  # CTk dark frame/scroll background
_TEXT_FG = "#e5e5e5"  # default label text
_TEXT_FG_DIM = "#b3b3b3"  # 'gray70' hints
_TEXT_FG_FAINT = "#999999"  # 'gray60' status text
_ACCENT_FG = "#e67e22"  # active-mode unit chip (mV) — orange accent
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")
# Unit toggle (MHz/mV): slightly smaller than body so the thin border reads
# as a chip around the unit, not a button competing with the ✓ apply.
_FONT_UNIT = ("Segoe UI", 10)


if TYPE_CHECKING:
    from src.app import App


class OverclockTab:
    """Overclock tab for GPU OC offset settings with slider + numeric entry."""

    # ── Fallback defaults (overridden by real GPU info) ──
    _DEFAULTS = {
        "core_clock_min": -500,
        "core_clock_max": 500,
        "mem_clock_min": -500,
        "mem_clock_max": 1500,
        "power_limit_min": 50,
        "power_limit_max": 150,
        "power_limit_default": 100,
        "thermal_limit_min": 60,
        "thermal_limit_max": 95,
        "thermal_limit_default": 83,
        "voltage_boost_min": 0,
        "voltage_boost_max": 100,
    }

    def __init__(
        self,
        parent: ctk.CTkFrame,
        app: "App",
        content_parent: Optional[Any] = None,
        fan_parent: Optional[Any] = None,
    ):
        self.app = app
        self.frame = parent
        self._syncing = False  # guard against feedback loops
        self._is_vfp_mode = False
        self._vfp_uniform_offset_mhz = None  # type: Optional[int]
        self._limit_supported_state = True
        self._limit_panel_mode = "desktop"  # "desktop" | "mobile" | "off"
        self._mobile_mode = False
        self._tgp_policy_index = 2
        self._mobile_ppab_initialized_for = None  # type: Optional[str]
        self._mobile_limits_gpu = None  # type: Optional[str]
        self._mobile_load_in_flight = False
        self._volt_rail_bit = 0  # VoltRails rail bit (0 on single-rail mobile GPUs)
        # GPUs whose NVML fan-info reports zero coolers (fanless server cards:
        # P100/A100 …) — the Fan pane greys out for these via the same
        # set_supported_state path the mobile verdict drives.
        self._fanless_gpus = set()  # type: Set[str]
        # GPUs whose NVML cooler count came back ≥ 1 — observed fans win over
        # the is_server classification (ServerLovelace L40/L4 carry fans).
        self._fanned_gpus = set()  # type: Set[str]
        self._fan_surface_gpu = None  # type: Optional[str]
        self._xbar_supported = False  # Xbar row: Pascal (GTX 10系) and newer
        # ── Clock-offset pager state ──
        # pre-Pascal (Kepler/Fermi/Maxwell-9) has no pager: only Core/Mem.
        # Pascal+ has 3 pages: (Core/Mem)(Xbar/Sys)(Msd/Host). Bit mapping is
        # the Ada-verified WRITE-record table (see gpu_type.rs is_ada): bit0=GPC,
        # bit1=XBAR(30+ couples SYS), bit2=Mem, bit3=SYS, bit5=MSD, bit9=HOST.
        self._has_oc_pager = False
        self._oc_page = 0
        self._oc_page_frames = []  # type: List[tk.Frame]
        self._oc_page_indicator = None  # type: Optional[tk.Label]
        self._is_ampere_plus = False  # 30系+: bit1 couples SYS → bit3 -f抵消
        self._is_pascal_gpu = False  # Pascal verdict captured at caps-query time
        self._clk_domain_mask = (
            0  # controllable mask from query_private_freq_domain_info
        )
        self._sys_supported = False  # mask has bit3
        self._msd_supported = False  # mask has bit5 AND not Pascal (Pascal: SET N/A)
        self._host_supported = False  # mask has bit9
        self._is_resize_active = False
        self._pending_limits = None  # type: Optional[Dict[str, Any]]
        self._pending_capabilities = None  # type: Optional[Dict[str, Any]]
        self._pending_vfp_state = None  # type: Optional[Tuple[bool, Optional[int]]]
        self._supported_pstates = []  # type: List[str]

        scroll = ctk.CTkScrollableFrame(self.frame)
        scroll.pack(fill="both", expand=True, padx=10, pady=10)
        install_mousewheel_support(scroll)

        # Mutable defaults – updated by update_limits() when real GPU info arrives
        self._power_default = self._DEFAULTS["power_limit_default"]
        self._thermal_default = self._DEFAULTS["thermal_limit_default"]

        d = self._DEFAULTS

        # Top panels (V/F Offsets + Power & Thermal Limits) can be hosted by
        # the dashboard (integration mode) or live in this tab's scroll frame.
        content_host = content_parent if content_parent is not None else scroll
        content_row = tk.Frame(content_host, bg=_PANEL_BG)
        content_row.pack(fill="x", pady=(0, 10))
        # uniform: strictly equal card widths regardless of requested sizes
        content_row.grid_columnconfigure(0, weight=1, uniform="oc_cards")
        content_row.grid_columnconfigure(1, weight=1, uniform="oc_cards")

        # ═══════════════════════════════════════════
        # Clock Offset (OC) — 3-page pager (pre-Pascal: no pager, Core/Mem only)
        # ═══════════════════════════════════════════
        # Page 0: Core / Mem (pstate20, -104 fallback → ClkDomains bit0/bit2)
        # Page 1: Xbar / Sys (ClkDomains bit1/bit3; 30+ couples bit1→bit3 -f)
        # Page 2: Msd / Host (bit5/bit9; Pascal MSD N/A, host via bit9 in mask)
        oc_frame = ctk.CTkFrame(
            content_row, border_width=1, border_color="#1f4e79", corner_radius=10
        )
        oc_frame.grid(row=0, column=0, sticky="new", padx=(0, 5))
        oc_header = tk.Frame(oc_frame, bg=_PANEL_BG)
        oc_header.pack(fill="x", padx=10, pady=(10, 9))
        tk.Label(
            oc_header,
            text="⚡ V/F Offsets",
            font=_FONT_HEADER,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        ).pack(side="left")
        # Pager < > + page indicator — packed between title and API selector,
        # hidden entirely on pre-Pascal (no fabric pages to flip).
        pager = tk.Frame(oc_header, bg=_PANEL_BG)
        pager.pack(side="left", padx=(8, 0))
        self._oc_btn_prev = LiteButton(
            pager,
            text="◂",
            width=26,
            command=lambda: self._oc_page_step(-1),
        )
        self._oc_btn_prev.pack(side="left", padx=(0, 2))
        self._oc_page_indicator = tk.Label(
            pager,
            text="1/1",
            font=_FONT_BODY,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
            width=4,
        )
        self._oc_page_indicator.pack(side="left")
        self._oc_btn_next = LiteButton(
            pager,
            text="▸",
            width=26,
            command=lambda: self._oc_page_step(1),
        )
        self._oc_btn_next.pack(side="left", padx=(2, 0))
        pager.pack_forget()  # hidden until check_capabilities enables it
        self._oc_pager = pager
        HoverTooltip(
            self._oc_btn_next,
            "Fabric-clock pages: Xbar/Sys, Msd/Host. Pre-Pascal cards have "
            "Core/Mem only — no pager.",
        )
        self.oc_api_var = ctk.StringVar(value="NVAPI")
        self.oc_api_selector = ctk.CTkOptionMenu(
            oc_header,
            values=["NVAPI", "NVML"],
            variable=self.oc_api_var,
            width=84,
            anchor="center",
            font=ct_button_font(oc_header),
            height=28,
            command=self._on_oc_api_changed,
        )
        self.oc_api_selector.pack(side="right")
        oc_api_tip = (
            "Clock offset API selector (core/memory + PState lock).\n"
            "- NVAPI: --core-offset / --mem-offset values are in kHz.\n"
            "- NVML: --core-offset / --mem-offset values are in MHz.\n"
            "NVAPI-only pages (Xbar/Sys, Msd/Host) grey out under NVML."
        )
        HoverTooltip(self.oc_api_selector, oc_api_tip)

        # PState lock selector (spans all pages)
        ps_row = tk.Frame(oc_frame, bg=_PANEL_BG)
        ps_row.pack(fill="x", padx=(26, 10), pady=(0, 5))
        ps_row.grid_columnconfigure(1, weight=1)
        tk.Label(
            ps_row,
            text="PState 🔒:",
            anchor="w",
            font=_FONT_BODY,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        ).grid(row=0, column=0, sticky="nw", pady=(5, 0))
        self.pstate_selector = SegmentRangeSelector(ps_row, values=[])
        self.pstate_selector.grid(row=0, column=1, sticky="ew", padx=(2, 8))
        ps_btns = tk.Frame(ps_row, bg=_PANEL_BG)
        ps_btns.grid(row=0, column=2, sticky="ne", pady=(4, 0))
        self.btn_apply_pstate = LiteButton(
            ps_btns, text="✅", width=34, command=self._apply_pstate_lock
        )
        self.btn_apply_pstate.pack(side="left", padx=(0, 5))
        self.btn_unlock_pstate = LiteButton(
            ps_btns,
            text="🔄",
            width=34,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._unlock_pstate_lock,
        )
        self.btn_unlock_pstate.pack(side="left")
        self.set_supported_pstates([])

        # ── Page frames ──
        # Three containers; only the current page is packed. Pre-Pascal only
        # builds page 0 and never shows the pager. padx=2 keeps the page
        # frames OFF the card border: CTk draws the border manually on its
        # canvas (the widget's bd is 0), so a padx=0 child spans the widget's
        # full bounds and paints _PANEL_BG right over the left/right border
        # lines — the border visually vanished for the page's whole span.
        page_core = tk.Frame(oc_frame, bg=_PANEL_BG)
        page_core.pack(fill="x", padx=2, pady=(0, 3))
        page_fabric = tk.Frame(oc_frame, bg=_PANEL_BG)
        page_uncore = tk.Frame(oc_frame, bg=_PANEL_BG)
        self._oc_page_frames = [page_core, page_fabric, page_uncore]
        self._oc_n_pages = 1  # raised to 3 once Pascal+ capabilities land

        # ── Page 0: Core / Mem ──
        # Core: step 2.5 MHz / one decimal — LCM grid for 7.5 (30+) and
        # 12.5 (10/16/20) hardware steps. entry_width=8: "+122.5" needs it.
        (
            self.core_slider,
            self.core_entry,
            self.core_var,
            btn_apply_core,
        ) = self._make_slider_row(
            page_core,
            "Core:",
            d["core_clock_min"],
            d["core_clock_max"],
            0,
            step=2.5,
            apply_cmd=self._apply_core_only,
            signed=True,
            unit="MHz",
            decimals=1,
            entry_width=8,
            volt_bit=0,
        )
        self.mem_slider, self.mem_entry, self.mem_var, btn_apply_mem = (
            self._make_slider_row(
                page_core,
                "Mem:",
                d["mem_clock_min"],
                d["mem_clock_max"],
                0,
                step=5,
                apply_cmd=self._apply_mem_only,
                signed=True,
                unit="MHz",
                volt_bit=2,
            )
        )
        # The ClkDomains slot-1 volt plane (unit toggle) exists from Pascal —
        # page 0's Core/Mem toggles stay hidden until _enable_oc_pager turns
        # them on. The fabric/uncore rows sit on pager-gated pages already.
        for _slider in (self.core_slider, self.mem_slider):
            self._set_unit_toggle_visible(_slider, False)
        btn_apply_mem.configure(shift_command=self._apply_mem_with_sync)
        HoverTooltip(
            btn_apply_mem,
            "Shift+Click: apply global offset then sync P2 memory VFP to P0 frequency",
        )
        # Volt-plane per-row resets (↺ next to ✓): built like the fabric
        # rows' but VOLT-ONLY — the MHz plane's reset is the section reset's
        # public path (pstate20/NVML), not a ClkDomains write. The chip
        # toggle grids/grid_removes them with the plane.
        self.btn_reset_core = self._make_domain_reset_button(
            btn_apply_core, "Core", 0, self.core_slider, self.core_var
        )
        self.btn_reset_mem = self._make_domain_reset_button(
            btn_apply_mem, "Mem", 2, self.mem_slider, self.mem_var
        )
        self.core_slider._oc_reset_volt_only = True
        self.mem_slider._oc_reset_volt_only = True
        self.btn_reset_core.grid_remove()
        self.btn_reset_mem.grid_remove()

        # ── Page 1: Xbar / Sys ──
        # Xbar = ClkDomains bit1. 30系+Ada couples bit1→SYS, so超 XBAR f writes
        # bit1=+f AND bit3=-f to cancel the SYS drift; 10/16/20/Pascal直写 bit1.
        # Range ±500 MHz / 5 MHz step — let the driver reject unsafe writes
        # (snapshot/SET/readback/restore guards the write itself).
        (
            self.xbar_slider,
            self.xbar_entry,
            self.xbar_var,
            self.btn_apply_xbar,
        ) = self._make_slider_row(
            page_fabric,
            "Xbar:",
            -500,
            500,
            0,
            step=5,
            apply_cmd=self._apply_xbar_only,
            signed=True,
            unit="MHz",
            volt_bit=1,
        )
        self.btn_reset_xbar = self._make_domain_reset_button(
            self.btn_apply_xbar, "Xbar", 1, self.xbar_slider, self.xbar_var
        )
        # Sys = ClkDomains bit3 (纯 SYS). RMW: read bit3 current offset, +f, write
        # back (preserves any Xbar抵消 already on bit3).
        self.sys_slider, self.sys_entry, self.sys_var, self.btn_apply_sys = (
            self._make_slider_row(
                page_fabric,
                "Sys:",
                -500,
                500,
                0,
                step=5,
                apply_cmd=self._apply_sys_only,
                signed=True,
                unit="MHz",
                volt_bit=3,
            )
        )
        self.btn_reset_sys = self._make_domain_reset_button(
            self.btn_apply_sys, "Sys", 3, self.sys_slider, self.sys_var
        )

        # ── Page 2: Msd / Host ──
        # Msd = bit5 (Pascal: SET N/A → greyed). Host = bit9 (presence via
        # controllable mask: 0x3FF has bit9, 0xFF does not).
        self.msd_slider, self.msd_entry, self.msd_var, self.btn_apply_msd = (
            self._make_slider_row(
                page_uncore,
                "Msd:",
                -500,
                500,
                0,
                step=5,
                apply_cmd=self._apply_msd_only,
                signed=True,
                unit="MHz",
                volt_bit=5,
            )
        )
        self.btn_reset_msd = self._make_domain_reset_button(
            self.btn_apply_msd, "Msd", 5, self.msd_slider, self.msd_var
        )
        self.host_slider, self.host_entry, self.host_var, self.btn_apply_host = (
            self._make_slider_row(
                page_uncore,
                "Host:",
                -500,
                500,
                0,
                step=5,
                apply_cmd=self._apply_host_only,
                signed=True,
                unit="MHz",
                volt_bit=9,
            )
        )
        self.btn_reset_host = self._make_domain_reset_button(
            self.btn_apply_host, "Host", 9, self.host_slider, self.host_var
        )
        # Fabric/uncore pages start hidden until check_capabilities enables the
        # pager (Pascal+). Their slider rows live inside page_fabric/uncore,
        # which themselves are not packed until paged-to.
        page_fabric.pack_forget()
        page_uncore.pack_forget()

        # Buttons — apply/reset the CURRENT page
        btn_oc = tk.Frame(oc_frame, bg=_PANEL_BG)
        btn_oc.pack(fill="x", padx=(26, 10), pady=(5, 10))
        btn_oc.columnconfigure(0, weight=1, uniform="oc_btns")
        btn_oc.columnconfigure(1, weight=1, uniform="oc_btns")
        self._oc_buttons_row = btn_oc
        self.btn_apply_oc = LiteButton(
            btn_oc, text="✅ Apply Section", width=10, command=self._apply_oc
        )
        self.btn_apply_oc.grid(row=0, column=0, sticky="ew", padx=(0, 5))
        self.btn_reset_oc = LiteButton(
            btn_oc,
            text="🔄 Reset Section",
            width=10,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_oc,
        )
        self.btn_reset_oc.grid(row=0, column=1, sticky="ew", padx=(5, 0))

        # ═══════════════════════════════════════════
        # Power & Thermal Limits
        # ═══════════════════════════════════════════
        self.limit_frame = ctk.CTkFrame(
            content_row, border_width=1, border_color="#1f4e79", corner_radius=10
        )
        self.limit_frame.grid(row=0, column=1, sticky="new", padx=(5, 0))
        limit_header = tk.Frame(self.limit_frame, bg=_PANEL_BG)
        limit_header.pack(fill="x", padx=10, pady=(10, 13))
        self.limit_title_label = tk.Label(
            limit_header,
            text="⚡ Power & Thermal Limits",
            font=_FONT_HEADER,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        )
        self.limit_title_label.pack(side="left")
        self.power_api_var = ctk.StringVar(value="NVAPI")
        self.power_api_selector = ctk.CTkOptionMenu(
            limit_header,
            values=["NVAPI", "NVML"],
            variable=self.power_api_var,
            width=84,
            anchor="center",
            font=ct_button_font(limit_header),
            height=28,
            command=self._on_power_api_changed,
        )
        self.power_api_selector.pack(side="right")
        power_api_tip = (
            "Power limit API selector (power slider only).\n"
            "- NVAPI: --power-limit is percentage (%).\n"
            "- NVML: --power-limit is watts (W)."
        )
        HoverTooltip(self.power_api_selector, power_api_tip)
        self.limit_status_label = tk.Label(
            self.limit_frame,
            text="Power / thermal controls are unsupported on mobile/laptop GPUs.",
            font=_FONT_BODY,
            bg=_PANEL_BG,
            fg=_TEXT_FG_FAINT,
        )

        # ── Mobile-mode widgets (packed only when a mobile GPU is active) ──
        self.ppab_var = ctk.BooleanVar(value=False)
        # CTk-styled checkbox with the box to the RIGHT of the text.
        self.ppab_checkbox = LiteCheckbutton(
            limit_header,
            text="PPAB",
            variable=self.ppab_var,
            command=self._on_ppab_toggled,
            font=_FONT_BODY,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        )
        ppab_tip = (
            "PPAB / Dynamic Boost (NVAPI, mobile only).\n"
            "CPU↔GPU dynamic power shifting (set-dynamic-boost).\n"
            "No read-back API exists; enabling the panel turns it on."
        )
        HoverTooltip(self.ppab_checkbox, ppab_tip)

        self.dnotifier_row = tk.Frame(self.limit_frame, bg=_PANEL_BG)
        self.dnotifier_row.grid_columnconfigure(1, weight=1)
        tk.Label(
            self.dnotifier_row,
            text="D-Notifier:",
            anchor="w",
            font=_FONT_BODY,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        ).grid(row=0, column=0, sticky="nw", pady=(5, 0))
        # Selection only applies via the ✓ button (mirrors the slider rows).
        self.dnotifier_selector = SegmentToggleSelector(self.dnotifier_row, values=[])
        self.dnotifier_selector.grid(row=0, column=1, sticky="ew", padx=(2, 8))
        self.btn_apply_dnotifier = LiteButton(
            self.dnotifier_row,
            text="✓",
            width=34,
            command=self._apply_dnotifier,
        )
        self.btn_apply_dnotifier.grid(row=0, column=2, sticky="ne", pady=(4, 0))
        dnotifier_tip = (
            "D-Notifier level (D1-D5, mobile NVAPI).\n"
            "The actual power cap never exceeds the active D-state's\n"
            "watt budget shown under each level (set-dnotifier)."
        )
        HoverTooltip(self.dnotifier_selector, dnotifier_tip)

        # Power Limit slider + entry
        self.plimit_label_var = ctk.StringVar(value="Pwr Limit:")
        self.plimit_unit_var = ctk.StringVar(value="%")
        (
            self.plimit_slider,
            self.plimit_entry,
            self.plimit_var,
            self.btn_apply_plimit,
        ) = self._make_slider_row(
            self.limit_frame,
            self.plimit_label_var,
            d["power_limit_min"],
            d["power_limit_max"],
            d["power_limit_default"],
            apply_cmd=self._apply_plimit_only,
            unit=self.plimit_unit_var,
        )

        # Thermal Limit slider + entry
        (
            self.tlimit_slider,
            self.tlimit_entry,
            self.tlimit_var,
            self.btn_apply_tlimit,
        ) = self._make_slider_row(
            self.limit_frame,
            "Thrm Limit:",
            d["thermal_limit_min"],
            d["thermal_limit_max"],
            d["thermal_limit_default"],
            apply_cmd=self._apply_tlimit_only,
            unit="℃",
        )

        # Volt Limit slider + entry (mobile-only absolute voltage target on the
        # private VoltRails path). Min is a hard 300 mV floor; max is overridden
        # by min(VBIOS, VRM) walls in update_mobile_limits(); the starting
        # position is the effective voltage wall (post-clamp). Desktop leaves
        # this row packed-away — VoltBoost is the desktop voltage control.
        # Step is 2.5 mV (the LCM-friendly grid that divides both the 5 mV
        # rail step on 30/40-series and the 12.5 mV step on 10/20-series);
        # decimals=1 renders that half-mV in the entry.
        self.vlimit_label_var = ctk.StringVar(value="Volt Limit:")
        (
            self.vlimit_slider,
            self.vlimit_entry,
            self.vlimit_var,
            self.btn_apply_vlimit,
        ) = self._make_slider_row(
            self.limit_frame,
            self.vlimit_label_var,
            300,
            1200,
            1085,
            step=2.5,
            apply_cmd=self._apply_vlimit_only,
            unit="mV",
            decimals=1,
            entry_width=8,
        )

        # Voltage Boost / Offset slider + entry
        self.vboost_label_var = ctk.StringVar(value="VoltBoost:")
        self.vboost_unit_var = ctk.StringVar(value="%")
        (
            self.vboost_slider,
            self.vboost_entry,
            self.vboost_var,
            self.btn_apply_vboost,
        ) = self._make_slider_row(
            self.limit_frame,
            self.vboost_label_var,
            d["voltage_boost_min"],
            d["voltage_boost_max"],
            0,
            step=100,
            apply_cmd=self._apply_vboost_only,
            unit=self.vboost_unit_var,
        )

        # Volt Limit row is mobile-only; hide it on the default desktop layout.
        self.vlimit_slider.master.pack_forget()

        btn_limits = tk.Frame(self.limit_frame, bg=_PANEL_BG)
        btn_limits.pack(fill="x", padx=(26, 10), pady=(5, 10))
        btn_limits.columnconfigure(0, weight=1, uniform="oc_btns")
        btn_limits.columnconfigure(1, weight=1, uniform="oc_btns")
        self.btn_apply_limits = LiteButton(
            btn_limits, text="✅ Apply Section", width=10, command=self._apply_limits
        )
        self.btn_apply_limits.grid(row=0, column=0, sticky="ew", padx=(0, 5))
        self.btn_reset_all = LiteButton(
            btn_limits,
            text="🔄 Reset Section",
            width=10,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_all,
        )
        self.btn_reset_all.grid(row=0, column=1, sticky="ew", padx=(5, 0))

        fan_host = fan_parent if fan_parent is not None else scroll
        self.fan_section = FanControlPane(fan_host, self.app.backend, embedded=True)
        self._limit_enabled_frame_color = self.limit_frame.cget("fg_color")
        self._limit_dim_frame_color = ("gray86", "gray20")
        self._limit_enabled_title_color = _TEXT_FG  # tk.Label: plain fg color
        self._limit_dim_title_color = "gray55"

    # ────────────────────────────────────────────
    # Dynamic limit update from GPU info
    # ────────────────────────────────────────────
    def _safe_get_state(self, widget) -> str:
        try:
            return widget.cget("state")
        except Exception:
            return "normal"

    def _safe_set_state(self, widget, state: str):
        try:
            widget.configure(state=state)
        except Exception:
            pass

    def _set_limit_panel_supported(self, supported: bool):
        """Back-compat shim: map a boolean onto the panel mode machine."""
        self._set_limit_panel_mode("desktop" if supported else "off")

    def _set_limit_panel_mode(self, mode: str):
        """Switch the power/thermal section between desktop/mobile/off modes.

        desktop: original NVAPI%/NVML-W layout.
        mobile: NVAPI-only mobile layout (PPAB checkbox, D-Notifier selector,
                TGP watts slider, target-temp slider).
        off: dimmed/unsupported.
        """
        if mode == self._limit_panel_mode:
            return
        self._limit_panel_mode = mode
        supported = mode != "off"

        if mode == "mobile":
            self._enter_mobile_mode()
        else:
            self._exit_mobile_mode()

        state = "normal" if supported else "disabled"
        widgets = [
            self.plimit_slider,
            self.plimit_entry,
            self.btn_apply_plimit,
            self.tlimit_slider,
            self.tlimit_entry,
            self.btn_apply_tlimit,
            self.btn_apply_limits,
            self.btn_reset_all,
        ]
        if mode == "mobile":
            widgets.append(self.dnotifier_selector)
            widgets.append(self.btn_apply_dnotifier)
            widgets.append(self.vlimit_slider)
            widgets.append(self.vlimit_entry)
            widgets.append(self.btn_apply_vlimit)
        else:
            widgets.extend(
                [
                    self.power_api_selector,
                    self.vboost_slider,
                    self.vboost_entry,
                    self.btn_apply_vboost,
                ]
            )
        for widget in widgets:
            self._safe_set_state(widget, state)

        # Card background stays unchanged in the dim state: repainting it gray
        # would mismatch the fixed-bg canvases/labels inside.
        self.limit_title_label.configure(
            fg=self._limit_enabled_title_color
            if supported
            else self._limit_dim_title_color
        )

        if supported:
            self.limit_status_label.pack_forget()
        elif not self.limit_status_label.winfo_manager():
            self.limit_status_label.pack(anchor="w", padx=10, pady=(0, 6))

        # Re-apply the OC-backend gate: the loop above just re-enabled the
        # mobile widgets, which would clear an active NVML greying.
        self._refresh_nvapi_only_rows()

    # ────────────────────────────────────────────
    # Mobile (laptop GPU) power & thermal mode
    # ────────────────────────────────────────────
    def _enter_mobile_mode(self):
        """Swap the limit panel into its mobile layout. Idempotent."""
        if self._mobile_mode:
            return
        self._mobile_mode = True
        self.power_api_selector.pack_forget()
        self.ppab_checkbox.pack(side="right", padx=(0, 8))
        self.plimit_unit_var.set("W")
        # D-Notifier selector sits above the power slider row.
        plimit_row = self.plimit_slider.master
        tlimit_row = self.tlimit_slider.master
        self.dnotifier_row.pack(fill="x", padx=(26, 10), pady=(0, 3), before=plimit_row)
        # Voltage Boost row is hidden on mobile (unvalidated NVAPI % path).
        self.vboost_slider.master.pack_forget()
        # Tighten the three mobile slider rows (Pwr/Thrm/Volt): repack with a
        # smaller pady than the desktop default (3) so they read as a group.
        # Repack in front-to-back order using before= against an already-packed
        # later sibling so relative order is preserved (dnotifier, plimit,
        # tlimit, vlimit, buttons).
        btn_limits_row = self.btn_apply_limits.master
        self.vlimit_slider.master.pack(
            fill="x", padx=(26, 10), pady=1, before=btn_limits_row
        )
        tlimit_row.pack(
            fill="x", padx=(26, 10), pady=1, before=self.vlimit_slider.master
        )
        plimit_row.pack(fill="x", padx=(26, 10), pady=1, before=tlimit_row)
        self._load_mobile_limits()

    def _exit_mobile_mode(self):
        """Restore the desktop limit panel layout. Idempotent."""
        if not self._mobile_mode:
            return
        self._mobile_mode = False
        self.ppab_checkbox.pack_forget()
        self.dnotifier_row.pack_forget()
        self.vlimit_slider.master.pack_forget()
        # Restore the desktop layout + pady=3. Repack the vboost row before the
        # buttons row (original order), then anchor plimit/tlimit before it so
        # the final order is plimit, tlimit, vboost, buttons — matching the
        # construction order — with pady restored to 3.
        btn_limits_row = self.btn_apply_limits.master
        self.vboost_slider.master.pack(fill="x", padx=10, pady=3, before=btn_limits_row)
        tlimit_row = self.tlimit_slider.master
        plimit_row = self.plimit_slider.master
        tlimit_row.pack(
            fill="x", padx=(26, 10), pady=3, before=self.vboost_slider.master
        )
        plimit_row.pack(fill="x", padx=(26, 10), pady=3, before=tlimit_row)
        self.power_api_selector.pack(side="right")
        self.plimit_unit_var.set("%")

    def _load_mobile_limits(self):
        """Background-load the mobile control surface (TGP range, D-Notifier,
        target-temp policies) via pynvoc and apply it on the UI thread."""
        gpu = self.app.selected_gpu_target()
        if gpu is None or self._mobile_load_in_flight:
            return
        self._mobile_load_in_flight = True
        backend = self.app.backend

        def worker():
            try:
                data = backend.query_mobile_limits(gpu)
            except Exception as exc:
                data = {"error": str(exc)}
            try:
                self.frame.after(0, lambda: self._mobile_limits_loaded(gpu, data))
            except Exception:
                self._mobile_load_in_flight = False

        try:
            self.app.run_background("mobile-limits", worker)
        except Exception:
            self._mobile_load_in_flight = False
            raise

    def _mobile_limits_loaded(self, gpu: str, data: dict):
        self._mobile_load_in_flight = False
        if not self._mobile_mode:
            return
        self._mobile_limits_gpu = gpu
        self.update_mobile_limits(data)

        # PPAB has no read-back API; enable it once per GPU on panel load.
        # Only attempt when the private NVAPI surface actually resolved —
        # on Linux (libnvidia-api stub) / older drivers the setters are
        # NO_IMPLEMENTATION and auto-enabling would just log an error.
        tgp_ok = isinstance(data.get("tgp"), dict) and data.get("tgp")
        dnotifier_ok = isinstance(data.get("dnotifier"), dict) and data.get("dnotifier")
        if (tgp_ok or dnotifier_ok) and self._mobile_ppab_initialized_for != gpu:
            self._mobile_ppab_initialized_for = gpu
            self._syncing = True
            self.ppab_var.set(True)
            self._syncing = False
            self.app.run_native_action(
                "enable dynamic boost",
                lambda native, gpu=gpu: (
                    native.set_ppab_status(gpu, True) or "Dynamic Boost (PPAB) enabled."
                ),
            )

    def update_mobile_limits(self, data: dict):
        """Apply the mobile control surface (see NativeBackend.query_mobile_limits).

        Mobile controls are assumed SUPPORTED: a missing sub-query is treated
        as a transient read failure (dGPU powering up after GC6 wake), not a
        capability verdict — controls stay enabled with the last/default
        ranges, and a genuinely failing SET reports through the console.
        """
        if not self._mobile_mode:
            return
        tgp = data.get("tgp") if isinstance(data.get("tgp"), dict) else None
        dnotifier = (
            data.get("dnotifier") if isinstance(data.get("dnotifier"), dict) else None
        )
        policies = data.get("temp_policies") or []

        if tgp and tgp.get("max_watt") is not None and tgp.get("min_watt") is not None:
            self._tgp_policy_index = int(tgp.get("policy_index", 2))
            min_w = int(round(float(tgp["min_watt"])))
            max_w = int(round(float(tgp["max_watt"])))
            self._power_default = int(
                round(float(tgp.get("default_watt") or tgp.get("min_watt")))
            )
            # Position the slider at the actually-effective power wall
            # (``power_limit_w`` from the mobile-limits query — min of the
            # requested TGP and the active D-Notifier cap, i.e. nvidia-smi's
            # PPAB Ceiling "Current"). When the wall sits OUTSIDE the fresh
            # TGP range (a just-applied clamp can leave it a few W off the
            # new bound), CLAMP to the nearest bound — never jump to the
            # VBIOS default, which is neither the old nor the new real wall
            # (the slider used to jump there and it read as a wild value
            # change).
            current_w = data.get("power_limit_w")
            position = self._power_default
            if current_w is not None:
                current_w = int(round(float(current_w)))
                position = max(min_w, min(max_w, current_w))
            self._reconfigure_slider(
                self.plimit_slider,
                self.plimit_var,
                min_w,
                max_w,
                position,
                step=1,
            )

        if dnotifier and dnotifier.get("levels"):
            levels = dnotifier["levels"]
            values = [str(item.get("level", "")).upper() for item in levels]
            subtitles = [
                f"{float(item['watts']):.0f}W"
                if item.get("watts") is not None
                else None
                for item in levels
            ]
            self.dnotifier_selector.set_values(values, subtitles)
            self.dnotifier_selector.set_selection(dnotifier.get("active"))
            self._safe_set_state(self.dnotifier_selector, "normal")

        target = None
        for policy in policies:
            # Only the TargetTemp slot exposes a writable range (min < max);
            # other slots report min == max and must be skipped.
            if (
                isinstance(policy, dict)
                and policy.get("min") is not None
                and policy.get("max") is not None
                and float(policy["max"]) > float(policy["min"])
            ):
                target = policy
                break
        if target is not None:
            current = int(
                round(float(target.get("celsius", target.get("default", 83))))
            )
            self._thermal_default = current
            self._reconfigure_slider(
                self.tlimit_slider,
                self.tlimit_var,
                int(round(float(target["min"]))),
                int(round(float(target["max"]))),
                current,
                step=1,
            )

        # Volt Limit (private VoltRails P0 bounds). Slider floor is a hard
        # 300 mV; the ceiling is min(VBIOS wall, VRM max wall). A wall reading
        # of 0 means 'not reported' and is skipped; both 0 falls back to
        # 1200 mV (the ~1.2 V domain ceiling observed on Ada mobile). The
        # starting position is the effective voltage wall (post-clamp), the
        # analog of TGP positioning at the enforced power limit — NOT the
        # live core voltage, which bounces with load. The 2.5 mV step grid is
        # applied in _volt_limit_bounds_from_p0 (LCM of 5 and 12.5 mV rail
        # steps) so the entry's one-decimal render and the thumb agree.
        vr = data.get("volt_rail") if isinstance(data.get("volt_rail"), dict) else None
        if vr:
            self._volt_rail_bit = self._resolve_volt_rail_bit(vr)
            p0 = vr.get("p0") if isinstance(vr.get("p0"), dict) else None
            if p0:
                min_mv, max_mv, pos_mv = self._volt_limit_bounds_from_p0(p0)
                self._reconfigure_slider(
                    self.vlimit_slider,
                    self.vlimit_var,
                    min_mv,
                    max_mv,
                    pos_mv,
                    step=2.5,
                )
                # Push the refreshed P0 effective wall to the VF curve tab so
                # its light-red boundary line tracks a Volt Limit apply. The
                # hardware walls (floor/ceiling) are cached once by the VF
                # curve's own first-load query and never re-pushed here.
                vfcurve = getattr(self.app, "tab_vfcurve", None)
                if vfcurve is not None and hasattr(vfcurve, "update_p0_effective_wall"):
                    eff = int(p0.get("effective_wall_uV", 0) or 0)
                    if eff > 0:
                        vfcurve.update_p0_effective_wall(eff)

    @staticmethod
    def _volt_limit_bounds_from_p0(p0: dict) -> tuple[float, float, float]:
        """Compute (min_mV, max_mV, current_mV) for the Volt Limit slider.

        - min is a hard 300 mV floor
        - max is min(VBIOS wall, VRM max wall); a wall of 0 ('not reported')
          is skipped; both 0 falls back to 1200 mV (the ~1.2 V domain ceiling
          observed on Ada mobile)
        - current is the effective voltage wall (post-clamp), the analog of
          TGP positioning at the enforced power limit — NOT the live core
          voltage, which bounces with load. Clamped into [min, max].
        Both max and current are snapped to the 2.5 mV grid (LCM of the 5 mV
        rail step on 30/40-series and the 12.5 mV step on 10/20-series) so
        the one-decimal entry text and the canvas thumb agree; max snaps
        DOWN so no offered position exceeds the actual wall.
        """
        STEP = 2.5
        vbios = int(p0.get("vbios_wall_uV", 0) or 0)
        vrm = int(p0.get("vrm_max_wall_uV", 0) or 0)
        walls = [w for w in (vbios, vrm) if w > 0]
        ceiling_uV = min(walls) if walls else 1_200_000
        max_mv = max(300.0, ceiling_uV / 1000.0)
        # int() truncates toward zero == floor for the positive span here.
        max_mv = int(max_mv / STEP) * STEP
        eff = int(p0.get("effective_wall_uV", 0) or 0)
        pos_mv = eff / 1000.0 if eff > 0 else max_mv
        pos_mv = max(300.0, min(max_mv, pos_mv))
        # Snap the starting position to the grid (round to nearest).
        pos_mv = round(pos_mv / STEP) * STEP
        return 300.0, max_mv, max(300.0, pos_mv)

    def _resolve_volt_rail_bit(self, volt_rail: dict) -> int:
        """Pick the VoltRails rail bit to target.

        Uses the first rail descriptor's ``rail_bit`` when descriptors are
        exposed; otherwise falls back to the lowest set bit of ``rail_mask``.
        Single-rail mobile GPUs (e.g. 4060L, mask 0x1) resolve to 0.
        """
        descs = volt_rail.get("rail_descriptors")
        if isinstance(descs, list) and descs:
            first = descs[0]
            if isinstance(first, dict) and first.get("rail_bit") is not None:
                try:
                    return int(first["rail_bit"])
                except (TypeError, ValueError):
                    pass
        mask = volt_rail.get("rail_mask")
        if isinstance(mask, str) and mask:
            try:
                return (int(mask, 16) & -int(mask, 16)).bit_length() - 1
            except ValueError:
                pass
        return 0

    @staticmethod
    def _xbar_supported_from_info(info: dict) -> bool:
        """Xbar support verdict for the GPU described by ``info``.

        Primary signal: the pynvoc ``query_info`` payload's
        ``xbar_supported`` flag, computed in Rust by core's ``gpu_type.rs``
        ``detect_gpu_type`` (name + codename) — the project's single source
        of truth for generation detection. That path is what makes Ada work:
        ``gpu_architecture`` reads 'Unknown:400:7:161' there (the ArchInfo
        enum has no AD variant), so string-matching the arch alone cannot
        see a 40-series.

        Fallback: the local ``_xbar_supported_arch`` heuristic for payloads
        without the flag (CLI-parsed info, NVML-only info, older pynvoc).
        """
        flag = info.get("xbar_supported")
        if isinstance(flag, bool):
            return flag
        return OverclockTab._xbar_supported_arch(
            str(info.get("gpu_architecture", "") or ""),
            str(info.get("codename", "") or ""),
            str(info.get("gpu_name", "") or ""),
        )

    def _query_clk_domain_capabilities(self, info: dict) -> None:
        """Async query the private ClockClient controllable mask and derive
        the per-domain presence for the Sys/Msd/Host rows. Piggybacks on the
        status thread (one escape, never on the render path). Sets
        ``_clk_domain_mask`` + ``_sys/msd/host_supported`` and re-applies the
        grey gate. Pascal MSD is force-disabled (bit5 SET N/A) regardless of
        mask. Host presence = bit9 in the mask (0x3FF vs 0xFF).
        """
        # Pascal verdict computed HERE (main thread, info in hand) — the
        # loaded callback runs later and must not reach for the info payload
        # again (the GUI App has no info cache; reaching for one crashed the
        # callback, leaving Msd/Host greyed forever).
        self._is_pascal_gpu = self._is_pascal_from_info(info)
        if not self._has_oc_pager:
            return
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return

        def worker() -> None:
            try:
                data = self.app.backend.query_private_freq_domain_info(gpu)
            except Exception:
                data = None
            try:
                self.frame.after(0, lambda: self._on_clk_domain_caps_loaded(data))
            except Exception:
                pass

        self.app.run_background("clk-domain-caps", worker)

    def _on_clk_domain_caps_loaded(self, data: Optional[dict]) -> None:
        if not isinstance(data, dict):
            return
        mask_str = data.get("controllable_mask")
        try:
            mask = int(str(mask_str), 0) if mask_str is not None else 0
        except ValueError:
            mask = 0
        self._clk_domain_mask = mask
        self._sys_supported = bool(mask & (1 << 3))
        # MSD: Pascal has no bit5 SET even if the mask claims the record —
        # the generation verdict captured at query time rules it out.
        has_bit5 = bool(mask & (1 << 5))
        self._msd_supported = has_bit5 and not self._is_pascal_gpu
        self._host_supported = bool(mask & (1 << 9))
        self._refresh_nvapi_only_rows()

    @staticmethod
    def _is_pascal_from_info(info: dict) -> bool:
        """Pascal (GTX 10-series) detection for the MSD grey-out (Pascal bit5
        SET N/A). Uses the gpu_series text from pynvoc query_info."""
        series = str(info.get("gpu_series") or "").lower()
        if "10 series" in series:
            return True
        # codename fallback
        codename = str(info.get("codename") or "").lower()
        return codename.startswith("gp")

    @staticmethod
    def _xbar_supported_arch(
        arch_id: str, codename: str = "", gpu_name: str = ""
    ) -> bool:
        """True for Pascal (GTX 10-series) and every newer architecture.

        Pascal is included since the Xbar offset was live-verified there
        (nvoc-cli set-private-freq-domain-global-offset xbar, 2026-08-31),
        and Volta (the server-card generation between P100 and T4) is
        allowed through alongside it. Kepler and older return False — the
        XBAR ClockClient domain postdates them.

        Three signals, in priority order:
        1. Chip codes from the codename or the arch string (gp104, tu106,
           ga102, ad107, gb202 — optionally suffixed ":rev", "-B",
           " (process)"). The codename matters: on Ada the pynvoc ArchInfo
           enum has no AD variant and reports
           ``gpu_architecture = 'Unknown:400:7:161'``, while
           ``codename = 'AD107-B'`` carries the real chip code.
        2. Friendly architecture names (Pascal, Turing, Ampere, Ada,
           Blackwell) from the CLI human output.
        3. Marketing-name fallback: ``RTX 4060`` / ``GTX 1660`` / ``GTX
           1080`` — the model number >= 1000 means 10-series or newer
           (GTX 980/9-series and below stay hidden).
        """
        # 1) chip codes (codename first — it is the reliable one on Ada).
        for raw in (codename, arch_id):
            head = (
                raw.lower().split("(", 1)[0].split(":", 1)[0].split("-", 1)[0].strip()
            )
            if head.startswith(("gp", "gv", "tu", "ga", "ad", "gb")):
                return True
        # 2) friendly names.
        if any(
            name in arch_id.lower()
            for name in ("pascal", "volta", "turing", "ampere", "ada", "blackwell")
        ):
            return True
        # 3) marketing name: RTX/GTX + model number, 1000 = 10-series floor.
        match = __import__("re").search(r"\b(?:rtx|gtx)\s*(\d{3,4})", gpu_name.lower())
        if match:
            return int(match.group(1)) >= 1000
        return False

    @staticmethod
    def _format_volt_rail_target_result(target_mv: float, result: Any) -> str:
        """Build the console message from a ``set_volt_rail_target`` result.

        Unlike the other NVAPI setters (``set_tgp_watt`` etc. return ``None``
        so ``setter() or "msg"`` works), ``set_volt_rail_target`` always
        returns a dict — either the applied payload (with the post-clamp
        ``effective_wall_uV``) or ``{"supported": False}``. A truthy dict
        would short-circuit ``or`` and yield the dict itself (which then fails
        ``.endswith`` in the native worker), so format the message here.
        ``target_mv`` may carry one decimal (2.5 mV grid); :g drops the
        trailing .0 for whole-mV values.
        """
        if isinstance(result, dict):
            if result.get("applied"):
                eff = result.get("effective_wall_uV")
                if isinstance(eff, (int, float)) and eff:
                    return (
                        f"Successfully applied Volt Limit {target_mv:g} mV "
                        f"(effective wall {eff / 1000.0:g} mV)."
                    )
                return f"Successfully applied Volt Limit {target_mv:g} mV."
            if result.get("supported") is False:
                return "Volt-rail target not supported by this driver."
        return f"Applied Volt Limit {target_mv:g} mV."

    @staticmethod
    def _format_xbar_offset_result(offset_mhz: int, result: Any) -> str:
        """Build the console message from a ``set_clk_domain_offset`` result.

        Like ``set_volt_rail_target``, the pynvoc call returns a dict (the
        applied payload with the driver's readback ``applied_mHz`` (or the
        legacy ``applied_kHz``), or
        ``{"supported": False}``) — never ``None`` — so the message must be
        formatted here rather than via ``setter() or "msg"``.
        """
        if isinstance(result, dict):
            if result.get("applied"):
                applied = result.get("applied_mHz")
                if applied is None:
                    legacy = result.get("applied_kHz")
                    applied = (
                        legacy / 1000.0 if isinstance(legacy, (int, float)) else None
                    )
                if isinstance(applied, (int, float)):
                    return (
                        f"Successfully applied Xbar offset {offset_mhz:+d} MHz "
                        f"(driver readback {applied:+g} MHz)."
                    )
                return f"Successfully applied Xbar offset {offset_mhz:+d} MHz."
            if result.get("supported") is False:
                return "Xbar clock-domain offset not supported by this driver."
        return f"Applied Xbar offset {offset_mhz:+d} MHz."

    def _on_ppab_toggled(self):
        if self._syncing or not self._mobile_mode:
            return
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        active = bool(self.ppab_var.get())
        self.app.run_native_action(
            "toggle dynamic boost",
            lambda native, gpu=gpu, active=active: (
                native.set_ppab_status(gpu, active)
                or f"Dynamic Boost (PPAB) {'enabled' if active else 'disabled'}."
            ),
        )

    def _on_dnotifier_selected(self, level: str):
        """Kept for API compatibility — selection no longer auto-applies."""
        return

    def _apply_dnotifier(self):
        """Apply the currently selected D-Notifier level (✓ button)."""
        if self._syncing or not self._mobile_mode:
            return
        level = self.dnotifier_selector.get_selection()
        if not level:
            return
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        try:
            d_level = int(str(level).strip().upper().lstrip("D"))
        except ValueError:
            return
        if not 1 <= d_level <= 5:
            return

        def on_finished(_code):
            # D-Notifier SET clamps the TGP wall — refresh levels + slider.
            self._load_mobile_limits()

        self.app.run_native_action(
            "set D-Notifier level",
            lambda native, gpu=gpu, d_level=d_level: (
                native.set_dnotifier(gpu, d_level)
                or f"Successfully set D-Notifier to D{d_level}."
            ),
            on_finished=on_finished,
        )

    def on_resize_state_changed(self, resizing: bool, force_flush: bool = False):
        """Coalesce expensive slider/state updates during active resize."""
        self._is_resize_active = resizing
        if (not resizing) and force_flush:
            if self._pending_capabilities is not None:
                pending = self._pending_capabilities
                self._pending_capabilities = None
                self.check_capabilities(pending)
            if self._pending_limits is not None:
                pending = self._pending_limits
                self._pending_limits = None
                self.update_limits(pending)
            if self._pending_vfp_state is not None:
                pending = self._pending_vfp_state
                self._pending_vfp_state = None
                self.set_vfp_state(*pending)

        cb = getattr(self.fan_section, "on_resize_state_changed", None)
        if callable(cb):
            cb(resizing=resizing, force_flush=force_flush)

    def update_limits(self, limits: Dict[str, Any]):
        """
        Update slider ranges with real hardware limits from GPU info.

        Expected keys (all optional):
            core_clock_min, core_clock_max,  [core_clock_current]
            mem_clock_min,  mem_clock_max,   [mem_clock_current]
            power_limit_min, power_limit_max, power_limit_default, [power_limit_current]
            power_limit_nvml_min_w, power_limit_nvml_max_w, [power_limit_nvml_current_w]
            thermal_limit_min, thermal_limit_max, thermal_limit_default, [thermal_limit_current]
            [voltage_boost_current]
        """
        if self._is_resize_active:
            if self._pending_limits is None:
                self._pending_limits = dict(limits)
            else:
                self._pending_limits.update(limits)
            return

        # Store current states of control widgets that may be disabled by check_capabilities
        plimit_entry_state = self._safe_get_state(self.plimit_entry)
        plimit_btn_state = self._safe_get_state(self.btn_apply_plimit)
        tlimit_entry_state = self._safe_get_state(self.tlimit_entry)
        tlimit_btn_state = self._safe_get_state(self.btn_apply_tlimit)
        vboost_entry_state = self._safe_get_state(self.vboost_entry)
        vboost_btn_state = self._safe_get_state(self.btn_apply_vboost)

        # Core/Mem rows are PLANE-AWARE: a row whose unit chip sits on mV
        # must not be re-anchored from the public pstate20 frequency table —
        # that payload's *_clock_current is the FREQUENCY offset, and writing
        # it into an mV row stomps the whole plane (range/step bounce back to
        # the MHz construction values, the entry shows the freq offset). The
        # mV row remembers the MHz range for the toggle-back and re-anchors
        # its value at the record's live ClkDomains slot-1 — after an apply
        # that reads back what the driver actually accepted.
        for slider, var, key_min, key_max, key_cur, mhz_step in (
            (
                self.core_slider,
                self.core_var,
                "core_clock_min",
                "core_clock_max",
                "core_clock_current",
                2.5,
            ),
            (
                self.mem_slider,
                self.mem_var,
                "mem_clock_min",
                "mem_clock_max",
                "mem_clock_current",
                5,
            ),
        ):
            self._update_clock_row_from_limits(
                slider, var, limits, key_min, key_max, key_cur, mhz_step
            )

        if self._mobile_mode:
            # Mobile ranges come from update_mobile_limits() (pynvoc NVAPI
            # private interfaces); the generic %/W desktop keys don't apply.
            if "supported_pstates" in limits:
                self.set_supported_pstates(limits.get("supported_pstates"))
            return

        power_backend = self._selected_power_backend()
        if power_backend == "nvml":
            self.plimit_unit_var.set("W")
            if (
                "power_limit_nvml_min_w" in limits
                and "power_limit_nvml_max_w" in limits
            ):
                min_w = int(limits["power_limit_nvml_min_w"])
                max_w = int(limits["power_limit_nvml_max_w"])
                current_w = limits.get("power_limit_nvml_current_w", min_w)
                default = int(current_w) if current_w is not None else min_w
                self._power_default = default
                self._reconfigure_slider(
                    self.plimit_slider,
                    self.plimit_var,
                    min_w,
                    max_w,
                    default,
                    step=1,
                )
            elif "power_limit_nvml_current_w" in limits:
                self._set_slider_value(
                    self.plimit_slider,
                    self.plimit_var,
                    int(limits["power_limit_nvml_current_w"]),
                )
        else:
            self.plimit_unit_var.set("%")
            if "power_limit_min" in limits and "power_limit_max" in limits:
                default_raw = limits.get("power_limit_default", 100)
                default = int(default_raw) if default_raw is not None else 100
                self._power_default = default
                current_raw = limits.get("power_limit_current", default)
                current = int(current_raw) if current_raw is not None else default
                self._reconfigure_slider(
                    self.plimit_slider,
                    self.plimit_var,
                    limits["power_limit_min"],
                    limits["power_limit_max"],
                    current,
                    step=1,
                )
            elif "power_limit_current" in limits:
                self._set_slider_value(
                    self.plimit_slider, self.plimit_var, limits["power_limit_current"]
                )

        if "thermal_limit_min" in limits and "thermal_limit_max" in limits:
            default_raw = limits.get("thermal_limit_default", 83)
            default = int(default_raw) if default_raw is not None else 83
            self._thermal_default = default
            current_raw = limits.get("thermal_limit_current", default)
            current = int(current_raw) if current_raw is not None else default
            self._reconfigure_slider(
                self.tlimit_slider,
                self.tlimit_var,
                limits["thermal_limit_min"],
                limits["thermal_limit_max"],
                current,
                step=1,
            )
        elif "thermal_limit_current" in limits:
            self._set_slider_value(
                self.tlimit_slider, self.tlimit_var, limits["thermal_limit_current"]
            )

        # Prefer explicit legacy overvolt bounds when present.
        # This avoids stale `_is_legacy_gpu` timing from preventing slider updates.
        if (
            "legacy_overvolt_min_mv" in limits
            and "legacy_overvolt_max_mv" in limits
            and "legacy_overvolt_current_mv" in limits
        ):
            current = limits.get("legacy_overvolt_current_mv", 0)
            self.vboost_label_var.set("Overvolt:")
            self.vboost_unit_var.set("mV")
            self._reconfigure_slider(
                self.vboost_slider,
                self.vboost_var,
                int(
                    limits["legacy_overvolt_min_mv"] / 10
                ),  # open too wide down-volt will result in instant crash!!!!!!
                int(limits["legacy_overvolt_max_mv"]) - 1,
                int(current),
                step=1,
            )
            self._set_slider_value(
                self.vboost_slider,
                self.vboost_var,
                int(limits["legacy_overvolt_current_mv"]),
            )

        else:
            current = limits.get("voltage_boost_current")
            min_boost = limits.get("voltage_boost_min")
            max_boost = limits.get("voltage_boost_max")

            if min_boost is not None and max_boost is not None:
                current_val = int(current) if current is not None else int(min_boost)
                self._reconfigure_slider(
                    self.vboost_slider,
                    self.vboost_var,
                    int(min_boost),
                    int(max_boost),
                    current_val,
                    step=1,
                )
                # Keep current value in sync after range changes.
                self._set_slider_value(self.vboost_slider, self.vboost_var, current_val)
            elif current is not None:
                # Partial cache update: only current value is known, keep existing range.
                self._set_slider_value(
                    self.vboost_slider, self.vboost_var, int(current)
                )

        # Restore saved states to entry and button widgets
        self._safe_set_state(self.plimit_entry, plimit_entry_state)
        self._safe_set_state(self.btn_apply_plimit, plimit_btn_state)
        self._safe_set_state(self.tlimit_entry, tlimit_entry_state)
        self._safe_set_state(self.btn_apply_tlimit, tlimit_btn_state)
        self._safe_set_state(self.vboost_entry, vboost_entry_state)
        self._safe_set_state(self.btn_apply_vboost, vboost_btn_state)

        if "supported_pstates" in limits:
            self.set_supported_pstates(limits.get("supported_pstates"))

    def _update_clock_row_from_limits(
        self,
        slider: Any,
        var: ctk.StringVar,
        limits: Dict[str, Any],
        key_min: str,
        key_max: str,
        key_cur: str,
        mhz_step: float,
    ):
        """One Core/Mem row's update_limits handling, plane-aware.

        MHz plane (chip on MHz): the original behavior — full range
        reconfigure from min/max, or value-only from current. mV plane (chip
        on mV): the payload's public freq table is NOT this row's state;
        only remember the MHz range for the toggle-back and re-anchor the
        value at the live ClkDomains slot-1. Payloads that say nothing
        about the row are ignored (no anchor query, no stomp mid-typing).
        """
        if not any(k in limits for k in (key_min, key_max, key_cur)):
            return
        if self._row_volt_mode(slider):
            if key_min in limits and key_max in limits:
                slider._oc_freq_min = limits[key_min]
                slider._oc_freq_max = limits[key_max]
            self._set_slider_value(slider, var, self._query_row_volt_anchor_mv(slider))
            return
        if key_min in limits and key_max in limits:
            self._reconfigure_slider(
                slider,
                var,
                limits[key_min],
                limits[key_max],
                limits.get(key_cur, 0),
                step=mhz_step,
            )
        elif key_cur in limits:
            self._set_slider_value(slider, var, limits[key_cur])

    def set_vfp_state(
        self, has_vfp_offset: bool, uniform_core_offset_mhz: Optional[int] = None
    ):
        """Track whether VFP offsets exist without changing scalar OC controls."""
        if self._is_resize_active:
            self._pending_vfp_state = (has_vfp_offset, uniform_core_offset_mhz)
            return

        self._is_vfp_mode = has_vfp_offset
        self._vfp_uniform_offset_mhz = (
            int(uniform_core_offset_mhz)
            if (has_vfp_offset and uniform_core_offset_mhz is not None)
            else None
        )

    def check_capabilities(self, info: dict):
        """Enable/disable controls based on GPU capabilities."""
        if self._is_resize_active:
            self._pending_capabilities = dict(info)
            return

        # Mobile/Laptop GPU test. Primary signal: the pynvoc query_info
        # payload's is_mobile flag (core gpu_type.rs detect_gpu_type — the
        # single source of truth; it reads name + codename, so it also works
        # on Ada where gpu_architecture is 'Unknown:...'). Fallback: the
        # name-keyword heuristic for payloads without the flag (CLI-parsed
        # info, NVML-only info, older pynvoc).
        gpu_name = str(info.get("gpu_name", "")).lower()
        arch_id = str(info.get("gpu_architecture", "")).lower().strip()
        arch_head = arch_id.split("(", 1)[0].strip().split(":", 1)[0].strip()
        mobile_flag = info.get("is_mobile")
        if isinstance(mobile_flag, bool):
            is_mobile = mobile_flag
        else:
            # Check for mobile/laptop indicators: explicit keywords, RTX XXM
            # (mobile suffix), RTX for laptops with M suffix
            is_mobile = (
                "mobile" in gpu_name
                or "laptop" in gpu_name
                or " m " in gpu_name
                or gpu_name.endswith(" m")
                or " mx " in gpu_name
                or gpu_name.endswith(" mx")
            )

        self._set_limit_panel_mode("mobile" if is_mobile else "desktop")
        if is_mobile and self._mobile_mode:
            # Re-load when switching between two mobile GPUs.
            gpu = self.app.selected_gpu_target()
            if gpu is not None and gpu != self._mobile_limits_gpu:
                self._load_mobile_limits()
        # Fan verdict: mobile GPUs AND fanless server cards both grey the
        # pane out through the same set_supported_state path. Server verdict
        # is synchronous (gpu_type.rs is_server flag in this payload — Tesla
        # P100/A100/… are passive); the async NVML cooler count refines it
        # (ServerLovelace L40/L4 carry onboard fans, count ≥ 1 re-enables).
        fan_gpu = self.app.selected_gpu_target()
        server_flag = info.get("is_server")
        fanless = (fan_gpu is not None and fan_gpu in self._fanless_gpus) or (
            isinstance(server_flag, bool)
            and server_flag
            and fan_gpu not in self._fanned_gpus
        )
        self.fan_section.set_supported(not is_mobile and not fanless)
        self._refresh_fan_surface(is_mobile)

        # Xbar (private ClockClient domain offset) exists from Pascal — the
        # GTX 10-series — onward. Pascal+ enables the 3-page clock-offset pager
        # (Core/Mem | Xbar/Sys | Msd/Host); pre-Pascal has Core/Mem only (no
        # pager). The pager is driven by the same xbar_supported flag; the
        # per-row presence (Sys/Msd/Host) is refined by the controllable mask
        # queried asynchronously below.
        xbar_ok = self._xbar_supported_from_info(info)
        if xbar_ok != self._has_oc_pager:
            self._xbar_supported = xbar_ok
            self._enable_oc_pager(xbar_ok)
        # 30系+ (Ampere/Ada/Blackwell): bit1 couples SYS → XBAR write must also
        # write bit3=-f to cancel the SYS drift. Pascal/Turing/GTX16 直写 bit1.
        ampere_flag = info.get("is_ampere_plus")
        if isinstance(ampere_flag, bool):
            self._is_ampere_plus = ampere_flag
        # Controllable mask (which WRITE records the driver exposes) + the
        # per-domain presence derived from it. Queried async — the first tick
        # after a GPU switch still has the previous/default mask.
        self._query_clk_domain_capabilities(info)
        # Maxwell / 900 series and older detection. Primary signal: the
        # payload's is_legacy_voltage flag (core gpu_type.rs — 9系 GM 及更旧,
        # 含 Kepler/Fermi 落 Unknown 的保守归类). Fallback heuristic below for
        # payloads without the flag. NOTE the deliberate semantic difference:
        # is_legacy_voltage(Unknown) = true (core's write-path conservatism),
        # so a day-one unrecognized future GPU shows the legacy Overvolt UI
        # here until detect_gpu_type learns its chip prefix.
        legacy_flag = info.get("is_legacy_voltage")
        if isinstance(legacy_flag, bool):
            is_legacy = legacy_flag
        else:
            # Simple heuristic: architectural series usually exposed in info
            # or if missing VFP, fallback arch check from name
            is_legacy = False
            if "gtx" in gpu_name:
                match = __import__("re").search(r"gtx\s*(\d+)", gpu_name)
                if match and int(match.group(1)) < 1000:
                    is_legacy = True
            if not is_legacy and arch_id:
                if any(x in arch_id for x in ["maxwell", "kepler", "fermi"]):
                    is_legacy = True
                elif arch_head.startswith(("gm", "gk", "gf")):
                    is_legacy = True

        if is_legacy:
            self._is_legacy_gpu = True
            if hasattr(self.app, "notebook"):
                # Ideally disable or hide VF Curve entirely if possible via app
                pass
            vfcurve_tab = getattr(self.app, "tab_vfcurve", None)
            if vfcurve_tab is not None and hasattr(vfcurve_tab, "frame"):
                for child in vfcurve_tab.frame.winfo_children():
                    try:
                        child.configure(state="disabled")
                    except Exception:
                        pass

            # Legacy GPUs use Overvolt controls in mV terminology.
            self.vboost_label_var.set("Overvolt:")
            self.vboost_unit_var.set("mV")
        else:
            self._is_legacy_gpu = False
            self.vboost_label_var.set("VoltBoost:")
            self.vboost_unit_var.set("%")

    # ── Fan surface (dropdown / initial level / backend preselect) ──────────
    def _refresh_fan_surface(self, is_mobile: bool) -> None:
        """Query NVML fan info and adapt the Fan Control pane.

        On legacy GPUs (≤ Kepler) the private NVAPI cooler family reports
        zero coolers while NVML still answers (v1 GetFanSpeed: count=1,
        live current percent). In that case the dropdown is restricted to
        the real cooler count (no "Fan 2"), the level slider starts at the
        live current duty, and the backend preselects NVML so Apply goes
        through the working path. Modern GPUs keep the All/Fan1/Fan2
        defaults (NVAPI count is authoritative there).
        """
        if is_mobile or getattr(self, "_fan_surface_load_in_flight", False):
            return
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        backend = self.app.backend
        self._fan_surface_load_in_flight = True
        self._fan_surface_gpu = gpu

        def worker():
            try:
                data = backend.query_fan_info(gpu)
            except Exception:
                data = None
            try:
                self.frame.after(0, lambda: self._fan_surface_loaded(gpu, data))
            except Exception:
                self._fan_surface_load_in_flight = False

        try:
            self.app.run_background("fan-surface", worker)
        except Exception:
            self._fan_surface_load_in_flight = False
            raise

    def _fan_surface_loaded(self, gpu: str, data: Optional[dict]) -> None:
        self._fan_surface_load_in_flight = False
        if not isinstance(data, dict):
            return
        # A GPU switch between dispatch and completion must not re-verdict
        # the pane for the wrong card.
        if gpu != self._fan_surface_gpu:
            return
        try:
            count = data.get("count")
            count = count if isinstance(count, int) else 0
            current = data.get("current_percent")
            current = current if isinstance(current, int) else None

            # Legacy signature: NVML sees fans where the NVAPI cooler family
            # reports none. (A modern GPU's NVML count also matches its NVAPI
            # count, so this branch is legacy-only in practice.)
            if count >= 1:
                self._fanless_gpus.discard(gpu)
                self._fanned_gpus.add(gpu)
                self.fan_section.set_supported(True)
                # Modern NVAPI CoolerPolicy types (continuous etc.) are
                # rejected by legacy drivers — restrict the dropdown to
                # manual/default and default the selection to manual. Keep the
                # NVAPI backend selected: on legacy GPUs the NVML control path
                # binds v2-only symbols (SetFanControlPolicy/SetFanSpeed_v2,
                # absent in R391's nvml.dll), so NVAPI manual is the working
                # control path (verified: `set-fan-speed --nvapi` manual).
                self.fan_section.set_legacy_nvapi(True)
                choices = ["All"] + [f"Fan {i}" for i in range(1, count + 1)]
                self.fan_section.set_fan_choices(choices)
                if count == 1 and current is not None:
                    # Seed the level with the live duty so the slider starts
                    # where the fan actually is.
                    self.fan_section.set_level(max(0, min(100, int(current))))
            elif count == 0:
                # Fanless server card (P100/A100 …): NVML reports zero
                # coolers and the private NVAPI cooler family answers
                # NOT_SUPPORTED — grey the section out (same path mobile
                # takes) instead of leaving dead controls behind.
                self._fanless_gpus.add(gpu)
                self._fanned_gpus.discard(gpu)
                self.fan_section.set_supported(False)
        except Exception:
            pass

    @staticmethod
    def _normalize_pstate_label(value: Any) -> Optional[str]:
        if isinstance(value, str):
            val = value.lower().strip()
            # If the CLI outputs 'p8(locked)', normalise it to just 'p8'
            if "(" in val:
                val = val.split("(")[0].strip()
            return val
        return str(value)

    def set_supported_pstates(self, pstates: Any):
        """Update the available P-State lock points from CLI 'get' output."""
        normalized = []  # type: List[str]
        seen = set()  # type: Set[str]
        for state in pstates or []:
            label = self._normalize_pstate_label(state)
            if not label or label in seen:
                continue
            seen.add(label)
            normalized.append(label)

        # A fresh p-state roster means a get/GPU-switch refresh — drop any
        # mem-range-failure fallback state from the previous GPU (the pin
        # re-arms per GPU when its own mem-range attempt fails).
        self._pstate_pin_fallback = False
        self.pstate_selector.set_point_mode(False)

        self._supported_pstates = normalized
        self.pstate_selector.set_values(normalized)

        state = "normal" if normalized else "disabled"
        self._safe_set_state(self.pstate_selector, state)
        self._safe_set_state(self.btn_apply_pstate, state)
        self._safe_set_state(self.btn_unlock_pstate, state)

    def _on_mem_range_lock_failed(self, start: str, end: str) -> None:
        """Mem-range lock failed at runtime → pre-Kepler part, fall back.

        On these parts the NVML P-State mem-clock-range query the window
        derivation needs is Not Supported (GT730/391.35), so the range lock
        can never succeed — switch to the native single-P-State pin. The pin
        has no range form, so fuse the selector into point-mode and re-apply
        with the range's high-perf endpoint (start).
        """
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        self._pstate_pin_fallback = True
        self.pstate_selector.set_point_mode(True)
        self.app.console.append(
            "[GUI] Memory-range P-State lock unavailable on this GPU — "
            "falling back to the native single-P-State pin "
            f"(P{str(start).lstrip('Pp')}).\n"
        )
        self.app.run_native_action(
            "apply P-State lock",
            lambda native, gpu=gpu, pstate=start: (
                native.set_pstate_native_lock(gpu, pstate)
                or f"Successfully pinned NVAPI P-State {pstate}."
            ),
        )

    @staticmethod
    def _oc_pstate() -> str:
        """Core/memory offset commands always target P0."""
        return "P0"

    def _selected_oc_backend(self) -> str:
        """Return the selected backend for core/memory offset commands."""
        selected = self.oc_api_var.get().strip().upper()
        return "nvml" if selected == "NVML" else "nvapi"

    def _on_oc_api_changed(self, _selected: str):
        """OC backend dropdown moved: re-apply the NVAPI-only row gate."""
        self._refresh_nvapi_only_rows()

    # ── Clock-offset pager ──
    _OC_PAGE_NAMES = ("Core/Mem", "Xbar/Sys", "Msd/Host")

    def _set_oc_page(self, page: int) -> None:
        """Switch the visible clock-offset page (0..n-1). No-op off-range."""
        n = max(1, self._oc_n_pages)
        page = max(0, min(page, n - 1))
        if page == self._oc_page and self._oc_page_frames[page].winfo_ismapped():
            # Same page already on screen — still refresh the chrome:
            # _enable_oc_pager raises _oc_n_pages at capability time and
            # re-points at page 0, and without this refresh the indicator
            # keeps its construct-time "1/1" until the first real flip.
            self._update_oc_pager_chrome(page)
            return
        # repack: hide all, show the target (below ps_row, above btn_oc)
        for i, fr in enumerate(self._oc_page_frames):
            if i < self._oc_n_pages:
                if i == page:
                    fr.pack(fill="x", padx=2, pady=(0, 3), before=self._oc_buttons_row)
                else:
                    fr.pack_forget()
            else:
                fr.pack_forget()
        self._oc_page = page
        self._update_oc_pager_chrome(page)
        self._refresh_nvapi_only_rows()

    def _oc_page_step(self, delta: int) -> None:
        """Round-robin page advance: past the last page wraps to the first
        (and before the first wraps to the last)."""
        n = max(1, self._oc_n_pages)
        self._set_oc_page((self._oc_page + delta) % n)

    def _update_oc_pager_chrome(self, page: int) -> None:
        """Indicator text + arrow states. Round-robin paging has no dead
        ends, so both arrows stay enabled whenever the pager exists."""
        if self._oc_page_indicator is not None:
            self._oc_page_indicator.configure(text=f"{page + 1}/{self._oc_n_pages}")
        self._oc_btn_prev.configure(state="normal")
        self._oc_btn_next.configure(state="normal")

    def _enable_oc_pager(self, enabled: bool) -> None:
        """Show/hide the pager + fabric/uncore pages (Pascal+ vs pre-Pascal)."""
        self._has_oc_pager = enabled
        self._oc_n_pages = 3 if enabled else 1
        # The ClkDomains slot-1 volt plane exists from Pascal — page 0's
        # Core/Mem unit toggles appear/disappear with the pager. (Switching
        # OFF re-hides the chips but leaves any mV mode standing; the rows
        # grey out entirely pre-Pascal anyway.)
        for slider in (self.core_slider, self.mem_slider):
            self._set_unit_toggle_visible(slider, enabled)
        if enabled:
            if not self._oc_pager.winfo_ismapped():
                self._oc_pager.pack(
                    side="left", padx=(8, 0), before=self.oc_api_selector
                )
        else:
            self._oc_pager.pack_forget()
        # ensure only the legal pages exist
        for i, fr in enumerate(self._oc_page_frames):
            if i >= self._oc_n_pages:
                fr.pack_forget()
        self._set_oc_page(0)

    def _refresh_nvapi_only_rows(self):
        """Sync the enabled state of the NVAPI-only pages/rows + Volt Limit.

        NVML exposes neither the private ClockClient domain offsets (Xbar/Sys/
        Msd/Host) nor the VoltRails family (Volt Limit), so the fabric/uncore
        pages grey out while the V/F Offsets backend sits on NVML. Volt Limit
        is only touched in mobile mode (re-applies the gate after
        _set_limit_panel_mode re-enables the mobile widgets).
        """
        nvapi = self._selected_oc_backend() != "nvml"
        state = "normal" if nvapi else "disabled"
        # Page 1 (Xbar/Sys): always shown on Pascal+ but disabled under NVML;
        # each row also gated by its controllable-mask bit.
        if self._has_oc_pager:
            for widget, ok in (
                (self.xbar_slider, True),  # bit1 — present whenever pager is on
                (self.xbar_entry, True),
                (self.btn_apply_xbar, True),
                (self.btn_reset_xbar, True),
                (self.sys_slider, self._sys_supported),
                (self.sys_entry, self._sys_supported),
                (self.btn_apply_sys, self._sys_supported),
                (self.btn_reset_sys, self._sys_supported),
                (self.msd_slider, self._msd_supported),
                (self.msd_entry, self._msd_supported),
                (self.btn_apply_msd, self._msd_supported),
                (self.btn_reset_msd, self._msd_supported),
                (self.host_slider, self._host_supported),
                (self.host_entry, self._host_supported),
                (self.btn_apply_host, self._host_supported),
                (self.btn_reset_host, self._host_supported),
            ):
                self._safe_set_state(widget, state if (nvapi and ok) else "disabled")
        # Volt Limit (mobile) gate unchanged
        if self._limit_panel_mode == "mobile":
            for widget in (
                self.vlimit_slider,
                self.vlimit_entry,
                self.btn_apply_vlimit,
            ):
                self._safe_set_state(widget, state)

    def _selected_power_backend(self) -> str:
        """Return the selected backend for power-limit commands only."""
        if self._mobile_mode:
            return "nvapi"
        selected = self.power_api_var.get().strip().upper()
        return "nvml" if selected == "NVML" else "nvapi"

    def _on_power_api_changed(self, _selected: str):
        """Re-sync power slider unit/range and refresh current values via get."""
        if self._mobile_mode:
            return
        cached = dict(getattr(self.app, "_gpu_limits_cache", {}) or {})
        if cached:
            self.update_limits(cached)
        # get now refreshes both NVML(W) and NVAPI(%) power current values.
        self.app._query_gpu_get()

    def _reconfigure_slider(
        self,
        slider: Any,
        var: ctk.StringVar,
        min_val: float,
        max_val: float,
        default: float,
        step: float = 1,
    ):
        """Reconfigure a slider's range, steps, and reset to default value."""
        # round() (not int()) so a binary-inexact step like 0.1 mV yields
        # 600/0.1 → 5999.99… → 6000 steps, not 5999 — the mV plane's anchor
        # must land ON the grid. Exact-representable steps (2.5, 5, 0.5) are
        # unaffected (round == int there).
        n_steps = round((max_val - min_val) / step) if step else int(max_val - min_val)

        # Preserve the current state (disabled/normal) before reconfiguring
        current_state = self._safe_get_state(slider)

        # Set _syncing BEFORE configure() — CTkSlider fires its command callback
        # internally during configure when number_of_steps changes, which would
        # otherwise overwrite var with the wrong (clamped-to-min) value.
        self._syncing = True
        try:
            slider.configure(
                from_=min_val,
                to=max_val,
                number_of_steps=n_steps,
                state=current_state,
                require_redraw=False,
            )
        except Exception:
            # Fallback for older custom tkinter versions that don't support state in configure
            slider.configure(from_=min_val, to=max_val, number_of_steps=n_steps)
            self._safe_set_state(slider, current_state)

        # Update stored range/step metadata on the slider widget
        slider._oc_min = min_val
        slider._oc_max = max_val
        slider._oc_step = step

        slider.set(default)
        var.set(self._fmt_slider_value(slider, default))
        self._syncing = False

    @staticmethod
    def _fmt_slider_value(slider: Any, value: float) -> str:
        """Format an entry value, signed for offset rows (see _make_slider_row).

        ``:+`` renders an explicit sign on positives AND on zero (``+0.0``) —
        offset rows read 0 as "no offset applied", so the + keeps the column
        visually consistent. Unit-toggle rows render per their ACTIVE plane
        (mV = one decimal) via ``_row_decimals``.
        """
        decimals = (
            OverclockTab._row_decimals(slider)
            if getattr(slider, "_oc_volt_bit", None) is not None
            else getattr(slider, "_oc_decimals", 0)
        )
        signed = getattr(slider, "_oc_signed", False)
        if decimals:
            return f"{float(value):{'+.' if signed else '.'}{decimals}f}"
        if signed:
            return f"{int(value):+d}"
        return str(int(value))

    def _set_slider_value(self, slider: Any, var: ctk.StringVar, value: float):
        """Update a slider's current value without changing its range."""
        min_val = getattr(slider, "_oc_min", float(slider.cget("from_")))
        max_val = getattr(slider, "_oc_max", float(slider.cget("to")))
        clamped = max(min_val, min(max_val, value))
        self._syncing = True
        slider.set(clamped)
        var.set(self._fmt_slider_value(slider, clamped))
        self._syncing = False

    # ── Unit toggle: slot-0 frequency (MHz) ↔ slot-1 voltage (mV) ──────────
    # Each ClkDomains row's bordered unit chip cycles MHz → mV → MHz. mV mode
    # reconfigures the SAME slider+entry onto the ±300 mV plane (so the
    # drag/type interaction is identical to MHz, just different numbers and
    # a one-decimal render) and reroutes the row's apply to the slot-1
    # per-domain V/F-curve voltage addend (set-private-freq-domain-global-
    # offset --volt). The two planes are SEPARATE RM storage — a domain's
    # slot0 and slot1 coexist — so each switch re-anchors the row at the
    # TARGET plane's CURRENT offset: mV mode reads the record's slot1 (µV,
    # from get-private-freq-domain-info); MHz mode anchors at 0 (the
    # frequency plane's current value belongs to the driver, not this row).

    def _toggle_row_unit(self, slider: Any) -> None:
        """Cycle one row's unit chip MHz ↔ mV, reconfiguring the row's own
        slider/entry onto the target plane and re-anchoring at its current
        offset (mV: the record's live slot1; MHz: 0)."""
        var = getattr(slider, "_oc_unit_var", None)
        toggle = getattr(slider, "_oc_unit_toggle", None)
        if var is None or toggle is None:
            return
        volt_mode = not getattr(slider, "_oc_volt_mode", False)
        slider._oc_volt_mode = volt_mode
        try:
            toggle.configure(
                text="mV" if volt_mode else "MHz",
                fg=_ACCENT_FG if volt_mode else _TEXT_FG_DIM,
            )
        except Exception:
            pass
        if volt_mode:
            # Re-anchor at the record's live slot-1 addend (µV → mV, clamped
            # into the ±300 plane). Unreadable/absent → 0.
            anchor = self._query_row_volt_anchor_mv(slider)
            self._reconfigure_slider(
                slider,
                var,
                slider._oc_volt_min,
                slider._oc_volt_max,
                anchor,
                slider._oc_volt_step,
            )
            slider._oc_decimals = 1
        else:
            # Back to the MHz plane: restore the construction range and
            # anchor at 0 (the row's MHz value is an intent, not readback).
            self._reconfigure_slider(
                slider,
                var,
                slider._oc_freq_min,
                slider._oc_freq_max,
                0,
                slider._oc_freq_step,
            )
            slider._oc_decimals = slider._oc_freq_decimals
        # Volt-only ↺ resets (Core/Mem) follow the plane: visible on mV,
        # hidden on MHz. Fabric rows' ↺ stays gridded regardless.
        reset_btn = getattr(slider, "_oc_reset_btn", None)
        if reset_btn is not None and getattr(slider, "_oc_reset_volt_only", False):
            try:
                if volt_mode:
                    reset_btn.grid()
                else:
                    reset_btn.grid_remove()
            except Exception:
                pass
        # The entry var's write callback (_on_entry) re-fires on the
        # re-anchored text; both are already in sync so nothing to do beyond
        # forcing the one-decimal render through the shared formatter.
        self._syncing = True
        var.set(
            self._fmt_slider_value(
                slider, slider.get() if hasattr(slider, "get") else 0
            )
        )
        self._syncing = False

    def _query_row_volt_anchor_mv(self, slider: Any) -> float:
        """Live slot-1 addend (mV) for one row's WRITE record — the anchor
        the row re-anchors at when its chip toggles to mV. Reads
        get-private-freq-domain-info and finds this row's bit's record;
        slot1 lives at values_kHz[1] (µV). Any miss → 0. Synchronous on
        purpose: the toggle click is a user action, a ~ms escape is fine,
        and an async anchor would land after the user already started
        dragging (wrong values would go out)."""
        bit = getattr(slider, "_oc_volt_bit", None)
        if bit is None:
            return 0.0
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return 0.0
        try:
            info = self.app.backend.query_private_freq_domain_info(gpu)
        except Exception:
            return 0.0
        if not isinstance(info, dict):
            return 0.0
        for e in info.get("entries") or []:
            if isinstance(e, dict) and e.get("bit") == bit:
                vals = e.get("values_kHz") or []
                if isinstance(vals, list) and len(vals) > 1:
                    try:
                        return int(vals[1] or 0) / 1000.0
                    except (TypeError, ValueError):
                        return 0.0
                break
        return 0.0

    def _set_unit_toggle_visible(self, slider: Any, visible: bool) -> None:
        """Show/hide one row's unit chip (page-0 rows hide theirs until the
        pager proves the ClkDomains family exists — pre-Pascal has no
        ClockClient WRITE records to toggle into)."""
        toggle = getattr(slider, "_oc_unit_toggle", None)
        if toggle is None:
            return
        try:
            if visible:
                toggle.grid()
            else:
                toggle.grid_remove()
        except Exception:
            pass

    def _row_volt_mode(self, slider: Any) -> bool:
        """True when the row's unit chip sits on mV (slot-1 volt mode)."""
        return getattr(slider, "_oc_volt_mode", False)

    # ────────────────────────────────────────────
    # Helper: create a  Label | Slider | Entry  row
    # ────────────────────────────────────────────

    @staticmethod
    def _row_decimals(slider: Any) -> int:
        """Entry render decimals for a unit-toggle row: 1 in mV mode (the
        ±300 mV plane snaps to the 2.5 mV-ish grid cleanly with one decimal),
        the row's construction decimals in MHz mode. Plain rows keep their
        construction decimals (this only reroutes toggle rows)."""
        if getattr(slider, "_oc_volt_mode", False):
            return 1
        return getattr(slider, "_oc_decimals", 0)

    def _make_slider_row(
        self,
        parent: ctk.CTkFrame,
        label: Union[str, ctk.StringVar],
        min_val: float,
        max_val: float,
        default: float,
        step: float = 1,
        apply_cmd=None,
        signed: bool = False,
        unit: Union[str, ctk.StringVar] = "",
        decimals: int = 0,
        entry_width: int = 7,
        volt_bit: Optional[int] = None,
    ) -> Tuple[Any, ctk.CTkEntry, ctk.StringVar, ctk.CTkButton]:
        """Create a row with label, slider, numeric entry and apply button.

        signed=True renders the entry value with an explicit +/- sign
        (offset rows — avoids reading e.g. 150 as an absolute frequency).

        decimals>0 switches the entry to a fixed-point render (e.g.
        ``decimals=1`` → ``1082.5``) and parses typed input as float — used by
        the Volt Limit row whose 2.5 mV step needs one decimal on 10/20-series.

        volt_bit (ClkDomains WRITE-record bit) replaces the plain unit label
        with a thin-bordered CLICKABLE toggle: MHz → mV → MHz …  mV mode
        reconfigures the SAME slider+entry widgets onto the ±300 mV plane
        (one decimal — the same drag/type interaction as MHz, different
        numbers) and reroutes the row's apply to the slot-1 per-domain V/F-
        curve VOLTAGE addend (the CLI's set-private-freq-domain-global-offset
        --volt). The two planes are separate RM storage, so switching
        re-anchors the row at the target plane's CURRENT offset (mV mode
        reads it from get-private-freq-domain-info; MHz mode anchors at 0 —
        the current offset plane is the driver's, not ours to track).
        """

        def _fmt(v: float) -> str:
            # `:+` also signs zero (+0 / +0.0) — offset rows show 0 as an
            # explicitly-applied zero offset (see _fmt_slider_value).
            # NOTE self._row_decimals, not the bare name — the nested scope
            # chain (locals → method locals → module globals) has no class
            # attributes; the bare reference NameError'd at construction.
            d = self._row_decimals(slider) if volt_bit is not None else decimals
            if d:
                return f"{float(v):{'+.' if signed else '.'}{d}f}"
            return f"{int(v):+d}" if signed else str(int(v))

        row_frame = tk.Frame(parent, bg=_PANEL_BG)
        row_frame.pack(fill="x", padx=(26, 10), pady=3)
        row_frame.grid_columnconfigure(1, weight=1)

        if isinstance(label, ctk.StringVar):
            tk.Label(
                row_frame,
                textvariable=label,
                anchor="w",
                font=_FONT_BODY,
                bg=_PANEL_BG,
                fg=_TEXT_FG,
            ).grid(row=0, column=0, sticky="w")
        else:
            tk.Label(
                row_frame,
                text=label,
                anchor="w",
                font=_FONT_BODY,
                bg=_PANEL_BG,
                fg=_TEXT_FG,
            ).grid(row=0, column=0, sticky="w")

        # Slider
        n_steps = int((max_val - min_val) / step) if step else int(max_val - min_val)
        slider = CanvasSlider(
            row_frame,
            from_=min_val,
            to=max_val,
            number_of_steps=n_steps,
        )
        slider.set(default)
        slider.grid(row=0, column=1, sticky="ew", padx=(2, 8))

        # Store range info on the slider for dynamic access in callbacks
        slider._oc_min = min_val
        slider._oc_max = max_val
        slider._oc_step = step
        slider._oc_signed = signed
        slider._oc_decimals = decimals

        # Entry (fixed width, right-aligned value)
        var = ctk.StringVar(value=_fmt(default))
        entry = LiteEntry(
            row_frame, textvariable=var, width=entry_width, justify="right"
        )
        entry.grid(row=0, column=2, padx=(0, 5))

        # ── Sync: slider → entry ──
        def _on_slider(value, _var=var, _slider=slider):
            if self._syncing:
                return
            self._syncing = True
            s = _slider._oc_step
            snapped = round(value / s) * s if s else round(value)
            _var.set(_fmt(snapped))
            self._syncing = False

        slider.configure(command=_on_slider)

        # ── Sync: entry → slider ──
        def _on_entry(*_, _slider=slider, _var=var):
            if self._syncing:
                return
            text = _var.get().strip()
            # Allow typing a minus sign or empty string without clamping
            if text in ("", "-", "+"):
                return
            if text == "Curve":
                return
            try:
                val = float(text) if _slider._oc_decimals else int(text)
            except ValueError:
                return
            clamped = max(_slider._oc_min, min(_slider._oc_max, val))
            self._syncing = True
            _slider.set(clamped)
            self._syncing = False

        var.trace_add("write", _on_entry)

        # ── On focus-out: clamp entry value ──
        def _on_focusout(event, _var=var, _slider=slider):
            text = _var.get().strip()
            if text == "Curve":
                return
            try:
                val = float(text) if _slider._oc_decimals else int(text)
            except ValueError:
                val = getattr(_slider, "_oc_min", float(_slider.cget("from_")))
            clamped = max(
                getattr(_slider, "_oc_min", float(_slider.cget("from_"))),
                min(getattr(_slider, "_oc_max", float(_slider.cget("to"))), val),
            )
            s = getattr(_slider, "_oc_step", 1)
            if s:
                clamped = round(clamped / s) * s
            self._syncing = True
            _var.set(_fmt(clamped))
            _slider.set(clamped)
            self._syncing = False

        entry.bind("<FocusOut>", _on_focusout)
        entry.bind("<Return>", _on_focusout)

        # Unit label right of the entry (matches the fan '%' style).
        if volt_bit is not None:
            # ClkDomains row: the label is a thin-bordered CYCLING toggle
            # (MHz → mV → MHz …). mV mode applies the row's value through
            # the slot-1 voltage plane instead of slot-0 frequency.
            toggle = tk.Label(
                row_frame,
                text="MHz",
                font=_FONT_UNIT,
                bg=_PANEL_BG,
                fg=_TEXT_FG_DIM,
                relief="solid",
                bd=1,
                padx=4,
                cursor="hand2",
            )
            toggle.grid(row=0, column=3, padx=(3, 0))

            # Per-plane row state: the SAME slider+entry widgets get
            # reconfigured onto the mV plane on toggle (±300 mV, 1 mV step,
            # one-decimal entry) and back onto the remembered MHz plane.
            # Construction values are stored per-plane on the slider.
            slider._oc_volt_bit = volt_bit
            slider._oc_volt_mode = False
            slider._oc_unit_toggle = toggle
            slider._oc_unit_var = var  # entry var (re-anchor target)
            # MHz-plane originals (restored when toggling back)
            slider._oc_freq_min = min_val
            slider._oc_freq_max = max_val
            slider._oc_freq_step = step
            slider._oc_freq_decimals = decimals
            # mV-plane bounds: ±300 mV, 0.1 mV step — the record's native
            # unit is µV, so a tenth-mV grid keeps the live slot-1 anchor
            # faithful (6250 µV anchors at 6.2/6.3, not snapped to 6 or 7).
            # The driver clamps what it refuses — the medium layer's
            # snapshot/readback guards the write.
            slider._oc_volt_min = -300
            slider._oc_volt_max = 300
            slider._oc_volt_step = 0.1
            toggle.bind("<Button-1>", lambda _e, _s=slider: self._toggle_row_unit(_s))
            HoverTooltip(
                toggle,
                "Click to switch this row between the slot-0 frequency "
                "offset (MHz) and the slot-1 per-domain V/F-curve voltage "
                "addend (mV) — set-private-freq-domain-global-offset "
                "--freq / --volt. The two planes are separate storage; "
                "switching re-anchors this row at the plane's current "
                "offset (mV mode reads it from get-private-freq-domain-info).",
            )
        elif isinstance(unit, ctk.StringVar):
            tk.Label(
                row_frame,
                textvariable=unit,
                font=_FONT_BODY,
                bg=_PANEL_BG,
                fg=_TEXT_FG_DIM,
            ).grid(row=0, column=3, padx=(3, 0))
        elif unit:
            tk.Label(
                row_frame,
                text=unit,
                font=_FONT_BODY,
                bg=_PANEL_BG,
                fg=_TEXT_FG_DIM,
            ).grid(row=0, column=3, padx=(3, 0))

        # Sub-apply button
        btn = LiteButton(row_frame, text="✓", width=34, command=apply_cmd)
        btn.grid(row=0, column=4, padx=(5, 0))

        return slider, entry, var, btn

    # ────────────────────────────────────────────
    # Actions
    # ────────────────────────────────────────────

    def _apply_pstate_lock(self):
        """Apply P-State lock: memory-clock-range first, native pin fallback.

        The mem-range range lock is always the first choice. A runtime
        failure on it means the part is older than Kepler (the NVML pstate
        mem-clock query it derives the window from is Not Supported there) —
        the worker-thread exception can't switch widgets itself, so it
        re-raises into the app's error path and the user's retry (or the
        `_pstate_pin_fallback` re-apply below) takes the native single-state
        pin. When the derived window also overlaps P-States outside the
        requested range (identical memory clocks, e.g. after a VBIOS edit)
        the lock still applies and the setter's warning is surfaced.
        """
        selection = self.pstate_selector.get_selection()
        gpu = self.app.selected_gpu_target()
        if gpu is None or selection is None:
            self.app.console.append("[GUI] No supported P-State selection available.\n")
            return

        start, end = selection
        # Swap for descending ranges (e.g. p8 to p0) if needed
        try:
            start_val = int(start.lower().replace("p", ""))
            end_val = int(end.lower().replace("p", ""))
            if start_val > end_val:
                start, end = end, start
        except ValueError:
            pass

        if getattr(self, "_pstate_pin_fallback", False):
            # Native single-P-State pin (no range form). Point-mode keeps
            # start == end; use start as the single target.
            self.app.run_native_action(
                "apply P-State lock",
                lambda native, gpu=gpu, pstate=start: (
                    native.set_pstate_native_lock(gpu, pstate)
                    or f"Successfully pinned NVAPI P-State {pstate}."
                ),
            )
            return

        backend = self._selected_oc_backend()

        def apply_mem_range(
            native, gpu=gpu, backend=backend, start=start, end=end
        ) -> str:
            try:
                warning = (
                    native.set_nvml_pstate_lock(gpu, start, end)
                    if backend == "nvml"
                    else native.set_nvapi_pstate_lock(gpu, start, end)
                )
            except Exception:
                # The window derivation needs the NVML pstate mem-clock
                # ranges — their absence (Not Supported) marks a pre-Kepler
                # part. Marshal the fallback to the main thread; this worker
                # thread must not touch widgets.
                self.app.after(0, lambda: self._on_mem_range_lock_failed(start, end))
                raise
            message = (
                f"Successfully applied {backend.upper()} P-State lock {start}-{end}."
            )
            if warning:
                # Overlapping P-States ride the same memory window by
                # construction — applied anyway, so surface the caveat.
                return f"Warning: {warning}\n{message}"
            return message

        self.app.run_native_action("apply P-State lock", apply_mem_range)

    def _unlock_pstate_lock(self):
        """Remove P-State lock for the selected OC backend.

        After a mem-range failure flipped the panel onto the native pin
        (`_pstate_pin_fallback`), unlock resets that pin; otherwise the
        NVML/VFP memory-clock lock is cleared as usual.
        """
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        if getattr(self, "_pstate_pin_fallback", False):
            self.app.run_native_action(
                "reset P-State lock",
                lambda native, gpu=gpu: (
                    native.reset_pstate_native_lock(gpu)
                    or "Successfully reset NVAPI P-State lock."
                ),
            )
            return
        backend = self._selected_oc_backend()
        self.app.run_native_action(
            "reset memory clocks",
            lambda native, gpu=gpu, backend=backend: (
                native.reset_mem_clocks(gpu, backend)
                or "Successfully reset memory clocks."
            ),
        )

    def _apply_core_only(self):
        core_mhz = self.core_var.get().strip()
        if core_mhz == "Curve":
            return

        backend = self._selected_oc_backend()
        try:
            # One decimal allowed — the 2.5 MHz grid divides both the 7.5 MHz
            # (30系+) and 12.5 MHz (10/16/20系) hardware frequency steps;
            # pynvoc floors it to kHz (NVML rounds to integer MHz).
            value = float(core_mhz)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        pstate = self._oc_pstate()

        if self._row_volt_mode(self.core_slider):
            # mV mode: slot-1 voltage addend only — there is no public
            # pstate20 path for the per-domain V/F voltage plane (and no
            # fallback chain to fall back from).
            self.app.run_native_action(
                "apply core volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Core",
                        value,
                        native.set_clk_domain_offset(
                            gpu, 0, int(value * 1000), 1, None
                        ),
                    )
                ),
            )
            return

        def action(native, gpu=gpu, backend=backend, value=value, pstate=pstate):
            # pstate20 public path first; NVAPI NotSupported (-104) on server
            # cards etc. → fallback to the ClkDomains bit0 WRITE record.
            try:
                native.set_clock_offset(gpu, backend, "core", value, pstate)
                return f"Successfully applied core offset {value:g} MHz."
            except Exception as exc:
                msg = str(exc)
                if (
                    "NotSupported" in msg
                    or "-104" in msg
                    or "not supported" in msg.lower()
                ):
                    res = native.set_clk_domain_offset(
                        gpu, 0, int(value * 1000), None, None
                    )
                    applied = res.get("applied_mHz") if isinstance(res, dict) else None
                    rb = f" (readback {applied:+g} MHz)" if applied is not None else ""
                    return (
                        f"pstate20 unsupported (-104); applied core offset "
                        f"{value:g} MHz via ClkDomains bit0{rb}."
                    )
                raise

        self.app.run_native_action("apply core offset", action)

    def _apply_mem_only(self):
        mem_mhz = self.mem_var.get().strip()
        backend = self._selected_oc_backend()
        try:
            value = int(mem_mhz)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        pstate = self._oc_pstate()

        if self._row_volt_mode(self.mem_slider):
            # mV mode: slot-1 voltage addend only (bit2 = WRITE memory M) —
            # no public path, same as the Core row.
            self.app.run_native_action(
                "apply memory volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Memory",
                        value,
                        native.set_clk_domain_offset(gpu, 2, value * 1000, 1, None),
                    )
                ),
            )
            return

        def action(native, gpu=gpu, backend=backend, value=value, pstate=pstate):
            # pstate20 → fallback ClkDomains bit2 (WRITE bit2 = 显存 M, NOT the
            # MEASURE bit2 which reads SYS — read/write are two tables).
            try:
                native.set_clock_offset(gpu, backend, "memory", value, pstate)
                return f"Successfully applied memory offset {value} MHz."
            except Exception as exc:
                msg = str(exc)
                if (
                    "NotSupported" in msg
                    or "-104" in msg
                    or "not supported" in msg.lower()
                ):
                    res = native.set_clk_domain_offset(gpu, 2, value * 1000, None, None)
                    applied = res.get("applied_mHz") if isinstance(res, dict) else None
                    rb = f" (readback {applied:+g} MHz)" if applied is not None else ""
                    return (
                        f"pstate20 unsupported (-104); applied memory offset "
                        f"{value} MHz via ClkDomains bit2{rb}."
                    )
                raise

        self.app.run_native_action("apply memory offset", action)

    def _apply_mem_with_sync(self):
        """Shift+click: apply memory offset then sync P2→P0."""
        mem_mhz = self.mem_var.get().strip()
        backend = self._selected_oc_backend()
        try:
            value = int(mem_mhz)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        self.app.run_native_action(
            "apply memory offset + sync P2→P0",
            lambda native, gpu=gpu, backend=backend, value=value: (
                native.set_clock_offset(
                    gpu, backend, "memory", value, self._oc_pstate()
                ),
                native.sync_memory_pstate_as_p0(gpu),
                f"Applied memory offset {value} MHz + synced P2→P0.",
            )[-1],
        )

    @staticmethod
    def _format_clk_domain_volt_result(
        label: str, offset_mv: float, result: Any
    ) -> str:
        """Console message for a slot-1 ClkDomains VOLTAGE write (the unit
        chip's mV mode). The pynvoc payload reuses the `applied_mHz` field
        for the applied value — slot 1 carries µV there, so divide by 1000
        for the mV readback (the payload's `slot` field confirms which plane
        answered; a non-1 slot means the write didn't land on the voltage
        plane and the readback is not a voltage)."""
        if isinstance(result, dict):
            if result.get("applied"):
                applied = result.get("applied_mHz")
                if isinstance(applied, (int, float)):
                    rb = (
                        f" (driver readback {applied / 1000:+g} mV)"
                        if result.get("slot") == 1
                        else ""
                    )
                    return f"Successfully applied {label} volt offset {offset_mv:+g} mV{rb}."
                return f"Successfully applied {label} volt offset {offset_mv:+g} mV."
            if result.get("supported") is False:
                return f"{label} clock-domain volt offset not supported by this driver."
        return f"Applied {label} volt offset {offset_mv:+g} mV."

    @staticmethod
    def _format_clk_domain_offset_result(
        label: str, offset_mhz: int, result: Any
    ) -> str:
        """Console message for a set_clk_domain_offset result (shared by
        Xbar/Sys/Msd/Host). Mirrors _format_xbar_offset_result."""
        if isinstance(result, dict):
            if result.get("applied"):
                applied = result.get("applied_mHz")
                if isinstance(applied, (int, float)):
                    return (
                        f"Successfully applied {label} offset {offset_mhz:+d} MHz "
                        f"(driver readback {applied:+g} MHz)."
                    )
                return f"Successfully applied {label} offset {offset_mhz:+d} MHz."
            if result.get("supported") is False:
                return f"{label} clock-domain offset not supported by this driver."
        return f"Applied {label} offset {offset_mhz:+d} MHz."

    def _make_domain_reset_button(self, apply_btn, label, bit, slider, var):
        """Small ↺ button right of a domain row's ✓ apply (grid column 5).

        Wired to _reset_clk_domain — the single-domain GUI twin of the
        reset-private-freq-domain-global-offset CLI command. Plane-aware on
        click: a row whose chip sits on mV resets only its slot-1 addend.
        """
        btn = LiteButton(
            apply_btn.master,
            text="↺",
            width=34,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=lambda: self._reset_clk_domain(label, bit, slider, var),
        )
        btn.grid(row=0, column=5, padx=(3, 0))
        # the chip toggle shows/hides volt-only resets (Core/Mem) with the
        # plane — fabric rows keep theirs always visible.
        slider._oc_reset_btn = btn
        HoverTooltip(
            btn,
            f"Reset the {label} domain global offset to 0 "
            f"(writes 0 to ClkDomains WRITE bit {bit}; mV mode: slot 1 only — "
            f"MHz mode: slots 0 and 1).",
        )
        return btn

    def _reset_clk_domain(self, label, bit, slider=None, var=None):
        """Reset ONE domain's global offset, plane-aware: a row whose chip
        sits on mV resets ONLY its slot-1 voltage addend (the voltage plane
        is per-domain independent — no bit3 coupled-cancel either, that
        lives on the frequency plane); on MHz it resets slots 0 AND 1 (the
        footprint of the reset-private-freq-domain-global-offset CLI
        command). Either way the row re-anchors at 0. On 30系+ an MHz Xbar
        reset also clears the coupled bit3 SYS-cancel — the section reset
        does the same, the cancel belongs to the Xbar write."""
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        volt_mode = slider is not None and self._row_volt_mode(slider)
        bits = [bit]
        if bit == 1 and self._is_ampere_plus and not volt_mode:
            bits.append(3)
        slots = (1,) if volt_mode else (0, 1)
        if slider is not None and var is not None:
            self._syncing = True
            slider.set(0)
            var.set(self._fmt_slider_value(slider, 0))
            self._syncing = False
        self.app.run_native_action(
            f"reset {label.lower()} domain {'volt ' if volt_mode else ''}offset",
            lambda native, gpu=gpu, label=label, bits=tuple(bits), slots=slots: (
                OverclockTab._reset_clk_domain_action(native, gpu, label, bits, slots)
            ),
        )

    @staticmethod
    def _reset_clk_domain_action(native, gpu, label, bits, slots=(0, 1)):
        """Worker body: zero each bit on the given slots (driver-opaque
        slots 2-7 are left alone — the CLI reset has the same footprint).
        A refused write degrades to a warning and the remaining writes
        continue, matching the CLI's warning semantics."""
        parts = []
        warnings = []
        for b in bits:
            for slot in slots:
                res = native.set_clk_domain_offset(gpu, b, 0, slot, None)
                if isinstance(res, dict) and res.get("supported") is False:
                    warnings.append(f"bit {b} slot {slot}: unsupported")
                else:
                    parts.append(f"bit {b} slot {slot} → 0")
        subject = (
            "all clock-domain global offsets"
            if label == "all"
            else f"{label} domain global offset"
        )
        msg = f"Successfully reset {subject} ({'; '.join(parts)})."
        if warnings:
            msg += " Warnings: " + "; ".join(warnings) + "."
        return msg

    @staticmethod
    def _reset_all_clk_domains_action(native, gpu):
        """Worker body: reset EVERY ClkDomains WRITE record (bits taken
        from query_private_freq_domain_info) on slots 0 and 1 — the
        all-domains form of the CLI reset. Unsupported bits warn and the
        rest continue. Reused by the VF curve tab's global reset (curve
        points and domain global offsets are separate storage)."""
        info = native.query_private_freq_domain_info(gpu)
        bits = []
        if isinstance(info, dict) and isinstance(info.get("entries"), list):
            bits = [
                e.get("bit")
                for e in info["entries"]
                if isinstance(e, dict) and isinstance(e.get("bit"), int)
            ]
        if not bits:
            return "No clock-domain WRITE records exposed — no global offsets to reset."
        return OverclockTab._reset_clk_domain_action(native, gpu, "all", bits)

    def _apply_xbar_only(self):
        """Apply the Xbar fabric-clock offset (ClkDomains WRITE bit1).

        30系+ (is_ampere_plus): bit1 couples SYS, so writing +f to bit1 also
        moves SYS — cancel that by RMW-ing bit3 (read current, write
        current − f) so any Sys offset already on bit3 is preserved. 10/
        16/20/Pascal: bit1 is pure Xbar, write it directly. GUI MHz → kHz ×1000.

        mV mode (unit chip): slot-1 voltage addend, a DIRECT write — the
        coupling/bit3-cancel is a slot-0 frequency-plane artifact; the
        voltage plane is per-domain independent (single-rail MAX
        arbitration), so no cancel is needed.
        """
        xbar = self.xbar_var.get().strip()
        try:
            value = int(xbar)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        if self._row_volt_mode(self.xbar_slider):
            self.app.run_native_action(
                "apply xbar volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Xbar",
                        value,
                        native.set_clk_domain_offset(gpu, 1, value * 1000, 1, None),
                    )
                ),
            )
            return
        coupled = self._is_ampere_plus
        self.app.run_native_action(
            "apply xbar offset",
            lambda native, gpu=gpu, value=value, coupled=coupled: (
                OverclockTab._apply_xbar_only_action(native, gpu, value, coupled)
            ),
        )

    def _apply_sys_only(self):
        """Apply the Sys fabric-clock offset (ClkDomains WRITE bit3, pure SYS).

        RMW: read bit3's current slot-0 offset, add +f, write back — so the
        write stacks on any Xbar-cancel (-f) already sitting on bit3 rather
        than overwriting it. GUI MHz → kHz ×1000.

        mV mode (unit chip): slot-1 voltage addend, a DIRECT write — the
        RMW exists to preserve the slot-0 Xbar-cancel on bit3, which lives
        on the frequency plane and is untouched by a slot-1 write.
        """
        sysv = self.sys_var.get().strip()
        try:
            value = int(sysv)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        if self._row_volt_mode(self.sys_slider):
            self.app.run_native_action(
                "apply sys volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Sys",
                        value,
                        native.set_clk_domain_offset(gpu, 3, value * 1000, 1, None),
                    )
                ),
            )
            return
        self.app.run_native_action(
            "apply sys offset",
            lambda native, gpu=gpu, value=value: OverclockTab._apply_sys_only_action(
                native, gpu, value
            ),
        )

    def _apply_msd_only(self):
        """Apply the Msd offset (ClkDomains WRITE bit5). Pascal greyed-out.

        mV mode (unit chip): slot-1 voltage addend — same record, other plane.
        """
        msd = self.msd_var.get().strip()
        try:
            value = int(msd)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        if self._row_volt_mode(self.msd_slider):
            self.app.run_native_action(
                "apply msd volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Msd",
                        value,
                        native.set_clk_domain_offset(gpu, 5, value * 1000, 1, None),
                    )
                ),
            )
            return
        self.app.run_native_action(
            "apply msd offset",
            lambda native, gpu=gpu, value=value: (
                OverclockTab._format_clk_domain_offset_result(
                    "Msd",
                    value,
                    native.set_clk_domain_offset(gpu, 5, value * 1000, None, None),
                )
            ),
        )

    def _apply_host_only(self):
        """Apply the Host offset (ClkDomains WRITE bit9).

        mV mode (unit chip): slot-1 voltage addend — same record, other plane.
        """
        host = self.host_var.get().strip()
        try:
            value = int(host)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        if self._row_volt_mode(self.host_slider):
            self.app.run_native_action(
                "apply host volt offset",
                lambda native, gpu=gpu, value=value: (
                    OverclockTab._format_clk_domain_volt_result(
                        "Host",
                        value,
                        native.set_clk_domain_offset(gpu, 9, value * 1000, 1, None),
                    )
                ),
            )
            return
        self.app.run_native_action(
            "apply host offset",
            lambda native, gpu=gpu, value=value: (
                OverclockTab._format_clk_domain_offset_result(
                    "Host",
                    value,
                    native.set_clk_domain_offset(gpu, 9, value * 1000, None, None),
                )
            ),
        )

    def _apply_plimit_only(self):
        plimit = self.plimit_var.get().strip()
        if plimit:
            gpu = self.app.selected_gpu_target()
            if self._mobile_mode:

                def on_finished(_code):
                    # The TGP SET can clamp (PPAB interplay, D-Notifier
                    # level, driver policy) — re-anchor the slider to the
                    # enforced wall right away instead of leaving the typed
                    # value (which the next unrelated mobile refresh would
                    # then overwrite with a jump).
                    self._load_mobile_limits()

                self.app.run_native_action(
                    "apply TGP watt limit",
                    lambda native, gpu=gpu, watts=int(plimit): (
                        native.set_tgp_watt(gpu, watts, self._tgp_policy_index)
                        or f"Successfully applied TGP limit {watts} W."
                    ),
                    on_finished=on_finished,
                )
                return
            backend = self._selected_power_backend()
            self.app.run_native_action(
                "apply power limit",
                lambda native, gpu=gpu, backend=backend, plimit=int(plimit): (
                    native.set_power_limit(gpu, backend, plimit)
                    or f"Successfully applied {backend.upper()} power limit."
                ),
            )

    def _apply_tlimit_only(self):
        tlimit = self.tlimit_var.get().strip()
        if tlimit:
            gpu = self.app.selected_gpu_target()
            if self._mobile_mode:
                self.app.run_native_action(
                    "apply target temperature",
                    lambda native, gpu=gpu, tlimit=float(tlimit): (
                        native.set_target_temp(gpu, tlimit, 2)
                        or f"Successfully applied target temperature {tlimit:.0f} C."
                    ),
                )
                return
            self.app.run_native_action(
                "apply thermal limit",
                lambda native, gpu=gpu, tlimit=int(tlimit): (
                    native.set_thermal_limit(gpu, tlimit)
                    or "Successfully applied thermal limit."
                ),
            )

    def _apply_vlimit_only(self):
        """Apply the Volt Limit (absolute target mV on the VoltRails path).

        The driver derives the µV offset internally and clamps the effective
        wall to min(target, vbios_wall, vrm_max_wall); on completion we
        re-load mobile limits so the slider reflects the new effective wall.
        ``target_mv`` is parsed as float to honor the 2.5 mV grid (one decimal
        on 10/20-series); it is floored to µV in the pynvoc layer.
        """
        vlimit = self.vlimit_var.get().strip()
        if not vlimit:
            return
        try:
            target_mv = float(vlimit)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        rail_bit = self._volt_rail_bit

        def on_finished(_code):
            self._load_mobile_limits()

        self.app.run_native_action(
            "apply volt-rail target",
            lambda native, gpu=gpu, rail_bit=rail_bit, target_mv=target_mv: (
                self._format_volt_rail_target_result(
                    target_mv,
                    native.set_volt_rail_target(gpu, rail_bit, target_mv, None),
                )
            ),
            on_finished=on_finished,
        )

    def _apply_vboost_only(self):
        vboost = self.vboost_var.get().strip()
        if vboost:
            gpu = self.app.selected_gpu_target()
            if getattr(self, "_is_legacy_gpu", False):
                try:
                    vboost_uv = int(vboost) * 1000
                except ValueError:
                    return
                self.app.run_native_action(
                    "apply legacy voltage delta",
                    lambda native, gpu=gpu, vboost_uv=vboost_uv: (
                        native.set_legacy_voltage_delta(gpu, vboost_uv, "P0")
                        or "Successfully applied legacy voltage delta."
                    ),
                )
            else:
                self.app.run_native_action(
                    "apply voltage boost",
                    lambda native, gpu=gpu, vboost=int(vboost): (
                        native.set_voltage_boost(gpu, vboost)
                        or "Successfully applied voltage boost."
                    ),
                )

    def _apply_oc(self):
        """Apply the CURRENT page's offsets (page 0 also handles -104 fallback
        via the per-domain handlers). NVML disables the NVAPI-only pages.

        A row whose unit chip sits on mV applies through the slot-1 voltage
        plane instead (direct write — no pstate20 path, no RMW/cancel, the
        voltage plane is per-domain independent)."""
        gpu = self.app.selected_gpu_target()
        if gpu is None:
            return
        page = self._oc_page
        if page == 0:
            actions = []
            core_mhz = self.core_var.get().strip()
            if core_mhz != "Curve":
                try:
                    cv = float(core_mhz)
                    if self._row_volt_mode(self.core_slider):
                        actions.append(
                            (
                                "apply core volt offset",
                                lambda native, gpu=gpu, cv=cv: (
                                    OverclockTab._format_clk_domain_volt_result(
                                        "Core",
                                        cv,
                                        native.set_clk_domain_offset(
                                            gpu, 0, int(cv * 1000), 1, None
                                        ),
                                    )
                                ),
                            )
                        )
                    else:
                        pstate = self._oc_pstate()
                        actions.append(
                            (
                                "apply core offset",
                                lambda native, gpu=gpu, cv=cv, pstate=pstate: (
                                    OverclockTab._apply_core_only_action(
                                        native,
                                        gpu,
                                        self._selected_oc_backend(),
                                        cv,
                                        pstate,
                                    )
                                ),
                            )
                        )
                except ValueError:
                    pass
            try:
                mv = int(self.mem_var.get().strip())
                if self._row_volt_mode(self.mem_slider):
                    actions.append(
                        (
                            "apply memory volt offset",
                            lambda native, gpu=gpu, mv=mv: (
                                OverclockTab._format_clk_domain_volt_result(
                                    "Memory",
                                    mv,
                                    native.set_clk_domain_offset(
                                        gpu, 2, mv * 1000, 1, None
                                    ),
                                )
                            ),
                        )
                    )
                else:
                    pstate = self._oc_pstate()
                    actions.append(
                        (
                            "apply memory offset",
                            lambda native, gpu=gpu, mv=mv, pstate=pstate: (
                                OverclockTab._apply_mem_only_action(
                                    native, gpu, self._selected_oc_backend(), mv, pstate
                                )
                            ),
                        )
                    )
            except ValueError:
                pass
            if not actions:
                self.app.console.append("[GUI] No valid clock offset values.\n")
                return
            self.app.run_native_action_chain(actions)
            return

        # Pages 1/2 are NVAPI-only.
        if self._selected_oc_backend() == "nvml":
            self.app.console.append("[GUI] Fabric/uncore offsets require NVAPI.\n")
            return
        coupled = self._is_ampere_plus
        if page == 1:
            actions = []
            if self.xbar_slider.cget("state") != "disabled":
                try:
                    xv = int(self.xbar_var.get().strip())
                    if xv:
                        if self._row_volt_mode(self.xbar_slider):
                            actions.append(
                                (
                                    "apply xbar volt offset",
                                    lambda native, gpu=gpu, xv=xv: (
                                        OverclockTab._format_clk_domain_volt_result(
                                            "Xbar",
                                            xv,
                                            native.set_clk_domain_offset(
                                                gpu, 1, xv * 1000, 1, None
                                            ),
                                        )
                                    ),
                                )
                            )
                        else:
                            actions.append(
                                (
                                    "apply xbar offset",
                                    lambda native, gpu=gpu, xv=xv, coupled=coupled: (
                                        OverclockTab._apply_xbar_only_action(
                                            native, gpu, xv, coupled
                                        )
                                    ),
                                )
                            )
                except ValueError:
                    pass
            if self._sys_supported and self.sys_slider.cget("state") != "disabled":
                try:
                    sv = int(self.sys_var.get().strip())
                    if sv:
                        if self._row_volt_mode(self.sys_slider):
                            actions.append(
                                (
                                    "apply sys volt offset",
                                    lambda native, gpu=gpu, sv=sv: (
                                        OverclockTab._format_clk_domain_volt_result(
                                            "Sys",
                                            sv,
                                            native.set_clk_domain_offset(
                                                gpu, 3, sv * 1000, 1, None
                                            ),
                                        )
                                    ),
                                )
                            )
                        else:
                            actions.append(
                                (
                                    "apply sys offset",
                                    lambda native, gpu=gpu, sv=sv: (
                                        OverclockTab._apply_sys_only_action(
                                            native, gpu, sv
                                        )
                                    ),
                                )
                            )
                except ValueError:
                    pass
            if not actions:
                self.app.console.append("[GUI] No valid clock offset values.\n")
                return
            self.app.run_native_action_chain(actions)
            return
        # page 2: Msd / Host
        actions = []
        if self._msd_supported and self.msd_slider.cget("state") != "disabled":
            try:
                mv = int(self.msd_var.get().strip())
                if mv:
                    if self._row_volt_mode(self.msd_slider):
                        actions.append(
                            (
                                "apply msd volt offset",
                                lambda native, gpu=gpu, mv=mv: (
                                    OverclockTab._format_clk_domain_volt_result(
                                        "Msd",
                                        mv,
                                        native.set_clk_domain_offset(
                                            gpu, 5, mv * 1000, 1, None
                                        ),
                                    )
                                ),
                            )
                        )
                    else:
                        actions.append(
                            (
                                "apply msd offset",
                                lambda native, gpu=gpu, mv=mv: (
                                    OverclockTab._format_clk_domain_offset_result(
                                        "Msd",
                                        mv,
                                        native.set_clk_domain_offset(
                                            gpu, 5, mv * 1000, None, None
                                        ),
                                    )
                                ),
                            )
                        )
            except ValueError:
                pass
        if self._host_supported and self.host_slider.cget("state") != "disabled":
            try:
                hv = int(self.host_var.get().strip())
                if hv:
                    if self._row_volt_mode(self.host_slider):
                        actions.append(
                            (
                                "apply host volt offset",
                                lambda native, gpu=gpu, hv=hv: (
                                    OverclockTab._format_clk_domain_volt_result(
                                        "Host",
                                        hv,
                                        native.set_clk_domain_offset(
                                            gpu, 9, hv * 1000, 1, None
                                        ),
                                    )
                                ),
                            )
                        )
                    else:
                        actions.append(
                            (
                                "apply host offset",
                                lambda native, gpu=gpu, hv=hv: (
                                    OverclockTab._format_clk_domain_offset_result(
                                        "Host",
                                        hv,
                                        native.set_clk_domain_offset(
                                            gpu, 9, hv * 1000, None, None
                                        ),
                                    )
                                ),
                            )
                        )
            except ValueError:
                pass
        if not actions:
            self.app.console.append("[GUI] No valid clock offset values.\n")
            return
        self.app.run_native_action_chain(actions)

    def _reset_oc(self):
        """Reset the CURRENT page's offsets to 0."""
        gpu = self.app.selected_gpu_target()
        page = self._oc_page
        self._syncing = True
        if page == 0:
            self.core_slider.set(0)
            self.core_var.set(self._fmt_slider_value(self.core_slider, 0))
            self.mem_slider.set(0)
            self.mem_var.set(self._fmt_slider_value(self.mem_slider, 0))
        elif page == 1:
            self.xbar_slider.set(0)
            self.xbar_var.set(self._fmt_slider_value(self.xbar_slider, 0))
            if self._sys_supported:
                self.sys_slider.set(0)
                self.sys_var.set(self._fmt_slider_value(self.sys_slider, 0))
        else:
            if self._msd_supported:
                self.msd_slider.set(0)
                self.msd_var.set(self._fmt_slider_value(self.msd_slider, 0))
            if self._host_supported:
                self.host_slider.set(0)
                self.host_var.set(self._fmt_slider_value(self.host_slider, 0))
        self._syncing = False

        if page == 0:
            backend = self._selected_oc_backend()
            resets = [
                (
                    "reset core offset",
                    lambda native, gpu=gpu, backend=backend: (
                        native.set_clock_offset(
                            gpu, backend, "core", 0, self._oc_pstate()
                        )
                        or "Successfully reset core offset."
                    ),
                ),
                (
                    "reset memory offset",
                    lambda native, gpu=gpu, backend=backend: (
                        native.set_clock_offset(
                            gpu, backend, "memory", 0, self._oc_pstate()
                        )
                        or "Successfully reset memory offset."
                    ),
                ),
            ]
            self.app.run_native_action_chain(resets)
            return

        # NVAPI-only reset for the page's bits. Xbar 30+ couples bit3, so the
        # reset clears bit1 AND bit3 (write 0 to both — no -f to cancel).
        resets = []
        if page == 1:
            resets.append(
                (
                    "reset xbar offset",
                    lambda native, gpu=gpu: (
                        OverclockTab._format_clk_domain_offset_result(
                            "Xbar",
                            0,
                            native.set_clk_domain_offset(gpu, 1, 0, None, None),
                        )
                    ),
                )
            )
            if self._is_ampere_plus:
                resets.append(
                    (
                        "reset sys-cancel",
                        lambda native, gpu=gpu: (
                            OverclockTab._format_clk_domain_offset_result(
                                "Sys-cancel",
                                0,
                                native.set_clk_domain_offset(gpu, 3, 0, None, None),
                            )
                        ),
                    )
                )
            if self._sys_supported:
                resets.append(
                    (
                        "reset sys offset",
                        lambda native, gpu=gpu: (
                            OverclockTab._format_clk_domain_offset_result(
                                "Sys",
                                0,
                                native.set_clk_domain_offset(gpu, 3, 0, None, None),
                            )
                        ),
                    )
                )
        else:
            if self._msd_supported:
                resets.append(
                    (
                        "reset msd offset",
                        lambda native, gpu=gpu: (
                            OverclockTab._format_clk_domain_offset_result(
                                "Msd",
                                0,
                                native.set_clk_domain_offset(gpu, 5, 0, None, None),
                            )
                        ),
                    )
                )
            if self._host_supported:
                resets.append(
                    (
                        "reset host offset",
                        lambda native, gpu=gpu: (
                            OverclockTab._format_clk_domain_offset_result(
                                "Host",
                                0,
                                native.set_clk_domain_offset(gpu, 9, 0, None, None),
                            )
                        ),
                    )
                )
        if resets:
            self.app.run_native_action_chain(resets)

    # Static action bodies (so the chain lambdas above stay picklable-free
    # and the logic is testable without a live widget).
    @staticmethod
    def _apply_core_only_action(native, gpu, backend, value, pstate):
        try:
            native.set_clock_offset(gpu, backend, "core", value, pstate)
            return f"Successfully applied core offset {value:g} MHz."
        except Exception as exc:
            msg = str(exc)
            if "NotSupported" in msg or "-104" in msg or "not supported" in msg.lower():
                res = native.set_clk_domain_offset(
                    gpu, 0, int(value * 1000), None, None
                )
                applied = res.get("applied_mHz") if isinstance(res, dict) else None
                rb = f" (readback {applied:+g} MHz)" if applied is not None else ""
                return (
                    f"pstate20 unsupported (-104); applied core offset "
                    f"{value:g} MHz via ClkDomains bit0{rb}."
                )
            raise

    @staticmethod
    def _apply_mem_only_action(native, gpu, backend, value, pstate):
        try:
            native.set_clock_offset(gpu, backend, "memory", value, pstate)
            return f"Successfully applied memory offset {value} MHz."
        except Exception as exc:
            msg = str(exc)
            if "NotSupported" in msg or "-104" in msg or "not supported" in msg.lower():
                res = native.set_clk_domain_offset(gpu, 2, value * 1000, None, None)
                applied = res.get("applied_mHz") if isinstance(res, dict) else None
                rb = f" (readback {applied:+g} MHz)" if applied is not None else ""
                return (
                    f"pstate20 unsupported (-104); applied memory offset "
                    f"{value} MHz via ClkDomains bit2{rb}."
                )
            raise

    @staticmethod
    def _bit3_current_khz(native, gpu) -> int:
        """Read the ClkDomains bit3 (pure SYS) slot-0 offset — the RMW
        baseline shared by the Sys write and the Xbar coupled-cancel."""
        info = native.query_private_freq_domain_info(gpu)
        if isinstance(info, dict):
            entries = info.get("entries") or []
            if isinstance(entries, list):
                for e in entries:
                    if isinstance(e, dict) and e.get("bit") == 3:
                        vals = e.get("values_kHz") or []
                        if isinstance(vals, list) and vals:
                            try:
                                return int(vals[0] or 0)
                            except (TypeError, ValueError):
                                return 0
                        break
        return 0

    @staticmethod
    def _apply_xbar_only_action(native, gpu, value, coupled):
        res = native.set_clk_domain_offset(gpu, 1, value * 1000, None, None)
        msgs = [OverclockTab._format_clk_domain_offset_result("Xbar", value, res)]
        if coupled:
            # RMW the cancel onto bit3 (current − f): a Sys offset already
            # sitting on bit3 survives — only the coupling drift is removed.
            cur_khz = OverclockTab._bit3_current_khz(native, gpu)
            new_khz = cur_khz - value * 1000
            res3 = native.set_clk_domain_offset(gpu, 3, new_khz, None, None)
            msgs.append(
                OverclockTab._format_clk_domain_offset_result(
                    "Sys-cancel", -value, res3
                )
                + f" (bit3 {int(round(cur_khz / 1000)):+d} → {int(round(new_khz / 1000)):+d} MHz)"
            )
        return " ".join(msgs)

    @staticmethod
    def _apply_sys_only_action(native, gpu, value):
        cur_khz = OverclockTab._bit3_current_khz(native, gpu)
        new_khz = cur_khz + value * 1000
        res = native.set_clk_domain_offset(gpu, 3, new_khz, None, None)
        return (
            OverclockTab._format_clk_domain_offset_result("Sys", value, res)
            + f" (bit3 {int(round(cur_khz / 1000)):+d} → {int(round(new_khz / 1000)):+d} MHz)"
        )

    def _apply_limits(self):
        gpu = self.app.selected_gpu_target()
        actions = []

        if self._mobile_mode:
            if self.plimit_slider.cget("state") != "disabled":
                plimit = self.plimit_var.get().strip()
                if plimit:
                    actions.append(
                        (
                            "apply TGP watt limit",
                            lambda native, gpu=gpu, watts=int(plimit): (
                                native.set_tgp_watt(gpu, watts, self._tgp_policy_index)
                                or f"Successfully applied TGP limit {watts} W."
                            ),
                        )
                    )
            if self.tlimit_slider.cget("state") != "disabled":
                tlimit = self.tlimit_var.get().strip()
                if tlimit:
                    actions.append(
                        (
                            "apply target temperature",
                            lambda native, gpu=gpu, tlimit=float(tlimit): (
                                native.set_target_temp(gpu, tlimit, 2)
                                or f"Successfully applied target temperature {tlimit:.0f} C."
                            ),
                        )
                    )
            if self.vlimit_slider.cget("state") != "disabled":
                vlimit = self.vlimit_var.get().strip()
                if vlimit:
                    try:
                        target_mv = float(vlimit)
                    except ValueError:
                        target_mv = None
                    if target_mv is not None:
                        rail_bit = self._volt_rail_bit
                        actions.append(
                            (
                                "apply volt-rail target",
                                lambda native, gpu=gpu, rail_bit=rail_bit, target_mv=target_mv: (
                                    self._format_volt_rail_target_result(
                                        target_mv,
                                        native.set_volt_rail_target(
                                            gpu, rail_bit, target_mv, None
                                        ),
                                    )
                                ),
                            )
                        )
            if not actions:
                self.app.console.append("[GUI] No limit values specified.\n")
                return
            self.app.run_native_action_chain(actions)
            return

        if self.plimit_slider.cget("state") != "disabled":
            plimit = self.plimit_var.get().strip()
            if plimit:
                backend = self._selected_power_backend()
                actions.append(
                    (
                        "apply power limit",
                        lambda native, gpu=gpu, backend=backend, plimit=int(plimit): (
                            native.set_power_limit(gpu, backend, plimit)
                            or f"Successfully applied {backend.upper()} power limit."
                        ),
                    )
                )

        if self.tlimit_slider.cget("state") != "disabled":
            tlimit = self.tlimit_var.get().strip()
            if tlimit:
                actions.append(
                    (
                        "apply thermal limit",
                        lambda native, gpu=gpu, tlimit=int(tlimit): (
                            native.set_thermal_limit(gpu, tlimit)
                            or "Successfully applied thermal limit."
                        ),
                    )
                )

        if self.vboost_slider.cget("state") != "disabled":
            vboost = self.vboost_var.get().strip()
            if vboost:
                if getattr(self, "_is_legacy_gpu", False):
                    try:
                        vboost_uv = int(vboost) * 1000
                    except ValueError:
                        pass
                    else:
                        actions.append(
                            (
                                "apply legacy voltage delta",
                                lambda native, gpu=gpu, vboost_uv=vboost_uv: (
                                    native.set_legacy_voltage_delta(
                                        gpu, vboost_uv, "P0"
                                    )
                                    or "Successfully applied legacy voltage delta."
                                ),
                            )
                        )
                else:
                    actions.append(
                        (
                            "apply voltage boost",
                            lambda native, gpu=gpu, vboost=int(vboost): (
                                native.set_voltage_boost(gpu, vboost)
                                or "Successfully applied voltage boost."
                            ),
                        )
                    )

        if not actions:
            self.app.console.append("[GUI] No limit values specified.\n")
            return
        self.app.run_native_action_chain(actions)

    def _reset_all(self):
        gpu = self.app.selected_gpu_target()
        # Reset sliders to their defaults from GPU info (var text through the
        # row formatter for the +0/+0.0 sign convention)
        self._syncing = True
        self.core_slider.set(0)
        self.core_var.set(self._fmt_slider_value(self.core_slider, 0))
        self.mem_slider.set(0)
        self.mem_var.set(self._fmt_slider_value(self.mem_slider, 0))
        self.plimit_var.set(str(self._power_default))
        self.plimit_slider.set(self._power_default)
        self.tlimit_var.set(str(self._thermal_default))
        self.tlimit_slider.set(self._thermal_default)
        self.vboost_var.set("0")
        self.vboost_slider.set(0)
        self._syncing = False

        if self._mobile_mode:
            policy_index = self._tgp_policy_index

            def on_finished(_code):
                # Re-loads TGP/D-Notifier/temp-policies AND volt-rail bounds,
                # which reconfigures the Volt Limit slider back to the live
                # effective wall. pynvoc exposes no per-rail volt-rail reset,
                # so there is no separate reset command — the slider simply
                # re-anchors to the hardware's current voltage wall.
                self._load_mobile_limits()

            self.app.run_native_action(
                "reset TGP to default",
                lambda native, gpu=gpu, policy_index=policy_index: (
                    native.reset_tgp_watt(gpu, policy_index)
                    or "Successfully reset TGP to default."
                ),
                on_finished=on_finished,
            )
            return

        self.app.run_native_action(
            "reset all settings",
            lambda native, gpu=gpu: (
                native.reset_all(gpu, None) or "Successfully reset all settings."
            ),
        )
