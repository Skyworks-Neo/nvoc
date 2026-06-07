# FAQ

## Driver/version issues

- Verify NVIDIA driver meets backend requirement (NVAPI/NVML/OpenCL/CUDA runtime).
- Re-test with read-only info commands before write commands.

## Permission problems

- Windows: run elevated where required.
- Linux: ensure required privileges for GPU control interfaces.

## Which stressor should I use?

- Prefer OpenCL only for compatibility checks or first-pass screening when CUDA stack is unavailable. It is not high-pressure enough for final overclocking stability validation, and OpenCL-only passes can produce inflated autoscan / V-F curve results.
- Prefer CUDA-Rust when you need a native Rust-only integration path.
- Revalidate any OpenCL-derived accepted result with the CUDA stressor or heavier real workloads before relying on it long term.

## Autoscan instability or resets

- Reduce scan aggressiveness.
- Confirm cooling and power headroom.
- Use reset workflow and retry from conservative baseline.

---

*Maintained from: `auto-optimizer/README.md`, stressor READMEs, troubleshooting issues/notes.*
