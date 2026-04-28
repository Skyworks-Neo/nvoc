"""Reusable hover tooltip helpers for CTk/Tk widgets."""

from __future__ import annotations

import tkinter as tk
from typing import Any


class HoverTooltip:
    """Attach a small delayed tooltip to a widget."""

    def __init__(
        self, widget: Any, text: str, delay_ms: int = 350, wraplength: int = 340
    ):
        self.widget = widget
        self.text = text
        self.delay_ms = max(0, int(delay_ms))
        self.wraplength = max(140, int(wraplength))

        self._after_id: str | None = None
        self._tip_window: tk.Toplevel | None = None
        self._x_root = 0
        self._y_root = 0

        self.widget.bind("<Enter>", self._on_enter, add="+")
        self.widget.bind("<Leave>", self._on_leave, add="+")
        self.widget.bind("<Motion>", self._on_motion, add="+")
        self.widget.bind("<Destroy>", self._on_destroy, add="+")

    def _on_enter(self, event: tk.Event):
        self._x_root, self._y_root = int(event.x_root), int(event.y_root)
        self._schedule_show()

    def _on_motion(self, event: tk.Event):
        self._x_root, self._y_root = int(event.x_root), int(event.y_root)
        if self._tip_window is not None:
            self._position_tip()

    def _on_leave(self, _event: tk.Event):
        self._cancel_show()
        self._hide()

    def _on_destroy(self, _event: tk.Event):
        self._cancel_show()
        self._hide()

    def _schedule_show(self):
        self._cancel_show()
        self._after_id = self.widget.after(self.delay_ms, self._show)

    def _cancel_show(self):
        if self._after_id is None:
            return
        try:
            self.widget.after_cancel(self._after_id)
        except Exception:
            pass
        self._after_id = None

    def _show(self):
        self._after_id = None
        if self._tip_window is not None:
            return

        tip = tk.Toplevel(self.widget)
        tip.wm_overrideredirect(True)
        tip.attributes("-topmost", True)
        tip.configure(bg="#1f1f1f")

        label = tk.Label(
            tip,
            text=self.text,
            justify="left",
            padx=8,
            pady=5,
            bg="#1f1f1f",
            fg="#f0f0f0",
            relief="solid",
            borderwidth=1,
            wraplength=self.wraplength,
            font=("Segoe UI", 9),
        )
        label.pack()

        self._tip_window = tip
        self._position_tip()

    def _position_tip(self):
        if self._tip_window is None:
            return
        x = self._x_root + 14
        y = self._y_root + 16
        self._tip_window.wm_geometry(f"+{x}+{y}")

    def _hide(self):
        if self._tip_window is None:
            return
        try:
            self._tip_window.destroy()
        except Exception:
            pass
        self._tip_window = None
