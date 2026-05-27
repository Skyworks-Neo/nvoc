# NVOC Wiki

> **NVOC** — NVIDIA GPU Overclocking & Control Toolset

NVOC 是一个 Rust/Python 混合 monorepo，用于 NVIDIA GPU 超频、V-F 曲线自动扫描、压力测试与稳定性验证。

⚠️ **安全提醒**：超频可能导致显示驱动崩溃、GPU 重置或系统不稳定。执行写入操作前，请确认目标 GPU、驱动、散热和恢复路径。

---

## 快速导航

| 页面 | 说明 |
|---|---|
| [[Architecture]] | 项目架构与仓库结构 |
| [[Build-and-Install]] | 编译、安装与环境配置 |
| [[GPU-Support-Matrix]] | GPU 世代 × 接口兼容性矩阵 |
| [[Auto-Optimizer-Guide]] | Auto-Optimizer 核心功能使用指南 |
| [[Autoscan-Workflow]] | V-F 曲线自动扫描完整流程 |
| [[Stress-Testing]] | 压力测试工具（CUDA / OpenCL / Rust） |
| [[GUI-Guide]] | GUI 图形界面使用指南 |
| [[TUI-Guide]] | TUI 终端界面使用指南 |
| [[SRV-Guide]] | Windows Service / HTTP 控制层 |
| [[Contributing]] | 贡献指南 |

## 组件一览

| 组件 | 路径 | 用途 |
|---|---|---|
| **NVOC-AUTO-OPTIMIZER** | `auto-optimizer/` | Rust CLI 核心：GPU 发现、状态读取、NVAPI/NVML 写入、V-F 曲线管理、autoscan |
| **NVOC-STRESSOR CUDA** | `cli-stressor-cuda/` | 基于 PyTorch CUDA 的 GPU 核心稳定性压力测试 |
| **NVOC-STRESSOR OpenCL** | `cli-stressor-opencl/` | 轻量 OpenCL 压力测试，不依赖 CUDA PyTorch |
| **NVOC-STRESSOR CUDA RS** | `cli-stressor-cuda-rs/` | Rust 原生 CUDA GEMM 压力测试 |
| **NVOC-GUI** | `gui/` | Python 图形界面前端 |
| **NVOC-TUI** | `tui/` | Python Textual 终端界面前端 |
| **NVOC-SRV** | `srv/` | Windows Service 与 localhost HTTP 控制层 |

## 快速开始

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

## 许可证

Apache License 2.0 — 详见 [LICENSE](https://github.com/Skyworks-Neo/nvoc/blob/main/LICENSE)
