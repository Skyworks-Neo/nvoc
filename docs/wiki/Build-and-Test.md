# Build and Test

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
  `ruff format . --preview --check --output-format=github && ruff check . --output-format=github`
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
