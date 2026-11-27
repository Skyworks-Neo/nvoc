"""Pixel-geometry scale for raw-tk widgets inside CTk layouts.

CustomTkinter scales the widgets it creates (frames, buttons, padding),
but raw ``tk.Canvas``/``tk.Frame`` width/height values and canvas
coordinates are plain pixels. Tk's Xft backend meanwhile renders
point-size text at the X display's DPI (Xft.dpi). On a 192-dpi session
that mismatch shrinks/clips the canvas chrome around 2× text. Multiply
pixel geometry by this factor; never multiply point font sizes.

Linux-only by design: CustomTkinter already tracks the per-monitor DPI
factor on Windows, and the app's Windows layout is verified as-is, so
``ui_px_scale`` returns 1 there.
"""

from __future__ import annotations

import sys


def ui_px_scale(widget) -> float:
    """Pixel scale for a widget's raw-tk children (1.0 = no scaling)."""
    if not sys.platform.startswith("linux"):
        return 1.0
    try:
        return max(1.0, float(widget._get_widget_scaling()))
    except Exception:
        pass
    try:
        return max(1.0, float(widget.winfo_fpixels("1i")) / 96.0)
    except Exception:
        return 1.0
