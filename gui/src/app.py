"""
NVOC-GUI Application - Main application window.
"""

import csv
import customtkinter as ctk
import ctypes
import os
import re
import sys
import threading
from typing import TYPE_CHECKING, Any, Callable, Dict, List, Optional, Tuple

import pystray
from PIL import Image

from src.cli_runner import CLIRunner
from src.config import Config
from src.widgets.output_console import OutputConsole
from src.widgets.lightweight_controls import LiteButton
from src.tabs.dashboard import DashboardTab
from src.tabs.autoscan import AutoscanTab
from src.tabs.overclock import OverclockTab
from src.tabs.vfcurve import VFCurveTab


import shutil

if TYPE_CHECKING:
    from src.single_instance import SingleInstanceGuard


def find_cli_exe() -> str:
    """Find the CLI executable from PATH. Returns empty string if not found."""
    return (
        shutil.which("nvoc-autooptimizer") or shutil.which("nvoc-auto-optimizer") or ""
    )


class App(ctk.CTk):
    """Main application window."""

    def __init__(self, single_instance_guard: Optional["SingleInstanceGuard"] = None):
        super().__init__()

        # Global resize session state (used to coalesce expensive per-tab redraw work)
        self._is_resizing = False
        self._resize_settle_after_id = None  # type: Optional[str]
        self._last_root_width = None  # type: Optional[int]
        self._resize_targets = []  # type: List[Any]

        self.title("NVOC-GUI — NVIDIA GPU VF Curve Optimizer")
        self.geometry("768x672")
        self.minsize(768, 360)
        self._single_instance_guard = single_instance_guard

        # Appearance
        ctk.set_appearance_mode("dark")
        ctk.set_default_color_theme("blue")

        # Tray icon (initialized lazily)
        self._tray_icon = None  # type: Optional[pystray.Icon]
        self._tray_thread = None  # type: Optional[threading.Thread]
        self._tray_image = None  # type: Optional[Image.Image]

        # ✕ Close button → exit completely
        # Minimize button → hide to tray (via <Unmap>)
        self.protocol("WM_DELETE_WINDOW", self._quit_app)
        self.bind("<Unmap>", self._on_unmap)

        # Config
        app_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        self.config = Config(app_dir)

        # CLI runner
        saved = self.config.get("cli_exe_path") or ""
        if saved and not os.path.isfile(saved):
            # Stale path in config – discard it and search PATH fresh
            self.config.set("cli_exe_path", "")
            saved = ""
        exe_path = saved or find_cli_exe()
        self.config.set("cli_exe_path", exe_path)

        # Determine working directory for CLI (the auto-optimizer project root)
        self.cli_cwd = (
            os.path.dirname(os.path.dirname(exe_path))
            if os.path.isfile(exe_path)
            else None
        )
        # cli_cwd should be the project root (parent of target/release/)
        if self.cli_cwd and os.path.basename(self.cli_cwd) == "target":
            self.cli_cwd = os.path.dirname(self.cli_cwd)

        self._build_ui()

        # Setup CLI runner (after console is created)
        self.runner = CLIRunner(exe_path, on_output=self._on_cli_output)

        if not exe_path:
            self.console.append(
                "[GUI] CLI executable not found in PATH.\n"
                "[GUI] Please click '⚙ CLI Path' to locate nvoc-autooptimizer.exe manually.\n"
            )

        # GPU index → display name mapping
        self.gpu_map: Dict[str, int] = {}  # display_text -> gpu_index
        self.gpu_names: Dict[
            int, str
        ] = {}  # gpu_index -> gpu_name string (for capabilities check)
        self.gpu_arches: Dict[
            int, str
        ] = {}  # gpu_index -> architecture string from 'info'
        self.gpu_uuid_map: Dict[int, str] = {}  # gpu_index -> UUID string
        self._gpu_short_label_by_idx: Dict[int, str] = {}
        self._gpu_long_label_by_idx: Dict[int, str] = {}
        self._gpu_limits_cache: Dict[
            str, Any
        ] = {}  # cached limits from last 'info' parse
        self._gpu_pstates_cache: list[str] = []
        self._dashboard_gpu_lock_active: bool = False
        self._dashboard_mem_lock_active: bool = False
        self._vfp_offset_state_cache = None  # type: Optional[Tuple[bool, Optional[int]]]
        self._vfp_offset_refresh_inflight = False  # is a worker running now
        self._pending_vfp_offset_refresh = False  # do we need one more run

        # Guard to suppress _on_gpu_changed during programmatic gpu_var.set() calls
        self._programmatic_gpu_set: bool = False

        # Initial GPU list
        self.after(500, self._refresh_gpu_list)

        if sys.platform == "win32" and self._single_instance_guard is not None:
            self.after(200, self._poll_single_instance_signal)

    def _build_ui(self):
        """Build the main UI layout."""

        # === Top Bar: GPU selector + settings ===
        top_bar = ctk.CTkFrame(self, height=25)
        top_bar.pack(fill="x", padx=10, pady=(5, 1))
        top_bar.pack_propagate(False)

        ctk.CTkLabel(top_bar, text="🎮 GPU:", font=("", 14, "bold")).pack(
            side="left", padx=(10, 5)
        )

        self.gpu_var = ctk.StringVar(value="(detecting...)")
        self.gpu_dropdown = ctk.CTkOptionMenu(
            top_bar,
            variable=self.gpu_var,
            values=["(detecting...)"],
            width=300,
            command=self._on_gpu_changed,
        )
        self.gpu_dropdown.pack(side="left", padx=5)

        LiteButton(
            top_bar, text="🔍 Detect", width=80, command=self._refresh_gpu_list
        ).pack(side="left", padx=5)

        # Pin/Topmost button
        self.is_topmost = False

        def toggle_topmost():
            self.is_topmost = not self.is_topmost
            self.attributes("-topmost", self.is_topmost)
            if self.is_topmost:
                self.pin_btn.configure(
                    text="📌", fg_color="#d35400", hover_color="#e67e22"
                )
            else:
                self.pin_btn.configure(
                    text="📌", fg_color="#2f6fa5", hover_color="#3B8ED0"
                )

        self.pin_btn = LiteButton(top_bar, text="📌", width=30, command=toggle_topmost)
        self.pin_btn.pack(side="right", padx=(5, 10))

        # Exe path button
        LiteButton(
            top_bar, text="⚙ CLI Path", width=85, command=self._set_cli_path
        ).pack(side="right", padx=5)

        # Show exe path label
        self.exe_label = ctk.CTkLabel(
            top_bar, text="", font=("", 9), text_color="gray60"
        )
        self.exe_label.pack(side="right", padx=0)
        self._update_exe_label()

        # === Main content: PanedWindow with tabs + console ===
        # Using a simple pack layout with configurable console

        # Tab view
        self.tabview = ctk.CTkTabview(self, anchor="nw", command=self._on_tab_changed)
        self.tabview.pack(fill="both", expand=True, padx=10, pady=(0, 5))

        # Resize coordinator: mark active resize sessions and flush once at settle.
        self.bind("<Configure>", self._on_root_configure, add="+")

        # Performance fix: CTkTabview is very sensitive to high-frequency <Configure> events.
        # However, CustomTkinter handles internal layout via these events.
        # We can try to reduce the impact by ensuring the main window doesn't update
        # too many internal state variables during the drag.

        # Create tabs
        _tab_dashboard = self.tabview.add("📊 Dashboard")
        _tab_autoscan = self.tabview.add("🔍 Autoscan")
        _tab_overclock = self.tabview.add("⚡ Overclock")
        _tab_vfcurve = self.tabview.add("📈 VF Curve")

        # === Bottom: Output Console ===
        # (Created before tabs so the main frame structure is visible during load)
        self.console = OutputConsole(self, height=200)
        self.console.pack(fill="x", padx=10, pady=(0, 10))

        # class MockConsole:
        #     def append(self, text: str): pass
        #     def clear(self): pass
        # self.console = MockConsole()

        # Show the window layout immediately before rendering heavy tabs
        self.update()

        # Placeholders for tabs
        self.tab_dashboard = None
        self.tab_autoscan = None
        self.tab_overclock = None
        self.tab_vfcurve = None

        # Defer initialization of the default selected tab to avoid blocking startup
        self.after(50, self._on_tab_changed)

    def _on_tab_changed(self):
        """Lazy load tabs when they are selected and handle visibility for performance."""
        current_tab = self.tabview.get()

        if current_tab.endswith("Dashboard") and self.tab_dashboard is None:
            self.tab_dashboard = DashboardTab(self.tabview.tab("📊 Dashboard"), self)
            self.register_resize_target(self.tab_dashboard)
            # Reset vfp_lock sentinel if needed
            self.tab_dashboard._last_vfp_lock_mv = object()
            self._sync_dashboard_lock_state_from_cache()
            # Force one immediate lock-state refresh chain for dashboard-only usage.
            self._query_gpu_get()
            self.tab_dashboard._fetch_once()

        elif current_tab.endswith("VF Curve"):
            if self.tab_vfcurve is None:
                self.tab_vfcurve = VFCurveTab(self.tabview.tab("📈 VF Curve"), self)
                self.register_resize_target(self.tab_vfcurve)
                if hasattr(self, "_locked_voltage_mv_cache"):
                    self.tab_vfcurve.sync_lock_from_voltage(
                        self._locked_voltage_mv_cache
                    )
                if hasattr(self, "_gpu_limits_cache") and self._gpu_limits_cache:
                    self.tab_vfcurve.sync_freq_locks_from_cache(self._gpu_limits_cache)
            # Refresh frequency lock states from CLI on every tab entry.
            self._query_gpu_get()
            # Always refresh when entering VF Curve to keep the plot up to date.
            self.tab_vfcurve._refresh_curve()

        elif current_tab.endswith("Autoscan") and self.tab_autoscan is None:
            self.tab_autoscan = AutoscanTab(self.tabview.tab("🔍 Autoscan"), self)
            self.register_resize_target(self.tab_autoscan)

        elif current_tab.endswith("Overclock"):
            if self.tab_overclock is None:
                self.tab_overclock = OverclockTab(
                    self.tabview.tab("⚡ Overclock"), self
                )
                self.register_resize_target(self.tab_overclock)
                # Sync any cached info if available
                if hasattr(self, "_gpu_limits_cache") and self._gpu_limits_cache:
                    self.tab_overclock.check_capabilities(self._gpu_limits_cache)
                    self.tab_overclock.update_limits(self._gpu_limits_cache)
                elif self._gpu_pstates_cache:
                    self.tab_overclock.set_supported_pstates(self._gpu_pstates_cache)
                if self._vfp_offset_state_cache is not None:
                    has_vfp_offset, uniform_offset = self._vfp_offset_state_cache
                    self.tab_overclock.set_vfp_state(has_vfp_offset, uniform_offset)
                if self.tab_vfcurve is None:
                    self.refresh_vfp_offset_state(force=True)
            # Always force-refresh status cache when entering Overclock.
            self._query_overclock_status()

        # If an older session suspended tab children, restore once and keep normal geometry.
        # Suspending CTkScrollableFrame-heavy tabs can corrupt layout after repeated resizes.
        for internal_name in [
            "tab_dashboard",
            "tab_autoscan",
            "tab_overclock",
            "tab_vfcurve",
        ]:
            tab_obj = getattr(self, internal_name, None)
            if tab_obj is None or not hasattr(tab_obj, "_suspended_children"):
                continue
            for child, mgr, info in tab_obj._suspended_children:
                if mgr == "pack":
                    child.pack(**info)
                elif mgr == "grid":
                    child.grid(**info)
                elif mgr == "place":
                    child.place(**info)
            del tab_obj._suspended_children

    def _update_exe_label(self):
        exe = self.config.get("cli_exe_path", "")
        short = os.path.basename(exe) if exe else "Not set"
        self.exe_label.configure(text=f"[{short}]")

    def _set_cli_path(self):
        """Open file dialog to select CLI executable."""
        from tkinter import filedialog

        path = filedialog.askopenfilename(
            title="Select nvoc-auto-optimizer.exe",
            filetypes=[("Executable", "*.exe"), ("All Files", "*.*")],
        )
        if path:
            self.config.set("cli_exe_path", path)
            self.runner.exe_path = path
            # Update cwd
            self.cli_cwd = os.path.dirname(os.path.dirname(path))
            if os.path.basename(self.cli_cwd) == "target":
                self.cli_cwd = os.path.dirname(self.cli_cwd)
            self._update_exe_label()
            self.console.append(f"[GUI] CLI path set to: {path}\n")
            self._refresh_gpu_list()

    def _on_cli_output(self, text: str):
        """Thread-safe callback: schedule append on the main thread."""
        self.after(0, lambda: self.console.append(text))

    def _debounce_tabview_configure(self, event=None):
        """Compatibility shim retained for older experiments; intentionally no-op."""
        return

    def _process_tabview_resize(self):
        return

    def register_resize_target(self, target: Any):
        """Register a tab-like object that supports on_resize_state_changed()."""
        if target is None or target in self._resize_targets:
            return
        self._resize_targets.append(target)

    def _notify_resize_targets(self, resizing: bool, force_flush: bool = False):
        for target in list(self._resize_targets):
            cb = getattr(target, "on_resize_state_changed", None)
            if not callable(cb):
                continue
            try:
                cb(resizing=resizing, force_flush=force_flush)
            except Exception:
                # Resize hooks are best-effort and must never break the UI thread.
                pass

    def _begin_resize_session(self):
        if self._is_resizing:
            return
        self._is_resizing = True
        self._notify_resize_targets(resizing=True)

    def _end_resize_session(self):
        self._resize_settle_after_id = None
        if not self._is_resizing:
            return
        self._is_resizing = False
        self._notify_resize_targets(resizing=False, force_flush=True)

    def _on_root_configure(self, event):
        """Track active drag-resize sessions from root window size changes."""
        if event.widget is not self:
            return
        width = int(event.width)
        if self._last_root_width == width:
            return
        self._last_root_width = width
        self._begin_resize_session()
        if self._resize_settle_after_id:
            try:
                self.after_cancel(self._resize_settle_after_id)
            except Exception:
                pass
        self._resize_settle_after_id = self.after(140, self._end_resize_session)

    def _on_tabview_configure(self, event):
        """Compatibility shim: tabview-level binding is not used in CustomTkinter."""
        return

    def _refresh_gpu_list(self):
        """Run 'list' command and populate GPU dropdown."""
        self.console.append("[GUI] Detecting GPUs...\n")

        def _worker():
            runner = CLIRunner(self.runner.exe_path, on_output=lambda _: None)
            retcode, output = runner.run_sync(["list"], cwd=self.cli_cwd)
            self.after(0, lambda: self._parse_gpu_list(retcode, output))

        threading.Thread(target=_worker, daemon=True).start()

    def _parse_gpu_list(self, retcode: int, output: str):
        """Parse GPU list output and update dropdown."""
        self.console.append(output)
        if retcode != 0:
            self.console.append("[GUI] Failed to detect GPUs.\n")
            self.gpu_dropdown.configure(values=["(detection failed)"])
            self.gpu_var.set("(detection failed)")
            return

        # Parse lines like "GPU 0: NVIDIA GeForce RTX 3060"
        # The CLI outputs multiple lines per GPU (NVML name, UUID, NVAPI bus info, etc.)
        # We only want the first "GPU <index>: <name>" line per unique index,
        # which is typically the NVML name line.
        gpu_pattern = re.compile(r"^GPU\s+(\d+)\s*:\s*(.+)$")
        uuid_pattern = re.compile(r"^UUID=(GPU-[\w-]+)")
        seen_indices = {}  # type: Dict[int, str]  # gpu_index -> display_text
        gpu_names = {}  # type: Dict[int, str]  # gpu_index -> gpu_name
        uuid_map = {}  # type: Dict[int, str]  # gpu_index -> UUID
        last_gpu_idx = None  # type: Optional[int]

        for line in output.strip().split("\n"):
            line = line.strip()
            m = gpu_pattern.match(line)
            if m:
                idx = int(m.group(1))
                name = m.group(2).strip()
                # Capture inline UUID if the CLI prints it on the same GPU line.
                m_inline_uuid = re.search(r"(?i)\buuid\s*[:=]\s*(GPU-[\w-]+)", name)
                if m_inline_uuid:
                    uuid_map[idx] = m_inline_uuid.group(1)
                # Some CLI builds append UUID to the same GPU line; keep selector compact.
                # Accept forms like: "... UUID=GPU-...", "... UUID:GPU-...", "...[GPU-...]".
                name = re.split(r"(?i)\buuid\s*[:=]\s*gpu-[\w-]+", name, maxsplit=1)[
                    0
                ].strip()
                name = re.sub(
                    r"\s*\[\s*GPU-[\w-]+\s*\].*$", "", name, flags=re.IGNORECASE
                )
                last_gpu_idx = idx
                if idx not in seen_indices:
                    # Keep the first match per GPU index (the NVML GPU name)
                    display = f"GPU {idx}: {name}"
                    seen_indices[idx] = display
                    gpu_names[idx] = name  # Store the raw name for capabilities check
            else:
                mu = uuid_pattern.match(line)
                if mu and last_gpu_idx is not None:
                    uuid_map[last_gpu_idx] = mu.group(1)

        # Build ordered list by GPU index
        ordered_indices = sorted(seen_indices.keys())
        short_labels = {}  # type: Dict[int, str]
        long_labels = {}  # type: Dict[int, str]
        for idx in ordered_indices:
            short = seen_indices[idx]
            uuid = uuid_map.get(idx)
            long = f"{short}  [{uuid}]" if uuid else short
            short_labels[idx] = short
            long_labels[idx] = long

        # Use long labels for dropdown entries so users can see full UUID when expanded.
        gpus = [long_labels[i] for i in ordered_indices]

        if gpus:
            # Build reverse map: display_text -> gpu_index
            self.gpu_map = {}
            for idx in ordered_indices:
                self.gpu_map[short_labels[idx]] = idx
                self.gpu_map[long_labels[idx]] = idx

            self._gpu_short_label_by_idx = short_labels
            self._gpu_long_label_by_idx = long_labels

            # Store GPU names for capabilities check
            self.gpu_names = gpu_names

            # Store UUID map
            self.gpu_uuid_map = uuid_map

            self.gpu_dropdown.configure(values=gpus)
            # Try to restore last selection by index first, then by legacy label
            last_idx_raw = str(self.config.get("last_gpu_idx", "")).strip()
            last_idx = int(last_idx_raw) if last_idx_raw.isdigit() else None
            last = self.config.get("last_gpu_id", "")
            if last_idx is None and last in self.gpu_map:
                last_idx = self.gpu_map[last]
            if last_idx is None and last:
                m_last = re.match(r"^GPU\s+(\d+)\s*:", last)
                if m_last:
                    last_idx = int(m_last.group(1))

            if last_idx not in ordered_indices:
                last_idx = ordered_indices[0]

            self._programmatic_gpu_set = True
            try:
                # Keep collapsed selector compact (no UUID), but dropdown values still include UUID.
                self.gpu_var.set(short_labels[last_idx])
            finally:
                self._programmatic_gpu_set = False
            self.console.append(f"[GUI] Found {len(gpus)} GPU(s).\n")
            # Query GPU info for hardware limits
            self._query_gpu_info()
        else:
            self.gpu_dropdown.configure(values=["(no GPUs found)"])
            self._programmatic_gpu_set = True
            try:
                self.gpu_var.set("(no GPUs found)")
            finally:
                self._programmatic_gpu_set = False

    def _on_gpu_changed(self, selected: str):
        """Called by CTkOptionMenu when the user picks a different GPU."""
        if self._programmatic_gpu_set:
            return  # ignore changes triggered by our own gpu_var.set() calls
        if selected.startswith("("):
            return  # placeholder value, nothing to do

        selected_idx = self.gpu_map.get(selected)
        if selected_idx is None:
            m = re.match(r"^GPU\s+(\d+)\s*:", selected)
            selected_idx = int(m.group(1)) if m else None
        if selected_idx is not None and selected_idx in self._gpu_short_label_by_idx:
            short = self._gpu_short_label_by_idx[selected_idx]
            self._programmatic_gpu_set = True
            try:
                self.gpu_var.set(short)
            finally:
                self._programmatic_gpu_set = False
            self.config.set("last_gpu_idx", str(selected_idx))
            self.config.set("last_gpu_id", short)
            self.console.append(f"[GUI] GPU switched to: {short}\n")
        else:
            self.config.set("last_gpu_id", selected)
            self.console.append(f"[GUI] GPU switched to: {selected}\n")

        # Reset dashboard VFP-lock sentinel so first poll after switch always syncs
        if hasattr(self, "tab_dashboard"):
            self.tab_dashboard._last_vfp_lock_mv = object()
        self._dashboard_gpu_lock_active = False
        self._dashboard_mem_lock_active = False
        self._vfp_offset_state_cache = None
        self._gpu_pstates_cache = []
        if self.tab_overclock:
            self.tab_overclock.set_vfp_state(False)
            self.tab_overclock.set_supported_pstates([])

        # Re-run the full init chain: info → limits → status → OC values → curve
        self._query_gpu_info()

    def _query_gpu_info(self):
        """Run 'info' for the selected GPU and parse hardware limits."""
        self.run_gpu_query_async(
            ["info"],
            lambda _retcode, output: self._parse_gpu_info(output),
            thread_name="gpu-info",
        )

    def _parse_gpu_info(self, output: str):
        """Parse 'info' output to extract hardware limits for overclock tab."""
        limits = {}

        # Add cached GPU identity fields to limits for capability checks
        current_gpu_idx = self.get_current_gpu_index()
        if current_gpu_idx is not None and current_gpu_idx in self.gpu_names:
            limits["gpu_name"] = self.gpu_names[current_gpu_idx]
        if current_gpu_idx is not None and current_gpu_idx in self.gpu_arches:
            limits["gpu_architecture"] = self.gpu_arches[current_gpu_idx]

        for line in output.split("\n"):
            line = line.strip()

            # Architecture........: GM200:161 (dGPU)
            if line.startswith("Architecture"):
                parts = line.split(":", 1)
                if len(parts) == 2:
                    arch = parts[1].strip()
                    if arch:
                        limits["gpu_architecture"] = arch
                        if current_gpu_idx is not None:
                            self.gpu_arches[current_gpu_idx] = arch

            # VFP (Graphics)......: -500 MHz ~ 500 MHz
            elif line.startswith("VFP (Graphics)"):
                m = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
                if m:
                    limits["core_clock_min"] = int(m.group(1))
                    limits["core_clock_max"] = int(m.group(2))

            # VFP (Memory)........: -500 MHz ~ 1500 MHz
            elif line.startswith("VFP (Memory)"):
                m = re.search(r"(-?\d+)\s*MHz\s*~\s*(-?\d+)\s*MHz", line)
                if m:
                    limits["mem_clock_min"] = int(m.group(1))
                    limits["mem_clock_max"] = int(m.group(2))

            # Power Limit.........: 58% ~ 124% (100% default) | 100W min / 211W current / 212W max
            elif line.startswith("Power Limit"):
                m = re.search(r"(\d+)%\s*~\s*(\d+)%\s*\((\d+)%\s*default\)", line)
                if m:
                    limits["power_limit_min"] = int(m.group(1))
                    limits["power_limit_max"] = int(m.group(2))
                    limits["power_limit_default"] = int(m.group(3))
                # Parse current percentage: "... | 100W min / 211W current / 212W max"
                # Also try to parse current % directly if present
                mc = re.search(r"\|\s*(\d+)%\s*current", line)
                if mc:
                    limits["power_limit_current"] = int(mc.group(1))
                # Parse absolute watt values: "100W min / 211W current / 212W max"
                mw = re.search(
                    r"(\d+)W\s*min\s*/\s*(\d+)W\s*current\s*/\s*(\d+)W\s*max", line
                )
                if mw:
                    limits["power_watt_min"] = int(mw.group(1))
                    limits["power_watt_current"] = int(mw.group(2))
                    limits["power_watt_max"] = int(mw.group(3))

            # Thermal Limit.......: 65C ~ 90C (83C default)
            elif line.startswith("Thermal Limit"):
                m = re.search(
                    r"(\d+)\s*C\s*~\s*(\d+)\s*C\s*\((\d+)\s*C\s*default\)", line
                )
                if m:
                    limits["thermal_limit_min"] = int(m.group(1))
                    limits["thermal_limit_max"] = int(m.group(2))
                    limits["thermal_limit_default"] = int(m.group(3))
                # Parse current thermal limit if present: e.g. "... | 87C current"
                mc = re.search(r"\|\s*(\d+)\s*C\s*current", line)
                if mc:
                    limits["thermal_limit_current"] = int(mc.group(1))

            # Overvolt P0.........: 0 mV (range: -1018.461 mV - 256.019 mV)
            elif line.startswith("Overvolt"):
                m = re.search(
                    r"^Overvolt\s+(P\d+).*?:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\(\s*range\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*-\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\)",
                    line,
                    re.IGNORECASE,
                )
                if m:
                    pstate = m.group(1).upper()
                    current_mv = self._voltage_text_to_mv(m.group(2), m.group(3))
                    min_mv = self._voltage_text_to_mv(m.group(4), m.group(5))
                    max_mv = self._voltage_text_to_mv(m.group(6), m.group(7))
                    # Prefer P0 when multiple Overvolt rows exist.
                    if limits.get("legacy_overvolt_pstate") == "P0" and pstate != "P0":
                        continue
                    limits["legacy_overvolt_pstate"] = pstate
                    limits["legacy_overvolt_current_mv"] = current_mv
                    limits["legacy_overvolt_min_mv"] = min_mv
                    limits["legacy_overvolt_max_mv"] = max_mv

        if limits:
            self.console.append(f"[GUI] GPU limits: {limits}\n")
            # Store limits so _apply_initial_status can merge current OC values.
            # NOTE: No race condition here — _query_initial_status() is called
            # synchronously at the end of this callback (which itself runs on the
            # main thread via after(0,...)).  The 'status' worker thread is only
            # spawned *after* _gpu_limits_cache is fully populated, so
            # _apply_initial_status always sees a complete cache.
            self._gpu_limits_cache = limits
            if self.tab_overclock:
                self.tab_overclock.check_capabilities(limits)
                self.tab_overclock.update_limits(limits)
            # Expose power watt info for dashboard
            if "power_watt_max" in limits:
                self.gpu_power_watt_max: int = limits["power_watt_max"]
                self.gpu_power_watt_min: int = limits.get("power_watt_min", 0)
        else:
            self.console.append("[GUI] Warning: 'info' returned no parseable limits.\n")

        # Always query status after info, regardless of parse result.
        # _apply_initial_status will merge into whatever _gpu_limits_cache contains.
        self._query_initial_status()
        self._query_gpu_get()

    def _query_initial_status(self):
        """Run 'status' once at startup to detect VFP lock, then refresh VF curve."""
        self.run_gpu_query_async(
            ["status"], self._apply_initial_status, thread_name="init-status"
        )

    def _query_gpu_get(self):
        """Run 'get' once to capture supported P-States and related OC capabilities."""
        self.run_gpu_query_async(["get"], self._apply_gpu_get, thread_name="gpu-get")

    def _query_overclock_status(self):
        """Run 'status' and refresh Overclock current values from latest cache."""
        self.run_gpu_query_async(
            ["status"], self._apply_overclock_status, thread_name="overclock-status"
        )

    @staticmethod
    def _voltage_text_to_mv(value_text: str, unit_text: str) -> int:
        """Convert a voltage text pair (value + unit) into integer mV."""
        value = float(value_text)
        unit = unit_text.lower()
        if unit in {"uv", "µv", "μv"}:
            return int(round(value / 1000.0))
        return int(round(value))

    @staticmethod
    def _parse_status_current_values(
        output: str,
    ) -> Tuple[Optional[float], Dict[str, Any]]:
        """Extract lock voltage and current OC/limit values from CLI 'status' output."""
        locked_voltage_mv = None  # type: Optional[float]
        limits_update = {}  # type: Dict[str, Any]

        for line in output.splitlines():
            line_s = line.strip()

            if re.search(r"vfp lock", line_s, re.IGNORECASE):
                m = re.search(
                    r"voltage\s*:\s*(\d+(?:\.\d+)?)\s*mv", line_s, re.IGNORECASE
                )
                if m:
                    locked_voltage_mv = float(m.group(1))
            elif re.search(
                r"Graphics.*Offset|OC\s*\(Graphics\)", line_s, re.IGNORECASE
            ):
                m = re.search(r"([+-]?\d+)\s*MHz", line_s)
                if m:
                    limits_update["core_clock_current"] = int(m.group(1))
            elif re.search(r"Memory.*Offset|OC\s*\(Memory\)", line_s, re.IGNORECASE):
                m = re.search(r"([+-]?\d+)\s*MHz", line_s)
                if m:
                    limits_update["mem_clock_current"] = int(m.group(1))
            elif re.search(r"power limit", line_s, re.IGNORECASE):
                m = re.search(r"([+-]?\d+)\s*%", line_s)
                if m:
                    limits_update["power_limit_current"] = int(m.group(1))
            elif re.search(r"thermal limit", line_s, re.IGNORECASE):
                m = re.search(r"(\d+)\s*[Cc]", line_s)
                if m:
                    limits_update["thermal_limit_current"] = int(m.group(1))
            elif re.search(r"voltage boost", line_s, re.IGNORECASE):
                m = re.search(r"([+-]?\d+)\s*%", line_s)
                if m:
                    limits_update["voltage_boost_current"] = int(m.group(1))

        return locked_voltage_mv, limits_update

    def _apply_overclock_status(self, retcode: int, output: str):
        """Apply status-derived current values to cache and Overclock controls."""
        if retcode != 0:
            self.console.append(
                "[GUI] Warning: failed to refresh overclock status cache.\n"
            )
            return

        _, limits_update = self._parse_status_current_values(output)
        if not limits_update:
            return

        merged = {**getattr(self, "_gpu_limits_cache", {}), **limits_update}
        self._gpu_limits_cache = merged
        if self.tab_overclock:
            self.tab_overclock.update_limits(merged)

    @staticmethod
    def _parse_supported_pstates(output: str) -> List[str]:
        """Extract ordered NVML P-State labels from CLI 'get' output."""
        match = re.search(
            r"Supported P-States:\s*(.*?)(?:\n\s*Supported Applications Clocks:|\Z)",
            output,
            re.IGNORECASE | re.DOTALL,
        )
        if not match:
            return []

        seen = set()  # type: Set[str]
        pstates = []  # type: List[str]
        for raw in match.group(1).splitlines():
            line = raw.strip()
            state_match = re.match(r"^P\s*(\d+)\s*:", line, re.IGNORECASE)
            if not state_match:
                continue
            label = f"P{int(state_match.group(1))}"
            if label in seen:
                continue
            seen.add(label)
            pstates.append(label)
        return pstates

    @staticmethod
    def _parse_nvml_power_limits_from_get(output: str) -> Dict[str, int]:
        """Extract NVML power limit values (W) from CLI 'get' output."""
        match = re.search(
            r"^\s*Power\s+Limit\s*:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*\(\s*Min:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*-\s*Max:\s*([0-9]+(?:\.[0-9]+)?)\s*W\s*\)",
            output,
            re.IGNORECASE | re.MULTILINE,
        )
        if not match:
            return {}

        return {
            "power_limit_nvml_current_w": int(round(float(match.group(1)))),
            "power_limit_nvml_min_w": int(round(float(match.group(2)))),
            "power_limit_nvml_max_w": int(round(float(match.group(3)))),
        }

    @staticmethod
    def _parse_nvapi_power_current_from_get(output: str) -> Dict[str, int]:
        """Extract NVAPI power limit current (%) from CLI 'get' output."""
        match = re.search(
            r"^\s*Power\s+Limit\.*\s*:\s*([+-]?\d+)\s*%",
            output,
            re.IGNORECASE | re.MULTILINE,
        )
        if not match:
            return {}
        return {"power_limit_current": int(match.group(1))}

    def _apply_gpu_get(self, retcode: int, output: str):
        """Merge supported P-State data from CLI 'get' into the cached GPU state."""
        merged = dict(getattr(self, "_gpu_limits_cache", {}))
        if retcode != 0:
            self.console.append(
                "[GUI] Warning: failed to query supported P-States via 'get'.\n"
            )
            self._gpu_pstates_cache = []
            merged["supported_pstates"] = []
            self._gpu_limits_cache = merged
            self._sync_dashboard_lock_state_from_cache(merged)
            if self.tab_overclock:
                self.tab_overclock.update_limits(merged)
            if self.tab_vfcurve:
                self.tab_vfcurve.sync_freq_locks_from_cache(merged)
            return

        pstates = self._parse_supported_pstates(output)
        self._gpu_pstates_cache = pstates

        for key in (
            "vfp_lock_gpu_core_upperbound_mhz",
            "vfp_lock_gpu_core_lowerbound_mhz",
            "vfp_lock_memory_upperbound_mhz",
            "vfp_lock_memory_lowerbound_mhz",
        ):
            merged.pop(key, None)
        merged.update(self._parse_vfp_lock_bounds(output))

        legacy_bounds = self._parse_legacy_overvolt_bounds(output)
        if legacy_bounds:
            for key in (
                "legacy_overvolt_pstate",
                "legacy_overvolt_current_mv",
                "legacy_overvolt_min_mv",
                "legacy_overvolt_max_mv",
            ):
                merged.pop(key, None)
            merged.update(legacy_bounds)

        nvml_power = self._parse_nvml_power_limits_from_get(output)
        if nvml_power:
            for key in (
                "power_limit_nvml_current_w",
                "power_limit_nvml_min_w",
                "power_limit_nvml_max_w",
            ):
                merged.pop(key, None)
            merged.update(nvml_power)

        nvapi_power = self._parse_nvapi_power_current_from_get(output)
        if nvapi_power:
            merged["power_limit_current"] = nvapi_power["power_limit_current"]

        merged["supported_pstates"] = pstates
        self._gpu_limits_cache = merged
        self._sync_dashboard_lock_state_from_cache(merged)

        if pstates:
            self.console.append(f"[GUI] Supported P-States: {', '.join(pstates)}\n")
        else:
            self.console.append(
                "[GUI] Warning: 'get' returned no parseable supported P-States.\n"
            )

        if self.tab_overclock:
            self.tab_overclock.update_limits(merged)
        if self.tab_vfcurve:
            self.tab_vfcurve.sync_freq_locks_from_cache(merged)

    def _sync_dashboard_lock_state_from_cache(
        self, limits: Optional[Dict[str, Any]] = None
    ) -> None:
        """Update dashboard GPU/MEM lock indicators from cached 'get' lock bounds."""
        source = (
            limits if limits is not None else getattr(self, "_gpu_limits_cache", {})
        )
        source = source or {}

        def _has_pair(lo_key: str, hi_key: str) -> bool:
            lo = source.get(lo_key)
            hi = source.get(hi_key)
            try:
                return (
                    lo is not None
                    and hi is not None
                    and str(lo).strip() != ""
                    and str(hi).strip() != ""
                )
            except Exception:
                return False

        self._dashboard_gpu_lock_active = _has_pair(
            "vfp_lock_gpu_core_lowerbound_mhz",
            "vfp_lock_gpu_core_upperbound_mhz",
        )
        self._dashboard_mem_lock_active = _has_pair(
            "vfp_lock_memory_lowerbound_mhz",
            "vfp_lock_memory_upperbound_mhz",
        )

    @staticmethod
    def _parse_vfp_lock_bounds(output: str) -> Dict[str, int]:
        """Parse VFP lock bounds (core/memory) from CLI 'get' output."""
        bounds = {}  # type: Dict[str, int]
        patterns = {
            "vfp_lock_gpu_core_upperbound_mhz": r"^\s*VFP\s+Lock\s+GPU\s+Core\s+Upperbound\s*:\s*([+-]?\d+)\s*MHz\b",
            "vfp_lock_gpu_core_lowerbound_mhz": r"^\s*VFP\s+Lock\s+GPU\s+Core\s+Lowerbound\s*:\s*([+-]?\d+)\s*MHz\b",
            "vfp_lock_memory_upperbound_mhz": r"^\s*VFP\s+Lock\s+Memory\s+Upperbound\s*:\s*([+-]?\d+)\s*MHz\b",
            "vfp_lock_memory_lowerbound_mhz": r"^\s*VFP\s+Lock\s+Memory\s+Lowerbound\s*:\s*([+-]?\d+)\s*MHz\b",
        }

        for key, pattern in patterns.items():
            m = re.search(pattern, output, re.IGNORECASE | re.MULTILINE)
            if not m:
                continue
            bounds[key] = int(m.group(1))

        return bounds

    @staticmethod
    def _parse_legacy_overvolt_bounds(output: str) -> Dict[str, int | str]:
        """Parse legacy Overvolt rows from CLI 'get' output and normalize to mV."""
        pattern = re.compile(
            r"^\s*Overvolt\s+(P\d+)\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\(\s*range\s*:\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*-\s*([+-]?\d+(?:\.\d+)?)\s*([mu\u00b5\u03bc]V)\s*\)",
            re.IGNORECASE,
        )

        rows: list[tuple[str, int, int, int]] = []
        for line in output.splitlines():
            m = pattern.match(line.strip())
            if not m:
                continue
            rows.append(
                (
                    m.group(1).upper(),
                    App._voltage_text_to_mv(m.group(2), m.group(3)),
                    App._voltage_text_to_mv(m.group(4), m.group(5)),
                    App._voltage_text_to_mv(m.group(6), m.group(7)),
                )
            )

        if not rows:
            return {}

        selected = next((row for row in rows if row[0] == "P0"), rows[0])
        return {
            "legacy_overvolt_pstate": selected[0],
            "legacy_overvolt_current_mv": selected[1],
            "legacy_overvolt_min_mv": selected[2],
            "legacy_overvolt_max_mv": selected[3],
        }

    def _apply_initial_status(self, retcode: int, output: str):
        """Parse VFP Lock and current OC settings from 'status' output, sync all tabs."""
        locked_voltage_mv = None  # type: Optional[float]

        if retcode == 0:
            locked_voltage_mv, limits_update = self._parse_status_current_values(output)

            # Merge parsed current values with cached limits and push to overclock tab
            if limits_update:
                cached = getattr(self, "_gpu_limits_cache", {})
                merged = {**cached, **limits_update}
                self._gpu_limits_cache = merged  # update cache for lazy load
                # Ensure check_capabilities is called with gpu_name for proper capability detection
                if "gpu_name" in merged and self.tab_overclock:
                    self.tab_overclock.check_capabilities(merged)
                self.console.append(
                    f"[GUI] OC current values from status: {limits_update}\n"
                )
                if self.tab_overclock:
                    self.tab_overclock.update_limits(merged)

        if locked_voltage_mv is not None:
            self.console.append(
                f"[GUI] VFP lock detected at {locked_voltage_mv:.2f} mV syncing...\n"
            )
        else:
            self.console.append("[GUI] VFP lock: None\n")

        # Sync lock state into vfcurve tab, then trigger a full curve refresh
        self._locked_voltage_mv_cache = locked_voltage_mv
        if self.tab_vfcurve:
            self.tab_vfcurve.sync_lock_from_voltage(locked_voltage_mv)
            self.tab_vfcurve._refresh_curve()
        else:
            self.refresh_vfp_offset_state()

    def _get_vfp_cache_path(self) -> str:
        """Return the VFP CSV cache path for the current GPU."""
        app_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        cache_dir = os.path.join(app_dir, VFCurveTab._EXPORT_DIR)
        os.makedirs(cache_dir, exist_ok=True)

        uuid = self.get_current_gpu_uuid()
        if uuid:
            fname = f"{uuid}.csv"
        else:
            idx = self.get_current_gpu_index()
            fname = f"gpu_{idx if idx is not None else 0}.csv"
        return os.path.join(cache_dir, fname)

    @staticmethod
    def _analyze_vfp_offsets(
        frequencies: List[float], defaults: List[float]
    ) -> Tuple[bool, Optional[int]]:
        """Return (has_any_offset, uniform_core_offset_mhz_if_flat_curve)."""
        if (not frequencies) or (len(frequencies) != len(defaults)):
            return False, None

        eps = 1e-4
        offsets = [freq - default for freq, default in zip(frequencies, defaults)]
        if all(abs(offset - offsets[0]) <= eps for offset in offsets):
            if abs(offsets[0]) <= eps:
                return False, None
            return True, int(round(offsets[0]))
        return any(abs(offset) > eps for offset in offsets), None

    @staticmethod
    def _get_vfp_offset_state_from_csv(
        csv_path: str,
    ) -> Optional[Tuple[bool, Optional[int]]]:
        """Read a VF export and return (has_any_offset, uniform_core_offset_mhz_if_flat_curve)."""
        if not os.path.isfile(csv_path):
            return None

        frequencies = []  # type: List[float]
        defaults = []  # type: List[float]
        try:
            with open(csv_path, newline="", encoding="utf-8-sig") as f:
                reader = csv.reader(f)
                for row in reader:
                    if not row or row[0].startswith("#"):
                        continue
                    if row[0].strip().lower() in {"voltage_uv", "voltage", "uv"}:
                        continue
                    try:
                        freq = float(row[1]) / 1000.0  # kHz -> MHz
                        default = (
                            float(row[3]) / 1000.0 if len(row) > 3 else freq
                        )  # kHz -> MHz
                    except (ValueError, IndexError):
                        continue
                    frequencies.append(freq)
                    defaults.append(default)
        except Exception:
            return None

        return App._analyze_vfp_offsets(frequencies, defaults)

    def _apply_vfp_offset_state(
        self, has_vfp_offset: bool, uniform_core_offset_mhz: Optional[int] = None
    ):
        """Cache and push the current VF curve offset state into interested tabs."""
        self._vfp_offset_state_cache = (has_vfp_offset, uniform_core_offset_mhz)
        if self.tab_overclock:
            self.tab_overclock.set_vfp_state(has_vfp_offset, uniform_core_offset_mhz)

    def _get_vfp_offset_gpu_key(self) -> str:
        """Return a stable key for the currently selected GPU."""
        uuid = self.get_current_gpu_uuid()
        if uuid:
            return uuid
        idx = self.get_current_gpu_index()
        return f"gpu_{idx if idx is not None else 0}"

    def refresh_vfp_offset_state(self, force: bool = False):
        """Refresh pointwise VF offset state without requiring the VF Curve tab UI."""
        if self.tab_vfcurve is not None:
            return
        if self._vfp_offset_refresh_inflight:
            self._pending_vfp_offset_refresh = True
            return
        if (not force) and self._vfp_offset_state_cache is not None:
            self._apply_vfp_offset_state(*self._vfp_offset_state_cache)
            return

        gpu_args = self.get_gpu_args()
        if not gpu_args or not self.runner.exe_path:
            return

        csv_path = self._get_vfp_cache_path()
        self._vfp_offset_refresh_inflight = True
        self._pending_vfp_offset_refresh = False
        gpu_key = self._get_vfp_offset_gpu_key()

        def _worker():
            runner = CLIRunner(self.runner.exe_path, on_output=lambda _: None)
            retcode, _output = runner.run_sync(
                gpu_args + ["set", "vfp", "export", "-q", csv_path],
                cwd=self.cli_cwd,
            )
            vfp_offset_state = None
            if retcode == 0:
                vfp_offset_state = self._get_vfp_offset_state_from_csv(csv_path)
            self.after(
                0,
                lambda: self._on_vfp_offset_refresh_done(
                    gpu_key, retcode, vfp_offset_state
                ),
            )

        threading.Thread(target=_worker, daemon=True, name="detect-vfp-offset").start()

    def _on_vfp_offset_refresh_done(
        self,
        gpu_key: str,
        retcode: int,
        vfp_offset_state: Optional[Tuple[bool, Optional[int]]],
    ) -> None:
        """Finalize a lightweight VF offset refresh."""
        self._vfp_offset_refresh_inflight = False
        rerun = self._pending_vfp_offset_refresh and self.tab_vfcurve is None
        self._pending_vfp_offset_refresh = False

        if (
            gpu_key == self._get_vfp_offset_gpu_key()
            and retcode == 0
            and vfp_offset_state is not None
        ):
            self._apply_vfp_offset_state(*vfp_offset_state)

        if rerun:
            self.refresh_vfp_offset_state(force=True)

    def get_gpu_args(self) -> List[str]:
        """Get the --gpu=ID argument list for the currently selected GPU."""
        gpu_text = self.gpu_var.get()
        if gpu_text.startswith("("):
            return []

        # Use the gpu_map built during detection
        if gpu_text in self.gpu_map:
            return [f"--gpu={self.gpu_map[gpu_text]}"]

        # Fallback: try to extract GPU index from "GPU N: ..." format
        m = re.match(r"^GPU\s+(\d+)\s*:", gpu_text)
        if m:
            return [f"--gpu={m.group(1)}"]

        # If we can't parse it, don't pass --gpu
        return []

    def get_current_gpu_index(self) -> Optional[int]:
        """Return the integer index of the currently selected GPU, or None."""
        gpu_text = self.gpu_var.get()
        if gpu_text in self.gpu_map:
            return self.gpu_map[gpu_text]
        m = re.match(r"^GPU\s+(\d+)\s*:", gpu_text)
        if m:
            return int(m.group(1))
        return None

    def get_current_gpu_uuid(self) -> Optional[str]:
        """Return the UUID string of the currently selected GPU, or None."""
        idx = self.get_current_gpu_index()
        if idx is not None:
            return self.gpu_uuid_map.get(idx)
        return None

    def show_gpu_command(self, command_args: List[str]) -> None:
        """Run a GPU-scoped CLI command and stream it to the console."""
        gpu_args = self.get_gpu_args()
        if gpu_args:
            self.run_cli_display(gpu_args + command_args)

    def run_gpu_query_async(
        self,
        command_args: List[str],
        callback: Callable[[int, str], None],
        thread_name: str = "gpu-query",
    ) -> bool:
        """Run a GPU-scoped CLI query asynchronously and return whether it started."""
        gpu_args = self.get_gpu_args()
        if not gpu_args:
            return False

        def _worker():
            runner = CLIRunner(self.runner.exe_path, on_output=lambda _: None)
            retcode, output = runner.run_sync(gpu_args + command_args, cwd=self.cli_cwd)
            self.after(0, lambda: callback(retcode, output))

        threading.Thread(target=_worker, daemon=True, name=thread_name).start()
        return True

    def run_cli_display(
        self, args: List[str], on_finished: Optional[Callable[[int], None]] = None
    ) -> None:
        """Run CLI command and stream output to the console.

        This helper now accepts an optional completion callback for command chaining.
        """
        self.runner.on_finished = on_finished
        self.runner.run(args, cwd=self.cli_cwd)

    def run_cli(
        self, args: List[str], on_finished: Optional[Callable[[int], None]] = None
    ) -> None:
        """Run CLI command with custom on_finished callback."""
        self.runner.on_finished = on_finished
        self.runner.run(args, cwd=self.cli_cwd)

    def cancel_cli(self):
        """Cancel the running CLI process."""
        self.runner.cancel()

    # ── System Tray ──────────────────────────────────────────────────────────

    def _get_tray_image(self) -> Image.Image:
        """Return a PIL Image for the tray icon.

        Priority:
          1. Same .ico file that PyInstaller uses (next to exe or frozen root).
          2. Any .ico / .png in the app directory.
          3. Programmatically generated NVOC icon as fallback.
        """
        if self._tray_image is not None:
            return self._tray_image

        # Determine the base directory (works both frozen and dev)
        if getattr(sys, "frozen", False):
            base = sys._MEIPASS  # type: ignore[attr-defined]
            exe_dir = os.path.dirname(sys.executable)
        else:
            base = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
            exe_dir = base

        # Candidate icon files in preferred order
        candidates = []
        for d in (exe_dir, base):
            for name in (
                "NVOC-GUI.ico",
                "nvoc_gui.ico",
                "icon.ico",
                "NVOC-GUI.png",
                "nvoc_gui.png",
                "icon.png",
            ):
                candidates.append(os.path.join(d, name))

        for path in candidates:
            if os.path.isfile(path):
                try:
                    img = Image.open(path).convert("RGBA")
                    img = img.resize((64, 64), Image.LANCZOS)
                    self._tray_image = img
                    return img
                except Exception:
                    pass

        # Fallback: generate a simple green "N" icon
        img = Image.new("RGBA", (64, 64), (30, 30, 30, 255))
        # Draw a simple styled rectangle as background
        from PIL import ImageDraw, ImageFont

        draw = ImageDraw.Draw(img)
        draw.rounded_rectangle([4, 4, 60, 60], radius=10, fill=(0, 120, 60, 255))
        try:
            font = ImageFont.truetype("arialbd.ttf", 36)
        except Exception:
            font = ImageFont.load_default()
        draw.text((18, 10), "N", font=font, fill=(255, 255, 255, 255))
        self._tray_image = img
        return img

    def _build_tray_icon(self) -> "pystray.Icon":
        """Create and return a new pystray.Icon instance."""
        menu = pystray.Menu(
            pystray.MenuItem("显示主界面", self._show_from_tray, default=True),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("退出", self._quit_app),
        )
        return pystray.Icon("NVOC-GUI", self._get_tray_image(), "NVOC-GUI", menu)

    def _on_unmap(self, event):
        """Called when the window is minimized (iconified)."""
        # Only act on the root window itself, not child widgets
        if event.widget is self:
            self.after(50, self._check_minimized)

    def _check_minimized(self):
        """Hide to tray if the window is currently in iconic (minimized) state."""
        if self.state() == "iconic":
            self._hide_to_tray()

    def _hide_to_tray(self):
        """Hide the main window and show the tray icon."""
        self.withdraw()
        # (Re)create tray icon each time so pystray state is clean
        if self._tray_icon is not None:
            try:
                self._tray_icon.stop()
            except Exception:
                pass
        self._tray_icon = self._build_tray_icon()
        self._tray_thread = threading.Thread(
            target=self._tray_icon.run, daemon=True, name="tray-icon"
        )
        self._tray_thread.start()

    def _show_from_tray(self, icon=None, item=None):
        """Restore the main window from the tray."""
        if self._tray_icon is not None:
            self._tray_icon.stop()
            self._tray_icon = None
        # Schedule on main thread
        self.after(0, self._restore_window)

    def _restore_window(self):
        self.deiconify()
        self.state("normal")
        self.update_idletasks()
        self.lift()
        self.attributes("-topmost", True)
        self.after(150, lambda: self.attributes("-topmost", self.is_topmost))
        self.focus_force()

    def _poll_single_instance_signal(self):
        """Restore the running instance when a duplicate launch requests it."""
        try:
            if (
                self._single_instance_guard
                and self._single_instance_guard.consume_activation_request()
            ):
                self._activate_from_second_launch()
        finally:
            if self.winfo_exists():
                self.after(200, self._poll_single_instance_signal)

    def _activate_from_second_launch(self):
        """Bring the main window back to the foreground and flash the taskbar icon."""
        if self._tray_icon is not None:
            self._tray_icon.stop()
            self._tray_icon = None
        self._restore_window()
        self.after(120, self._flash_taskbar_icon_twice)

    def _flash_taskbar_icon_twice(self):
        """Flash the window caption/taskbar icon twice on Windows."""
        if sys.platform != "win32":
            return

        class FLASHWINFO(ctypes.Structure):
            _fields_ = [
                ("cbSize", ctypes.c_uint),
                ("hwnd", ctypes.c_void_p),
                ("dwFlags", ctypes.c_uint),
                ("uCount", ctypes.c_uint),
                ("dwTimeout", ctypes.c_uint),
            ]

        FLASHW_CAPTION = 0x00000001
        FLASHW_TRAY = 0x00000002

        flash_info = FLASHWINFO(
            cbSize=ctypes.sizeof(FLASHWINFO),
            hwnd=self.winfo_id(),
            dwFlags=FLASHW_CAPTION | FLASHW_TRAY,
            uCount=2,
            dwTimeout=0,
        )
        try:
            ctypes.windll.user32.FlashWindowEx(ctypes.byref(flash_info))
        except Exception:
            pass

    def _quit_app(self, icon=None, item=None):
        """Fully exit the application from the tray menu."""
        if self._tray_icon is not None:
            self._tray_icon.stop()
            self._tray_icon = None
        self.after(0, self.destroy)
