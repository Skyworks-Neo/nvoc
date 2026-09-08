# Build & Install

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

### System Requirements

- NVIDIA GPU + compatible driver (≥ 537)
- Windows 10/11 or Linux (nvidia-open-dkms / proprietary driver)
- Administrator privileges (Windows) or sudo (Linux) for overclocking write operations

### One-Stop Development Entry

The monorepo ships an `xtask` orchestrator; `cargo xtask` works from any workspace
directory (the Rust toolchain auto-installs via `rust-toolchain.toml`):

```bash
cargo xtask setup    # doctor + bootstrap: submodule, uv envs, pynvoc build, fix hints
cargo xtask ci       # local mirror of the non-GPU CI gate (fmt + clippy + ruff + tests)
cargo xtask build    # workspace build; --cuda 12|11|none selects the stressor generation
cargo xtask run gui  # or: tui | cli | stressor
```

`cargo xtask help` lists the full command surface. GPU-write tests are never run by xtask.

### Rust Toolchain

```bash
# Install Rust (recommended: rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Requires toolchain 1.95.0+
rustup default 1.95.0
```

The project uses Rust Edition 2024.

### Python Environment

Use [`uv`](https://docs.astral.sh/uv/) to manage the Python environment:

```bash
# Install uv
pip install uv
# or
curl -LsSf https://astral.sh/uv/install.sh | sh
```

### Clone Repository

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

### Build Auto-Optimizer (Rust CLI Core)

```bash
cd auto-optimizer
cargo build --release
```

Output: `target/release/nvoc-auto-optimizer` (or `.exe`)

> Build all workspace Rust crates (excluding CUDA stress test):
> ```bash
> cargo build --workspace --exclude cli-stressor-cuda-rs
> ```

### Build Stress Testing Tools

> The former Python/PyTorch CUDA stressor (`cli-stressor-cuda/`) was removed in #200.
> Use the Rust CUDA stressor (`cli-stressor-cuda-rs/`) instead.

#### OpenCL Edition (Python)

```bash
cd cli-stressor-opencl
uv sync
```

#### Rust CUDA Edition

No CUDA Toolkit is needed to build — `cudarc` loads the NVRTC/cuBLAS/cudart
libraries dynamically at runtime (NVIDIA driver + those runtime libraries are
required only to run):

```bash
cargo build --release -p cli-stressor-cuda-rs                                          # cuda12 (default, driver >= 536)
cargo build --release -p cli-stressor-cuda-rs --no-default-features --features cuda11  # R470-era drivers (CUDA 11.4 API surface)
cargo run --release -p cli-stressor-cuda-rs -- --duration 30
```

### Build the Python Bindings (pynvoc)

The GUI and TUI depend on `pynvoc`, a native extension built with maturin from
`nvoc-python/` (requires the Rust toolchain above; on Windows also the MSVC build tools):

```bash
cd nvoc-python
uv sync
uv run maturin develop --release
cd ..
```

### Run Frontends

#### GUI

```bash
cd gui
uv sync
uv run python main.py
```

The GUI auto-detects `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`.

#### TUI

```bash
cd tui
uv sync
uv run nvoc-tui
```

### Package as Executable

#### GUI (PyInstaller)

```powershell
cd gui
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

#### TUI (PyInstaller)

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

### Development & Test Commands

Development and test commands mirrored from the monorepo's `docs/wiki/Build-and-Test.md`.
The one-stop mirror of the non-GPU CI gate is `cargo xtask ci` (see above):

```bash
# Rust workspace build (excludes the CUDA-Rust stressor)
cargo build --workspace --exclude cli-stressor-cuda-rs

# Rust format check
cargo fmt --all -- --check

# Rust lint
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings

# Core (nvoc-core) tests
cargo test --package nvoc-core --all-targets

# Python format & lint (matches CI flags)
ruff format . --preview --check --output-format=github && ruff check . --output-format=github

# GUI tests
cd gui && uv sync && uv run pytest

# TUI tests
cd tui && uv sync && uv run pytest

# GUI run
cd gui && uv sync && uv run python main.py
```

**CI mapping**: mirror the same command families locally before opening a PR —
Rust build/lint/test first, then Python lint/tests for the projects you touched.

---

<a id="chinese"></a>

## 中文

### 系统要求

- NVIDIA GPU + 兼容驱动（≥ 537）
- Windows 10/11 或 Linux（nvidia-open-dkms / proprietary driver）
- 管理员权限（Windows）或 sudo（Linux）用于超频写入操作

### 一站式开发入口

monorepo 自带 `xtask` 编排器；`cargo xtask` 可在任意工作区目录使用（Rust 工具链由
`rust-toolchain.toml` 自动安装）：

```bash
cargo xtask setup    # 环境体检 + 引导：子模块、uv 环境、pynvoc 构建、逐项修复提示
cargo xtask ci       # 本地镜像非 GPU CI 门禁（fmt + clippy + ruff + 测试）
cargo xtask build    # workspace 构建；--cuda 12|11|none 选择压测器世代
cargo xtask run gui  # 也可以是：tui | cli | stressor
```

`cargo xtask help` 查看完整命令面。xtask 永远不会运行 GPU 写入路径测试。

### Rust 工具链

```bash
# 安装 Rust（推荐 rustup）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 要求 toolchain 1.95.0+
rustup default 1.95.0
```

项目使用 Rust Edition 2024。

### Python 环境

推荐使用 [`uv`](https://docs.astral.sh/uv/) 管理 Python 环境：

```bash
# 安装 uv
pip install uv
# 或
curl -LsSf https://astral.sh/uv/install.sh | sh
```

### 克隆仓库

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

### 构建 Auto-Optimizer（Rust CLI 核心）

```bash
cd auto-optimizer
cargo build --release
```

产物：`target/release/nvoc-auto-optimizer`（或 `.exe`）

> 构建 workspace 全部 Rust crate（不含 CUDA 压力测试）：
> ```bash
> cargo build --workspace --exclude cli-stressor-cuda-rs
> ```

### 构建压力测试工具

> 原 Python/PyTorch CUDA 压力测试工具（`cli-stressor-cuda/`）已在 #200 中移除。
> 请改用 Rust CUDA 压力测试工具（`cli-stressor-cuda-rs/`）。

#### OpenCL 版（Python）

```bash
cd cli-stressor-opencl
uv sync
```

#### Rust CUDA 版

**构建不需要 CUDA Toolkit**——`cudarc` 在运行时动态加载 NVRTC/cuBLAS/cudart 库
（NVIDIA 驱动与这些运行时库仅在运行压测时需要）：

```bash
cargo build --release -p cli-stressor-cuda-rs                                          # cuda12（默认，驱动 >= 536）
cargo build --release -p cli-stressor-cuda-rs --no-default-features --features cuda11  # R470 世代驱动（CUDA 11.4 API 面）
cargo run --release -p cli-stressor-cuda-rs -- --duration 30
```

### 构建 Python 绑定（pynvoc）

GUI 与 TUI 依赖 `pynvoc`——一个通过 maturin 从 `nvoc-python/` 构建的原生扩展
（需要上文的 Rust 工具链；Windows 上还需要 MSVC 构建工具）：

```bash
cd nvoc-python
uv sync
uv run maturin develop --release
cd ..
```

### 运行前端

#### GUI

```bash
cd gui
uv sync
uv run python main.py
```

GUI 会自动检测 `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`。

#### TUI

```bash
cd tui
uv sync
uv run nvoc-tui
```

### 打包为可执行文件

#### GUI（PyInstaller）

```powershell
cd gui
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

#### TUI（PyInstaller）

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

### 开发与测试命令

开发与测试命令与 monorepo 的 `docs/wiki/Build-and-Test.md` 保持一致。
非 GPU CI 门禁的一站式本地镜像是 `cargo xtask ci`（见上文）：

```bash
# Rust workspace 构建（不含 CUDA-Rust 压力测试）
cargo build --workspace --exclude cli-stressor-cuda-rs

# Rust 格式检查
cargo fmt --all -- --check

# Rust lint
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings

# 核心（nvoc-core）测试
cargo test --package nvoc-core --all-targets

# Python 格式与 lint（与 CI 参数一致）
ruff format . --preview --check --output-format=github && ruff check . --output-format=github

# GUI 测试
cd gui && uv sync && uv run pytest

# TUI 测试
cd tui && uv sync && uv run pytest

# GUI 运行
cd gui && uv sync && uv run python main.py
```

**CI 对应关系**：提交 PR 前请在本地运行相同的命令族——
先 Rust 构建/lint/测试，再针对改动过的项目运行 Python lint/测试。

---