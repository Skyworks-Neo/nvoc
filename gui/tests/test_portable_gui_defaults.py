from __future__ import annotations

from src.config import DEFAULT_CONFIG
from src.widgets.output_console import OutputConsole


def test_autoscan_defaults_use_portable_path_separators() -> None:
    autoscan = DEFAULT_CONFIG["autoscan"]

    assert isinstance(autoscan, dict)
    assert autoscan["output_csv"] == "./ws/vfp-tem.csv"
    assert autoscan["init_csv"] == "./ws/vfp-init.csv"


def test_console_uses_native_fixed_font_off_windows(monkeypatch) -> None:
    class FakeFont:
        def cget(self, key: str):
            return {"family": "DejaVu Sans Mono", "size": 9}[key]

    monkeypatch.setattr("src.widgets.output_console.sys.platform", "linux")
    monkeypatch.setattr(
        "src.widgets.output_console.tk_font.nametofont",
        lambda name: FakeFont() if name == "TkFixedFont" else None,
    )

    assert OutputConsole._mono_font() == ("DejaVu Sans Mono", 10)


def test_console_keeps_consolas_on_windows(monkeypatch) -> None:
    monkeypatch.setattr("src.widgets.output_console.sys.platform", "win32")

    assert OutputConsole._mono_font() == ("Consolas", 12)
