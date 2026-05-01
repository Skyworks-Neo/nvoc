"""
Overclock Tab - OC offset (slider + entry) and power/thermal limits.
Ranges are queried from GPU hardware via the CLI 'info' command.
"""

import customtkinter as ctk
from typing import TYPE_CHECKING, Tuple, Dict, Any, Optional, Union, List, Set
from src.tabs.fan_control import FanControlTab
from src.widgets.lightweight_controls import (
    CanvasSlider,
    LiteButton,
    LiteEntry,
    SegmentRangeSelector,
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

    def __init__(self, parent: ctk.CTkFrame, app: "App"):
        self.app = app
        self.frame = parent
        self._syncing = False  # guard against feedback loops
        self._is_vfp_mode = False
        self._vfp_uniform_offset_mhz = None  # type: Optional[int]
        self._limit_supported_state = True
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

        content_row = ctk.CTkFrame(scroll, fg_color="transparent")
        content_row.pack(fill="x", pady=(0, 10))
        content_row.grid_columnconfigure(0, weight=1)
        content_row.grid_columnconfigure(1, weight=1)

        # ═══════════════════════════════════════════
        # Clock Offset (OC)
        # ═══════════════════════════════════════════
        oc_frame = ctk.CTkFrame(content_row)
        oc_frame.grid(row=0, column=0, sticky="nsew", padx=(0, 5))
        oc_header = ctk.CTkFrame(oc_frame, fg_color="transparent")
        oc_header.pack(fill="x", padx=10, pady=(10, 5))
        ctk.CTkLabel(oc_header, text="⚡ Clock Offsets", font=("", 14, "bold")).pack(
            side="left"
        )
        self.oc_api_var = ctk.StringVar(value="NVAPI")
        self.oc_api_selector = ctk.CTkOptionMenu(
            oc_header,
            values=["NVAPI", "NVML"],
            variable=self.oc_api_var,
            width=94,
            height=28,
        )
        self.oc_api_selector.pack(side="right")
        oc_api_help = ctk.CTkLabel(oc_header, text="→", text_color="gray70")
        oc_api_help.pack(side="right", padx=(0, 6))
        oc_api_tip = (
            "Clock offset API selector (core/memory + PState lock).\n"
            "- NVAPI: --core-offset / --mem-offset values are in kHz.\n"
            "- NVML: --core-offset / --mem-offset values are in MHz."
        )
        HoverTooltip(self.oc_api_selector, oc_api_tip)
        HoverTooltip(oc_api_help, oc_api_tip)

        # PState lock selector
        ps_row = ctk.CTkFrame(oc_frame, fg_color="transparent")
        ps_row.pack(fill="x", padx=10, pady=(0, 5))
        ps_row.grid_columnconfigure(1, weight=1)
        ctk.CTkLabel(ps_row, text="PState Lock:", width=90, anchor="w").grid(
            row=0, column=0, sticky="nw", pady=(5, 0)
        )
        self.pstate_selector = SegmentRangeSelector(ps_row, values=[])
        self.pstate_selector.grid(row=0, column=1, sticky="ew", padx=(5, 8))
        ps_btns = ctk.CTkFrame(ps_row, fg_color="transparent")
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
                "Core Offset(MHz):",
                d["core_clock_min"],
                d["core_clock_max"],
                0,
                step=5,
                apply_cmd=self._apply_core_only,
            )
        )

        # Memory Clock slider + entry
        self.mem_slider, self.mem_entry, self.mem_var, btn_apply_mem = (
            self._make_slider_row(
                oc_frame,
                "Mem Offset(MHz):",
                d["mem_clock_min"],
                d["mem_clock_max"],
                0,
                step=10,
                apply_cmd=self._apply_mem_only,
            )
        )

        # Buttons
        btn_oc = ctk.CTkFrame(oc_frame, fg_color="transparent")
        btn_oc.pack(fill="x", padx=10, pady=(5, 10))
        LiteButton(
            btn_oc, text="✅ Apply Offset", width=170, command=self._apply_oc
        ).pack(side="left", padx=5)
        LiteButton(
            btn_oc,
            text="🔄 Reset OC to 0",
            width=140,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_oc,
        ).pack(side="left", padx=5)

        # ═══════════════════════════════════════════
        # Power & Thermal Limits
        # ═══════════════════════════════════════════
        self.limit_frame = ctk.CTkFrame(content_row)
        self.limit_frame.grid(row=0, column=1, sticky="nsew", padx=(5, 0))
        limit_header = ctk.CTkFrame(self.limit_frame, fg_color="transparent")
        limit_header.pack(fill="x", padx=10, pady=(10, 5))
        self.limit_title_label = ctk.CTkLabel(
            limit_header, text="⚡ Power & Thermal Limits", font=("", 14, "bold")
        )
        self.limit_title_label.pack(side="left")
        self.power_api_var = ctk.StringVar(value="NVAPI")
        self.power_api_selector = ctk.CTkOptionMenu(
            limit_header,
            values=["NVAPI", "NVML"],
            variable=self.power_api_var,
            width=94,
            height=28,
            command=self._on_power_api_changed,
        )
        self.power_api_selector.pack(side="right")
        power_api_help = ctk.CTkLabel(limit_header, text="→", text_color="gray70")
        power_api_help.pack(side="right", padx=(0, 6))
        power_api_tip = (
            "Power limit API selector (power slider only).\n"
            "- NVAPI: --power-limit is percentage (%).\n"
            "- NVML: --power-limit is watts (W)."
        )
        HoverTooltip(self.power_api_selector, power_api_tip)
        HoverTooltip(power_api_help, power_api_tip)
        self.limit_status_label = ctk.CTkLabel(
            self.limit_frame,
            text="Power / thermal controls are unsupported on mobile/laptop GPUs.",
            text_color="gray60",
        )

        # Power Limit slider + entry
        self.plimit_label_var = ctk.StringVar(value="Pwr Limit(%):")
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
        )

        # Thermal Limit slider + entry
        (
            self.tlimit_slider,
            self.tlimit_entry,
            self.tlimit_var,
            self.btn_apply_tlimit,
        ) = self._make_slider_row(
            self.limit_frame,
            "Thrm Limit(℃):",
            d["thermal_limit_min"],
            d["thermal_limit_max"],
            d["thermal_limit_default"],
            apply_cmd=self._apply_tlimit_only,
        )

        # Voltage Boost / Offset slider + entry
        self.vboost_label_var = ctk.StringVar(value="VoltBoost(%):")
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
        )

        btn_limits = ctk.CTkFrame(self.limit_frame, fg_color="transparent")
        btn_limits.pack(fill="x", padx=10, pady=(5, 10))
        self.btn_apply_limits = LiteButton(
            btn_limits, text="✅ Apply Limits", width=140, command=self._apply_limits
        )
        self.btn_apply_limits.pack(side="left", padx=5)
        self.btn_reset_all = LiteButton(
            btn_limits,
            text="🔄 Reset All Settings",
            width=200,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_all,
        )
        self.btn_reset_all.pack(side="left", padx=5)

        fan_frame = ctk.CTkFrame(scroll)
        fan_frame.pack(fill="x", pady=(0, 10))
        self.fan_section = FanControlTab(fan_frame, self.app, embedded=True)
        self._limit_enabled_frame_color = self.limit_frame.cget("fg_color")
        self._limit_dim_frame_color = ("gray86", "gray20")
        self._limit_enabled_title_color = self.limit_title_label.cget("text_color")
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
        """Enable/disable the whole power/thermal section with a dimmed visual state."""
        if supported == self._limit_supported_state:
            return
        self._limit_supported_state = supported

        state = "normal" if supported else "disabled"
        for widget in [
            self.power_api_selector,
            self.plimit_slider,
            self.plimit_entry,
            self.btn_apply_plimit,
            self.tlimit_slider,
            self.tlimit_entry,
            self.btn_apply_tlimit,
            self.vboost_slider,
            self.vboost_entry,
            self.btn_apply_vboost,
            self.btn_apply_limits,
            self.btn_reset_all,
        ]:
            self._safe_set_state(widget, state)

        self.limit_frame.configure(
            fg_color=self._limit_enabled_frame_color
            if supported
            else self._limit_dim_frame_color
        )
        self.limit_title_label.configure(
            text_color=self._limit_enabled_title_color
            if supported
            else self._limit_dim_title_color
        )
        if supported:
            self.limit_status_label.pack_forget()
        elif not self.limit_status_label.winfo_manager():
            self.limit_status_label.pack(anchor="w", padx=10, pady=(0, 6))

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

        power_backend = self._selected_power_backend()
        if power_backend == "nvml":
            self.plimit_label_var.set("Pwr Limit(W):")
            if "power_limit_nvml_min_w" in limits and "power_limit_nvml_max_w" in limits:
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
            self.plimit_label_var.set("Pwr Limit(%):")
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
            self.vboost_label_var.set("Overvolt(mV):")
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
                self._set_slider_value(self.vboost_slider, self.vboost_var, int(current))

        # Restore saved states to entry and button widgets
        self._safe_set_state(self.plimit_entry, plimit_entry_state)
        self._safe_set_state(self.btn_apply_plimit, plimit_btn_state)
        self._safe_set_state(self.tlimit_entry, tlimit_entry_state)
        self._safe_set_state(self.btn_apply_tlimit, tlimit_btn_state)
        self._safe_set_state(self.vboost_entry, vboost_entry_state)
        self._safe_set_state(self.btn_apply_vboost, vboost_btn_state)

        if "supported_pstates" in limits:
            self.set_supported_pstates(limits.get("supported_pstates"))

    def _get_vfp_core_display_text(self) -> str:
        """Display text for Core Offset while VF curve mode is active."""
        if self._vfp_uniform_offset_mhz is not None:
            return str(int(self._vfp_uniform_offset_mhz))
        return "Curve"

    def set_vfp_state(
        self, has_vfp_offset: bool, uniform_core_offset_mhz: Optional[int] = None
    ):
        """Update core clock display if VFP has offsets."""
        if self._is_resize_active:
            self._pending_vfp_state = (has_vfp_offset, uniform_core_offset_mhz)
            return

        previous_vfp_mode = self._is_vfp_mode
        self._is_vfp_mode = has_vfp_offset
        self._vfp_uniform_offset_mhz = (
            int(uniform_core_offset_mhz)
            if (has_vfp_offset and uniform_core_offset_mhz is not None)
            else None
        )
        if has_vfp_offset:
            self._syncing = True
            self.core_var.set(self._get_vfp_core_display_text())
            self._syncing = False
        else:
            # If VF mode just ended, reset display back to the slider's scalar value.
            if previous_vfp_mode or self.core_var.get() == "Curve":
                self._syncing = True
                self.core_var.set(str(int(self.core_slider.get())))
                self._syncing = False

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

        self._set_limit_panel_supported(not is_mobile)
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
            self.vboost_label_var.set("Overvolt(mV):")
        else:
            self._is_legacy_gpu = False
            self.vboost_label_var.set("VoltBoost(%):")

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
        selected = self.power_api_var.get().strip().upper()
        return "nvml" if selected == "NVML" else "nvapi"

    def _on_power_api_changed(self, _selected: str):
        """Re-sync power slider unit/range and refresh current values via get."""
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
        if var is self.core_var and self._is_vfp_mode:
            var.set(self._get_vfp_core_display_text())
        else:
            var.set(str(default))
        self._syncing = False

    def _set_slider_value(self, slider: Any, var: ctk.StringVar, value: int):
        """Update a slider's current value without changing its range."""
        min_val = getattr(slider, "_oc_min", int(slider.cget("from_")))
        max_val = getattr(slider, "_oc_max", int(slider.cget("to")))
        clamped = max(min_val, min(max_val, value))
        self._syncing = True
        slider.set(clamped)
        if var is self.core_var and self._is_vfp_mode:
            var.set(self._get_vfp_core_display_text())
        else:
            var.set(str(clamped))
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
    ) -> Tuple[Any, ctk.CTkEntry, ctk.StringVar, ctk.CTkButton]:
        """Create a row with label, slider, numeric entry and apply button."""
        row_frame = ctk.CTkFrame(parent, fg_color="transparent")
        row_frame.pack(fill="x", padx=10, pady=3)
        row_frame.grid_columnconfigure(1, weight=1)

        if isinstance(label, ctk.StringVar):
            ctk.CTkLabel(row_frame, textvariable=label, width=90, anchor="w").grid(
                row=0, column=0, sticky="w"
            )
        else:
            ctk.CTkLabel(row_frame, text=label, width=90, anchor="w").grid(
                row=0, column=0, sticky="w"
            )

        # Slider
        n_steps = (max_val - min_val) // step if step else (max_val - min_val)
        slider = CanvasSlider(
            row_frame,
            from_=min_val,
            to=max_val,
            number_of_steps=n_steps,
        )
        slider.set(default)
        slider.grid(row=0, column=1, sticky="ew", padx=(5, 10))

        # Store range info on the slider for dynamic access in callbacks
        slider._oc_min = min_val
        slider._oc_max = max_val
        slider._oc_step = step

        # Entry (fixed width, right-aligned value)
        var = ctk.StringVar(value=str(default))
        entry = LiteEntry(row_frame, textvariable=var, width=7, justify="right")
        entry.grid(row=0, column=2, padx=(0, 5))

        # ── Sync: slider → entry ──
        def _on_slider(value, _var=var, _slider=slider):
            if self._syncing:
                return
            self._syncing = True
            s = _slider._oc_step
            snapped = round(value / s) * s if s else round(value)
            _var.set(str(int(snapped)))
            self._syncing = False

        slider.configure(command=_on_slider)

        # ── Sync: entry → slider ──
        def _on_entry(*_, _slider=slider, _var=var):
            if self._syncing:
                return
            text = _var.get().strip()
            if _var is self.core_var and self._is_vfp_mode:
                display = self._get_vfp_core_display_text()
                if text != display:
                    self._syncing = True
                    _var.set(display)
                    self._syncing = False
                return
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
            if _var is self.core_var and self._is_vfp_mode:
                self._syncing = True
                _var.set(self._get_vfp_core_display_text())
                self._syncing = False
                return
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
            _var.set(str(int(clamped)))
            _slider.set(clamped)
            self._syncing = False

        entry.bind("<FocusOut>", _on_focusout)
        entry.bind("<Return>", _on_focusout)

        # Sub-apply button
        btn = LiteButton(row_frame, text="✓", width=34, command=apply_cmd)
        btn.grid(row=0, column=3, padx=(5, 0))

        return slider, entry, var, btn

    # ────────────────────────────────────────────
    # Actions
    # ────────────────────────────────────────────

    def _apply_pstate_lock(self):
        """Apply P-State lock range with the selected OC backend."""
        selection = self.pstate_selector.get_selection()
        gpu_args = self.app.get_gpu_args()
        if not gpu_args or selection is None:
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
        self.app.run_cli_display(
            gpu_args + ["set", backend, "--pstate-lock", start, end]
        )

    def _unlock_pstate_lock(self):
        """Remove memory lock settings for the selected OC backend."""
        gpu_args = self.app.get_gpu_args()
        if gpu_args:
            backend = self._selected_oc_backend()
            flag = "--reset-mem-clocks"
            self.app.run_cli_display(gpu_args + ["set", backend, flag])

    def _apply_core_only(self):
        if self._is_vfp_mode:
            return
        core_mhz = self.core_var.get().strip()
        if core_mhz == "Curve":
            return

        gpu_args = self.app.get_gpu_args()
        backend = self._selected_oc_backend()
        value = self._format_oc_value_for_backend(core_mhz, backend)
        if value is None:
            return
        args = gpu_args + ["set", backend, "--core-offset", value]
        self.app.run_cli_display(args)

    def _apply_mem_only(self):
        mem_mhz = self.mem_var.get().strip()
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_oc_backend()
        value = self._format_oc_value_for_backend(mem_mhz, backend)
        if value is None:
            return
        args = gpu_args + ["set", backend, "--mem-offset", value]
        self.app.run_cli_display(args)

    def _apply_plimit_only(self):
        plimit = self.plimit_var.get().strip()
        if plimit:
            backend = self._selected_power_backend()
            self.app.run_cli_display(
                self.app.get_gpu_args() + ["set", backend, "--power-limit", plimit],
                on_finished=lambda _rc: self.app._query_gpu_get(),
            )

    def _apply_tlimit_only(self):
        tlimit = self.tlimit_var.get().strip()
        if tlimit:
            self.app.run_cli_display(
                self.app.get_gpu_args() + ["set", "nvapi", "--thermal-limit", tlimit]
            )

    def _apply_vboost_only(self):
        vboost = self.vboost_var.get().strip()
        if vboost:
            if getattr(self, "_is_legacy_gpu", False):
                try:
                    vboost_uv = str(int(vboost) * 1000)
                except ValueError:
                    return
                self.app.run_cli_display(
                    self.app.get_gpu_args()
                    + ["set", "nvapi", "--voltage-delta", vboost_uv]
                )
            else:
                self.app.run_cli_display(
                    self.app.get_gpu_args()
                    + ["set", "nvapi", "--voltage-boost", vboost]
                )

    def _apply_oc(self):
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_oc_backend()

        core_mhz = self.core_var.get().strip()
        mem_mhz = self.mem_var.get().strip()

        applied = False
        # Apply core clock offset (convert MHz → kHz)
        if (not self._is_vfp_mode) and core_mhz != "Curve":
            value = self._format_oc_value_for_backend(core_mhz, backend)
            if value is not None:
                args = gpu_args + ["set", backend, "--core-offset", value]
                self.app.run_cli_display(args)
                applied = True

        # Apply memory clock offset using the selected backend units.
        value = self._format_oc_value_for_backend(mem_mhz, backend)
        if value is not None:
            args = gpu_args + ["set", backend, "--mem-offset", value]
            self.app.run_cli_display(args)
            applied = True

        if not applied:
            self.app.console.append("[GUI] No valid clock offset values.\n")

    def _reset_oc(self):
        gpu_args = self.app.get_gpu_args()
        # Reset both sliders to 0
        self._syncing = True
        self.core_var.set("0")
        self.core_slider.set(0)
        self.mem_var.set("0")
        self.mem_slider.set(0)
        self._syncing = False

        backend = self._selected_oc_backend()
        self.app.run_cli_display(gpu_args + ["set", backend, "--core-offset", "0"])
        self.app.run_cli_display(gpu_args + ["set", backend, "--mem-offset", "0"])

    def _apply_limits(self):
        gpu_args = self.app.get_gpu_args()
        nvapi_args = gpu_args + ["set", "nvapi"]
        has_nvapi = False
        power_args = None

        if self.plimit_slider.cget("state") != "disabled":
            plimit = self.plimit_var.get().strip()
            if plimit:
                if self._selected_power_backend() == "nvml":
                    power_args = gpu_args + ["set", "nvml", "--power-limit", plimit]
                else:
                    nvapi_args += ["--power-limit", plimit]
                    has_nvapi = True

        if self.tlimit_slider.cget("state") != "disabled":
            tlimit = self.tlimit_var.get().strip()
            if tlimit:
                nvapi_args += ["--thermal-limit", tlimit]
                has_nvapi = True

        if self.vboost_slider.cget("state") != "disabled":
            vboost = self.vboost_var.get().strip()
            if vboost:
                if getattr(self, "_is_legacy_gpu", False):
                    try:
                        nvapi_args += ["--voltage-delta", str(int(vboost) * 1000)]
                        has_nvapi = True
                    except ValueError:
                        pass
                else:
                    nvapi_args += ["--voltage-boost", vboost]
                    has_nvapi = True

        if power_args is None and (not has_nvapi):
            self.app.console.append("[GUI] No limit values specified.\n")
            return

        if power_args is not None:
            self.app.run_cli_display(power_args)
        if has_nvapi:
            self.app.run_cli_display(nvapi_args)

    def _reset_all(self):
        gpu_args = self.app.get_gpu_args()
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

        self.app.run_cli_display(gpu_args + ["reset"])
