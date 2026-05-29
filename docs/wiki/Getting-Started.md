# Getting Started

1. Clone repository.
   ```bash
   git clone https://github.com/Skyworks-Neo/nvoc.git
   cd nvoc
   ```
2. Install Rust toolchain `1.95.0` and Python `uv`.
   For NVOC-GUI, use a Python interpreter with Tk support (`tkinter`);
   Linux may require `tk`, `python3-tk`, or `python3-tkinter`.
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
