# 压力测试

NVOC 提供三种压力测试工具，用于在 autoscan 流程中验证 GPU 稳定性或独立运行。

## 概览

| 工具 | 语言 | 后端 | 特点 |
|---|---|---|---|
| `cli-stressor-cuda` | Python | PyTorch CUDA | 功能最全，多精度 + 混合 kernel + 严格校验 |
| `cli-stressor-opencl` | Python | OpenCL | 轻量，不依赖 CUDA PyTorch |
| `cli-stressor-cuda-rs` | Rust | CUDA (cuBLAS) | 原生高性能，无 Python 依赖 |

## cli-stressor-cuda（Python + PyTorch）

### 安装

```bash
cd cli-stressor-cuda
uv sync
```

### 运行

```bash
uv run test.py [参数]
```

### 关键参数

| 参数 | 默认值 | 说明 |
|---|---|---|
| `--duration` | `90.0` | 每精度压力持续时间（秒） |
| `--precisions` | `fp16,bf16` | 精度列表：`fp64` `fp32` `tf32` `fp16` `bf16` `fp8` |
| `--matrix-sizes` | `2049,4096,4097,8192,8193,16384` | 随机矩阵尺寸 |
| `--validate-interval` | `10` | 旁路校验间隔（秒） |
| `--kernel-types` | `gemm,memcpy,memset,...` | 启用的 kernel 类型 |
| `--kernel-mixture` | 空（等权） | kernel 权重混合，如 `gemm:0.5,memcpy:0.3` |
| `--stream-mode` | `single` | 流并发模式：`single` `dual` `triple` |
| `--config` | — | TOML 配置文件 |

### 特性

- 多精度：FP64 / FP32 / TF32 / FP16 / BF16 / FP8
- 混合 kernel：GEMM / Memcpy / Memset / Transpose / Elementwise / Reduction / Atomic
- 旁路校验：CPU FP64 参考算法验证，捕获静默数据错误
- 随机化矩阵尺寸（含非对齐尺寸），制造冷热交替负载

## cli-stressor-opencl（Python + OpenCL）

### 安装

```bash
cd cli-stressor-opencl
uv sync
```

### 运行

```bash
uv run test.py [参数]
```

与 CUDA 版参数基本相同，精度支持 FP32 / FP16（及受支持设备的 FP64）。

### 适用场景

- 无 CUDA PyTorch 的环境
- 跨平台轻量测试
- 非 NVIDIA GPU 的基础压力测试

## cli-stressor-cuda-rs（Rust + CUDA）

### 构建

需要 CUDA Toolkit 和 CUDA runtime DLL/SO。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- [参数]
```

### 配置文件

支持 TOML 配置，优先级：`命令行 > config > 默认值`

```toml
duration = 120
precisions = ["fp16", "bf16", "tf32"]
matrix_sizes = [2049, 4096, 8192]
kernel_mixture = { gemm = 0.4, memcpy = 0.3, reduction = 0.2, atomic = 0.1 }

[kernel_params.gemm]
precisions = ["fp16", "bf16"]
matrix_sizes = [4096, 8192]
```

### CUDA 版本兼容性

| CUDA 版本 | 支持范围 |
|---|---|
| CUDA 12.x | Maxwell 及以上 |
| CUDA 13.x | Ampere 及以上（需要新驱动） |

建议按 CUDA 12.x / 13.x 分开打包发布，客户端按 GPU 架构选择。

## 判定标准

所有压力测试工具统一使用**进程返回码**判据：
- 返回 `0` = 通过（稳定）
- 非 `0` = 失败（不稳定）

autoscan 流程中，auto-optimizer 通过封装脚本调用 stressor，根据返回码决定升降频率偏移。
