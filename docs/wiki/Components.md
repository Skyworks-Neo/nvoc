# Components

## Product Components

| Component | Path | Purpose |
|---|---|---|
| NVOC-AUTO-OPTIMIZER | `auto-optimizer/` | Rust CLI core for GPU discovery/control, autoscan, reset, V-F operations, and result handling. |
| NVOC-STRESSOR CUDA (Python) | `cli-stressor-cuda/` | CUDA/PyTorch-based stress workload used by autoscan and standalone checks. |
| NVOC-STRESSOR CUDA (Rust) | `cli-stressor-cuda-rs/` | Rust CUDA stressor variant for native Rust pipeline usage. |
| NVOC-STRESSOR OpenCL | `cli-stressor-opencl/` | Lightweight OpenCL stress workload for non-CUDA stacks. |
| NVOC-GUI | `gui/` | Desktop GUI for dashboard, autoscan, overclock, and live runner views. |
| NVOC-TUI | `tui/` | Textual-based terminal frontend for headless or SSH workflows. |
| NVOC-SRV | `srv/` | Service wrapper + localhost HTTP control endpoint. |

## Library / Shared Components

| Library | Path | Purpose |
|---|---|---|
| nvoc-core | `nvoc-core/` | Backend abstraction over NVAPI/NVML and common types/errors/operations. |
| pynvoc | Python package outputs | Python-facing bindings/adapters used by frontends to invoke control paths. |

---

*Maintained from: `README.md`, `nvoc-core/src/lib.rs`, workspace `Cargo.toml`.*
