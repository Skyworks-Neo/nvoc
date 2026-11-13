# Stressors

| Stressor | Stack | Platforms | Best for |
|---|---|---|---|
| CUDA (Rust) | Rust + CUDA toolchain | CUDA-capable dev/runtime setups | Native Rust integration and lower Python dependency |
| OpenCL | OpenCL runtime | Broader GPU runtime coverage | Lightweight checks when CUDA stack is unavailable |

Selection rule: choose the stressor that matches deployment constraints first (driver/runtime), then optimize for workflow integration.

**Warning:** the OpenCL stressor is not high-pressure enough for final overclocking stability validation. OpenCL-only passes can report higher autoscan / V-F curve results than the GPU can actually sustain, which may cause driver resets, system instability, data corruption, or hardware failure if applied long term. Use OpenCL as a compatibility or first-pass check, then revalidate accepted results with the CUDA stressor or heavier real workloads.

## Validation & load-shape features (CUDA/Vulkan stressor)

Borrowed from NVIDIA's production MODS toolchain (see `docs/reverse-engineering/mods/` and
`reverse/mods_analysis/A6_stress_tests.md` for the reverse-engineering basis):

| Feature | Flags | What it does |
|---|---|---|
| Duty-cycle power pulsing | `--pulse-mode fixed\|sweep`, `--pulse-duty`, `--pulse-low-hz`, `--pulse-high-hz`, `--pulse-octaves`, `--pulse-spin-idle` | Alternates render bursts with idle windows; the 50 Hz floor matches the RM power-capping period (20 ms), the sweep probes VRM transients toward the kHz range. Measured duty/Hz are logged next to FPS. |
| L2-sized verification working set | `--mem-mode auto\|resident\|stream` | The resident pattern block is sized from the reported L2 cache (0.75×L2, 8–128 MiB) so pattern checks stay cache-resident while bursts thrash VRAM; `stream` enables the address-walk slab that sweeps pages beyond L2. |
| High-priority verification lane | default | The resident-check tick runs on a highest-priority CUDA stream so the SDC cadence survives a saturated foreground; `--vulkan-queue-priority-low` optionally drops the render queue instead. |
| SDC event stream | `--sdc-log <PATH>` | JSONL: structured fault records (op, seed, index, expected, actual) with NVML clock/temperature/power sampled at detection time, plus a 500 ms telemetry cadence — the data behind SDC-rate-vs-frequency mapping. |
| Video-engine stress | `--video`, `--video-only`, `--video-validate`, `--video-width/--video-height` | NVENC→NVDEC closed loop without ffmpeg: synthetic frames are H.264-encoded (all-I), decoded through cuvid, and CRC-validated against the source pattern. Runtime-loads the driver-shipped encode/decode libraries. |

Bare `cli-stressor-rs` (no arguments) prints a grouped help by engine family (CUDA / Vulkan /
Video / Common) in the same style as `nvoc-cli`.

---

*Maintained from: `cli-stressor-rs/`, `cli-stressor-opencl/`, `auto-optimizer/README.md`.*
