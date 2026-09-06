# -*- mode: python ; coding: utf-8 -*-
from pathlib import Path

from PyInstaller.utils.hooks import collect_data_files, collect_submodules, copy_metadata

block_cipher = None
root = Path(SPECPATH).resolve()

packages = ("textual", "textual_plotext", "plotext", "pynvoc")
hiddenimports = []
datas = []
for package in packages:
    hiddenimports.extend(collect_submodules(package))
    # pynvoc: skip data collection - it swept the Rust build's 5.3MB
    # _native.pdb debug symbols into the exe. The extension module itself
    # arrives via the hiddenimports entry above.
    if package == "pynvoc":
        continue
    datas.extend(collect_data_files(package))

# Preserve the distribution metadata used by nvoc_tui.__version__.
datas.extend(copy_metadata("nvoc-tui"))

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
    # Trimmed after measuring the archive composition (81.8MB payload):
    #  - PIL: 13.4MB pulled in by plotext's static imports. The TUI never
    #    uses plotext's image plotting (no image_plot calls in nvoc_tui),
    #    and plotext imports PIL lazily inside those functions only -
    #    verified: `import plotext` leaves PIL out of sys.modules.
    #  - ssl + _hashlib + libcrypto-3: the TUI does no networking/crypto.
    #    ssl's only importer chain was aiohttp (textual devtools client).
    #  - aiohttp + msgpack + textual.dev: collect_submodules("textual")
    #    blindly swept the devtools client stack; the TUI never connects
    #    to a textual devtools server.
    # numpy + numpy.libs' OpenBLAS (21MB) STAY: plotext (VF curve) is a
    # hard numpy dependency.
    excludes=[
        "PIL",
        "ssl",
        "_hashlib",
        "aiohttp",
        "msgpack",
        "textual.dev",
    ],
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
