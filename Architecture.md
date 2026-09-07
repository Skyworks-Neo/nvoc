# Project Architecture

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC uses a Rust/Python hybrid monorepo architecture. Core overclocking logic is implemented in Rust (`nvoc-core`, `nvoc-cli`, `auto-optimizer`), while the GUI/TUI frontends are Python (via the `pynvoc` PyO3 bindings) and the OpenCL stressor is Python.

### Repository Structure

```
nvoc/
├── auto-optimizer/       # Rust autoscan CLI core — V-F curve, autoscan orchestration
├── cli/                  # nvoc-cli — direct NVAPI/NVML control commands
├── core/                 # nvoc-core — shared Rust GPU domain library
├── cli-common/           # Shared CLI support layer for Rust command-line components
├── nvoc-python/          # pynvoc — native Python bindings (PyO3 + maturin)
├── gui/                  # Python GUI frontend (customtkinter)
├── tui/                  # Python TUI frontend (Textual)
├── srv/                  # Windows Service + localhost HTTP control layer
├── cli-stressor-cuda-rs/ # Rust native CUDA stress test (GEMM/memcpy/reduction/atomic)
├── cli-stressor-opencl/  # Python OpenCL stress test
├── nvapi-rs/             # NVAPI Rust bindings (fork, pinned as a git submodule)
├── docs/                 # Canonical long-form docs (docs/wiki/), synced to the GitHub Wiki
└── Cargo.toml            # Workspace root configuration
```

> The former Python/PyTorch stressor `cli-stressor-cuda/` was removed in #200; `cli-stressor-cuda-rs/` is the CUDA stressor now. Historically named directories `nvoc-core/` and `nvoc-cli-common/` are now `core/` and `cli-common/`.

### Layered Architecture

```
┌──────────────────────────────────────────────────────────┐
│                      Frontend Layer                      │
│     GUI (gui/)    │    TUI (tui/)    │    SRV (srv/)     │
├───────────────────┴──────────────────┴───────────────────┤
│                       CLI Layer                          │
│   nvoc-cli (cli/) — flat direct NVAPI/NVML commands      │
│   auto-optimizer — autoscan & V-F curve orchestration    │
├──────────────────────────────────────────────────────────┤
│                Access Path (for Python)                  │
│    pynvoc bindings (PyO3) │ CLI subprocess invocation    │
├──────────────────────────────────────────────────────────┤
│          nvoc-core (core/) shared domain library         │
├──────────────────────────────────────────────────────────┤
│                   Stress Testing Layer                   │
│     cli-stressor-cuda-rs │ cli-stressor-opencl           │
├──────────────────────────────────────────────────────────┤
│                      Low-Level API                       │
│       NVAPI (nvapi-rs fork)  │  NVML (nvml-wrapper)      │
└──────────────────────────────────────────────────────────┘
```

### Path Selection

- **CLI direct path**: `nvoc-cli` for direct NVAPI/NVML control commands (GPU discovery, status, resets, setting writes); direct `auto-optimizer` execution for scripting, autoscan, and low-level operations.
- **Frontend orchestration path**: GUI/TUI/SRV orchestrate user workflows and reach the hardware through pynvoc → nvoc-core → NVAPI/NVML (and/or by spawning the CLI binaries) with validated configuration.

Use the CLI for reproducible automation and troubleshooting; use the frontends for operator UX and guardrails.

### Data Flow

1. **GUI/TUI** collects user input → builds a validated command → calls `nvoc-cli` / `auto-optimizer` (subprocess) or `pynvoc` (native bindings)
2. **nvoc-core** interacts with the GPU driver via NVAPI/NVML
3. During **autoscan**, auto-optimizer invokes the embedded `cli-stressor-cuda-rs` stressor in a self-spawned worker subprocess for stability validation (CUDA isolation)
4. Scan results are saved in the `ws/` working directory (CSV + logs), with breakpoint-resume support

### Rust Workspace

```toml
# Cargo.toml workspace members
members = [
  "core",                 # nvoc-core — shared types and utilities
  "cli-common",           # nvoc-cli-common — shared CLI support
  "cli",                  # nvoc-cli — direct NVAPI/NVML commands
  "auto-optimizer",       # autoscan / V-F curve CLI core
  "srv",                  # Windows Service + HTTP control
  "cli-stressor-cuda-rs", # Rust CUDA stress test
  "nvoc-python",          # pynvoc Python bindings
]
exclude = ["nvapi-rs"]    # NVAPI bindings fork, pinned as a git submodule
resolver = "3"
edition = "2024"
rust-version = "1.95"
```

### Python Projects

Each Python component is managed with `uv` and has an independent `pyproject.toml`:

- `gui/` — customtkinter GUI
- `tui/` — Textual TUI
- `cli-stressor-opencl/` — OpenCL stress test

### Key Dependencies

| Dependency | Purpose |
|---|---|
| `nvapi` (in-repo `nvapi-rs` fork) | NVAPI Rust bindings |
| `nvml-wrapper` | NVML Rust bindings |
| `clap` | CLI argument parsing |
| `serde` / `serde_json` | Serialization |
| `cudarc` | CUDA runtime for `cli-stressor-cuda-rs` |
| `tiny_http` | SRV HTTP service |
| `windows-service` | Windows Service framework |
| PyO3 / maturin | pynvoc native Python bindings |
| Textual | TUI framework |
| customtkinter | GUI framework |

---

<a id="chinese"></a>

## 中文

NVOC 采用 Rust/Python 混合 monorepo 架构。核心超频逻辑由 Rust 实现（`nvoc-core`、`nvoc-cli`、`auto-optimizer`），GUI/TUI 前端为 Python（通过 `pynvoc` PyO3 绑定），OpenCL 压力测试工具为 Python。

### 仓库结构

```
nvoc/
├── auto-optimizer/       # Rust autoscan CLI 核心 — V-F 曲线、autoscan 编排
├── cli/                  # nvoc-cli — 直接 NVAPI/NVML 控制命令
├── core/                 # nvoc-core — 共享 Rust GPU 领域库
├── cli-common/           # Rust 命令行组件的共享支撑层
├── nvoc-python/          # pynvoc — 原生 Python 绑定（PyO3 + maturin）
├── gui/                  # Python GUI 前端（customtkinter）
├── tui/                  # Python TUI 前端（Textual）
├── srv/                  # Windows Service + HTTP 控制层
├── cli-stressor-cuda-rs/ # Rust 原生 CUDA 压力测试（GEMM/memcpy/reduction/atomic）
├── cli-stressor-opencl/  # Python OpenCL 压力测试
├── nvapi-rs/             # NVAPI Rust 绑定（fork，以 git 子模块固定）
├── docs/                 # 长篇文档权威来源（docs/wiki/），同步到 GitHub Wiki
└── Cargo.toml            # Workspace 根配置
```

> 原 Python/PyTorch 压力测试工具 `cli-stressor-cuda/` 已在 #200 中移除，现在的 CUDA 压力测试工具是 `cli-stressor-cuda-rs/`。历史目录名 `nvoc-core/`、`nvoc-cli-common/` 现为 `core/`、`cli-common/`。

### 分层架构

```
┌──────────────────────────────────────────────────────────┐
│                        前端层                             │
│     GUI (gui/)    │    TUI (tui/)    │    SRV (srv/)     │
├───────────────────┴──────────────────┴───────────────────┤
│                        CLI 层                             │
│   nvoc-cli (cli/) — 扁平的直接 NVAPI/NVML 命令             │
│   auto-optimizer — autoscan 与 V-F 曲线编排               │
├──────────────────────────────────────────────────────────┤
│                  （Python 的）访问路径                     │
│    pynvoc 绑定（PyO3） │ CLI 子进程调用                    │
├──────────────────────────────────────────────────────────┤
│           nvoc-core (core/) 共享领域库                    │
├──────────────────────────────────────────────────────────┤
│                      压力测试层                           │
│     cli-stressor-cuda-rs │ cli-stressor-opencl           │
├──────────────────────────────────────────────────────────┤
│                       底层 API                            │
│       NVAPI（nvapi-rs fork）  │  NVML（nvml-wrapper）     │
└──────────────────────────────────────────────────────────┘
```

### 路径选择

- **CLI 直连路径**：`nvoc-cli` 提供直接的 NVAPI/NVML 控制命令（GPU 发现、状态读取、重置、参数写入）；直接运行 `auto-optimizer` 用于脚本化、autoscan 和底层操作。
- **前端编排路径**：GUI/TUI/SRV 编排用户工作流，经 pynvoc → nvoc-core → NVAPI/NVML 到达硬件（和/或以校验过的配置启动 CLI 二进制）。

自动化与排障使用 CLI；交互运维与防呆保护使用前端。

### 数据流

1. **GUI/TUI** 收集用户输入 → 构建校验过的命令 → 调用 `nvoc-cli` / `auto-optimizer`（子进程）或 `pynvoc`（原生绑定）
2. **nvoc-core** 通过 NVAPI/NVML 与 GPU 驱动交互
3. **autoscan** 流程中，auto-optimizer 在自派生的 worker 子进程中调用内嵌的 `cli-stressor-cuda-rs` 压力测试进行稳定性验证（CUDA 隔离）
4. 扫描结果保存在 `ws/` 工作目录（CSV + 日志），支持断点续扫

### Rust Workspace

```toml
# Cargo.toml workspace members
members = [
  "core",                 # nvoc-core — 共享类型和工具
  "cli-common",           # nvoc-cli-common — 共享 CLI 支撑
  "cli",                  # nvoc-cli — 直接 NVAPI/NVML 命令
  "auto-optimizer",       # autoscan / V-F 曲线 CLI 核心
  "srv",                  # Windows Service + HTTP 控制
  "cli-stressor-cuda-rs", # Rust CUDA 压力测试
  "nvoc-python",          # pynvoc Python 绑定
]
exclude = ["nvapi-rs"]    # NVAPI 绑定 fork，以 git 子模块固定
resolver = "3"
edition = "2024"
rust-version = "1.95"
```

### Python 项目

各 Python 组件使用 `uv` 管理，独立 `pyproject.toml`：

- `gui/` — customtkinter GUI
- `tui/` — Textual TUI
- `cli-stressor-opencl/` — OpenCL 压力测试

### 关键依赖

| 依赖 | 用途 |
|---|---|
| `nvapi`（仓库内 `nvapi-rs` fork） | NVAPI Rust 绑定 |
| `nvml-wrapper` | NVML Rust 绑定 |
| `clap` | CLI 参数解析 |
| `serde` / `serde_json` | 序列化 |
| `cudarc` | `cli-stressor-cuda-rs` 的 CUDA 运行时 |
| `tiny_http` | SRV HTTP 服务 |
| `windows-service` | Windows 服务框架 |
| PyO3 / maturin | pynvoc 原生 Python 绑定 |
| Textual | TUI 框架 |
| customtkinter | GUI 框架 |
