Confirmed true enough to fix: GUI code does not import `pynvoc`, and GUI dependency files do not declare it, so the hidden import was unnecessary packaging risk.

Changed [gui/nvoc_gui.spec](/data/worktrees/job-13/gui/nvoc_gui.spec) to remove `"pynvoc"` from `hiddenimports`.

Tests run:
- `cd gui && uv run python -m compileall main.py src`
- `cd gui && uvx ruff format . --check`
- `cd gui && uvx ruff check .`