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
    store.data = config
    store.save()

    reloaded = ConfigStore(tmp_path).load()

    assert reloaded.cli.exe_path == "/tmp/tool"
    assert reloaded.last_gpu_idx == 1
    assert reloaded.autoscan.mode == "legacy"
