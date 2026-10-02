from __future__ import annotations

import os

from src.backend.native import (
    NVAPI_PATH_ENV,
    NVML_PATH_ENV,
    apply_native_paths,
)
from src.config import DEFAULT_CONFIG, Config


def test_default_config_has_native_path_keys() -> None:
    assert DEFAULT_CONFIG["nvapi_lib_path"] == ""
    assert DEFAULT_CONFIG["nvml_lib_path"] == ""


def test_config_merge_fills_native_path_defaults(tmp_path) -> None:
    config = Config(str(tmp_path))
    try:
        assert config.get("nvapi_lib_path") == ""
        assert config.get("nvml_lib_path") == ""
    finally:
        config.close()


def test_apply_native_paths_writes_env(monkeypatch) -> None:
    monkeypatch.delenv(NVAPI_PATH_ENV, raising=False)
    monkeypatch.delenv(NVML_PATH_ENV, raising=False)

    apply_native_paths(" /opt/nvapi ", "")

    assert os.environ[NVAPI_PATH_ENV] == "/opt/nvapi"
    assert NVML_PATH_ENV not in os.environ


def test_apply_native_paths_override_beats_preset(monkeypatch) -> None:
    monkeypatch.setenv(NVAPI_PATH_ENV, "/preset/api")

    apply_native_paths("/flag/api", None, override=True)

    assert os.environ[NVAPI_PATH_ENV] == "/flag/api"


def test_apply_native_paths_respects_existing_env_when_not_overriding(
    monkeypatch,
) -> None:
    monkeypatch.setenv(NVAPI_PATH_ENV, "/preset/api")
    monkeypatch.delenv(NVML_PATH_ENV, raising=False)

    apply_native_paths("/config/api", "/config/ml", override=False)

    assert os.environ[NVAPI_PATH_ENV] == "/preset/api"
    assert os.environ[NVML_PATH_ENV] == "/config/ml"
