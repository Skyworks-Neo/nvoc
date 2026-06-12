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

### Packaging a self-contained executable

Prerequisites:

- Python 3.11 or higher
- Rust toolchain and a C build toolchain for the native `pynvoc` build
- NVIDIA driver libraries on the target machine at runtime

**Using uv (recommended):** Run the following from the repository root on the
target OS. PyInstaller produces OS-specific executables, so build on Windows for
`tui\dist\nvoc-tui.exe` and on Linux for `tui/dist/nvoc-tui`. Both builds bundle
Python, `nvoc_tui`, the TCSS styles, and `pynvoc` into a one-file executable.

```bash
cd tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
./dist/nvoc-tui
```

```powershell
Set-Location tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
.\dist\nvoc-tui.exe
```

The executable is self-contained for Python code and Python/Rust package
artifacts, but PyInstaller does not cross-compile or bundle platform system
libraries. Build on the target OS. For Linux, build on the oldest glibc target
you intend to support; for Windows, build with a matching Python architecture and
Windows Rust/C toolchain. NVIDIA driver libraries still come from the target
host.

If a Linux build environment cannot write to the workspace venv or Cargo target
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

### 打包自包含可执行文件

前置条件：

- Python 3.11 或更高
- 用于构建原生 `pynvoc` 的 Rust 工具链和 C 构建工具链
- 目标机器运行时需要 NVIDIA 驱动库

**使用 uv（推荐）：** 在目标操作系统上从仓库根目录执行以下命令。
PyInstaller 会生成对应平台的可执行文件：在 Windows 上构建会得到
`tui\dist\nvoc-tui.exe`，在 Linux 上构建会得到 `tui/dist/nvoc-tui`。
两者都会将 Python、`nvoc_tui`、TCSS 样式和 `pynvoc` 打包进单文件
可执行文件。

```bash
cd tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
./dist/nvoc-tui
```

```powershell
Set-Location tui
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
.\dist\nvoc-tui.exe
```

该可执行文件对 Python 代码以及 Python/Rust 包产物是自包含的，但
PyInstaller 不会交叉编译，也不会打包平台系统库。请在目标操作系统上
构建。Linux 版本应在你要支持的最旧 glibc 目标环境中构建；Windows
版本应使用匹配架构的 Python 以及 Windows Rust/C 工具链构建。NVIDIA
驱动库仍由目标主机提供。

如果 Linux 构建环境不能写入 workspace venv 或 Cargo target 目录，可将
这些路径放到 `/tmp`：

```bash
UV_PROJECT_ENVIRONMENT=/tmp/nvoc-tui-venv \
UV_CACHE_DIR=/tmp/uv-cache \
CARGO_HOME=/tmp/cargo-home \
CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
```

[返回顶部](#nvoc-tui)
