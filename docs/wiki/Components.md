# Components

## Product Components

| Component | Path | Purpose |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | `auto-optimizer/` | Rust CLI for autoscan, V-F curve import/export, result fixing, and retained VFP reset workflows. |
| NVOC-CLI | `cli/` | Focused Rust wrapper over `nvoc-core` for GPU discovery, status, general resets, and NVAPI/NVML setting writes. |
| NVOC-STRESSOR CUDA (Rust) | `cli-stressor-cuda-rs/` | Rust CUDA stressor variant for native Rust pipeline usage. |
| NVOC-STRESSOR OpenCL | `cli-stressor-opencl/` | Lightweight OpenCL first-pass workload for non-CUDA stacks; not a final overclocking stability gate. |
| NVOC-GUI | `gui/` | Desktop GUI for dashboard, autoscan, overclock, and live runner views. |
| NVOC-TUI | `tui/` | Textual-based terminal frontend for headless or SSH workflows. |
| NVOC-SRV | `srv/` | Service wrapper + localhost HTTP control endpoint. |

## Library / Shared Components

| Library | Path | Purpose |
|---|---|---|
| nvoc-core | `core/` | Backend abstraction over NVAPI/NVML and common types/errors/operations. |
| pynvoc | Python package outputs | Python-facing bindings/adapters used by frontends to invoke control paths. |

---

*Maintained from: `README.md`, `core/src/lib.rs`, workspace `Cargo.toml`.*
