# Project Architecture

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC uses a Rust/Python hybrid monorepo architecture. Core overclocking logic is implemented in Rust, while frontends and stress testing tools use Python.

### Repository Structure

```
nvoc/
├── auto-optimizer/       # Rust CLI core — GPU control, V-F curve, autoscan
├── nvoc-core/            # Shared Rust library (GPU types, utilities)
├── nvoc-cli-common/      # Shared CLI arguments and output utilities
├── nvoc-python/          # pynvoc Python bindings (PyO3)
├── cli-stressor-cuda/    # Python CUDA/PyTorch stress test
├── cli-stressor-cuda-rs/ # Rust native CUDA GEMM stress test
├── cli-stressor-opencl/  # Python OpenCL stress test
├── gui/                  # Python GUI frontend (PyQt)
├── tui/                  # Python TUI frontend (Textual)
├── srv/                  # Windows Service + HTTP control layer
├── nvapi-rs/             # NVAPI Rust bindings (fork)
└── Cargo.toml            # Workspace root configuration
```

### Layered Architecture

```
┌─────────────────────────────────────────────────┐
│                 Frontend Layer                   │
│   GUI (gui/)   │   TUI (tui/)   │   SRV (srv/)  │
├────────────────┴────────────────┴────────────────┤
│                 CLI Core Layer                   │
│          auto-optimizer (Rust binary)            │
│      invoked via subprocess or PyO3 bindings     │
├──────────────────────────────────────────────────┤
│              Stress Testing Layer                │
│  cli-stressor-cuda │ cli-stressor-opencl │ cuda-rs│
├──────────────────────────────────────────────────┤
│                 Low-Level API                    │
│    NVAPI (nvapi-rs)  │  NVML (nvml-wrapper)      │
└──────────────────────────────────────────────────┘
```

### Data Flow

1. **GUI/TUI** collects user input → builds command → calls `auto-optimizer` CLI
2. **auto-optimizer** interacts with GPU driver via NVAPI/NVML
3. During **autoscan**, auto-optimizer invokes cli-stressor for stability validation
4. Scan results are saved in the `ws/` working directory (CSV + logs), with breakpoint-resume support

### Rust Workspace

```toml
# Cargo.toml workspace members
members = [
  "nvoc-core",           # Shared types and utilities
  "nvoc-cli-common",     # CLI argument definitions
  "auto-optimizer",      # Core overclocking CLI
  "srv",                 # Windows Service
  "cli-stressor-cuda-rs",# Rust CUDA stress test
  "nvoc-python",         # Python bindings
]
resolver = "3"
edition = "2024"
rust-version = "1.95"
```

### Python Projects

Each Python component is managed with `uv` and has an independent `pyproject.toml`:

- `gui/` — PyQt GUI
- `tui/` — Textual TUI
- `cli-stressor-cuda/` — PyTorch CUDA stress test
- `cli-stressor-opencl/` — OpenCL stress test

### Key Dependencies

| Dependency | Purpose |
|---|---|
| `nvapi-hi` / `nvapi-sys` | NVAPI Rust bindings |
| `nvml-wrapper` | NVML Rust bindings |
| `clap` | CLI argument parsing |
| `serde` / `serde_json` | Serialization |
| `tiny_http` | SRV HTTP service |
| `windows-service` | Windows Service framework |
| PyTorch | CUDA stress testing |
| Textual | TUI framework |
| PyQt | GUI framework |

---

<a id="chinese"></a>

## 中文

NVOC 采用 Rust/Python 混合 monorepo 架构，核心超频逻辑由 Rust 实现，前端和压力测试工具使用 Python。

### 仓库结构

```
nvoc/
├── auto-optimizer/       # Rust CLI 核心 — GPU 控制、V-F 曲线、autoscan
├── nvoc-core/            # 共享 Rust 库（GPU 类型、通用工具）
├── nvoc-cli-common/      # 共享 CLI 参数与输出工具
├── nvoc-python/          # pynvoc Python 绑定（PyO3）
├── cli-stressor-cuda/    # Python CUDA/PyTorch 压力测试
├── cli-stressor-cuda-rs/ # Rust 原生 CUDA GEMM 压力测试
├── cli-stressor-opencl/  # Python OpenCL 压力测试
├── gui/                  # Python GUI 前端（PyQt）
├── tui/                  # Python TUI 前端（Textual）
├── srv/                  # Windows Service + HTTP 控制层
├── nvapi-rs/             # NVAPI Rust 绑定（fork）
└── Cargo.toml            # Workspace 根配置
```

### 分层架构

```
┌─────────────────────────────────────────────────┐
│                  前端层                          │
│   GUI (gui/)   │   TUI (tui/)   │   SRV (srv/)  │
├────────────────┴────────────────┴────────────────┤
│                 CLI 核心层                        │
│          auto-optimizer (Rust binary)             │
│          通过子进程调用或 PyO3 绑定                │
├──────────────────────────────────────────────────┤
│                压力测试层                         │
│  cli-stressor-cuda │ cli-stressor-opencl │ cuda-rs│
├──────────────────────────────────────────────────┤
│                底层 API                           │
│    NVAPI (nvapi-rs)  │  NVML (nvml-wrapper)       │
└──────────────────────────────────────────────────┘
```

### 数据流

1. **GUI/TUI** 收集用户输入 → 构建命令 → 调用 `auto-optimizer` CLI
2. **auto-optimizer** 通过 NVAPI/NVML 与 GPU 驱动交互
3. **autoscan** 流程中，auto-optimizer 调用 cli-stressor 进行稳定性验证
4. 扫描结果保存在 `ws/` 工作目录（CSV + 日志），支持断点续扫

### Rust Workspace

```toml
# Cargo.toml workspace members
members = [
  "nvoc-core",           # 共享类型和工具
  "nvoc-cli-common",     # CLI 参数定义
  "auto-optimizer",      # 核心超频 CLI
  "srv",                 # Windows Service
  "cli-stressor-cuda-rs",# Rust CUDA 压力测试
  "nvoc-python",         # Python 绑定
]
resolver = "3"
edition = "2024"
rust-version = "1.95"
```

### Python 项目

各 Python 组件使用 `uv` 管理，独立 `pyproject.toml`：

- `gui/` — PyQt GUI
- `tui/` — Textual TUI
- `cli-stressor-cuda/` — PyTorch CUDA 压力测试
- `cli-stressor-opencl/` — OpenCL 压力测试

### 关键依赖

| 依赖 | 用途 |
|---|---|
| `nvapi-hi` / `nvapi-sys` | NVAPI Rust 绑定 |
| `nvml-wrapper` | NVML Rust 绑定 |
| `clap` | CLI 参数解析 |
| `serde` / `serde_json` | 序列化 |
| `tiny_http` | SRV HTTP 服务 |
| `windows-service` | Windows 服务框架 |
| PyTorch | CUDA 压力测试 |
| Textual | TUI 框架 |
| PyQt | GUI 框架 |
