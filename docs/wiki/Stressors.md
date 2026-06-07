# Stressors

| Stressor | Stack | Platforms | Best for |
|---|---|---|---|
| CUDA (Rust) | Rust + CUDA toolchain | CUDA-capable dev/runtime setups | Native Rust integration and lower Python dependency |
| OpenCL | OpenCL runtime | Broader GPU runtime coverage | Lightweight checks when CUDA stack is unavailable |

Selection rule: choose the stressor that matches deployment constraints first (driver/runtime), then optimize for workflow integration.

**Warning:** the OpenCL stressor is not high-pressure enough for final overclocking stability validation. OpenCL-only passes can report higher autoscan / V-F curve results than the GPU can actually sustain, which may cause driver resets, system instability, data corruption, or hardware failure if applied long term. Use OpenCL as a compatibility or first-pass check, then revalidate accepted results with the CUDA stressor or heavier real workloads.

---

*Maintained from: `cli-stressor-cuda-rs/`, `cli-stressor-opencl/`, `auto-optimizer/README.md`.*
