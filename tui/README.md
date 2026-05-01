# NVOC-TUI

Terminal UI frontend for `nvoc-auto-optimizer`.

License: Apache 2.0

## 配套产品——使用所有配套产品以达到最好体验

[NVOC-AUTO-OPTIMIZER](https://github.com/Skyworks-Neo/nvoc-auto-optimizer)：核心模块。

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor)：压力测试模块，用于自动超频扫描部分。没有该模块仍可以使用自动扫描之外的所有功能。（nvoc-auto-optimizer开放任何你的自定义压力测试模块接入，只需满足return
code定义即可。）

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI)：跨平台超频图形界面，直接对标MSI Afterburner。 （为了避免GPU超炸带走图形界面，使用CPU渲染，在低端机器如遇到性能问题，建议使用NVOC-TUI）；

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI)：跨平台超频命令行界面，用于没有图形界面的机器，兼容性好，性能要求低；

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV)：client-server架构控制模块，用于机房、服务器、工作站等场景的 Web 管理、~~远程超频~~（TODO）

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

