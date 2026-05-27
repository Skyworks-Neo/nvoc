# 编译与安装

## 系统要求

- NVIDIA GPU + 兼容驱动（≥ 537）
- Windows 10/11 或 Linux（nvidia-open-dkms / proprietary driver）
- 管理员权限（Windows）或 sudo（Linux）用于超频写入操作

## Rust 工具链

```bash
# 安装 Rust（推荐 rustup）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 要求 toolchain 1.95.0+
rustup default 1.95.0
```

项目使用 Rust Edition 2024。

## Python 环境

推荐使用 [`uv`](https://docs.astral.sh/uv/) 管理 Python 环境：

```bash
# 安装 uv
pip install uv
# 或
curl -LsSf https://astral.sh/uv/install.sh | sh
```

## 克隆仓库

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

## 构建 Auto-Optimizer（Rust CLI 核心）

```bash
cd auto-optimizer
cargo build --release
```

产物：`target/release/nvoc-auto-optimizer`（或 `.exe`）

> 构建 workspace 全部 Rust crate（不含 CUDA 压力测试）：
> ```bash
> cargo build --workspace --exclude cli-stressor-cuda-rs
> ```

## 构建压力测试工具

### CUDA 版（Python + PyTorch）

```bash
cd cli-stressor-cuda
uv sync
# 或手动安装
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu129
pip install numpy
```

### OpenCL 版（Python）

```bash
cd cli-stressor-opencl
uv sync
```

### Rust CUDA 版

需要安装 CUDA Toolkit：

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30
```

## 运行前端

### GUI

```bash
cd gui
uv sync
uv run python main.py
```

GUI 会自动检测 `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`。

### TUI

```bash
cd tui
uv sync
uv run nvoc-tui
```

## 打包为可执行文件

### GUI（PyInstaller）

```powershell
cd gui
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

### TUI（PyInstaller）

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

## 开发检查

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings
cargo test --package nvoc-core --all-targets

# Python TUI
cd tui && uv run pytest

# Python 格式与 lint
ruff format . --check && ruff check .
```
