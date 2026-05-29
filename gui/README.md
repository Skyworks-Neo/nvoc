<a id="top"></a>

# NVOC-GUI

> GUI frontend for nvoc-auto-optimizer — NVIDIA GPU VF Curve Optimizer

[English](#english) | [中文](#chinese)

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](../LICENSE)

<a id="english"></a>

## English

### License

This project is licensed under the [Apache License 2.0](../LICENSE).

### Features

- **Dashboard** — View GPU info, status (clocks, sensors, coolers, VFP), and current OC settings
- **Autoscan** — One-click VF curve auto-optimization with Standard / Ultrafast / Legacy modes
- **Overclock** — Apply clock offsets, power limits, thermal limits, and voltage boost
- **VF Curve** — Export/import VFP curves (CSV), lock/unlock voltage points, and adjust single points
- **Fan Control** — Manual fan speed control with presets and slider
- **Output Console** — Real-time native and CLI output display for operations

### Requirements

- Python 3.8+
- Windows 10/11 (64-bit)
- Built nvoc-auto-optimizer binary (`../auto-optimizer/target/release/nvoc-auto-optimizer.exe`) for Autoscan and non-quick VFP export workflows
- Rust toolchain 1.95.0 when installing from source, because `pynvoc` builds the local Rust/Python bindings
- NVIDIA GPU with compatible drivers
- A Python environment managed by `uv` (recommended) or `venv`/`pip`
- Python Tk support (`tkinter`). Windows Python installers usually include it;
  on Linux it may require a system package such as `tk` (Arch),
  `python3-tk` (Debian/Ubuntu), or `python3-tkinter` (Fedora).

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
- Native short operations use the local `pynvoc` binding from `../nvoc-python`; `uv` resolves it through the workspace, and `requirements.txt` installs it in editable mode for `pip` users.
- Packages with shifting Python support windows use Python-version markers so each interpreter resolves an appropriate release line.
- Build-only tooling lives in the `build` dependency group; install packaging tools separately when you need them.
- `uv sync` resolves dependencies for the active interpreter. To verify Python 3.8 compatibility specifically, create or sync a Python 3.8 environment first.
- `tkinter` is provided by the Python interpreter or operating system, not by `pip`.
  If the GUI fails with `ModuleNotFoundError: No module named 'tkinter'`,
  use an interpreter that passes `python -c "import tkinter"` and recreate or
  resync the environment.
- If you are using `venv`, install dependencies with `pip install -r requirements.txt` after activating the environment.

### Directory Structure

```
gui/
├── main.py                     # Entry point
├── pyproject.toml              # Project metadata and dependencies
├── src/
│   ├── app.py                  # Main application window
│   ├── backend/                # Native pynvoc and CLI adapter layer
│   ├── cli_runner.py           # Subprocess wrapper for the CLI tool
│   ├── config.py               # JSON config persistence
│   ├── controllers/            # UI-independent controller logic
│   ├── panes/                  # Reusable pane implementations
│   ├── parsing.py              # CLI/native output parsing helpers
│   ├── task_runner.py          # Shared GUI background task runner
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
2. The GUI auto-detects GPUs through `pynvoc`
3. The GUI auto-detects `nvoc-auto-optimizer.exe` in `../auto-optimizer/target/release/` for CLI-backed workflows
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
  - Key classes/functions: `App` (main window class), lazy tab initialization, shared query/action helpers.

- **`src/backend/`**
  - Purpose: Adapts semantic GUI operations to either native `pynvoc` calls or CLI arguments.
  - Key classes/functions: `NativeBackend`, `CliBackend`, `FanSettings`.

- **`src/cli_runner.py`**
  - Purpose: Wraps command-line interaction with the external nvoc-auto-optimizer CLI, handling subprocess lifetime, cancellation, and streamed output.
  - Key classes/functions: `CLIRunner`.

- **`src/config.py`**
  - Purpose: Loads, saves, and validates GUI configuration in JSON format.
  - Key classes/functions: `Config`.

- **`src/controllers/`, `src/panes/`, `src/parsing.py`, `src/task_runner.py`**
  - Purpose: Keep reusable pane behavior, parsing, and background execution out of tab modules where practical.

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
5. User actions trigger controller/tab logic. Short queries and settings use `pynvoc`; Autoscan and dynamic VFP export use the external CLI via `src/cli_runner.py`.
6. Results are shown through widgets such as `src/widgets/output_console.py`.

### External Integration

- **pynvoc native binding**
  - Integration method: `src/backend/native.py` lazily imports `pynvoc` and runs short GPU queries/actions in the shared GUI task runner.
  - Data flow: GUI collects user input → calls native binding → normalizes structured results → updates the interface.

- **nvoc-auto-optimizer CLI tool**
  - Integration method: `src/cli_runner.py` invokes the CLI as a subprocess for long-running streamed workflows such as Autoscan and non-quick VFP export.
  - Data flow: GUI collects user input → builds CLI command → executes CLI → streams output → updates the interface.
  - Dependency handling: the CLI tool must be installed and available in the expected environment; the GUI can also use the CLI path specified in the configuration file.

### Architecture

- **Layered structure**
  - Presentation layer (UI): `src/app.py`, `src/tabs/`, `src/widgets/`
  - Business logic layer: `src/controllers/`, `src/panes/`, `src/parsing.py`, `src/task_runner.py`, `src/config.py`
  - External dependency layer: `pynvoc` native binding and nvoc-auto-optimizer CLI tool

- **Module relationship diagram**
```
  [main.py]
      ↓
  [src/app.py] ──→ [src/tabs/*] ──→ [src/widgets/*]
      ├──→ [src/backend/native.py] ──→ [pynvoc]
      ├──→ [src/cli_runner.py] ──→ [nvoc-auto-optimizer CLI]
      └──→ [src/config.py]
```

- **Data flow summary**
  - User action → UI event → business logic processing (native/CLI/local) → result returned to the UI

### Notes

- New contributors are encouraged to read `main.py`, `src/app.py`, and the files under `src/tabs/` first to understand the main flow and each feature module.
- Configuration and CLI paths can be customized through `nvoc_gui_config.json` or command-line arguments.
- To add a new feature tab, create a module under `src/tabs/` and register it in `app.py`.

<a id="chinese"></a>

## Chinese

### 许可证

本项目采用 [Apache License 2.0](../LICENSE) 许可。

### 功能

- **Dashboard** — 查看 GPU 信息、状态（核心频率、传感器、风扇、VFP）以及当前超频设置
- **Autoscan** — 一键自动优化 VF 曲线，支持 Standard / Ultrafast / Legacy 模式
- **Overclock** — 设置频率偏移、功率限制、温度限制和电压增强
- **VF Curve** — 导出/导入 VFP 曲线（CSV），锁定/解锁电压点，并支持单点调整
- **Fan Control** — 使用预设和滑块手动控制风扇转速
- **Output Console** — 实时展示原生调用和 CLI 工作流输出

### 运行要求

- Python 3.8+
- Windows 10/11（64 位）
- 已构建的 nvoc-auto-optimizer 可执行文件（`../auto-optimizer/target/release/nvoc-auto-optimizer.exe`），用于 Autoscan 和非 quick 的 VFP 导出工作流
- 从源码安装时需要 Rust 1.95.0 工具链，因为 `pynvoc` 会构建本地 Rust/Python 绑定
- 安装了兼容驱动的 NVIDIA GPU
- 使用 `uv`（推荐）或 `venv`/`pip` 管理的 Python 环境
- Python Tk 支持（`tkinter`）。Windows Python 安装包通常自带；Linux
  可能需要安装系统包，例如 Arch 的 `tk`、Debian/Ubuntu 的 `python3-tk`
  或 Fedora 的 `python3-tkinter`。

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
- 短耗时 GPU 查询和设置通过 `../nvoc-python` 中的本地 `pynvoc` 绑定完成；`uv` 通过 workspace 解析，`requirements.txt` 为 `pip` 用户以 editable 方式安装。
- 对 Python 支持窗口会变化的包使用 Python 版本标记，确保不同解释器解析到合适的发行版本。
- 仅用于构建的工具放在 `build` 依赖组中；需要打包时再单独安装构建工具。
- `uv sync` 会针对当前活动解释器解析依赖。若要专门验证 Python 3.8 兼容性，请先创建或切换到 Python 3.8 环境再同步。
- `tkinter` 由 Python 解释器或操作系统提供，不由 `pip` 安装。若 GUI 报错
  `ModuleNotFoundError: No module named 'tkinter'`，请使用能通过
  `python -c "import tkinter"` 的解释器，并重新创建或同步环境。
- 如果使用 `venv`，请在激活环境后执行 `pip install -r requirements.txt`。

### 目录结构

```
gui/
├── main.py                     # 程序入口
├── pyproject.toml              # 项目元数据与依赖
├── src/
│   ├── app.py                  # 主应用窗口
│   ├── backend/                # 原生 pynvoc 与 CLI 适配层
│   ├── cli_runner.py           # CLI 工具的子进程封装
│   ├── config.py               # JSON 配置持久化
│   ├── controllers/            # 与 UI 解耦的控制逻辑
│   ├── panes/                  # 可复用面板实现
│   ├── parsing.py              # CLI/原生输出解析辅助函数
│   ├── task_runner.py          # GUI 后台任务运行器
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
2. GUI 会通过 `pynvoc` 自动检测 GPU
3. GUI 会自动检测 `../auto-optimizer/target/release/` 下的 `nvoc-auto-optimizer.exe`，供 CLI 工作流使用
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
  - 主要类/函数：`App`（主窗口类）、懒加载页签、共享查询/动作辅助函数。

- **`src/backend/`**
  - 作用：将 GUI 的语义化操作适配到原生 `pynvoc` 调用或 CLI 参数。
  - 主要类/函数：`NativeBackend`、`CliBackend`、`FanSettings`。

- **`src/cli_runner.py`**
  - 作用：封装与外部 nvoc-auto-optimizer CLI 工具的命令行交互，负责子进程生命周期、取消与流式输出。
  - 主要类/函数：`CLIRunner`。

- **`src/config.py`**
  - 作用：负责 GUI 配置的加载、保存与校验，支持 JSON 格式配置文件。
  - 主要类/函数：`Config`。

- **`src/controllers/`、`src/panes/`、`src/parsing.py`、`src/task_runner.py`**
  - 作用：承载可复用面板行为、解析逻辑和后台执行逻辑，减少页签模块负担。

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
5. 用户操作触发控制器/页签逻辑。短耗时查询和设置使用 `pynvoc`；Autoscan 和动态 VFP 导出通过 `src/cli_runner.py` 调用外部 CLI。
6. 结果会通过 `src/widgets/output_console.py` 等控件显示出来。

### 与外部依赖的集成

- **pynvoc 原生绑定**
  - 集成方式：`src/backend/native.py` 延迟导入 `pynvoc`，并在共享 GUI 后台任务运行器中执行短耗时 GPU 查询/设置。
  - 数据流：GUI 收集用户输入 → 调用原生绑定 → 规范化结构化结果 → 更新界面。

- **nvoc-auto-optimizer CLI 工具**
  - 集成方式：`src/cli_runner.py` 以子进程方式调用 CLI，用于 Autoscan 和非 quick VFP 导出等长耗时流式工作流。
  - 数据流：GUI 收集用户输入 → 构建 CLI 命令 → 执行 CLI → 流式输出 → 更新界面。
  - 依赖处理：CLI 工具需要预先安装并可在目标环境中访问；GUI 也可以使用配置文件中指定的 CLI 路径。

### 架构分层与模块关系

- **分层结构**
  - 表现层（UI）：`src/app.py`、`src/tabs/`、`src/widgets/`
  - 业务逻辑层：`src/controllers/`、`src/panes/`、`src/parsing.py`、`src/task_runner.py`、`src/config.py`
  - 外部依赖层：`pynvoc` 原生绑定和 nvoc-auto-optimizer CLI 工具

- **模块关系图（简述）**
```
  [main.py]
      ↓
  [src/app.py] ──→ [src/tabs/*] ──→ [src/widgets/*]
      ├──→ [src/backend/native.py] ──→ [pynvoc]
      ├──→ [src/cli_runner.py] ──→ [nvoc-auto-optimizer CLI]
      └──→ [src/config.py]
```

- **数据流简述**
  - 用户操作 → UI 事件 → 业务逻辑处理（原生/CLI/本地）→ 结果返回 UI

### 补充说明

- 建议开发者先阅读 `main.py`、`src/app.py` 和 `src/tabs/` 下的文件，以理解主流程和各功能模块。
- 配置和 CLI 路径可以通过 `nvoc_gui_config.json` 或命令行参数自定义。
- 如果需要扩展功能页签，可以在 `src/tabs/` 下新增模块并在 `app.py` 中注册。

<div style="text-align: right;">Back to top / 返回顶部</div>
