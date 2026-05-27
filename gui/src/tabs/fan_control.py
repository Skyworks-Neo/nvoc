"""
Fan Control Tab - Cooler policy and level controls.
"""

from typing import TYPE_CHECKING, List

import customtkinter as ctk
from src.widgets.lightweight_controls import (
    CanvasSlider,
    LiteButton,
    LiteEntry,
    install_mousewheel_support,
)

if TYPE_CHECKING:
    from src.app import App


class FanControlTab:
    """Fan/cooler control tab."""

    _NVAPI_POLICIES = [
        "default",
        "manual",
        "perf",
        "discrete",
        "continuous",
        "hybrid",
        "software",
        "default32",
    ]
    _NVML_POLICIES = ["continuous", "manual"]

    def __init__(
        self, parent: ctk.CTkFrame, app: "App", embedded: bool = False
    ) -> None:
        self.app = app
        self.frame = parent
        self._interactive_widgets = []  # type: List[object]
        self._supported_state = True

        if embedded:
            container = self.frame
        else:
            container = ctk.CTkScrollableFrame(self.frame)
            container.pack(fill="both", expand=True, padx=10, pady=10)
            install_mousewheel_support(container)

        self._build_content(container)

    def _build_content(self, parent: ctk.CTkFrame) -> None:

        self.content_row = ctk.CTkFrame(parent, fg_color="transparent")
        self.content_row.pack(fill="x", pady=(0, 10))
        self.content_row.grid_columnconfigure(0, weight=1)
        self.content_row.grid_columnconfigure(1, weight=1)

        self.cooler_frame = ctk.CTkFrame(self.content_row)
        self.cooler_frame.grid(row=0, column=0, sticky="nsew", padx=(0, 5))

        cooler_header = ctk.CTkFrame(self.cooler_frame, fg_color="transparent")
        cooler_header.pack(fill="x", padx=10, pady=(10, 5))
        self.cooler_title = ctk.CTkLabel(
            cooler_header, text="Fan / Cooler Control", font=("", 14, "bold")
        )
        self.cooler_title.pack(side="left")

        self.cooler_api_var = ctk.StringVar(value="NVAPI")
        self.cooler_api_menu = ctk.CTkOptionMenu(
            cooler_header,
            variable=self.cooler_api_var,
            values=["NVAPI", "NVML"],
            width=120,
            command=self._on_backend_change,
        )
        self.cooler_api_menu.pack(side="right")
        ctk.CTkLabel(cooler_header, text="→", text_color="gray70").pack(
            side="right", padx=(0, 6)
        )
        self._interactive_widgets.append(self.cooler_api_menu)

        grid = ctk.CTkFrame(self.cooler_frame, fg_color="transparent")
        grid.pack(fill="x", padx=10, pady=(0, 10))
        grid.columnconfigure(1, weight=1)

        row = 0
        ctk.CTkLabel(grid, text="Fan Target:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.fan_id_var = ctk.StringVar(value="All")
        self.fan_id_menu = ctk.CTkOptionMenu(
            grid, variable=self.fan_id_var, values=["All", "Fan 1", "Fan 2"], width=120
        )
        self.fan_id_menu.grid(row=row, column=1, sticky="w", padx=5, pady=3)
        self._interactive_widgets.append(self.fan_id_menu)

        row += 1
        ctk.CTkLabel(grid, text="Policy:").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.policy_var = ctk.StringVar(value="continuous")
        self.policy_menu = ctk.CTkOptionMenu(
            grid,
            variable=self.policy_var,
            values=self._NVAPI_POLICIES,
            width=220,
        )
        self.policy_menu.grid(row=row, column=1, sticky="w", padx=5, pady=3)
        self._interactive_widgets.append(self.policy_menu)

        row += 1
        ctk.CTkLabel(grid, text="Fan Level (%):").grid(
            row=row, column=0, sticky="w", padx=5, pady=3
        )
        self.level_var = ctk.StringVar(value="60")

        level_frame = ctk.CTkFrame(grid, fg_color="transparent")
        level_frame.grid(row=row, column=1, sticky="w", padx=5, pady=3)

        self.level_slider = CanvasSlider(
            level_frame,
            from_=0,
            to=100,
            number_of_steps=100,
            command=self._on_slider_change,
        )
        self.level_slider.configure(width=180)
        self.level_slider.set(60)
        self.level_slider.grid(row=0, column=0, sticky="w", padx=(0, 10))
        self._interactive_widgets.append(self.level_slider)

        self.level_entry = LiteEntry(
            level_frame, textvariable=self.level_var, width=6, justify="right"
        )
        self.level_entry.grid(row=0, column=1)
        self._interactive_widgets.append(self.level_entry)
        ctk.CTkLabel(level_frame, text="%").grid(row=0, column=2, padx=(3, 0))

        self.level_var.trace_add("write", self._on_entry_change)

        btn_row = ctk.CTkFrame(self.cooler_frame, fg_color="transparent")
        btn_row.pack(fill="x", padx=10, pady=(0, 10))
        self.btn_apply_cooler = LiteButton(
            btn_row,
            text="✅ Apply Fan Settings",
            width=180,
            fg_color="#1a6b2a",
            hover_color="#145220",
            command=self._apply_cooler,
        )
        self.btn_apply_cooler.pack(side="left", padx=5)
        self._interactive_widgets.append(self.btn_apply_cooler)

        self.btn_reset_cooler = LiteButton(
            btn_row,
            text="🔄 Reset to Auto",
            width=150,
            fg_color="#c0392b",
            hover_color="#96281b",
            command=self._reset_cooler,
        )
        self.btn_reset_cooler.pack(side="left", padx=5)
        self._interactive_widgets.append(self.btn_reset_cooler)

        self.preset_frame = ctk.CTkFrame(self.content_row)
        self.preset_frame.grid(row=0, column=1, sticky="nsew", padx=(5, 0))
        self.preset_title = ctk.CTkLabel(
            self.preset_frame, text="Quick Presets", font=("", 14, "bold")
        )
        self.preset_title.pack(anchor="w", padx=10, pady=(10, 5))

        preset_grid = ctk.CTkFrame(self.preset_frame, fg_color="transparent")
        preset_grid.pack(fill="x", padx=10, pady=(0, 10))
        preset_grid.grid_columnconfigure(0, weight=1)
        preset_grid.grid_columnconfigure(1, weight=1)

        presets = [
            ("Silent (30%)", 30),
            ("Balanced (50%)", 50),
            ("Performance (70%)", 70),
            ("Max (100%)", 100),
        ]
        for idx, (label, level) in enumerate(presets):
            btn = LiteButton(
                preset_grid,
                text=label,
                command=lambda lvl=level: self._set_preset(lvl),
                width=140,
            )
            btn.grid(row=idx // 2, column=idx % 2, sticky="ew", padx=5, pady=5)
            self._interactive_widgets.append(btn)

        self._enabled_frame_color = self.cooler_frame.cget("fg_color")
        self._dim_frame_color = ("gray86", "gray20")
        self._enabled_title_color = self.cooler_title.cget("text_color")
        self._dim_title_color = "gray55"

    def set_supported(self, supported: bool) -> None:
        """Enable/disable the whole fan control section with a dimmed visual state."""
        if supported == self._supported_state:
            return
        self._supported_state = supported

        state = "normal" if supported else "disabled"
        for widget in self._interactive_widgets:
            try:
                widget.configure(state=state)
            except Exception:
                pass

        frame_color = self._enabled_frame_color if supported else self._dim_frame_color
        title_color = self._enabled_title_color if supported else self._dim_title_color
        self.cooler_frame.configure(fg_color=frame_color)
        self.preset_frame.configure(fg_color=frame_color)
        self.cooler_title.configure(text_color=title_color)
        self.preset_title.configure(text_color=title_color)

    def on_resize_state_changed(
        self, resizing: bool, force_flush: bool = False
    ) -> None:
        """Compatibility hook for app-level resize coordinator."""
        return

    def _fan_id_args(self) -> List[str]:
        """Return [--id, N] if a specific fan is selected, else []."""
        val = self.fan_id_var.get()
        if val.startswith("Fan "):
            try:
                return ["--id", val.split()[1]]
            except IndexError:
                pass
        return []

    def _selected_cooler_backend(self) -> str:
        """Return the selected backend command for cooler control."""
        selected = self.cooler_api_var.get().strip().upper()
        return "nvml-cooler" if selected == "NVML" else "nvapi-cooler"

    def _allowed_policies_for_backend(self) -> List[str]:
        backend = self._selected_cooler_backend()
        return self._NVML_POLICIES if backend == "nvml-cooler" else self._NVAPI_POLICIES

    def _normalize_policy_for_backend(self) -> str:
        policy = self.policy_var.get().lower().strip()
        allowed = self._allowed_policies_for_backend()
        if policy not in allowed:
            policy = "continuous" if "continuous" in allowed else allowed[0]
            self.policy_var.set(policy)
        return policy

    def _on_backend_change(self, _: str) -> None:
        """Sync policy dropdown choices when cooler backend changes."""
        allowed = self._allowed_policies_for_backend()
        self.policy_menu.configure(values=allowed)
        self._normalize_policy_for_backend()

    def _on_slider_change(self, value: float) -> None:
        self.level_var.set(str(int(value)))

    def _on_entry_change(self, *_: object) -> None:
        try:
            val = int(self.level_var.get())
            if 0 <= val <= 100:
                self.level_slider.set(val)
        except ValueError:
            pass

    def _set_preset(self, level: int) -> None:
        self.policy_var.set("continuous")
        self.level_var.set(str(level))
        self.level_slider.set(level)
        self._apply_cooler()

    def _apply_cooler(self) -> None:
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_cooler_backend()
        policy = self._normalize_policy_for_backend()

        args = gpu_args + ["set", backend]

        # Keep target selection sourced from the actual UI control.
        args.extend(self._fan_id_args())

        args.extend(
            [
                "--policy",
                policy,
                "--level",
                str(int(self.level_slider.get())),
            ]
        )

        self.app.run_cli_display(args)

    def _reset_cooler(self) -> None:
        gpu_args = self.app.get_gpu_args()
        backend = self._selected_cooler_backend()
        args = gpu_args + ["set", backend]
        args.extend(self._fan_id_args())

        args.extend(["--policy", "auto", "--level", "0"])

        self.app.run_cli_display(args)
