from __future__ import annotations

from typing import Any

import pytest

import main


def test_require_gui_runtime_reports_missing_tkinter() -> None:
    def import_module(name: str) -> Any:
        if name == "tkinter":
            raise ModuleNotFoundError("No module named 'tkinter'", name="tkinter")
        return object()

    with pytest.raises(main.GuiStartupError) as excinfo:
        main._require_gui_runtime(import_module)

    message = str(excinfo.value)
    assert "requires Python Tk support" in message
    assert 'python -c "import tkinter"' in message


def test_require_gui_runtime_reports_missing_customtkinter() -> None:
    def import_module(name: str) -> Any:
        if name == "customtkinter":
            raise ModuleNotFoundError(
                "No module named 'customtkinter'",
                name="customtkinter",
            )
        return object()

    with pytest.raises(main.GuiStartupError) as excinfo:
        main._require_gui_runtime(import_module)

    assert "customtkinter" in str(excinfo.value)
    assert "uv sync" in str(excinfo.value)
