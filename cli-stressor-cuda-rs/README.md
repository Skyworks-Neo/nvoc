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
  - [注意事项](#注意事项)
- [English](#english)
  - [Overview](#overview)
  - [Features](#features)
  - [Requirements](#requirements)
  - [Build & Run](#build--run)
  - [Notes](#notes)

---

## 中文

### 概述

`cli-stressor-cuda-rs` 是一个用 Rust 编写的 CUDA GEMM 压力测试工具，参考 `cli-stressor-cuda` 的设计实现。它通过随机化 GEMM 负载和周期性的 CPU 侧校验，帮助发现静默数据损坏和硬件稳定性问题。

### 功能特点

- 随机化的 GEMM 尺寸，包含预热阶段和突发阶段
- 支持的精度模式：FP64、FP32、TF32、FP16、BF16（SM80+ 支持）；FP8 目前仅解析，尚未实现
- 使用 CPU FP64 参考结果进行周期性校验

### 环境要求

- NVIDIA GPU，且已安装 CUDA 驱动/工具包
- Rust 1.70+

### 构建与运行

CUDA 支持通过 feature flag 控制。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

### 注意事项

- TF32/BF16 使用 cuBLAS 的数学模式切换；BF16 需要 SM80+，更旧的 GPU 会给出明确提示并跳过；FP8 目前尚未实现。
- 校验路径使用 CPU FP64 GEMM，并按元素比较容差。
- 兼容性总结（2026-05-13 记录）：
  - CUDA 13 + `cuda13.dll` 在 10 系 GPU / Pascal 上不可用，编译成功也可能跑不起来。
  - 较老驱动组合下，CUDA 13 可能出现 `CUBLAS_STATUS_NOT_INITIALIZED` 或 `CUBLAS_STATUS_ARCH_MISMATCH`。
  - 更稳妥的发布方式是同时提供 CUDA 12.x 和 CUDA 13.x 两套构建；其中 CUDA 12.x 至少可覆盖到 Maxwell。
  - CUDA 13 需要足够新的显卡和匹配的驱动；例如 40 系 GPU 搭配 CUDA 13 与新驱动（如 595 / CUDA 13.2）可正常工作。
  - 客户端部署时要确保驱动支持性与 CUDA 版本都和目标 GPU 架构匹配。
  - 结论：若要兼顾老卡与新卡，推荐按 CUDA 12.x / 13.x 分开打包与发布，并在客户端侧按 GPU 架构选择对应版本。

---

<a id="english"></a>

## English

### Overview

`cli-stressor-cuda-rs` is a Rust-based CUDA GEMM stress tool modeled after `cli-stressor-cuda`. It uses randomized GEMM workloads and periodic CPU-side validation to help detect silent data corruption and hardware stability issues.

### Features

- Randomized GEMM sizes with warmup and burst phases
- Precision modes: FP64, FP32, TF32, FP16, BF16 (SM80+ required); FP8 is parsed but not implemented
- Periodic validation using a CPU FP64 reference result

### Requirements

- NVIDIA GPU with the CUDA driver/toolkit installed
- Rust 1.70+

### Build & Run

CUDA support is behind a feature flag.

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

### Notes

- TF32 and BF16 use cuBLAS math-mode switching. BF16 requires SM80+; older GPUs receive a clear message and the precision is skipped. FP8 is not yet implemented.
- The validation path uses CPU FP64 GEMM and compares values with element-wise tolerances.
- Compatibility summary (recorded on 2026-05-13):
  - CUDA 13 with `cuda13.dll` does not run on 10-series GPUs / Pascal, even if it builds successfully.
  - Older driver combinations may fail with `CUBLAS_STATUS_NOT_INITIALIZED` or `CUBLAS_STATUS_ARCH_MISMATCH`.
  - The safer release strategy is to ship both CUDA 12.x and CUDA 13.x builds; CUDA 12.x reaches at least Maxwell.
  - CUDA 13 requires a sufficiently new GPU and a matching driver; for example, RTX 40-series GPUs work normally with CUDA 13 and a newer driver (such as 595 / CUDA 13.2).
  - In client deployments, driver support and CUDA version must match the target GPU architecture.
  - Conclusion: to cover both legacy and newer GPUs, package and release separate CUDA 12.x and CUDA 13.x builds, then select the appropriate one on the client side by GPU architecture.

