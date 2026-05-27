# nvoc-python

> Native Python bindings for nvoc-core

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

This package provides a high-performance Python bridge to the underlying Rust functions using PyO3 and Maturin. It allows the Python UIs (like the GUI and TUI) to safely and efficiently control and monitor NVIDIA GPUs.

### Development Setup

`nvoc-python` relies on `maturin` to build the Rust extensions. Due to our workspace design, development tools are handled seamlessly by `uv`.

#### 1. Environment Synchronization

Ensure you are at the workspace root or inside the `nvoc-python` directory and synchronize your environment:

```bash
uv sync
```

*Note: The `[dependency-groups].dev` configuration automatically installs `maturin` to your virtual environment alongside formatting and testing tools.*

#### 2. Building the Native Extension

To compile the Rust code into a Python extension module (`.pyd` on Windows, `.so` on Linux), use:

```bash
uv run maturin develop
```

This will perform a debug build and install the bindings in editable mode. If you change any Rust code (`src/*.rs` or `Cargo.toml`), you must run `uv run maturin develop` again for those changes to be reflected in Python.

#### 3. Usage & Testing

Once built, the module can be imported anywhere in the workspace environment:

```python
import pynvoc._native

# Example usage
print(pynvoc._native.__file__)  # Should point to the generated .pyd
```

To run the Python tests for the bindings:

```bash
uv run pytest
```

### Troubleshooting

#### "Invalid NT Headers signature" when running PyInstaller
If you encounter this error during PyInstaller packaging downstream (e.g., in `gui/` or `tui/`), it means the `_native.pyd` binary file has been corrupted (often misidentified as text and mangled during Git operations or text editor auto-saves).

**Fix:** Simply re-run `uv run maturin develop` inside `nvoc-python` to cleanly regenerate the binary file.

#### ImportError: cannot import name 'Image' from 'PIL'
If Pillow or other libraries installed from PyPI become corrupted (e.g. from an aborted download or aggressive caching):

**Fix:** Force reinstall the package using `uv`:
```bash
uv sync --reinstall-package pillow
```

---

<a id="chinese"></a>

## 中文

本包提供了底层 Rust 函数（`nvoc-core`）的高性能 Python 桥接模块支持。它借助 PyO3 和 Maturin 构建，使得 Python 前端（如 GUI 和 TUI）能够安全、高效地控制和监控 NVIDIA GPU。

### 开发环境配置

`nvoc-python` 依赖 `maturin` 来构建 Rust 原生扩展模块。基于目前的工作区架构设计，开发工具可通过 `uv` 无缝管理。

#### 1. 环境同步

请确保处于工作区根目录或 `nvoc-python` 目录下，并同步您的环境：

```bash
uv sync
```

*注意：`[dependency-groups].dev` 的配置会自动将 `maturin` 连同其他测试、格式化工具安装到您的虚拟环境中。*

#### 2. 构建原生扩展模块

要将 Rust 代码编译为 Python 扩展模块（Windows 下为 `.pyd`，Linux 下为 `.so`），请运行：

```bash
uv run maturin develop
```

此命令将以 debug 模式编译并在当前环境中进行可编辑安装模式（editable mode）配置。如果您修改了 Rust 相关的代码（如 `src/*.rs` 或 `Cargo.toml`），必须再次运行 `uv run maturin develop` 才能让更改在 Python 中生效。

#### 3. 使用与测试

编译完成后，您可以在工作区的任何模块导入它：

```python
import pynvoc._native

# 验证调用
print(pynvoc._native.__file__)  # 应指向新生成的 .pyd 模块
```

运行绑定代码的 Python 测试：

```bash
uv run pytest
```

### 疑难解答

#### 运行 PyInstaller 时遇到 "Invalid NT Headers signature"
如果在下游模块（例如 `gui/` 或 `tui/`）打包时遇到此错误，意味着 `_native.pyd` 二进制文件已被损坏（通常是在 Git 操作或编辑器自动保存时被误认为是文本文件并被错误覆盖导致）。

**修复方法：** 只需在 `nvoc-python` 中重新运行 `uv run maturin develop` 重建该二进制文件。

#### 导入包时遇到 ImportError: cannot import name 'Image' from 'PIL'
如果 Pillow 或从 PyPI 安装的其他库本身出现了损坏（可能是下载中断或过度缓存引发）：

**修复方法：** 使用 `uv` 强制重新安装该包：
```bash
uv sync --reinstall-package pillow
```
