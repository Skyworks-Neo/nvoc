"""
NVOC-GUI — NVIDIA GPU VF Curve Optimizer GUI
Entry point for the application.
"""

import importlib
import os
import sys
from typing import Any, Callable, Optional

# Ensure the project root is in path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Fix blurry/tiny rendering on Windows HiDPI displays (e.g. 150% scaling).
if sys.platform == "win32":
    try:
        import ctypes

        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:
            pass


class GuiStartupError(RuntimeError):
    """Raised when the GUI runtime is missing a required local dependency."""


def _tkinter_install_hint() -> str:
    if sys.platform == "win32":
        return (
            "Install Python from python.org with the Tcl/Tk option enabled, "
            "then recreate or resync the virtual environment."
        )
    if sys.platform == "darwin":
        return (
            "Install a Python build that includes Tcl/Tk support, then recreate "
            "or resync the virtual environment."
        )
    return (
        "Install Tk support for the Python interpreter, then recreate or resync "
        "the virtual environment if needed.\n"
        "Examples:\n"
        "  Arch Linux: sudo pacman -S tk\n"
        "  Debian/Ubuntu: sudo apt install python3-tk\n"
        "  Fedora: sudo dnf install python3-tkinter\n"
        "With uv, select an interpreter that passes "
        '`python -c "import tkinter"` before running `uv sync`.'
    )


def _require_gui_runtime(
    import_module: Callable[[str], Any] = importlib.import_module,
) -> None:
    try:
        import_module("tkinter")
    except ModuleNotFoundError as exc:
        if exc.name != "tkinter":
            raise
        raise GuiStartupError(
            "NVOC-GUI requires Python Tk support (`tkinter`), but this "
            "interpreter cannot import it.\n\n"
            f"{_tkinter_install_hint()}"
        ) from exc

    try:
        import_module("customtkinter")
    except ModuleNotFoundError as exc:
        if exc.name != "customtkinter":
            raise
        raise GuiStartupError(
            "NVOC-GUI dependency `customtkinter` is missing. Run `uv sync` "
            "from the `gui/` directory, or install `gui/requirements.txt` "
            "in the active environment."
        ) from exc


def main() -> int:
    _require_gui_runtime()

    from src.app import App
    from src.single_instance import SingleInstanceGuard

    guard: Optional[Any] = SingleInstanceGuard()
    try:
        if not guard.acquire():
            guard.signal_existing_instance()
            return 0
    except OSError as exc:
        raise RuntimeError(
            f"Failed to initialize single-instance guard: {exc}"
        ) from exc

    import time

    start_time = time.perf_counter()

    try:
        app = App(single_instance_guard=guard)

        def log_startup_time() -> None:
            elapsed_time = time.perf_counter() - start_time
            app.console.append(
                f"[GUI] Application started in {elapsed_time:.3f} seconds.\n"
            )

        app.after(50, log_startup_time)
        app.mainloop()
        return 0
    finally:
        if guard is not None:
            guard.release()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except GuiStartupError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1) from None
