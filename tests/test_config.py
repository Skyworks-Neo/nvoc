# Copyright (C) 2026 Ajax Dong
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program. If not, see <https://www.gnu.org/licenses/>.
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
