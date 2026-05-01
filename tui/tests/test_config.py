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
from pathlib import Path

from nvoc_tui.config import ConfigStore


def test_imports_gui_config_on_first_run(tmp_path: Path) -> None:
    gui_path = tmp_path / "nvoc_gui_config.json"
    gui_path.write_text(
        """
        {
          "cli_exe_path": "/tmp/nvoc-auto-optimizer",
          "last_gpu_idx": "2",
          "autoscan": {
            "mode": "ultrafast",
            "test_exe": "test.bat"
          }
        }
        """,
        encoding="utf-8",
    )

    store = ConfigStore(tmp_path)
    config = store.load()

    assert config.cli.exe_path == "/tmp/nvoc-auto-optimizer"
    assert config.last_gpu_idx == 2
    assert config.autoscan.mode == "ultrafast"
    assert config.autoscan.test_exe == "test.bat"
    assert (tmp_path / "nvoc_tui_config.json").is_file()


def test_persists_tui_config(tmp_path: Path) -> None:
    store = ConfigStore(tmp_path)
    config = store.load()
    config.cli.exe_path = "/tmp/tool"
    config.last_gpu_idx = 1
    config.autoscan.mode = "legacy"
    config.vfcurve.auto_refresh = True
    store.data = config
    store.save()

    reloaded = ConfigStore(tmp_path).load()

    assert reloaded.cli.exe_path == "/tmp/tool"
    assert reloaded.last_gpu_idx == 1
    assert reloaded.autoscan.mode == "legacy"
    assert reloaded.vfcurve.auto_refresh is True
