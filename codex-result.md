Implemented the GUI VF curve layout/packaging change.

Changed:
- Moved the VF curve Export/Import/Reset controls from the scrollable lower panel into the top chart area in [vfcurve.py](/data/worktrees/job-10/gui/src/tabs/vfcurve.py).
- Kept the visible CSV path controls to avoid the hidden-path reuse regression.
- Kept quick export default enabled.
- Added `pynvoc` to PyInstaller `hiddenimports` in [nvoc_gui.spec](/data/worktrees/job-10/gui/nvoc_gui.spec).

Test run:
- `cd gui && uv run python -m compileall main.py src` passed.

I did not run a full PyInstaller build.