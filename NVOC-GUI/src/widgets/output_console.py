"""
Output Console Widget - A read-only scrollable text area for CLI output.
"""

import threading

import customtkinter as ctk


class OutputConsole(ctk.CTkFrame):
    """A docked output console that displays CLI output in real-time."""

    _MAX_LINES = 100

    def __init__(self, master, **kwargs) -> None:
        super().__init__(master, **kwargs)
        self._expanded = False
        self._lock = threading.Lock()

        # Header stays visible; clicking it toggles the console body.
        self.header = ctk.CTkFrame(
            self, height=30, fg_color="transparent", cursor="hand2"
        )
        self.header.pack(fill="x", padx=5, pady=(5, 0))

        self.toggle_label = ctk.CTkLabel(
            self.header,
            text="[+] Output Console",
            font=("", 13, "bold"),
            cursor="hand2",
        )
        self.toggle_label.pack(side="left")
        self.toggle_label.bind("<Button-1>", self.toggle)
        self.header.bind("<Button-1>", self.toggle)

        self.clear_button = ctk.CTkButton(
            self.header, text="Clear", width=60, height=24, command=self.clear
        )
        self.clear_button.pack(side="right")

        self.textbox = ctk.CTkTextbox(
            self, state="disabled", font=("Consolas", 12), wrap="none", height=200
        )
        self.textbox.tag_config("lime", foreground="lime")
        self.textbox.tag_config("red", foreground="red")
        self._set_expanded(False)

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
        """Append text to the console (thread-safe) and keep only the last 1000 lines."""
        with self._lock:
            self.textbox.configure(state="normal")

            start_index = self.textbox.index("end-1c")
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

            # Keep only the last 1000 lines
            line_count = int(float(self.textbox.index("end-1c")))
            if line_count > self._MAX_LINES:
                self.textbox.delete("1.0", f"{line_count - self._MAX_LINES}.0")

            self.textbox.see("end")
            self.textbox.configure(state="disabled")

    def clear(self) -> None:
        """Clear all console text."""
        self.textbox.configure(state="normal")
        self.textbox.delete("1.0", "end")
        self.textbox.configure(state="disabled")
