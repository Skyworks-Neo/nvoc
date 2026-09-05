from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import Mock

from src.widgets import lightweight_controls
from src.widgets.lightweight_controls import CanvasSlider


class FakeCanvas:
    def __init__(self) -> None:
        self.scrolls: list[tuple[int, str]] = []

    def yview_scroll(self, steps: int, unit: str) -> None:
        self.scrolls.append((steps, unit))


class FakeTopLevel:
    def __init__(self) -> None:
        self.hovered = None
        self.bindings: dict[str, object] = {}

    def winfo_pointerx(self) -> int:
        return 1

    def winfo_pointery(self) -> int:
        return 1

    def winfo_containing(self, _x: int, _y: int):
        return self.hovered

    def bind_all(self, event: str, callback, *, add: str) -> None:
        assert add == "+"
        self.bindings[event] = callback


class FakeFrame:
    def __init__(self, toplevel: FakeTopLevel) -> None:
        self.toplevel = toplevel
        self._parent_canvas = FakeCanvas()

    def winfo_toplevel(self) -> FakeTopLevel:
        return self.toplevel

    def winfo_exists(self) -> bool:
        return True


def test_mousewheel_uses_one_toplevel_dispatcher_for_multiple_frames(
    monkeypatch,
) -> None:
    toplevel = FakeTopLevel()
    first = FakeFrame(toplevel)
    second = FakeFrame(toplevel)
    monkeypatch.setattr(
        lightweight_controls,
        "_is_descendant_widget",
        lambda hovered, frame: hovered is frame,
    )

    lightweight_controls.install_mousewheel_support(first)
    lightweight_controls.install_mousewheel_support(second)

    assert set(toplevel.bindings) == {"<MouseWheel>", "<Button-4>", "<Button-5>"}
    assert toplevel._nvoc_mousewheel_frames == [first, second]

    toplevel.hovered = second
    result = toplevel.bindings["<MouseWheel>"](SimpleNamespace(num=None, delta=-120))

    assert result == "break"
    assert first._parent_canvas.scrolls == []
    assert second._parent_canvas.scrolls == [(6, "units")]


def test_linux_wheel_event_uses_same_scroll_multiplier(monkeypatch) -> None:
    toplevel = FakeTopLevel()
    frame = FakeFrame(toplevel)
    toplevel.hovered = frame
    monkeypatch.setattr(
        lightweight_controls,
        "_is_descendant_widget",
        lambda hovered, candidate: hovered is candidate,
    )
    lightweight_controls.install_mousewheel_support(frame)

    result = toplevel.bindings["<Button-4>"](SimpleNamespace(num=4, delta=0))

    assert result == "break"
    assert frame._parent_canvas.scrolls == [(-6, "units")]


def test_canvas_slider_skips_redraw_when_clamped_value_is_unchanged() -> None:
    slider = CanvasSlider.__new__(CanvasSlider)
    slider._from = 0.0
    slider._to = 100.0
    slider._steps = 100
    slider._value = 50.0
    slider._geometry_dirty = False
    slider._redraw = Mock()

    slider.set(50)

    slider._redraw.assert_not_called()


def test_canvas_slider_repaints_noop_set_after_deferred_range_change() -> None:
    """configure(range) with require_redraw=False defers the repaint to the
    next set() — including a no-op set (same value). This is the startup
    stale-render fix: get pins 100 on 50..150, info reconfigures to 70..100
    with the same 100, and without the dirty flag the canvas would keep
    drawing the construction range forever."""
    slider = CanvasSlider.__new__(CanvasSlider)
    slider._from = 50.0
    slider._to = 150.0
    slider._steps = 100
    slider._value = 100.0
    slider._geometry_dirty = True
    slider._redraw = Mock()

    slider.set(100)

    assert slider._geometry_dirty is False
    slider._redraw.assert_called_once_with()


def test_canvas_slider_redraws_when_value_changes() -> None:
    slider = CanvasSlider.__new__(CanvasSlider)
    slider._from = 0.0
    slider._to = 100.0
    slider._steps = 100
    slider._value = 50.0
    slider._redraw = Mock()

    slider.set(51)

    assert slider._value == 51.0
    slider._redraw.assert_called_once_with()
