# GUI 使用指南

NVOC-GUI 是基于 Python 的图形界面前端，提供 Dashboard、Autoscan、Overclock、VF Curve、Fan Control 等功能。

## 运行

```bash
cd gui
uv sync
uv run python main.py
```

GUI 会自动检测 `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`。

## 功能页签

### Dashboard

- GPU 信息与实时状态（频率、温度、风扇、VFP）
- 当前超频设置一览

<img width="1156" height="1051" alt="image" src="https://github.com/user-attachments/assets/7cfefc10-47e3-40aa-aeeb-f7d37f527c6f" width="50%"/>

### Autoscan

一键自动优化 VF 曲线工作流：

1. 点击 **Export Init VFP** 保存出厂曲线
2. 点击 **Reset & Unlock VFP** 准备扫描
3. 配置参数（模式、分数阈值等）
4. 点击 **Start Autoscan** — 输出实时流式显示到控制台
5. 扫描完成后点击 **Fix Results** 后处理
6. 点击 **Import Final VFP** 应用优化曲线

支持 Standard / Ultrafast / Legacy 三种模式。

<img width="1144" height="1032" alt="image" src="https://github.com/user-attachments/assets/5b7ca629-e6b2-4bab-a586-41bc50670cb9" />


### Overclock

- 核心频率偏移（`--core-offset`）
- 显存频率偏移（`--mem-offset`）
- 功耗墙（`-P`）
- 温度墙（`-T`）
- Voltage Boost（`-V`）
- 手动风扇转速控制
- 预设方案与滑块调节
- NVAPI / NVML 双接口支持

<img width="1154" height="1044" alt="image" src="https://github.com/user-attachments/assets/debbe0de-25e7-42d6-811c-6c2670cc7bd5" />


### VF Curve

- 导出 / 导入 VFP 曲线（CSV 格式）
- 锁定 / 解锁电压点
- 单点频率调整
- 终端绘图预览

<img width="1143" height="1028" alt="image" src="https://github.com/user-attachments/assets/e7f79884-abbd-4ea6-9a54-9584e0c2f20e" />

### Output Console

- 所有操作的 CLI 输出实时展示
- 停靠式窗口，可滚动查看历史输出

## 架构

```
main.py → src/app.py → src/tabs/*
                     → src/widgets/*
         → src/config.py
         → src/cli_runner.py → nvoc-auto-optimizer CLI
```

- **`src/cli_runner.py`**：以子进程方式调用 CLI，传递参数并解析输出
- **`src/config.py`**：JSON 格式配置管理
- **`src/tabs/`**：每个文件对应一个功能页签

## 配置

- 配置文件：`nvoc_gui_config.json`
- 可自定义 CLI 路径、GPU 选择等

## 打包

```powershell
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```
