# Getting Started

1. Clone repository.
   ```bash
   git clone https://github.com/Skyworks-Neo/nvoc.git
   cd nvoc
   ```
2. Install Rust toolchain `1.95.0` and Python `uv`.
3. Build optimizer first.
   ```bash
   cd auto-optimizer
   cargo build --release
   ```
4. Run one frontend.

GUI:
```bash
cd gui
uv sync
uv run python main.py
```

TUI:
```bash
cd tui
uv sync
uv run nvoc-tui
```

---

*Maintained from: `README.md`, `rust-toolchain.toml`, `gui/README.md`, `tui/README.md`.*
