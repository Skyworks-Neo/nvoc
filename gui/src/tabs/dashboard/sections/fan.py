"""Fan Control pane - compact full-width section for the dashboard.

Layout:
  row 1 : [🌀 Fan Control] [All/Fan dropdown] [0-30-50-70-100 node slider + entry%] ...[NVAPI/NVML]
  row 2 : [Policy: [menu]] (1/3)      [✅ Apply] (1/3)      [🔄 Reset] (1/3)
"""

from __future__ import annotations

import tkinter as tk
from typing import TYPE_CHECKING, Callable, Optional, Protocol, Sequence

import customtkinter as ctk

from src.backend.base import FanSettings, GuiBackend
from src.widgets.lightweight_controls import (
    ct_button_font,
    LiteButton,
    LiteEntry,
)

if TYPE_CHECKING:
    from src.backend.cli import CliBackend

# De-CTk'd inner-panel palette (matches overclock.py / CTk dark theme)
_PANE_BG = "#2b2b2b"
_TEXT_FG = "#e5e5e5"
_TEXT_FG_DIM = "#b3b3b3"
_FONT_BODY = ("Segoe UI", 11)
_FONT_HEADER = ("Segoe UI", 13, "bold")
_SECTION_BORDER = "#1f4e79"


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


class FanLevelSelector(tk.Frame):
    """Fan level control: 5 snap nodes (0/30/50/70/100) on a track.

    - Click/drag snaps to the nearest node (PState/D-selector styling).
    - Mouse wheel adjusts steplessly (+/- 1%) across 0..100.
    - Value is mirrored to ``variable`` (the % entry); external writes to
      the variable redraw the handle.
    """

    _NODES = (0, 30, 50, 70, 100)
    _NODE_COLORS = ("#44cc88", "#9acd32", "#f9a825", "#ff8c00", "#e53935")
    _TRACK_Y = 14
    _NODE_R = 9
    _HANDLE_R = 8
    _PAD_X = 34

    def __init__(
        self,
        master,
        variable: ctk.StringVar,
        command: Optional[Callable[[int], None]] = None,
        bg: str = _PANE_BG,
    ):
        super().__init__(master, bg=bg, bd=0, highlightthickness=0)
        self._variable = variable
        self._command = command
        self._bg = bg
        self._value = 60
        self._syncing = False
        self._active = True

        self._canvas = tk.Canvas(
            self, width=1, height=48, highlightthickness=0, bd=0, bg=bg
        )
        self._canvas.pack(fill="both", expand=True)
        self._canvas.bind("<Button-1>", self._on_click)
        self._canvas.bind("<B1-Motion>", self._on_drag)
        self._canvas.bind("<MouseWheel>", self._on_wheel)
        self._canvas.bind("<Button-4>", lambda e: self._step(-1))
        self._canvas.bind("<Button-5>", lambda e: self._step(1))
        self._canvas.bind("<Configure>", lambda _e: self._redraw())
        variable.trace_add("write", lambda *_: self._on_var())

    # ── geometry helpers ──────────────────────────────────────────────
    def _track(self):
        w = max(1, self._canvas.winfo_width())
        x0 = self._PAD_X
        x1 = max(x0 + 1, w - self._PAD_X)
        return x0, x1

    def _value_to_x(self, value: float) -> float:
        x0, x1 = self._track()
        return x0 + (value / 100.0) * (x1 - x0)

    def _x_to_value(self, x: float) -> float:
        x0, x1 = self._track()
        ratio = max(0.0, min(1.0, (x - x0) / max(1e-9, (x1 - x0))))
        return ratio * 100.0

    # ── public API (CanvasSlider-compatible subset) ───────────────────
    def get(self) -> float:
        return float(self._value)

    def set(self, value) -> None:
        v = int(max(0, min(100, round(float(value)))))
        if v == self._value:
            return
        self._value = v
        self._redraw()

    def configure(self, **kwargs):
        state = kwargs.pop("state", None)
        if state is not None:
            self._state = state
        self._redraw()

    def set_active(self, active: bool) -> None:
        """Unsupported GPUs hide the handle (no misleading position)."""
        if active != self._active:
            self._active = active
            self._redraw()

    _state = "normal"

    # ── interaction ───────────────────────────────────────────────────
    def _on_click(self, event):
        """Click snaps to the nearest preset node."""
        if getattr(self, "_state", "normal") == "disabled":
            return
        value = self._x_to_value(float(event.x))
        node = min(self._NODES, key=lambda n: abs(n - value))
        self._commit(node)

    def _on_drag(self, event):
        """Dragging is stepless — snapping only happens on click."""
        if getattr(self, "_state", "normal") == "disabled":
            return
        self._commit(int(round(self._x_to_value(float(event.x)))))

    def _on_wheel(self, event):
        if getattr(self, "_state", "normal") == "disabled":
            return
        delta = int(getattr(event, "delta", 0) or 0)
        if delta == 0:
            return
        self._step(-1 if delta > 0 else 1)

    def _step(self, direction: int):
        if getattr(self, "_state", "normal") == "disabled":
            return
        self._commit(int(max(0, min(100, self._value + direction))))

    def _commit(self, value: int):
        value = int(max(0, min(100, value)))
        if value == self._value:
            return
        self._syncing = True
        try:
            self._value = value
            self._variable.set(str(value))
            if callable(self._command):
                self._command(value)
        finally:
            self._syncing = False
        self._redraw()

    def _on_var(self):
        if self._syncing:
            return
        try:
            value = int(float(self._variable.get().strip()))
        except ValueError:
            return
        value = int(max(0, min(100, value)))
        if value != self._value:
            self._value = value
            self._redraw()

    # ── drawing ───────────────────────────────────────────────────────
    def _redraw(self):
        c = self._canvas
        c.delete("all")
        x0, x1 = self._track()

        c.create_line(
            x0,
            self._TRACK_Y,
            x1,
            self._TRACK_Y,
            fill="#3a3a3a",
            width=4,
            capstyle=tk.ROUND,
        )

        for node, color in zip(self._NODES, self._NODE_COLORS):
            nx = self._value_to_x(node)
            c.create_oval(
                nx - self._NODE_R,
                self._TRACK_Y - self._NODE_R,
                nx + self._NODE_R,
                self._TRACK_Y + self._NODE_R,
                fill=color,
                outline="",
            )
            c.create_text(
                nx,
                self._TRACK_Y + 22,
                text=str(node),
                fill="#7e8da1",
                font=("Segoe UI", 8, "bold"),
            )

        # current-value handle (PState/D handle styling) — hidden when the
        # fan control is unsupported so no default position misleads.
        if self._active:
            hx = self._value_to_x(self._value)
            c.create_oval(
                hx - self._HANDLE_R,
                self._TRACK_Y - self._HANDLE_R,
                hx + self._HANDLE_R,
                self._TRACK_Y + self._HANDLE_R,
                fill="#f5f7fb",
                outline="#59b0ff",
                width=2,
            )


class FanControlPane:
    """Compact fan-control section (dashboard integration)."""

    def __init__(
        self,
        parent: ctk.CTkFrame,
        backend: "CliBackend",
        embedded: bool = True,
        content_parent: ctk.CTkFrame | None = None,
    ) -> None:
        self.frame = parent
        self.controller = FanControlController(self, backend)
        self._interactive_widgets: list[object] = []
        self._supported_state = True

        host = content_parent if content_parent is not None else parent
        self._build_content(host)

    # ── UI ────────────────────────────────────────────────────────────────
    def _build_content(self, host: ctk.CTkFrame) -> None:
        section = ctk.CTkFrame(
            host, border_width=1, border_color=_SECTION_BORDER, corner_radius=10
        )
        section.pack(fill="x", pady=(0, 10))

        # ── One shared grid, two rows x (third | separator | third | separator | third).
        # Hard vertical separators enforce the 1/3 alignment visually.
        grid = tk.Frame(section, bg=_PANE_BG)
        grid.pack(fill="x", padx=12, pady=(10, 10))
        for col in (0, 2, 4):
            grid.columnconfigure(col, weight=1, uniform="fan_thirds")
        for col in (1, 3):
            grid.columnconfigure(col, weight=0)
        for sep_col in (1, 3):
            tk.Frame(
                grid, width=1, height=84, bg="#3f3f3f", bd=0, highlightthickness=0
            ).grid(row=0, column=sep_col, rowspan=2, sticky="ns", padx=8)

        # Row 0, left third: title + fan ID
        r0_left = tk.Frame(grid, bg=_PANE_BG)
        r0_left.grid(row=0, column=0, sticky="ew")
        self.cooler_title = tk.Label(
            r0_left,
            text="🌀 Fan Control",
            font=_FONT_HEADER,
            bg=_PANE_BG,
            fg=_TEXT_FG,
        )
        self.cooler_title.pack(side="left")
        self.fan_id_var = ctk.StringVar(value="All")
        self.fan_id_menu = ctk.CTkOptionMenu(
            r0_left,
            variable=self.fan_id_var,
            values=["All", "Fan 1", "Fan 2"],
            width=84,
            anchor="center",
            font=ct_button_font(r0_left),
        )
        # right-aligned to the 1/3 boundary (matches the Policy dropdown edge)
        self.fan_id_menu.pack(side="right")
        self._interactive_widgets.append(self.fan_id_menu)

        # Row 0, middle third: level slider + entry%
        self.level_var = ctk.StringVar(value="60")
        r0_mid = tk.Frame(grid, bg=_PANE_BG)
        r0_mid.grid(row=0, column=2, sticky="ew")
        self.level_slider = FanLevelSelector(
            r0_mid,
            self.level_var,
            command=self.controller.on_slider_change,
        )
        self.level_slider.pack(side="left", fill="x", expand=True)
        self._interactive_widgets.append(self.level_slider)

        # Row 0, right third: NVAPI/NVML
        r0_right = tk.Frame(grid, bg=_PANE_BG)
        r0_right.grid(row=0, column=4, sticky="ew")
        self.level_entry = LiteEntry(
            r0_right, textvariable=self.level_var, width=4, justify="right"
        )
        self.level_entry.pack(side="left", padx=(0, 2))
        self._interactive_widgets.append(self.level_entry)
        tk.Label(
            r0_right, text="%", font=_FONT_BODY, bg=_PANE_BG, fg=_TEXT_FG_DIM
        ).pack(side="left", padx=(0, 10))
        self.cooler_api_var = ctk.StringVar(value="NVAPI")
        self.cooler_api_menu = ctk.CTkOptionMenu(
            r0_right,
            variable=self.cooler_api_var,
            values=["NVAPI", "NVML"],
            width=84,
            anchor="center",
            command=lambda _: self.controller.on_backend_change(),
            font=ct_button_font(r0_right),
        )
        self.cooler_api_menu.pack(side="right")
        self._interactive_widgets.append(self.cooler_api_menu)

        self.level_var.trace_add("write", lambda *_: self.controller.on_entry_change())

        # Row 1, left third: Policy
        r1_left = tk.Frame(grid, bg=_PANE_BG)
        r1_left.grid(row=1, column=0, sticky="ew", pady=(8, 0))
        r1_left.grid_columnconfigure(1, weight=1)
        tk.Label(
            r1_left,
            text="Policy:",
            font=_FONT_BODY,
            bg=_PANE_BG,
            fg=_TEXT_FG,
        ).grid(row=0, column=0, sticky="w")
        self.policy_var = ctk.StringVar(value="continuous")
        self.policy_menu = ctk.CTkOptionMenu(
            r1_left,
            variable=self.policy_var,
            values=NVAPI_POLICIES,
            width=1,  # grid stretches it to fill the third
            anchor="center",
            font=ct_button_font(r1_left),
        )
        self.policy_menu.grid(row=0, column=1, sticky="ew", padx=(6, 0))
        self._interactive_widgets.append(self.policy_menu)

        # Row 1, middle third: Apply
        self.btn_apply_cooler = LiteButton(
            grid,
            text="✅ Apply Section",
            width=10,
            fg_color="#1a6b2a",
            hover_color="#145220",
            command=self.controller.apply,
        )
        self.btn_apply_cooler.grid(row=1, column=2, sticky="ew", pady=(8, 0))
        self._interactive_widgets.append(self.btn_apply_cooler)

        # Row 1, right third: Reset
        self.btn_reset_cooler = LiteButton(
            grid,
            text="🔄 Reset to Auto",
            width=10,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self.controller.reset,
        )
        self.btn_reset_cooler.grid(row=1, column=4, sticky="ew", pady=(8, 0))
        self._interactive_widgets.append(self.btn_reset_cooler)

    # ── Controller protocol ───────────────────────────────────────────────
    def selected_api(self) -> str:
        return self.cooler_api_var.get()

    def selected_fan_id(self) -> str:
        return self.fan_id_var.get()

    def selected_policy(self) -> str:
        return self.policy_var.get()

    def fan_level(self) -> int:
        try:
            return int(self.level_slider.get())
        except (TypeError, ValueError):
            return 0

    def fan_level_text(self) -> str:
        return self.level_var.get()

    def set_policy_values(self, values: Sequence[str]) -> None:
        self.policy_menu.configure(values=list(values))

    def set_policy(self, policy: str) -> None:
        self.policy_var.set(policy)

    def set_level(self, level: int) -> None:
        text = str(level)
        if self.level_var.get() != text:
            self.level_var.set(text)
        self.level_slider.set(level)

    def set_supported(self, supported: bool) -> None:
        self.set_supported_state(supported)

    def set_supported_state(self, supported: bool) -> None:
        if supported == self._supported_state:
            return
        self._supported_state = supported

        state = "normal" if supported else "disabled"
        for widget in self._interactive_widgets:
            try:
                widget.configure(state=state)
            except Exception:
                pass

        self.level_slider.set_active(supported)
        if supported:
            if not self.level_var.get().strip():
                self.level_var.set(str(int(self.level_slider.get())))
        else:
            # No fan control on this GPU: blank the % value instead of
            # showing the 60 default.
            self.level_var.set("")

        self.cooler_title.configure(fg=_TEXT_FG if supported else "#8a8a8a")

    def on_resize_state_changed(
        self, resizing: bool, force_flush: bool = False
    ) -> None:
        """Compatibility hook for app-level resize coordinator."""
        return
