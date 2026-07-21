# nvoc-auto-optimizer

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

[中文](./README.md) | [English](./README-en.md)

This project is released under the [Apache License 2.0](LICENSE).

> **NVIDIA GPU VF Curve Auto-Overclocking Optimizer**  
> Written in Rust, it controls the GPU through NVAPI / NVML interfaces and works with `cli-stressor` pressure testing to perform point-by-point Voltage-Frequency (V-F Curve) automatic scanning, finding the stable overclocking upper limit for each voltage point of the NVIDIA GPU and generating an optimized curve.

## nvoc-auto-optimizer—A professional N-card overclocking tool for developers who truly understand overclocking

## Table of Contents

- [Background and Principles](#background-and-principles)
- [Supported GPU Generations](#supported-gpu-generations)
- [Compatibility Overview (Interface × GPU Generation / Basic Functions)](#compatibility-overview-interface--gpu-generation--basic-functions)
- [Dependencies and Environment Requirements](#dependencies-and-environment-requirements)
- [Directory Structure](#directory-structure)
- [Quick Start](#quick-start)
- [Command Reference](#command-reference)
  - [Global Parameters](#global-parameters)
  - [Commands](#commands)
    - [export-vfp](#export-vfp)
    - [import-vfp](#import-vfp)
    - [sync-vfp-memory-pstate](#sync-vfp-memory-pstate)
- [Detailed Scanning Process](#detailed-scanning-process)
  - [Phase 0: Preparation](#phase-0-preparation)
  - [Phase 1: Voltage Range Probing](#phase-1-voltage-range-probing)
  - [Phase 2: Point-by-point Core Frequency Scanning](#phase-2-point-by-point-core-frequency-scanning)
  - [Phase 3: Video Memory Frequency Scanning (Optional)](#phase-3-video-memory-frequency-scanning-optional)
  - [Phase 4: fix_result Post-processing](#phase-4-fix_result-post-processing)
  - [Phase 5: Import and Export Final Curve](#phase-5-import-and-export-final-curve)
- [Ultrafast Mode](#ultrafast-mode)
- [Crash Recovery Mechanism](#crash-recovery-mechanism)
- [Breakpoint Resumption](#breakpoint-resumption)
- [Legacy GPU Mode (Maxwell / Pascal)](#legacy-gpu-mode-maxwell--pascal)
- [Working Files Description](#working-files-description)
- [Test Environment Suggestions](#test-environment-suggestions)
- [Build from Source](#build-from-source)
- [Disclaimer](#disclaimer)

---

## Background and Principles

### What is the V-F Curve?

Starting from Pascal (10 series), NVIDIA introduced **GPU Boost 3.0**, the core of which is a Voltage-Frequency (VFP) lookup table: the GPU will look up the corresponding target frequency in the table based on the current core voltage and track it in real-time during operation.  
The factory default curve is a conservative value calibrated by NVIDIA for the worst silicon with a generous margin. Actual stability limits vary greatly between different wafer batches and silicon individuals, and the true usable frequency of high-quality silicon is often much higher than the factory value.

### The Essence of Overclocking

Overclocking is essentially applying a positive frequency offset (`KilohertzDelta`) to each voltage point in the VFP table to increase the target frequency corresponding to that voltage point. When the offset is too large, internal timing violations occur, and the GPU triggers TDR (Timeout Detection and Recovery) or crashes directly.  
The goal of this tool is: **For each voltage point on the curve, find the maximum frequency offset value that can stably pass the pressure test at that voltage**.

### Pressure Test Load: cli-stressor

`cli-stressor` is used to provide stability pressure at a given voltage/frequency. The criterion uses the **process return code**: returning `0` is considered passing, and non-`0` is considered failing.

> **Warning:** do not use the OpenCL stressor as the final acceptance gate for autoscan results. It is not high-pressure enough, so OpenCL-only passes can make the scan accept frequency offsets higher than the hardware can actually sustain. Applying those inflated results can cause driver resets, system instability, data corruption, or hardware failure; treat them as provisional and revalidate with the CUDA stressor or heavier real workloads.

### Maxwell / 9 Series Legacy Mode

Maxwell (GM code name, 9xx series) and earlier GPUs do not support point-by-point V-F curve writing and can only apply a global frequency offset to the P0 state through `SetPstates20`. This tool uses the `autoscan-vfp-legacy` flow for such GPUs, scanning only a single global offset value.

---

## Supported GPU Generations

| Generation                   | Code Name Prefix | Mode                                               | Description                                                           |
|------------------------------|------------------|----------------------------------------------------|-----------------------------------------------------------------------|
| RTX 50 Series (Blackwell)    | `GB`             | VF Curve                                           | Defaults to aggressive BSOD recovery; supports Max-Q step calibration |
| RTX 40 Series (Ada Lovelace) | `AD`             | VF Curve                                           | —                                                                     |
| RTX 30 Series (Ampere)       | `GA`             | VF Curve                                           | —                                                                     |
| RTX 20 Series (Turing TU10x) | `TU10`           | VF Curve (Small diff between light and heavy load) | —                                                                     |
| GTX 16 Series (Turing TU11x) | `TU11`           | VF Curve (Small diff between light and heavy load) | —                                                                     |
| GTX 10 Series (Pascal)       | `GP1`            | VFP Curve (79 points)                              | Small diff between light and heavy load; fix_result still recommended |
| GTX 9 Series (Maxwell)       | `GM`             | **Legacy Global Offset**                           | Point-by-point VFP not supported; scanned via `autoscan-vfp-legacy`       |
| Volta Compute Cards          | `GV`             | Legacy                                             | Same as above                                                         |

> **Mobile GPUs** (name contains `Laptop`) cannot modify TDP / Temperature Wall / VDDQ boost; the tool will automatically skip these settings when detected.

---

## Compatibility Overview (Interface × GPU Generation / Basic Functions)

The backend/function compatibility matrix is maintained in [NVOC-CLI](../cli/README.md#compatibility-overview-interface--gpu-generation--basic-functions), because those rows describe the shared NVAPI/NVML control surface exposed by `nvoc-cli`.

For autoscan-specific GPU generation support, see [Supported GPU Generations](#supported-gpu-generations).

## Dependencies and Environment Requirements

### Runtime Dependencies

| Dependency                                                       | Description                                                                                                                                                                            |
|------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Windows 10/11 or any Linux distribution                          | The tool is invoked via NVAPI/NVML and supports Windows 10/11 and *any Linux distribution using nvidia-open-dkms* (theoretically, ArchLinux + KDE, Ubuntu 22.04, Debian 12/13 tested). |
| NVIDIA Driver (≥ 537, nvidia-open-dkms or proprietary for Linux) | Must support NVAPI/NVML interface for target GPU. Version 395 known not to work; hard to support Kepler and earlier GPUs with old drivers.                                             |
| CUDA runtime libraries                                           | Required by the bundled `cli-stressor-cuda-rs` worker. The optimizer launches it as an isolated subprocess.                                                                            |
| Administrator/Sudo privileges                                    | OC parameter writing requires admin privileges; most reads do not.                                                                                                                     |

### Integrated workflow

Run `nvoc-auto-optimizer optimize --gpu <id>`. The optimizer handles workspace creation,
reset, initial export, resumable autoscan, result fixing/import, and final export itself.
Use `--mode ultrafast` or `--mode legacy`, and `--fresh` to discard resumable scan state.

### Build Dependencies (Only if building from source)

- Rust toolchain
- Build tools for target architecture
- The default `stressor-bundled` feature builds CUDA and Vulkan support into the optimizer.
- `stressor-external` is an optional fallback that launches a standalone
  `cli-stressor-cuda-rs` executable with structured arguments, without a shell wrapper.

---

## Directory Structure

```
auto-optimizer/
├── src/
│   ├── main.rs               # Entry point, command routing
│   ├── arg_help.rs           # clap command line argument definitions
│   ├── basic_func.rs         # GPU generation detection, resolution tools, handle_info/list/status/get/reset
│   ├── nvidia_gpu_type.rs     # GPU generation identification and parameter typing
│   ├── oc_get_set_function_nvapi.rs # NVAPI: VFP lock/reset, cooler, voltage/frequency settings
│   ├── oc_get_set_function_nvml.rs  # NVML: Power wall, clock lock, P-State lock
│   ├── oc_profile_function.rs# VFP export/import, fix_result, autoscan auxiliary functions
│   ├── oc_scanner.rs         # autoscan_gpuboostv3 / legacy scanner core scanning loop
│   ├── autoscan_config.rs    # Scan parameter struct (unified ArgMatches parsing)
│   ├── types.rs              # OutputFormat, ResetSettings, VfpResetDomain enums
│   ├── conv.rs               # Enum string conversions
│   ├── error.rs              # Unified error types
│   ├── human.rs              # Human-readable output formatting
│   └── lib.rs
├── ws/                       # Created automatically at runtime, stores intermediate scan files
│   ├── vfp.jsonl             # Structured scan log (resumption depends on this file)
│   ├── vfp-init.csv          # Factory original curve exported before first scan
│   ├── vfp-tem.csv           # Real-time temporary results saved per point during autoscan
│   └── vfp.csv / vfp-final.csv  # fix_result post-processing results / final exported confirmation file
├── test/
│   └── test.bat              # Pressure test wrapper script calling cli-stressor
├── GpuTdrRecovery.reg        # Registry related to TDR recovery
├── recover.bat               # Manual recovery script
└── Cargo.toml
```

---

## Quick Start

### Standard Scan (Recommended, for RTX 20 series and above)

Run as **Administrator**:

```bat
nvoc-auto-optimizer optimize
```

The script will automatically:
1. Detect and list GPUs, prompting for target GPU ID
2. Reset VFP curve and unlock voltage lock
3. Export and save factory original curve (`ws\vfp-init.csv`)
4. Perform autoscan for all voltage points
5. Perform fix_result light/heavy load compensation post-processing
6. Import the optimal curve into the GPU and export final file (`ws\vfp-final.csv`)

### Ultrafast Scan (Recommended for saving time)

```bat
nvoc-auto-optimizer optimize --mode ultrafast
```

Only scans 4 key voltage points, other points are linearly interpolated, significantly speed up. See [Ultrafast Mode](#ultrafast-mode).

### Legacy GPU Scan (GTX 9 series and earlier / only Maxwell tested so far)

```bat
nvoc-auto-optimizer optimize --mode legacy
```

Uses global P0 frequency offset scanning, no VFP curve writing.

### Re-scan (Clear history)

To start from scratch (discarding breakpoint resume state):

```bat
nvoc-auto-optimizer optimize --fresh
```

Passing argument `1` will clear `ws\vfp.jsonl` and `ws\vfp-tem.csv`.

---

## Command Reference

`nvoc-auto-optimizer` exposes only VFP optimizer workflows. Run mutating commands with administrator/root privileges:

```text
nvoc-auto-optimizer.exe [--gpu GPU_ID] [--no-color] <command> [command options]
```

For GPU discovery, status, general overclock settings, fan control, power limits, locks, and generic resets, use `nvoc-cli` instead, for example `nvoc-cli list-gpus`, `nvoc-cli get-status`, `nvoc-cli set-core-offset-mhz`, or `nvoc-cli reset-vfp-lock`.

### Global Parameters

| Parameter | Shorthand | Description |
|-----------|-----------|-------------|
| `--gpu <GPU_ID>` | `-g` | Target GPU. Accepts decimal or hex and can be specified multiple times. |
| `--no-color` | - | Disable ANSI color output. |

### Commands

| Command | Description |
|---------|-------------|
| `export-vfp [OPTIONS] [OUTPUT]` | Export current VFP curve as CSV. |
| `export-vfp-log [OPTIONS]` | Export VFP points parsed from an autoscan JSONL log. |
| `import-vfp [OPTIONS] [INPUT]` | Import a modified VFP curve from CSV. |
| `sync-vfp-memory-pstate` | Sync the second-highest adjustable memory VFP stage to P0 memory frequency. |
| `fix-vfp-result [OPTIONS]` | Post-process autoscan results. |
| `autoscan-vfp [OPTIONS]` | Auto-scan a VFP curve. |
| `autoscan-vfp-legacy [OPTIONS]` | Auto-scan legacy GPUs using a global P-State OC offset. |
| `reset-vfp --vfp-domain all\|core\|memory` | Reset VFP curve deltas. |

---

#### export-vfp

Export current VFP curve as CSV.

```bat
nvoc-auto-optimizer.exe export-vfp .\ws\vfp-init.csv
nvoc-auto-optimizer.exe export-vfp --quick .\ws\vfp-quick.csv
```

Defaults to the Graphics (core) curve; use domain flags to export other VFP tables.

| Parameter   | Shorthand | Description                                             |
|-------------|-----------|---------------------------------------------------------|
| `<OUTPUT>`  | —         | Output path (`-` for stdout)                            |
| `--quick`   | `-q`      | Skip dynamic load measurement, export static curve only |
| `--nocheck` | `-n`      | Skip plausibility check for dynamic results             |
| `--memory`  | —         | Export Memory domain VFP curve                          |
| `--processor` | —       | Export Processor domain VFP curve                       |
| `--video`   | —         | Export Video domain VFP curve                           |
| `--undefined` | —       | Export Undefined domain VFP curve                       |

**CSV Columns (Full Dynamic Export):**

| Column Name              | Description                                      |
|--------------------------|--------------------------------------------------|
| `voltage`                | Voltage point (μV)                               |
| `frequency`              | Current set frequency (kHz)                      |
| `delta`                  | Offset relative to factory (kHz)                 |
| `default_frequency`      | Factory static default frequency (kHz)           |
| `default_frequency_load` | Measured frequency under dynamic load (kHz)      |
| `margin`                 | Difference between load and static default (kHz) |
| `margin_bin`             | `margin` converted to min step units (integer)   |

> Dynamic export will run the pressure test load for ~45s before reading, ensure the default test script is executable.

---

#### import-vfp

Write modified curve from CSV to GPU.

```bat
nvoc-auto-optimizer.exe import-vfp .\ws\vfp.csv
```

Defaults to the Graphics (core) curve; Memory domain import aligns by point index (edit an export file), other domains match by voltage.

| Parameter | Shorthand | Description                |
|-----------|-----------|----------------------------|
| `<INPUT>` | —         | Input path (`-` for stdin) |
| `--memory` | —        | Import Memory domain VFP curve |
| `--processor` | —     | Import Processor domain VFP curve |
| `--video` | —         | Import Video domain VFP curve   |
| `--undefined` | —     | Import Undefined domain VFP curve |

---

#### sync-vfp-memory-pstate

Sync the second-highest memory VFP stage to the P0 frequency (useful for Windows P2/P3 alignment).

```bat
nvoc-auto-optimizer.exe sync-vfp-memory-pstate
```

---

## Detailed Scanning Process

### Phase 0: Preparation

`nvoc-auto-optimizer optimize` sequentially executes before calling `autoscan-vfp`:

1. `nvoc-cli get-info`: Print GPU info and identify generation.
2. `nvoc-cli reset-pstate-clock-offsets`: Zero P-State frequency offsets to ensure a clean starting point.
3. `reset-vfp` (or `reset-vfp --vfp-domain all`): Zero all VFP curve point offsets.
   - Use `reset-vfp --vfp-domain core` for core OC only.
   - Use `reset-vfp --vfp-domain memory` for memory OC only.
4. `nvoc-cli reset-vfp-lock`: Release voltage/frequency locks.
5. `export-vfp .\ws\vfp-init.csv` (if it doesn't exist): Save factory original curve as reference.
6. Add firewall rules for pressure test executable (optional, to avoid network activity affecting the test).

### Phase 1: Voltage Range Probing

`autoscan` starts by calling `handle_test_voltage_limits`. Starting from generation-specific preset points, it probes the actual usable voltage range of the GPU by advancing the voltage lock (`handle_lock_vfp`) step by step:

- **Upper Bound Probe**: Advance upward point by point from the preset upper point until locking fails (the curve reaches a flat region or goes out of range).
- **Lower Bound Probe**: Advance downward from the preset lower point to find the minimum usable voltage point.

Results are recorded in `vfp.jsonl` as structured `voltage_range` events and skipped on resume by reading the JSONL log directly.

### Phase 2: Point-by-point Core Frequency Scanning

For the range `[lower_voltage_point, upper_voltage_point]`, scan every 3 points in standard mode, or use 4 key points in ultrafast mode. At each voltage point, run a two-stage binary search:

**Short Test**: use exponential stepping (`2^n × minimum step`) to quickly converge close to the stable upper frequency limit.

**Long Test**: based on the value found in the short test, perform single-step endurance validation to confirm true stability.

During each round of testing, the tool periodically applies **frequency fluctuations**: it raises or lowers the current set frequency periodically to simulate dynamic workload changes and prevent the GPU from passing at a static frequency but crashing when switching in practice. A watchdog also monitors whether the stress-test process is still alive to prevent the scan from hanging.

If thermal / power throttling is detected (`thrm_or_pwr_limit_flag`), ~~the test resolution will be lowered automatically to reduce GPU load~~ (TODO since the load was changed to CLI-based testing), ensuring the result reflects overclocking capability rather than TDP bottlenecks.

After each voltage point finishes, the result is appended to `vfp-tem.csv` in real time and recorded in `vfp.jsonl` (resume scanning supported).

### Phase 3: Video Memory Frequency Scanning (Optional)

Enabled with `-m`. After core scan finishes, the tool locks at the highest voltage point and scans the memory OC ceiling using a similar binary-search approach.

### Phase 4: fix_result Post-processing

`nvoc-auto-optimizer optimize` automatically calls after `autoscan`:

```bat
nvoc-auto-optimizer.exe fix-vfp-result -m 1
```

**Principle**: Since Pascal (10 series), NVIDIA V-F curves exhibit a small difference between light load and heavy load — similar to CPU Load-Line Calibration (LLC). If the stable maximum frequency found under full load is written back without correction, instability may occur when the operating point shifts during load transitions.

fix_result uses `vfp-init.csv` dynamic measurement's `margin_bin` (difference between loaded frequency and static default frequency, converted to step bins) to conservatively adjust each point from `autoscan`:
- `margin_bin > 5`: reduce by `(5 + minus_bin)` steps.
- `|margin_bin| < 2`: reduce by `(1 + minus_bin)` steps.
- otherwise: reduce by `(|margin_bin| + 1 + minus_bin)` steps.

In ultrafast mode, the missing voltage points between the 4 key points are first filled in by linear interpolation.

### Phase 5: Import and Export Final Curve

```bat
nvoc-auto-optimizer.exe import-vfp .\ws\vfp.csv
nvoc-auto-optimizer.exe export-vfp .\ws\vfp-final.csv
```

Writes corrected curve to GPU and saves final snapshot.

---

## Ultrafast Mode

The core assumption of ultrafast mode is: **under the factory-calibrated curve, overclock headroom decreases monotonically as voltage rises**. Therefore, only a few points need to be tested and the rest can be linearly interpolated, greatly reducing scan time.

**Key Point Selection (for RTX 50 series GPUs with a Max-Q step):**

The factory curve of NVIDIA 50 series GPUs contains a "Max-Q step" — at a certain mid-voltage point, frequency jumps sharply, forming a step. The step location differs between light load and heavy load. To handle this correctly, ultrafast mode detects these 4 key points from `vfp-init.csv` (which includes a dynamic load column):

| Key Point | Detection Basis                                                              |
|-----------|------------------------------------------------------------------------------|
| p1        | Static curve frequency max jump (step lower bound, light load)               |
| p2        | Loaded curve frequency max jump (step lower bound, heavy load)               |
| p3        | First point where `margin_bin` changes from 0 to negative (step upper bound) |
| p4        | Point with most negative `margin_bin` (strongest heavy-load suppression)     |

The scan tests these 4 points once each. In fix_result, linear interpolation fills gaps between segments, and an extra safety reduction is applied to overclock headroom in the stepped region.

**GPUs without a Max-Q step** (30 series, 40 series, etc.): p1–p4 evenly distributed across scan range and interpolated linearly.

---

## Crash Recovery Mechanism

Crashes are expected during overclocking scans. Built-in recovery strategies:

### Traditional

Wait for Windows TDR (Timeout Detection and Recovery) to automatically detect GPU hang and recover driver. Suitable for most GPUs.

### Aggressive (Default for 50 Series)

Some RTX 50 series drivers fail to auto-recover after TDR, causing permanent system freeze.
Aggressive mode actively triggers a kernel crash (BSOD) to force reboot, continuing from breakpoint via Windows startup item after reboot.

> **Note**: Intentional BSOD is a normal part of flow control, **not hardware damage**. Auto-resumption will follow reboot.

Override via `-b`:
```bat
autoscan-vfp -b traditional    # force traditional
autoscan-vfp -b aggressive     # force aggressive
```

---

## Breakpoint Resumption

Structured results are appended to `ws\vfp.jsonl` after each voltage point finishes. Next run, the tool parses:

- `voltage_range`: skip probing.
- `point_finished`: continue from the next point.
- `test_result`: restore binary search convergence directly.
- `scan_mode` / `key_points`: restore normal or ultrafast mode state.

If the JSONL file contains corrupt records, the tool prints the parse errors, recovers valid records where possible, and asks before resuming from recovered state.

Whether exit, manual interrupt, or crash, **just rerun `nvoc-auto-optimizer optimize` to resume**.

To start over, run `nvoc-auto-optimizer optimize --fresh` to clear log.

---

## Legacy GPU Mode (Maxwell / Pascal)

NVAPI for GTX 9 series (Maxwell) lacks point-by-point VFP writing; uses global P0 offset via `autoscan-vfp-legacy`:

1. Write global offset through `set_pstates` (`ClockDomain::Graphics`, `PState::P0`).
2. Scan max stable offset with binary search + endurance test.
3. Not supported: ultrafast interpolation, memory segment scans, per-point fix_result.

For voltage control, Maxwell uses `SetPstates20` `baseVoltages` (`--voltage-delta` in μV) instead of VoltRails boost.

Legacy OverVolt often fails in practice; VBIOS editing often recommended for pre-900 series for max performance.

---

## Working Files Description

| File               | Purpose               | Notes                                                |
|--------------------|-----------------------|------------------------------------------------------|
| `ws\vfp.jsonl`     | Structured scan log   | Basis for resumption; deleting restarts from scratch |
| `ws\vfp-init.csv`  | Factory snapshot      | Captured before scan; ref for fix_result             |
| `ws\vfp-tem.csv`   | autoscan temp results | Written in real-time; input to fix_result            |
| `ws\vfp.csv`       | fix_result output     | Final compensated curve; input for import            |
| `ws\vfp-final.csv` | Final snapshot        | Exported after import for backup                     |

---

## Test Environment Suggestions

- **Room Temp**: 20–25°C recommended; do not exceed 30°C or go below 15°C.
- **Cooling**: Ensure GPU properly cooled; use stands for laptops.
- **GPU Temp**: If > 82°C during scan, check cooling or let system cool and continue.
- **System State**: Close other GPU loads (games, encoding); clean load environment vital for dynamic VFP export.
- **Power**: Plug in laptops; ensure sufficient PSU for desktops.

**BSOD / black screen / Linux drop / ERR during scanning is normal** — indicates frequency limit exceeded. Tool will backtrack. If system freezes > 3 min, force reboot and rerun `nvoc-auto-optimizer optimize`. On Linux, exit GPU-using processes (including GUI) and run `linux_oc_recover.sh` to reset; reboot if deadlocked. Recovery tolerance on Linux lower than Windows TDR; save work.

---

## Build from Source

```bat
git clone https://github.com/Skyworks-Neo/nvoc
cd nvoc/auto-optimizer
cargo build --release
```

Binary: `target\release\nvoc-auto-optimizer.exe`

> Requires private `nvapi-rs` git branch; GitHub access needed during build.

---

## Disclaimer

- Overclocking operates GPU outside factory specs; potential for instability.
- Crashes/BSOD are design behaviors, **not hardware damage**, but use at your own risk.
- OC settings do not persist after reboot without re-import.
- **The tool is not responsible for hardware damage, data loss, or other losses due to improper use.**

---

## License

Released under **Apache License 2.0**. Full terms in `LICENSE` file.

- SPDX: `Apache-2.0`
- Crate manifest points to license via `license-file`.

---

*nvoc-auto-optimizer v0.0.3 — by Skyworks*
