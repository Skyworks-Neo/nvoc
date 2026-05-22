# Copyright (C) 2026 Ajax Dong
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
import sys
from pathlib import Path

from nvoc_tui.config import ConfigStore
from nvoc_tui.models import repo_root


def test_imports_gui_config_on_first_run(tmp_path: Path) -> None:
    gui_path = tmp_path / "nvoc_gui_config.json"
    gui_path.write_text(
        """
        {
          "cli_exe_path": "/tmp/nvoc-auto-optimizer",
          "last_gpu_idx": "2"
        }
        """,
        encoding="utf-8",
    )

    store = ConfigStore(tmp_path)
    config = store.load()

    assert config.last_gpu_idx == 2
    assert (tmp_path / "nvoc_tui_config.json").is_file()


def test_persists_tui_config(tmp_path: Path) -> None:
    store = ConfigStore(tmp_path)
    config = store.load()
    config.last_gpu_idx = 1
    config.vfcurve.auto_refresh = True
    store.data = config
    store.save()

    reloaded = ConfigStore(tmp_path).load()

    assert reloaded.last_gpu_idx == 1
    assert reloaded.vfcurve.auto_refresh is True


def test_stale_autoscan_active_tab_falls_back_to_dashboard(tmp_path: Path) -> None:
    (tmp_path / "nvoc_tui_config.json").write_text(
        """
        {
          "ui": {
            "active_tab": "autoscan"
          }
        }
        """,
        encoding="utf-8",
    )

    config = ConfigStore(tmp_path).load()

    assert config.ui.active_tab == "dashboard"


def test_repo_root_uses_executable_dir_when_frozen(monkeypatch, tmp_path: Path) -> None:
    exe_path = tmp_path / "portable" / "nvoc-tui.exe"
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe_path))

    assert repo_root() == exe_path.parent
