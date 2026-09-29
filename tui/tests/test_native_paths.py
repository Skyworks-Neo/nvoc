from __future__ import annotations

import json
import os
from pathlib import Path

from nvoc_tui.__main__ import _build_parser
from nvoc_tui.config import ConfigStore
from nvoc_tui.models import NativeSettings
from nvoc_tui.native import (
    NVAPI_PATH_ENV,
    NVML_PATH_ENV,
    apply_native_paths,
)


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


def test_parser_library_path_flags() -> None:
    args = _build_parser().parse_args(["--nvapi-path", "/a", "--nvml-path", "/b"])
    assert args.nvapi_path == "/a"
    assert args.nvml_path == "/b"

    defaults = _build_parser().parse_args([])
    assert defaults.nvapi_path is None
    assert defaults.nvml_path is None


def test_config_round_trips_native_paths(tmp_path: Path) -> None:
    store = ConfigStore(tmp_path)
    config = store.load()
    config.native = NativeSettings(nvapi_path="/opt/api", nvml_path="/opt/ml")
    store.data = config
    store.save()

    reloaded = ConfigStore(tmp_path).load()

    assert reloaded.native.nvapi_path == "/opt/api"
    assert reloaded.native.nvml_path == "/opt/ml"


def test_gui_config_fallback_carries_native_paths(tmp_path: Path) -> None:
    gui_path = tmp_path / "nvoc_gui_config.json"
    gui_path.write_text(
        json.dumps({"nvapi_lib_path": "/gui/api", "nvml_lib_path": "/gui/ml"}),
        encoding="utf-8",
    )

    config = ConfigStore(tmp_path).load()

    assert config.native.nvapi_path == "/gui/api"
    assert config.native.nvml_path == "/gui/ml"
