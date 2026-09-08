# Build and Test

## One-stop entry (recommended)

- First-time setup (doctor + bootstrap: submodule, uv envs, pynvoc build):
  `cargo xtask setup` (add `--dry-run` to preview, `--install-missing` to auto-install uv)
- Local CI mirror (rustfmt + clippy + ruff + safe-tier tests):
  `cargo xtask ci` (`cargo xtask ci --fmt` force-applies fmt/lint fixes before the gate)
- Build the workspace (CUDA stressor generation selectable):
  `cargo xtask build [--release] [--cuda 12|11|none]`
- Run a component from source:
  `cargo xtask run gui|tui|cli|stressor`
- Full command surface: `cargo xtask help`

## Rust workspace

- Build (exclude CUDA-Rust stressor):
  `cargo build --workspace --exclude cli-stressor-cuda-rs`
- Format check:
  `cargo fmt --all -- --check`
- Lint:
  `cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings`
- Core tests:
  `cargo test --package nvoc-core --all-targets`

## Python projects

- Lint/format:
  `ruff format . --check --output-format=github && ruff check . --output-format=github`
- GUI tests:
  `cd gui && uv sync && uv run pytest`
- TUI tests:
  `cd tui && uv sync && uv run pytest`
- GUI run:
  `cd gui && uv sync && uv run python main.py`

## CI mapping

Mirror the same command families locally before PR: Rust build/lint/test, then Python lint/tests for changed projects.

---

*Maintained from: `AGENTS.md`, project `pyproject.toml` files, CI workflow files.*
