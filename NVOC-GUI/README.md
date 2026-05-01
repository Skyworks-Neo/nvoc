<a id="top"></a>

# NVOC-GUI

> GUI frontend for NVOC-AutoOptimizer — NVIDIA GPU VF Curve Optimizer

[English](#english) | [中文](#chinese)

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

---

## 配套产品——使用所有配套产品以达到最好体验

[NVOC-AUTOOPTIMIZER](https://github.com/Skyworks-Neo/NVOC-AutoOptimizer)：核心模块。

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor)：压力测试模块，用于自动超频扫描部分。没有该模块仍可以使用自动扫描之外的所有功能。（NVOC-AutoOptimizer开放任何你的自定义压力测试模块接入，只需满足return
code定义即可。）

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI)：跨平台超频图形界面，直接对标MSI Afterburner。 （为了避免GPU超炸带走图形界面，使用CPU渲染，在低端机器如遇到性能问题，建议使用NVOC-TUI）；

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI)：跨平台超频命令行界面，用于没有图形界面的机器，兼容性好，性能要求低；

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV)：client-server架构控制模块，用于机房、服务器、工作站等场景的 Web 管理、~~远程超频~~（TODO）

<a id="english"></a>

## English

### License

This project is licensed under the [Apache License 2.0](LICENSE).

### Features

- **Dashboard** — View GPU info, status (clocks, sensors, coolers, VFP), and current OC settings
- **Autoscan** — One-click VF curve auto-optimization with Standard / Ultrafast / Legacy modes
- **Overclock** — Apply clock offsets, power limits, thermal limits, and voltage boost
- **VF Curve** — Export/import VFP curves (CSV), lock/unlock voltage points, and adjust single points
- **Fan Control** — Manual fan speed control with presets and slider
- **Output Console** — Real-time CLI output display for all operations

### Requirements

- Python 3.8+
- Windows 10/11 (64-bit)
- Built NVOC-AutoOptimizer binary (`../NVOC-Autooptimizer/target/release/nvoc-auto-optimizer.exe`)
- NVIDIA GPU with compatible drivers
- A Python environment managed by `uv` (recommended) or `venv`/`pip`

### Setup

Recommended (`uv`):

```powershell
uv sync
uv run python main.py
```

Alternative (`venv` + `pip`):

```powershell
py -3.8 -m venv .venv
.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
pip install -r requirements.txt
python main.py
```

### Compile a Single Executable (Optional)

Recommended (`uv`):

```powershell
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

Alternative (`venv` + `pip`):

```powershell
pip install pyinstaller
pyinstaller nvoc_gui.spec
```

### Dependency Management

This repository uses `pyproject.toml` as the source of truth for `uv`, and provides `requirements.txt` for users who prefer `venv` + `pip`.

- Runtime dependencies use relaxed version ranges and are mirrored in `requirements.txt`.
- Packages with shifting Python support windows use Python-version markers so each interpreter resolves an appropriate release line.
- Build-only tooling lives in the `build` dependency group; install packaging tools separately when you need them.
- `uv sync` resolves dependencies for the active interpreter. To verify Python 3.8 compatibility specifically, create or sync a Python 3.8 environment first.
- If you are using `venv`, install dependencies with `pip install -r requirements.txt` after activating the environment.

### Directory Structure

```
NVOC-GUI/
├── main.py                     # Entry point
├── pyproject.toml              # Project metadata and dependencies
├── src/
│   ├── app.py                  # Main application window
│   ├── cli_runner.py           # Subprocess wrapper for the CLI tool
│   ├── config.py               # JSON config persistence
│   ├── tabs/
│   │   ├── dashboard.py        # GPU info and status tab
│   │   ├── autoscan.py         # VFP autoscan workflow tab
│   │   ├── overclock.py        # Clock offset and power limits tab
│   │   ├── vfcurve.py          # VFP export/import/lock tab
│   │   └── fan_control.py      # Cooler control tab
│   └── widgets/
│       └── output_console.py   # Docked output console widget
```

### Usage

1. Run `uv run python main.py`
2. The GUI auto-detects `nvoc-auto-optimizer.exe` in `../NVOC-Autooptimizer/target/release/`
3. Select a GPU from the dropdown, which is auto-populated via the `list` command
4. Use the tabs to interact with GPU settings

### Autoscan Workflow

1. Open the **Autoscan** tab
2. Click **Export Init VFP** to save the factory curve
3. Click **Reset & Unlock VFP** to prepare for scanning
4. Configure parameters such as mode and score threshold
5. Click **Start Autoscan** — output streams to the console in real time
6. After the scan completes, click **Fix Results** for post-processing
7. Click **Import Final VFP** to apply the optimized curve

### Module Responsibilities

- **`main.py`**
  - Purpose: Application entry point; parses arguments, initializes configuration, and launches the main window from `src.app`.
  - Startup flow: load configuration → initialize the main window → enter the event loop.

- **`src/app.py`**
  - Purpose: Defines the `App` main class, responsible for creating the GUI window and loading/layout of each tab.
  - Key classes/functions: `App` (main window class), `setup_tabs()` (loads all tabs).

- **`src/cli_runner.py`**
  - Purpose: Wraps command-line interaction with the external NVOC-AutoOptimizer CLI, handling command construction, process management, and result parsing.
  - Key classes/functions: `CLIRunner` (CLI interaction class), `run_command()` (executes CLI commands).

- **`src/config.py`**
  - Purpose: Loads, saves, and validates GUI configuration in JSON format.
  - Key classes/functions: `ConfigManager` (configuration manager), `load_config()`, `save_config()`.

- **`src/tabs/`**
  - Purpose: Each `.py` file maps to a feature tab and handles its own UI and business logic.
    - `dashboard.py`: system overview and live status display.
    - `autoscan.py`: automatic scanning and detection workflow.
    - `fan_control.py`: fan speed control and curve settings.
    - `overclock.py`: overclock parameter configuration and application.
    - `vfcurve.py`: voltage-frequency curve editing.
  - Key classes/functions: each file contains a corresponding tab class (for example, `DashboardTab`) that handles UI and events.

- **`src/widgets/`**
  - Purpose: Reusable custom widgets such as the output console.
  - Key classes/functions: `OutputConsole` (displays logs and CLI output).

### Startup Flow

1. Run `main.py` and parse command-line arguments such as a config file path.
2. Load or initialize configuration from `src/config.py`.
3. Create and display the main window from `src/app.py`.
4. The main window loads all feature tabs from `src/tabs/`.
5. User actions trigger business logic, and some actions call the external CLI via `src/cli_runner.py`.
6. Results are shown through widgets such as `src/widgets/output_console.py`.

### External Integration

- **NVOC-AutoOptimizer CLI tool**
  - Integration method: `src/cli_runner.py` invokes the CLI as a subprocess, passes arguments, and parses output.
  - Data flow: GUI collects user input → builds CLI command → executes CLI → parses result → updates the interface.
  - Dependency handling: the CLI tool must be installed and available in the expected environment; the GUI can also use the CLI path specified in the configuration file.

### Architecture

- **Layered structure**
  - Presentation layer (UI): `src/app.py`, `src/tabs/`, `src/widgets/`
  - Business logic layer: `src/cli_runner.py`, `src/config.py`
  - External dependency layer: NVOC-AutoOptimizer CLI tool

- **Module relationship diagram**
```
  [main.py]
      ↓
  [src/app.py] ──→ [src/tabs/*] ──→ [src/widgets/*]
      ↓
  [src/config.py]
      ↓
  [src/cli_runner.py] ──→ [NVOC-AutoOptimizer CLI]
```

- **Data flow summary**
  - User action → UI event → business logic processing (local/CLI) → result returned to the UI

### Notes

- New contributors are encouraged to read `main.py`, `src/app.py`, and the files under `src/tabs/` first to understand the main flow and each feature module.
- Configuration and CLI paths can be customized through `nvoc_gui_config.json` or command-line arguments.
- To add a new feature tab, create a module under `src/tabs/` and register it in `app.py`.

<a id="chinese"></a>

## Chinese

### 许可证

本项目采用 [Apache License 2.0](LICENSE) 许可。

### 功能

- **Dashboard** — 查看 GPU 信息、状态（核心频率、传感器、风扇、VFP）以及当前超频设置
- **Autoscan** — 一键自动优化 VF 曲线，支持 Standard / Ultrafast / Legacy 模式
- **Overclock** — 设置频率偏移、功率限制、温度限制和电压增强
- **VF Curve** — 导出/导入 VFP 曲线（CSV），锁定/解锁电压点，并支持单点调整
- **Fan Control** — 使用预设和滑块手动控制风扇转速
- **Output Console** — 所有操作的 CLI 输出实时展示

### 运行要求

- Python 3.8+
- Windows 10/11（64 位）
- 已构建的 NVOC-AutoOptimizer 可执行文件（`../NVOC-Autooptimizer/target/release/nvoc-auto-optimizer.exe`）
- 安装了兼容驱动的 NVIDIA GPU
- 使用 `uv`（推荐）或 `venv`/`pip` 管理的 Python 环境

### 安装与启动

推荐使用（`uv`）：

```powershell
uv sync
uv run python main.py
```

备用方案（`venv` + `pip`）：

```powershell
py -3.8 -m venv .venv
.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
pip install -r requirements.txt
python main.py
```

### 打包为单文件（可选）

推荐使用（`uv`）：

```powershell
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

备用方案（`venv` + `pip`）：

```powershell
pip install pyinstaller
pyinstaller nvoc_gui.spec
```

### 依赖管理

本仓库以 `pyproject.toml` 作为 `uv` 的依赖来源，同时提供 `requirements.txt` 方便偏好 `venv` + `pip` 的用户。

- 运行时依赖使用较宽松的版本范围，并且会同步到 `requirements.txt`。
- 对 Python 支持窗口会变化的包使用 Python 版本标记，确保不同解释器解析到合适的发行版本。
- 仅用于构建的工具放在 `build` 依赖组中；需要打包时再单独安装构建工具。
- `uv sync` 会针对当前活动解释器解析依赖。若要专门验证 Python 3.8 兼容性，请先创建或切换到 Python 3.8 环境再同步。
- 如果使用 `venv`，请在激活环境后执行 `pip install -r requirements.txt`。

### 目录结构

```
NVOC-GUI/
├── main.py                     # 程序入口
├── pyproject.toml              # 项目元数据与依赖
├── src/
│   ├── app.py                  # 主应用窗口
│   ├── cli_runner.py           # CLI 工具的子进程封装
│   ├── config.py               # JSON 配置持久化
│   ├── tabs/
│   │   ├── dashboard.py        # GPU 信息与状态页签
│   │   ├── autoscan.py         # VFP 自动扫描工作流页签
│   │   ├── overclock.py        # 频率偏移与功率限制页签
│   │   ├── vfcurve.py          # VFP 导出/导入/锁定页签
│   │   └── fan_control.py      # 风扇控制页签
│   └── widgets/
│       └── output_console.py   # 停靠式输出控制台控件
```

### 使用方法

1. 运行 `uv run python main.py`
2. GUI 会自动检测 `../NVOC-Autooptimizer/target/release/` 下的 `nvoc-auto-optimizer.exe`
3. 从下拉框中选择 GPU（会通过 `list` 命令自动填充）
4. 使用各页签与 GPU 设置交互

### Autoscan 工作流

1. 打开 **Autoscan** 页签
2. 点击 **Export Init VFP** 保存出厂曲线
3. 点击 **Reset & Unlock VFP** 为扫描做准备
4. 配置参数，例如模式和分数阈值
5. 点击 **Start Autoscan**，输出会实时流式显示到控制台
6. 扫描完成后点击 **Fix Results** 做后处理
7. 点击 **Import Final VFP** 应用优化后的曲线

### 模块职责

- **`main.py`**
  - 作用：项目入口，负责解析参数、初始化配置，并启动 `src.app` 中的主窗口。
  - 启动流程：加载配置 → 初始化主窗口 → 进入事件循环。

- **`src/app.py`**
  - 作用：定义 `App` 主类，负责创建 GUI 主窗口以及加载/布局各个页签。
  - 主要类/函数：`App`（主窗口类），`setup_tabs()`（加载所有页签）。

- **`src/cli_runner.py`**
  - 作用：封装与外部 NVOC-AutoOptimizer CLI 工具的命令行交互，负责命令构建、进程管理和结果解析。
  - 主要类/函数：`CLIRunner`（CLI 交互类），`run_command()`（执行 CLI 命令）。

- **`src/config.py`**
  - 作用：负责 GUI 配置的加载、保存与校验，支持 JSON 格式配置文件。
  - 主要类/函数：`ConfigManager`（配置管理类），`load_config()`，`save_config()`。

- **`src/tabs/`**
  - 作用：每个 `.py` 文件对应一个功能页签，负责各自的业务逻辑与界面。
    - `dashboard.py`：系统总览与实时状态展示。
    - `autoscan.py`：自动扫描与检测流程。
    - `fan_control.py`：风扇转速控制与曲线设置。
    - `overclock.py`：超频参数配置与应用。
    - `vfcurve.py`：电压-频率曲线编辑。
  - 主要类/函数：每个文件中都有对应的页签类（例如 `DashboardTab`），负责 UI 与事件处理。

- **`src/widgets/`**
  - 作用：提供可复用的自定义控件，例如输出控制台。
  - 主要类/函数：`OutputConsole`（展示日志和 CLI 输出）。

### 主程序启动流程

1. 运行 `main.py`，解析命令行参数（例如配置文件路径）。
2. 加载或初始化 `src/config.py` 中的配置。
3. 创建并显示 `src/app.py` 中的主窗口。
4. 主窗口加载 `src/tabs/` 下的所有功能页签。
5. 用户操作触发业务逻辑，部分操作会通过 `src/cli_runner.py` 调用外部 CLI 工具。
6. 结果会通过 `src/widgets/output_console.py` 等控件显示出来。

### 与外部依赖的集成

- **NVOC-AutoOptimizer CLI 工具**
  - 集成方式：`src/cli_runner.py` 以子进程方式调用 CLI，传递参数并解析输出。
  - 数据流：GUI 收集用户输入 → 构建 CLI 命令 → 执行 CLI → 解析结果 → 更新界面。
  - 依赖处理：CLI 工具需要预先安装并可在目标环境中访问；GUI 也可以使用配置文件中指定的 CLI 路径。

### 架构分层与模块关系

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
  - 用户操作 → UI 事件 → 业务逻辑处理（本地/CLI）→ 结果返回 UI

### 补充说明

- 建议开发者先阅读 `main.py`、`src/app.py` 和 `src/tabs/` 下的文件，以理解主流程和各功能模块。
- 配置和 CLI 路径可以通过 `nvoc_gui_config.json` 或命令行参数自定义。
- 如果需要扩展功能页签，可以在 `src/tabs/` 下新增模块并在 `app.py` 中注册。

<div style="text-align: right;">Back to top / 返回顶部</div>
