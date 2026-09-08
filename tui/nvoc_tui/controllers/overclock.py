from __future__ import annotations

import re
import threading

from textual.widgets import Input, Select

from .base import PaneController


class OverclockController(PaneController):
    def __init__(self, app) -> None:
        super().__init__(app)
        self._mobile_limits_gpu: str | None = None
        self._mobile_load_lock = threading.Lock()
        self._fan_surface_lock = threading.Lock()
        # GPUs whose NVML fan-info reports zero coolers (fanless server cards:
        # P100/A100 …) — the Fan pane greys out for these, the same verdict
        # surface the GUI drives through set_supported_state.
        self._fanless_gpus: set[str] = set()
        # GPUs whose NVML cooler count came back ≥ 1 — observed fans win over
        # the is_server classification (ServerLovelace L40/L4 carry fans).
        self._fanned_gpus: set[str] = set()
        self._tgp_policy_index = 2
        self._tgp_range = (5, 140)
        self._target_temp_range = (75, 87)
        # VoltRails state for the mobile Volt Limit row (GUI dashboard parity).
        self._volt_rail_bit = 0  # rail bit (0 on single-rail mobile GPUs)
        self._volt_limit_range = (300.0, 1200.0)  # mV, walls refine the ceiling
        self._volt_limit_supported = False
        # Set when a mem-range P-State lock failed at runtime (pre-Kepler
        # part: the NVML pstate mem-clock query is Not Supported there) —
        # apply/reset then use the native single-P-State pin instead.
        self._pstate_pin_fallback = False

    def available_pstates(self) -> list[str]:
        pstates = self.app.cache.settings.get("supported_pstates", [])
        if not isinstance(pstates, list):
            return []
        normalized: list[str] = []
        for pstate in pstates:
            value = self.normalize_pstate(str(pstate))
            if value and value not in normalized:
                normalized.append(value)
        return normalized

    def normalize_pstate(self, value: str) -> str:
        stripped = value.strip().upper()
        if stripped.isdigit():
            return f"P{int(stripped)}"
        if len(stripped) > 1 and stripped.startswith("P") and stripped[1:].isdigit():
            return f"P{int(stripped[1:])}"
        return stripped

    def pstate_error(self, pstate: str) -> str:
        available = self.available_pstates()
        if available:
            return (
                f"Unknown pstate {pstate}. Available pstates: {', '.join(available)}."
            )
        return (
            f"Unknown pstate {pstate}. Available pstates are not loaded; run Get first."
        )

    def validate_pstates(self, *pstates: str) -> str | None:
        available = self.available_pstates()
        if not available:
            return None
        available_set = set(available)
        for pstate in pstates:
            if pstate and pstate not in available_set:
                return self.pstate_error(pstate)
        return None

    def enrich_pstate_exception(self, exc: Exception) -> Exception:
        message = str(exc)
        if "unknown pstate" not in message.lower():
            return exc
        available = self.available_pstates()
        if available:
            return RuntimeError(
                f"{message}. Available pstates: {', '.join(available)}."
            )
        return exc

    def activate_shortcut(self, target_id: str) -> bool:
        try:
            self.app.query_one(f"#{target_id}").focus()
            return True
        except Exception:
            return False

    def prime_inputs(self) -> None:
        fields = {
            "#core-offset": str(
                self.app.cache.settings.get(
                    "core_clock_current", self.app.cache.info.get("core_clock_min", 0)
                )
            ),
            "#mem-offset": str(
                self.app.cache.settings.get(
                    "mem_clock_current", self.app.cache.info.get("mem_clock_min", 0)
                )
            ),
            "#power-limit": str(
                self.app.cache.settings.get(
                    "power_limit_current",
                    self.app.cache.info.get("power_limit_default", 100),
                )
            ),
            "#thermal-limit": str(self.app.cache.info.get("thermal_limit_default", 83)),
            "#voltage-boost": str(
                self.app.cache.settings.get("voltage_boost_current", 0)
            ),
        }
        for selector, value in fields.items():
            try:
                self.app.query_one(selector, Input).value = value
            except Exception:
                pass
        # Fabric/uncore rows (Xbar/Sys/Msd/Host) are NVAPI-only; grey them out
        # under NVML or on pre-Pascal archs. Per-row presence (Sys bit3/Msd
        # bit5/Host bit9) is refined by the controllable mask polled below.
        # Default-disabled until the mask lands.
        self._prime_fabric_inputs()
        self._poll_clk_domain_mask()
        # Mobile Power pane: mobile GPUs only. Same verdict the loader uses
        # (is_mobile — Rust detect_gpu_type flag primary, name heuristic
        # fallback); desktop/compute cards (P100/TCC, Fermi GT730, …) have
        # no PPAB/D-Notifier/TGP/target-temp surface, so hide the whole
        # subpane — load_mobile_limits already refuses to load values there,
        # leaving a dead panel of default inputs + no-op Apply buttons.
        try:
            self.app.query_one("#mobile-power-pane").display = self.is_mobile()
        except Exception:
            pass
        # Fan pane: apply the fanless verdict known for THIS gpu immediately —
        # the observed cooler-count sets when re-entering the tab, plus the
        # synchronous gpu_type.rs is_server classification (Tesla P100/A100 …
        # are passive; observed fans exempt L40/L4). The async surface load
        # refines it.
        try:
            gpu_now = self.app.selected_gpu_target()
            fanless = gpu_now is not None and (
                gpu_now in self._fanless_gpus
                or (self.is_server() and gpu_now not in self._fanned_gpus)
            )
            self._set_fan_pane_disabled(fanless)
        except Exception:
            pass
        self.load_mobile_limits()
        self._load_fan_surface()
        # Fresh get/settings load (also fires on GPU switch) — drop any
        # mem-range-failure pin fallback from the previous GPU; the pin
        # re-arms per GPU when its own mem-range attempt fails.
        self._pstate_pin_fallback = False

    def apply_oc(
        self,
        native,
        gpu: str,
        backend: str,
        core_offset: int,
        mem_offset: int,
        xbar_offset: int | None = None,
        sys_offset: int | None = None,
        msd_offset: int | None = None,
        host_offset: int | None = None,
    ) -> str:
        messages: list[str] = []
        coupled = self.is_ampere_plus()

        def apply_pstate20(domain: str, value: int, bit: int, label: str) -> str:
            # pstate20 public path first; -104 NotSupported → ClkDomains bit.
            try:
                native.set_clock_offset(gpu, backend, domain, value, "P0")
                return f"Successfully applied {label} offset {value} MHz."
            except Exception as exc:
                msg = str(exc)
                if (
                    "NotSupported" in msg
                    or "-104" in msg
                    or "not supported" in msg.lower()
                ):
                    res = native.set_clk_domain_offset(
                        gpu, bit, value * 1000, None, None
                    )
                    return self._format_clk_domain_offset_result(label, value, res) + (
                        f" (pstate20 -104 fallback bit{bit})"
                    )
                raise

        messages.append(apply_pstate20("core", core_offset, 0, "core"))
        messages.append(apply_pstate20("memory", mem_offset, 2, "memory"))

        # Xbar (bit1). 30系+ couples SYS → RMW the cancel onto bit3
        # (current − f) so a Sys offset already there survives; only the
        # coupling drift is removed. 10/16/20/Pascal: bit1 pure, direct write.
        if xbar_offset is not None:
            messages.append(
                self._format_xbar_offset_result(
                    xbar_offset,
                    native.set_clk_domain_offset(
                        gpu, 1, xbar_offset * 1000, None, None
                    ),
                )
            )
            if coupled:
                cur_khz = self._clk_domain_current_offset(native, gpu, 3)
                new_khz = cur_khz - xbar_offset * 1000
                messages.append(
                    self._format_clk_domain_offset_result(
                        "Sys-cancel",
                        -xbar_offset,
                        native.set_clk_domain_offset(gpu, 3, new_khz, None, None),
                    )
                    + f" (bit3 {int(round(cur_khz / 1000)):+d} → {int(round(new_khz / 1000)):+d} MHz)"
                )
        # Sys (bit3) RMW: read current offset, +f, write back. Skipped at 0
        # (no-op — avoids overwriting an Xbar-cancel sitting on bit3).
        if sys_offset:
            cur_khz = self._clk_domain_current_offset(native, gpu, 3)
            new_khz = cur_khz + sys_offset * 1000
            messages.append(
                self._format_clk_domain_offset_result(
                    "Sys",
                    sys_offset,
                    native.set_clk_domain_offset(gpu, 3, new_khz, None, None),
                )
                + f" (bit3 {int(round(cur_khz / 1000)):+d} → {int(round(new_khz / 1000)):+d} MHz)"
            )
        if msd_offset:
            messages.append(
                self._format_clk_domain_offset_result(
                    "Msd",
                    msd_offset,
                    native.set_clk_domain_offset(gpu, 5, msd_offset * 1000, None, None),
                )
            )
        if host_offset:
            messages.append(
                self._format_clk_domain_offset_result(
                    "Host",
                    host_offset,
                    native.set_clk_domain_offset(
                        gpu, 9, host_offset * 1000, None, None
                    ),
                )
            )
        return "\n".join(messages)

    def xbar_supported(self) -> bool:
        """Xbar support verdict (port of the GUI dashboard gate).

        Primary signal: the query_info payload's ``xbar_supported`` flag,
        computed in Rust by core's gpu_type.rs detect_gpu_type — the single
        source of truth for generation detection (the ArchInfo enum has no
        AD variant, so Ada reports ``Unknown:400:7:161`` and only the
        codename/flag carry the real chip). Fallback: the architecture
        heuristic below for payloads without the flag.
        """
        flag = self.app.cache.info.get("xbar_supported")
        if isinstance(flag, bool):
            return flag
        return self._xbar_supported_arch(
            str(self.app.cache.info.get("gpu_architecture", "") or ""),
            str(self.app.cache.info.get("codename", "") or ""),
            str(self.app.cache.info.get("gpu_name", "") or ""),
        )

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
           Blackwell).
        3. Marketing-name fallback: RTX/GTX + model number, 1000 =
           10-series floor (GTX 980/9-series and below stay hidden).
        """
        for raw in (codename, arch_id):
            head = (
                raw.lower().split("(", 1)[0].split(":", 1)[0].split("-", 1)[0].strip()
            )
            if head.startswith(("gp", "gv", "tu", "ga", "ad", "gb")):
                return True
        if any(
            name in arch_id.lower()
            for name in ("pascal", "volta", "turing", "ampere", "ada", "blackwell")
        ):
            return True
        match = re.search(r"\b(?:rtx|gtx)\s*(\d{3,4})", gpu_name.lower())
        if match:
            return int(match.group(1)) >= 1000
        return False

    @staticmethod
    def _format_xbar_offset_result(offset_mhz: int, result: object) -> str:
        """Build the log message from a ``set_clk_domain_offset`` result.

        The pynvoc call returns a dict (the applied payload with the
        driver's readback ``applied_mHz`` (or the legacy ``applied_kHz``),
        or ``{"supported": False}``) — never ``None`` — so the message is
        formatted here.
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

    @staticmethod
    def _format_clk_domain_offset_result(
        label: str, offset_mhz: int, result: object
    ) -> str:
        """Generic version of _format_xbar_offset_result for Sys/Msd/Host."""
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
                        f"Successfully applied {label} offset {offset_mhz:+d} MHz "
                        f"(driver readback {applied:+g} MHz)."
                    )
                return f"Successfully applied {label} offset {offset_mhz:+d} MHz."
            if result.get("supported") is False:
                return f"{label} clock-domain offset not supported by this driver."
        return f"Applied {label} offset {offset_mhz:+d} MHz."

    def is_ampere_plus(self) -> bool:
        """30系+ (Ampere/Ada/Blackwell): bit1 couples SYS, so an Xbar write
        must also write bit3=-f to cancel the SYS drift. Pascal/GTX16/RTX20
        are pure Xbar (direct write). Primary signal: the query_info payload's
        ``is_ampere_plus`` flag (core gpu_type.rs); fallback None → False
        (conservative: direct write, no cancel)."""
        flag = self.app.cache.info.get("is_ampere_plus")
        if isinstance(flag, bool):
            return flag
        return False

    def _is_pascal(self) -> bool:
        """Pascal detection for the MSD grey-out (Pascal bit5 SET N/A)."""
        series = str(self.app.cache.info.get("gpu_series") or "").lower()
        if "10 series" in series:
            return True
        return str(self.app.cache.info.get("codename") or "").lower().startswith("gp")

    def _sys_supported(self) -> bool:
        mask = self.app.cache.clk_domain_mask
        return mask is not None and bool(mask & (1 << 3))

    def _msd_supported(self) -> bool:
        # Pascal: bit5 SET N/A even if the mask claims the record.
        mask = self.app.cache.clk_domain_mask
        return mask is not None and bool(mask & (1 << 5)) and not self._is_pascal()

    def _host_supported(self) -> bool:
        mask = self.app.cache.clk_domain_mask
        return mask is not None and bool(mask & (1 << 9))

    def _poll_clk_domain_mask(self) -> None:
        """Async poll the controllable mask for the Sys/Msd/Host row gates.
        Piggybacks on a worker thread; never on the render path."""
        gpu = self.app.selected_gpu_target()
        if gpu is None or not self.xbar_supported():
            return

        def worker() -> None:
            try:
                data = self.app.native_service.query_private_freq_domain_info(gpu)
            except Exception:
                data = None
            try:
                self.app.call_from_thread(self._on_clk_domain_mask_loaded, data)
            except Exception:
                pass

        try:
            self.app.native_service.submit_query(worker)
        except Exception:
            pass

    def _on_clk_domain_mask_loaded(self, data: object) -> None:
        if not isinstance(data, dict):
            return
        mask_str = data.get("controllable_mask")
        try:
            mask = int(str(mask_str), 0) if mask_str is not None else None
        except ValueError:
            mask = None
        if mask is None:
            return
        self.app.cache.clk_domain_mask = mask
        # Re-apply the row disabled states now that the mask is known.
        try:
            self._prime_fabric_inputs()
        except Exception:
            pass

    def _prime_fabric_inputs(self) -> None:
        """Set the enabled state of the fabric/uncore offset rows from the
        cached controllable mask + generation (Pascal MSD greyed)."""
        fabric = self.xbar_supported() and self._oc_backend_is_nvapi()
        for wid, ok in (
            ("#xbar-offset", True),
            ("#sys-offset", self._sys_supported()),
            ("#msd-offset", self._msd_supported()),
            ("#host-offset", self._host_supported()),
        ):
            try:
                self.app.query_one(wid, Input).disabled = not (fabric and ok)
            except Exception:
                pass

    def _oc_backend_is_nvapi(self) -> bool:
        try:
            return (
                str(self.app.query_one("#oc-api", Select).value or "nvapi") == "nvapi"
            )
        except Exception:
            return True

    def _clk_domain_current_offset(self, native, gpu: str, bit: int) -> int:
        """Read a ClkDomains WRITE record's slot-0 offset (kHz) for the Sys
        RMW baseline. Returns 0 on any failure."""
        try:
            info = native.query_private_freq_domain_info(gpu)
        except Exception:
            return 0
        if not isinstance(info, dict):
            return 0
        entries = info.get("entries") or []
        if isinstance(entries, list):
            for e in entries:
                if isinstance(e, dict) and e.get("bit") == bit:
                    vals = e.get("values_kHz") or []
                    if isinstance(vals, list) and vals:
                        try:
                            return int(vals[0] or 0)
                        except (TypeError, ValueError):
                            return 0
                    break
        return 0

    @staticmethod
    def _format_volt_rail_result(target_mv: float, result: object) -> str:
        """Build the log message from a ``set_volt_rail_target`` result.

        Like ``set_clk_domain_offset``, the pynvoc call returns a dict —
        either the applied payload (with the post-clamp
        ``effective_wall_uV``) or ``{"supported": False}`` — never ``None``,
        so the message is formatted here. ``target_mv`` may carry one
        decimal (2.5 mV grid); :g drops the trailing .0.
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
    def _resolve_volt_rail_bit(volt_rail: dict) -> int:
        """Pick the VoltRails rail bit to target (GUI dashboard port).

        First rail descriptor's ``rail_bit`` when descriptors are exposed;
        otherwise the lowest set bit of ``rail_mask``. Single-rail mobile
        GPUs (e.g. 4060 Laptop, mask 0x1) resolve to 0.
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
                value = int(mask, 16)
                return (value & -value).bit_length() - 1
            except ValueError:
                pass
        return 0

    @staticmethod
    def _volt_limit_bounds_from_p0(p0: dict) -> tuple[float, float, float]:
        """Compute (min_mV, max_mV, pos_mV) for the Volt Limit input.

        - min is a hard 300 mV floor
        - max is min(VBIOS wall, VRM max wall); 0 = 'not reported' is
          skipped; both 0 falls back to 1200 mV (the ~1.2 V domain ceiling
          observed on Ada mobile)
        - pos is the effective voltage wall (post-clamp) — the analog of
          TGP positioning at the enforced power limit, NOT the live core
          voltage. Both max and pos snap to the 2.5 mV grid (LCM of the
          5 mV rail step on 30/40-series and 12.5 mV on 10/20-series); max
          snaps DOWN so no offered position exceeds the actual wall.
        """
        step = 2.5
        vbios = int(p0.get("vbios_wall_uV", 0) or 0)
        vrm = int(p0.get("vrm_max_wall_uV", 0) or 0)
        walls = [w for w in (vbios, vrm) if w > 0]
        ceiling_uV = min(walls) if walls else 1_200_000
        max_mv = max(300.0, ceiling_uV / 1000.0)
        max_mv = int(max_mv / step) * step
        eff = int(p0.get("effective_wall_uV", 0) or 0)
        pos_mv = eff / 1000.0 if eff > 0 else max_mv
        pos_mv = max(300.0, min(max_mv, pos_mv))
        pos_mv = round(pos_mv / step) * step
        return 300.0, max_mv, max(300.0, pos_mv)

    def is_legacy_voltage(self) -> bool:
        """Legacy-voltage verdict (Maxwell / GTX 900 series and older).

        Primary signal: the query_info payload's ``is_legacy_voltage`` flag
        (core gpu_type.rs). Fallback heuristic: GTX model < 1000, or a
        Maxwell/Kepler/Fermi arch string / gm/gk/gf chip prefix.
        """
        flag = self.app.cache.info.get("is_legacy_voltage")
        if isinstance(flag, bool):
            return flag
        gpu_name = str(self.app.cache.info.get("gpu_name", "")).lower()
        arch = str(self.app.cache.info.get("gpu_architecture", "") or "").lower()
        codename = str(self.app.cache.info.get("codename", "") or "").lower()
        if "gtx" in gpu_name:
            match = re.search(r"gtx\s*(\d+)", gpu_name)
            if match and int(match.group(1)) < 1000:
                return True
        if any(x in arch for x in ("maxwell", "kepler", "fermi")):
            return True
        head = codename.split("(", 1)[0].split(":", 1)[0].split("-", 1)[0].strip()
        return head.startswith(("gm", "gk", "gf"))

    def apply_pstate_limits(
        self,
        native,
        gpu: str,
        backend: str,
        pstart: str,
        pend: str,
    ) -> str:
        """Mem-range range lock first; native single-state pin on failure.

        A runtime failure on the mem-range lock marks a pre-Kepler part (the
        NVML pstate mem-clock query the window derivation needs is Not
        Supported there) — retry once with the native pin, collapsing the
        range to its high-perf endpoint (the pin has no range form).
        """
        if self._pstate_pin_fallback:
            # Native single-P-State pin (no range form). The caller collapses
            # pend == pstart; pstart is the single target.
            native.set_pstate_native_lock(gpu, pstart)
            return f"Successfully pinned NVAPI P-State {pstart}."
        try:
            warning = (
                native.set_nvml_pstate_lock(gpu, pstart, pend)
                if backend == "nvml"
                else native.set_nvapi_pstate_lock(gpu, pstart, pend)
            )
        except Exception:
            # First mem-range attempt failed → pre-Kepler part. The NVML
            # pstate mem-clock ranges the window derivation needs are Not
            # Supported there; the native single-state pin is the only path.
            self._pstate_pin_fallback = True
            try:
                native.set_pstate_native_lock(gpu, pstart)
            except Exception as pin_exc:
                raise self.enrich_pstate_exception(pin_exc) from pin_exc
            return (
                "Memory-range P-State lock unavailable on this GPU — "
                f"falling back to the native single-P-State pin.\n"
                f"Successfully pinned NVAPI P-State {pstart}."
            )
        message = f"Successfully applied {backend} PState limits {pstart}-{pend}."
        if warning:
            # Overlapping P-States ride the same memory window by
            # construction (e.g. a VBIOS edit pinning P2 to P0's clocks) —
            # the lock applied anyway; surface the caveat.
            message = f"Warning: {warning}\n{message}"
        return message

    def reset_pstate_limits(self, native, gpu: str, backend: str) -> str:
        if self._pstate_pin_fallback:
            native.reset_pstate_native_lock(gpu)
            return "Successfully reset NVAPI P-State lock."
        if backend == "nvml":
            native.reset_locked_clocks(gpu, backend, "memory")
        else:
            native.reset_vfp_frequency_lock(gpu, "memory")
        return f"Successfully reset {backend} PState limits."

    def apply_limits(
        self,
        native,
        gpu: str,
        backend: str,
        power_limit: int,
        thermal_limit: int,
        voltage_boost: int,
    ) -> str:
        native.set_power_limit(gpu, backend, power_limit)
        if backend == "nvapi":
            native.set_thermal_limit(gpu, thermal_limit)
            if self.is_legacy_voltage():
                # Maxwell/900-series and older: the boost input is an
                # Overvolt value in mV, routed to the legacy delta path.
                native.set_legacy_voltage_delta(gpu, voltage_boost * 1000, "P0")
            else:
                native.set_voltage_boost(gpu, voltage_boost)
        return f"Successfully applied {backend} limits."

    def is_mobile(self) -> bool:
        """Mobile-GPU verdict for the Mobile Power pane.

        Primary signal: the query_info payload's ``is_mobile`` flag computed
        in Rust by core's gpu_type.rs detect_gpu_type (name + codename — the
        single source of truth). Fallback: the name-keyword heuristic for
        payloads without the flag (older pynvoc, CLI-parsed info).
        """
        flag = self.app.cache.info.get("is_mobile")
        if isinstance(flag, bool):
            return flag
        gpu_name = str(self.app.cache.info.get("gpu_name", "")).lower()
        return (
            "mobile" in gpu_name
            or "laptop" in gpu_name
            or " m " in gpu_name
            or gpu_name.endswith(" m")
            or " mx " in gpu_name
            or gpu_name.endswith(" mx")
        )

    def is_server(self) -> bool:
        """Server-grade verdict (Tesla/datacenter passive parts: P100/A100 …).

        Primary signal: the query_info payload's ``is_server`` flag from core
        gpu_type.rs detect_gpu_type — Server* variants only (all Volta folds
        into ServerVolta; Titan V's blower fan is corrected by the async
        cooler count). Used to grey the Fan pane out synchronously; the
        async NVML cooler count refines it (ServerLovelace L40/L4 carry
        onboard fans, count ≥ 1 re-enables).
        """
        flag = self.app.cache.info.get("is_server")
        return isinstance(flag, bool) and flag

    def _load_fan_surface(self) -> None:
        """Background-load the fan surface (NVML fan info + NVAPI cooler
        family) and adapt the Fan pane.

        The two answers together carry the legacy verdict: on ≤ Kepler
        drivers the private NVAPI cooler family reports zero coolers while
        NVML still answers (v1 GetFanSpeed: count=1 + live current percent)
        — that pair restricts the policy dropdown to default/manual. Modern
        cards answer both surfaces (1650 Super / A4000: NVAPI count=1) and
        keep continuous/manual. Either way the fan Target dropdown is
        restricted to the real count (no "Fan 2" on single-fan cards) and
        the Level input is seeded with the live duty.
        Fanless server cards (P100/A100 …) report count=0 — the whole pane
        greys out (same verdict surface as the GUI's set_supported_state).
        """
        gpu = self.app.selected_gpu_target()
        if gpu is None or not self._fan_surface_lock.acquire(blocking=False):
            return

        def worker() -> None:
            try:
                data = self.app.native_service.query_fan_info(gpu)
            except Exception:
                data = None
            try:
                cooler = self.app.native_service.query_cooler_info(gpu)
            except Exception:
                cooler = None
            finally:
                self._fan_surface_lock.release()
            try:
                self.app.call_from_thread(self._on_fan_surface, gpu, data, cooler)
            except Exception:
                pass

        threading.Thread(
            target=worker, daemon=True, name="nvoc-tui-fan-surface"
        ).start()

    def _set_fan_pane_disabled(self, disabled: bool) -> None:
        """Grey out (or restore) the whole Fan pane.

        Textual's ``disabled`` reactive cascades to every child widget
        (Select/Input/Button refuse interaction) and the ``:disabled``
        pseudo-class drives the dim style in overclock.tcss — the TUI
        counterpart of the GUI fan pane's ``set_supported_state``.
        """
        for selector in ("#fan-controls", "#fan-actions"):
            try:
                self.app.query_one(selector).disabled = disabled
            except Exception:
                pass

    def _on_fan_surface(
        self, gpu: str, data: dict | None, cooler_data: dict | None = None
    ) -> None:
        if not isinstance(data, dict):
            return
        # A GPU switch between dispatch and completion must not re-verdict
        # the pane for the wrong card.
        try:
            if gpu != self.app.selected_gpu_target():
                return
        except Exception:
            pass
        try:
            count = data.get("count")
            count = count if isinstance(count, int) else 0
            current = data.get("current_percent")
            current = current if isinstance(current, int) else None
            if count >= 1:
                self._fanless_gpus.discard(gpu)
                self._fanned_gpus.add(gpu)
                self._set_fan_pane_disabled(False)
                options = [("All", "all")] + [
                    (f"Fan {i}", str(i)) for i in range(1, count + 1)
                ]
                select = self.app.query_one("#fan-id", Select)
                current_value = str(select.value or "all")
                select.set_options(options)
                if current_value in {value for _, value in options}:
                    select.value = current_value
                # Legacy verdict = the original GT730 signature: NVML sees
                # fans while the private NVAPI cooler family reports none
                # (≤ Kepler drivers). Modern cards answer the family too
                # (1650 Super / A4000 both count=1), so they KEEP the
                # continuous/manual policy list — the old unconditional
                # verdict flagged every fanned GPU as legacy. A missing
                # NVAPI answer conservatively counts as legacy. The verdict
                # must run both ways: switching legacy → modern restores
                # the modern list on GPU switch.
                cooler_count = (
                    cooler_data.get("count") if isinstance(cooler_data, dict) else None
                )
                # None (NVAPI unanswered) or 0 → legacy; ≥1 → modern.
                legacy_nvapi = not cooler_count
                # Legacy GPUs (≤ Kepler): modern NVAPI CoolerPolicy types are
                # rejected by the old driver — restrict the policy dropdown to
                # default/manual and default to manual. Keep the NVAPI backend
                # (the working control path there; NVML's control path binds
                # v2-only symbols absent in R391's nvml.dll).
                policy_select = self.app.query_one("#fan-policy", Select)
                policy_value = str(policy_select.value or "continuous")
                if legacy_nvapi:
                    policy_options = [("default", "default"), ("manual", "manual")]
                    fallback = "manual"
                else:
                    # Modern coolers ignore `manual` on the NVAPI path —
                    # offer continuous only.
                    policy_options = [("contin.", "continuous")]
                    fallback = "continuous"
                policy_select.set_options(policy_options)
                policy_select.value = (
                    policy_value
                    if policy_value in {value for _, value in policy_options}
                    else fallback
                )
                if count == 1 and current is not None:
                    self.set_input("#fan-level", str(max(0, min(100, int(current)))))
            elif count == 0:
                # Fanless card (P100/A100 server parts — the private NVAPI
                # cooler family also reports NOT_SUPPORTED there): grey the
                # pane out instead of leaving dead Apply buttons behind.
                self._fanless_gpus.add(gpu)
                self._fanned_gpus.discard(gpu)
                self._set_fan_pane_disabled(True)
        except Exception:
            pass

    def load_mobile_limits(self, force: bool = False) -> None:
        """Background-load the mobile control surface via pynvoc (NVAPI)."""
        gpu = self.app.selected_gpu_target()
        if gpu is None or not self.is_mobile():
            return
        if not force and gpu == self._mobile_limits_gpu:
            return
        if not self._mobile_load_lock.acquire(blocking=False):
            return

        def worker() -> None:
            try:
                data = self.app.native_service.query_mobile_limits(gpu)
            except Exception as exc:
                data = {"error": str(exc)}
            finally:
                self._mobile_load_lock.release()
            try:
                self.app.call_from_thread(self._on_mobile_limits, gpu, data)
            except Exception:
                pass

        threading.Thread(
            target=worker, daemon=True, name="nvoc-tui-mobile-limits"
        ).start()

    def _on_mobile_limits(self, gpu: str, data: dict) -> None:
        first_load = self._mobile_limits_gpu != gpu
        self._mobile_limits_gpu = gpu
        tgp = data.get("tgp") if isinstance(data.get("tgp"), dict) else None
        dnotifier = (
            data.get("dnotifier") if isinstance(data.get("dnotifier"), dict) else None
        )
        policies = data.get("temp_policies") or []
        notes: list[str] = []

        if tgp and tgp.get("min_watt") is not None and tgp.get("max_watt") is not None:
            self._tgp_policy_index = int(tgp.get("policy_index", 2))
            lo = int(round(float(tgp["min_watt"])))
            hi = int(round(float(tgp["max_watt"])))
            self._tgp_range = (lo, hi)
            # Anchor the input at the actually-effective power wall
            # (``power_limit_w`` — min of requested TGP and the active
            # D-Notifier cap, i.e. nvidia-smi's PPAB Ceiling "Current"),
            # clamped into the TGP range. The VBIOS ``default_watt`` is only
            # the no-reading fallback: anchoring at the default made the
            # input jump to a value that is neither the old nor the new real
            # wall after every D-Notifier apply/reset.
            position = int(round(float(tgp.get("default_watt") or lo)))
            current = data.get("power_limit_w")
            if current is not None:
                try:
                    position = max(lo, min(hi, int(round(float(current)))))
                except (TypeError, ValueError):
                    pass
            self.set_input("#mobile-tgp", str(position))
        else:
            notes.append("TGP range unavailable")

        if dnotifier and dnotifier.get("levels"):
            options = []
            for item in dnotifier["levels"]:
                label = str(item.get("level", "")).upper()
                try:
                    level_num = int(label.lstrip("D"))
                except ValueError:
                    continue
                watts = item.get("watts")
                display = (
                    f"{label} · {float(watts):.0f}W" if watts is not None else label
                )
                options.append((display, level_num))
            select = self.app.query_one("#mobile-dnotifier", Select)
            select.set_options(options)
            active = dnotifier.get("active")
            if active:
                try:
                    select.value = int(str(active).upper().lstrip("D"))
                except ValueError:
                    pass
        else:
            notes.append("D-Notifier unavailable")

        target = None
        for policy in policies:
            if (
                isinstance(policy, dict)
                and policy.get("min") is not None
                and policy.get("max") is not None
                and float(policy["max"]) > float(policy["min"])
            ):
                target = policy
                break
        if target is not None:
            self._target_temp_range = (
                int(round(float(target["min"]))),
                int(round(float(target["max"]))),
            )
            self.set_input(
                "#mobile-target-temp", str(int(round(float(target.get("celsius", 87)))))
            )
        else:
            notes.append("Target Temp range unavailable")

        # Volt Limit (private VoltRails P0 bounds; GUI dashboard parity).
        vr = data.get("volt_rail") if isinstance(data.get("volt_rail"), dict) else None
        p0 = vr.get("p0") if vr and isinstance(vr.get("p0"), dict) else None
        if p0:
            self._volt_rail_bit = self._resolve_volt_rail_bit(vr)
            min_mv, max_mv, pos_mv = self._volt_limit_bounds_from_p0(p0)
            self._volt_limit_range = (min_mv, max_mv)
            self._volt_limit_supported = True
            # Strip the trailing .0 of whole-mV positions (:g-style).
            self.set_input(
                "#mobile-volt-limit",
                f"{pos_mv:g}",
            )
        else:
            self._volt_limit_supported = False
            notes.append("Volt Limit unavailable")
        try:
            self.app.query_one("#mobile-volt-limit", Input).disabled = not (
                self._volt_limit_supported
            )
        except Exception:
            pass

        if notes:
            self.app.write_log("Mobile power: " + ", ".join(notes) + ".")

        # PPAB has no read-back API; enable it once per GPU on load.
        # Only attempt when the private NVAPI surface actually resolved —
        # on Linux (libnvidia-api stub) / older drivers the setter is
        # NO_IMPLEMENTATION and auto-enabling would just log an error.
        if first_load and (tgp or dnotifier):
            self.app.run_native_action(
                "enable dynamic boost",
                lambda native, gpu=gpu: (
                    native.set_ppab_status(gpu, True) or "Dynamic Boost (PPAB) enabled."
                ),
            )

    def set_input(self, selector: str, value: str) -> None:
        try:
            self.app.query_one(selector, Input).value = value
        except Exception:
            pass

    def apply_mobile(
        self,
        native,
        gpu: str,
        ppab: bool,
        d_level: int,
        tgp_watts: int,
        target_temp: int,
        volt_limit_mv: float | None = None,
        volt_rail_bit: int | None = None,
    ) -> str:
        native.set_ppab_status(gpu, ppab)
        native.set_dnotifier(gpu, d_level)
        native.set_tgp_watt(gpu, tgp_watts, self._tgp_policy_index)
        native.set_target_temp(gpu, float(target_temp), 2)
        message = (
            f"Successfully applied mobile power: PPAB {'on' if ppab else 'off'}, "
            f"D{d_level}, TGP {tgp_watts} W, target {target_temp} C."
        )
        if volt_limit_mv is not None:
            # set_volt_rail_target returns a dict (never None), so the
            # message is appended rather than ``or``-chained.
            message += "\n" + self._format_volt_rail_result(
                volt_limit_mv,
                native.set_volt_rail_target(
                    gpu,
                    self._volt_rail_bit if volt_rail_bit is None else volt_rail_bit,
                    volt_limit_mv,
                    None,
                ),
            )
        return message

    def reset_mobile(self, native, gpu: str) -> str:
        native.reset_tgp_watt(gpu, self._tgp_policy_index)
        return "Successfully reset TGP to default."

    def apply_fan(
        self,
        native,
        gpu: str,
        backend: str,
        fan_id: str,
        reset: bool,
        policy: str,
        level: int,
    ) -> str:
        if reset:
            native.set_fan(gpu, backend, fan_id, "auto", 0)
            return "Successfully reset fan control."
        else:
            native.set_fan(gpu, backend, fan_id, policy, level)
            return f"Successfully applied fan {fan_id} {policy} level {level}%."

    def handle_button(self, button_id: str) -> bool:
        if button_id == "oc-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
            core_offset = self.get_int("#core-offset")
            mem_offset = self.get_int("#mem-offset")
            # Fabric/uncore ride the NVAPI-only ClockClient path — skipped under
            # NVML and on pre-Pascal archs (rows disabled, inputs stay 0).
            fabric_ok = backend == "nvapi" and self.xbar_supported()
            xbar_offset = self.get_int("#xbar-offset") if fabric_ok else None
            sys_offset = (
                self.get_int("#sys-offset")
                if fabric_ok and self._sys_supported()
                else None
            )
            msd_offset = (
                self.get_int("#msd-offset")
                if fabric_ok and self._msd_supported()
                else None
            )
            host_offset = (
                self.get_int("#host-offset")
                if fabric_ok and self._host_supported()
                else None
            )

            def apply_oc(
                native,
                gpu=gpu,
                backend=backend,
                core_offset=core_offset,
                mem_offset=mem_offset,
                xbar_offset=xbar_offset,
                sys_offset=sys_offset,
                msd_offset=msd_offset,
                host_offset=host_offset,
            ) -> str:
                return self.apply_oc(
                    native,
                    gpu,
                    backend,
                    core_offset,
                    mem_offset,
                    xbar_offset,
                    sys_offset,
                    msd_offset,
                    host_offset,
                )

            self.app.run_native_action(
                "apply overclock",
                apply_oc,
            )
            return True
        if button_id == "pstate-limits-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")
            pstart = (
                self.normalize_pstate(self.app.query_one("#pstate-start", Input).value)
                or "P0"
            )
            pend = (
                self.normalize_pstate(self.app.query_one("#pstate-end", Input).value)
                or pstart
            )
            # After a mem-range failure (pre-Kepler part) the native pin has
            # no range form — collapse to the single high-perf endpoint.
            if self._pstate_pin_fallback:
                pend = pstart

            pstate_error = self.validate_pstates(pstart, pend)
            if pstate_error:
                self.app.write_log(pstate_error)
                return True

            def apply_pstate_limits(
                native, gpu=gpu, backend=backend, pstart=pstart, pend=pend
            ) -> str:
                return self.apply_pstate_limits(native, gpu, backend, pstart, pend)

            self.app.run_native_action(
                "apply PState limits",
                apply_pstate_limits,
            )
            return True
        if button_id == "pstate-limits-reset":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#oc-api", Select).value or "nvapi")

            def reset_pstate_limits(native, gpu=gpu, backend=backend) -> str:
                return self.reset_pstate_limits(native, gpu, backend)

            self.app.run_native_action(
                "reset PState limits",
                reset_pstate_limits,
            )
            return True
        if button_id == "oc-reset":
            backend = self.app.query_one("#oc-api", Select).value or "nvapi"
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True
            resets = [
                (
                    "reset core offset",
                    lambda native, gpu=gpu, backend=str(backend): (
                        native.set_clock_offset(gpu, backend, "core", 0, "P0")
                        or "Successfully reset core offset."
                    ),
                ),
                (
                    "reset memory offset",
                    lambda native, gpu=gpu, backend=str(backend): (
                        native.set_clock_offset(gpu, backend, "memory", 0, "P0")
                        or "Successfully reset memory offset."
                    ),
                ),
            ]
            if str(backend) == "nvapi" and self.xbar_supported():
                coupled = self.is_ampere_plus()
                resets.append((
                    "reset xbar offset",
                    lambda native, gpu=gpu: self._format_xbar_offset_result(
                        0, native.set_clk_domain_offset(gpu, 1, 0, None, None)
                    ),
                ))
                # 30+ couples bit3 — clear the -f cancel too (bit3=0).
                if coupled:
                    resets.append((
                        "reset sys-cancel",
                        lambda native, gpu=gpu: self._format_clk_domain_offset_result(
                            "Sys-cancel",
                            0,
                            native.set_clk_domain_offset(gpu, 3, 0, None, None),
                        ),
                    ))
                if self._sys_supported():
                    resets.append((
                        "reset sys offset",
                        lambda native, gpu=gpu: self._format_clk_domain_offset_result(
                            "Sys",
                            0,
                            native.set_clk_domain_offset(gpu, 3, 0, None, None),
                        ),
                    ))
                if self._msd_supported():
                    resets.append((
                        "reset msd offset",
                        lambda native, gpu=gpu: self._format_clk_domain_offset_result(
                            "Msd",
                            0,
                            native.set_clk_domain_offset(gpu, 5, 0, None, None),
                        ),
                    ))
                if self._host_supported():
                    resets.append((
                        "reset host offset",
                        lambda native, gpu=gpu: self._format_clk_domain_offset_result(
                            "Host",
                            0,
                            native.set_clk_domain_offset(gpu, 9, 0, None, None),
                        ),
                    ))
            self.app.run_action_chain(resets)
            return True
        if button_id == "limits-apply":
            gpu = self.app.selected_gpu_target()
            backend = str(self.app.query_one("#power-api", Select).value or "nvapi")
            power_limit = self.get_int("#power-limit")
            thermal_limit = self.get_int("#thermal-limit")
            voltage_boost = self.get_int("#voltage-boost")

            def apply_limits(
                native,
                gpu=gpu,
                backend=backend,
                power_limit=power_limit,
                thermal_limit=thermal_limit,
                voltage_boost=voltage_boost,
            ) -> str:
                return self.apply_limits(
                    native,
                    gpu,
                    backend,
                    power_limit,
                    thermal_limit,
                    voltage_boost,
                )

            self.app.run_native_action(
                "apply limits",
                apply_limits,
            )
            return True
        if button_id == "reset-limits":
            gpu = self.app.selected_gpu_target()

            def reset_limits(native, gpu=gpu) -> str:
                native.reset_all(gpu, None)
                return "Successfully reset all limits."

            self.app.run_native_action(
                "reset all limits",
                reset_limits,
            )
            return True
        if button_id == "fan-apply":
            gpu = self.app.selected_gpu_target()
            backend = (
                "nvml-cooler"
                if str(self.app.query_one("#fan-api", Select).value or "nvapi")
                == "nvml"
                else "nvapi-cooler"
            )
            fan_id = str(self.app.query_one("#fan-id", Select).value or "all")
            policy = str(
                self.app.query_one("#fan-policy", Select).value or "continuous"
            )
            level = self.get_int("#fan-level", 60)

            def apply_fan(
                native,
                gpu=gpu,
                backend=backend,
                fan_id=fan_id,
                policy=policy,
                level=level,
            ) -> str:
                return self.apply_fan(
                    native, gpu, backend, fan_id, False, policy, level
                )

            self.app.run_native_action(
                "apply fan",
                apply_fan,
            )
            return True
        if button_id == "fan-reset":
            gpu = self.app.selected_gpu_target()
            backend = (
                "nvml-cooler"
                if str(self.app.query_one("#fan-api", Select).value or "nvapi")
                == "nvml"
                else "nvapi-cooler"
            )
            fan_id = str(self.app.query_one("#fan-id", Select).value or "all")

            def reset_fan(native, gpu=gpu, backend=backend, fan_id=fan_id) -> str:
                return self.apply_fan(native, gpu, backend, fan_id, True, "auto", 0)

            self.app.run_native_action(
                "reset fan",
                reset_fan,
            )
            return True
        if button_id == "mobile-apply":
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True
            ppab = str(self.app.query_one("#mobile-ppab", Select).value or "on") == "on"
            try:
                d_level = int(
                    self.app.query_one("#mobile-dnotifier", Select).value or 1
                )
            except (TypeError, ValueError):
                d_level = 1
            if not 1 <= d_level <= 5:
                self.app.write_log("D-Notifier level must be D1-D5.")
                return True
            tgp_watts = self.get_int("#mobile-tgp", 100)
            lo, hi = self._tgp_range
            tgp_watts = max(lo, min(hi, tgp_watts))
            target_temp = self.get_int("#mobile-target-temp", 87)
            tlo, thi = self._target_temp_range
            target_temp = max(tlo, min(thi, target_temp))
            # Volt Limit rides along when the VoltRails surface resolved;
            # empty input = leave the wall untouched.
            volt_limit = None
            if self._volt_limit_supported:
                raw = self.app.query_one("#mobile-volt-limit", Input).value.strip()
                if raw:
                    try:
                        volt_limit = float(raw)
                    except ValueError:
                        self.app.write_log(
                            f"Invalid Volt Limit value: {raw!r} (expected mV)."
                        )
                        return True
                    vlo, vhi = self._volt_limit_range
                    volt_limit = max(vlo, min(vhi, volt_limit))

            def apply_mobile(
                native,
                gpu=gpu,
                ppab=ppab,
                d_level=d_level,
                tgp_watts=tgp_watts,
                target_temp=target_temp,
                volt_limit=volt_limit,
            ) -> str:
                result = self.apply_mobile(
                    native, gpu, ppab, d_level, tgp_watts, target_temp, volt_limit
                )
                # ALWAYS re-load after a mobile apply (not just when a Volt
                # Limit rode along): the D-Notifier level clamps the TGP
                # wall and the driver may clamp the TGP SET itself, so the
                # typed values can differ from what took effect — re-anchor
                # every input to the real wall before the user reads them
                # (the GUI reloads after each of its applies for the same
                # reason). Scheduled via call_from_thread so it runs on the
                # UI thread AFTER the SET completed.
                try:
                    self.app.call_from_thread(self.load_mobile_limits, True)
                except Exception:
                    pass
                return result

            self.app.run_native_action(
                "apply mobile power",
                apply_mobile,
            )
            return True
        if button_id == "mobile-reset":
            gpu = self.app.selected_gpu_target()
            if gpu is None:
                self.app.write_log("No GPU selected.")
                return True

            def reset_mobile(native, gpu=gpu) -> str:
                return self.reset_mobile(native, gpu)

            self.app.run_native_action(
                "reset mobile power",
                reset_mobile,
            )
            self.load_mobile_limits(force=True)
            return True
        return False
