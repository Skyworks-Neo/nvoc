# Frontends

## GUI

- Path: `gui/`
- Start: `cd gui && uv sync && uv run python main.py`
- Use case: desktop operational control with visual tabs and live output.

## TUI

- Path: `tui/`
- Start: `cd tui && uv sync && uv run nvoc-tui`
- Use case: terminal-first operation, remote/SSH friendly.

## SRV

- Path: `srv/`
- Use case: service lifecycle and localhost HTTP control in managed environments.

## Config & CLI discovery

Frontends should explicitly resolve `auto-optimizer` executable path/config and expose it in settings to avoid hidden path failures.

---

*Maintained from: `gui/README.md`, `tui/README.md`, `srv/README.md`, frontend config sources.*
