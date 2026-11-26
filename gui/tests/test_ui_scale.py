"""Raw-tk pixel geometry scale contract.

Linux/X11: the factor follows CustomTkinter's effective widget scaling (the
app mirrors Xft.dpi into it) and falls back to the realized screen DPI.
Windows: always 1.0 — CTk already tracks per-monitor DPI there and the
Windows layout is verified as-is.
"""

from __future__ import annotations

import sys

import pytest

from src.ui_scale import ui_px_scale


class _Widget:
    def __init__(self, widget_scaling=None, fpixels=96.0):
        self._widget_scaling = widget_scaling
        self._fpixels = fpixels

    def _get_widget_scaling(self):
        if self._widget_scaling is None:
            raise AttributeError("no CTk scaling")
        return self._widget_scaling

    def winfo_fpixels(self, _units):
        return self._fpixels


def test_linux_uses_ctk_widget_scaling(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "linux")
    assert ui_px_scale(_Widget(widget_scaling=2.0)) == pytest.approx(2.0)


def test_linux_falls_back_to_screen_dpi(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "linux")
    assert ui_px_scale(_Widget(fpixels=144.0)) == pytest.approx(1.5)


def test_linux_never_shrinks(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "linux")
    assert ui_px_scale(_Widget(widget_scaling=0.5)) == pytest.approx(1.0)
    assert ui_px_scale(_Widget(fpixels=72.0)) == pytest.approx(1.0)


def test_linux_unavailable_sources_default_to_one(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "linux")

    class _Broken:
        def _get_widget_scaling(self):
            raise RuntimeError("no Tcl")

        def winfo_fpixels(self, _units):
            raise RuntimeError("no Tcl")

    assert ui_px_scale(_Broken()) == pytest.approx(1.0)


def test_windows_is_always_one(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "win32")
    assert ui_px_scale(_Widget(widget_scaling=2.0, fpixels=144.0)) == pytest.approx(1.0)
