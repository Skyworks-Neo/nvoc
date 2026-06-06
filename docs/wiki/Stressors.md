# Stressors

| Stressor | Stack | Platforms | Best for |
|---|---|---|---|
| CUDA (Rust) | Rust + CUDA toolchain | CUDA-capable dev/runtime setups | Native Rust integration and lower Python dependency |
| OpenCL | OpenCL runtime | Broader GPU runtime coverage | Lightweight checks when CUDA stack is unavailable |

Selection rule: choose the stressor that matches deployment constraints first (driver/runtime), then optimize for workflow integration.

---

*Maintained from: `cli-stressor-cuda-rs/`, `cli-stressor-opencl/`, `auto-optimizer/README.md`.*
