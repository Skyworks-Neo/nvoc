"""
Output Console Widget - A read-only scrollable text area for CLI output.
"""

import threading
import sys

import tkinter.font as tk_font

import customtkinter as ctk


class OutputConsole(ctk.CTkFrame):
    """A docked output console that displays CLI output in real-time."""

    _MAX_LINES = 100

    def __init__(self, master, **kwargs) -> None:
        super().__init__(master, **kwargs)
        # Standalone window console: body always expanded, no fold header.
        self._expanded = True
        self._lock = threading.Lock()

        self.clear_button = ctk.CTkButton(
            self, text="Clear", width=60, height=24, command=self.clear
        )
        self.clear_button.pack(anchor="e", padx=5, pady=(5, 0))

        self.textbox = ctk.CTkTextbox(
            self, state="disabled", font=self._mono_font(), wrap="none"
        )
        self.textbox.pack(fill="both", expand=True, padx=5, pady=(0, 5))
        self.textbox.tag_config("lime", foreground="lime")
        self.textbox.tag_config("red", foreground="red")

    @staticmethod
    def _mono_font() -> tuple[str, int]:
        """Use Consolas on Windows and Tk's native fixed font elsewhere."""
        if sys.platform == "win32":
            return ("Consolas", 12)
        fixed = tk_font.nametofont("TkFixedFont")
        return (str(fixed.cget("family")), max(10, int(fixed.cget("size"))))

    def toggle(self, _event: object = None) -> None:
        """Toggle the console body between folded and expanded."""
        self._set_expanded(not self._expanded)

    def _set_expanded(self, expanded: bool):
        """Show or hide the console body while keeping the header visible."""
        self._expanded = expanded
        self.toggle_label.configure(
            text=f"{'[-]' if expanded else '[+]'} Output Console"
        )
        if expanded:
            self.textbox.pack(fill="both", expand=True, padx=5, pady=5)
        else:
            self.textbox.pack_forget()

    def append(self, text: str) -> None:
        """Append text to the console (thread-safe) and keep only the last 100 lines."""
        self.append_batch([text])

    def append_batch(self, texts: list) -> None:
        """Append multiple chunks in ONE text-widget update.

        Each per-chunk append costs several Tcl round-trips plus see("end")
        (which forces scroll relayout); batching N chunks into a single
        insert/trim/see is ~150x cheaper for large bursts (e.g. streaming
        autoscan output).
        """
        with self._lock:
            self.textbox.configure(state="normal")

            start_index = self.textbox.index("end-1c")
            for text in texts:
                self.textbox.insert("end", text)
                end_index = self.textbox.index("end-1c")

                # Check if it has a return code
                if "code 0" in text.lower() or "successfully" in text.lower():
                    self.textbox.tag_add("lime", start_index, end_index)
                elif (
                    "return code" in text.lower()
                    or "code " in text.lower()
                    or "failed" in text.lower()
                ):
                    # If it's a message with a return code that isn't 0
                    self.textbox.tag_add("red", start_index, end_index)

                start_index = end_index

            # Keep only the last 100 lines
            line_count = int(float(self.textbox.index("end-1c")))
            if line_count > self._MAX_LINES:
                self.textbox.delete("1.0", f"{line_count - self._MAX_LINES}.0")

            # Scrolling a folded console is wasted re-render work
            if self._expanded:
                self.textbox.see("end")
            self.textbox.configure(state="disabled")

    def clear(self) -> None:
        """Clear all console text."""
        self.textbox.configure(state="normal")
        self.textbox.delete("1.0", "end")
        self.textbox.configure(state="disabled")
