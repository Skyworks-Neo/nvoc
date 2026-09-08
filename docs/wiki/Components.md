# Components

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

This page mirrors the canonical component inventory in the monorepo `README.md` → "Components (canonical)". Read the component README before building or running that component.

## User-facing products

| Component | Path | Purpose |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | `auto-optimizer/` | Rust CLI for autoscan, V-F curve export/import, result fixing, and retained VFP reset workflows. Use NVOC-CLI for GPU discovery, status, general resets, and NVAPI/NVML setting writes. |
| NVOC-CLI | `cli/` | Focused Rust wrapper over `nvoc-core` with flat function-style commands and NVAPI/NVML backend selection. |
| NVOC-STRESSOR CUDA (Rust, recommended) | `cli-stressor-cuda-rs/` | Rust CUDA stress tool for CUDA-capable systems and native Rust pipeline usage. |
| NVOC-STRESSOR OpenCL | `cli-stressor-opencl/` | Lightweight OpenCL first-pass workload for broader backend coverage without CUDA-specific dependencies; not a final overclocking stability gate. |
| NVOC-GUI | `gui/` | Python GUI frontend for dashboard, autoscan, overclock, V-F curve, fan control, and live CLI output workflows. |
| NVOC-TUI | `tui/` | Textual terminal UI frontend for machines where a desktop GUI is unavailable or undesirable (headless, remote/SSH friendly). |
| NVOC-SRV | `srv/` | Windows service and localhost HTTP control layer for server, workstation, and managed-machine use cases. |

## Internal libraries and shared components

| Component | Path | Purpose |
|---|---|---|
| NVOC-CORE (`nvoc-core`) | `core/` | Core overclocking/domain library shared by Rust components: backend abstraction over NVAPI/NVML plus common types, errors, and operations. |
| NVOC-CLI-COMMON | `cli-common/` | Shared CLI support layer for Rust command-line components. |
| NVOC-PYTHON (pynvoc) | `nvoc-python/` | Python bindings and shared Python-side integration surface used by the Python frontends. |

## Removed components

- The Python/PyTorch CUDA stressor `cli-stressor-cuda/` has been **removed** from the repository. Use the Rust CUDA stressor `cli-stressor-cuda-rs/` instead — see [[Stress-Testing]].

## Where to go next

- [[Auto-Optimizer-Guide]] and [[Autoscan-Workflow]] — autoscan and V-F curve workflows.
- [[CLI-Guide]] and [[CLI-Command-Reference]] — NVOC-CLI usage and per-command details.
- [[Stress-Testing]] — CUDA (Rust) vs OpenCL stressors.
- [[GUI-Guide]], [[TUI-Guide]], [[SRV-Guide]] — frontend guides.
- [[PyNvoc-API]] — Python-side integration surface.
- [[GPU-Support-Matrix]] — GPU generation × backend compatibility.
- [[Architecture]] — how the components fit together in the repository.

---

*Maintained from: `docs/wiki/Components.md`, `README.md` → "Components (canonical)", workspace `Cargo.toml`.*

<a id="chinese"></a>

## 中文

本页与 monorepo `README.md` → "Components (canonical)" 中的权威组件清单保持一致。在编译或运行某个组件之前，请先阅读该组件的 README。

## 用户可见的产品组件

| 组件 | 路径 | 用途 |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | `auto-optimizer/` | Rust CLI：autoscan、V-F curve 导出/导入、结果修复、保留 VFP 重置工作流。GPU 发现、状态读取、常规重置以及 NVAPI/NVML 设置写入请使用 NVOC-CLI。 |
| NVOC-CLI | `cli/` | 基于 `nvoc-core` 的专注 Rust 封装，提供扁平的函数式命令和 NVAPI/NVML 后端选择。 |
| NVOC-STRESSOR CUDA（Rust，推荐） | `cli-stressor-cuda-rs/` | Rust CUDA 压力测试工具，适用于支持 CUDA 的系统和原生 Rust 流水线。 |
| NVOC-STRESSOR OpenCL | `cli-stressor-opencl/` | 轻量 OpenCL 首轮压力测试，覆盖不依赖 CUDA 的后端；不能作为最终的超频稳定性门槛。 |
| NVOC-GUI | `gui/` | Python 图形界面前端：仪表盘、autoscan、超频、V-F curve、风扇控制和实时 CLI 输出。 |
| NVOC-TUI | `tui/` | 基于 Textual 的终端界面前端，适合无桌面 GUI 的场景（headless、远程/SSH 友好）。 |
| NVOC-SRV | `srv/` | Windows 服务与 localhost HTTP 控制层，面向服务器、工作站和受管机器。 |

## 内部库与共享组件

| 组件 | 路径 | 用途 |
|---|---|---|
| NVOC-CORE（`nvoc-core`） | `core/` | Rust 组件共享的超频/领域核心库：NVAPI/NVML 后端抽象，以及公共类型、错误与操作。 |
| NVOC-CLI-COMMON | `cli-common/` | Rust 命令行组件共享的 CLI 支撑层。 |
| NVOC-PYTHON（pynvoc） | `nvoc-python/` | Python 绑定与共享的 Python 侧集成接口，供 Python 前端使用。 |

## 已移除的组件

- Python/PyTorch CUDA 压力测试器 `cli-stressor-cuda/` 已从仓库中**移除**。请改用 Rust CUDA 压力测试器 `cli-stressor-cuda-rs/` —— 见 [[Stress-Testing]]。

## 后续阅读

- [[Auto-Optimizer-Guide]] 与 [[Autoscan-Workflow]] — autoscan 与 V-F curve 工作流。
- [[CLI-Guide]] 与 [[CLI-Command-Reference]] — NVOC-CLI 用法与逐命令说明。
- [[Stress-Testing]] — CUDA（Rust）与 OpenCL 压力测试器对比。
- [[GUI-Guide]]、[[TUI-Guide]]、[[SRV-Guide]] — 前端使用指南。
- [[PyNvoc-API]] — Python 侧集成接口。
- [[GPU-Support-Matrix]] — GPU 世代 × 后端兼容性。
- [[Architecture]] — 各组件在仓库中的组织方式。

---

*维护来源：`docs/wiki/Components.md`、`README.md` → "Components (canonical)"、workspace `Cargo.toml`。*
