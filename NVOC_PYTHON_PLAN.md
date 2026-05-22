A previous agent produced the plan below to accomplish the user's task. Implement the plan in a
  fresh context. Treat the plan as the source of user intent, re-read files as needed, and carry
  the work through implementation and verification.

  # Native Python Binding For `nvoc-core`

  ## Summary
  Add a new PyO3/maturin binding crate for `nvoc-core`, while keeping `auto-optimizer` workflows on
  the existing CLI. GUI/TUI will call native Python for GPU discovery, read-only state queries, and
  simple core set/reset operations, but continue using subprocess CLI for autoscan, VFP CSV import/
  export, fix_result, and other `auto-optimizer`-owned flows.

  References: PyO3 recommends `maturin` for Python extensions and `cdylib` crates; maturin supports
  mixed Rust/Python packages and submodule names.

  ## Key Changes
  - Add a Rust workspace member, e.g. `nvoc-python/`, with:
    - `Cargo.toml` depending on `nvoc-core`, `pyo3`, `serde`, `serde_json`.
    - `crate-type = ["cdylib"]`.
    - `pyproject.toml` using `maturin>=1.9.4,<2`.
    - import module `nvoc_core_native._native`, exposed through a small Python package
  `nvoc_core_native`.
  - Use `abi3-py38` so one wheel works for GUI Python `>=3.8` and TUI Python `>=3.11`.
  - Do not add PyO3 to `nvoc-core`; keep bindings in the wrapper crate so Rust consumers stay
  clean.
  - Add `nvoc-core` DTO/serialization helpers only if needed, but prefer implementing Python-facing
  DTO conversion in the binding crate.

  ## Python API
  Expose a small stable Python API, not a CLI argument emulator:

  - `discover_gpus(backends: str = "both") -> list[dict]`
    - Returns GPU index, core GPU id, hex id, backend availability, name/uuid when available.
  - `query_info(gpu: int | str, backends: str = "both") -> dict`
    - Normalized fields already used by GUI/TUI: architecture, GPU name, VFP ranges, NVAPI power/
  thermal defaults, NVML watt limits, legacy overvolt bounds.
  - `query_status(gpu: int | str, backends: str = "both") -> dict`
    - Normalized fields: clocks, voltage, temperature, power, VFP lock state, lock voltage.
  - `query_settings(gpu: int | str, backends: str = "both") -> dict`
    - Normalized `get`-style data: current offsets, supported P-states, lock bounds, fan range,
  power limits.
  - Mutating helpers for existing UI controls:
    - `set_clock_offset(gpu, backend, domain, value, pstate="P0")`
    - `set_power_limit(gpu, backend, value)`
    - `set_thermal_limit(gpu, celsius)`
    - `set_voltage_boost(gpu, value)`
    - `set_legacy_voltage_delta(gpu, uv, pstate="P0")`
    - `set_fan(gpu, backend, fan_id="all", policy="continuous", level=60)`
    - `reset_core_clocks(gpu, backend)`, `reset_mem_clocks(gpu, backend)`, `reset_vfp_lock(gpu)`,
  `reset_all(gpu, domain=None)`
  - Keep CLI-only for now:
    - `set vfp export/import/autoscan/autoscan_legacy/fix_result/pointwiseoc`
    - stress-test orchestration
    - any workflow requiring streamed CLI logs.

  ## GUI/TUI Migration
  - Add a Python adapter module in both apps, e.g. `core_api.py`, which imports `nvoc_core_native`
  and falls back to current CLI behavior if unavailable.
  - TUI:
    - Replace `CliService.list_gpus()` and `run_query(info/status/get)` with native calls.
    - Keep `run_action()` CLI-backed for auto-optimizer and long-running streamed actions.
    - Route overclock/fan/limit button actions through native helpers when they map directly to
  `nvoc-core`.
  - GUI:
    - Replace `_refresh_gpu_list`, `run_gpu_query_async(["info"|"status"|"get"])`, and simple
  overclock/fan/limit actions with native calls.
    - Preserve CLI path configuration because autoscan/VFP workflows still need it.
  - Keep existing parsers temporarily as CLI fallback and delete them only after native paths are
  stable.

  ## Tests
  - Rust:
    - Build binding crate with `cargo check --package nvoc-python`.
    - Add non-GPU unit tests for selector parsing, enum parsing, dict normalization, and error
  conversion.
    - Keep existing `cargo test --package nvoc-core --all-targets`.
  - Python:
    - Add fake/native-adapter tests for GUI/TUI to verify native results populate existing cache
  fields.
    - Preserve current CLI argument-construction tests for CLI-only workflows.
    - Add fallback tests proving missing `nvoc_core_native` still uses CLI.
  - GPU/manual:
    - On NVIDIA hardware, smoke-test `discover_gpus`, `query_info`, `query_status`,
  `query_settings`, then one supervised read/write operation per backend.

  ## Assumptions
  - Native binding is additive first; no immediate removal of CLI code.
  - `auto-optimizer` stays subprocess-driven until a later migration.
  - Python package name will be `nvoc-core-native`, import name `nvoc_core_native`.
  - Binding build artifacts are included in PyInstaller specs once native imports are wired.


