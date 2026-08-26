from __future__ import annotations

import importlib
import os
from typing import Any

import pytest

import main


BLAS_THREAD_ENV_VARS = (
    "OPENBLAS_NUM_THREADS",
    "OMP_NUM_THREADS",
    "MKL_NUM_THREADS",
)


def test_main_caps_unconfigured_blas_thread_pools(monkeypatch) -> None:
    for name in BLAS_THREAD_ENV_VARS:
        monkeypatch.delenv(name, raising=False)

    importlib.reload(main)

    assert {name: os.environ[name] for name in BLAS_THREAD_ENV_VARS} == {
        name: "1" for name in BLAS_THREAD_ENV_VARS
    }


def test_main_preserves_explicit_blas_thread_configuration(monkeypatch) -> None:
    for name in BLAS_THREAD_ENV_VARS:
        monkeypatch.setenv(name, "4")

    importlib.reload(main)

    assert {name: os.environ[name] for name in BLAS_THREAD_ENV_VARS} == {
        name: "4" for name in BLAS_THREAD_ENV_VARS
    }


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
