# NVOC

[English](#english) | [中文](#chinese)

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

<a id="english"></a>
## English

NVOC is a monorepo for NVIDIA GPU overclocking and stability tools. The stack centers on a Rust command-line optimizer that controls NVIDIA GPUs through NVAPI / NVML, uses CUDA or OpenCL stress workloads to validate stability, and exposes GUI, TUI, and service frontends for different operating environments.

Overclocking can crash the display driver, reset the GPU, or make a machine unstable. Run write operations only when you understand the target GPU, driver, cooling, and recovery path.

## Documentation & Wiki Policy

For this repository size and contributor model, documentation is maintained in this monorepo first.

- Canonical docs path: [`docs/wiki/`](./docs/wiki/)
- English Home: [`docs/wiki/Home.md`](./docs/wiki/Home.md)
- Chinese Home: [`docs/wiki/Home-zh.md`](./docs/wiki/Home-zh.md)
- Review flow: open PRs in `nvoc`, review here, then sync to GitHub Wiki (`nvoc-wiki`) if needed.

If wiki pages differ from `docs/wiki`, treat `docs/wiki` as source of truth and sync the wiki repo.

## Components (canonical)

This section is the canonical component inventory for the monorepo. `CONTRIBUTING.md` and `AGENTS.md` intentionally reference this table instead of duplicating entries.

### User-facing products

| Component | Path | Purpose |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | [auto-optimizer/](./auto-optimizer/) | Rust CLI for V-F curve export/import, autoscan, result fixing, and retained VFP reset workflows. Use NVOC-CLI for GPU discovery, status, general resets, and NVAPI / NVML setting writes. |
| NVOC-CLI | [cli/](./cli/) | Focused Rust wrapper over `nvoc-core` with flat function-style commands and NVAPI/NVML backend selection. |
| NVOC-STRESSOR CUDA (Rust, experimental) | [cli-stressor-cuda-rs/](./cli-stressor-cuda-rs/) | Rust CUDA stress tool for CUDA-capable systems and native Rust pipeline usage. |
| NVOC-STRESSOR OpenCL | [cli-stressor-opencl/](./cli-stressor-opencl/) | Lightweight OpenCL stress tool for broader backend coverage without CUDA-specific dependencies. |
| NVOC-GUI | [gui/](./gui/) | Python GUI frontend for dashboard, autoscan, overclock, V-F curve, fan control, and live CLI output workflows. |
| NVOC-TUI | [tui/](./tui/) | Textual terminal UI frontend for machines where a desktop GUI is unavailable or undesirable. |
| NVOC-SRV | [srv/](./srv/) | Windows service and localhost HTTP control layer for server, workstation, and managed-machine use cases. |

### Internal libraries and experimental modules

| Component | Path | Scope |
|---|---|---|
| NVOC-CORE | [core/](./core/) | Core overclocking/domain library shared by Rust components. |
| NVOC-CLI-COMMON | [cli-common/](./cli-common/) | Shared CLI support layer for Rust command-line components. |
| NVOC-PYTHON (pynvoc) | [nvoc-python/](./nvoc-python/) | Python bindings and shared Python-side integration surface. |

Read the component README before building or running that component. The detailed command reference, compatibility matrices, and scanning theory live in [auto-optimizer/README-en.md](./auto-optimizer/README-en.md) and [auto-optimizer/README.md](./auto-optimizer/README.md).

## Requirements

- NVIDIA GPU with a compatible driver.
- Administrator privileges on Windows or sudo privileges on Linux for operations that write overclocking settings.
- Rust toolchain for `auto-optimizer/` and `srv/`.
- Python plus `uv` for the Python frontends and OpenCL stressor.
- Python Tk support (`tkinter`) for NVOC-GUI. On Linux this may require a
  system package such as `tk`, `python3-tk`, or `python3-tkinter`.
- OpenCL runtime support for `cli-stressor-opencl/`.

Platform support and feature availability vary by GPU generation and backend. The auto-optimizer README contains the current support matrix for RTX 50/40/30/20, GTX 16/10/9, Volta, mobile GPUs, NVAPI, and NVML.

## Quick Start

Clone the monorepo:

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

Build the optimizer:

```bash
cd auto-optimizer
cargo build --release
```

Run a frontend from the repository root after the optimizer binary has been built:

```bash
cd gui
uv sync
uv run python main.py
```

or:

```bash
cd tui
uv sync
uv run nvoc-tui
```

For autoscan workflows, configure one of the stressor modules and review the scripts under [auto-optimizer/test/](./auto-optimizer/test/). The CUDA stressor default config targets cards with 8G+ VRAM; use `cli-stressor-cuda-rs-6g-8g.toml` for 6G-8G cards.

## Repository Layout

```text
nvoc/
├── auto-optimizer/       # Rust CLI core and autoscan implementation
├── cli-stressor-opencl/  # OpenCL stress workload
├── gui/                  # Python GUI frontend
├── srv/                  # Windows service wrapper and HTTP control endpoint
└── tui/                  # Python Textual terminal frontend
```

## Development

Each component keeps its own build metadata because the monorepo contains both Rust and Python projects.

Common commands:

```bash
cd auto-optimizer && cargo build
cd srv && cargo build
cd gui && uv sync
cd tui && uv sync && uv run pytest
cd cli-stressor-opencl && uv sync
```

When changing shared behavior, start with `auto-optimizer/`, then verify the GUI/TUI/service wrappers that invoke it.

## License

This repository is licensed under the Apache License 2.0. See [LICENSE](./LICENSE) for the full text and [NOTICE](./NOTICE) for repository-level notices.

<a id="chinese"></a>
## 中文

NVOC 是一个 NVIDIA GPU 超频与稳定性工具的 monorepo。核心是 Rust 编写的命令行优化器，通过 NVAPI / NVML 控制 NVIDIA GPU，配合 CUDA 或 OpenCL 压力测试验证稳定性，并提供 GUI、TUI、Windows Service 等前端。

超频可能导致显示驱动崩溃、GPU 重置或系统不稳定。执行写入类操作前，请确认目标 GPU、驱动、散热和恢复路径。

## 组件

该章节是 monorepo 组件清单的唯一权威来源；`CONTRIBUTING.md` 与 `AGENTS.md` 仅引用此处，不再重复维护列表。

### 用户向产品

| 组件 | 路径 | 用途 |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | [auto-optimizer/](./auto-optimizer/) | Rust CLI，负责 V-F 曲线导入导出、autoscan、结果后处理和保留的 VFP 重置流程。GPU 发现、状态读取、通用重置和 NVAPI / NVML 写入由 NVOC-CLI 负责。 |
| NVOC-CLI | [cli/](./cli/) | `nvoc-core` 的精简 Rust 包装器，提供扁平函数式命令和 NVAPI/NVML 后端选择。 |
| NVOC-STRESSOR CUDA（Rust，实验性） | [cli-stressor-cuda-rs/](./cli-stressor-cuda-rs/) | 面向 CUDA 环境与 Rust 原生流水线的 Rust CUDA 压力测试工具。 |
| NVOC-STRESSOR OpenCL | [cli-stressor-opencl/](./cli-stressor-opencl/) | 轻量 OpenCL 压力测试工具，用于不依赖 CUDA 专有依赖的后端覆盖。 |
| NVOC-GUI | [gui/](./gui/) | Python 图形界面，提供 Dashboard、Autoscan、Overclock、V-F Curve、Fan Control 和实时 CLI 输出。 |
| NVOC-TUI | [tui/](./tui/) | 基于 Textual 的终端界面，适用于没有桌面环境或不适合运行 GUI 的机器。 |
| NVOC-SRV | [srv/](./srv/) | Windows Service 与 localhost HTTP 控制层，面向服务器、工作站和托管机器场景。 |

### 内部库与实验模块

| 组件 | 路径 | 说明 |
|---|---|---|
| NVOC-CORE | [core/](./core/) | Rust 共享核心库，承载超频/设备领域能力。 |
| NVOC-CLI-COMMON | [cli-common/](./cli-common/) | Rust CLI 共享支撑层。 |
| NVOC-PYTHON (pynvoc) | [nvoc-python/](./nvoc-python/) | Python 绑定与跨 Python 组件共享接口。 |

构建或运行某个组件前，请先阅读对应子目录 README。详细命令参考、兼容性矩阵和扫描原理在 [auto-optimizer/README.md](./auto-optimizer/README.md) 和 [auto-optimizer/README-en.md](./auto-optimizer/README-en.md) 中。

## 依赖

- 支持的 NVIDIA GPU 与驱动。
- Windows 管理员权限，或 Linux sudo 权限，用于写入超频参数。
- `auto-optimizer/` 与 `srv/` 需要 Rust 工具链。
- Python 前端与 OpenCL 压力测试工具推荐使用 `uv` 管理环境。
- NVOC-GUI 需要 Python Tk 支持（`tkinter`）。Linux 上可能需要安装
  `tk`、`python3-tk` 或 `python3-tkinter` 等系统包。
- `cli-stressor-opencl/` 需要 OpenCL 运行环境。

不同 GPU 世代与后端支持的功能不同，请以 auto-optimizer README 中的兼容性矩阵为准。

## 快速开始

克隆 monorepo：

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

构建优化器：

```bash
cd auto-optimizer
cargo build --release
```

构建完成后，可从仓库根目录运行图形或终端前端：

```bash
cd gui
uv sync
uv run python main.py
```

或：

```bash
cd tui
uv sync
uv run nvoc-tui
```

如需使用 autoscan，请配置压力测试模块，并阅读 [auto-optimizer/test/](./auto-optimizer/test/) 下的脚本。CUDA 压力测试默认配置面向 8G+ 显存显卡；6G-8G 显存显卡请使用 `cli-stressor-cuda-rs-6g-8g.toml`。

## 许可证

本仓库采用 Apache License 2.0 许可发布。完整条款见 [LICENSE](./LICENSE)，仓库级声明见 [NOTICE](./NOTICE)。
