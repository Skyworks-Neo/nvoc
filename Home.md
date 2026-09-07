# NVOC Wiki

> **NVOC** — NVIDIA GPU Overclocking & Control Toolset

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC is a Rust/Python hybrid monorepo for NVIDIA GPU overclocking, V-F curve automatic scanning, stress testing, and stability validation. `nvoc-cli` owns the direct NVAPI/NVML control commands, `auto-optimizer` orchestrates autoscan workflows, and the GUI/TUI/SRV frontends wrap the same core for different operating environments.

⚠️ **Safety Notice**: Overclocking may cause display driver crashes, GPU resets, or system instability. Before performing write operations, verify the target GPU, driver, cooling, and recovery path.

---

### Quick Navigation

✨ marks pages added in this wiki update — several new pages are being published as part of this refresh.

| Page | Description |
|---|---|
| [[Home]] | Wiki home and component overview |
| [[Getting-Started]] ✨ | Toolchain setup, build, and first run |
| [[Components]] ✨ | Canonical component inventory and responsibilities |
| [[Architecture]] | Project architecture and repository structure |
| [[Build-and-Install]] | Build, installation, and environment setup |
| [[GPU-Support-Matrix]] | GPU generation × API compatibility matrix |
| [[CLI-Guide]] ✨ | nvoc-cli: direct NVAPI/NVML control commands |
| [[CLI-Command-Reference]] ✨ | Flat function-style command reference |
| [[Auto-Optimizer-Guide]] | Auto-Optimizer core usage guide |
| [[Autoscan-Workflow]] | Complete V-F curve autoscan workflow |
| [[Stress-Testing]] | Stress testing tools (CUDA / OpenCL / Rust) |
| [[GUI-Guide]] | GUI graphical interface guide |
| [[TUI-Guide]] | TUI terminal interface guide |
| [[SRV-Guide]] | Windows Service / HTTP control layer |
| [[PyNvoc-API]] ✨ | pynvoc native Python bindings API |
| [[Theory]] | Theoretical foundations of GPU overclocking & V-F curve optimization |
| [[Container-Usage]] ✨ | Running nvoc-cli in NVIDIA Container Toolkit containers |
| [[Frontends]] ✨ | GUI / TUI / SRV frontend overview |
| [[Safety-and-Recovery]] ✨ | High-risk operations and recovery controls |
| [[Reverse-Engineering]] ✨ | Reverse-engineering notes and coverage boundaries |
| [[FAQ]] ✨ | Frequently asked questions |
| [[Contributing]] | Contribution guidelines |

### Components

Canonical inventory — mirrors the root `README.md` → "Components (canonical)" section.

User-facing products:

| Component | Path | Purpose |
|---|---|---|
| **NVOC-CLI** | `cli/` | Direct NVAPI/NVML control commands: GPU discovery, status reading, general resets, and setting writes (flat function-style commands with backend selection) |
| **NVOC-AUTO-OPTIMIZER** | `auto-optimizer/` | Rust CLI core: V-F curve export/import, autoscan, result fixing, and retained VFP reset workflows |
| **NVOC-STRESSOR CUDA RS** | `cli-stressor-cuda-rs/` | Rust native CUDA GEMM stress test (recommended CUDA stressor; embedded in the optimizer binary) |
| **NVOC-STRESSOR OpenCL** | `cli-stressor-opencl/` | Lightweight OpenCL stress test, no CUDA/PyTorch dependency |
| **NVOC-GUI** | `gui/` | Python graphical interface frontend |
| **NVOC-TUI** | `tui/` | Python Textual terminal interface frontend |
| **NVOC-SRV** | `srv/` | Windows Service and localhost HTTP control layer |

Internal libraries and shared modules:

| Component | Path | Purpose |
|---|---|---|
| **NVOC-CORE** | `core/` | Core overclocking/domain library shared by all Rust components |
| **NVOC-CLI-COMMON** | `cli-common/` | Shared CLI support layer for Rust command-line components |
| **NVOC-PYTHON (pynvoc)** | `nvoc-python/` | Native Python bindings (PyO3) used by the Python frontends |
| **nvapi-rs (fork)** | `nvapi-rs/` | NVAPI Rust bindings, pinned as a git submodule |

> The former Python/PyTorch stressor `cli-stressor-cuda/` has been removed — use `cli-stressor-cuda-rs/` instead.

### Quick Start

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc/auto-optimizer
cargo build --release
```

Then run the frontends:

```bash
# GUI
cd gui && uv sync && uv run python main.py

# TUI
cd tui && uv sync && uv run nvoc-tui
```

### License

Apache License 2.0 — see [LICENSE](https://github.com/Skyworks-Neo/nvoc/blob/main/LICENSE)

---

<a id="chinese"></a>

## 中文

NVOC 是一个 Rust/Python 混合 monorepo，用于 NVIDIA GPU 超频、V-F 曲线自动扫描、压力测试与稳定性验证。`nvoc-cli` 负责直接的 NVAPI/NVML 控制命令，`auto-optimizer` 负责编排 autoscan 工作流，GUI/TUI/SRV 前端则面向不同运行环境封装同一套核心能力。

⚠️ **安全提醒**：超频可能导致显示驱动崩溃、GPU 重置或系统不稳定。执行写入操作前，请确认目标 GPU、驱动、散热和恢复路径。

---

### 快速导航

✨ 表示本次更新新增的页面——本次 Wiki 刷新将一并发布多个新页面。

| 页面 | 说明 |
|---|---|
| [[Home]] | Wiki 首页与组件总览 |
| [[Getting-Started]] ✨ | 工具链安装、编译与首次运行 |
| [[Components]] ✨ | 组件清单与职责划分（权威版本） |
| [[Architecture]] | 项目架构与仓库结构 |
| [[Build-and-Install]] | 编译、安装与环境配置 |
| [[GPU-Support-Matrix]] | GPU 世代 × 接口兼容性矩阵 |
| [[CLI-Guide]] ✨ | nvoc-cli：直接 NVAPI/NVML 控制命令 |
| [[CLI-Command-Reference]] ✨ | 扁平函数式命令参考 |
| [[Auto-Optimizer-Guide]] | Auto-Optimizer 核心功能使用指南 |
| [[Autoscan-Workflow]] | V-F 曲线自动扫描完整流程 |
| [[Stress-Testing]] | 压力测试工具（CUDA / OpenCL / Rust） |
| [[GUI-Guide]] | GUI 图形界面使用指南 |
| [[TUI-Guide]] | TUI 终端界面使用指南 |
| [[SRV-Guide]] | Windows Service / HTTP 控制层 |
| [[PyNvoc-API]] ✨ | pynvoc 原生 Python 绑定 API |
| [[Theory]] | GPU 超频与 V-F 曲线优化的理论基础 |
| [[Container-Usage]] ✨ | 在 NVIDIA Container Toolkit 容器中运行 nvoc-cli |
| [[Frontends]] ✨ | GUI / TUI / SRV 前端总览 |
| [[Safety-and-Recovery]] ✨ | 高风险操作与恢复控制 |
| [[Reverse-Engineering]] ✨ | 逆向工程笔记与覆盖边界 |
| [[FAQ]] ✨ | 常见问题 |
| [[Contributing]] | 贡献指南 |

### 组件一览

权威组件清单，与根目录 `README.md` 的 "Components (canonical)" 章节保持一致。

用户向产品：

| 组件 | 路径 | 用途 |
|---|---|---|
| **NVOC-CLI** | `cli/` | 直接 NVAPI/NVML 控制命令：GPU 发现、状态读取、通用重置与参数写入（扁平函数式命令，支持后端选择） |
| **NVOC-AUTO-OPTIMIZER** | `auto-optimizer/` | Rust CLI 核心：V-F 曲线导入导出、autoscan、结果后处理与保留的 VFP 重置流程 |
| **NVOC-STRESSOR CUDA RS** | `cli-stressor-cuda-rs/` | Rust 原生 CUDA GEMM 压力测试（推荐的 CUDA 压力工具，已内嵌进优化器二进制） |
| **NVOC-STRESSOR OpenCL** | `cli-stressor-opencl/` | 轻量 OpenCL 压力测试，不依赖 CUDA/PyTorch |
| **NVOC-GUI** | `gui/` | Python 图形界面前端 |
| **NVOC-TUI** | `tui/` | Python Textual 终端界面前端 |
| **NVOC-SRV** | `srv/` | Windows Service 与 localhost HTTP 控制层 |

内部库与共享模块：

| 组件 | 路径 | 用途 |
|---|---|---|
| **NVOC-CORE** | `core/` | 所有 Rust 组件共享的超频/设备领域核心库 |
| **NVOC-CLI-COMMON** | `cli-common/` | Rust 命令行组件的共享支撑层 |
| **NVOC-PYTHON (pynvoc)** | `nvoc-python/` | 原生 Python 绑定（PyO3），供 Python 前端使用 |
| **nvapi-rs（fork）** | `nvapi-rs/` | NVAPI Rust 绑定，以 git 子模块固定版本 |

> 原来的 Python/PyTorch 压力测试工具 `cli-stressor-cuda/` 已被移除——请改用 `cli-stressor-cuda-rs/`。

### 快速开始

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc/auto-optimizer
cargo build --release
```

然后运行前端：

```bash
# GUI
cd gui && uv sync && uv run python main.py

# TUI
cd tui && uv sync && uv run nvoc-tui
```

### 许可证

Apache License 2.0 — 详见 [LICENSE](https://github.com/Skyworks-Neo/nvoc/blob/main/LICENSE)
