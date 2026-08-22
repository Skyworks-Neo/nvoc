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
import sys
import importlib

# ── Fail fast on Python 3.14+ (Tcl/Tk 9 zipfs) ──
# Python 3.14 ships Tcl/Tk 9.0 with the library data embedded in the DLL's
# zipfs virtual filesystem ('//zipfs:/lib/tcl/...') instead of real on-disk
# directories. PyInstaller's TclTkInfo cannot collect from zipfs and only
# emits warnings — the resulting exe bundles ZERO tcl/tk data files and dies
# at first launch with:
#   FileNotFoundError: Tcl data directory "..._MEI..._tcl_data" not found
# Build with Python 3.13 (Tcl/Tk 8.6, real tcl\tcl8.6\ dirs) until PyInstaller
# gains zipfs support.
if sys.version_info >= (3, 14):
    raise SystemExit(
        "ERROR: Python %d.%d detected. Tcl/Tk 9.0 embeds its library data in "
        "the DLL zipfs, which PyInstaller cannot bundle — the exe would fail "
        "at first launch with '_tcl_data not found'. Rebuild with Python "
        "3.13 (e.g. `uv venv -p 3.13 && uv pip install -r requirements.txt`)."
        % sys.version_info[:2]
    )

# ── Locate customtkinter assets automatically ──
ctk_path = os.path.dirname(importlib.import_module("customtkinter").__file__)

a = Analysis(
    ["main.py"],
    pathex=[],
    binaries=[],
    datas=[
        # Bundle customtkinter's themes / assets
        (ctk_path, "customtkinter"),
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
        "pynvoc",
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
