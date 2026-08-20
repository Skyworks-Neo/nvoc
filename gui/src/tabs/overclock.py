"""
Overclock Tab - OC offset (slider + entry) and power/thermal limits.
Ranges are queried from GPU hardware via the CLI 'info' command.
"""

from typing import TYPE_CHECKING, Tuple, Dict, Any, Optional, Union

import tkinter as tk

import customtkinter as ctk

# ── De-CTk'd panel palette ──────────────────────────────────────────────
# Inner layout uses plain tk widgets: one bg for every container (matches
# CTk dark-theme frame/scroll bg), plus text colors. Keeping these in one
# place makes the light/dark question a one-spot change.
_PANEL_BG = "#2b2b2b"      # CTk dark frame/scroll background
_TEXT_FG = "#e5e5e5"       # default label text
_TEXT_FG_DIM = "#b3b3b3"   # 'gray70' hints
_TEXT_FG_FAINT = "#999999"  # 'gray60' status text
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")

from src.panes.fan_control import FanControlPane
from src.widgets.lightweight_controls import (
    CanvasSlider,
    LiteButton,
    LiteCheckbutton,
    LiteEntry,
    SegmentRangeSelector,
    SegmentToggleSelector,
    install_mousewheel_support,
)
from src.widgets.hover_tooltip import HoverTooltip

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
            height=28,
        )
        self.oc_api_selector.pack(side="right")
        oc_api_tip = (
            "Clock offset API selector (core/memory + PState lock).\n"
            "- NVAPI: --core-offset / --mem-offset values are in kHz.\n"
            "- NVML: --core-offset / --mem-offset values are in MHz."
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

        # Core Clock slider + entry
        self.core_slider, self.core_entry, self.core_var, btn_apply_core = (
            self._make_slider_row(
                oc_frame,
                "Core:",
                d["core_clock_min"],
                d["core_clock_max"],
                0,
                step=5,
                apply_cmd=self._apply_core_only,
                signed=True,
                unit="MHz",
            )
        )

        # Memory Clock slider + entry
        self.mem_slider, self.mem_entry, self.mem_var, btn_apply_mem = (
            self._make_slider_row(
                oc_frame,
                "Mem:",
                d["mem_clock_min"],
                d["mem_clock_max"],
                0,
                step=10,
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

        # Buttons — apply/reset each take half the row
        btn_oc = tk.Frame(oc_frame, bg=_PANEL_BG)
        btn_oc.pack(fill="x", padx=(26, 10), pady=(5, 10))
        btn_oc.columnconfigure(0, weight=1, uniform="oc_btns")
        btn_oc.columnconfigure(1, weight=1, uniform="oc_btns")
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
        self.fan_section = FanControlPane(
            fan_host, self.app.backend, embedded=True
        )
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
        else:
            widgets.extend([
                self.power_api_selector,
                self.vboost_slider,
                self.vboost_entry,
                self.btn_apply_vboost,
            ])
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
        self.dnotifier_row.pack(fill="x", padx=(26, 10), pady=(0, 3), before=plimit_row)
        # Voltage Boost row is hidden on mobile (unvalidated NVAPI % path).
        self.vboost_slider.master.pack_forget()
        self._load_mobile_limits()

    def _exit_mobile_mode(self):
        """Restore the desktop limit panel layout. Idempotent."""
        if not self._mobile_mode:
            return
        self._mobile_mode = False
        self.ppab_checkbox.pack_forget()
        self.dnotifier_row.pack_forget()
        self.power_api_selector.pack(side="right")
        self.plimit_unit_var.set("%")
        # Repack the vboost row before the buttons row (original order).
        btn_limits_row = self.btn_apply_limits.master
        self.vboost_slider.master.pack(fill="x", padx=10, pady=3, before=btn_limits_row)

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
        dnotifier_ok = isinstance(data.get("dnotifier"), dict) and data.get(
            "dnotifier"
        )
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
        dnotifier = data.get("dnotifier") if isinstance(data.get("dnotifier"), dict) else None
        policies = data.get("temp_policies") or []

        if tgp and tgp.get("max_watt") is not None and tgp.get("min_watt") is not None:
            self._tgp_policy_index = int(tgp.get("policy_index", 2))
            self._power_default = int(
                round(float(tgp.get("default_watt") or tgp.get("min_watt")))
            )
            self._reconfigure_slider(
                self.plimit_slider,
                self.plimit_var,
                int(round(float(tgp["min_watt"]))),
                int(round(float(tgp["max_watt"]))),
                self._power_default,
                step=1,
            )

        if dnotifier and dnotifier.get("levels"):
            levels = dnotifier["levels"]
            values = [str(item.get("level", "")).upper() for item in levels]
            subtitles = [
                f"{float(item['watts']):.0f}W" if item.get("watts") is not None else None
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
            current = int(round(float(target.get("celsius", target.get("default", 83)))))
            self._thermal_default = current
            self._reconfigure_slider(
                self.tlimit_slider,
                self.tlimit_var,
                int(round(float(target["min"]))),
                int(round(float(target["max"]))),
                current,
                step=1,
            )

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
                or f"D-Notifier set to D{d_level}."
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
                step=5,
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
                step=10,
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

        # Mobile/Laptop GPU test
        gpu_name = str(info.get("gpu_name", "")).lower()
        arch_id = str(info.get("gpu_architecture", "")).lower().strip()
        arch_head = arch_id.split("(", 1)[0].strip().split(":", 1)[0].strip()
        # Check for mobile/laptop indicators: explicit keywords, RTX XXM (mobile suffix), RTX for laptops with M suffix
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
        # Maxwell / 900 series and older detection
        # Simple heuristic: architectural series usually exposed in info or if missing VFP
        # fallback arch check from name
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

    @staticmethod
    def _format_oc_value_for_backend(mhz_text: str, backend: str) -> str | None:
        """Convert entry MHz text into CLI units for the selected OC backend."""
        try:
            mhz = int(mhz_text)
        except ValueError:
            return None
        return str(mhz if backend == "nvml" else mhz * 1000)

    def _reconfigure_slider(
        self,
        slider: Any,
        var: ctk.StringVar,
        min_val: int,
        max_val: int,
        default: int,
        step: int = 1,
    ):
        """Reconfigure a slider's range, steps, and reset to default value."""
        n_steps = (max_val - min_val) // step if step else (max_val - min_val)

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
    def _fmt_slider_value(slider: Any, value: int) -> str:
        """Format an entry value, signed for offset rows (see _make_slider_row)."""
        if getattr(slider, "_oc_signed", False):
            return f"{int(value):+d}"
        return str(int(value))

    def _set_slider_value(self, slider: Any, var: ctk.StringVar, value: int):
        """Update a slider's current value without changing its range."""
        min_val = getattr(slider, "_oc_min", int(slider.cget("from_")))
        max_val = getattr(slider, "_oc_max", int(slider.cget("to")))
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
        min_val: int,
        max_val: int,
        default: int,
        step: int = 1,
        apply_cmd=None,
        signed: bool = False,
        unit: Union[str, ctk.StringVar] = "",
    ) -> Tuple[Any, ctk.CTkEntry, ctk.StringVar, ctk.CTkButton]:
        """Create a row with label, slider, numeric entry and apply button.

        signed=True renders the entry value with an explicit +/- sign
        (offset rows — avoids reading e.g. 150 as an absolute frequency).
        """

        def _fmt(v: int) -> str:
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
        n_steps = (max_val - min_val) // step if step else (max_val - min_val)
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

        # Entry (fixed width, right-aligned value)
        var = ctk.StringVar(value=_fmt(default))
        entry = LiteEntry(row_frame, textvariable=var, width=7, justify="right")
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
                val = int(text)
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
                val = int(text)
            except ValueError:
                val = getattr(_slider, "_oc_min", int(_slider.cget("from_")))
            clamped = max(
                getattr(_slider, "_oc_min", int(_slider.cget("from_"))),
                min(getattr(_slider, "_oc_max", int(_slider.cget("to"))), val),
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
            value = int(core_mhz)
        except ValueError:
            return
        gpu = self.app.selected_gpu_target()
        self.app.run_native_action(
            "apply core offset",
            lambda native, gpu=gpu, backend=backend, value=value: (
                native.set_clock_offset(gpu, backend, "core", value, self._oc_pstate())
                or f"Successfully applied core offset {value} MHz."
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
                core_value = int(core_mhz)
                actions.append((
                    "apply core offset",
                    lambda native, gpu=gpu, backend=backend, core_value=core_value: (
                        native.set_clock_offset(
                            gpu, backend, "core", core_value, self._oc_pstate()
                        )
                        or f"Successfully applied core offset {core_value} MHz."
                    ),
                ))
            except ValueError:
                pass

        try:
            mem_value = int(mem_mhz)
            actions.append((
                "apply memory offset",
                lambda native, gpu=gpu, backend=backend, mem_value=mem_value: (
                    native.set_clock_offset(
                        gpu, backend, "memory", mem_value, self._oc_pstate()
                    )
                    or f"Successfully applied memory offset {mem_value} MHz."
                ),
            ))
        except ValueError:
            pass

        if not actions:
            self.app.console.append("[GUI] No valid clock offset values.\n")
            return
        self.app.run_native_action_chain(actions)

    def _reset_oc(self):
        gpu = self.app.selected_gpu_target()
        # Reset both sliders to 0
        self._syncing = True
        self.core_var.set("0")
        self.core_slider.set(0)
        self.mem_var.set("0")
        self.mem_slider.set(0)
        self._syncing = False

        backend = self._selected_oc_backend()
        self.app.run_native_action_chain([
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
        ])

    def _apply_limits(self):
        gpu = self.app.selected_gpu_target()
        actions = []

        if self._mobile_mode:
            if self.plimit_slider.cget("state") != "disabled":
                plimit = self.plimit_var.get().strip()
                if plimit:
                    actions.append((
                        "apply TGP watt limit",
                        lambda native, gpu=gpu, watts=int(plimit): (
                            native.set_tgp_watt(gpu, watts, self._tgp_policy_index)
                            or f"Successfully applied TGP limit {watts} W."
                        ),
                    ))
            if self.tlimit_slider.cget("state") != "disabled":
                tlimit = self.tlimit_var.get().strip()
                if tlimit:
                    actions.append((
                        "apply target temperature",
                        lambda native, gpu=gpu, tlimit=float(tlimit): (
                            native.set_target_temp(gpu, tlimit, 2)
                            or f"Successfully applied target temperature {tlimit:.0f} C."
                        ),
                    ))
            if not actions:
                self.app.console.append("[GUI] No limit values specified.\n")
                return
            self.app.run_native_action_chain(actions)
            return

        if self.plimit_slider.cget("state") != "disabled":
            plimit = self.plimit_var.get().strip()
            if plimit:
                backend = self._selected_power_backend()
                actions.append((
                    "apply power limit",
                    lambda native, gpu=gpu, backend=backend, plimit=int(plimit): (
                        native.set_power_limit(gpu, backend, plimit)
                        or f"Successfully applied {backend.upper()} power limit."
                    ),
                ))

        if self.tlimit_slider.cget("state") != "disabled":
            tlimit = self.tlimit_var.get().strip()
            if tlimit:
                actions.append((
                    "apply thermal limit",
                    lambda native, gpu=gpu, tlimit=int(tlimit): (
                        native.set_thermal_limit(gpu, tlimit)
                        or "Successfully applied thermal limit."
                    ),
                ))

        if self.vboost_slider.cget("state") != "disabled":
            vboost = self.vboost_var.get().strip()
            if vboost:
                if getattr(self, "_is_legacy_gpu", False):
                    try:
                        vboost_uv = int(vboost) * 1000
                    except ValueError:
                        pass
                    else:
                        actions.append((
                            "apply legacy voltage delta",
                            lambda native, gpu=gpu, vboost_uv=vboost_uv: (
                                native.set_legacy_voltage_delta(gpu, vboost_uv, "P0")
                                or "Successfully applied legacy voltage delta."
                            ),
                        ))
                else:
                    actions.append((
                        "apply voltage boost",
                        lambda native, gpu=gpu, vboost=int(vboost): (
                            native.set_voltage_boost(gpu, vboost)
                            or "Successfully applied voltage boost."
                        ),
                    ))

        if not actions:
            self.app.console.append("[GUI] No limit values specified.\n")
            return
        self.app.run_native_action_chain(actions)

    def _reset_all(self):
        gpu = self.app.selected_gpu_target()
        # Reset sliders to their defaults from GPU info
        self._syncing = True
        self.core_var.set("0")
        self.core_slider.set(0)
        self.mem_var.set("0")
        self.mem_slider.set(0)
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
