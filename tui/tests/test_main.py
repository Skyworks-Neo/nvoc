from __future__ import annotations

import importlib
import os

from nvoc_tui import __main__


BLAS_THREAD_ENV_VARS = (
    "OPENBLAS_NUM_THREADS",
    "OMP_NUM_THREADS",
    "MKL_NUM_THREADS",
)


def test_main_caps_unconfigured_blas_thread_pools(monkeypatch) -> None:
    for name in BLAS_THREAD_ENV_VARS:
        monkeypatch.delenv(name, raising=False)

    importlib.reload(__main__)

    assert {name: os.environ[name] for name in BLAS_THREAD_ENV_VARS} == {
        name: "1" for name in BLAS_THREAD_ENV_VARS
    }


def test_main_preserves_explicit_blas_thread_configuration(monkeypatch) -> None:
    for name in BLAS_THREAD_ENV_VARS:
        monkeypatch.setenv(name, "4")

    importlib.reload(__main__)

    assert {name: os.environ[name] for name in BLAS_THREAD_ENV_VARS} == {
        name: "4" for name in BLAS_THREAD_ENV_VARS
    }
