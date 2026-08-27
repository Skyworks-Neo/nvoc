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
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")


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
        self._xbar_supported = False  # Xbar row: Turing (GTX 16系) and newer
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

        # Top panels (Clock Offsets + Power & Thermal Limits) can be hosted by
        # the dashboard (integration mode) or live in this tab's scroll frame.
        content_host = content_parent if content_parent is not None else scroll
        content_row = tk.Frame(content_host, bg=_PANEL_BG)
        content_row.pack(fill="x", pady=(0, 10))
        # uniform: strictly equal card widths regardless of requested sizes
        content_row.grid_columnconfigure(0, weight=1, uniform="oc_cards")
        content_row.grid_columnconfigure(1, weight=1, uniform="oc_cards")

        # ═══════════════════════════════════════════
        # Clock Offset (OC)
        # ═══════════════════════════════════════════
        # Thin rounded dark-blue frame marks the section boundary
        oc_frame = ctk.CTkFrame(
            content_row, border_width=1, border_color="#1f4e79", corner_radius=10
        )
        # "new": natural height, top aligned — with "nsew" the two cards
        # stretch to the taller one's height and the shorter card shows a
        # large empty gap under its sliders (e.g. mobile mode layout).
        oc_frame.grid(row=0, column=0, sticky="new", padx=(0, 5))
        oc_header = tk.Frame(oc_frame, bg=_PANEL_BG)
        oc_header.pack(fill="x", padx=10, pady=(10, 9))
        tk.Label(
            oc_header,
            text="⚡ Clock Offsets",
            font=_FONT_HEADER,
            bg=_PANEL_BG,
            fg=_TEXT_FG,
        ).pack(side="left")
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
            "NVAPI-only rows (Xbar, Volt Limit) grey out under NVML."
        )
        HoverTooltip(self.oc_api_selector, oc_api_tip)

        # PState lock selector
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

        # Core Clock slider + entry. Step 2.5 MHz with one decimal — the
        # LCM-friendly grid that divides both the 7.5 MHz frequency step on
        # 30-series and newer and the 12.5 MHz step on 10/16/20-series.
        # entry_width=8 like Volt Limit: "+122.5" needs the extra char.
        self.core_slider, self.core_entry, self.core_var, _ = self._make_slider_row(
            oc_frame,
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
        )

        # Memory Clock slider + entry
        self.mem_slider, self.mem_entry, self.mem_var, btn_apply_mem = (
            self._make_slider_row(
                oc_frame,
                "Mem:",
                d["mem_clock_min"],
                d["mem_clock_max"],
                0,
                step=5,
                apply_cmd=self._apply_mem_only,
                signed=True,
                unit="MHz",
            )
        )
        btn_apply_mem.configure(shift_command=self._apply_mem_with_sync)
        HoverTooltip(
            btn_apply_mem,
            "Shift+Click: apply global offset then sync P2 memory VFP to P0 frequency",
        )

        # Xbar (crossbar fabric) clock offset — NVAPI-only private ClockClient
        # domain offset (pynvoc set_clk_domain_offset, the GUI-MHz face of the
        # CLI's `set-clk-domain-offset xbar <kHz>`). Same row format as
        # Core/Mem; arch-gated to Turing (GTX 16-series) and newer, and greyed
        # under the NVML backend selection. Range ±500 MHz with a 5 MHz
        # wheel/drag step — the article only documented a ±60 MHz XBAR bound
        # on GB202, but 50-series fabric clocks run far higher, so leave the
        # wide range and let the driver reject unsafe writes (the medium
        # layer snapshot/SET/readback/restore guards the write itself).
        (
            self.xbar_slider,
            self.xbar_entry,
            self.xbar_var,
            self.btn_apply_xbar,
        ) = self._make_slider_row(
            oc_frame,
            "Xbar:",
            -500,
            500,
            0,
            step=5,
            apply_cmd=self._apply_xbar_only,
            signed=True,
            unit="MHz",
        )

        # Buttons — apply/reset each take half the row
        btn_oc = tk.Frame(oc_frame, bg=_PANEL_BG)
        btn_oc.pack(fill="x", padx=(26, 10), pady=(5, 10))
        btn_oc.columnconfigure(0, weight=1, uniform="oc_btns")
        btn_oc.columnconfigure(1, weight=1, uniform="oc_btns")
        # Anchor for repacking the arch-gated Xbar row (kept just below Mem).
        self._oc_buttons_row = btn_oc
        LiteButton(
            btn_oc, text="✅ Apply Section", width=10, command=self._apply_oc
        ).grid(row=0, column=0, sticky="ew", padx=(0, 5))
        LiteButton(
            btn_oc,
            text="🔄 Reset Section",
            width=10,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_oc,
        ).grid(row=0, column=1, sticky="ew", padx=(5, 0))

        # Xbar is arch-gated (Turing/16-series and newer): hidden until
        # check_capabilities() sees a matching architecture.
        self.xbar_slider.master.pack_forget()

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
                    native.set_dynamic_boost(gpu, True)
                    or "Dynamic Boost (PPAB) enabled."
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
            # Position the slider at the enforced power limit (NVML, from
            # the mobile-limits query) — the actually-active wall. The TGP
            # policy exposes no current-value read; default_watt is only
            # the fallback.
            current_w = data.get("power_limit_w")
            position = self._power_default
            if current_w is not None:
                current_w = int(round(float(current_w)))
                if min_w <= current_w <= max_w:
                    position = current_w
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

    @staticmethod
    def _xbar_supported_arch(
        arch_id: str, codename: str = "", gpu_name: str = ""
    ) -> bool:
        """True for Turing (the GTX 16-series) and every newer architecture.

        Three signals, in priority order:
        1. Chip codes from the codename or the arch string (tu106, ga102,
           ad107, gb202 — optionally suffixed ":rev", "-B", " (process)").
           The codename matters: on Ada the pynvoc ArchInfo enum has no AD
           variant and reports ``gpu_architecture = 'Unknown:400:7:161'``,
           while ``codename = 'AD107-B'`` carries the real chip code.
        2. Friendly architecture names (Turing, Ampere, Ada, Blackwell) from
           the CLI human output.
        3. Marketing-name fallback: ``RTX 4060`` / ``GTX 1660`` — the model
           number >= 1600 means 16-series or newer (GTX 1080/10-series and
           below stay hidden).
        Pascal (gp), Volta (gv) and older return False — the XBAR
        ClockClient domain postdates them.
        """
        # 1) chip codes (codename first — it is the reliable one on Ada).
        for raw in (codename, arch_id):
            head = (
                raw.lower().split("(", 1)[0].split(":", 1)[0].split("-", 1)[0].strip()
            )
            if head.startswith(("tu", "ga", "ad", "gb")):
                return True
        # 2) friendly names.
        if any(
            name in arch_id.lower() for name in ("turing", "ampere", "ada", "blackwell")
        ):
            return True
        # 3) marketing name: RTX/GTX + model number, 1600 = 16-series floor.
        match = __import__("re").search(r"\b(?:rtx|gtx)\s*(\d{3,4})", gpu_name.lower())
        if match:
            return int(match.group(1)) >= 1600
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
        applied payload with the driver's readback ``applied_kHz``, or
        ``{"supported": False}``) — never ``None`` — so the message must be
        formatted here rather than via ``setter() or "msg"``.
        """
        if isinstance(result, dict):
            if result.get("applied"):
                applied = result.get("applied_kHz")
                if isinstance(applied, (int, float)):
                    return (
                        f"Successfully applied Xbar offset {offset_mhz:+d} MHz "
                        f"(driver readback {applied / 1000.0:+g} MHz)."
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
                native.set_dynamic_boost(gpu, active)
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

        if "core_clock_min" in limits and "core_clock_max" in limits:
            current = limits.get("core_clock_current", 0)
            self._reconfigure_slider(
                self.core_slider,
                self.core_var,
                limits["core_clock_min"],
                limits["core_clock_max"],
                current,
                step=2.5,
            )
        elif "core_clock_current" in limits:
            self._set_slider_value(
                self.core_slider, self.core_var, limits["core_clock_current"]
            )

        if "mem_clock_min" in limits and "mem_clock_max" in limits:
            current = limits.get("mem_clock_current", 0)
            self._reconfigure_slider(
                self.mem_slider,
                self.mem_var,
                limits["mem_clock_min"],
                limits["mem_clock_max"],
                current,
                step=5,
            )
        elif "mem_clock_current" in limits:
            self._set_slider_value(
                self.mem_slider, self.mem_var, limits["mem_clock_current"]
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
        self.fan_section.set_supported(not is_mobile)

        # Xbar (private ClockClient domain offset) exists from Turing — the
        # GTX 16-series — onward. Older archs hide the row entirely; when it
        # is shown, the OC backend dropdown gate also applies (NVML has no
        # Xbar path).
        xbar_ok = self._xbar_supported_from_info(info)
        if xbar_ok != self._xbar_supported:
            self._xbar_supported = xbar_ok
            if xbar_ok:
                self.xbar_slider.master.pack(
                    fill="x", padx=(26, 10), pady=3, before=self._oc_buttons_row
                )
            else:
                self.xbar_slider.master.pack_forget()
            self._refresh_nvapi_only_rows()
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

        self._supported_pstates = normalized
        self.pstate_selector.set_values(normalized)

        state = "normal" if normalized else "disabled"
        self._safe_set_state(self.pstate_selector, state)
        self._safe_set_state(self.btn_apply_pstate, state)
        self._safe_set_state(self.btn_unlock_pstate, state)

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

    def _refresh_nvapi_only_rows(self):
        """Sync the enabled state of the NVAPI-only rows (Xbar, Volt Limit).

        NVML exposes neither the private ClockClient domain offsets (Xbar)
        nor the VoltRails family (Volt Limit), so both rows grey out while
        the Clock Offsets backend dropdown sits on NVML. Volt Limit is only
        touched in mobile mode — the desktop panel hides the row and the
        'off' mode has already disabled the whole panel (this re-applies the
        gate after _set_limit_panel_mode re-enables the mobile widgets).
        """
        state = "disabled" if self._selected_oc_backend() == "nvml" else "normal"
        if self._xbar_supported:
            for widget in (self.xbar_slider, self.xbar_entry, self.btn_apply_xbar):
                self._safe_set_state(widget, state)
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
        # int() (truncation) == floor for the non-negative span/step here; use
        # it instead of // so a fractional step like 2.5 mV works.
        n_steps = int((max_val - min_val) / step) if step else int(max_val - min_val)

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
        visually consistent.
        """
        decimals = getattr(slider, "_oc_decimals", 0)
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

    # ────────────────────────────────────────────
    # Helper: create a  Label | Slider | Entry  row
    # ────────────────────────────────────────────
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
    ) -> Tuple[Any, ctk.CTkEntry, ctk.StringVar, ctk.CTkButton]:
        """Create a row with label, slider, numeric entry and apply button.

        signed=True renders the entry value with an explicit +/- sign
        (offset rows — avoids reading e.g. 150 as an absolute frequency).

        decimals>0 switches the entry to a fixed-point render (e.g.
        ``decimals=1`` → ``1082.5``) and parses typed input as float — used by
        the Volt Limit row whose 2.5 mV step needs one decimal on 10/20-series.
        """

        def _fmt(v: float) -> str:
            # `:+` also signs zero (+0 / +0.0) — offset rows show 0 as an
            # explicitly-applied zero offset (see _fmt_slider_value).
            if decimals:
                return f"{float(v):{'+.' if signed else '.'}{decimals}f}"
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

        # Unit label right of the entry (matches the fan '%' style)
        if isinstance(unit, ctk.StringVar):
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
        """Apply P-State lock range with the selected OC backend."""
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

        backend = self._selected_oc_backend()
        self.app.run_native_action(
            "apply P-State lock",
            lambda native, gpu=gpu, backend=backend, start=start, end=end: (
                (
                    native.set_nvml_pstate_lock(gpu, start, end)
                    if backend == "nvml"
                    else native.set_nvapi_pstate_lock(gpu, start, end)
                )
                or f"Successfully applied {backend.upper()} P-State lock {start}-{end}."
            ),
        )

    def _unlock_pstate_lock(self):
        """Remove memory lock settings for the selected OC backend."""
        gpu = self.app.selected_gpu_target()
        if gpu is None:
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
        self.app.run_native_action(
            "apply core offset",
            lambda native, gpu=gpu, backend=backend, value=value: (
                native.set_clock_offset(gpu, backend, "core", value, self._oc_pstate())
                or f"Successfully applied core offset {value:g} MHz."
            ),
        )

    def _apply_mem_only(self):
        mem_mhz = self.mem_var.get().strip()
        backend = self._selected_oc_backend()
        try:
            value = int(mem_mhz)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        self.app.run_native_action(
            "apply memory offset",
            lambda native, gpu=gpu, backend=backend, value=value: (
                native.set_clock_offset(
                    gpu, backend, "memory", value, self._oc_pstate()
                )
                or f"Successfully applied memory offset {value} MHz."
            ),
        )

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

    def _apply_xbar_only(self):
        """Apply the Xbar fabric-clock offset (NVAPI-only ClockClient path).

        The GUI speaks signed MHz like Core/Mem; pynvoc's
        ``set_clk_domain_offset`` takes kHz (the CLI spelling is
        ``set-clk-domain-offset xbar <kHz>``; xbar = domain bit 1). The
        medium layer snapshots the control block, patches, SETs, readbacks,
        and restores on mismatch.
        """
        xbar = self.xbar_var.get().strip()
        try:
            value = int(xbar)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        self.app.run_native_action(
            "apply xbar offset",
            lambda native, gpu=gpu, value=value: self._format_xbar_offset_result(
                value,
                native.set_clk_domain_offset(gpu, 1, value * 1000, None, None),
            ),
        )

    def _apply_plimit_only(self):
        plimit = self.plimit_var.get().strip()
        if plimit:
            gpu = self.app.selected_gpu_target()
            if self._mobile_mode:
                self.app.run_native_action(
                    "apply TGP watt limit",
                    lambda native, gpu=gpu, watts=int(plimit): (
                        native.set_tgp_watt(gpu, watts, self._tgp_policy_index)
                        or f"Successfully applied TGP limit {watts} W."
                    ),
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
        gpu = self.app.selected_gpu_target()
        backend = self._selected_oc_backend()

        core_mhz = self.core_var.get().strip()
        mem_mhz = self.mem_var.get().strip()

        actions = []
        if core_mhz != "Curve":
            try:
                # One decimal (2.5 MHz grid) — see _apply_core_only.
                core_value = float(core_mhz)
                actions.append(
                    (
                        "apply core offset",
                        lambda native, gpu=gpu, backend=backend, core_value=core_value: (
                            native.set_clock_offset(
                                gpu, backend, "core", core_value, self._oc_pstate()
                            )
                            or f"Successfully applied core offset {core_value:g} MHz."
                        ),
                    )
                )
            except ValueError:
                pass

        try:
            mem_value = int(mem_mhz)
            actions.append(
                (
                    "apply memory offset",
                    lambda native, gpu=gpu, backend=backend, mem_value=mem_value: (
                        native.set_clock_offset(
                            gpu, backend, "memory", mem_value, self._oc_pstate()
                        )
                        or f"Successfully applied memory offset {mem_value} MHz."
                    ),
                )
            )
        except ValueError:
            pass

        # Xbar is NVAPI-only: the row is disabled under the NVML backend
        # selection, and the state check skips it there (and on archs where
        # the row is hidden).
        if self._xbar_supported and self.xbar_slider.cget("state") != "disabled":
            try:
                xbar_value = int(self.xbar_var.get().strip())
                actions.append(
                    (
                        "apply xbar offset",
                        lambda native, gpu=gpu, xbar_value=xbar_value: (
                            self._format_xbar_offset_result(
                                xbar_value,
                                native.set_clk_domain_offset(
                                    gpu, 1, xbar_value * 1000, None, None
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
        gpu = self.app.selected_gpu_target()
        # Reset sliders to 0 — var text goes through the row formatter so the
        # sign convention (+0 / +0.0) matches what typing/focusout renders.
        self._syncing = True
        self.core_slider.set(0)
        self.core_var.set(self._fmt_slider_value(self.core_slider, 0))
        self.mem_slider.set(0)
        self.mem_var.set(self._fmt_slider_value(self.mem_slider, 0))
        if self._xbar_supported:
            self.xbar_slider.set(0)
            self.xbar_var.set(self._fmt_slider_value(self.xbar_slider, 0))
        self._syncing = False

        backend = self._selected_oc_backend()
        resets = [
            (
                "reset core offset",
                lambda native, gpu=gpu, backend=backend: (
                    native.set_clock_offset(gpu, backend, "core", 0, self._oc_pstate())
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
        if self._xbar_supported:
            resets.append(
                (
                    "reset xbar offset",
                    lambda native, gpu=gpu: self._format_xbar_offset_result(
                        0, native.set_clk_domain_offset(gpu, 1, 0, None, None)
                    ),
                )
            )
        self.app.run_native_action_chain(resets)

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
