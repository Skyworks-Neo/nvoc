# FAQ

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Quick answers to common problems. For deep dives, see [[Auto-Optimizer-Guide]], [[Autoscan-Workflow]], [[Stress-Testing]], and [[Safety-and-Recovery]].

## Driver/version issues

- Verify the NVIDIA driver meets the backend requirement (NVAPI/NVML/OpenCL/CUDA runtime).
- Re-test with read-only info commands before write commands.
- Coverage varies by GPU generation and backend — cross-check [[GPU-Support-Matrix]].

## Permission problems

- Windows: run elevated where required.
- Linux: ensure required privileges for GPU control interfaces.
- In containers, writes additionally require root, `--cap-add SYS_ADMIN`, and (for NVAPI) a `libnvidia-api.so.1` bind mount — see [[Container-Usage]].

## Which stressor should I use?

- Prefer OpenCL only for compatibility checks or first-pass screening when a CUDA stack is unavailable. It is not high-pressure enough for final overclocking stability validation, and OpenCL-only passes can produce inflated autoscan / V-F curve results.
- Prefer CUDA-Rust (`cli-stressor-cuda-rs/`) when you need a native Rust-only integration path; it is the recommended CUDA stressor.
- The Python/PyTorch CUDA stressor `cli-stressor-cuda/` has been removed from the repository — use the Rust CUDA stressor instead.
- Revalidate any OpenCL-derived accepted result with the CUDA stressor or heavier real workloads before relying on it long term.

Details and workload design: [[Stress-Testing]].

## Autoscan instability or resets

- Reduce scan aggressiveness.
- Confirm cooling and power headroom.
- Use the reset workflow and retry from a conservative baseline.

See [[Autoscan-Workflow]] for the full procedure and [[Safety-and-Recovery]] for rollback controls.

---

*Maintained from: `docs/wiki/FAQ.md`, `auto-optimizer/README.md`, stressor READMEs, troubleshooting issues/notes.*

<a id="chinese"></a>

## 中文

常见问题的快速解答。深入内容见 [[Auto-Optimizer-Guide]]、[[Autoscan-Workflow]]、[[Stress-Testing]] 与 [[Safety-and-Recovery]]。

## 驱动/版本问题

- 确认 NVIDIA 驱动满足后端要求（NVAPI/NVML/OpenCL/CUDA runtime）。
- 在执行写入命令之前，先用只读 info 命令重新验证。
- 覆盖范围因 GPU 世代和后端而异 —— 请对照 [[GPU-Support-Matrix]] 核查。

## 权限问题

- Windows：在需要提升权限的操作上以管理员身份运行。
- Linux：确保具备 GPU 控制接口所需的特权。
- 容器内写入还需要 root、`--cap-add SYS_ADMIN`，NVAPI 写入还需要 bind mount `libnvidia-api.so.1` —— 见 [[Container-Usage]]。

## 应该使用哪个压力测试器？

- 仅当 CUDA 栈不可用时，才用 OpenCL 做兼容性检查或首轮筛选。它的压力不足以作为最终的超频稳定性验证，且仅通过 OpenCL 测试可能得到偏乐观的 autoscan / V-F curve 结果。
- 需要纯 Rust 的集成路径时，优先选择 CUDA-Rust（`cli-stressor-cuda-rs/`）；它是推荐的 CUDA 压力测试器。
- Python/PyTorch CUDA 压力测试器 `cli-stressor-cuda/` 已从仓库移除 —— 请改用 Rust CUDA 压力测试器。
- 任何由 OpenCL 得出的可接受结果，长期依赖前必须用 CUDA 压力测试器或更重的真实负载重新验证。

负载设计与细节：[[Stress-Testing]]。

## Autoscan 不稳定或触发重置

- 降低扫描激进程度。
- 确认散热与功耗余量。
- 使用重置（reset）工作流，并从保守基线重试。

完整流程见 [[Autoscan-Workflow]]，回滚手段见 [[Safety-and-Recovery]]。

---

*维护来源：`docs/wiki/FAQ.md`、`auto-optimizer/README.md`、各压力测试器 README、故障排查 issue 与笔记。*
