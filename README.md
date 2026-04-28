# NVOC-CLI-Stressor

这是一个基于 PyTorch 的 GPU 核心域稳定性压力测试工具。它通过时间驱动、随机化的通用矩阵乘法 (GEMM) 工作负载，结合旁路校验机制来对显卡进行压力测试，能够有效检测显卡的静默计算错误 (Silent Data Corruption) 或硬件稳定性问题。

> **提示**：本项目包含 `CUDA`（默认，基于 PyTorch 的高级特性与严格校验）和 `opencl`（轻量级，基于 OpenCL 实现，不再依赖体积巨大的 CUDA 版 PyTorch，用于广泛的跨平台支持）两个分支。建议根据您的测试需求选择合适的分支。

## 功能特点

- **多精度支持**: 支持测试多种计算精度，包括 FP64, FP32, TF32, FP16, BF16 和 FP8 (E4M3FN)。(OpenCL 分支支持 FP32、FP16，并在受支持设备上兼容 FP64)
- **随机化工作负载**: 动态改变矩阵尺寸（包含非对齐的尺寸），制造冷热交替的计算阶段，对显卡供电和内存分配器施加压力。
- **跨后端支持 (多分支)**: 主分支默认执行严格的 CUDA 验证，`opencl`分支允许您在非 CUDA 平台环境（如某些核显等）免驱/免庞大依赖进行压力测试。
- **旁路数据校验**: 周期性中断压力测试，并使用 CPU 上 FP64 参考算法进行确定性计算校验，捕获静默错误。
- **持续高压执行**: 可自定义执行时长，对 GPU 持续平缓或剧烈施压。

## 环境要求

- 最低 Python 版本：`>=3.11`
- 兼容的 CUDA 硬件（推荐 NVIDIA GPU）或是 Apple MPS 平台。

## 安装说明

项目推荐使用 `uv` 进行虚拟环境和依赖的管理。

1. 克隆本仓库:
```bash
git clone https://github.com/your-username/NVOC-CLI-Stressor.git
cd NVOC-CLI-Stressor
```

2. 安装依赖并自动建立独立环境:
```bash
uv sync
```

或者如果您习惯使用 pip 手动安装：
```bash
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu129
pip install numpy
```

## 使用方法

借助 `uv` 可以直接运行测试环境并拉起测试：

```bash
uv run test.py [参数]
```

如果是在当前 Python 环境中：
```bash
python test.py [参数]
```

### 可用参数

- `--duration` (默认: 90.0): 每个精度模式的压力持续时间（秒）。
- `--matrix-sizes` (默认: `2049, 4096, 4097, 8192, 8193, 16384`): 用于常规压力测试的随机矩阵尺寸列表，以逗号分隔。
- `--fp64-matrix-sizes` (默认: `2048, 4096`): 专门用于 FP64 模式的矩阵尺寸，为了适应消费级 GPU 较低的双精度算力（避免测试假死）。
- `--precisions` (默认: `fp16,bf16`): 需要测试的精度列表。可选: `fp64`, `fp32`, `tf32`, `fp16`, `bf16`, `fp8`。
- `--warmup-iters` (默认: 3): 每个工作负载窗口的预热轮数。
- `--burst-iters` (默认: 6): 每个工作负载窗口的正式压力突发轮数。
- `--validate-interval` (默认: 10): 旁路校验的间隔秒数。
- `--validate-size` (默认: 768): 旁路校验所用的固定矩阵尺寸。
- `--transpose-prob` (默认: 0.5): 随机转置 a/b 矩阵的概率，用于轻度扰动 kernel 执行路径。
- `--seed` (默认: 12345): 随机种子，保证结果一致性。
- `--disable-fp8`: 添加此标志可强制跳过 FP8 测试，即便当前环境支持被检测到可用。

## 输出日志概览

- 检测并输出基本的设备架构信息（架构版本，Compute Capability 等）。
- 实时持续显示迭代矩阵的尺寸、瞬间算力(TFLOPS)及验证测验进度。
- 在每个精度测试结束后，核心稳定性总结面板将给出会否出现报错的情况。
