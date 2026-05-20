# Repository Guidelines

## Project Structure & Module Organization

NVOC is a mixed Rust and Python monorepo for NVIDIA GPU overclocking and stress tooling. The root `Cargo.toml` workspace contains `nvoc-core/` for shared NVAPI/NVML APIs, `auto-optimizer/` for the main CLI, `srv/` for the Windows service layer, and `cli-stressor-cuda-rs/` for the Rust CUDA stressor. The root `uv` workspace contains `gui/`, `tui/`, `cli-stressor-cuda/`, and `cli-stressor-opencl/`. TUI source is in `tui/nvoc_tui/`, tests in `tui/tests/`, and styles in `tui/nvoc_tui/styles/`. Platform scripts and helpers live in `auto-optimizer/test/` and `auto-optimizer/systemd/`.

## Build, Test, and Development Commands

- `cargo build --workspace --exclude cli-stressor-cuda-rs`: build Rust crates that do not require CUDA linkage.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings`: run Rust lint checks used by CI.
- `cargo test --package nvoc-core --all-targets`: run non-GPU Rust core tests.
- `cd tui && uv sync && uv run pytest`: install TUI deps and run unit tests.
- `ruff format . --check` and `ruff check .`: verify Python formatting and linting.
- `cd gui && uv sync && uv run python main.py` or `cd tui && uv run nvoc-tui`: run frontends locally after building the optimizer.

## Coding Style & Naming Conventions

Rust uses edition 2024 with toolchain `1.95.0`; keep code `rustfmt` clean and resolve all clippy warnings. Python code should be Ruff-formatted, use 4-space indentation, `snake_case` functions and modules, and `PascalCase` classes. Keep Textual widget IDs stable unless controllers, tests, and config sync paths are updated. Prefer component-local helpers over cross-component shortcuts.

## Testing Guidelines

Keep tests close to the component changed. Use Rust integration tests in `*/tests/*.rs`; name GPU-dependent tests clearly and keep mutating GPU checks ignored or hardware-gated. Python tests use `pytest` with `test_*.py` naming. For CLI parsing, config migration, and controller argument changes, add focused TUI tests. State which checks ran and whether GPU hardware was available.

## Safety & Configuration Notes

Treat overclocking writes as high risk. Prefer read-only validation first, document backend assumptions, and keep recovery behavior visible.
