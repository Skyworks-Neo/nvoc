# NVOC-AutoOptimizer

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

[中文](./README.md) | [English](./README-en.md)

This project is released under the [Apache License 2.0](LICENSE).

> **NVIDIA GPU VF curve auto overclock optimizer**  
> Written in Rust, it controls NVIDIA GPUs through NVAPI / NVML and works with `cli-stressor` stress tests to automatically scan the voltage-frequency (V-F Curve) curve point by point, find the stable overclock ceiling at each voltage point, and generate an optimized curve.

## NVOC-AutoOptimizer — a tool that really understands overclocking

## Companion projects — use the full stack for the best experience

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor): stress test module used by the auto-overclocking scan. Without it, you can still use all functions except auto-scan. (NVOC-AutoOptimizer allows any custom stress test module as long as it follows the return code convention.)

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI): cross-platform overclocking GUI, positioned as an alternative to MSI Afterburner. (To avoid the GUI being taken down when the GPU crashes, it uses CPU rendering. On low-end systems, NVOC-TUI is recommended if performance becomes an issue.)

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI): cross-platform overclocking CLI for machines without a GUI. It has good compatibility and low performance overhead.

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV): client-server control module for web management in data centers, servers, and workstations. ~~Remote overclocking~~ (TODO)

## Table of contents

- [Background and theory](#background-and-theory)
- [Supported GPU generations](#supported-gpu-generations)
- [Compatibility matrix: interfaces × GPU generations / basic features](#compatibility-matrix-interfaces--gpu-generations--basic-features)
- [Requirements and environment](#requirements-and-environment)
- [Directory layout](#directory-layout)
- [Quick start](#quick-start)
- [Command reference](#command-reference)
  - [Top-level arguments](#top-level-arguments)
  - [info](#info)
  - [list](#list)
  - [status](#status)
  - [get](#get)
  - [reset](#reset)
  - [set](#set)
    - [set nvml-cooler](#set-nvml-cooler)
    - [set nvapi-cooler](#set-nvapi-cooler)
    - [set vfp export](#set-vfp-export)
    - [set vfp import](#set-vfp-import)
    - [set nvapi lock / reset](#set-nvapi-lock--reset)
    - [set vfp autoscan](#set-vfp-autoscan)
    - [set vfp autoscan_legacy](#set-vfp-autoscan_legacy)
    - [set vfp fix_result](#set-vfp-fix_result)
    - [set vfp single_point_adj](#set-vfp-single_point_adj)
- [Scanning workflow explained](#scanning-workflow-explained)
  - [Stage 0: Preparation](#stage-0-preparation)
  - [Stage 1: Voltage range probing](#stage-1-voltage-range-probing)
  - [Stage 2: Per-point core frequency scan](#stage-2-per-point-core-frequency-scan)
  - [Stage 3: Memory frequency scan (optional)](#stage-3-memory-frequency-scan-optional)
  - [Stage 4: fix_result post-processing](#stage-4-fix_result-post-processing)
  - [Stage 5: Import and export the final curve](#stage-5-import-and-export-the-final-curve)
- [Ultrafast mode](#ultrafast-mode)
- [Crash recovery](#crash-recovery)
- [Resume scanning](#resume-scanning)
- [Legacy GPU mode (Maxwell / Pascal)](#legacy-gpu-mode-maxwell--pascal)
- [Working files](#working-files)
- [Recommended test environment](#recommended-test-environment)
- [Building from source](#building-from-source)
- [License](#license)

---

## Background and theory

### What is a V-F curve?

Starting with Pascal (10-series), NVIDIA introduced **GPU Boost 3.0**, which is driven by a voltage-frequency lookup table (VFP). The GPU looks up the target frequency for the current core voltage and tracks it dynamically at runtime.  
The factory default curve is a conservative value calibrated by NVIDIA with plenty of headroom for the worst silicon. Real stability limits vary significantly between wafers and individual chips, and high-quality chips often can run much higher than the stock curve.

### The essence of overclocking

Overclocking means applying a positive frequency offset (`KilohertzDelta`) to each voltage point in the VFP table, raising the target frequency for that voltage. If the offset is too high, the chip violates timing margins and the GPU may trigger TDR (Timeout Detection and Recovery) or crash directly.  
The goal of this tool is: **find the maximum stable frequency offset for each point on the curve**.

### Stress-test load: cli-stressor

`cli-stressor` provides a stable stress workload at a given voltage/frequency. The pass/fail criterion is based on the **process return code**: `0` means success, any non-zero value means failure.

### Maxwell / 9-series legacy mode

Maxwell (GM codename, 9xx series) and earlier GPUs do not support per-point V-F curve writes. They can only apply a global frequency offset to P0 through `SetPstates20`. For such GPUs, this tool uses the `autoscan_legacy` workflow and scans a single global offset.

---

## Supported GPU generations

| Generation | Prefix | Mode | Notes |
|---|---|---|---|
| RTX 50 series (Blackwell) | `GB` | VF curve | Aggressive BSOD recovery by default; supports Max-Q step calibration |
| RTX 40 series (Ada Lovelace) | `AD` | VF curve | — |
| RTX 30 series (Ampere) | `GA` | VF curve | — |
| RTX 20 series (Turing TU10x) | `TU10` | VF curve (small heavy/light-load difference) | — |
| GTX 16 series (Turing TU11x) | `TU11` | VF curve (small heavy/light-load difference) | — |
| GTX 10 series (Pascal) | `GP1` | VFP curve (79 points) | Small heavy/light-load difference; `fix_result` is still recommended |
| GTX 9 series (Maxwell) | `GM` | **Legacy global offset** | No per-point VFP; use `autoscan_legacy` |
| Volta compute cards | `GV` | Legacy | Same as above |

> **Mobile GPUs** (names containing `Laptop`) cannot change TDP / thermal limits / VDDQ boost. The tool automatically skips those settings when detected.

---

## Compatibility matrix: interfaces × GPU generations / basic features

### **NVAPI + desktop consumer GPUs**

| Feature | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9 / some 7-series (GM) | Notes |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|---|
| VF curve edit + export | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | |
| Auto overclocking `autoscan` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | |
| Legacy auto overclocking `autoscan_legacy` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Core frequency offset (`--core-offset`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Memory frequency offset (`--mem-offset`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Not independent from VF curve; applying it resets the VF curve |
| Power limit (`--power-limit`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Thermal limit (`--thermal-limit`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Fan control (NVAPI cooler) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Fan reset (NVAPI cooler) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❌) | ✅ (Linux❌) | ✅ (Linux❌) | |
| Locked voltage (`--locked-voltage`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | |
| Unlock voltage (`--locked-voltage`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | |
| Lock core clock range (`--locked-core-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Unlock core clock range (`--locked-core-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Lock memory clock range (`--locked-mem-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Partial | |
| Unlock memory clock range (`--locked-mem-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Partial | |
| Boost voltage (`--voltage-boost`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | |
| Overvolt (`--voltage-delta`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | |

### **NVML + desktop consumer GPUs**

| Feature | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9 / some 7-series (GM) | Notes |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|---|
| VF curve edit + export | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | |
| Auto overclocking `autoscan` | TODO | TODO | TODO | TODO | TODO | TODO | ❌ | A new algorithm based on clock locking and offsets is required |
| Legacy auto overclocking `autoscan_legacy` | TODO | TODO | TODO | TODO | TODO | TODO | TODO | A new algorithm based on clock locking and offsets is required |
| Core frequency offset (`--core-offset`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Memory frequency offset (`--mem-offset`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Applying it resets the VF curve |
| Power limit (`--power-limit`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Thermal limit (`--thermal-limit`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Fan control (NVML cooler) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Fan reset (NVML cooler) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | |
| Locked voltage (`--locked-voltage`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | |
| Unlock voltage (`--locked-voltage`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | |
| Lock core clock range (`--locked-core-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ? | |
| Unlock core clock range (`--locked-core-clocks`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ? | |
| Lock memory clock range (`--locked-mem-clocks`) | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | |
| Unlock memory clock range (`--locked-mem-clocks`) | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | |
| Boost voltage (`--voltage-boost`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | |
| Overvolt (`--voltage-delta`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | |

### Mobile GPUs

Generally do not support **boost voltage, fan-related controls, thermal limits, or power limits**.

## Requirements and environment

### Runtime dependencies

| Dependency | Description |
|---|---|
| Windows 10/11 | The tool uses NVAPI/NVML and supports Windows 10/11 and *any Linux distribution using nvidia-open-dkms* (theoretically; tested on Arch Linux + KDE, Ubuntu 22.04, Debian 12/13) |
| NVIDIA driver (>= 537; Linux uses nvidia-open-dkms) | Must support the target GPU's NVAPI/NVML interface. Driver 395 is known to fail completely, so Kepler and older cards are difficult to support with very old drivers |
| `cli-stressor` stress-test script (for auto OC) | Default invocation comes from `test/test_cuda_windows.bat` / `test/test_opencl_linux.sh` |
| Administrator / sudo privileges | Writing overclock settings requires elevated privileges; most read-only actions do not |

### Default stress-test script locations

```
NVOC-AutoOptimizer/
├── test/
│   ├── test.bat
│   ├── test_cuda_windows.bat
│   └── test_opencl_linux.sh
└── ws/
```

### Build dependencies (only when building from source)

- Rust toolchain
- Target platform build tools

---

## Directory layout

```
NVOC-AutoOptimizer/
├── src/
│   ├── main.rs               # entry point, command routing
│   ├── arg_help.rs           # clap command-line argument definitions
│   ├── basic_func.rs         # GPU generation detection, resolution helpers, handle_info/list/status/get/reset
│   ├── nvidia_gpu_type.rs    # GPU generation identification and classification parameters
│   ├── oc_get_set_function_nvapi.rs # NVAPI: VFP lock/reset, cooler, voltage/frequency settings
│   ├── oc_get_set_function_nvml.rs  # NVML: power limits, clock locks, P-State locks
│   ├── oc_profile_function.rs# VFP export/import, fix_result, autoscan helpers
│   ├── oc_scanner.rs         # autoscan_gpuboostv3 / autoscan_legacy scanning loops
│   ├── autoscan_config.rs    # scan parameter structures (ArgMatches parsing)
│   ├── types.rs              # OutputFormat, ResetSettings, VfpResetDomain enums
│   ├── conv.rs               # enum <-> string conversions
│   ├── error.rs              # unified error type
│   ├── human.rs              # human-readable output formatting
│   └── lib.rs
├── ws/                       # created at runtime for scan intermediates
│   ├── vfp.log               # scan log (used by resume scanning)
│   ├── vfp-init.csv          # factory curve snapshot exported before the first scan
│   ├── vfp-tem.csv           # temporary autoscan results saved after each point
│   └── vfp.csv / vfp-final.csv  # fixed final curve / final export snapshot
├── test/
│   └── test.bat              # stress-test wrapper script
├── start.bat                 # standard scan launcher
├── start_ultrafast.bat       # ultrafast scan launcher
├── start_legacy.bat          # legacy GPU scan launcher
├── GpuTdrRecovery.reg        # TDR recovery registry settings
├── recover.bat               # manual recovery script
└── Cargo.toml
```

---

## Quick start

### Standard scan (recommended for RTX 20-series and newer)

Run as **Administrator**:

```bat
start.bat
```

The script will automatically:
1. Detect and list the GPUs in the system and prompt for a target GPU ID
2. Reset the VFP curve and unlock voltage locks
3. Export and save the factory curve (`ws\vfp-init.csv`)
4. Run `autoscan` across the full voltage range
5. Run `fix_result` for heavy/light-load compensation
6. Import the best curve into the GPU and export the final file (`ws\vfp-final.csv`)

### Ultrafast scan (recommended when you want to save time)

```bat
start_ultrafast.bat
```

Only 4 key voltage points are scanned and the rest are linearly interpolated, which greatly reduces runtime. See [Ultrafast mode](#ultrafast-mode).

### Legacy GPU scan (GTX 9 series and earlier / currently tested on Maxwell only)

```bat
start_legacy.bat
```

Uses a global P0 frequency offset scan and does not write a VFP curve.

### Rescan from scratch (clear history)

If you want to start over and discard resume state:

```bat
start.bat 1
```

Passing `1` clears `ws\vfp.log` and `ws\vfp-tem.csv`.

---

## Command reference

All commands should be run with Administrator privileges:

```
NVOC-Auto-Optimizer.exe [global options] <subcommand> [subcommand options]
```

### Top-level arguments

| Argument | Short | Description |
|---|---|---|
| `--gpu <GPU_ID>` | `-g` | Select a target GPU. Accepts decimal or hexadecimal (`0x0800`), or the index shown by `list` (`0, 1, 2...`). Can be specified multiple times to select multiple GPUs. If omitted, all GPUs are operated on. |
| `--output-format <OFORMAT>` | `-O` | Output format: `human` (default, human-readable) or `json` |

### info

Display detailed GPU information (model, codename, performance state, power limits, sensor limits, etc.).

```bat
NVOC-Auto-Optimizer.exe info
NVOC-Auto-Optimizer.exe -O json info -o gpu_info
```

| Argument | Description |
|---|---|
| `-o <OUTPUT>` | JSON output prefix (will generate `<OUTPUT>_gpu<ID>.json`) |

### list

List all NVIDIA GPUs in the system, showing index, ID, PCI information, and UUID.

```bat
NVOC-Auto-Optimizer.exe list
```

### status

Display the current GPU status (frequencies, voltage, temperature, fans, current VFP curve values, etc.).

```bat
NVOC-Auto-Optimizer.exe status
NVOC-Auto-Optimizer.exe status --all
NVOC-Auto-Optimizer.exe status --monitor 2.0
```

| Argument | Short | Default | Description |
|---|---|---|---|
| `--all` | `-a` | — | Show all information |
| `--status <on|off>` | `-s` | `on` | Show basic status |
| `--clocks <on|off>` | `-c` | `on` | Show clocks |
| `--coolers <on|off>` | `-C` | `off` | Show fan information |
| `--sensors <on|off>` | `-S` | `off` | Show sensor temperatures |
| `--vfp <on|off>` | `-v` | `off` | Show current VFP curve values |
| `--pstates <on|off>` | `-P` | `off` | Show P-State configuration |
| `--monitor <seconds>` | `-m` | — | Continuously refresh every N seconds |

### get

Display the currently active overclock settings (VFP offsets, P-State offsets, etc.) along with detailed NVML status information.

```bat
NVOC-Auto-Optimizer.exe get
```

> **NVML status notes**:
> `get` prints detailed NVML settings, including:
> - Power limit range (Min / Current / Max, in W)
> - Core and memory frequency bounds for each P-State
> - Core / memory overclock offsets defined per P-State
> - All supported application clock combinations, with min/max frequency boundaries and step granularity automatically summarized

### reset

Restore overclock settings to defaults. Supports flexible `setting` selection, the `--domain` alias, and fine-grained `--vfp-domain` control for VFP resets. Also provides the `reset nvml-cooler` subcommand to restore NVML fan control to automatic mode.

```bat
reset all settings
NVOC-Auto-Optimizer.exe reset

reset selected settings
NVOC-Auto-Optimizer.exe reset voltage-boost power nvapi-cooler

use `--domain` aliases (equivalent to the above)
NVOC-Auto-Optimizer.exe reset --domain voltage-boost --domain power --domain nvapi-cooler

reset NVML fans to automatic control
NVOC-Auto-Optimizer.exe reset nvml-cooler

reset NVML fan ID 1 to automatic control
NVOC-Auto-Optimizer.exe reset nvml-cooler --id 1

reset only the VFP curve and clear core frequency offsets
NVOC-Auto-Optimizer.exe reset vfp --vfp-domain core

clear only the VFP memory frequency offsets
NVOC-Auto-Optimizer.exe reset --domain vfp --vfp-domain memory

`--vfp-domain` alone defaults to `reset vfp`
NVOC-Auto-Optimizer.exe reset --vfp-domain core
```

**Selectable reset items:**

| Value | Alias | Description |
|---|---|---|
| `voltage-boost` | — | Reset boost voltage to zero |
| `thermal` or `sensor-limits` | — | Restore thermal limits to default |
| `power` or `power-limits` | — | Restore power limits to default |
| `nvapi-cooler` | — | Restore NVAPI fan control to automatic |
| `nvml-cooler` | — | Restore NVML fan control to automatic |
| `vfp` or `vfp-deltas` | — | Reset all VFP curve offsets to zero (can be combined with `--vfp-domain`) |
| `lock` or `vfp-lock` | — | Unlock voltage locking |
| `pstate` or `pstate-deltas` | — | Reset P-State frequency offsets to zero |
| `overvolt` | — | Clear legacy GPU `baseVoltage` delta (Maxwell / 9-series) |

**VFP reset domain options:**

| Value | Description |
|---|---|
| `all` | Reset both core and memory offsets (default) |
| `core` | Reset only core (Graphics) offsets |
| `memory` | Reset only memory offsets |

### set

The overclocking entry point. This now distinguishes between `nvapi` and `nvml` interfaces and supports the following subcommands and options.

#### set nvml-cooler

Set NVML fan policy and speed.

```bat
NVOC-Auto-Optimizer.exe set nvml-cooler --id 1 --policy manual --level 60
NVOC-Auto-Optimizer.exe set nvml-cooler --policy continuous --level 80
```

| Argument | Description |
|---|---|
| `--id` | Fan ID (`1` / `2` / `all`, default `all`) |
| `--policy` | Fan policy (for example `continuous` / `manual` / `auto`) |
| `--level` | Fan speed percentage |

#### set nvapi

Use the official NVAPI interface for overclocking, mainly for VFP curve locking, boost voltage, and core/memory clock range locks.

```bat
NVOC-Auto-Optimizer.exe set nvapi --voltage-boost 100 --thermal-limit 90
NVOC-Auto-Optimizer.exe set nvapi --core-offset 150000 --mem-offset 500000
NVOC-Auto-Optimizer.exe set nvapi --locked-voltage 68
NVOC-Auto-Optimizer.exe set nvapi --locked-core-clocks 210 2100
NVOC-Auto-Optimizer.exe set nvapi --locked-mem-clocks 5000 9501
NVOC-Auto-Optimizer.exe set nvapi --reset-vfp-locks
```

| Argument | Short | Description |
|---|---|---|
| `--voltage-boost <0-100>` | `-V` | Set boost voltage percentage (desktop GPUs) |
| `--thermal-limit <°C>` | `-T` | Set thermal limit |
| `--power-limit <%>` | `-P` | Set power limit percentage |
| `--voltage-delta <μV>` | `-U` | Core voltage offset in microvolts (for Maxwell / 900-series and earlier) |
| `--pstate <PSTATE>` | `-z` | Target P-State for `--voltage-delta` and offsets (default `P0`) |
| `--core-offset <kHz>` | — | Set a core frequency offset for a specific P-State through NVAPI |
| `--mem-offset <kHz>` | — | Set a memory frequency offset for a specific P-State through NVAPI |
| `--locked-voltage <POINT_OR_VOLT>` | — | Lock a VFP point. Bare numbers are treated as points (e.g. `68`); voltage values must have units (e.g. `850mV`) |
| `--locked-core-clocks <MIN> <MAX>` | — | Lock NVAPI Graphics core frequency range (MHz) |
| `--locked-mem-clocks <MIN> <MAX>` | — | Lock NVAPI Memory frequency range (MHz) |
| `--reset-core-clocks` | — | Unlock NVAPI core clock range |
| `--reset-mem-clocks` | — | Unlock NVAPI memory clock range (alias: `--pstate-unlock`) |
| `--reset-vfp-locks` | — | Clear NVAPI VFP lock state (voltage lock / frequency lock state) |

#### set nvml

Use the official NVML interface for overclocking, mainly for P-State-level offsets, power limits (watts), and memory clock window locks.

```bat
NVOC-Auto-Optimizer.exe set nvml --core-offset 150 --mem-offset 1000
NVOC-Auto-Optimizer.exe set nvml -P 350
NVOC-Auto-Optimizer.exe set nvml --pstate-lock P0
```

| Argument | Description |
|---|---|
| `--pstate <ID>` | Target P-State ID (`0` for P0, `2` for P2, default `0`) |
| `--core-offset <MHz>` | Set a core frequency offset for the specified P-State |
| `--mem-offset <MHz>` | Set a memory frequency offset for the specified P-State |
| `-T, --thermal-limit <°C>` | Thermal-limit compatibility parameter (alias: `--thermal-gpu-max`), parsed but not written in the current version |
| `--thermal-shutdown <°C>` | NVML `Shutdown` compatibility parameter, parsed but not written in the current version |
| `--thermal-slowdown <°C>` | NVML `Slowdown` compatibility parameter, parsed but not written in the current version |
| `--thermal-memory-max <°C>` | NVML `MemoryMax` compatibility parameter, parsed but not written in the current version |
| `--thermal-acoustic-min <°C>` | NVML `AcousticMin` compatibility parameter, parsed but not written in the current version |
| `--thermal-acoustic-curr <°C>` | NVML `AcousticCurr` compatibility parameter, parsed but not written in the current version |
| `--thermal-acoustic-max <°C>` | NVML `AcousticMax` compatibility parameter, parsed but not written in the current version |
| `--thermal-gps-curr <°C>` | NVML `GpsCurr` compatibility parameter, parsed but not written in the current version |
| `-P, --power-limit <W>` | Set the power limit exactly in watts |
| `--app-clock <Mem> <Core>` | Lock application clocks, passing memory (MHz) first and core (MHz) second |
| `--locked-core-clocks <Min> <Max>` | Lock GPU core clock range (MHz) |
| `--reset-core-clocks` | Unlock GPU core clock range |
| `--locked-mem-clocks <Min> <Max>` | Lock memory clock range (MHz) |
| `--pstate-lock <ID> [<ID>]` | Lock the GPU to a single or continuous NVML P-State range through memory clock windows (e.g. `P0`) |
| `--reset-mem-clocks` | Unlock memory clocks (including P-State lock release) |

`get` / `status` human-readable output shows detailed NVML Temperature Thresholds values (unsupported items show `N/A`). The current version does not write NVML temperature thresholds.

#### set nvapi-cooler

Set fan policy and speed.

```bat
NVOC-Auto-Optimizer.exe set nvapi-cooler --id 1 --policy continuous --level 60
NVOC-Auto-Optimizer.exe set nvapi-cooler --policy manual --level 80
```

| Argument | Description |
|---|---|
| `--id` | Fan ID (`1` / `2` / `all`, default `all`) |
| `--policy` | Fan policy (for example `continuous` / `manual`) |
| `--level` | Fan speed percentage |

#### set vfp export

Export the current VFP curve to a CSV file.

```bat
NVOC-Auto-Optimizer.exe set vfp export .\ws\vfp-init.csv
NVOC-Auto-Optimizer.exe set vfp export --quick .\ws\vfp-quick.csv
```

| Argument | Short | Description |
|---|---|---|
| `<OUTPUT>` | — | Output path (`-` means stdout) |
| `--tabs` | `-t` | Use tab as separator (default: comma) |
| `--quick` | `-q` | Skip dynamic load measurement and export only the static curve |
| `--nocheck` | `-n` | Skip sanity checks for dynamic measurement results |

**CSV columns for a full dynamic export:**

| Column | Description |
|---|---|
| `voltage` | Voltage point (μV) |
| `frequency` | Current set frequency (kHz) |
| `delta` | Offset relative to the factory frequency (kHz) |
| `default_frequency` | Factory static default frequency (kHz) |
| `default_frequency_load` | Measured frequency under load (kHz) |
| `margin` | Difference between loaded frequency and static default frequency (kHz) |
| `margin_bin` | `margin` converted to minimum step bins (integer) |

> Dynamic export starts the stress workload and reads the curve after about 45 seconds. Make sure the default stress-test script is executable.

#### set vfp import

Write a modified curve from CSV back into the GPU.

```bat
NVOC-Auto-Optimizer.exe set vfp import .\ws\vfp.csv
```

| Argument | Short | Description |
|---|---|---|
| `<INPUT>` | — | Input path (`-` means stdin) |
| `--tabs` | `-t` | Tab separator |

#### set nvapi lock / reset

Lock the GPU voltage to a VFP point or explicit voltage, or unlock the VFP lock state.

```bat
NVOC-Auto-Optimizer.exe set --nvapi-locked-voltage 68
NVOC-Auto-Optimizer.exe set --nvapi-locked-voltage 850mV
NVOC-Auto-Optimizer.exe set --nvapi-locked-voltage 850000uV
NVOC-Auto-Optimizer.exe set --nvapi-locked-core-clocks 210 2100
NVOC-Auto-Optimizer.exe set --nvapi-locked-mem-clocks 5000 9501
NVOC-Auto-Optimizer.exe set --nvapi-reset-core-clocks
NVOC-Auto-Optimizer.exe set --nvapi-reset-mem-clocks
NVOC-Auto-Optimizer.exe set --nvapi-reset-vfp-locks
```

| Argument | Description |
|---|---|
| `--nvapi-locked-voltage <POINT_OR_VOLTAGE>` | Bare numbers are treated as points; voltage values must explicitly use units (`mV` / `uV`) |
| `--nvapi-locked-core-clocks <MIN> <MAX>` | Lock NVAPI Graphics core frequency range (MHz) |
| `--nvapi-locked-mem-clocks <MIN> <MAX>` | Lock NVAPI Memory frequency range (MHz) |
| `--nvapi-reset-core-clocks` | Unlock NVAPI core clock range |
| `--nvapi-reset-mem-clocks` | Unlock NVAPI memory clock range (alias: `--pstate-unlock`) |
| `--nvapi-reset-vfp-locks` | Clear NVAPI VFP lock state (voltage lock / core/memory clock lock state) |

#### set vfp autoscan

**Core feature**: perform a full VFP curve auto-scan on the current GPU.

```bat
NVOC-Auto-Optimizer.exe set vfp autoscan
NVOC-Auto-Optimizer.exe set vfp autoscan -u
NVOC-Auto-Optimizer.exe set vfp autoscan -u -b aggressive
```

| Argument | Short | Default | Description |
|---|---|---|---|
| `--ultrafast` | `-u` | off | Enable ultrafast mode (scan only 4 key points and interpolate the rest) |
| `-w <path>` | — | `./test/test.bat` | Stress-test script path |
| `-l <path>` | — | `./ws/vfp.log` | Scan log path |
| `-q <sequence>` | — | `-` | Custom scan point sequence (`-` means automatic) |
| `-t <count>` | — | `30` | Number of timeout-detection loops per test |
| `-o <path>` | — | `./ws/vfp-tem.csv` | CSV path for incremental per-point results |
| `-i <path>` | — | `./ws/vfp-init.csv` | Reference factory curve CSV path |
| `-m` | — | off | Scan memory overclocking at the same time |
| `-b <mode>` | — | auto by GPU generation | Crash recovery mode: `aggressive` (active BSOD reboot) or `traditional` (wait for TDR recovery) |

#### set vfp autoscan_legacy

Auto-scan the global offset for Maxwell (GTX 9-series) and older GPUs.

```bat
NVOC-Auto-Optimizer.exe set vfp autoscan_legacy
NVOC-Auto-Optimizer.exe set vfp autoscan_legacy -b aggressive
```

The options are basically the same as `autoscan`, but `--ultrafast`, `-m` (memory scan), `-q` (point sequence), and `-i` (initial curve) are not supported because legacy mode only has one global offset.

#### set vfp fix_result

Run **heavy/light-load compensation post-processing** on the temporary CSV generated by `autoscan` to produce the final stable curve.

```bat
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1 -u
```

| Argument | Short | Description |
|---|---|---|
| `-m <integer>` | — | Extra conservative offset bins (recommended `1`, i.e. one extra step down on top of the margin correction) |
| `--ultrafast` | `-u` | off | Interpolate and complete the ultrafast result with only 4 key points |
| `-v <path>` | — | `./ws/vfp-tem.csv` | Input: temporary CSV generated by `autoscan` |
| `-o <path>` | — | `./ws/vfp.csv` | Output: compensated final curve CSV |
| `-i <path>` | — | `./ws/vfp-init.csv` | Reference: factory curve CSV |
| `-l <path>` | — | `./ws/vfp.log` | Scan log (used to read ultrafast key points) |
| `-d <integer>` | — | `3` | Reference frequency difference bin (internal parameter) |

#### set vfp single_point_adj

Manually set one point on the VFP curve to a specific frequency offset for debugging.

```bat
NVOC-Auto-Optimizer.exe set vfp single_point_adj -s 50 -d 150000
```

| Argument | Short | Default | Description |
|---|---|---|---|
| `-s <index>` | — | `50` | Starting point index |
| `-d <kHz>` | — | `150000` | Frequency offset (kHz) |

---

## Scanning workflow explained

### Stage 0: Preparation

Before calling `autoscan`, `start.bat` performs the following steps:

1. `info`: print GPU information and detect the generation
2. `reset pstate`: clear P-State frequency offsets to ensure a clean starting point
3. `reset vfp` (or `reset --domain vfp --vfp-domain all`): clear all VFP offsets
   - If you only want to clear core overclocking, use `reset --domain vfp --vfp-domain core`
   - If you only want to clear memory overclocking, use `reset --domain vfp --vfp-domain memory`
4. `set --nvapi-reset-vfp-locks`: unlock voltage / frequency locks
5. `set vfp export .\ws\vfp-init.csv` (if it does not exist): save the factory curve as a reference
6. Add a firewall rule for the stress-test executable (optional, to avoid network activity affecting the test)

### Stage 1: Voltage range probing

When `autoscan` starts, it first calls `handle_test_voltage_limits`. Starting from generation-specific preset points, it probes the actual usable voltage range of the GPU by advancing the voltage lock (`handle_lock_vfp`) step by step:

- **Upper-bound probing**: advance upward point by point from the preset upper point until locking fails (the curve reaches a flat region or goes out of range)
- **Lower-bound probing**: advance downward from the preset lower point to find the minimum usable voltage point

The results are recorded in `vfp.log` (`minimum_voltage_point` / `maximum_voltage_point`). During resume scanning, this stage is skipped by reading the log directly.

### Stage 2: Per-point core frequency scan

For the range `[lower_voltage_point, upper_voltage_point]`, scan every 3 points in standard mode, or use 4 key points in ultrafast mode. At each voltage point, run a two-stage binary search:

**Short Test**: use exponential stepping (`2^n × minimum step`) to quickly converge close to the stable upper frequency limit.

**Long Test**: based on the value found in the short test, perform single-step endurance validation to confirm true stability.

During each round of testing, the tool periodically applies **frequency fluctuations**: it raises or lowers the current set frequency periodically to simulate dynamic workload changes and prevent the GPU from passing at a static frequency but crashing when switching in practice. A watchdog also monitors whether the stress-test process is still alive to prevent the scan from hanging.

If thermal / power throttling is detected (`thrm_or_pwr_limit_flag`), ~~the test resolution will be lowered automatically to reduce GPU load~~ (TODO since the load was changed to CLI-based testing), ensuring the result reflects overclocking capability rather than TDP bottlenecks.

After each voltage point finishes, the result is appended to `vfp-tem.csv` in real time and recorded in `vfp.log` (resume scanning supported).

### Stage 3: Memory frequency scan (optional)

Enabled with `-m`. After the core scan finishes, the tool locks to the highest voltage point and scans the memory overclock ceiling using a similar binary-search approach.

### Stage 4: `fix_result` post-processing

After `autoscan` completes, `start.bat` automatically runs:

```bat
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1
```

**Why it exists**: Starting from Pascal (10-series), NVIDIA V-F curves exhibit a small difference between light load and heavy load — similar to CPU Load-Line Calibration (LLC). If the stable maximum frequency found under full load is written back without correction, instability may occur when the operating point shifts during load transitions.

`fix_result` uses the `margin_bin` values recorded in `vfp-init.csv` (difference between loaded frequency and static default frequency, converted to step bins) to conservatively adjust each point from `autoscan`:
- `margin_bin > 5`: reduce by `(5 + minus_bin)` steps
- `|margin_bin| < 2`: reduce by `(1 + minus_bin)` steps
- otherwise: reduce by `(|margin_bin| + 1 + minus_bin)` steps

In ultrafast mode, the missing voltage points between the 4 key points are first filled in by linear interpolation.

### Stage 5: Import and export the final curve

```bat
NVOC-Auto-Optimizer.exe set vfp import .\ws\vfp.csv
NVOC-Auto-Optimizer.exe set vfp export .\ws\vfp-final.csv
```

Write the corrected curve into the GPU and save the final snapshot file.

---

## Ultrafast mode

The core assumption of ultrafast mode is: **under the factory-calibrated curve, overclock headroom decreases monotonically as voltage rises**. Therefore, only a few points need to be tested and the rest can be linearly interpolated, greatly reducing scan time.

**Key-point selection logic (for RTX 50-series GPUs with a Max-Q step):**

The factory curve of NVIDIA 50-series GPUs contains a "Max-Q step" — at a certain mid-voltage point, frequency jumps sharply, forming a step. The step position differs between light load and heavy load. To handle this correctly, ultrafast mode detects these 4 key points from `vfp-init.csv` (which includes a dynamic load column):

| Key point | Detection basis |
|---|---|
| p1 | Point of maximum jump in the static curve (lower boundary of the step, light load) |
| p2 | Point of maximum jump in the loaded curve (lower boundary of the step, heavy load) |
| p3 | First point where `margin_bin` changes from 0 to negative (upper boundary of the step) |
| p4 | Point with the most negative `margin_bin` (strongest heavy-load suppression) |

The scan only tests these 4 points once each. In `fix_result`, linear interpolation fills the gaps between segments, and an extra safety reduction is applied to the overclock headroom in the stepped region.

**GPUs without a Max-Q step** (30-series, 40-series, etc.): p1–p4 are evenly distributed across the scan range at quarter intervals and are interpolated linearly.

---

## Crash recovery

Crashes are expected during overclocking scans. The tool includes two recovery strategies:

### Traditional mode

Wait for Windows TDR (Timeout Detection and Recovery) to detect the GPU hang and recover the driver automatically. This works for most GPUs.

### Aggressive mode (default on RTX 50-series)

Some RTX 50-series drivers have a bug where TDR does not recover the driver, leaving the system permanently stuck and preventing the auto-scan flow from continuing.  
Aggressive mode actively triggers a kernel crash (BSOD) to force a reboot, and uses a Windows startup auto-run entry to resume scanning from the breakpoint after reboot.

> **Note**: An intentional BSOD is a normal part of the flow control and **does not indicate hardware damage**. It will not damage the GPU or OS. After reboot, simply continue — the scan will recover automatically.

You can override the mode with `-b`:

```bat
set vfp autoscan -b traditional    # force traditional mode
set vfp autoscan -b aggressive      # force aggressive mode
```

---

## Resume scanning

After each voltage point finishes, the result is appended to `ws\vfp.log`. On the next run, the tool parses the log for:

- `minimum_voltage_point` / `maximum_voltage_point`: skip the voltage range probing stage
- The last `Finished core OC on point` entry: continue from the next point
- The last successful/failed frequency offset values: restore the binary-search convergence state directly

Therefore, whether the process exits normally, is interrupted manually, or crashes and reboots, **you only need to run `start.bat` again and the scan will resume from where it stopped**.

To start over, run `start.bat 1` to clear the log.

---

## Legacy GPU mode (Maxwell / Pascal)

The NVAPI used by GTX 9-series (Maxwell, GM codename) does not support per-point VFP curve writes. It can only set a single global frequency offset for P0. The `autoscan_legacy` flow:

1. Write a global offset through `set_pstates` (`ClockDomain::Graphics`, `PState::P0`)
2. Use the same binary-search + endurance-test logic as the VFP workflow to scan the maximum stable offset
3. Not supported: ultrafast interpolation, memory segment scans, per-point `fix_result`

For voltage control, Maxwell uses the `baseVoltages` field in `SetPstates20` (the `--voltage-delta` parameter, in μV) rather than the VoltRails boost mechanism used on Pascal and later.

Legacy OverVolt often does not work well in practice, and NVIDIA's support is not great. For cards older than the 900-series, and only if core, memory, and VRM cooling are sufficiently strong, direct VBIOS editing is often the path to the best performance.

---

## Working files

| File | Purpose | Notes |
|---|---|---|
| `ws\vfp.log` | Scan log | Core basis for resume scanning; deleting it forces a restart from scratch |
| `ws\vfp-init.csv` | Factory curve snapshot | Automatically exported before the first scan; reference for `fix_result` |
| `ws\vfp-tem.csv` | Temporary autoscan results | Written after each point; input to `fix_result` |
| `ws\vfp.csv` | `fix_result` output | Final compensated curve; input to `import` |
| `ws\vfp-final.csv` | Final snapshot | Saved again after `import` + `export`; useful for comparison and backup |

---

## Recommended test environment

- **Room temperature**: 20–25°C is recommended; do not exceed 30°C and do not go below 15°C
- **Cooling**: Ensure the GPU is properly cooled; for laptops, use a stand and avoid soft surfaces
- **GPU temperature**: If the core temperature stays above 82°C during scanning, check your cooling or let the system cool down and continue later
- **System state**: Close other GPU workloads before scanning (games, video encoding, etc.); this is especially important for dynamic VFP export
- **Power**: Make sure laptops are plugged in and desktop systems have sufficient PSU capacity

**BSOD / black screen / Linux GPU drop / ERR during scanning is normal** — it means the current offset has exceeded the GPU's stable limit. The tool will back off and continue from a more conservative value. If the system freezes for more than 3 minutes, force a reboot manually and run `start.bat` again. On Linux, you may need to exit all processes using the GPU (including graphical sessions) and run `linux_oc_recover.sh` to reset it; if the process deadlocks, a reboot is usually the only option. Linux recovery tolerance for unstable overclocking is generally lower than Windows TDR, so please save your work.

---

## Building from source

```bat
git clone https://github.com/your-org/NVOC-AutoOptimizer.git
cd NVOC-AutoOptimizer
cargo build --release
```

Binary output: `target\release\NVOC-Auto-Optimizer.exe`

> This project depends on a private git branch of `nvapi-rs`, so GitHub access is required during build.

---

## License

- This project is released under the **Apache License 2.0**.
- See the root `LICENSE` file for the full text.
- SPDX identifier: `Apache-2.0`
- The crate manifest explicitly points to the license text via `license-file = "LICENSE"`
- Release packages and source archives keep `LICENSE` for redistribution and downstream use

---

*NVOC-AutoOptimizer v0.0.3 — by Skyworks*

