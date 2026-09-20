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

# NOTE: requires PyInstaller >= 6.22 for Tcl/Tk 9 (Python 3.14) embedded
# data-archive support; older versions silently bundle zero tcl/tk files.

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
    # Trimmed after measuring the archive composition (96.4MB → these were
    # ~9MB of dead weight; importers verified via xref-nvoc_gui.html):
    #  - ssl: NOTHING in the collected graph imports it (empty "imported by"
    #    in the xref) — dropping it also drops _ssl.pyd + libcrypto-3.dll +
    #    libssl-3.dll (~7.8MB). The GUI does no networking.
    #  - jinja2/markupsafe: only pyparsing.diagram imports them, and
    #    pyparsing imports that lazily inside create_diagram() with a
    #    graceful "pip install pyparsing[diagrams]" fallback — we never call
    #    it (matplotlib renders no railroad diagrams).
    # numpy itself STAYS: our code doesn't use it, but matplotlib (VF curve)
    # hard-requires it, including numpy.libs' OpenBLAS DLLs (21MB — the
    # single largest package after matplotlib).
    #  - _hashlib: CPython's hashlib falls back to the built-in message
    #    digest modules when the OpenSSL accelerator is absent; the GUI does
    #    no crypto, and dropping it removes libcrypto-3.dll (6.2MB).
    excludes=["ssl", "jinja2", "pyparsing.diagram", "_hashlib",
              "PIL.AvifImagePlugin", "PIL._avif"],
    noarchive=False,
)

# PyInstaller 6's Analysis has no exclude_datas parameter (it is swallowed
# into **kwargs), so the dead mpl-data payload is filtered here instead —
# same intent as the excludes comment above, applied post-Analysis to the
# dest names (os separators normalized to /): STIX/CM fonts (mathtext
# alternatives — the default fontset is dejavusans), afm + pdfcorefonts
# metrics (PS/PDF backends only), sample_data and images (matplotlib's demo
# payload). NOTE LastResortHE-Regular.ttf is NOT cut: matplotlib 3.11's
# _get_font loads it unconditionally as the glyph fallback (a missing file
# kills every chart draw with FileNotFoundError).
def _keep_data(dest_name: str) -> bool:
    n = dest_name.replace("\\", "/")
    if not n.startswith("matplotlib/mpl-data/"):
        return True
    dead_prefixes = (
        "matplotlib/mpl-data/fonts/afm/",
        "matplotlib/mpl-data/fonts/pdfcorefonts/",
        "matplotlib/mpl-data/sample_data/",
        "matplotlib/mpl-data/images/",
    )
    if n.startswith(dead_prefixes):
        return False
    if n.startswith("matplotlib/mpl-data/fonts/ttf/"):
        base = n.rsplit("/", 1)[1]
        if base.startswith(("STIX", "cmr", "cmmi", "cmsy", "cmtt", "cmex",
                            "cmss", "cmb")):
            return False
    return True


a.datas = [entry for entry in a.datas if _keep_data(entry[0])]

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
