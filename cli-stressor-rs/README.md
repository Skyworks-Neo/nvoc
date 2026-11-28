# cli-stressor-rs

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

`cli-stressor-rs` 是一个用 Rust 编写的 CUDA 计算压力测试工具。它通过随机化 GEMM / 整数 ALU / 访存 / 原子操作负载和周期性的 CPU 侧校验，帮助发现静默数据损坏和硬件稳定性问题。

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
- Vulkan 图形压力侧车：渲染负载 `--vulkan`（默认无头 offscreen，`--vulkan-window` 出窗，仅 Windows）；需 `--features vulkan`
- `--vulkan-only` 是纯 Vulkan 路径：在 CUDA 初始化之前处理，GPU 直接按 `--gpu-uuid` / `--pci-bus` / `--gpu-index`（Vulkan 侧 PCI 排序索引，选中前打印完整映射表）选择，无需可用 CUDA 驱动。注意 Vulkan 枚举包含非 NVIDIA 设备，精确选择优先用 UUID 或 PCI 地址
- PCI 总线 / UUID / 排序索引 GPU 选择；`--get-cuda-uuid-list` 枚举 CUDA 设备（含 UUID），`--get-vulkan-index` 打印选中 GPU 的 Vulkan 排序 index

### 支持精度一览

| 精度 | CLI 别名 | Kernel 类型 | 底层引擎 | 代际要求 |
|---|---|---|---|---|
| FP64 | `fp64` | `gemm` | cuBLAS `gemm` | 全部 SM（部分老卡性能受限） |
| FP32 | `fp32` | `gemm` | cublasLt `CUDA_R_32F`（默认）**或**经典 cuBLAS | 全部 SM |
| TF32 | `tf32` | `gemm` | cublasLt `COMPUTE_32F_FAST_TF32`（默认）**或**经典 math mode | SM ≥ 8.0（A100 / 30 系+） |
| FP16 | `fp16` | `gemm` | cublasLt `CUDA_R_16F`（默认）**或**经典 cuBLAS（`--gemm-legacy-cublas`） | 全部 SM |
| BF16 | `bf16` | `gemm` | cublasLt `CUDA_R_16BF`（默认）**或**经典 cuBLAS | SM ≥ 8.0（A100 / 30 系+） |
| **FP8** | `fp8` / `fp8e4m3` | `gemm` | cublasLt `CUDA_R_8F_E4M3FN`（E4M3FN，`COMPUTE_32F`，TN；ride-on-load 采样校验） | SM ≥ 8.9（40 系+）；驱动 cublasLt 需 CUDA 11.8+（R520+） |
| **FP8 E5M2** | `fp8e5m2` / `e5m2` | `gemm` | cublasLt `CUDA_R_8F_E5M2`（同 E4M3 约束；饱和语义对齐硬件 `cvt.rn.satfinite`） | SM ≥ 8.9；**kernel 可用性由 cublasLt 构建版本决定**——实测 cublasLt 12.8.4/12.0.2 在 Ada 上零候选（RTX 4060），E4M3 才是 Ada 原生 FP8 格式；运行时仲裁自动 SKIP。可试 `--cuda-dll-version 12.9` 换更新的 cublasLt 构建 |
| **FP4** | `fp4` | `gemm` | cublasLt `CUDA_R_4F_E2M1` + 块缩放（NVFP4 优先，回退 MXFP4；TN；采样校验） | SM 100/103/120（50 系+）；驱动 cublasLt 需 CUDA 12.8+（R570+） |
| **INT8** | `int8` / `i8` | `gemm` **_或_** `intalu` | cublasLt `CUDA_R_8I`→`CUDA_R_32I`（IMMA TC，TN 默认）或 `intalu` 自定义 DP4A/MAD 内核；经典 `gemm_ex` 经 `--gemm-legacy-cublas` 保留 | SM ≥ 5.0；IMMA TC 需 SM ≥ 7.5（Turing+） |
| **INT4** | `int4` / `i4` | `gemm` | cublasLt `CUDA_R_4I`→`CUDA_R_32I`（IMMA TC，TN；采样 + 精确 sidecar 双重校验） | SM ≥ 7.5；**运行时可用性取决于驱动 cublasLt 是否返回 `CUDA_R_4I` 候选**（无候选时干净报错） |
| **INT16** | `int16` / `i16` | `intalu` **_仅_** ¹ | 自定义 NVRTC（标量 int16 MAD链） | 全部 SM |
| **INT32** | `int32` / `i32` | `intalu` **_仅_** ¹ | 自定义 NVRTC（标量 int32 MAD链） | 全部 SM |

> ¹ **INT16/INT32 无 cuBLAS GEMM 路径**（`CUDA_R_16I` / `CUDA_R_32I` 仅为存储类型，无 `computeType`）。若需压力测试 INT16/INT32，必须同时指定 `--kernel-types intalu`。工具会在启动时主动报错并退出，避免空转。

> **INT4 可用性注记**：PTX `mma.s4` 硬件在 SM 8.0+ 存在，cuBLASLt 文档的类型组合表含 `CUDA_R_4I` + `CUBLAS_COMPUTE_32I`（SM80+ IMMA），但不同驱动版本的启发式可能不返回候选。工具在运行时实测：无候选即报错退出（不静默空转）——这正是"读回是唯一判据"的口径；有候选则采样校验 + 周期精确 sidecar 双重把关。INT6 在 CUDA 中不存在；INT64 标量乘加极慢，无法有效占满 GPU 核心。

### 精度 ↔ Kernel 兼容矩阵

启动时自动检查并拒绝无路径组合：

| 精度 | GEMM | IntAlu | Mem* / Atomic |
|---|---|---|---|
| FP64 / FP32 / TF32 / FP16 / BF16 | ✅ | — | ✅ |
| FP8 / FP4 / INT4 | ✅（cublasLt 专用） | — | ❌ |
| INT8 | ✅（推荐） | ✅（纯 ALU 备用） | ✅ |
| INT16 / INT32 | ❌ ² | ✅ | ✅ |

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

> 构建代际选择器：`cuda11` / `cuda12` / `cuda13` 三个 feature 互斥，必须选一个（例如 `cargo build -p cli-stressor-rs --features cuda13 --release`）；默认构建（不带 CUDA 代际）是纯 stub，仅供解析/单测。

CUDA 支持通过 feature flag 控制。

```bash
cargo run -p cli-stressor-rs --features cuda12 -- --duration 30 --precisions fp16,tf32
```

```bash
# 混合 FP + 整数压力（推荐）
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions fp16,bf16,int8,int32 \
  --kernel-types gemm,intalu,memcpy

# INT16 压力（仅 intalu）
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions int16 \
  --kernel-types intalu

# INT8 cuBLAS GEMM + INT32 intalu 混合
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions int8,int32 \
  --kernel-types gemm,intalu
```

若启用 Vulkan 图形压力，请同时启用 `vulkan` feature：

```bash
cargo build --release -p cli-stressor-rs --features cuda12,vulkan
```

可选：通过配置文件运行（所有选项都可放入 config）。

```bash
cargo run -p cli-stressor-rs --features cuda12 -- --config ./stressor.toml
```

### 配置文件

- 参数优先级：`命令行显式传入 > config 文件 > 内置默认值`
- 内置 autoscan profile 位于 `cli-stressor-rs/profiles/`：
  - `40-50.toml`：面向 NVIDIA GeForce RTX 40/50 系显卡，由 auto-optimizer 自动选择。
  - `standard.toml`：默认配置，面向 8G+ 显存显卡。
  - `low-vram.toml`：较低显存配置，面向 6G-8G 显存显卡。
- `kernel_mixture` 支持两种写法：
  - 字符串：`"gemm:0.4,intalu:0.2,memcpy:0.2,reduction:0.2"`
  - 映射：`{ gemm = 0.4, intalu = 0.2, memcpy = 0.2, reduction = 0.2 }`
- `kernel_params.<kernel>` 支持按 kernel 覆盖参数，包括 `precisions`
- `validate_interval = 0` 可关闭周期性验证

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

### cublasLt 车道与 kernel 自选

全部 FP 精度 + INT8/INT4 的 GEMM 压力路径默认走 **cublasLt**（与经典 `cublasGemmEx` 语义对齐：FP16/BF16 同为 `CUBLAS_COMPUTE_32F` + f32 scale，TF32 走 desc 级 `COMPUTE_32F_FAST_TF32`，FP64 走 `COMPUTE_64F`——读数口径可比）。每个新的（精度, 尺寸, 转置）组合首次遇到时打印一行选中 kernel 的身份：

```
[GEMM-LT] FP16 4096x4096 ta=T tb=N | algo #0/8: id=8 tile=34 stages=2 splitk=1 red=0 swz=1 ws=4.0MiB
```

- cublasLt API 不暴露 SASS kernel 名；tile/stages/split-K/reduction/swizzle 配置属性即用户可见的 kernel 身份（SASS 符号需 NCU）。
- 解锁卡（如解锁 RTX 4090）的 FP16 端到端读数双峰（49/63 TF）根因是库内静态启发式按出厂半速模型选了次优 kernel（参见 [Harry-Chen/unlocked-ada-gemm](https://github.com/Harry-Chen/unlocked-ada-gemm) 与[原始博客](https://harrychen.xyz/2026/11/17/unlocked-rtx-4090-gemm-perf/)）。`--gemm-algo-list` 内建"枚举候选 + 逐个计时"探针，`--gemm-algo-index N` 可手动钉住候选 N；报告行同时写入 verdict JSON 的 `precisions[].gemm_algos`。

| Flag | 说明 |
|---|---|
| `--gemm-heuristics <N>` | 每形状请求的启发式候选数 [8] |
| `--gemm-algo-index <N>` | 全局钉住候选 N（0 = 库默认；某形状候选数不足时报错并打印实际数量） |
| `--gemm-algo-list` | 首遇形状对全部候选计时打 TFLOPS 表（lane 0，各 3 iter） |
| `--gemm-legacy-cublas` | 全部 FP 精度 + INT8 回退经典 cublas 车道（矩阵收录口径 A/B 对照） |
| `--fp8-fast-accum` | FP8 启用 `CUBLASLT_MATMUL_DESC_FAST_ACCUM`（更快但大 K 相对误差累积 10–15%；默认分段累加精度安全） |
| `--gpu-generate` | FP8/FP4 操作数改由 NVRTC fill kernel 直接在显存生成（均匀 [-2,2] → kernel 内 RN-even 量化，零 H2D、零 host gap）；默认主机端生成 + rayon 并行量化（O(1) 位运算编解码） |

### 代际兼容性参考

#### INTEGER 路径

| 路径 | 机制 | 最低 SM | Tensor Core | 说明 |
|---|---|---|---|---|
| INT8 GEMM | cublasLt `CUDA_R_8I`→`CUDA_R_32I`、`CUBLAS_COMPUTE_32I`（TN 强制，IMMA 保证命中）；经典 `gemm_ex`+`DEFAULT_TENSOR_OP` 经 `--gemm-legacy-cublas` 保留 | SM ≥ 5.0（标量）；**SM ≥ 7.5 IMMA TC** | Lt TN 强制命中 IMMA；经典车道随机转置下仅 ~25% op 是 TN（其余落 DP4A/标量） | 推荐 INT8 路径 |
| INT8 DP4A `__dp4a`（IntAlu） | NVRTC intrinsic | SM ≥ 6.1（Pascal GP100/GP10x） | INT8 点积 ALU（非 mma） | `__CUDA_ARCH__>=610` 守卫 |
| INT16 ALU（IntAlu） | 标量 int16 MAD | 全部 SM | 无（无 INT16 tensor core） | 通用，实测 RTX 3060 ~1.6 TIOPs |
| INT32 ALU（IntAlu） | 标量 int32 MAD | 全部 SM | 无 | 通用 |
| INT4 GEMM | cublasLt `CUDA_R_4I`→`CUDA_R_32I`（TN，2 nibble/byte） | SM ≥ 7.5（文档组合表 SM80+ IMMA） | SM ≥ 8.0 `mma.s4` 硬件；**候选存在性由驱动实测**（无候选即报错） | 采样校验精确（乘积 ≤ 64·K < 2²⁴）+ 周期精确 sidecar |

> **单次构建跨代覆盖**：NVRTC `compile_ptx_with_opts(arch="compute_XX")` 生成虚拟架构 PTX，驱动在加载模块时 JIT 编译到真实 SASS。`compute_90` 上限 + `__CUDA_ARCH__` 守卫使单个二进制覆盖 Pascal → Blackwell。

#### 其他路径

| 路径 | 说明 |
|---|---|
| FP64 / FP32 / FP16 GEMM | 全部 SM 可用 |
| TF32 / BF16 GEMM | SM ≥ 8.0（Ampere / 30 系+），旧架构自动跳过 |
| FP8 GEMM | cublasLt E4M3FN，SM ≥ 8.9（40 系+）；FAST_ACCUM 默认关 |
| FP4 GEMM | cublasLt E2M1 + 块缩放，SM 100/103/120（50 系+）；40 系自动 SKIP |
| Atomic kernel | SM ≥ 7.5（Turing+）；更老卡自动禁用 |
| Mem* / Reduction | 全部 SM 可用 |

### 注意事项

- **多卡 fan-out**：`--gpu-index 0,1,2`（逗号分隔）会为每张卡派发一个 worker 子进程跑相同参数，所有输出按行加 `[GPU<i>]` 前缀，整体退出码取最差子进程；单卡进程隔离（一张卡的驱动故障不殃及他卡），与 auto-optimizer 的 per-GPU worker 模式一致。单索引行为完全不变；`--pci-bus` / `--gpu-uuid` 仍为单选（多卡请用索引列表）。`--vulkan-only` / `--video-only` 仅取首个索引。
- **CUDA 运行库版本选择**（`--cuda-dll-version 12.8`）：从 NVIDIA 官方 PyPI wheel（`nvidia-cublas-cu12/13` 等三个包）解析、下载、sha256 校验后只提取需要的 DLL 到 `%LOCALAPPDATA%
voc\cuda-dlls\<版本>\` 缓存，并用 `LoadLibraryExW` 预加载——预加载模块在进程内优先于任何搜索序命中，彻底规避"exe 同目录/PATH 里的旧 cublasLt 遮蔽驱动新版"这类问题（实测：cublasLt 12.0.2 遮蔽 CUDA 13.5 驱动）。版本主号必须与构建的 cuda11/12/13 feature 一致。中国大陆网络用 `--cuda-dll-mirror tuna|aliyun` 走 TUNA/Aliyun 镜像（默认 pypi.org，文件下载失败自动回退 TUNA/Aliyun）。Linux 同目录下载 `.so` 并提示 `LD_LIBRARY_PATH` 设置。`--cuda-dll-dir <DIR>` 改为手动指定目录。头部 `[CUDA] versions` + `[CUDA] dlls` 行始终记录实际加载的版本与文件路径——矩阵收录的两个必读行。
- TF32 和 BF16 均使用 cuBLAS 的数学模式切换；BF16 需要 SM80+（Ampere 及以后），旧架构会自动跳过并给出明确提示。**全部 FP 精度 + INT8/INT4 GEMM 默认走 cublasLt 车道**（见上文"cublasLt 车道与 kernel 自选"）；TF32 的 Lt 形式是 desc 级 `COMPUTE_32F_FAST_TF32`（语义同经典 math mode）。
- **INT8 GEMM** 默认走 cublasLt TN（`CUDA_R_8I`→`CUDA_R_32I`，`CUBLAS_COMPUTE_32I`）：IMMA 张量核路径只支持 Aᵀ·B 布局，Lt 强制 TN 后必然命中；经典 `cublasGemmEx` 车道的随机转置（`transpose_prob=0.5`）下仅 ~25% 的 op 是 TN，其余落在 DP4A/标量路径。SM < 7.5 的 GPU 收到警告提示（路径仍可用，仅吞吐较低）。INT8 张量核路径要求矩阵维度为 **16 的倍数**；工具自动将奇数/未对齐尺寸向上对齐至 `(size + 15) & !15`。INT8 无 ride-on-load 采样校验（i32 点积超 2²⁴，f32 比较不精确），由周期精确整数 sidecar（`cpu_reference_int_tn`）覆盖。
- **FP8 GEMM** 走 cublasLt E4M3FN + `COMPUTE_32F` + f16 输出，TN 布局为 cublasLt 硬性要求（`transpose_prob` 不适用，报告行显示 ta=T tb=N）；尺寸自动对齐到 16 的倍数。`--fp8-fast-accum` 启用 FAST_ACCUM 快路径（约 90% 峰值但大 K 相对误差累积 10–15%）；默认分段累加路径精度安全。操作数由 f32 随机矩阵经 O(1) 位运算 RN-even 量化（穷举幂等测试覆盖 256 个位型），或经 `--gpu-generate` 由 NVRTC kernel 在显存内直接生成（device 量化器与 host 逐位对齐），采样校验从同一 A/B 精确重算。
- **FP4 GEMM** 走 cublasLt E2M1 + 块缩放（NVFP4：16 元素块 + E4M3 scale，优先；驱动无候选时回退 MXFP4：32 元素块 + UE8M0 2 的幂 scale），TN 布局，尺寸对齐到 32 的倍数；仅 Blackwell（SM100/103/120）+ CUDA 12.8+ 驱动可用，否则报错。
- **INT4 GEMM** 走 cublasLt `CUDA_R_4I`（2 nibble/byte，low nibble 先），TN 布局，尺寸对齐到 32 的倍数；周期精确 sidecar（`cpu_reference_int4_tn`）+ 采样校验（乘积 ≤ 64·K < 2²⁴，f32 精确，容差 0）。
- **INT16 / INT32** 无 cuBLAS GEMM 路径（`CUDA_R_16I` / `CUDA_R_32I` 为纯存储类型）。若指定了这些精度但未在 `--kernel-types` 中加入 `intalu`，工具会**主动报错退出**（不空转、不假死）。
- 校验路径使用 CPU FP64 GEMM，并按元素比较容差。INT 精度跳过 FP 校验（`IntAlu` 为不可比对 hash 链；INT8/INT4 GEMM 由精确整数 sidecar 覆盖；FP8/FP4 由 ride-on-load 采样校验覆盖）。
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

`cli-stressor-rs` is a Rust-based CUDA compute stress tool. It uses randomized GEMM / integer ALU / memory / atomic workloads and periodic CPU-side validation to help detect silent data corruption and hardware stability issues.

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
- Optional Vulkan graphics stress sidecar: render load `--vulkan` (headless offscreen by default, `--vulkan-window` to present a window, Windows only); requires `--features vulkan`
- `--vulkan-only` is a pure-Vulkan path handled before any CUDA init: the GPU is selected directly via `--gpu-uuid` / `--pci-bus` / `--gpu-index` (Vulkan-side PCI-sorted index; the full mapping is printed before selection), so no working CUDA driver is required. Note the Vulkan enumeration includes non-NVIDIA devices; prefer UUID or PCI address for exact selection
- PCI bus / UUID / sorted-index GPU selection; `--get-cuda-uuid-list` to enumerate CUDA devices, `--get-vulkan-index` to print the Vulkan-sorted index

### Precision Matrix

| Precision | CLI Aliases | Kernel Type(s) | Backend Engine | Generation Requirement |
|---|---|---|---|---|
| FP64 | `fp64` | `gemm` | cuBLAS `gemm` | All SM (performance-limited on some older cards) |
| FP32 | `fp32` | `gemm` | cublasLt `CUDA_R_32F` (default) **or** classic cuBLAS | All SM |
| TF32 | `tf32` | `gemm` | cublasLt `COMPUTE_32F_FAST_TF32` (default) **or** classic math mode | SM ≥ 8.0 (A100 / 30-series+) |
| FP16 | `fp16` | `gemm` | cublasLt `CUDA_R_16F` (default) **or** classic cuBLAS (`--gemm-legacy-cublas`) | All SM |
| BF16 | `bf16` | `gemm` | cublasLt `CUDA_R_16BF` (default) **or** classic cuBLAS | SM ≥ 8.0 (A100 / 30-series+) |
| **FP8** | `fp8` / `fp8e4m3` | `gemm` | cublasLt `CUDA_R_8F_E4M3FN` (E4M3FN, `COMPUTE_32F`, TN; ride-on-load sampled check) | SM ≥ 8.9 (40-series+); driver cublasLt needs CUDA 11.8+ (R520+) |
| **FP8 E5M2** | `fp8e5m2` / `e5m2` | `gemm` | cublasLt `CUDA_R_8F_E5M2` (same constraints as E4M3; saturating like hardware `cvt.rn.satfinite`) | SM ≥ 8.9; **kernel availability depends on the cublasLt build** — cublasLt 12.8.4/12.0.2 ship zero E5M2 candidates on Ada (observed on RTX 4060), and E4M3 is Ada's native FP8 format; runtime arbitration auto-SKIPs. Try `--cuda-dll-version 12.9` to test a newer cublasLt build |
| **FP4** | `fp4` | `gemm` | cublasLt `CUDA_R_4F_E2M1` + block scales (NVFP4 preferred, MXFP4 fallback; TN; sampled check) | SM 100/103/120 (50-series+); driver cublasLt needs CUDA 12.8+ (R570+) |
| **INT8** | `int8` / `i8` | `gemm` **_or_** `intalu` | cublasLt `CUDA_R_8I`→`CUDA_R_32I` (IMMA TC, TN default) or the custom DP4A/MAD `intalu` kernel; classic `gemm_ex` kept via `--gemm-legacy-cublas` | SM ≥ 5.0; IMMA TC requires SM ≥ 7.5 (Turing+) |
| **INT4** | `int4` / `i4` | `gemm` | cublasLt `CUDA_R_4I`→`CUDA_R_32I` (IMMA TC, TN; sampled + exact sidecar checks) | SM ≥ 7.5; **runtime availability depends on the driver's cublasLt returning `CUDA_R_4I` candidates** (clean error when none) |
| **INT16** | `int16` / `i16` | `intalu` **_only_** ¹ | Custom NVRTC (scalar int16 MAD chain) | All SM |
| **INT32** | `int32` / `i32` | `intalu` **_only_** ¹ | Custom NVRTC (scalar int32 MAD chain) | All SM |

> ¹ **INT16/INT32 have no cuBLAS GEMM path** (`CUDA_R_16I` / `CUDA_R_32I` are storage types only, with no corresponding `computeType`). To stress INT16/INT32 you must also specify `--kernel-types intalu`. The tool detects the missing combination at startup and exits with a clear error (no silent spin).

> **INT4 availability note**: INT4 tensor-core hardware exists on SM 8.0+ (PTX `mma.s4`), and the cuBLASLt type-combination table lists `CUDA_R_4I` + `CUBLAS_COMPUTE_32I` (SM80+ IMMA) — but different driver releases may return no heuristic candidates. The tool probes at runtime: no candidates → clean error exit (no silent spin), per the "readback is the only verdict" doctrine; when candidates exist, both the ride-on-load sampled check and the periodic exact sidecar guard correctness. INT6 does not exist in CUDA. INT64 scalar MAD is too slow to saturate GPU cores.

### Precision ↔ Kernel Compatibility Matrix

Enforced at startup; incompatible combos exit with an error:

| Precision | GEMM | IntAlu | Mem* / Atomic |
|---|---|---|---|
| FP64 / FP32 / TF32 / FP16 / BF16 | ✅ | — | ✅ |
| FP8 / FP4 / INT4 | ✅ (cublasLt only) | — | ❌ |
| INT8 | ✅ (recommended) | ✅ (pure-ALU fallback) | ✅ |
| INT16 / INT32 | ❌ ² | ✅ | ✅ |

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

> Build-generation selector: the `cuda11` / `cuda12` / `cuda13` features are mutually exclusive and one must be passed (e.g. `cargo build -p cli-stressor-rs --features cuda13 --release`); the default build (no CUDA generation) is a pure stub for parsing/unit tests.

CUDA support is behind a feature flag.

```bash
cargo run -p cli-stressor-rs --features cuda12 -- --duration 30 --precisions fp16,tf32
```

```bash
# Mixed FP + integer stress (recommended)
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions fp16,bf16,int8,int32 \
  --kernel-types gemm,intalu,memcpy

# INT16 stress (intalu only)
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions int16 \
  --kernel-types intalu

# INT8 cuBLAS GEMM + INT32 intalu mixed
cargo run -p cli-stressor-rs --features cuda12 -- \
  --duration 60 \
  --precisions int8,int32 \
  --kernel-types gemm,intalu
```

When enabling Vulkan graphics stress, build with both features:

```bash
cargo build --release -p cli-stressor-rs --features cuda12,vulkan
```

Optional: run with a config file (all options can be provided in config).

```bash
cargo run -p cli-stressor-rs --features cuda12 -- --config ./stressor.toml
```

### Config File

- Precedence: `explicit CLI value > config file > built-in default`
- Built-in autoscan profiles are under `cli-stressor-rs/profiles/`:
  - `40-50.toml`: tuned for NVIDIA GeForce RTX 40/50-series GPUs and selected automatically by auto-optimizer.
  - `standard.toml`: default profile for cards with 8G+ VRAM.
  - `low-vram.toml`: lower-VRAM profile for cards with 6G-8G VRAM.
- `kernel_mixture` supports two formats:
  - string: `"gemm:0.4,intalu:0.2,memcpy:0.2,reduction:0.2"`
  - map: `{ gemm = 0.4, intalu = 0.2, memcpy = 0.2, reduction = 0.2 }`
- `kernel_params.<kernel>` supports per-kernel overrides, including `precisions`
- `validate_interval = 0` disables periodic validation

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

### cublasLt Lane & Kernel Selection

All FP precisions plus INT8/INT4 GEMM stress default to **cublasLt** (semantically aligned with the classic `cublasGemmEx`: FP16/BF16 use the same `CUBLAS_COMPUTE_32F` + f32 scale, TF32 rides the desc-level `COMPUTE_32F_FAST_TF32`, FP64 uses `COMPUTE_64F` — readings stay comparable). Each new (precision, size, transpose) combination prints the identity of the selected kernel on first encounter:

```
[GEMM-LT] FP16 4096x4096 ta=T tb=N | algo #0/8: id=8 tile=34 stages=2 splitk=1 red=0 swz=1 ws=4.0MiB
```

- cublasLt does not expose SASS kernel names through its API; the tile/stages/split-K/reduction/swizzle config attributes ARE the user-visible kernel identity (SASS symbols require NCU).
- The bimodal FP16 end-to-end readings on unlocked cards (e.g. 49/63 TF on an unlocked RTX 4090) trace back to the library's static heuristics picking suboptimal kernels for the modified hardware (see [Harry-Chen/unlocked-ada-gemm](https://github.com/Harry-Chen/unlocked-ada-gemm) and the [original blog post](https://harrychen.xyz/2026/11/17/unlocked-rtx-4090-gemm-perf/)). `--gemm-algo-list` is the built-in "enumerate + time" probe and `--gemm-algo-index N` pins a candidate by hand; the report lines also land in the verdict JSON under `precisions[].gemm_algos`.

| Flag | Effect |
|---|---|
| `--gemm-heuristics <N>` | Candidate count requested per shape [8] |
| `--gemm-algo-index <N>` | Globally pin heuristic candidate #N (0 = library default; errors when a shape returns fewer candidates) |
| `--gemm-algo-list` | Time every candidate at first encounter of each shape (TFLOPS table, lane 0, 3 iters each) |
| `--gemm-legacy-cublas` | Route all FP precisions + INT8 through the classic cublas path (baseline comparability) |
| `--fp8-fast-accum` | FP8 uses `CUBLASLT_MATMUL_DESC_FAST_ACCUM` (faster but 10–15% relative error at large K; default is the precision-safe segmented accumulate) |
| `--gpu-generate` | FP8/FP4 operands come straight from NVRTC fill kernels in VRAM (uniform [-2,2] → in-kernel RN-even quantization; zero H2D, zero host gap); the default host path quantizes in parallel with O(1) bit-math codecs |

### Generation Compatibility Reference

#### INTEGER Paths

| Path | Mechanism | Min SM | Tensor Core | Notes |
|---|---|---|---|---|
| INT8 GEMM | cublasLt `CUDA_R_8I`→`CUDA_R_32I`, `CUBLAS_COMPUTE_32I` (TN forced, IMMA guaranteed); classic `gemm_ex`+`DEFAULT_TENSOR_OP` kept via `--gemm-legacy-cublas` | SM ≥ 5.0 (scalar); **SM ≥ 7.5 IMMA TC** | Lt's forced TN always hits IMMA; the classic lane's randomized transposes land only ~25% of ops on TN (rest fall to DP4A/scalar) | Recommended INT8 path |
| INT8 DP4A `__dp4a` (IntAlu) | NVRTC intrinsic | SM ≥ 6.1 (Pascal GP100/GP10x) | INT8 dot-product ALU (not mma) | Guarded by `__CUDA_ARCH__>=610` |
| INT16 ALU (IntAlu) | Scalar int16 MAD | All SM | None (no INT16 tensor core exists) | Universal; measured ~1.6 TIOPs on RTX 3060 |
| INT32 ALU (IntAlu) | Scalar int32 MAD | All SM | None | Universal |
| INT4 GEMM | cublasLt `CUDA_R_4I`→`CUDA_R_32I` (TN, 2 nibbles/byte) | SM ≥ 7.5 (type table lists SM80+ IMMA) | SM ≥ 8.0 `mma.s4` HW; **candidate existence probed at runtime** (clean error when none) | Sampled check exact (products ≤ 64·K < 2²⁴) + periodic exact sidecar |

> **Single-binary cross-generation coverage**: NVRTC `compile_ptx_with_opts(arch="compute_XX")` emits virtual-arch PTX; the driver JIT-compiles it to real SASS at module load time. The `compute_90` cap + `__CUDA_ARCH__` guards let a single binary cover Pascal → Blackwell.

#### Other Paths

| Path | Notes |
|---|---|
| FP64 / FP32 / FP16 GEMM | All SM |
| TF32 / BF16 GEMM | SM ≥ 8.0 (Ampere / 30-series+); older GPUs auto-skip with a message |
| FP8 GEMM | cublasLt E4M3FN, SM ≥ 8.9 (40-series+); FAST_ACCUM off by default |
| FP4 GEMM | cublasLt E2M1 + block scales, SM 100/103/120 (50-series+); auto-SKIP row on 40-series |
| Atomic kernel | SM ≥ 7.5 (Turing+); auto-disabled on older GPUs |
| Mem* / Reduction | All SM |

### Notes

- **Multi-GPU fan-out**: `--gpu-index 0,1,2` (comma-separated) spawns one worker process per card running the same arguments; every output line is prefixed `[GPU<i>]` and the overall exit code is the worst child. Process isolation keeps one card's driver fault from killing the other cards' stress (matching the auto-optimizer's per-GPU worker pattern). Single-index behavior is unchanged; `--pci-bus` / `--gpu-uuid` remain single-selection (use the index list for multi-GPU). `--vulkan-only` / `--video-only` use the first index only.
- **CUDA runtime-library version selection** (`--cuda-dll-version 12.8`): resolves, downloads, sha256-verifies, and extracts only the needed binaries from NVIDIA's official PyPI wheels (`nvidia-cublas-cu12/13` etc.) into a per-version cache under `%LOCALAPPDATA%
voc\cuda-dlls\`, then preloads them with `LoadLibraryExW` — the preloaded module wins cudarc's name-based lookup over any search-order shadowing (observed in the wild: cublasLt 12.0.2 shadowing a CUDA 13.5 driver). The major must match the build's cuda11/12/13 feature. Mainland-China networks: `--cuda-dll-mirror tuna|aliyun` (default pypi.org with automatic TUNA/Aliyun file fallback). Linux downloads the `.so` set into the same directory and prints the `LD_LIBRARY_PATH` setup line. `--cuda-dll-dir <DIR>` preloads a manual directory instead. The leading `[CUDA] versions` + `[CUDA] dlls` lines always record what actually loaded — two must-read lines for the matrix.
- TF32 and BF16 both use cuBLAS math-mode switching. BF16 requires SM80+ (Ampere and later); older architectures skip it with a clear message. **All FP precisions + INT8/INT4 GEMM default to the cublasLt lane** (see "cublasLt Lane & Kernel Selection" above); TF32's Lt form is the desc-level `COMPUTE_32F_FAST_TF32` (semantically the classic math mode).
- **INT8 GEMM** defaults to cublasLt TN (`CUDA_R_8I`→`CUDA_R_32I`, `CUBLAS_COMPUTE_32I`): the IMMA tensor-op path only supports the Aᵀ·B layout, and forcing TN on the Lt lane always hits it. On the classic `cublasGemmEx` lane, randomized transposes (`transpose_prob=0.5`) put only ~25% of ops on TN — the rest fall to DP4A/scalar paths. GPUs with SM < 7.5 receive a warning (the path still works, just at lower throughput). The INT8 tensor-op path requires matrix dimensions to be **multiples of 16**; the tool auto-aligns odd/unaligned sizes to `(size + 15) & !15`. INT8 has no ride-on-load sampled check (i32 dot products exceed 2²⁴ where the f32 compare is inexact); the periodic exact integer sidecar (`cpu_reference_int_tn`) covers it.
- **FP8 GEMM** runs cublasLt E4M3FN + `COMPUTE_32F` with f16 output; the TN layout is a hard cublasLt requirement (`transpose_prob` does not apply — the report line shows ta=T tb=N); sizes auto-align to multiples of 16. `--fp8-fast-accum` enables the FAST_ACCUM path (~90% of peak but 10–15% relative error accumulating at large K); the default segmented-accumulate path is precision-safe. Operands are RN-even quantized from the f32 random matrices with O(1) bit-math codecs (exhaustive idempotence tests over all 256 bit patterns), or generated in VRAM by the NVRTC fill kernels under `--gpu-generate` (device quantizer bit-aligned with the host one), and the sampled check recomputes exactly from the same A/B.
- **FP4 GEMM** runs cublasLt E2M1 + block scales (NVFP4: 16-element blocks with E4M3 scales, preferred; MXFP4 fallback — 32-element blocks with power-of-two UE8M0 scales — when the driver has no NVFP4 candidates), TN layout, sizes aligned to multiples of 32; Blackwell-only (SM100/103/120) with a CUDA 12.8+ driver, otherwise a clear error.
- **INT4 GEMM** runs cublasLt `CUDA_R_4I` (2 nibbles per byte, low nibble first), TN layout, sizes aligned to multiples of 32; guarded by the periodic exact sidecar (`cpu_reference_int4_tn`) plus an exact sampled check (products ≤ 64·K < 2²⁴, f32-exact, tolerance 0).
- **INT16 / INT32** have no cuBLAS GEMM path (`CUDA_R_16I` / `CUDA_R_32I` are storage-only types). If these precisions are requested without `intalu` in `--kernel-types`, the tool **exits with an error** at startup (no silent spin / apparent hang).
- The validation path uses CPU FP64 GEMM and compares values with element-wise tolerances. INT precisions skip the FP validation (`IntAlu` is a non-referenceable hash chain; INT8/INT4 GEMM are covered by the exact integer sidecar; FP8/FP4 by the ride-on-load sampled checks).
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
