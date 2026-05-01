# NVOC-GUI

> GUI frontend for NVOC-AutoOptimizer — NVIDIA GPU VF Curve Optimizer

## License

This project is licensed under the [Apache License 2.0](LICENSE).

## Features

- **Dashboard** — View GPU info, status (clocks, sensors, coolers, VFP), and current OC settings
- **Autoscan** — One-click VF curve auto-optimization with Standard / Ultrafast / Legacy modes
- **Overclock** — Apply clock offsets, power limits, thermal limits, and voltage boost
- **VF Curve** — Export/import VFP curves (CSV), lock/unlock voltage points, single-point adjustments
- **Fan Control** — Manual fan speed control with presets and slider
- **Output Console** — Real-time CLI output display for all operations

## Requirements

- Python 3.8+
- Windows 10/11 (64-bit)
- NVOC-AutoOptimizer built (`../NVOC-Autooptimizer/target/release/nvoc-auto-optimizer.exe`)
- NVIDIA GPU with compatible drivers

## Setup

```bash
uv sync
uv run python main.py
```

## Compile Single Executable (Optional)

```bash
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

## Dependency Management

This repo uses `uv` with `pyproject.toml` instead of `requirements.txt`.

- Runtime dependencies use relaxed version ranges.
- Packages with changing Python support windows use Python-version markers, so each interpreter resolves an appropriate release line.
- Build-only tooling lives in the `build` dependency group.
- `uv sync` resolves for the active interpreter. To verify Python 3.8 compatibility specifically, create/sync a Python 3.8 environment first.

## Directory Structure

```
NVOC-GUI/
├── main.py                     # Entry point
├── pyproject.toml              # Project metadata and dependencies
├── src/
│   ├── app.py                  # Main application window
│   ├── cli_runner.py           # Subprocess wrapper for CLI tool
│   ├── config.py               # JSON config persistence
│   ├── tabs/
│   │   ├── dashboard.py        # GPU info & status tab
│   │   ├── autoscan.py         # VFP autoscan workflow tab
│   │   ├── overclock.py        # Clock offset & power limits tab
│   │   ├── vfcurve.py          # VFP export/import/lock tab
│   │   └── fan_control.py      # Cooler control tab
│   └── widgets/
│       └── output_console.py   # Docked output console widget
```

## Usage

1. Launch `uv run python main.py`
2. The GUI auto-detects `nvoc-auto-optimizer.exe` in `../NVOC-Autooptimizer/target/release/`
3. Select a GPU from the dropdown (auto-populated via `list` command)
4. Use tabs to interact with GPU settings

### Autoscan Workflow

1. Go to **Autoscan** tab
2. Click **Export Init VFP** to save the factory curve
3. Click **Reset & Unlock VFP** to prepare for scanning
4. Configure parameters (mode, score threshold, etc.)
5. Click **Start Autoscan** — output streams to the console in real-time
6. After scan completes, click **Fix Results** for post-processing
7. Click **Import Final VFP** to apply the optimized curve


### 2. 各模块职责与主要类/函数

- **main.py**
  - 作用：项目主入口，负责解析参数、初始化配置、启动 `src.app` 的主窗口。
  - 启动流程：加载配置 → 初始化主窗口 → 进入事件循环。

- **src/app.py**
  - 作用：定义 `App` 主类，负责GUI主窗口的创建、各功能页签的加载与布局。
  - 主要类/函数：`App`（主窗口类），`setup_tabs()`（加载各功能页签）。

- **src/cli_runner.py**
  - 作用：封装与外部 NVOC-AutoOptimizer CLI 工具的命令行交互，负责命令构建、进程管理、结果解析。
  - 主要类/函数：`CLIRunner`（CLI交互类），`run_command()`（执行CLI命令）。

- **src/config.py**
  - 作用：负责GUI配置的加载、保存与校验，支持JSON格式配置文件。
  - 主要类/函数：`ConfigManager`（配置管理类），`load_config()`，`save_config()`。

- **src/tabs/**
  - 作用：每个.py文件对应一个功能页签，负责各自业务逻辑与界面。
    - `dashboard.py`：系统状态总览、实时数据展示。
    - `autoscan.py`：自动扫描与检测功能。
    - `fan_control.py`：风扇调速与曲线设置。
    - `overclock.py`：超频参数设置与应用。
    - `vfcurve.py`：电压-频率曲线编辑。
  - 主要类/函数：每个文件内有对应的Tab类（如 `DashboardTab`），负责UI与事件处理。

- **src/widgets/**
  - 作用：自定义可复用控件，如输出控制台。
  - 主要类/函数：`OutputConsole`（输出日志/命令行结果展示）。

### 3. 主程序启动流程

1. 运行 `main.py`，解析命令行参数（如配置文件路径）。
2. 加载/初始化配置（`src/config.py`）。
3. 创建并显示主窗口（`src/app.py`）。
4. 主窗口加载各功能页签（`src/tabs/`）。
5. 用户操作触发业务逻辑，部分操作通过 `src/cli_runner.py` 调用外部CLI工具。
6. 结果通过 `src/widgets/output_console.py` 等控件展示。

### 4. 与外部依赖的集成

- **NVOC-AutoOptimizer CLI 工具**
  - 集成方式：通过 `src/cli_runner.py` 以子进程方式调用CLI，传递参数并解析输出。
  - 数据流：GUI收集用户输入 → 构建CLI命令 → 执行CLI → 解析结果 → 更新界面。
  - 依赖管理：CLI工具需预先安装并配置好环境变量，GUI通过配置文件指定CLI路径。

### 5. 架构分层与模块关系

- **分层结构**
  - 表现层（UI）：`src/app.py`、`src/tabs/`、`src/widgets/`
  - 业务逻辑层：`src/cli_runner.py`、`src/config.py`
  - 外部依赖层：NVOC-AutoOptimizer CLI 工具

- **模块关系图（简述）**
```
  [main.py]
      ↓
  [src/app.py] ──→ [src/tabs/*] ──→ [src/widgets/*]
      ↓
  [src/config.py]
      ↓
  [src/cli_runner.py] ──→ [NVOC-AutoOptimizer CLI]
```

- **数据流简述**
  - 用户操作 → UI事件 → 业务逻辑处理（本地/CLI）→ 结果回传UI

### 进一步说明

- 推荐开发者先阅读 `main.py`、`src/app.py` 和各 `src/tabs/` 文件，理解主流程和各功能模块。
- 配置和CLI路径可通过 `nvoc_gui_config.json` 或命令行参数自定义。
- 如需扩展功能页签，可在 `src/tabs/` 下新增模块并在 `app.py` 注册。
