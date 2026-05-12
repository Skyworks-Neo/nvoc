# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller spec for the OpenCL branch.

This spec currently targets the repository entry script `test.py`.
If you later split the OpenCL branch into a dedicated launcher module,
update `entry_script` below before building.
"""

from pathlib import Path

project_root = Path.cwd().resolve()
entry_script = project_root / "test.py"

a = Analysis(
    [str(entry_script)],
    pathex=[str(project_root)],
    binaries=[],
    datas=[],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
)

pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="NVOC-CLI-Stressor-opencl",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=True,
)


