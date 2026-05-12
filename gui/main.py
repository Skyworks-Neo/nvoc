"""
NVOC-GUI — NVIDIA GPU VF Curve Optimizer GUI
Entry point for the application.
"""

import sys
import os
from typing import Any, Optional

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

from src.app import App


def main() -> int:
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
    raise SystemExit(main())
