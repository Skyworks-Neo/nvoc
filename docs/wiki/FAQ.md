# FAQ

## Driver/version issues

- Verify NVIDIA driver meets backend requirement (NVAPI/NVML/OpenCL/CUDA runtime).
- Re-test with read-only info commands before write commands.

## Permission problems

- Windows: run elevated where required.
- Linux: ensure required privileges for GPU control interfaces.

## Which stressor should I use?

- Prefer CUDA-Python when PyTorch CUDA is already installed.
- Prefer OpenCL when CUDA stack is unavailable.
- Prefer CUDA-Rust when you need a native Rust-only integration path.

## Autoscan instability or resets

- Reduce scan aggressiveness.
- Confirm cooling and power headroom.
- Use reset workflow and retry from conservative baseline.

---

*Maintained from: `auto-optimizer/README.md`, stressor READMEs, troubleshooting issues/notes.*
