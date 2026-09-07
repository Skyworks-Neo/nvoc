# Getting Started

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Get a working NVOC frontend running in a few minutes. Overclocking writes are high-risk: prefer read-only commands first and verify your recovery path before any write.

1. Clone the repository.

   ```bash
   git clone https://github.com/Skyworks-Neo/nvoc.git
   cd nvoc
   ```

   Initialize the `nvapi-rs` submodule at the commit pinned by this repository (required for building):

   ```bash
   git submodule update --init
   ```

   Alternatively, download a prebuilt release from the [GitHub Releases](https://github.com/Skyworks-Neo/nvoc/releases) page and verify it with `SHA256SUMS` and `gh attestation verify` before running it.

2. Install the Rust toolchain `1.95.0` (pinned by `rust-toolchain.toml`) and Python `uv`.
   For NVOC-GUI, use a Python interpreter with Tk support (`tkinter`);
   Linux may require `tk`, `python3-tk`, or `python3-tkinter`.

3. Build the optimizer first.

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

### Next steps

- [[Build-and-Install]] — full build, installation, and environment setup (rustup, uv, verification).
- [[CLI-Guide]] — NVOC-CLI usage and command families.
- [[GUI-Guide]] — desktop GUI walkthrough.
- [[TUI-Guide]] — terminal/SSH frontend walkthrough.

---

*Maintained from: `docs/wiki/Getting-Started.md`, `README.md`, `rust-toolchain.toml`, `gui/README.md`, `tui/README.md`.*

<a id="chinese"></a>

## 中文

几分钟内即可运行起一个 NVOC 前端。超频写入属于高风险操作：请优先使用只读命令，并在执行任何写入前确认恢复路径。

1. 克隆仓库。

   ```bash
   git clone https://github.com/Skyworks-Neo/nvoc.git
   cd nvoc
   ```

   按本仓库锁定的提交初始化 `nvapi-rs` 子模块（编译必需）：

   ```bash
   git submodule update --init
   ```

   也可以从 [GitHub Releases](https://github.com/Skyworks-Neo/nvoc/releases) 页面下载预编译版本，并在运行前使用 `SHA256SUMS` 和 `gh attestation verify` 完成校验。

2. 安装 Rust 工具链 `1.95.0`（由 `rust-toolchain.toml` 锁定）和 Python `uv`。
   NVOC-GUI 需要带 Tk 支持的 Python 解释器（`tkinter`）；
   Linux 上可能需要安装 `tk`、`python3-tk` 或 `python3-tkinter`。

3. 先编译 optimizer。

   ```bash
   cd auto-optimizer
   cargo build --release
   ```

4. 运行其中一个前端。

GUI：

```bash
cd gui
uv sync
uv run python main.py
```

TUI：

```bash
cd tui
uv sync
uv run nvoc-tui
```

### 后续步骤

- [[Build-and-Install]] — 完整的编译、安装与环境配置（rustup、uv、校验）。
- [[CLI-Guide]] — NVOC-CLI 用法与命令族。
- [[GUI-Guide]] — 桌面 GUI 使用指南。
- [[TUI-Guide]] — 终端 / SSH 前端使用指南。

---

*维护来源：`docs/wiki/Getting-Started.md`、`README.md`、`rust-toolchain.toml`、`gui/README.md`、`tui/README.md`。*
