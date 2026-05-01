# NVOC-CLI-Stressor

## 配套产品——使用所有配套产品以达到最好体验

[NVOC-AUTOOPTIMIZER](https://github.com/Skyworks-Neo/NVOC-AutoOptimizer)：核心模块。

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor)：压力测试模块，用于自动超频扫描部分。没有该模块仍可以使用自动扫描之外的所有功能。（NVOC-AutoOptimizer开放任何你的自定义压力测试模块接入，只需满足return
code定义即可。）

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI)：跨平台超频图形界面，直接对标MSI Afterburner。 （为了避免GPU超炸带走图形界面，使用CPU渲染，在低端机器如遇到性能问题，建议使用NVOC-TUI）；

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI)：跨平台超频命令行界面，用于没有图形界面的机器，兼容性好，性能要求低；

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV)：client-server架构控制模块，用于机房、服务器、工作站等场景的 Web 管理、~~远程超频~~（TODO）

> Language switch / 语言切换: [中文](#zh-cn) | [English](#en)
>
> License / 许可证: [Apache 2.0](LICENSE)
>
> 本仓库根目录的 `LICENSE` 适用于所有分支（包括 CUDA 和 opencl 分支）。

---

## 中文 <a id="zh-cn"></a>

这是一个基于 PyTorch 的 GPU 核心域稳定性压力测试工具。它通过时间驱动、随机化的通用矩阵乘法（GEMM）工作负载，结合旁路校验机制来对显卡进行压力测试，能够有效检测显卡的静默计算错误（Silent Data Corruption）或硬件稳定性问题。

> **提示**：本项目包含 `CUDA`（默认，基于 PyTorch 的高级特性与严格校验）和 `opencl`（轻量级，基于 OpenCL 实现，不再依赖体积巨大的 CUDA 版 PyTorch，用于广泛的跨平台支持）两个分支。建议根据您的测试需求选择合适的分支。

### 功能特点

- **多精度支持**：支持测试多种计算精度，包括 FP64、FP32、TF32、FP16、BF16 和 FP8（E4M3FN）。`opencl` 分支支持 FP32、FP16，并在受支持设备上兼容 FP64。
- **随机化工作负载**：动态改变矩阵尺寸（包含非对齐的尺寸），制造冷热交替的计算阶段，对显卡供电和内存分配器施加压力。
- **跨后端支持（多分支）**：主分支默认执行严格的 CUDA 验证，`opencl` 分支允许您在非 CUDA 平台环境（如某些核显等）免驱或免庞大依赖进行压力测试。
- **旁路数据校验**：周期性中断压力测试，并使用 CPU 上 FP64 参考算法进行确定性计算校验，捕获静默错误。
- **持续高压执行**：可自定义执行时长，对 GPU 持续平缓或剧烈施压。

### 环境要求

- 最低 Python 版本：`>=3.11`
- 兼容的 CUDA 硬件（推荐 NVIDIA GPU）或 Apple MPS 平台。

### 安装说明

项目推荐使用 `uv` 进行虚拟环境和依赖的管理。

1. 克隆本仓库：

```bash
git clone https://github.com/your-username/NVOC-CLI-Stressor.git
cd NVOC-CLI-Stressor
```

2. 安装依赖并自动建立独立环境：

```bash
uv sync
```

或者如果您习惯使用 pip 手动安装：

```bash
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu129
pip install numpy
```

### 使用方法

借助 `uv` 可以直接运行测试环境并拉起测试：

```bash
uv run test.py [参数]
```

如果是在当前 Python 环境中：

```bash
python test.py [参数]
```

#### 可用参数

- `--duration`（默认：90.0）：每个精度模式的压力持续时间（秒）。
- `--matrix-sizes`（默认：`2049, 4096, 4097, 8192, 8193, 16384`）：用于常规压力测试的随机矩阵尺寸列表，以逗号分隔。
- `--fp64-matrix-sizes`（默认：`2048, 4096`）：专门用于 FP64 模式的矩阵尺寸，为了适应消费级 GPU 较低的双精度算力（避免测试假死）。
- `--precisions`（默认：`fp16,bf16`）：需要测试的精度列表。可选：`fp64`、`fp32`、`tf32`、`fp16`、`bf16`、`fp8`。
- `--warmup-iters`（默认：3）：每个工作负载窗口的预热轮数。
- `--burst-iters`（默认：6）：每个工作负载窗口的正式压力突发轮数。
- `--validate-interval`（默认：10）：旁路校验的间隔秒数。
- `--validate-size`（默认：768）：旁路校验所用的固定矩阵尺寸。
- `--transpose-prob`（默认：0.5）：随机转置 a/b 矩阵的概率，用于轻度扰动 kernel 执行路径。
- `--seed`（默认：12345）：随机种子，保证结果一致性。
- `--disable-fp8`：添加此标志可强制跳过 FP8 测试，即便当前环境支持被检测到可用。

### 输出日志概览

- 检测并输出基本的设备架构信息（架构版本、Compute Capability 等）。
- 实时持续显示迭代矩阵的尺寸、瞬间算力（TFLOPS）及验证进度。
- 在每个精度测试结束后，核心稳定性总结面板将给出是否出现报错的情况。

### 许可证

本项目采用 Apache License 2.0，详见根目录的 `LICENSE` 文件。

---

## English <a id="en"></a>

This is a PyTorch-based GPU core-stability stress tool. It drives randomized generalized matrix multiplication (GEMM) workloads over time, combined with a sidecar validation path, to stress the GPU and help detect silent data corruption or hardware stability issues.

> **Note**: This repository includes a `CUDA` branch (default, with PyTorch advanced features and strict validation) and an `opencl` branch (lightweight, OpenCL-based, no dependency on the large CUDA PyTorch build, for broader cross-platform support). Choose the branch that matches your testing needs.

### Features

- **Multiple precisions**: Supports FP64, FP32, TF32, FP16, BF16, and FP8 (E4M3FN). The `opencl` branch supports FP32 and FP16, and FP64 on supported devices.
- **Randomized workloads**: Dynamically changes matrix sizes, including misaligned sizes, to alternate hot and cold compute phases and stress power delivery plus memory allocators.
- **Cross-backend support (multi-branch)**: The main branch performs strict CUDA validation by default, while the `opencl` branch enables stress testing on non-CUDA platforms without heavy dependencies.
- **Sidecar validation**: Periodically interrupts the stress loop and runs deterministic CPU-side FP64 reference checks to catch silent errors.
- **Sustained high load**: You can customize the runtime to apply steady or aggressive pressure to the GPU.

### Requirements

- Minimum Python version: `>=3.11`
- Compatible CUDA hardware (NVIDIA GPU recommended) or Apple MPS.

### Installation

This project recommends `uv` for virtual environment and dependency management.

1. Clone the repository:

```bash
git clone https://github.com/your-username/NVOC-CLI-Stressor.git
cd NVOC-CLI-Stressor
```

2. Install dependencies and create the environment automatically:

```bash
uv sync
```

Or, if you prefer manual pip installation:

```bash
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu129
pip install numpy
```

### Usage

Run the stress test directly with `uv`:

```bash
uv run test.py [arguments]
```

Or use the current Python environment:

```bash
python test.py [arguments]
```

#### Available arguments

- `--duration` (default: 90.0): Stress duration per precision mode, in seconds.
- `--matrix-sizes` (default: `2049, 4096, 4097, 8192, 8193, 16384`): Comma-separated list of random matrix sizes for the main stress workload.
- `--fp64-matrix-sizes` (default: `2048, 4096`): Matrix sizes dedicated to FP64 mode to better fit consumer GPUs with limited double-precision throughput.
- `--precisions` (default: `fp16,bf16`): Precision list to test. Supported values: `fp64`, `fp32`, `tf32`, `fp16`, `bf16`, `fp8`.
- `--warmup-iters` (default: 3): Warmup rounds for each workload window.
- `--burst-iters` (default: 6): Main stress rounds for each workload window.
- `--validate-interval` (default: 10): Interval, in seconds, for sidecar validation.
- `--validate-size` (default: 768): Fixed matrix size used by validation.
- `--transpose-prob` (default: 0.5): Probability of randomly transposing matrix A/B to perturb the kernel path.
- `--seed` (default: 12345): Random seed for reproducible runs.
- `--disable-fp8`: Force skipping FP8 tests even if the runtime reports support.

### Log overview

- Detects and prints basic device architecture information such as compute capability.
- Continuously reports matrix size, instantaneous throughput (TFLOPS), and validation progress.
- After each precision test, the final summary shows whether any runtime error occurred.

### License

This project is licensed under the Apache License 2.0. See the root `LICENSE` file for details.
