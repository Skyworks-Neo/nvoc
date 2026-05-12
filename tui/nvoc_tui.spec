# -*- mode: python ; coding: utf-8 -*-
from pathlib import Path

from PyInstaller.utils.hooks import collect_data_files, collect_submodules

block_cipher = None
root = Path(SPECPATH).resolve()

packages = ("textual", "textual_plotext", "plotext")
hiddenimports = []
datas = []
for package in packages:
    hiddenimports.extend(collect_submodules(package))
    datas.extend(collect_data_files(package))

# Include nvoc_tui style assets (e.g., base.tcss) for runtime loading.
datas += [(str(root / "nvoc_tui" / "styles"), "nvoc_tui/styles")]

a = Analysis(
    ["nvoc_tui/__main__.py"],
    pathex=[str(root)],
    binaries=[],
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
)
pyz = PYZ(a.pure, cipher=block_cipher)
exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.zipfiles,
    a.datas,
    [],
    name="nvoc-tui",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
)
