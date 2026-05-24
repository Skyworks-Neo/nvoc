# cli-stressor-cuda-rs

> Language switch / 语言切换: [中文](#中文) | [English](#english)
>
> License / 许可证: [Apache 2.0](../LICENSE)
>
> 本 monorepo 根目录的 `LICENSE` 适用于所有 NVOC 组件。

---

## 目录

- [中文](#中文)
  - [概述](#概述)
  - [功能特点](#功能特点)
  - [环境要求](#环境要求)
  - [构建与运行](#构建与运行)
  - [配置文件](#配置文件)
  - [注意事项](#注意事项)
- [English](#english)
  - [Overview](#overview)
  - [Features](#features)
  - [Requirements](#requirements)
  - [Build & Run](#build--run)
  - [Config File](#config-file)
  - [Notes](#notes)

---

## 中文

### 概述

`cli-stressor-cuda-rs` 是一个用 Rust 编写的 CUDA GEMM 压力测试工具，参考 `cli-stressor-cuda` 的设计实现。它通过随机化 GEMM 负载和周期性的 CPU 侧校验，帮助发现静默数据损坏和硬件稳定性问题。

### 功能特点

- 随机化的 GEMM 尺寸，包含预热阶段和突发阶段
- 支持的精度模式：FP64、FP32、TF32、FP16、BF16（SM80+ 架构，如 Ampere 及以后）；FP8 尚未实现
- 使用 CPU FP64 参考结果进行周期性校验

### 环境要求

- NVIDIA GPU，且已安装 CUDA 驱动/工具包
- Rust 1.70+
- Windows 运行时需确保以下 DLL （*取决于CUDA版本）可被加载（位于系统 PATH 或程序同目录）：
  - `nvrtc64_*.dll`
  - `cublasLt64_*.dll`
  - `cublas64_*.dll`
  - `cudart64_*.dll`

### 构建与运行

CUDA 支持通过 feature flag 控制。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

可选：通过配置文件运行（所有选项都可放入 config）。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --config ./stressor.toml
```

### 配置文件

- 参数优先级：`命令行显式传入 > config 文件 > 内置默认值`
- `kernel_mixture` 支持两种写法：
  - 字符串：`"gemm:0.4,memcpy:0.3,reduction:0.3"`
  - 映射：`{ gemm = 0.4, memcpy = 0.3, reduction = 0.3 }`
- `kernel_params.<kernel>` 支持按 kernel 覆盖参数，包括 `precisions`

示例（`stressor.toml`）：

```toml
duration = 120
matrix_sizes = [2049, 4096, 8192]
fp64_matrix_sizes = [2048, 4096]
precisions = ["fp16", "bf16", "tf32"]
warmup_iters = 3
burst_iters = 6
validate_interval = 10.0
validate_size = 1024
transpose_prob = 0.5
minor_mixture_rate = 0.15
seed = 12345
kernel_types = ["gemm", "memcpy", "reduction", "atomic"]
kernel_mixture = { gemm = 0.4, memcpy = 0.3, reduction = 0.2, atomic = 0.1 }
stream_mode = "dual"
disable_fp8 = true

[kernel_params.gemm]
precisions = ["fp16", "bf16"]
matrix_sizes = [4096, 8192]
warmup_iters = 4
burst_iters = 8

[kernel_params.reduction]
precisions = ["fp32"]
burst_iters = 64
```

### 注意事项

- TF32 和 BF16 均使用 cuBLAS 的数学模式切换；BF16 需要 SM80+（Ampere 及以后），旧架构会自动跳过并给出明确提示。FP8 尚未实现。
- 校验路径使用 CPU FP64 GEMM，并按元素比较容差。
- 兼容性总结（2026-05-13 记录）：
  - CUDA 13 + `cuda13.dll` 在 10 系 GPU / Pascal 上不可用，编译成功也可能跑不起来。
  - 较老驱动组合下，CUDA 13 可能出现 `CUBLAS_STATUS_NOT_INITIALIZED` 或 `CUBLAS_STATUS_ARCH_MISMATCH`。
  - 更稳妥的发布方式是同时提供 CUDA 12.x 和 CUDA 13.x 两套构建；其中 CUDA 12.x 至少可覆盖到 Maxwell。
  - CUDA 13 需要足够新的显卡和匹配的驱动；例如 40 系 GPU 搭配 CUDA 13 与新驱动（如 595 / CUDA 13.2）可正常工作。
  - 客户端部署时要确保驱动支持性与 CUDA 版本都和目标 GPU 架构匹配。
  - 结论：若要兼顾老卡与新卡，推荐按 CUDA 12.x / 13.x 分开打包与发布，并在客户端侧按 GPU 架构选择对应版本。
- `atomic` kernel path 在本项目中建议 SM80+ 使用；在 SM75（Turing）及以下默认会自动禁用该 path，避免执行期失败。
- Linux 对应依赖是同版本 `.so` 动态库（典型如 `libnvrtc.so.*`、`libcublasLt.so.*`、`libcublas.so.*`、`libcudart.so.*`）。请确保动态链接器可找到它们：
  - 临时方式：设置 `LD_LIBRARY_PATH` 指向 CUDA 库目录（如 `/usr/local/cuda/lib64`）
  - 持久方式：将 CUDA 库目录写入 `/etc/ld.so.conf.d/*.conf` 后执行 `ldconfig`
  - 可用 `ldd <binary>` 检查是否有 `not found` 依赖

---

<a id="english"></a>

## English

### Overview

`cli-stressor-cuda-rs` is a Rust-based CUDA GEMM stress tool modeled after `cli-stressor-cuda`. It uses randomized GEMM workloads and periodic CPU-side validation to help detect silent data corruption and hardware stability issues.

### Features

- Randomized GEMM sizes with warmup and burst phases
- Precision modes: FP64, FP32, TF32, FP16, BF16 (SM80+/Ampere and later); FP8 is not yet implemented
- Periodic validation using a CPU FP64 reference result

### Requirements

- NVIDIA GPU with the CUDA driver/toolkit installed
- Rust 1.70+
- On Windows, make sure these DLLs (* depends on CUDA version) are discoverable (in `PATH` or next to the executable):
  - `nvrtc64_*.dll`
  - `cublasLt64_*.dll`
  - `cublas64_*.dll`
  - `cudart64_*.dll`

### Build & Run

CUDA support is behind a feature flag.

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

Optional: run with a config file (all options can be provided in config).

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --config ./stressor.toml
```

### Config File

- Precedence: `explicit CLI value > config file > built-in default`
- `kernel_mixture` supports two formats:
  - string: `"gemm:0.4,memcpy:0.3,reduction:0.3"`
  - map: `{ gemm = 0.4, memcpy = 0.3, reduction = 0.3 }`
- `kernel_params.<kernel>` supports per-kernel overrides, including `precisions`

Example (`stressor.toml`):

```toml
duration = 120
matrix_sizes = [2049, 4096, 8192]
fp64_matrix_sizes = [2048, 4096]
precisions = ["fp16", "bf16", "tf32"]
warmup_iters = 3
burst_iters = 6
validate_interval = 10.0
validate_size = 1024
transpose_prob = 0.5
minor_mixture_rate = 0.15
seed = 12345
kernel_types = ["gemm", "memcpy", "reduction", "atomic"]
kernel_mixture = { gemm = 0.4, memcpy = 0.3, reduction = 0.2, atomic = 0.1 }
stream_mode = "dual"
disable_fp8 = true

[kernel_params.gemm]
precisions = ["fp16", "bf16"]
matrix_sizes = [4096, 8192]
warmup_iters = 4
burst_iters = 8

[kernel_params.reduction]
precisions = ["fp32"]
burst_iters = 64
```

### Notes

- TF32 and BF16 both use cuBLAS math-mode switching. BF16 requires SM80+ (Ampere and later); older architectures skip it with a clear message. FP8 is not yet implemented.
- The validation path uses CPU FP64 GEMM and compares values with element-wise tolerances.
- Compatibility summary (recorded on 2026-05-13):
  - CUDA 13 with `cuda13.dll` does not run on 10-series GPUs / Pascal, even if it builds successfully.
  - Older driver combinations may fail with `CUBLAS_STATUS_NOT_INITIALIZED` or `CUBLAS_STATUS_ARCH_MISMATCH`.
  - The safer release strategy is to ship both CUDA 12.x and CUDA 13.x builds; CUDA 12.x reaches at least Maxwell.
  - CUDA 13 requires a sufficiently new GPU and a matching driver; for example, RTX 40-series GPUs work normally with CUDA 13 and a newer driver (such as 595 / CUDA 13.2).
  - In client deployments, driver support and CUDA version must match the target GPU architecture.
  - Conclusion: to cover both legacy and newer GPUs, package and release separate CUDA 12.x and CUDA 13.x builds, then select the appropriate one on the client side by GPU architecture.
- In this project, the `atomic` kernel path is recommended for SM80+ only; it is auto-disabled on SM75 (Turing) and below to avoid runtime failure.
- Linux equivalents are the matching `.so` runtime libraries (typically `libnvrtc.so.*`, `libcublasLt.so.*`, `libcublas.so.*`, `libcudart.so.*`). Ensure the dynamic loader can locate them:
  - Temporary: set `LD_LIBRARY_PATH` to CUDA library directories (e.g. `/usr/local/cuda/lib64`)
  - Persistent: add CUDA library directories to `/etc/ld.so.conf.d/*.conf` and run `ldconfig`
  - Use `ldd <binary>` to verify there are no `not found` dependencies
