# NVOC-TUI

Terminal UI frontend for `nvoc-auto-optimizer`.

License: Apache 2.0

[English](#english) | [中文](#中文)

<a id="english"></a>
## English

### Disclaimer

Code in this repo are mostly written by CodeX. Functionalities are NOT COMPLETE
as for now, use at your own risk. The Dashboard page and VF Curve page are
mostly tested, while Autoscan and Overclock page is NOT TESTED.

### Features

- Dashboard polling for live GPU status
- Autoscan workflow management
- Overclock and fan-control actions
- VF curve export/import/edit workflows with terminal plotting
- Streaming output console for CLI operations

### Development

```bash
uv sync
uv run nvoc-tui
```

### Tests

```bash
uv run pytest
```

[Back to top](#nvoc-tui)

<a id="中文"></a>
## 中文

### 免责声明

本仓库中的代码大多由 CodeX 编写。当前功能尚未完整实现，请自行评估风险。
其中 Dashboard 页面和 VF Curve 页面经过了较多测试，而 Autoscan 和 Overclock 页面尚未充分测试。

### 功能

- 实时轮询 GPU 状态面板
- 自动扫描工作流管理
- 超频与风扇控制操作
- VF 曲线导出 / 导入 / 编辑流程，并支持终端绘图
- CLI 操作的流式输出控制台

### 开发

```bash
uv sync
uv run nvoc-tui
```

### 测试

```bash
uv run pytest
```

[返回顶部](#nvoc-tui)
