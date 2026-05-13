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
- 支持的精度模式：FP64、FP32、TF32、FP16（BF16/FP8 目前仅解析，尚未实现）
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

- TF32 使用 cuBLAS 的数学模式切换；BF16/FP8 目前会被跳过，并给出明确提示。
- 校验路径使用 CPU FP64 GEMM，并按元素比较容差。

---

<a id="english"></a>

## English

### Overview

`cli-stressor-cuda-rs` is a Rust-based CUDA GEMM stress tool modeled after `cli-stressor-cuda`. It uses randomized GEMM workloads and periodic CPU-side validation to help detect silent data corruption and hardware stability issues.

### Features

- Randomized GEMM sizes with warmup and burst phases
- Precision modes: FP64, FP32, TF32, FP16 (BF16/FP8 are parsed but not implemented yet)
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

- TF32 uses cuBLAS math-mode switching. BF16/FP8 are currently skipped with a clear message.
- The validation path uses CPU FP64 GEMM and compares values with element-wise tolerances.

