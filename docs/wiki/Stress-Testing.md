# Stress Testing

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC ships two standalone GPU stressors. The former Python/PyTorch CUDA stressor (`cli-stressor-cuda`) was removed from the repo (#200) and folded conceptually into the Rust stressor; the CUDA stressor is now `cli-stressor-cuda-rs` only.

### Overview

| Tool | Language | Backend | Characteristics |
|---|---|---|---|
| `cli-stressor-cuda-rs` | Rust | CUDA (cuBLAS + NVRTC) | Native, no Python dependency. GEMM + integer ALU workloads, mixed kernel paths, periodic CPU-side validation |
| `cli-stressor-opencl` | Python | OpenCL | Lightweight, no CUDA/PyTorch stack. Compatibility check and first-pass screening only |

The Rust stressor is also bundled into auto-optimizer by default (#245): autoscan can invoke it either as an external binary or as a worker embedded in the optimizer executable itself. See [[Auto-Optimizer-Guide]] and [[Autoscan-Workflow]] for the autoscan integration.

### cli-stressor-opencl (Python + OpenCL)

A GEMM-based core-stability stress tool for environments without a CUDA stack. It drives randomized (including misaligned) matrix sizes across FP32 / FP16 (FP64 on supported devices), with periodic CPU FP64 sidecar validation to catch silent data corruption.

#### Install and run

```bash
cd cli-stressor-opencl
uv sync
uv run test.py [options]
```

(`python test.py` works inside an already-activated environment; only `numpy` + `pyopencl` are required.)

#### Key options

| Option | Default | Description |
|---|---|---|
| `--duration` | `90.0` | Stress duration per precision (seconds) |
| `--precisions` | `fp16,fp32` | Precision list: `fp16` `fp32` `fp64` (auto-skipped per device capability) |
| `--matrix-sizes` | `2049,4096,4097,8192,8193,16384` | Random matrix sizes for the main workload |
| `--fp64-matrix-sizes` | `2048,4096` | Sizes dedicated to FP64 mode (fits consumer FP64 throughput) |
| `--warmup-iters` | `3` | Warmup rounds per workload window |
| `--burst-iters` | `6` | Main stress rounds per workload window |
| `--validate-interval` | `10` | Sidecar validation interval (seconds) |
| `--validate-size` | `1024` | Fixed matrix size used by validation |
| `--transpose-prob` | `0.5` | Probability of transposing A/B to perturb the kernel path |
| `--seed` | `12345` | Random seed for reproducible runs |

#### MANDATORY WARNING — OpenCL is a first-pass screen, not a final gate

> **The OpenCL stressor is NOT high-pressure enough to be the final overclocking stability gate.** OpenCL-only passes can inflate autoscan / V-F curve results above what the GPU can actually sustain; applying those inflated results long term can cause driver resets, system instability, data corruption, or hardware failure. Use OpenCL only for compatibility checks and first-pass screening, and **revalidate every accepted result with the CUDA stressor (`cli-stressor-cuda-rs`) or heavier real workloads.**

### cli-stressor-cuda-rs (Rust + CUDA)

A native CUDA compute stressor. It mixes randomized workloads across multiple kernel paths with periodic CPU-side validation to detect silent data corruption and instability.

#### Workload types

- **GEMM** (cuBLAS): FP64 / FP32 / TF32 / FP16 / BF16, plus **INT8** GEMM (`cublasGemmEx` with IMMA tensor-core dispatch on Turing+/SM ≥ 7.5, DP4A/scalar fallback on older GPUs).
- **Integer ALU** (`intalu` kernel path, #246): a custom NVRTC kernel driving integer multiply-accumulate chains — INT32 MAD, INT16 narrowing ops, and INT8 DP4A dot products (Pascal+ `__dp4a`, scalar MAD fallback on older GPUs). INT16/INT32 have **no cuBLAS GEMM path**; requesting them without `--kernel-types intalu` exits with an error at startup.
- **Memory / atomics**: memcpy / memset / transpose / elementwise / reduction, plus a custom NVRTC atomic kernel (SM ≥ 7.5 recommended; auto-disabled below SM 8.0).
- Precisions: `fp64,fp32,tf32,fp16,bf16,fp8,int8,int16,int32` (aliases `i8/i16/i32` accepted; FP8 is not yet implemented; TF32/BF16 need SM ≥ 8.0 and auto-skip on older GPUs).

#### Verification and output

- Periodic **CPU FP64 reference validation**: the GPU result is compared element-wise against a CPU-computed reference within tolerances (`--validate-interval`, `0` disables). INT paths are annotated "validation skipped for INT path" (the IntAlu chain is a non-referenceable hash chain; INT8 GEMM has no CPU reference yet).
- The final summary prints one row per precision: `OK` / `FAIL` / `SKIP` status, iteration count, wall/compute time, efficiency, throughput, validation-failure count, and max absolute/relative error.
- **Exit code** is the pass/fail criterion: `0` = stable, non-zero (`1` runtime/validation failure, `2` invalid arguments/config) = fail.

#### Build: two CUDA generations

CUDA support is a feature flag, and the CUDA driver generation is selected with a mutually exclusive feature pair (enforced at compile time in `src/lib.rs`):

| Build | Feature | cudarc backend | Target |
|---|---|---|---|
| CUDA 12 (default) | `cuda12` (default) | `cudarc/cuda-12090` | Modern drivers/GPUs; NVRTC PTX capped at `compute_90`, driver JIT-covers Pascal → Blackwell |
| CUDA 11 (legacy) | `cuda11` | `cudarc/cuda-11040` | R470-era legacy drivers (CUDA 11.4 API surface: no `cuDeviceGetUuid_v2`, NVRTC capped at `compute_86`) |

```bash
# CUDA 12 build (default features):
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32

# CUDA 11 legacy build:
cargo build --release --no-default-features --features cuda11
```

The two features are mutually exclusive because cudarc resolves its bindings and the NVRTC library name from exactly one `cuda-XXXXXX` feature. Whichever generation you build, the matching runtime libraries must be discoverable (`nvrtc64_*.dll`, `cublasLt64_*.dll`, `cublas64_*.dll`, `cudart64_*.dll` on Windows; the equivalent `.so` files on Linux via `LD_LIBRARY_PATH` or `ldconfig`).

Add `vulkan` to the feature list to enable the optional Vulkan graphics stress sidecar (also required by the built-in `minload` profile), e.g. `--features cuda,vulkan` (CUDA 12) or `--no-default-features --features cuda11,vulkan` (legacy).

#### Common run commands

```bash
# Mixed FP + integer stress (recommended)
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 --precisions fp16,bf16,int8,int32 --kernel-types gemm,intalu,memcpy

# INT16 stress (intalu only)
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 --precisions int16 --kernel-types intalu
```

#### Key options

Precedence: `explicit CLI value > --config/--profile file > built-in default`. `--profile` selects one of the TOML profiles compiled into the binary (`standard`, `low-vram`, `40-50` for GeForce RTX 40/50-series and auto-selected by auto-optimizer, `minload`, `dynamic-export`); `--config` reads a user TOML file instead. The two conflict with each other.

| Option | Default | Description |
|---|---|---|
| `--duration` | `90.0` | Stress duration per precision (seconds) |
| `--precisions` | `fp16,bf16` | Precision list (see workload types above) |
| `--kernel-types` | all eight | Enabled kernel paths: `gemm,memcpy,memset,transpose,elementwise,reduction,atomic,intalu` |
| `--kernel-mixture` | empty (equal weights) | Kernel weights, e.g. `gemm:0.5,intalu:0.3,reduction:0.2` |
| `--kernel-params` | — | Per-kernel overrides, e.g. `gemm:precisions=fp16|bf16,matrix_sizes=2049|4096,warmup=4,burst=8` |
| `--matrix-sizes` / `--fp64-matrix-sizes` | `2049,4096,...` / `2048,4096` | Random matrix sizes (misaligned sizes included) |
| `--warmup-iters` / `--burst-iters` | `3` / `6` | Warmup/burst rounds per workload window |
| `--validate-interval` | `5.0` | CPU FP64 validation interval in seconds (`0` disables) |
| `--validate-size` | `1024` | Validation matrix size |
| `--transpose-prob` | `0.5` | Probability of transposing A/B |
| `--minor-mixture-rate` | `0.15` | Rate at which small "minor" workloads are mixed in |
| `--seed` | `12345` | Base random seed |
| `--stream-mode` | `single` | Submission streams: `single` / `dual` / `triple` |
| `--gpu-index` / `--pci-bus` / `--gpu-uuid` | first GPU (PCI-sorted index 0) | GPU selection (mutually exclusive); `--list-gpus` enumerates devices |
| `--enable-vulkan-stress` / `--vulkan-only` | off | Vulkan graphics stress sidecar (requires the `vulkan` feature), with image size/count/MSAA options |

#### Throughput note (host-side RNG/perf fix)

Host matrix generation is rayon-parallelized with deterministic per-chunk seeding (each chunk's `StdRng` is derived from the base seed via the splitmix64 golden-ratio multiplier), and the GEMM output buffer is allocated once and reused across warmup+burst iterations. As a result, higher `--burst-iters` raises sustained GPU load instead of starving it — reproducibility and FP64 validation are unaffected.

#### Example config: dynamic-export / 50-series profile

`auto-optimizer/test/cli-stressor-cuda-rs-dyn-export-50series.toml` (an example file under `auto-optimizer/test/`) shows a GEMM-only variant of the embedded `dynamic-export` profile tuned for a GeForce RTX 50-series card:

```toml
fp64_matrix_sizes = [2048,4096]
precisions = [ "fp16"]
duration = 45
validate_interval = 0
validate_size = 1024
transpose_prob = 0.5
seed = 12345
kernel_types = ["gemm"]
kernel_mixture = { gemm = 1.0 }
stream_mode = "dual"
disable_fp8 = true
vulkan_only = false
enable_vulkan_stress = false

[kernel_params.gemm]
matrix_sizes = [8192]
precisions = ["fp16"]
precision_weight = [1.0]
warmup_iters = 0
burst_iters = 30
minor_mixture_rate = 0
weight = 1.00
```

This is the load auto-optimizer runs (~45 s) before reading sensors in dynamic V-F export mode; the repo's built-in `dynamic-export` profile is identical except it defaults to FP32 at the top level. The embedded `dynamic-export` profile is selected by auto-optimizer itself.

#### Bundled into auto-optimizer

Since #245, auto-optimizer builds `cli-stressor-cuda-rs` in by default (its `stressor-bundled` feature embeds the stressor with CUDA + Vulkan). A scan can reference the sentinel `@bundled:cli-stressor-cuda-rs` instead of a binary path; the optimizer then re-executes its own executable with an internal worker flag (`--nvoc-stressor-cuda-rs-worker`), so a fatal CUDA failure in the child cannot poison the long-running optimizer process. External stressor binaries remain supported. Scan-side options (`--stressor-backend`, `stressor_profile`, `stressor_config`, `stressor_extra_args`) and the resilient JSONL-based scan resume introduced alongside the integer workloads (#246) are covered in [[Autoscan-Workflow]] and [[Auto-Optimizer-Guide]].

### Pass/Fail Criteria

All stress testing uses the **process exit code** as the criterion:

- `0` = pass (stable)
- non-zero = fail (unstable, or invalid configuration)

During autoscan, auto-optimizer launches the stressor (bundled worker or external binary) per scan point and raises/lowers the frequency offset based on the exit code.

### Which stressor should I use?

| Situation | Use |
|---|---|
| Final overclocking stability validation (required) | `cli-stressor-cuda-rs` (CUDA 12 build) |
| Legacy system on an R470-era driver / older GPU | `cli-stressor-cuda-rs` CUDA 11 build (`--no-default-features --features cuda11`) |
| No CUDA stack; non-NVIDIA or cross-platform GPU | `cli-stressor-opencl` — first-pass screening **only**; revalidate with the CUDA stressor |
| Autoscan / V-F curve sweeps | auto-optimizer with the bundled CUDA stressor (see [[Autoscan-Workflow]]) |

Generation-level support (which GPU families the CUDA/OpenCL backends cover) is tracked in [[GPU-Support-Matrix]].

> Overclocking and stress testing push the GPU beyond stock operating limits. Read [[Safety-and-Recovery]] before running scans, keep recovery behavior visible, and never leave an inflated (OpenCL-only) result applied long term.

---

<a id="chinese"></a>

## 中文

NVOC 目前提供两个独立的 GPU 压力测试工具。此前的 Python/PyTorch CUDA 压力工具（`cli-stressor-cuda`）已从仓库移除（#200），其设计思路已合并进 Rust 压力工具；CUDA 压力测试现在只由 `cli-stressor-cuda-rs` 承担。

### 概览

| 工具 | 语言 | 后端 | 特点 |
|---|---|---|---|
| `cli-stressor-cuda-rs` | Rust | CUDA（cuBLAS + NVRTC） | 原生实现，无 Python 依赖。GEMM + 整数 ALU 负载，多 kernel 路径混合，周期性 CPU 侧校验 |
| `cli-stressor-opencl` | Python | OpenCL | 轻量，不依赖 CUDA/PyTorch 栈。仅用于兼容性检查与初筛 |

Rust 压力工具默认还会被打包进 auto-optimizer（#245）：autoscan 既可调用外部二进制，也可调用内嵌在 optimizer 可执行文件中的 worker。autoscan 集成细节见 [[Auto-Optimizer-Guide]] 与 [[Autoscan-Workflow]]。

### cli-stressor-opencl（Python + OpenCL）

面向无 CUDA 栈环境的 GEMM 核心稳定性压力工具。它以随机化（含非对齐）矩阵尺寸驱动 FP32 / FP16（受支持设备上的 FP64）负载，并周期性使用 CPU FP64 旁路校验捕获静默数据错误。

#### 安装与运行

```bash
cd cli-stressor-opencl
uv sync
uv run test.py [参数]
```

（在已激活的 Python 环境中也可 `python test.py`；仅需要 `numpy` + `pyopencl`。）

#### 关键参数

| 参数 | 默认值 | 说明 |
|---|---|---|
| `--duration` | `90.0` | 每精度压力持续时间（秒） |
| `--precisions` | `fp16,fp32` | 精度列表：`fp16` `fp32` `fp64`（按设备能力自动跳过） |
| `--matrix-sizes` | `2049,4096,4097,8192,8193,16384` | 主负载的随机矩阵尺寸 |
| `--fp64-matrix-sizes` | `2048,4096` | FP64 模式专用尺寸（适配消费级 FP64 吞吐） |
| `--warmup-iters` | `3` | 每个工作负载窗口的预热轮数 |
| `--burst-iters` | `6` | 每个工作负载窗口的正式压力轮数 |
| `--validate-interval` | `10` | 旁路校验间隔（秒） |
| `--validate-size` | `1024` | 校验所用固定矩阵尺寸 |
| `--transpose-prob` | `0.5` | 随机转置 A/B 以扰动 kernel 路径的概率 |
| `--seed` | `12345` | 随机种子，保证结果可复现 |

#### 强制警告 —— OpenCL 只是初筛，不是最终判据

> **OpenCL 压力工具的压力不足以作为最终超频稳定性判据。** 仅通过 OpenCL 测试可能得到高于 GPU 真实稳定上限的 autoscan / V-F 曲线结果；长期套用这些偏高结果可能导致驱动重置、系统不稳定、数据损坏甚至硬件故障。请仅将 OpenCL 用于兼容性检查与初筛，并**使用 CUDA 压力工具（`cli-stressor-cuda-rs`）或更重的真实负载重新验证每一个被采纳的结果**。

### cli-stressor-cuda-rs（Rust + CUDA）

原生 CUDA 计算压力工具。它在多条 kernel 路径上混合随机化负载，并通过周期性 CPU 侧校验检测静默数据错误与硬件不稳定。

#### 负载类型

- **GEMM**（cuBLAS）：FP64 / FP32 / TF32 / FP16 / BF16，以及 **INT8** GEMM（`cublasGemmEx`，Turing+/SM ≥ 7.5 自动走 IMMA tensor core，更老 GPU 回退 DP4A/标量路径）。
- **整数 ALU**（`intalu` kernel 路径，#246）：自定义 NVRTC kernel，驱动整数乘加链 —— INT32 MAD、INT16 窄化运算、INT8 DP4A 点积（Pascal+ `__dp4a`，更老 GPU 回退标量 MAD）。INT16/INT32 **没有 cuBLAS GEMM 路径**；未在 `--kernel-types` 中加入 `intalu` 时工具会在启动时报错退出。
- **访存 / 原子**：memcpy / memset / transpose / elementwise / reduction，外加自定义 NVRTC atomic kernel（建议 SM ≥ 7.5；SM 8.0 以下自动禁用）。
- 支持精度：`fp64,fp32,tf32,fp16,bf16,fp8,int8,int16,int32`（接受别名 `i8/i16/i32`；FP8 尚未实现；TF32/BF16 需要 SM ≥ 8.0，旧架构自动跳过）。

#### 校验与输出

- 周期性 **CPU FP64 参考校验**：GPU 结果与 CPU 参考结果按元素带容差比较（`--validate-interval`，`0` 表示关闭）。INT 路径会标注 "validation skipped for INT path"（IntAlu 链为不可比对 hash 链；INT8 GEMM 尚无 CPU 参考实现）。
- 最终摘要按精度逐行输出：`OK` / `FAIL` / `SKIP` 状态、迭代数、墙钟/纯计算时间、效率、吞吐、校验失败计数、最大绝对/相对误差。
- **进程退出码**即通过/失败判据：`0` = 稳定，非零（`1` 运行/校验失败，`2` 参数/配置非法）= 失败。

#### 构建：两代 CUDA

CUDA 支持由 feature flag 控制，CUDA 驱动代际由一对互斥 feature 选择（`src/lib.rs` 在编译期强制）：

| 构建 | Feature | cudarc 后端 | 目标 |
|---|---|---|---|
| CUDA 12（默认） | `cuda12`（默认） | `cudarc/cuda-12090` | 现代驱动/GPU；NVRTC PTX 上限 `compute_90`，由驱动 JIT 覆盖 Pascal → Blackwell |
| CUDA 11（传统） | `cuda11` | `cudarc/cuda-11040` | R470 时代的旧驱动（CUDA 11.4 API 面：无 `cuDeviceGetUuid_v2`，NVRTC 上限 `compute_86`） |

```bash
# CUDA 12 构建（默认 feature）：
cargo run -p cli-stressor-cuda-rs --features cuda -- --duration 30 --precisions fp16,tf32

# CUDA 11 传统构建：
cargo build --release --no-default-features --features cuda11
```

两个 feature 互斥，因为 cudarc 只能从一个 `cuda-XXXXXX` feature 解析绑定与 NVRTC 库名。无论构建哪一代，运行时都必须能找到匹配的动态库（Windows 上为 `nvrtc64_*.dll`、`cublasLt64_*.dll`、`cublas64_*.dll`、`cudart64_*.dll`；Linux 上为对应的 `.so`，通过 `LD_LIBRARY_PATH` 或 `ldconfig` 提供）。

在 feature 列表中追加 `vulkan` 可启用可选的 Vulkan 图形压力侧车（内置 `minload` profile 也需要它），例如 `--features cuda,vulkan`（CUDA 12）或 `--no-default-features --features cuda11,vulkan`（传统构建）。

#### 常用运行命令

```bash
# FP + 整数混合压力（推荐）
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 --precisions fp16,bf16,int8,int32 --kernel-types gemm,intalu,memcpy

# INT16 压力（仅 intalu）
cargo run -p cli-stressor-cuda-rs --features cuda -- \
  --duration 60 --precisions int16 --kernel-types intalu
```

#### 关键参数

优先级：`命令行显式传入 > --config/--profile 文件 > 内置默认值`。`--profile` 选择编译进二进制的内置 TOML profile（`standard`、`low-vram`、面向 GeForce RTX 40/50 系并由 auto-optimizer 自动选择的 `40-50`、`minload`、`dynamic-export`）；`--config` 则读取用户 TOML 文件。两者互斥。

| 参数 | 默认值 | 说明 |
|---|---|---|
| `--duration` | `90.0` | 每精度压力持续时间（秒） |
| `--precisions` | `fp16,bf16` | 精度列表（见上文负载类型） |
| `--kernel-types` | 全部八种 | 启用的 kernel 路径：`gemm,memcpy,memset,transpose,elementwise,reduction,atomic,intalu` |
| `--kernel-mixture` | 空（等权） | kernel 权重，如 `gemm:0.5,intalu:0.3,reduction:0.2` |
| `--kernel-params` | — | 按 kernel 覆盖参数，如 `gemm:precisions=fp16|bf16,matrix_sizes=2049|4096,warmup=4,burst=8` |
| `--matrix-sizes` / `--fp64-matrix-sizes` | `2049,4096,...` / `2048,4096` | 随机矩阵尺寸（含非对齐尺寸） |
| `--warmup-iters` / `--burst-iters` | `3` / `6` | 每个工作负载窗口的预热/突发轮数 |
| `--validate-interval` | `5.0` | CPU FP64 校验间隔（秒，`0` 关闭） |
| `--validate-size` | `1024` | 校验矩阵尺寸 |
| `--transpose-prob` | `0.5` | 随机转置 A/B 的概率 |
| `--minor-mixture-rate` | `0.15` | 小负载（minor workload）混入比例 |
| `--seed` | `12345` | 基础随机种子 |
| `--stream-mode` | `single` | 提交流模式：`single` / `dual` / `triple` |
| `--gpu-index` / `--pci-bus` / `--gpu-uuid` | 第一块 GPU（PCI 排序索引 0） | GPU 选择（三者互斥）；`--list-gpus` 枚举设备 |
| `--enable-vulkan-stress` / `--vulkan-only` | 关闭 | Vulkan 图形压力侧车（需 `vulkan` feature），含图像尺寸/数量/MSAA 选项 |

#### 吞吐说明（主机侧 RNG/性能修复）

主机端矩阵生成已用 rayon 并行化，并采用确定性的按块播种（每块的 `StdRng` 由基础种子经 splitmix64 黄金比例乘数派生），同时 GEMM 输出缓冲区每次调用只分配一次并在 warmup+burst 迭代间复用。因此更高的 `--burst-iters` 会提高 GPU 持续负载而不是让 GPU 空转 —— 可复现性与 FP64 校验均不受影响。

#### 示例配置：dynamic-export / 50 系 profile

`auto-optimizer/test/cli-stressor-cuda-rs-dyn-export-50series.toml`（`auto-optimizer/test/` 下的示例文件）展示了内置 `dynamic-export` profile 面向 GeForce RTX 50 系显卡的 GEMM-only 变体：

```toml
fp64_matrix_sizes = [2048,4096]
precisions = [ "fp16"]
duration = 45
validate_interval = 0
validate_size = 1024
transpose_prob = 0.5
seed = 12345
kernel_types = ["gemm"]
kernel_mixture = { gemm = 1.0 }
stream_mode = "dual"
disable_fp8 = true
vulkan_only = false
enable_vulkan_stress = false

[kernel_params.gemm]
matrix_sizes = [8192]
precisions = ["fp16"]
precision_weight = [1.0]
warmup_iters = 0
burst_iters = 30
minor_mixture_rate = 0
weight = 1.00
```

这是 auto-optimizer 在动态 V-F 导出模式下读取传感器之前运行的负载（约 45 秒）；仓库内置的 `dynamic-export` profile 与之相同，仅顶层默认精度为 FP32。内置 `dynamic-export` profile 由 auto-optimizer 自动选择。

#### 打包进 auto-optimizer

自 #245 起，auto-optimizer 默认将 `cli-stressor-cuda-rs` 编译打包（其 `stressor-bundled` feature 内嵌带 CUDA + Vulkan 的压力工具）。扫描配置可以用哨兵值 `@bundled:cli-stressor-cuda-rs` 代替二进制路径；此时 optimizer 会以内部 worker 标志（`--nvoc-stressor-cuda-rs-worker`）重新执行自身可执行文件，从而保证子进程中的致命 CUDA 故障不会污染长期运行的 optimizer 进程。外部压力二进制仍然支持。扫描侧选项（`--stressor-backend`、`stressor_profile`、`stressor_config`、`stressor_extra_args`）以及随整数负载一同引入的、基于 JSONL 的韧性扫描续跑（#246）见 [[Autoscan-Workflow]] 与 [[Auto-Optimizer-Guide]]。

### 判定标准

所有压力测试统一使用**进程退出码**判据：

- `0` = 通过（稳定）
- 非 `0` = 失败（不稳定，或配置/参数非法）

autoscan 流程中，auto-optimizer 在每个扫描点启动压力工具（内嵌 worker 或外部二进制），根据退出码决定升高或降低频率偏移。

### 我该用哪个压力工具？

| 场景 | 选择 |
|---|---|
| 最终超频稳定性验证（必做） | `cli-stressor-cuda-rs`（CUDA 12 构建） |
| 使用 R470 时代旧驱动 / 旧 GPU 的系统 | `cli-stressor-cuda-rs` CUDA 11 构建（`--no-default-features --features cuda11`） |
| 无 CUDA 栈；非 NVIDIA 或跨平台 GPU | `cli-stressor-opencl` —— **仅限**初筛；须用 CUDA 压力工具复验 |
| Autoscan / V-F 曲线扫描 | auto-optimizer + 内嵌 CUDA 压力工具（见 [[Autoscan-Workflow]]） |

各代际的 GPU 支持范围（CUDA/OpenCL 后端覆盖哪些 GPU 系列）见 [[GPU-Support-Matrix]]。

> 超频与压力测试会使 GPU 超出出厂运行限制。运行扫描前请先阅读 [[Safety-and-Recovery]]，保持恢复手段可见，切勿长期套用仅由 OpenCL 得出的偏高结果。

---

*Maintained from: `cli-stressor-cuda-rs/`, `cli-stressor-opencl/`, `auto-optimizer/` (bundled stressor, dynamic-export load profile, resilient scan resume).*
