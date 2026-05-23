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

### Packaging with PyInstaller

Prerequisites:

- Python 3.11 or higher

**Using uv (recommended):** Run the following from the repository root — it installs the build
dependencies into the project environment and builds the executable:

```powershell
Set-Location tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

**Using pip:** Activate a Python 3.11+ virtual environment inside the `tui` directory, then:

```bash
pip install "pyinstaller~=6.0"
pyinstaller --clean --noconfirm nvoc_tui.spec
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

### 使用 PyInstaller 打包

前置条件：

- Python 3.11 或更高

**使用 uv（推荐）：** 从仓库根目录执行以下命令——自动安装构建依赖并生成可执行文件：

```powershell
Set-Location tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

**使用 pip：** 在 `tui` 目录下激活 Python 3.11+ 虚拟环境，然后运行：

```bash
pip install "pyinstaller~=6.0"
pyinstaller --clean --noconfirm nvoc_tui.spec
```

[返回顶部](#nvoc-tui)
