# Repository Guidelines

## Project Structure & Module Organization

NVOC is a Rust/Python monorepo for NVIDIA GPU overclocking and stress tooling. Rust crates are `nvoc-core/`, `auto-optimizer/`, `srv/`, and `cli-stressor-cuda-rs/`. Python `uv` projects are `gui/`, `tui/`, `cli-stressor-cuda/`, and `cli-stressor-opencl/`. TUI code lives in `tui/nvoc_tui/`, tests in `tui/tests/`, and styles in `tui/nvoc_tui/styles/`. Platform helpers are in `auto-optimizer/test/` and `auto-optimizer/systemd/`.

## Build, Test, and Development Commands

- `cargo build --workspace --exclude cli-stressor-cuda-rs`: build Rust crates without CUDA linkage.
- `cargo fmt --all -- --check`: check Rust formatting.
- `cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings`: run Rust CI linting.
- `cargo test --package nvoc-core --all-targets`: run non-GPU Rust core tests.
- `cd tui && uv sync && uv run pytest`: run TUI unit tests.
- `ruff format . --check && ruff check .`: check Python format and lint.
- `cd gui && uv sync && uv run python main.py`: run the GUI.
- `cd tui && uv sync && uv run nvoc-tui`: run the TUI.

## Coding Style & Naming Conventions

Rust uses edition 2024 and toolchain `1.95.0`; keep `rustfmt` and clippy clean. Python uses Ruff, 4-space indentation, `snake_case` modules/functions, and `PascalCase` classes. Keep Textual widget IDs stable unless updating controllers, tests, and config sync together.

## Testing Guidelines

Keep tests near changed code. Rust integration tests use `*/tests/*.rs`; Python tests use `pytest` and `test_*.py`. Keep GPU-mutating tests ignored or hardware-gated. Report checks run and GPU availability.

## Commit & Pull Request Guidelines

Use short, imperative commit summaries, often with prefixes like `core: Added tests`, `fix(clippy): ...`, or `Fix #137`. PRs should name the component, behavior change, linked issues, and tests run.

## Safety & Configuration Notes

Treat overclocking writes as high risk. Prefer read-only validation first, document backend assumptions, and keep recovery behavior visible.
