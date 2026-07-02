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
  - [支持精度一览](#支持精度一览)
  - [环境要求](#环境要求)
  - [构建与运行](#构建与运行)
  - [配置文件](#配置文件)
  - [代际兼容性参考](#代际兼容性参考)
  - [注意事项](#注意事项)
- [English](#english)
  - [Overview](#overview)
  - [Features](#features)
  - [Precision Matrix](#precision-matrix)
  - [Requirements](#requirements)
  - [Build & Run](#build--run)
  - [Config File](#config-file)
  - [Generation Compatibility Reference](#generation-compatibility-reference)
  - [Notes](#notes)

---

## 中文

### 概述

`cli-stressor-cuda-rs` 是一个用 Rust 编写的 CUDA 计算压力测试工具。它通过随机化 GEMM / 整数 ALU / 访存 / 原子操作负载和周期性的 CPU 侧校验，帮助发现静默数据损坏和硬件稳定性问题。

### 功能特点

- 多 kernel 路径混合压力：
  - **GEMM**：cuBLAS FP64 / FP32 / TF32 / FP16 / BF16 矩阵乘，以及 INT8 cuBLAS GEMM（IMMA tensor core 加速）
  - **整数 ALU**（`intalu`）：自定义 NVRTC 内核，INT32 *MAD / INT16 窄化运算 / INT8 DP4A 点积* 链（Pascal+ `__dp4a`；旧架构自动回退为标量 MAD）
  - **访存**：memcpy / memset / transpose / elementwise / reduction
  - **原子操作**：自定义 NVRTC atomic 内核
  - 内核权重可调（`--kernel_mixture`），支持 per-kernel 参数覆盖
- 随机化矩阵尺寸，包含预热阶段和突发阶段
- 支持的精度模式：**FP64、FP32、TF32、FP16、BF16、INT8、INT16、INT32**；FP8 尚未实现
- 使用 CPU FP64 参考结果进行周期性校验（INT 精度跳过校验，详见[注意事项](#注意事项)）
- 多流提交模式：`single` / `dual` / `triple`
- 可选 Vulkan 图形压力侧车（`--enable-vulkan-stress`，需 `--features vulkan`）
- PCI 总线 / UUID / 排序索引 GPU 选择；`--list-gpus` 枚举设备

### 支持精度一览

| 精度 | CLI 别名 | Kernel 类型 | 底层引擎 | 代际要求 |
|---|---|---|---|---|
| FP64 | `fp64` | `gemm` | cuBLAS `gemm` | 全部 SM（部分老卡性能受限） |
| FP32 | `fp32` | `gemm` | cuBLAS `gemm` | 全部 SM |
| TF32 | `tf32` | `gemm` | cuBLAS `gemm`（Tensor Core） | SM ≥ 8.0（A100 / 30 系+） |
| FP16 | `fp16` | `gemm` | cuBLAS `gemm` | 全部 SM |
| BF16 | `bf16` | `gemm` | cuBLAS `gemm`（Tensor Core） | SM ≥ 8.0（A100 / 30 系+） |
| FP8 | `fp8` | ⚠️ 未实现 | — | — |
| **INT8** | `int8` / `i8` | `gemm` **_或_** `intalu` | cuBLAS `gemm_ex`（IMMA TC）**或** 自定义 DP4A/MAD 内核 | SM ≥ 5.0；IMMA TC 需 SM ≥ 7.5（Turing+） |
| **INT16** | `int16` / `i16` | `intalu` **_仅_** ¹ | 自定义 NVRTC（标量 int16 MAD链） | 全部 SM |
| **INT32** | `int32` / `i32` | `intalu` **_仅_** ¹ | 自定义 NVRTC（标量 int32 MAD链） | 全部 SM |

> ¹ **INT16/INT32 无 cuBLAS GEMM 路径**（`CUDA_R_16I` / `CUDA_R_32I` 仅为存储类型，无 `computeType`）。若需压力测试 INT16/INT32，必须同时指定 `--kernel-types intalu`。工具会在启动时主动报错并退出，避免空转。

> **INT4 / INT64 / INT6**：`CUDA_R_4I` 和 `CUDA_R_64I` 在 cuBLAS 中为纯存储类型，**无 GEMM 计算路径**（cuBLAS 12.9 仅支持 `CUDA_R_8I` → `CUDA_R_32I` 的整数 GEMM）。INT4 tensor core 硬件在 SM 8.0+ 存在（PTX `mma.s32.s4.s4.s32`），但 cuBLAS 不暴露，需自行 inline PTX 实现；INT6 在 CUDA 中不存在；INT64 标量乘加极慢，无法有效占满 GPU 核心。综合判断：**暂无添加规划**。

### 精度 ↔ Kernel 兼容矩阵

启动时自动检查并拒绝无路径组合：

| 精度 | GEMM | IntAlu | Mem* / Atomic |
|---|---|---|---|
| FP64 / FP32 / TF32 / FP16 / BF16 | ✅ | — | ✅ |
| INT8 | ✅（推荐） | ✅（纯 ALU 备用） | ✅ |
| INT16 / INT32 | ❌ ² | ✅ | ✅ |
| FP8 | ❌ | — | — |

> ² INT16/INT32 + `gemm` 会在启动时被 `kernel_precision_compatible` 检测到并直接退出，附带 "add `intalu` to --kernel-types" 提示。

### 环境要求

- NVIDIA GPU，且已安装 CUDA 驱动/工具包
- Rust 1.70+
- Windows 运行时需确保以下 DLL（*取决于 CUDA 版本）可被加载（位于系统 PATH 或程序同目录）：
  - `nvrtc64_*.dll`
  - `cublasLt64_*.dll`
  - `cublas64_*.dll`
  - `cudart64_*.dll`

### 构建与运行

CUDA 支持通过 feature flag 控制。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32
```

```bash
# 混合 FP + 整数压力（推荐）
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions fp16,bf16,int8,int32 \
  --kernel-types gemm,intalu,memcpy

# INT16 压力（仅 intalu）
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions int16 \
  --kernel-types intalu

# INT8 cuBLAS GEMM + INT32 intalu 混合
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions int8,int32 \
  --kernel-types gemm,intalu
```

若使用 `auto-optimizer/test/cli-stressor-cuda-rs-minload.toml` 或启用 Vulkan 图形压力，请同时启用 `vulkan` feature：

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda,vulkan
```

可选：通过配置文件运行（所有选项都可放入 config）。

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --config ./stressor.toml
```

### 配置文件

- 参数优先级：`命令行显式传入 > config 文件 > 内置默认值`
- autoscan 示例配置位于 `auto-optimizer/test/`：
  - `cli-stressor-cuda-rs.toml`：默认配置，面向 8G+ 显存显卡。
  - `cli-stressor-cuda-rs-6g-8g.toml`：较低显存配置，面向 6G-8G 显存显卡。
- `kernel_mixture` 支持两种写法：
  - 字符串：`"gemm:0.4,intalu:0.2,memcpy:0.2,reduction:0.2"`
  - 映射：`{ gemm = 0.4, intalu = 0.2, memcpy = 0.2, reduction = 0.2 }`
- `kernel_params.<kernel>` 支持按 kernel 覆盖参数，包括 `precisions`
- `validate_interval = 0` 可关闭周期性验证
- `vulkan_minor_mixture_rate` 用于 Vulkan 图形压力：启用 Vulkan 时会按该比例混入小尺寸 3D 图像（宽高随机取 127/256/511/512/1023，depth 保持不变）

示例（`stressor.toml`）：

```toml
duration = 120
matrix_sizes = [2049, 4096, 8192]
fp64_matrix_sizes = [2048, 4096]
precisions = ["fp16", "bf16", "tf32", "int8", "int32"]
warmup_iters = 3
burst_iters = 6
validate_interval = 10.0
validate_size = 1024
transpose_prob = 0.5
minor_mixture_rate = 0.15
seed = 12345
kernel_types = ["gemm", "intalu", "memcpy", "reduction", "atomic"]
kernel_mixture = { gemm = 0.35, intalu = 0.25, memcpy = 0.2, reduction = 0.1, atomic = 0.1 }
stream_mode = "dual"
disable_fp8 = true

[kernel_params.gemm]
precisions = ["fp16", "bf16", "int8"]
matrix_sizes = [4096, 8192]
warmup_iters = 4
burst_iters = 8

[kernel_params.intalu]
precisions = ["int32", "int16"]
burst_iters = 16

[kernel_params.reduction]
precisions = ["fp32"]
burst_iters = 64
```

### 代际兼容性参考

#### INTEGER 路径

| 路径 | 机制 | 最低 SM | Tensor Core | 说明 |
|---|---|---|---|---|
| INT8 GEMM | cuBLAS `gemm_ex` `CUDA_R_8I`→`CUDA_R_32I`、`CUBLAS_COMPUTE_32I`、`DEFAULT_TENSOR_OP` | SM ≥ 5.0（标量）；**SM ≥ 7.5 IMMA TC** | cuBLAS 自动分发：Turing+/Ampere/Ada/Hopper/Blackwell 走 IMMA；更老卡走 DP4A 或标量 | 推荐 INT8 路径 |
| INT8 DP4A `__dp4a`（IntAlu） | NVRTC intrinsic | SM ≥ 6.1（Pascal GP100/GP10x） | INT8 点积 ALU（非 mma） | `__CUDA_ARCH__>=610` 守卫 |
| INT16 ALU（IntAlu） | 标量 int16 MAD | 全部 SM | 无（无 INT16 tensor core） | 通用，实测 RTX 3060 ~1.6 TIOPs |
| INT32 ALU（IntAlu） | 标量 int32 MAD | 全部 SM | 无 | 通用 |
| INT4 Tensor Core | PTX `mma.m16n8k64.s32.s4.s4.s32` | SM ≥ 8.0 | SM ≥ 8.0 **硬件存在**但 cuBLAS 12.9 不暴露，需自行 inline PTX | ⚠️ 未实现 |

> **单次构建跨代覆盖**：NVRTC `compile_ptx_with_opts(arch="compute_XX")` 生成虚拟架构 PTX，驱动在加载模块时 JIT 编译到真实 SASS。`compute_90` 上限 + `__CUDA_ARCH__` 守卫使单个二进制覆盖 Pascal → Blackwell。

#### 其他路径

| 路径 | 说明 |
|---|---|
| FP64 / FP32 / FP16 GEMM | 全部 SM 可用 |
| TF32 / BF16 GEMM | SM ≥ 8.0（Ampere / 30 系+），旧架构自动跳过 |
| FP8 GEMM | 尚未实现 |
| Atomic kernel | SM ≥ 7.5（Turing+）；更老卡自动禁用 |
| Mem* / Reduction | 全部 SM 可用 |

### 注意事项

- TF32 和 BF16 均使用 cuBLAS 的数学模式切换；BF16 需要 SM80+（Ampere 及以后），旧架构会自动跳过并给出明确提示。FP8 尚未实现。
- **INT8 GEMM** 使用 cuBLAS `cublasGemmEx` + `CUDA_R_8I` → `CUBLAS_COMPUTE_32I`。cuBLAS 自动分发到 IMMA tensor core（Turing+/SM ≥ 7.5）或 DP4A/标量回退。SM < 7.5 的 GPU 会收到警告提示（路径仍可用，仅吞吐较低）。INT8 tensor-op 路径要求矩阵维度为 **16 的倍数**；工具自动将奇数/未对齐尺寸向上对齐至 `(size + 15) & !15`。
- **INT16 / INT32** 无 cuBLAS GEMM 路径（`CUDA_R_16I` / `CUDA_R_32I` 为纯存储类型）。若指定了这些精度但未在 `--kernel-types` 中加入 `intalu`，工具会**主动报错退出**（不空转、不假死）。
- 校验路径使用 CPU FP64 GEMM，并按元素比较容差。**INT 精度跳过校验**（`IntAlu` 为不可比对 hash 链，`INT8 GEMM` 尚无 CPU 参考实现），日志中会标注 "validation skipped for INT path"。
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

`cli-stressor-cuda-rs` is a Rust-based CUDA compute stress tool. It uses randomized GEMM / integer ALU / memory / atomic workloads and periodic CPU-side validation to help detect silent data corruption and hardware stability issues.

### Features

- Mixed kernel-path stress:
  - **GEMM**: cuBLAS FP64 / FP32 / TF32 / FP16 / BF16 matrix multiply, plus INT8 cuBLAS GEMM (IMMA tensor core accelerated)
  - **Integer ALU** (`intalu`): custom NVRTC kernel driving INT32 *MAD / INT16 narrow / INT8 DP4A dot* chains (Pascal+ `__dp4a`; scalar MAD fallback for older GPUs)
  - **Memory**: memcpy / memset / transpose / elementwise / reduction
  - **Atomics**: custom NVRTC atomic kernel
  - Tunable kernel weights (`--kernel_mixture`), with per-kernel parameter overrides
- Randomized matrix sizes with warmup and burst phases
- Precision modes: **FP64, FP32, TF32, FP16, BF16, INT8, INT16, INT32**; FP8 is not yet implemented
- Periodic validation using a CPU FP64 reference result (INT precisions skip validation; see [Notes](#notes-english))
- Multi-stream submission: `single` / `dual` / `triple`
- Optional Vulkan graphics stress sidecar (`--enable-vulkan-stress`; requires `--features vulkan`)
- PCI bus / UUID / sorted-index GPU selection; `--list-gpus` to enumerate devices

### Precision Matrix

| Precision | CLI Aliases | Kernel Type(s) | Backend Engine | Generation Requirement |
|---|---|---|---|---|
| FP64 | `fp64` | `gemm` | cuBLAS `gemm` | All SM (performance-limited on some older cards) |
| FP32 | `fp32` | `gemm` | cuBLAS `gemm` | All SM |
| TF32 | `tf32` | `gemm` | cuBLAS `gemm` (Tensor Core) | SM ≥ 8.0 (A100 / 30-series+) |
| FP16 | `fp16` | `gemm` | cuBLAS `gemm` | All SM |
| BF16 | `bf16` | `gemm` | cuBLAS `gemm` (Tensor Core) | SM ≥ 8.0 (A100 / 30-series+) |
| FP8 | `fp8` | ⚠️ unimplemented | — | — |
| **INT8** | `int8` / `i8` | `gemm` **_or_** `intalu` | cuBLAS `gemm_ex` (IMMA TC) **or** custom DP4A/MAD kernel | SM ≥ 5.0; IMMA TC requires SM ≥ 7.5 (Turing+) |
| **INT16** | `int16` / `i16` | `intalu` **_only_** ¹ | Custom NVRTC (scalar int16 MAD chain) | All SM |
| **INT32** | `int32` / `i32` | `intalu` **_only_** ¹ | Custom NVRTC (scalar int32 MAD chain) | All SM |

> ¹ **INT16/INT32 have no cuBLAS GEMM path** (`CUDA_R_16I` / `CUDA_R_32I` are storage types only, with no corresponding `computeType`). To stress INT16/INT32 you must also specify `--kernel-types intalu`. The tool detects the missing combination at startup and exits with a clear error (no silent spin).

> **INT4 / INT64 / INT6**: `CUDA_R_4I` and `CUDA_R_64I` exist in cuBLAS as storage-only types with **no GEMM compute path** (cuBLAS 12.9 only supports `CUDA_R_8I` → `CUDA_R_32I` for integer GEMM). INT4 tensor core hardware _does_ exist on SM 8.0+ (PTX `mma.s32.s4.s4.s32`) but cuBLAS does not expose it — inline PTX would be required. INT6 does not exist in CUDA. INT64 scalar MAD is too slow to saturate GPU cores. Verdict: **not planned**.

### Precision ↔ Kernel Compatibility Matrix

Enforced at startup; incompatible combos exit with an error:

| Precision | GEMM | IntAlu | Mem* / Atomic |
|---|---|---|---|
| FP64 / FP32 / TF32 / FP16 / BF16 | ✅ | — | ✅ |
| INT8 | ✅ (recommended) | ✅ (pure-ALU fallback) | ✅ |
| INT16 / INT32 | ❌ ² | ✅ | ✅ |
| FP8 | ❌ | — | — |

> ² INT16/INT32 + `gemm` is caught by `kernel_precision_compatible` at startup; the tool exits immediately with "add `intalu` to --kernel-types".

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

```bash
# Mixed FP + integer stress (recommended)
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions fp16,bf16,int8,int32 \
  --kernel-types gemm,intalu,memcpy

# INT16 stress (intalu only)
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions int16 \
  --kernel-types intalu

# INT8 cuBLAS GEMM + INT32 intalu mixed
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 \
  --precisions int8,int32 \
  --kernel-types gemm,intalu
```

When using `auto-optimizer/test/cli-stressor-cuda-rs-minload.toml` or Vulkan graphics stress, build with both features:

```bash
cargo build --release -p cli-stressor-cuda-rs --features cuda,vulkan
```

Optional: run with a config file (all options can be provided in config).

```bash
cargo run -p cli-stressor-cuda-rs --features cuda -- --config ./stressor.toml
```

### Config File

- Precedence: `explicit CLI value > config file > built-in default`
- Autoscan example configs are under `auto-optimizer/test/`:
  - `cli-stressor-cuda-rs.toml`: default profile for cards with 8G+ VRAM.
  - `cli-stressor-cuda-rs-6g-8g.toml`: lower-VRAM profile for cards with 6G-8G VRAM.
- `kernel_mixture` supports two formats:
  - string: `"gemm:0.4,intalu:0.2,memcpy:0.2,reduction:0.2"`
  - map: `{ gemm = 0.4, intalu = 0.2, memcpy = 0.2, reduction = 0.2 }`
- `kernel_params.<kernel>` supports per-kernel overrides, including `precisions`
- `validate_interval = 0` disables periodic validation
- `vulkan_minor_mixture_rate` controls Vulkan graphics stress: when Vulkan is enabled, small 3D images are mixed in at that rate (width/height randomly chosen from 127/256/511/512/1023; depth stays the same)

Example (`stressor.toml`):

```toml
duration = 120
matrix_sizes = [2049, 4096, 8192]
fp64_matrix_sizes = [2048, 4096]
precisions = ["fp16", "bf16", "tf32", "int8", "int32"]
warmup_iters = 3
burst_iters = 6
validate_interval = 10.0
validate_size = 1024
transpose_prob = 0.5
minor_mixture_rate = 0.15
seed = 12345
kernel_types = ["gemm", "intalu", "memcpy", "reduction", "atomic"]
kernel_mixture = { gemm = 0.35, intalu = 0.25, memcpy = 0.2, reduction = 0.1, atomic = 0.1 }
stream_mode = "dual"
disable_fp8 = true

[kernel_params.gemm]
precisions = ["fp16", "bf16", "int8"]
matrix_sizes = [4096, 8192]
warmup_iters = 4
burst_iters = 8

[kernel_params.intalu]
precisions = ["int32", "int16"]
burst_iters = 16

[kernel_params.reduction]
precisions = ["fp32"]
burst_iters = 64
```

### Generation Compatibility Reference

#### INTEGER Paths

| Path | Mechanism | Min SM | Tensor Core | Notes |
|---|---|---|---|---|
| INT8 GEMM | cuBLAS `gemm_ex` `CUDA_R_8I`→`CUDA_R_32I`, `CUBLAS_COMPUTE_32I`, `DEFAULT_TENSOR_OP` | SM ≥ 5.0 (scalar); **SM ≥ 7.5 IMMA TC** | cuBLAS auto-dispatches: IMMA on Turing+/Ampere/Ada/Hopper/Blackwell; DP4A or scalar on older GPUs | Recommended INT8 path |
| INT8 DP4A `__dp4a` (IntAlu) | NVRTC intrinsic | SM ≥ 6.1 (Pascal GP100/GP10x) | INT8 dot-product ALU (not mma) | Guarded by `__CUDA_ARCH__>=610` |
| INT16 ALU (IntAlu) | Scalar int16 MAD | All SM | None (no INT16 tensor core exists) | Universal; measured ~1.6 TIOPs on RTX 3060 |
| INT32 ALU (IntAlu) | Scalar int32 MAD | All SM | None | Universal |
| INT4 Tensor Core | PTX `mma.m16n8k64.s32.s4.s4.s32` | SM ≥ 8.0 | SM ≥ 8.0 **HW exists** but cuBLAS 12.9 does not expose it; inline PTX required | ⚠️ Not implemented |

> **Single-binary cross-generation coverage**: NVRTC `compile_ptx_with_opts(arch="compute_XX")` emits virtual-arch PTX; the driver JIT-compiles it to real SASS at module load time. The `compute_90` cap + `__CUDA_ARCH__` guards let a single binary cover Pascal → Blackwell.

#### Other Paths

| Path | Notes |
|---|---|
| FP64 / FP32 / FP16 GEMM | All SM |
| TF32 / BF16 GEMM | SM ≥ 8.0 (Ampere / 30-series+); older GPUs auto-skip with a message |
| FP8 GEMM | Not yet implemented |
| Atomic kernel | SM ≥ 7.5 (Turing+); auto-disabled on older GPUs |
| Mem* / Reduction | All SM |

### Notes

- TF32 and BF16 both use cuBLAS math-mode switching. BF16 requires SM80+ (Ampere and later); older architectures skip it with a clear message. FP8 is not yet implemented.
- **INT8 GEMM** uses cuBLAS `cublasGemmEx` + `CUDA_R_8I` → `CUBLAS_COMPUTE_32I`. cuBLAS auto-dispatches to IMMA tensor cores (Turing+/SM ≥ 7.5) or DP4A/scalar fallback. GPUs with SM < 7.5 receive a warning (the path still works, just at lower throughput). The INT8 tensor-op path requires matrix dimensions to be **multiples of 16**; the tool auto-aligns odd/unaligned sizes to `(size + 15) & !15`.
- **INT16 / INT32** have no cuBLAS GEMM path (`CUDA_R_16I` / `CUDA_R_32I` are storage-only types). If these precisions are requested without `intalu` in `--kernel-types`, the tool **exits with an error** at startup (no silent spin / apparent hang).
- The validation path uses CPU FP64 GEMM and compares values with element-wise tolerances. **INT precisions skip validation** (`IntAlu` is a non-referenceable hash chain; `INT8 GEMM` has no CPU reference yet); the log annotates this as "validation skipped for INT path".
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
