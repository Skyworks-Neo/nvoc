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

| Component                              | Path | Purpose |
|----------------------------------------|---|---|
| NVOC-AUTO-OPTIMIZER                    | [auto-optimizer/](./auto-optimizer/) | Rust CLI for V-F curve export/import, autoscan, result fixing, and retained VFP reset workflows. Use NVOC-CLI for GPU discovery, status, general resets, and NVAPI / NVML setting writes. |
| NVOC-CLI                               | [cli/](./cli/) | Focused Rust wrapper over `nvoc-core` with flat function-style commands and NVAPI/NVML backend selection. |
| NVOC-STRESSOR CUDA (Rust, recommended) | [cli-stressor-cuda-rs/](./cli-stressor-cuda-rs/) | Rust CUDA stress tool for CUDA-capable systems and native Rust pipeline usage. |
| NVOC-STRESSOR OpenCL                   | [cli-stressor-opencl/](./cli-stressor-opencl/) | Lightweight OpenCL stress tool for broader backend coverage without CUDA-specific dependencies. |
| NVOC-GUI                               | [gui/](./gui/) | Python GUI frontend for dashboard, autoscan, overclock, V-F curve, fan control, and live CLI output workflows. |
| NVOC-TUI                               | [tui/](./tui/) | Textual terminal UI frontend for machines where a desktop GUI is unavailable or undesirable. |
| NVOC-SRV                               | [srv/](./srv/) | Windows service and localhost HTTP control layer for server, workstation, and managed-machine use cases. |

### Internal libraries and experimental modules

| Component | Path | Scope |
|---|---|---|
| NVOC-CORE | [core/](./core/) | Core overclocking/domain library shared by Rust components. |
| NVOC-CLI-COMMON | [cli-common/](./cli-common/) | Shared CLI support layer for Rust command-line components. |
| NVOC-PYTHON (pynvoc) | [nvoc-python/](./nvoc-python/) | Python bindings and shared Python-side integration surface. |

Read the component README before building or running that component. The backend compatibility matrix lives in [cli/README.md](./cli/README.md#compatibility-overview-interface--gpu-generation--basic-functions); the autoscan command reference and scanning theory live in [auto-optimizer/README-en.md](./auto-optimizer/README-en.md) and [auto-optimizer/README.md](./auto-optimizer/README.md).

## Requirements

- NVIDIA GPU with a compatible driver.
- Administrator privileges on Windows or sudo privileges on Linux for operations that write overclocking settings.
- Rust toolchain for `auto-optimizer/` and `srv/`.
- Python plus `uv` for the Python frontends and OpenCL stressor.
- Python Tk support (`tkinter`) for NVOC-GUI. On Linux this may require a
  system package such as `tk`, `python3-tk`, or `python3-tkinter`.
- OpenCL runtime support for `cli-stressor-opencl/`.

Platform support and feature availability vary by GPU generation and backend. The CLI README contains the current support matrix for RTX 50/40/30/20, GTX 16/10/9, Volta, mobile GPUs, NVAPI, and NVML.

## Quick Start

### 1 — Clone the monorepo

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

Initialize the `nvapi-rs` submodule and switch it to the required branch:

```bash
git submodule init
git submodule update
cd nvapi-rs
git checkout v0.2.x
cd ..
```

### 2 — Install the Rust toolchain (rustup)

The project requires Rust **1.95.0** with the `minimal` profile, `clippy`, and `rustfmt`.

**Linux:**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Choose the default (1) installation, then reload the shell:
source "$HOME/.cargo/env"
# The repo's rust-toolchain.toml pins the exact channel; cargo/rustc will
# auto-install the correct version on first use.
```

**Windows:**

Download and run the installer from [https://rustup.rs](https://rustup.rs), or use `winget`:

```powershell
winget install Rustlang.Rustup
# After installation, restart the terminal.
# rust-toolchain.toml pins the exact channel; cargo/rustc will auto-install
# the correct version on first use.
```

Verify:

```bash
cargo --version    # should resolve to 1.95.0 via rust-toolchain.toml
rustc --version
```

### 3 — Install uv (Python package manager)

**Linux:**

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**Windows:**

```powershell
winget install astral-sh.uv
# — or via pip —
pip install uv
```

Verify:

```bash
uv --version
```

### 4 — Build the Rust optimizer

```bash
cd auto-optimizer
cargo build --release
```

The binary is emitted at `auto-optimizer/target/release/nvoc-auto-optimizer` (Linux) or
`auto-optimizer\target\release\nvoc-auto-optimizer.exe` (Windows).

### CUDA stressor and single-binary optimizer

The default `nvoc-auto-optimizer` build embeds `cli-stressor-cuda-rs` and self-spawns a worker
subprocess for CUDA isolation. A fatal CUDA failure therefore terminates the worker rather than
running inside the optimizer process. The standalone `cli-stressor-cuda-rs` binary remains
available for direct stress testing and for optimizer builds using `stressor-external`; releases
publish it as a separate versioned executable rather than placing it in the optimizer tools
archive. Releases also publish `nvoc-auto-optimizer` directly as the bundled, single executable;
release packaging no longer creates a separate tools/stressor archive.

Feature flags control the backends:

| Flag | Enables | Dependencies |
|---|---|---|
| `cuda` | CUDA GEMM / memcpy / reduction / atomic kernels | `cudarc`, `half`; needs CUDA Toolkit + driver at runtime |
| `vulkan` | Vulkan graphics stress (3D render workloads) | `ash`; needs Vulkan driver at runtime |

**Build with CUDA only:**

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda
```

**Build with CUDA + Vulkan:**

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda,vulkan
```

**Quick test run:**

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

**Run with a config file:**

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --profile standard
```

> **Windows runtime DLLs:** `nvrtc64_*.dll`, `cublasLt64_*.dll`, `cublas64_*.dll`, `cudart64_*.dll`
> must be on `PATH` or next to the executable (names vary by CUDA version).
>
> **Linux runtime `.so` files:** `libnvrtc.so.*`, `libcublasLt.so.*`, `libcublas.so.*`,
> `libcudart.so.*` must be discoverable by the dynamic loader. Set `LD_LIBRARY_PATH` or
> configure `ldconfig`.
>
> **Default features are empty** — building without `--features cuda` compiles a stub that
> skips all CUDA paths (used for non-GPU CI). The `cudarc` dependency pins
> `cuda-12090` (CUDA 12.9). For broader GPU coverage, see the compatibility notes in
> [cli-stressor-cuda-rs/README.md](./cli-stressor-cuda-rs/README.md).

### 5 — Build the native Python bindings (pynvoc)

The Python frontends depend on `pynvoc`, a native extension built with maturin from
`nvoc-python/`. Run this from the repository root:

```bash
cd nvoc-python
uv sync
uv run maturin develop --release
cd ..
```

> **Note:** This step requires the Rust toolchain (step 2) because `pynvoc` compiles
> Rust code via PyO3. On Windows you also need the MSVC build tools (installed with
> "Desktop development with C++" in Visual Studio Build Tools).

### 6 — Run a frontend

**GUI (requires tkinter):**

```bash
cd gui
uv sync
uv run python main.py
```

**TUI:**

```bash
cd tui
uv sync
uv run nvoc-tui
```

### 7 — Package a standalone executable (PyInstaller)

PyInstaller produces OS-specific executables; build on the target OS.

#### NVOC-GUI

**Linux:**

```bash
cd gui
uv sync --group build
uv run --frozen --no-editable --group build pyinstaller --clean --noconfirm nvoc_gui.spec
# Output: gui/dist/NVOC-GUI
```

**Windows:**

```powershell
Set-Location gui
uv sync --group build
uv run --frozen --no-editable --group build pyinstaller --clean --noconfirm nvoc_gui.spec
# Output: gui\dist\NVOC-GUI.exe
```

#### NVOC-TUI

**Linux:**

```bash
cd tui
uv sync --group dev
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
# Output: tui/dist/nvoc-tui
```

**Windows:**

```powershell
Set-Location tui
uv sync --group dev
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
# Output: tui\dist\nvoc-tui.exe
```

> **Note:** If a Linux build environment cannot write to the workspace venv or
> Cargo target directory, redirect those paths:
>
> ```bash
> UV_PROJECT_ENVIRONMENT=/tmp/nvoc-venv \
> UV_CACHE_DIR=/tmp/uv-cache \
> CARGO_HOME=/tmp/cargo-home \
> CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
> uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
> ```

### Autoscan & Stress Testing

Use `nvoc-auto-optimizer optimize --gpu <id>` for the integrated autoscan workflow. It selects
the embedded `low-vram` profile for 6–8 GiB GPUs and `standard` above 8 GiB; GPUs below 6 GiB
require an explicit profile/config override. The legacy low-level autoscan commands remain available.

Do not use the OpenCL stressor as the final stability gate for overclocking. It is not high-pressure enough and can produce autoscan / V-F curve results higher than the GPU can safely sustain, which may cause instability or hardware failure. Revalidate OpenCL-derived results with the CUDA stressor or heavier real workloads before relying on them.

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

| 组件                          | 路径 | 用途 |
|-----------------------------|---|---|
| NVOC-AUTO-OPTIMIZER         | [auto-optimizer/](./auto-optimizer/) | Rust CLI，负责 V-F 曲线导入导出、autoscan、结果后处理和保留的 VFP 重置流程。GPU 发现、状态读取、通用重置和 NVAPI / NVML 写入由 NVOC-CLI 负责。 |
| NVOC-CLI                    | [cli/](./cli/) | `nvoc-core` 的精简 Rust 包装器，提供扁平函数式命令和 NVAPI/NVML 后端选择。 |
| NVOC-STRESSOR CUDA（Rust，推荐） | [cli-stressor-cuda-rs/](./cli-stressor-cuda-rs/) | 面向 CUDA 环境与 Rust 原生流水线的 Rust CUDA 压力测试工具。 |
| NVOC-STRESSOR OpenCL        | [cli-stressor-opencl/](./cli-stressor-opencl/) | 轻量 OpenCL 压力测试工具，用于不依赖 CUDA 专有依赖的后端覆盖。 |
| NVOC-GUI                    | [gui/](./gui/) | Python 图形界面，提供 Dashboard、Autoscan、Overclock、V-F Curve、Fan Control 和实时 CLI 输出。 |
| NVOC-TUI                    | [tui/](./tui/) | 基于 Textual 的终端界面，适用于没有桌面环境或不适合运行 GUI 的机器。 |
| NVOC-SRV                    | [srv/](./srv/) | Windows Service 与 localhost HTTP 控制层，面向服务器、工作站和托管机器场景。 |

### 内部库与实验模块

| 组件 | 路径 | 说明 |
|---|---|---|
| NVOC-CORE | [core/](./core/) | Rust 共享核心库，承载超频/设备领域能力。 |
| NVOC-CLI-COMMON | [cli-common/](./cli-common/) | Rust CLI 共享支撑层。 |
| NVOC-PYTHON (pynvoc) | [nvoc-python/](./nvoc-python/) | Python 绑定与跨 Python 组件共享接口。 |

构建或运行某个组件前，请先阅读对应子目录 README。后端兼容性矩阵在 [cli/README.md](./cli/README.md#compatibility-overview-interface--gpu-generation--basic-functions) 中；autoscan 命令参考和扫描原理在 [auto-optimizer/README.md](./auto-optimizer/README.md) 和 [auto-optimizer/README-en.md](./auto-optimizer/README-en.md) 中。

## 依赖

- 支持的 NVIDIA GPU 与驱动。
- Windows 管理员权限，或 Linux sudo 权限，用于写入超频参数。
- `auto-optimizer/` 与 `srv/` 需要 Rust 工具链。
- Python 前端与 OpenCL 压力测试工具推荐使用 `uv` 管理环境。
- NVOC-GUI 需要 Python Tk 支持（`tkinter`）。Linux 上可能需要安装
  `tk`、`python3-tk` 或 `python3-tkinter` 等系统包。
- `cli-stressor-opencl/` 需要 OpenCL 运行环境。

不同 GPU 世代与后端支持的功能不同，请以 CLI README 中的兼容性矩阵为准。

## 快速开始

### 1 — 克隆 monorepo

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
```

初始化 `nvapi-rs` 子模块并切换到所需分支：

```bash
git submodule init
git submodule update
cd nvapi-rs
git checkout v0.2.x
cd ..
```

### 2 — 安装 Rust 工具链（rustup）

项目要求 Rust **1.95.0**，使用 `minimal` 配置文件，包含 `clippy` 和 `rustfmt`。

**Linux：**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# 选择默认安装（1），然后重新加载 shell：
source "$HOME/.cargo/env"
# 仓库的 rust-toolchain.toml 固定了工具链版本；cargo/rustc 会在首次使用时
# 自动安装正确版本。
```

**Windows：**

从 [https://rustup.rs](https://rustup.rs) 下载并运行安装程序，或使用 `winget`：

```powershell
winget install Rustlang.Rustup
# 安装完成后重启终端。
# rust-toolchain.toml 固定了工具链版本；cargo/rustc 会在首次使用时
# 自动安装正确版本。
```

验证：

```bash
cargo --version    # 应通过 rust-toolchain.toml 解析为 1.95.0
rustc --version
```

### 3 — 安装 uv（Python 包管理器）

**Linux：**

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**Windows：**

```powershell
winget install astral-sh.uv
# — 或通过 pip —
pip install uv
```

验证：

```bash
uv --version
```

### 4 — 构建 Rust 优化器

```bash
cd auto-optimizer
cargo build --release
```

构建产物位于 `auto-optimizer/target/release/nvoc-auto-optimizer`（Linux）或
`auto-optimizer\target\release\nvoc-auto-optimizer.exe`（Windows）。

###（可选）构建 CUDA 压力测试工具

CUDA 压力测试工具（`cli-stressor-cuda-rs`）是可选的，仅在使用真实 GPU 负载进行
autoscan 稳定性验证时需要。构建机器需要安装 **CUDA Toolkit** 或手动提取并提供库文件。

通过 feature flag 控制后端：

| Flag | 启用功能 | 依赖 |
|---|---|---|
| `cuda` | CUDA GEMM / memcpy / reduction / atomic 内核 | `cudarc`、`half`；运行时需要 CUDA Toolkit + 驱动 |
| `vulkan` | Vulkan 图形压力测试（ 3D 渲染负载） | `ash`；运行时需要 Vulkan 驱动 |

**仅启用 CUDA 构建：**

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda
```

**启用 CUDA + Vulkan 构建：**

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda,vulkan
```

**快速测试运行：**

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

**使用配置文件运行：**

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --profile standard
```

> **Windows 运行时 DLL：** `nvrtc64_*.dll`、`cublasLt64_*.dll`、`cublas64_*.dll`、`cudart64_*.dll`
> 需在 `PATH` 中或与可执行文件同目录（名称因 CUDA 版本而异）。
>
> **Linux 运行时 `.so` 文件：** `libnvrtc.so.*`、`libcublasLt.so.*`、`libcublas.so.*`、
> `libcudart.so.*` 需可被动态链接器找到。可设置 `LD_LIBRARY_PATH` 或配置 `ldconfig`。
>
> **默认 feature 为空**——不带 `--features cuda` 构建会编译一个跳过所有 CUDA 路径的
> 存根（用于无 GPU 的 CI）。`cudarc` 依赖固定为 `cuda-12090`（CUDA 12.9）。
> 如需兼容更多 GPU 架构，请参阅 [cli-stressor-cuda-rs/README.md](./cli-stressor-cuda-rs/README.md)
> 中的兼容性说明。

### 5 — 构建原生 Python 绑定（pynvoc）

Python 前端依赖 `pynvoc`，这是一个通过 maturin 从 `nvoc-python/` 构建的原生扩展。
从仓库根目录执行：

```bash
cd nvoc-python
uv sync
uv run maturin develop --release
cd ..
```

> **注意：** 此步骤需要 Rust 工具链（步骤 2），因为 `pynvoc` 通过 PyO3 编译 Rust 代码。
> 在 Windows 上还需要 MSVC 构建工具（通过 Visual Studio Build Tools 安装
> "Desktop development with C++"）。

### 6 — 运行前端

**GUI（需要 tkinter）：**

```bash
cd gui
uv sync
uv run python main.py
```

**TUI：**

```bash
cd tui
uv sync
uv run nvoc-tui
```

### 7 — 打包为独立可执行文件（PyInstaller）

PyInstaller 生成对应平台的可执行文件，需在目标操作系统上构建。

#### NVOC-GUI

**Linux：**

```bash
cd gui
uv sync --group build
uv run --frozen --no-editable --group build pyinstaller --clean --noconfirm nvoc_gui.spec
# 产物：gui/dist/NVOC-GUI
```

**Windows：**

```powershell
Set-Location gui
uv sync --group build
uv run --frozen --no-editable --group build pyinstaller --clean --noconfirm nvoc_gui.spec
# 产物：gui\dist\NVOC-GUI.exe
```

#### NVOC-TUI

**Linux：**

```bash
cd tui
uv sync --group dev
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
# 产物：tui/dist/nvoc-tui
```

**Windows：**

```powershell
Set-Location tui
uv sync --group dev
uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
# 产物：tui\dist\nvoc-tui.exe
```

> **注意：** 如果 Linux 构建环境无法写入 workspace venv 或 Cargo target 目录，
> 可以将这些路径重定向：
>
> ```bash
> UV_PROJECT_ENVIRONMENT=/tmp/nvoc-venv \
> UV_CACHE_DIR=/tmp/uv-cache \
> CARGO_HOME=/tmp/cargo-home \
> CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
> uv run --frozen --no-editable --group dev pyinstaller --clean --noconfirm nvoc_tui.spec
> ```

### Autoscan 与压力测试

如需使用 autoscan，请配置压力测试模块，并阅读 [auto-optimizer/test/](./auto-optimizer/test/) 下的脚本。CUDA 压力测试默认配置面向 8G+ 显存显卡；6G-8G 显存显卡请使用 `cli-stressor-cuda-rs-6g-8g.toml`。

不要将 OpenCL stressor 作为超频最终稳定性判据。它的压力不足，可能得到高于 GPU 安全承受范围的 autoscan / V-F 曲线结果，并进一步导致不稳定或硬件故障。依赖 OpenCL 结果前，请使用 CUDA stressor 或更重的真实负载重新验证。

## 许可证

本仓库采用 Apache License 2.0 许可发布。完整条款见 [LICENSE](./LICENSE)，仓库级声明见 [NOTICE](./NOTICE)。
