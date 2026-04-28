# NVOC-TUI

Terminal UI frontend for `nvoc-auto-optimizer`.

# DISCLAIMER

Code in this repo are mostly written by CodeX. Functionalities are NOT COMPLETE
as for now, use at your own risk. The Dashboard page and VF Curve page are
mostly tested, while Autoscan and Overclock page is NOT TESTED.

## Features

- Dashboard polling for live GPU status
- Autoscan workflow management
- Overclock and fan-control actions
- VF curve export/import/edit workflows with terminal plotting
- Streaming output console for CLI operations

## Development

```bash
uv sync
uv run nvoc-tui
```

## Tests

```bash
uv run pytest
```
