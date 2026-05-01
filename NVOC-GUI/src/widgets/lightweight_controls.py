"""
Lightweight Tk/CTk hybrid controls for high-density panels.
"""

import tkinter as tk
from typing import List, Optional, Tuple

import customtkinter as ctk


def _is_descendant_widget(widget: tk.Misc, ancestor: tk.Misc) -> bool:
    """Return True when *widget* is *ancestor* or one of its descendants."""
    current: Optional[tk.Misc] = widget
    while current is not None:
        if current == ancestor:
            return True
        try:
            parent_name = current.winfo_parent()
            if not parent_name:
                return False
            current = current.nametowidget(parent_name)
        except Exception:
            return False
    return False


def install_mousewheel_support(scroll_frame: ctk.CTkScrollableFrame) -> None:
    """Install reliable wheel scrolling for CTkScrollableFrame on all platforms.

    Linux users on Wayland/XWayland may receive wheel events as either
    ``<Button-4>/<Button-5>`` or ``<MouseWheel>`` with tiny deltas. We bind all
    variants and normalize them into canvas unit scrolling.
    """
    if getattr(scroll_frame, "_nvoc_mousewheel_installed", False):
        return
    setattr(scroll_frame, "_nvoc_mousewheel_installed", True)

    def _resolve_canvas() -> Optional[tk.Misc]:
        canvas = getattr(scroll_frame, "_parent_canvas", None)
        if canvas is None:
            return None
        return canvas

    def _pointer_inside_frame() -> bool:
        if not scroll_frame.winfo_exists():
            return False
        try:
            pointer_x = scroll_frame.winfo_pointerx()
            pointer_y = scroll_frame.winfo_pointery()
            hovered = scroll_frame.winfo_containing(pointer_x, pointer_y)
        except tk.TclError:
            return False
        if hovered is None:
            return False
        return _is_descendant_widget(hovered, scroll_frame)

    def _on_mousewheel(event) -> Optional[str]:
        if not _pointer_inside_frame():
            return None

        canvas = _resolve_canvas()
        if canvas is None:
            return None

        steps = 0
        event_num = getattr(event, "num", None)
        if event_num == 4:
            steps = -1
        elif event_num == 5:
            steps = 1
        else:
            delta = int(getattr(event, "delta", 0) or 0)
            if delta == 0:
                return None
            if abs(delta) >= 120:
                steps = int(-delta / 120)
            else:
                steps = -1 if delta > 0 else 1

        if steps == 0:
            return "break"

        try:
            canvas.yview_scroll(steps, "units")
        except Exception:
            return None
        return "break"

    toplevel = scroll_frame.winfo_toplevel()
    toplevel.bind_all("<MouseWheel>", _on_mousewheel, add="+")
    toplevel.bind_all("<Button-4>", _on_mousewheel, add="+")
    toplevel.bind_all("<Button-5>", _on_mousewheel, add="+")


class CanvasSlider(ctk.CTkFrame):
    """Lightweight Canvas-based slider with a CTkSlider-like interface."""

    def __init__(self, parent, from_: int, to: int, number_of_steps: int, command=None):
        super().__init__(parent, fg_color="transparent", height=24)
        self._from = float(from_)
        self._to = float(to)
        self._steps = max(1, int(number_of_steps)) if number_of_steps is not None else 1
        self._command = command
        self._state = "normal"
        self._value = float(from_)

        self._track_pad_x = 8
        self._track_h = 6
        self._thumb_r = 8
        self._command_after_id = None
        self._pending_command_value = None
        self._command_interval_ms = 16

        self._canvas = tk.Canvas(
            self, height=24, highlightthickness=0, bd=0, bg="#242424"
        )
        self._canvas.pack(fill="both", expand=True)
        self._canvas.bind("<Configure>", lambda _e: self._redraw())
        self._canvas.bind("<Button-1>", self._on_pointer)
        self._canvas.bind("<B1-Motion>", self._on_pointer)
        self._canvas.bind("<ButtonRelease-1>", self._on_release)

    def configure(self, require_redraw: bool = True, **kwargs):
        if "from_" in kwargs:
            self._from = float(kwargs.pop("from_"))
        if "to" in kwargs:
            self._to = float(kwargs.pop("to"))
        if "number_of_steps" in kwargs:
            self._steps = max(1, int(kwargs.pop("number_of_steps")))
        if "command" in kwargs:
            self._command = kwargs.pop("command")
        if "state" in kwargs:
            self._state = kwargs.pop("state")

        super().configure(**kwargs)
        self._value = self._clamp(self._value)
        if require_redraw:
            self._redraw()

    def cget(self, key):
        if key == "from_":
            return self._from
        if key == "to":
            return self._to
        if key == "number_of_steps":
            return self._steps
        if key == "state":
            return self._state
        return super().cget(key)

    def set(self, value):
        self._value = self._clamp(float(value))
        self._redraw()

    def get(self):
        return self._value

    def _clamp(self, value: float) -> float:
        lo, hi = (
            (self._from, self._to) if self._from <= self._to else (self._to, self._from)
        )
        value = max(lo, min(hi, value))
        step = (hi - lo) / self._steps if self._steps else 0.0
        if step > 0:
            value = lo + round((value - lo) / step) * step
            value = max(lo, min(hi, value))
        return value

    def _value_to_x(self, value: float, width: float) -> float:
        lo, hi = (
            (self._from, self._to) if self._from <= self._to else (self._to, self._from)
        )
        span = max(1e-9, hi - lo)
        ratio = (value - lo) / span
        x0 = self._track_pad_x
        x1 = max(x0 + 1, width - self._track_pad_x)
        return x0 + ratio * (x1 - x0)

    def _x_to_value(self, x: float, width: float) -> float:
        lo, hi = (
            (self._from, self._to) if self._from <= self._to else (self._to, self._from)
        )
        x0 = self._track_pad_x
        x1 = max(x0 + 1, width - self._track_pad_x)
        ratio = (x - x0) / max(1e-9, (x1 - x0))
        ratio = max(0.0, min(1.0, ratio))
        return self._clamp(lo + ratio * (hi - lo))

    def _on_pointer(self, event):
        if self._state == "disabled":
            return
        width = max(1, self._canvas.winfo_width())
        new_value = self._x_to_value(float(event.x), float(width))
        if abs(new_value - self._value) < 1e-9:
            return
        self._value = new_value
        self._redraw()
        self._schedule_command(self._value)

    def _schedule_command(self, value: float):
        if callable(self._command):
            self._pending_command_value = value
            if self._command_after_id is None:
                self._command_after_id = self.after(
                    self._command_interval_ms, self._flush_command
                )

    def _flush_command(self):
        self._command_after_id = None
        value = self._pending_command_value
        self._pending_command_value = None
        if value is None or not callable(self._command):
            return
        self._command(value)

    def _on_release(self, _event):
        if self._command_after_id is not None:
            try:
                self.after_cancel(self._command_after_id)
            except Exception:
                pass
            self._command_after_id = None
        if self._pending_command_value is not None and callable(self._command):
            value = self._pending_command_value
            self._pending_command_value = None
            self._command(value)

    def _redraw(self):
        c = self._canvas
        w = max(1, c.winfo_width())
        h = max(1, c.winfo_height())
        c.delete("all")

        y = h / 2
        x0 = self._track_pad_x
        x1 = max(x0 + 1, w - self._track_pad_x)
        x_val = self._value_to_x(self._value, w)

        disabled = self._state == "disabled"
        bg_track = "#4a4a4a" if disabled else "#3a3a3a"
        fg_track = "#5f7f9f" if disabled else "#3B8ED0"
        thumb = "#8a8a8a" if disabled else "#d9d9d9"

        c.create_line(
            x0, y, x1, y, fill=bg_track, width=self._track_h, capstyle=tk.ROUND
        )
        c.create_line(
            x0, y, x_val, y, fill=fg_track, width=self._track_h, capstyle=tk.ROUND
        )
        c.create_oval(
            x_val - self._thumb_r,
            y - self._thumb_r,
            x_val + self._thumb_r,
            y + self._thumb_r,
            fill=thumb,
            outline="",
        )


class SegmentRangeSelector(ctk.CTkFrame):
    """Discrete range selector with draggable endpoints and snapped segments."""

    def __init__(self, parent, values: Optional[List[str]] = None, command=None):
        super().__init__(parent, fg_color="transparent", height=74)
        self._values = list(values or [])
        self._command = command
        self._state = "normal"
        self._start_idx = 0
        self._end_idx = 0
        self._active_handle = None  # type: Optional[str]
        self._last_active_handle = "end"
        self._pad_x = 18
        self._line_y = 26
        self._node_r = 5
        self._handle_r = 9
        self._track_w = 4
        self._hit_radius = 16

        self.grid_columnconfigure(0, weight=1)

        self._canvas = tk.Canvas(
            self, height=56, highlightthickness=0, bd=0, bg="#242424"
        )
        self._canvas.grid(row=0, column=0, sticky="ew")
        self._canvas.bind("<Configure>", lambda _e: self._redraw())
        self._canvas.bind("<Button-1>", self._on_press)
        self._canvas.bind("<B1-Motion>", self._on_drag)
        self._canvas.bind("<ButtonRelease-1>", self._on_release)

        self._summary = ctk.CTkLabel(
            self,
            text="No P-State data",
            anchor="w",
            font=("Segoe UI", 10),
            text_color="#7e8da1",
        )
        self._summary.grid(row=1, column=0, sticky="ew", pady=(0, 2))

        self.set_values(self._values)

    def configure(self, **kwargs):
        if "values" in kwargs:
            self.set_values(kwargs.pop("values"))
        if "command" in kwargs:
            self._command = kwargs.pop("command")
        if "state" in kwargs:
            self._state = kwargs.pop("state")
        super().configure(**kwargs)
        self._redraw()

    def cget(self, key):
        if key == "state":
            return self._state
        if key == "values":
            return list(self._values)
        return super().cget(key)

    def set_values(self, values: Optional[List[str]]):
        old_selection = self.get_selection()
        old_start = old_selection[0] if old_selection else None
        old_end = old_selection[1] if old_selection else None
        seen = set()  # type: Set[str]
        normalized = []  # type: List[str]
        for value in values or []:
            label = str(value).strip().upper()
            if not label or label in seen:
                continue
            seen.add(label)
            normalized.append(label)

        self._values = normalized
        if not self._values:
            self._start_idx = 0
            self._end_idx = 0
            self._update_summary()
            self._redraw()
            return

        if old_start in self._values and old_end in self._values:
            self._start_idx = self._values.index(old_start)
            self._end_idx = self._values.index(old_end)
            if self._start_idx > self._end_idx:
                self._start_idx, self._end_idx = self._end_idx, self._start_idx
        else:
            default_idx = len(self._values) - 1
            self._start_idx = default_idx
            self._end_idx = default_idx

        self._update_summary()
        self._redraw()

    def set_selection(self, start: str, end: Optional[str] = None):
        if not self._values:
            return
        start_label = str(start).strip().upper()
        end_label = str(end or start).strip().upper()
        if start_label not in self._values or end_label not in self._values:
            return
        self._start_idx = self._values.index(start_label)
        self._end_idx = self._values.index(end_label)
        if self._start_idx > self._end_idx:
            self._start_idx, self._end_idx = self._end_idx, self._start_idx
        self._update_summary()
        self._redraw()

    def get_selection(self) -> Optional[Tuple[str, str]]:
        if not self._values:
            return None
        return self._values[self._start_idx], self._values[self._end_idx]

    def _positions(self) -> List[float]:
        width = max(1, self._canvas.winfo_width())
        if len(self._values) <= 1:
            return [width / 2.0]
        x0 = self._pad_x
        x1 = max(x0 + 1, width - self._pad_x)
        step = (x1 - x0) / (len(self._values) - 1)
        return [x0 + step * i for i in range(len(self._values))]

    def _nearest_index(self, x: float) -> int:
        positions = self._positions()
        return min(range(len(positions)), key=lambda i: abs(positions[i] - x))

    def _pick_handle(self, x: float) -> str:
        positions = self._positions()
        start_x = positions[self._start_idx]
        end_x = positions[self._end_idx]

        if self._start_idx == self._end_idx:
            if abs(x - start_x) <= 2:
                return self._last_active_handle
            return "start" if x < start_x else "end"

        dist_start = abs(x - start_x)
        dist_end = abs(x - end_x)
        if dist_start <= self._hit_radius and dist_start <= dist_end:
            return "start"
        if dist_end <= self._hit_radius:
            return "end"
        if x <= start_x:
            return "start"
        if x >= end_x:
            return "end"
        return "start" if dist_start <= dist_end else "end"

    def _apply_drag_index(self, idx: int):
        if self._active_handle == "start":
            self._start_idx = min(idx, self._end_idx)
        elif self._active_handle == "end":
            self._end_idx = max(idx, self._start_idx)
        self._update_summary()
        self._redraw()
        if callable(self._command):
            selection = self.get_selection()
            if selection is not None:
                self._command(*selection)

    def _on_press(self, event):
        if self._state == "disabled" or not self._values:
            return
        self._active_handle = self._pick_handle(float(event.x))
        self._last_active_handle = self._active_handle
        self._apply_drag_index(self._nearest_index(float(event.x)))

    def _on_drag(self, event):
        if self._state == "disabled" or not self._values or self._active_handle is None:
            return
        self._apply_drag_index(self._nearest_index(float(event.x)))

    def _on_release(self, _event):
        self._active_handle = None

    def _update_summary(self):
        if not self._values:
            self._summary.configure(text="No P-State data")
            return
        start, end = self.get_selection() or ("", "")
        if start == end:
            text = f"Lock target: {start}"
        else:
            text = f"Lock range: {start} - {end}"
        self._summary.configure(text=text)

    def _redraw(self):
        c = self._canvas
        w = max(1, c.winfo_width())
        h = max(1, c.winfo_height())
        c.delete("all")

        if not self._values:
            c.create_text(
                w / 2,
                h / 2 - 6,
                text="No supported P-States",
                fill="#7e8da1",
                font=("Segoe UI", 10),
            )
            return

        positions = self._positions()
        disabled = self._state == "disabled"
        base_track = "#4a4a4a" if disabled else "#3a3a3a"
        active_track = "#6689a8" if disabled else "#3B8ED0"
        node_fill = "#707070" if disabled else "#c7d1dc"
        label_fill = "#7e8da1" if disabled else "#d6dfeb"
        handle_fill = "#969696" if disabled else "#f5f7fb"
        handle_outline = "#5f7f9f" if disabled else "#59b0ff"

        x0 = positions[0]
        x1 = positions[-1]
        c.create_line(
            x0,
            self._line_y,
            x1,
            self._line_y,
            fill=base_track,
            width=self._track_w,
            capstyle=tk.ROUND,
        )

        active_x0 = positions[self._start_idx]
        active_x1 = positions[self._end_idx]
        c.create_line(
            active_x0,
            self._line_y,
            active_x1,
            self._line_y,
            fill=active_track,
            width=self._track_w + 1,
            capstyle=tk.ROUND,
        )

        for idx, (x, label) in enumerate(zip(positions, self._values)):
            in_range = self._start_idx <= idx <= self._end_idx
            radius = self._node_r
            fill = active_track if in_range and not disabled else node_fill
            outline = ""
            if idx == self._start_idx or idx == self._end_idx:
                radius = self._handle_r
                fill = handle_fill
                outline = handle_outline
            c.create_oval(
                x - radius,
                self._line_y - radius,
                x + radius,
                self._line_y + radius,
                fill=fill,
                outline=outline,
                width=2 if outline else 0,
            )
            c.create_text(
                x,
                self._line_y + 20,
                text=label,
                fill=label_fill,
                font=("Segoe UI", 10, "bold"),
            )


class LiteButton(ctk.CTkFrame):
    """Rounded canvas-backed button with lightweight rendering and CTk-like API."""

    def __init__(
        self,
        parent,
        text: str,
        command=None,
        width: int = 90,
        height: int = 28,
        fg_color: str = "#2f6fa5",
        hover_color: str = "#3B8ED0",
        text_color: str = "#f2f2f2",
        corner_radius: int = 9,
    ):
        super().__init__(parent, fg_color="transparent", width=width, height=height)
        self._text = text
        self._command = command
        self._state = "normal"
        self._fg_color = fg_color
        self._hover_color = hover_color
        self._text_color = text_color
        self._corner_radius = max(3, int(corner_radius))
        self._hovered = False
        self._pressed = False

        self.grid_propagate(False)
        self.pack_propagate(False)

        self._canvas = tk.Canvas(self, highlightthickness=0, bd=0, bg="#242424")
        self._canvas.pack(fill="both", expand=True)
        self._canvas.bind("<Configure>", lambda _e: self._redraw())
        self._canvas.bind("<Enter>", self._on_enter)
        self._canvas.bind("<Leave>", self._on_leave)
        self._canvas.bind("<ButtonPress-1>", self._on_press)
        self._canvas.bind("<ButtonRelease-1>", self._on_release)

    def configure(self, **kwargs):
        if "text" in kwargs:
            self._text = kwargs.pop("text")
        if "command" in kwargs:
            self._command = kwargs.pop("command")
        if "state" in kwargs:
            self._state = kwargs.pop("state")
        if "fg_color" in kwargs:
            self._fg_color = kwargs.pop("fg_color")
        if "hover_color" in kwargs:
            self._hover_color = kwargs.pop("hover_color")
        if "text_color" in kwargs:
            self._text_color = kwargs.pop("text_color")
        width = kwargs.pop("width", None)
        height = kwargs.pop("height", None)
        if width is not None:
            super().configure(width=int(width))
        if height is not None:
            super().configure(height=int(height))
        super().configure(**kwargs)
        self._redraw()

    def cget(self, key):
        if key == "text":
            return self._text
        if key == "state":
            return self._state
        if key == "fg_color":
            return self._fg_color
        return super().cget(key)

    def _on_enter(self, _event):
        self._hovered = True
        self._redraw()

    def _on_leave(self, _event):
        self._hovered = False
        self._pressed = False
        self._redraw()

    def _on_press(self, _event):
        if self._state == "disabled":
            return
        self._pressed = True
        self._redraw()

    def _on_release(self, event):
        was_pressed = self._pressed
        self._pressed = False
        self._redraw()
        if self._state == "disabled":
            return
        if not was_pressed:
            return
        w = max(1, self._canvas.winfo_width())
        h = max(1, self._canvas.winfo_height())
        if 0 <= event.x <= w and 0 <= event.y <= h and callable(self._command):
            self._command()

    def _rounded_rect(self, x0, y0, x1, y1, r, **kwargs):
        points = [
            x0 + r,
            y0,
            x1 - r,
            y0,
            x1,
            y0,
            x1,
            y0 + r,
            x1,
            y1 - r,
            x1,
            y1,
            x1 - r,
            y1,
            x0 + r,
            y1,
            x0,
            y1,
            x0,
            y1 - r,
            x0,
            y0 + r,
            x0,
            y0,
        ]
        return self._canvas.create_polygon(
            points, smooth=True, splinesteps=24, **kwargs
        )

    def _redraw(self):
        c = self._canvas
        w = max(1, c.winfo_width())
        h = max(1, c.winfo_height())
        c.delete("all")
        r = min(self._corner_radius, h // 2)

        if self._state == "disabled":
            fill = "#4a4a4a"
            text_color = "#8a8a8a"
        elif self._pressed:
            fill = "#2b5f8c"
            text_color = self._text_color
        elif self._hovered:
            fill = self._hover_color
            text_color = self._text_color
        else:
            fill = self._fg_color
            text_color = self._text_color

        self._rounded_rect(1, 1, w - 1, h - 1, r, fill=fill, outline="")
        c.create_text(
            w / 2,
            h / 2,
            text=self._text,
            fill=text_color,
            font=("Segoe UI", 10, "bold"),
        )


class LiteEntry(ctk.CTkFrame):
    """Rounded dark-theme entry with larger bold text and lower resize overhead."""

    def __init__(
        self,
        parent,
        textvariable=None,
        width: int = 12,
        justify: str = "left",
        height: int = 32,
        font: Tuple[str, int, str] = ("Segoe UI", 12, "bold"),
        min_px: int = 48,
    ):
        self._char_width = max(4, int(width))
        self._min_px = max(24, int(min_px))
        px_width = max(self._min_px, int(self._char_width * 8.5))
        super().__init__(
            parent,
            fg_color="#2b2b2b",
            corner_radius=10,
            border_width=1,
            border_color="#3a3a3a",
            width=px_width,
            height=height,
        )
        self.grid_propagate(False)
        self.pack_propagate(False)
        self._entry = tk.Entry(
            self,
            textvariable=textvariable,
            relief=tk.FLAT,
            bd=0,
            highlightthickness=0,
            bg="#2b2b2b",
            fg="#f0f0f0",
            insertbackground="#f0f0f0",
            justify=justify,
            font=font,
            width=self._char_width,
        )
        self._entry.pack(fill="both", expand=True, padx=10, pady=4)

    def configure(self, **kwargs):
        state = kwargs.pop("state", None)
        width = kwargs.pop("width", None)
        textvariable = kwargs.pop("textvariable", None)
        justify = kwargs.pop("justify", None)
        font = kwargs.pop("font", None)

        if width is not None:
            self._char_width = max(4, int(width))
            self._entry.configure(width=self._char_width)
            super().configure(width=max(self._min_px, int(self._char_width * 8.5)))
        if state is not None:
            self._entry.configure(state=state)
            border = "#3a3a3a" if state == "normal" else "#4a4a4a"
            self._entry.configure(
                disabledbackground="#2f2f2f", disabledforeground="#8a8a8a"
            )
            super().configure(border_color=border)
        if textvariable is not None:
            self._entry.configure(textvariable=textvariable)
        if justify is not None:
            self._entry.configure(justify=justify)
        if font is not None:
            self._entry.configure(font=font)

        super().configure(**kwargs)

    config = configure

    def cget(self, key):
        if key in {"state", "textvariable", "justify", "font", "width"}:
            return self._entry.cget(key)
        return super().cget(key)

    def bind(self, sequence=None, func=None, add=None):
        return self._entry.bind(sequence, func, add)

    def get(self):
        return self._entry.get()

    def delete(self, first, last=None):
        self._entry.delete(first, last)

    def insert(self, index, string):
        self._entry.insert(index, string)
