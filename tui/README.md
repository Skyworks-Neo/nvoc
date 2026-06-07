# NVOC-TUI

Terminal UI frontend for NVOC using the native `pynvoc` bindings.

License: Apache 2.0

[English](#english) | [中文](#中文)

<a id="english"></a>
## English

### Disclaimer

Code in this repo are mostly written by CodeX. Functionalities are NOT COMPLETE
as for now, use at your own risk.

### Features

- Dashboard polling for live GPU status
- Overclock and fan-control actions
- Static VF curve export/import/edit workflows with terminal plotting
- Output console for native operations

### Development

```bash
uv sync
uv run nvoc-tui
```

### Tests

```bash
uv run pytest
```

### Packaging a self-contained Linux binary

Prerequisites:

- Python 3.11 or higher
- Rust toolchain and a C build toolchain for the native `pynvoc` build
- NVIDIA driver libraries on the target machine at runtime

**Using uv (recommended):** Run the following from the repository root. It builds
a one-file PyInstaller executable at `tui/dist/nvoc-tui` with Python, `nvoc_tui`,
the TCSS styles, and `pynvoc` bundled into the binary.

```bash
cd tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
./dist/nvoc-tui
```

The binary is self-contained for Python code and Python/Rust package artifacts,
but it is still a Linux binary: build it on the oldest glibc target you intend to
support, and expect system libraries such as glibc and NVIDIA driver libraries to
come from the target host.

If the build environment cannot write to the workspace venv or Cargo target
directory, put those paths under `/tmp`:

```bash
UV_PROJECT_ENVIRONMENT=/tmp/nvoc-tui-venv \
UV_CACHE_DIR=/tmp/uv-cache \
CARGO_HOME=/tmp/cargo-home \
CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
```

[Back to top](#nvoc-tui)

<a id="中文"></a>
## 中文

### 免责声明

本仓库中的代码大多由 CodeX 编写。当前功能尚未完整实现，请自行评估风险。

### 功能

- 实时轮询 GPU 状态面板
- 超频与风扇控制操作
- 静态 VF 曲线导出 / 导入 / 编辑流程，并支持终端绘图
- 原生操作输出控制台

### 开发

```bash
uv sync
uv run nvoc-tui
```

### 测试

```bash
uv run pytest
```

### 打包自包含 Linux 可执行文件

前置条件：

- Python 3.11 或更高
- 用于构建原生 `pynvoc` 的 Rust 工具链和 C 构建工具链
- 目标机器运行时需要 NVIDIA 驱动库

**使用 uv（推荐）：** 从仓库根目录执行以下命令。它会生成单文件
PyInstaller 可执行文件 `tui/dist/nvoc-tui`，其中包含 Python、`nvoc_tui`、
TCSS 样式和 `pynvoc`。

```bash
cd tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
./dist/nvoc-tui
```

该二进制文件对 Python 代码以及 Python/Rust 包产物是自包含的，但它仍然
是 Linux 二进制文件：请在你要支持的最旧 glibc 目标环境中构建，并预期
glibc、NVIDIA 驱动库等系统库由目标主机提供。

如果构建环境不能写入 workspace venv 或 Cargo target 目录，可将这些路径
放到 `/tmp`：

```bash
UV_PROJECT_ENVIRONMENT=/tmp/nvoc-tui-venv \
UV_CACHE_DIR=/tmp/uv-cache \
CARGO_HOME=/tmp/cargo-home \
CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
```

[返回顶部](#nvoc-tui)
