Implemented the GUI VF curve layout/packaging change.

Changed:
- Moved VF Export/Import/Reset controls to the top chart area in `gui/src/tabs/vfcurve.py`.
- Kept the visible CSV path field and `Use for I/O` behavior to avoid hidden path reuse.
- `Quick export` defaults to enabled.
- Added `pynvoc` to `gui/nvoc_gui.spec` hidden imports.
- Removed missing `nvoc_gui_config.json` from PyInstaller datas.

Checks run:
- `cd gui && uv run python -m compileall main.py src`
- `cd gui && uvx ruff format . --check`
- `cd gui && uvx ruff check .`

All passed.