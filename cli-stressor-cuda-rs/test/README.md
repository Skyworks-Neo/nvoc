# cli-stressor-cuda-rs — multi-GPU reference test

A manual hardware test that verifies the CUDA stressor runs **correctly and
without per-card throughput loss when one instance is run per GPU concurrently**
(the way an autoscan fleet exercises many cards), and records reference
throughput / CPU / power numbers.

This is **not** a CI unit test — it needs real NVIDIA GPUs and a CUDA-feature
build of the stressor.

## What it checks

`multi_gpu_stress.ps1` runs a single-card baseline on the first selected GPU,
then runs every selected GPU concurrently (one stressor process pinned per card
via `--gpu-index`), and reports per-card TFLOPS, validation failures, CPU load
and total GPU power.

**PASS criteria** (script exits non-zero otherwise):
1. every instance exits `0` with `val_fail == 0` (no silent corruption), and
2. concurrent per-card throughput stays ≥ `-MinConcurrentRatio` of the solo
   baseline (default `0.85`) — i.e. the parallel host-side input generation does
   not oversubscribe the CPU and starve concurrent instances.

## Prerequisites

1. Build with the CUDA feature:
   ```
   cargo build --release -p cli-stressor-cuda-rs --features cuda
   ```
2. Make the CUDA 12.x runtime DLLs discoverable (next to the exe, on `PATH`, or
   via `-DllDir`): `nvrtc64_120_0.dll`, `nvrtc-builtins64_129.dll`,
   `cublas64_12.dll`, `cublasLt64_12.dll`, `cudart64_12.dll`.

## Usage

```powershell
# all detected cards, default config
pwsh ./multi_gpu_stress.ps1

# specific cards, longer run, explicit DLL dir
pwsh ./multi_gpu_stress.ps1 -Gpus 0,1,2,3 -Duration 60 -DllDir ..\..\cuda-runtime
```

Key knobs: `-Gpus`, `-Duration`, `-Precisions`, `-MatrixSizes`, `-StreamMode`,
`-BurstIters`, `-ValidateInterval`, `-MinConcurrentRatio`.

## Reference results

Measured on an 8-GPU host (6× GTX 1080 + 2× GTX 1080 Ti, Pascal SM 6.1, 64-core
Xeon), config `fp32 / matrix 4096 / single-stream / burst 16`:

| Scenario | Per-card TFLOPS | val_fail | CPU load | Notes |
|---|---|---|---|---|
| 1 card (GTX 1080, solo) | 8.49 | 0 | — | baseline |
| 4 cards concurrent | GPU0 8.40 (**99% of solo**); 1080 Ti ≈ 11.3 | 0 (all) | 28% | total ≈ 39.5 TFLOPS |
| 8 cards concurrent | 1080 ≈ 8.3–8.6, 1080 Ti ≈ 11.2–11.3 | 0 (all) | 35% | total ≈ 72.3 TFLOPS, ≈ 512 W |

Takeaways used as the standard reference:
- **No oversubscription:** concurrent per-card throughput holds at ~99% of solo
  through 8 cards; CPU stays ≤ ~35% of 64 cores.
- **Correctness:** every instance validates clean (`val_fail=0`) at 1/4/8 cards.
- Power figures are single 1 Hz `nvidia-smi` snapshots (per-card util bounces by
  sample timing) — indicative, not a precise sustained number. The pass gate is
  correctness + no throughput collapse, not an absolute wattage.

> cuBLAS GEMM on Pascal tops out ~50–60% TDP per card regardless of config — see
> the main crate README and issue #142 for the headless-Vulkan / power-virus
> direction if you need near-TDP per-card draw.
