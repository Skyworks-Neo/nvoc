# -*- mode: python ; coding: utf-8 -*-
"""
PyInstaller spec file for NVOC-GUI.
Usage:  pyinstaller nvoc_gui.spec
        pyinstaller --clean nvoc_gui.spec  (for clean rebuild)

Optimizations:
  - noarchive=True: Faster startup, modular build
  - excludes: Remove unused modules to speed up analysis
  - datas: Only include necessary assets
"""

import os
import importlib

# ── Locate customtkinter assets automatically ──
ctk_path = os.path.dirname(importlib.import_module("customtkinter").__file__)

a = Analysis(
    ["main.py"],
    pathex=[],
    binaries=[],
    datas=[
        # Bundle customtkinter's themes / assets
        (ctk_path, "customtkinter"),
        # Bundle GUI config template (optional, will be created at runtime if missing)
        ("nvoc_gui_config.json", "."),
    ],
    hiddenimports=[
        "customtkinter",
        "darkdetect",
        "packaging",
        "packaging.version",
        "packaging.requirements",
        "matplotlib",
        "matplotlib.backends.backend_tkagg",
        "numpy",
        "pystray",
        "pystray._win32",
        "PIL",
        "PIL.Image",
        "PIL.ImageDraw",
        "PIL.ImageFont",
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
)

pyz = PYZ(a.pure, cipher=None)  # cipher=None: Faster build (no encryption overhead)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="NVOC-GUI",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,  # ✓ OPTIMIZATION: Disable UPX (very slow, minimal size gain ~5-10%)
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,          # 保留控制台窗口方便调试; 发布时改为 False
    icon=[os.path.join(ctk_path, "assets", "icons", "CustomTkinter_icon_Windows.ico")],
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    uac_admin=True,
)
