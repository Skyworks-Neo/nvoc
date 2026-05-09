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
  - [Top-level Parameters](#top-level-parameters)
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

### Maxwell / 9 Series Legacy Mode

Maxwell (GM code name, 9xx series) and earlier GPUs do not support point-by-point V-F curve writing and can only apply a global frequency offset to the P0 state through `SetPstates20`. This tool uses the `autoscan_legacy` flow for such GPUs, scanning only a single global offset value.

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
| GTX 9 Series (Maxwell)       | `GM`             | **Legacy Global Offset**                           | Point-by-point VFP not supported; scanned via `autoscan_legacy`       |
| Volta Compute Cards          | `GV`             | Legacy                                             | Same as above                                                         |

> **Mobile GPUs** (name contains `Laptop`) cannot modify TDP / Temperature Wall / VDDQ boost; the tool will automatically skip these settings when detected.

---

## Compatibility Overview (Interface × GPU Generation / Basic Functions)

Note: Testing on Linux NV proprietary drivers shows that NVAPI on Linux is essentially a compatibility layer for /lib/x86_64-linux-gnu/libnvidia-api.so.1, directly leading to /lib/x86_64-linux-gnu/libnvidia-ml.so.1. In other words, on Linux, only the NVML interface actually exists. However, due to NVAPI's "translation" of NVML, for professional cards (such as P100, V100, etc.)—since this project's primary GPU ID index uses the NVAPI interface—support on Linux may be better than on Windows.
Additionally, the core frequency range lock of NVML is actually a voltage range lock at the bottom. We have verified through dynamic adjustment—after adjusting the frequency offset, the frequency range lock still takes effect within the working point of the voltage range corresponding to the frequency range at the time of setting, rather than the original frequency range.

### **NVAPI Interface + Desktop Consumer GPUs:**

|                      Function                      | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) |  GTX 9/Part 7 (GM)  |                          Remarks                           |
|:--------------------------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:-------------------:|:----------------------------------------------------------:|
|               VF curve Edit + Export               |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|                  Auto OC autoscan                  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|           Legacy Auto OC autoscan_legacy           |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|      Core Frequency Offset (`--core-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|      Memory Frequency Offset (`--mem-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          | Not independent from VF Curve; applying it resets VF Curve |
|            Power Wall (`--power-limit`)            |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|        Temperature Wall (`--thermal-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|          Fan Speed Control (NVAPI cooler)          |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|           Fan Speed Reset (NVAPI cooler)           | ✅ (Linux❓)  | ✅ (Linux❓)  | ✅ (Linux❓)  |  ✅ (Linux❌)   |  ✅ (Linux❌)   |  ✅ (Linux❌)  |     ✅ (Linux❌)      |                                                            |
|       Voltage Point Lock (--locked-voltage)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|      Voltage Point Unlock (reset-volt-locks)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|  Core Frequency Range Lock (--locked-core-clocks)  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
| Core Frequency Range Unlock (--reset-core-clocks)  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
| Memory Frequency Range Lock (--locked-mem-clocks)  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       | Partially Supported |                                                            |
| Memory Frequency Range Unlock (--reset-mem-clocks) |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       | Partially Supported |                                                            |
|     Boost Voltage Voltboost (--voltage-boost)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|             Overvolt (--voltage-delta)             |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |          ✅          |                                                            |

### **NVML Interface + Desktop Consumer GPUs:**

|                      Function                      | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9/Part 7 (GM) |                          Remarks                           |
|:--------------------------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:-----------------:|:----------------------------------------------------------:|
|               VF curve Edit + Export               |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|                  Auto OC autoscan                  |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |         ❌         | Requires new algorithm based on frequency lock and offset  |
|           Legacy Auto OC autoscan_legacy           |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |       TODO        | Requires new algorithm based on frequency lock and offset  |
|      Core Frequency Offset (`--core-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|      Memory Frequency Offset (`--mem-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         | Not independent from VF Curve; applying it resets VF Curve |
|            Power Wall (`--power-limit`)            |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|        Temperature Wall (`--thermal-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|          Fan Speed Control (NVML cooler)           |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|           Fan Speed Reset (NVML cooler)            |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|       Voltage Point Lock (--locked-voltage)        |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|      Voltage Point Unlock (reset-volt-locks)       |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|  Core Frequency Range Lock (--locked-core-clocks)  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ❓         |                                                            |
| Core Frequency Range Unlock (--reset-core-clocks)  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ❓         |                                                            |
| Memory Frequency Range Lock (--locked-mem-clocks)  |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
| Memory Frequency Range Unlock (--reset-mem-clocks) |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|   App Frequency Range Lock (--locked-app-clocks)   |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|  App Frequency Range Unlock (--reset-app-clocks)   |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|     Boost Voltage Voltboost (--voltage-boost)      |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |
|             Overvolt (--voltage-delta)             |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |         ❌         |                                                            |

### **NVAPI Interface + Server Grade GPUs:**

|                      Function                      | Blackwell (GB) | Hopper (GH)  | Ampere (GA)  | Turing (GT)  |  Volta (GV)  | Pascal (GP)  |                     Remarks                     |
|:--------------------------------------------------:|:--------------:|:------------:|:------------:|:------------:|:------------:|:------------:|:-----------------------------------------------:|
|               VF curve Edit + Export               |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|                  Auto OC autoscan                  |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|      Core Frequency Offset (`--core-offset`)       |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|      Memory Frequency Offset (`--mem-offset`)      |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|            Power Wall (`--power-limit`)            |       ✅        |      ✅       |      ✅       |      ✅       |      ✅       |      ✅       |  Usually locked by driver, needs NVML attempt   |
|        Temperature Wall (`--thermal-limit`)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❓       |   Cases where return code 0 but not effective   |
|          Fan Speed Control (NVAPI cooler)          |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       | No onboard fan or motherboard/system controlled |
|       Voltage Point Lock (--locked-voltage)        |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|  Core Frequency Range Lock (--locked-core-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Core Frequency Range Unlock (--reset-core-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Lock (--locked-mem-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) |      ❌       |      ❌       |      ❌       | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Unlock (--reset-mem-clocks) |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) |      ❌       |      ❌       |      ❌       | Diff driver model + Linux has NVAPI translation |
|     Boost Voltage Voltboost (--voltage-boost)      |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|             Overvolt (--voltage-delta)             |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                        ❌                        | |

### **NVAPI Interface + Workstation Grade GPUs:**

|                     Function                      | Blackwell (GB) | Ada (AD) | Ampere (GA) | Turing (TU) | Pascal (GP) | Remarks |
|:-------------------------------------------------:|:--------------:|:--------:|:-----------:|:-----------:|:-----------:|:-------:|
|              VF curve Edit + Export               |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |         |
|                 Auto OC autoscan                  |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |         |
|      Core Frequency Offset (`--core-offset`)      |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |         |
|     Memory Frequency Offset (`--mem-offset`)      |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |         |
|           Power Wall (`--power-limit`)            |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|       Temperature Wall (`--thermal-limit`)        |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|         Fan Speed Control (NVAPI cooler)          |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|       Voltage Point Lock (--locked-voltage)       |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |         |
| Core Frequency Range Lock (--locked-core-clocks)  |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
| Memory Frequency Range Lock (--locked-mem-clocks) |       ✅        |    ✅     |      ✅      |      ❓      |      ❌      |         |
|     Boost Voltage Voltboost (--voltage-boost)     |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |

### **NVML Interface + Server Grade GPUs:**

|                      Function                      | Blackwell (GB) | Hopper (GH)  | Ampere (GA)  | Turing (GT)  |  Volta (GV)  | Pascal (GP)  |                     Remarks                     |
|:--------------------------------------------------:|:--------------:|:------------:|:------------:|:------------:|:------------:|:------------:|:-----------------------------------------------:|
|               VF curve Edit + Export               |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|                  Auto OC autoscan                  |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|           Legacy Auto OC autoscan_legacy           |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|      Core Frequency Offset (`--core-offset`)       |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|      Memory Frequency Offset (`--mem-offset`)      |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|            Power Wall (`--power-limit`)            |       ✅        |      ✅       |      ✅       |      ✅       |      ✅       |      ✅       |                                                 |
|        Temperature Wall (`--thermal-limit`)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❓       |   Cases where return code 0 but not effective   |
|          Fan Speed Control (NVML cooler)           |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       | No onboard fan or motherboard/system controlled |
|           Fan Speed Reset (NVML cooler)            |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       | No onboard fan or motherboard/system controlled |
|       Voltage Point Lock (--locked-voltage)        |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|      Voltage Point Unlock (reset-volt-locks)       |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|  Core Frequency Range Lock (--locked-core-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Core Frequency Range Unlock (--reset-core-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Lock (--locked-mem-clocks)  |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) |      ❌       |      ❌       |      ❌       | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Unlock (--reset-mem-clocks) |  ✅ (Windows❌)  | ✅ (Windows❌) | ✅ (Windows❌) |      ❌       |      ❌       |      ❌       | Diff driver model + Linux has NVAPI translation |
|   App Frequency Range Lock (--locked-app-clocks)   |       ✅        |      ✅       |      ✅       |      ✅       |      ✅       |      ✅       |                                                 |
|  App Frequency Range Unlock (--reset-app-clocks)   |       ✅        |      ✅       |      ✅       |      ✅       |      ✅       |      ✅       |                                                 |
|     Boost Voltage Voltboost (--voltage-boost)      |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                                                 |
|             Overvolt (--voltage-delta)             |       ❌        |      ❌       |      ❌       |      ❌       |      ❌       |      ❌       |                        ❌                        | |

### **NVML Interface + Workstation Grade GPUs:**

|                      Function                      | Blackwell (GB) | Ada (AD) | Ampere (GA) | Turing (TU) | Pascal (GP) | Remarks |
|:--------------------------------------------------:|:--------------:|:--------:|:-----------:|:-----------:|:-----------:|:-------:|
|               VF curve Edit + Export               |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|                  Auto OC autoscan                  |      TODO      |   TODO   |    TODO     |    TODO     |    TODO     |         |
|           Legacy Auto OC autoscan_legacy           |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|      Core Frequency Offset (`--core-offset`)       |       ❓        |    ❓     |      ❓      |      ❓      |      ❓      |         |
|      Memory Frequency Offset (`--mem-offset`)      |       ❓        |    ❓     |      ❓      |      ❓      |      ❓      |         |
|            Power Wall (`--power-limit`)            |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|        Temperature Wall (`--thermal-limit`)        |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|          Fan Speed Control (NVML cooler)           |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|           Fan Speed Reset (NVML cooler)            |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
|       Voltage Point Lock (--locked-voltage)        |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|      Voltage Point Unlock (reset-volt-locks)       |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|  Core Frequency Range Lock (--locked-core-clocks)  |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
| Core Frequency Range Unlock (--reset-core-clocks)  |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |         |
| Memory Frequency Range Lock (--locked-mem-clocks)  |       ✅        |    ✅     |      ✅      |      ❌      |      ❌      |         |
| Memory Frequency Range Unlock (--reset-mem-clocks) |       ✅        |    ✅     |      ✅      |      ❌      |      ❌      |         |
|   App Frequency Range Lock (--locked-app-clocks)   |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|  App Frequency Range Unlock (--reset-app-clocks)   |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|     Boost Voltage Voltboost (--voltage-boost)      |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|             Overvolt (--voltage-delta)             |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |    ❌    | ❌ |

### Mobile GPUs / Workstation GPUs / Server GPUs:

Generally do not support **Boost Voltage, fan-related controls, temperature wall related controls, power wall related controls**.

## Dependencies and Environment Requirements

### Runtime Dependencies

| Dependency                                                       | Description                                                                                                                                                                            |
|------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Windows 10/11 or any Linux distribution                          | The tool is invoked via NVAPI/NVML and supports Windows 10/11 and *any Linux distribution using nvidia-open-dkms* (theoretically, ArchLinux + KDE, Ubuntu 22.04, Debian 12/13 tested). |
| NVIDIA Driver (≥ 537, nvidia-open-dkms or proprietary for Linux) | Must support NVAPI/NVML interface for target GPU. Version 395 known not to work; hard to support Kepler and earlier GPUs with old drivers.                                             |
| `cli-stressor` pressure test script (for auto OC)                | By default called by `test\test_cuda_windows.bat` / `test\test_opencl_linux.sh`.                                                                                                       |
| Administrator/Sudo privileges                                    | OC parameter writing requires admin privileges; most reads do not.                                                                                                                     |

### Pressure Test Script Location (Default)

```
auto-optimizer/
├── test/
│   ├── test.bat
│   ├── test_cuda_windows.bat
│   └── test_opencl_linux.sh
└── ws/
```

### Build Dependencies (Only if building from source)

- Rust toolchain
- Build tools for target architecture

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
│   ├── oc_scanner.rs         # autoscan_gpuboostv3 / autoscan_legacy core scanning loop
│   ├── autoscan_config.rs    # Scan parameter struct (unified ArgMatches parsing)
│   ├── types.rs              # OutputFormat, ResetSettings, VfpResetDomain enums
│   ├── conv.rs               # Enum string conversions
│   ├── error.rs              # Unified error types
│   ├── human.rs              # Human-readable output formatting
│   └── lib.rs
├── ws/                       # Created automatically at runtime, stores intermediate scan files
│   ├── vfp.log               # Scan process log (resumption depends on this file)
│   ├── vfp-init.csv          # Factory original curve exported before first scan
│   ├── vfp-tem.csv           # Real-time temporary results saved per point during autoscan
│   └── vfp.csv / vfp-final.csv  # fix_result post-processing results / final exported confirmation file
├── test/
│   └── test.bat              # Pressure test wrapper script calling cli-stressor
├── start.bat                 # Standard scan startup script
├── start_ultrafast.bat       # Ultrafast scan startup script
├── start_legacy.bat          # Legacy GPU scan startup script
├── GpuTdrRecovery.reg        # Registry related to TDR recovery
├── recover.bat               # Manual recovery script
└── Cargo.toml
```

---

## Quick Start

### Standard Scan (Recommended, for RTX 20 series and above)

Run as **Administrator**:

```bat
start.bat
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
start_ultrafast.bat
```

Only scans 4 key voltage points, other points are linearly interpolated, significantly speed up. See [Ultrafast Mode](#ultrafast-mode).

### Legacy GPU Scan (GTX 9 series and earlier / only Maxwell tested so far)

```bat
start_legacy.bat
```

Uses global P0 frequency offset scanning, no VFP curve writing.

### Re-scan (Clear history)

To start from scratch (discarding breakpoint resume state):

```bat
start.bat 1
```

Passing argument `1` will clear `ws\vfp.log` and `ws\vfp-tem.csv`.

---

## Command Reference

All commands must be run with administrator privileges:

```
nvoc-auto-optimizer.exe [global parameters] <subcommand> [subcommand parameters]
```

### Top-level Parameters

| Parameter                   | Shorthand | Description                                                                                                                              |
|-----------------------------|-----------|------------------------------------------------------------------------------------------------------------------------------------------|
| `--gpu <GPU_ID>`            | `-g`      | Target GPU. Accepts decimal or hex (`0x0800`), or index from `list` (0, 1, 2...). Can be specified multiple times. Defaults to all GPUs. |
| `--output-format <OFORMAT>` | `-O`      | Output format: `human` (default) or `json`.                                                                                              |

---

### info

Display detailed GPU information (model, code name, performance state, power limit, sensor limits, etc.).

```bat
nvoc-auto-optimizer.exe info
nvoc-auto-optimizer.exe -O json info -o gpu_info
```

| Parameter     | Description                                                  |
|---------------|--------------------------------------------------------------|
| `-o <OUTPUT>` | JSON output path prefix (generates `<OUTPUT>_gpu<ID>.json`). |

---

### list

List all NVIDIA GPUs in the system, showing index, ID, PCI info, and UUID.

```bat
nvoc-auto-optimizer.exe list
```

---

### status

Display current GPU running status (frequency, voltage, temperature, fan, current VFP curve values, etc.).

```bat
nvoc-auto-optimizer.exe status
nvoc-auto-optimizer.exe status --all
nvoc-auto-optimizer.exe status --monitor 2.0
```

| Parameter               | Shorthand | Default | Description                                           |
|-------------------------|-----------|---------|-------------------------------------------------------|
| `--all`                 | `-a`      | —       | Display all information.                              |
| `--status <on\|off>`    | `-s`      | `on`    | Display basic status.                                 |
| `--clocks <on\|off>`    | `-c`      | `on`    | Display clock frequencies.                            |
| `--coolers <on\|off>`   | `-C`      | `off`   | Display fan information.                              |
| `--sensors <on\|off>`   | `-S`      | `off`   | Display sensor temperatures.                          |
| `--vfp <on\|off>`       | `-v`      | `off`   | Display current VFP curve values.                     |
| `--pstates <on\|off>`   | `-P`      | `off`   | Display P-State config.                               |
| `--monitor <seconds>`   | `-m`      | —       | Continuous monitoring, refresh at specified interval. |

---

### get

Display current effective OC settings (VFP offsets, P-State offsets, etc.) and detailed NVML status.

```bat
nvoc-auto-optimizer.exe get
```

> **Additional Note (NVML Status)**:
> The `get` command now outputs detailed underlying NVML settings, including:
> - Power wall limit range (Min / Current / Max, in Watts)
> - Core and memory frequency boundaries for each P-State
> - **Core / Memory OC Offset** set for each P-State
> - All supported Application Clock combinations for this card, automatically organizing min/max boundaries and step values.

---

### reset

Restore OC settings to defaults. Supports flexible `setting` selectors, `--domain` aliases, and fine-grained `--vfp-domain` control for VFP reset; additionally provides `reset nvml-cooler` to restore default NVML fan control.

```bat
# Reset all settings
nvoc-auto-optimizer.exe reset

# Reset only specified settings
nvoc-auto-optimizer.exe reset voltage-boost power nvapi-cooler

# Using --domain aliases (equivalent to above)
nvoc-auto-optimizer.exe reset --domain voltage-boost --domain power --domain nvapi-cooler

# Reset NVML fans to default control mode
nvoc-auto-optimizer.exe reset nvml-cooler

# Reset NVML fan default control mode by fan ID
nvoc-auto-optimizer.exe reset nvml-cooler --id 1

# Reset only VFP curve and core frequency offset
nvoc-auto-optimizer.exe reset vfp --vfp-domain core

# Clear only VFP memory frequency offset
nvoc-auto-optimizer.exe reset --domain vfp --vfp-domain memory

# Only --vfp-domain provided, defaults to reset vfp
nvoc-auto-optimizer.exe reset --vfp-domain core
```

**Specified items to reset (multiple selectable):**

| Value                        | Shorthand or Alias | Description                                                 |
|------------------------------|--------------------|-------------------------------------------------------------|
| `voltage-boost`              | —                  | Reset Voltage Boost to zero                                 |
| `thermal` or `sensor-limits` | —                  | Restore Temperature Wall to default                         |
| `power` or `power-limits`    | —                  | Restore Power Wall to default                               |
| `nvapi-cooler`               | —                  | Restore NVAPI fan control to automatic                      |
| `nvml-cooler`                | —                  | Restore NVML fan control to automatic                       |
| `vfp` or `vfp-deltas`        | —                  | Zero all VFP curve offsets (can use `--vfp-domain`)         |
| `lock` or `vfp-lock`         | —                  | Release voltage lock                                        |
| `pstate` or `pstate-deltas`  | —                  | Zero P-State frequency offsets                              |
| `overvolt`                   | —                  | Zero baseVoltage delta for legacy GPUs (Maxwell / 9 series) |

**VFP Reset Domain Options (active only when `--vfp-domain` is specified):**

| Value    | Description                                            |
|----------|--------------------------------------------------------|
| `all`    | Reset both core and memory frequency offsets (default) |
| `core`   | Reset Graphics frequency offset only                   |
| `memory` | Reset Memory frequency offset only                     |

---

### set

Entry point for OC settings, now distinguished between `nvapi` and `nvml` interfaces, supporting the following subcommands and parameters.

#### set nvml-cooler

Set NVML fan policy and level.

```bat
nvoc-auto-optimizer.exe set nvml-cooler --id 1 --policy manual --level 60
nvoc-auto-optimizer.exe set nvml-cooler --policy continuous --level 80
```

| Parameter  | Description                                         |
|------------|-----------------------------------------------------|
| `--id`     | Fan ID (`1` / `2` / `all`, default `all`)           |
| `--policy` | Fan policy (e.g., `continuous` / `manual` / `auto`) |
| `--level`  | Fan speed percentage                                |

#### set nvapi

OC through official NVAPI interface, mainly for VFP curve locking, Voltage Boost, and core/memory frequency range locks.

```bat
nvoc-auto-optimizer.exe set nvapi --voltage-boost 100 --thermal-limit 90
nvoc-auto-optimizer.exe set nvapi --core-offset 150000 --mem-offset 500000
nvoc-auto-optimizer.exe set nvapi --locked-voltage 68
nvoc-auto-optimizer.exe set nvapi --locked-core-clocks 210 2100
nvoc-auto-optimizer.exe set nvapi --locked-mem-clocks 5000 9501
nvoc-auto-optimizer.exe set nvapi --reset-vfp-locks
```

| Parameter                          | Shorthand | Description                                                                       |
|------------------------------------|-----------|-----------------------------------------------------------------------------------|
| `--voltage-boost <0-100>`          | `-V`      | Set Voltage Boost percentage (Desktop GPUs)                                       |
| `--thermal-limit <℃>`              | `-T`      | Set Temperature Wall (Celsius)                                                    |
| `--power-limit <%>`                | `-P`      | Set Power Wall percentage                                                         |
| `--voltage-delta <μV>`             | `-U`      | Core voltage offset (μV, for Maxwell / 900 series and earlier)                    |
| `--pstate <PSTATE>`                | `-z`      | Target P-State for `--voltage-delta` and Offset (default `P0`)                    |
| `--core-offset <kHz>`              | —         | Set core frequency offset via NVAPI for specific P-State (kHz)                    |
| `--mem-offset <kHz>`               | —         | Set memory frequency offset via NVAPI for specific P-State (kHz)                  |
| `--locked-voltage <POINT_OR_VOLT>` | —         | Lock VFP voltage. Number by point (e.g., `68`); voltage with unit (e.g., `850mV`) |
| `--locked-core-clocks <MIN> <MAX>` | —         | Lock NVAPI Graphics core frequency range (MHz)                                    |
| `--locked-mem-clocks <MIN> <MAX>`  | —         | Lock NVAPI Memory frequency range (MHz)                                           |
| `--reset-core-clocks`              | —         | Release NVAPI core frequency lock                                                 |
| `--reset-mem-clocks`               | —         | Release NVAPI memory frequency lock (alias: `--pstate-unlock`)                    |
| `--reset-vfp-locks`                | —         | Clear NVAPI VFP lock (voltage / frequency lock status)                            |

#### set nvml

OC through official NVML interface, mainly for P-State level Offset settings, Power Limit (Watts), and memory lock windows.

```bat
nvoc-auto-optimizer.exe set nvml --core-offset 150 --mem-offset 1000
nvoc-auto-optimizer.exe set nvml -P 350
nvoc-auto-optimizer.exe set nvml --pstate-lock P0
```

| Parameter                          | Description                                                                                         |
|------------------------------------|-----------------------------------------------------------------------------------------------------|
| `--pstate <ID>`                    | Target P-State index (`0` for P0, `2` for P2, default `0`)                                          |
| `--core-offset <MHz>`              | Set core frequency offset for specific P-State (MHz)                                                |
| `--mem-offset <MHz>`               | Set memory frequency offset for specific P-State (MHz)                                              |
| `-T, --thermal-limit <℃>`          | Temp wall compatibility param (alias: `--thermal-gpu-max`), parse only, not written in this version |
| `--thermal-shutdown <℃>`           | NVML `Shutdown` compat, parse only                                                                  |
| `--thermal-slowdown <℃>`           | NVML `Slowdown` compat, parse only                                                                  |
| `--thermal-memory-max <℃>`         | NVML `MemoryMax` compat, parse only                                                                 |
| `--thermal-acoustic-min <℃>`       | NVML `AcousticMin` compat, parse only                                                               |
| `--thermal-acoustic-curr <℃>`      | NVML `AcousticCurr` compat, parse only                                                              |
| `--thermal-acoustic-max <℃>`       | NVML `AcousticMax` compat, parse only                                                               |
| `--thermal-gps-curr <℃>`           | NVML `GpsCurr` compat, parse only                                                                   |
| `-P, --power-limit <W>`            | Set precise power wall limit (Watts)                                                                |
| `--locked-app-clocks <Mem> <Core>` | Lock App clocks, memory (MHz) and core (MHz)                                                        |
| `--reset-app-clocks`               | Reset App clocks to defaults |
| `--locked-core-clocks <Min> <Max>` | Lock GPU core frequency range (MHz)                                                                 |
| `--reset-core-clocks`              | Release GPU core frequency lock                                                                     |
| `--locked-mem-clocks <Min> <Max>`  | Lock memory frequency range (MHz)                                                                   |
| `--pstate-lock <ID> [<ID>]`        | Lock GPU to a single or range of NVML P-States (e.g., `P0`) via memory locking                      |
| `--reset-mem-clocks`               | Release memory frequency lock (includes P-State unlock)                                             |

`get` / `status` human-readable output shows detailed NVML Temperature Thresholds values (unsupported items show `N/A`); NVML temp threshold writing is not performed in the current version.

#### set nvapi-cooler

Set fan policy and level.

```bat
nvoc-auto-optimizer.exe set nvapi-cooler --id 1 --policy continuous --level 60
nvoc-auto-optimizer.exe set nvapi-cooler --policy manual --level 80
```

| Parameter  | Description                                |
|------------|--------------------------------------------|
| `--id`     | Fan ID (`1` / `2` / `all`, default `all`)  |
| `--policy` | Fan policy (e.g., `continuous` / `manual`) |
| `--level`  | Fan speed percentage                       |

---

#### set vfp export

Export current VFP curve as CSV.

```bat
nvoc-auto-optimizer.exe set vfp export .\ws\vfp-init.csv
nvoc-auto-optimizer.exe set vfp export --quick .\ws\vfp-quick.csv
```

| Parameter   | Shorthand | Description                                             |
|-------------|-----------|---------------------------------------------------------|
| `<OUTPUT>`  | —         | Output path (`-` for stdout)                            |
| `--tabs`    | `-t`      | Use Tab as delimiter (default comma)                    |
| `--quick`   | `-q`      | Skip dynamic load measurement, export static curve only |
| `--nocheck` | `-n`      | Skip plausibility check for dynamic results             |

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

#### set vfp import

Write modified curve from CSV to GPU.

```bat
nvoc-auto-optimizer.exe set vfp import .\ws\vfp.csv
```

| Parameter | Shorthand | Description                |
|-----------|-----------|----------------------------|
| `<INPUT>` | —         | Input path (`-` for stdin) |
| `--tabs`  | `-t`      | Tab delimiter              |

---

#### set nvapi lock / reset

Lock GPU voltage to VFP point or specific voltage, or release VFP lock.

```bat
nvoc-auto-optimizer.exe set --nvapi-locked-voltage 68
nvoc-auto-optimizer.exe set --nvapi-locked-voltage 850mV
nvoc-auto-optimizer.exe set --nvapi-locked-voltage 850000uV
nvoc-auto-optimizer.exe set --nvapi-locked-core-clocks 210 2100
nvoc-auto-optimizer.exe set --nvapi-locked-mem-clocks 5000 9501
nvoc-auto-optimizer.exe set --nvapi-reset-core-clocks
nvoc-auto-optimizer.exe set --nvapi-reset-mem-clocks
nvoc-auto-optimizer.exe set --nvapi-reset-vfp-locks
```

| Parameter                                   | Description                                                         |
|---------------------------------------------|---------------------------------------------------------------------|
| `--nvapi-locked-voltage <POINT_OR_VOLTAGE>` | Bare number as point; voltage must have explicit unit (`mV` / `uV`) |
| `--nvapi-locked-core-clocks <MIN> <MAX>`    | Lock NVAPI Graphics core frequency range (MHz)                      |
| `--nvapi-locked-mem-clocks <MIN> <MAX>`     | Lock NVAPI Memory frequency range (MHz)                             |
| `--nvapi-reset-core-clocks`                 | Release NVAPI core frequency lock                                   |
| `--nvapi-reset-mem-clocks`                  | Release NVAPI memory frequency lock (alias: `--pstate-unlock`)      |
| `--nvapi-reset-vfp-locks`                   | Clear NVAPI VFP lock (voltage / core/memory clock lock)             |

---

#### set vfp autoscan

**Core Function**: Perform full VFP curve auto-scanning on current GPU.

```bat
nvoc-auto-optimizer.exe set vfp autoscan
nvoc-auto-optimizer.exe set vfp autoscan -u
nvoc-auto-optimizer.exe set vfp autoscan -u -b aggressive
```

| Parameter     | Shorthand | Default             | Description                                                         |
|---------------|-----------|---------------------|---------------------------------------------------------------------|
| `--ultrafast` | `-u`      | Off                 | Enable ultrafast mode (scan 4 key points, interpolation for others) |
| `-w <path>`   | —         | `./test/test.bat`   | Path to pressure test script                                        |
| `-l <path>`   | —         | `./ws/vfp.log`      | Path to scan log                                                    |
| `-q <seq>`    | —         | `-`                 | Custom scan point sequence (`-` for auto)                           |
| `-t <count>`  | —         | `30`                | Timeout detection loop count per test                               |
| `-o <path>`   | —         | `./ws/vfp-tem.csv`  | Path for real-time per-point results CSV                            |
| `-i <path>`   | —         | `./ws/vfp-init.csv` | Path for reference original curve CSV                               |
| `-m`          | —         | Off                 | Scan video memory OC simultaneously                                 |
| `-b <method>` | —         | Auto per GPU gen    | Recovery method: `aggressive` (BSOD reboot) or `traditional` (TDR)  |

---

#### set vfp autoscan_legacy

Global offset auto-scanning for Maxwell (GTX 9 series) and earlier GPUs.

```bat
nvoc-auto-optimizer.exe set vfp autoscan_legacy
nvoc-auto-optimizer.exe set vfp autoscan_legacy -b aggressive
```

Parameters mostly same as `autoscan`, but without `--ultrafast`, `-m` (memory scan), `-q` (sequence), or `-i` (initial curve) support, as Legacy mode only has single global offset.

---

#### set vfp fix_result

Apply **light/heavy load compensation post-processing** to `autoscan` temporary CSV to generate final stable curve.

```bat
nvoc-auto-optimizer.exe set vfp fix_result -m 1
nvoc-auto-optimizer.exe set vfp fix_result -m 1 -u
```

| Parameter     | Shorthand | Description                                                                              |
|---------------|-----------|------------------------------------------------------------------------------------------|
| `-m <int>`    | —         | Extra conservative offset bins (Recommend `1`, reduce 1 step extra on top of margin fix) |
| `--ultrafast` | `-u`      | Off                                                                                      | Complete interpolation for 4 key point ultrafast results |
| `-v <path>`   | —         | `./ws/vfp-tem.csv`                                                                       | Input: temporary CSV from autoscan |
| `-o <path>`   | —         | `./ws/vfp.csv`                                                                           | Output: final compensated curve CSV |
| `-i <path>`   | —         | `./ws/vfp-init.csv`                                                                      | Ref: factory original curve CSV |
| `-l <path>`   | —         | `./ws/vfp.log`                                                                           | Scan log (for reading ultrafast key points) |
| `-d <int>`    | —         | `3`                                                                                      | Ref frequency delta bin (internal param) |

---

#### set vfp single_point_adj

Manually set a single VFP point to specified frequency offset for debugging.

```bat
nvoc-auto-optimizer.exe set vfp single_point_adj -s 50 -d 150000
```

| Parameter    | Shorthand | Default  | Description            |
|--------------|-----------|----------|------------------------|
| `-s <index>` | —         | `50`     | Start point index      |
| `-d <kHz>`   | —         | `150000` | Frequency offset (kHz) |

---

## Detailed Scanning Process

### Phase 0: Preparation

`start.bat` sequentially executes before calling `autoscan`:

1. `info`: Print GPU info and identify generation.
2. `reset pstate`: Zero P-State frequency offsets to ensure a clean starting point.
3. `reset vfp` (or `reset --domain vfp --vfp-domain all`): Zero all VFP curve point offsets.
   - Use `reset --domain vfp --vfp-domain core` for core OC only.
   - Use `reset --domain vfp --vfp-domain memory` for memory OC only.
4. `set --nvapi-reset-vfp-locks`: Release voltage/frequency locks.
5. `set vfp export .\ws\vfp-init.csv` (if it doesn't exist): Save factory original curve as reference.
6. Add firewall rules for pressure test executable (optional, to avoid network activity affecting the test).

### Phase 1: Voltage Range Probing

`autoscan` starts by calling `handle_test_voltage_limits`. Starting from generation-specific preset points, it probes the actual usable voltage range of the GPU by advancing the voltage lock (`handle_lock_vfp`) step by step:

- **Upper Bound Probe**: Advance upward point by point from the preset upper point until locking fails (the curve reaches a flat region or goes out of range).
- **Lower Bound Probe**: Advance downward from the preset lower point to find the minimum usable voltage point.

Results recorded in `vfp.log` (`minimum_voltage_point` / `maximum_voltage_point`), skipped on resume scanning by reading the log directly.

### Phase 2: Point-by-point Core Frequency Scanning

For the range `[lower_voltage_point, upper_voltage_point]`, scan every 3 points in standard mode, or use 4 key points in ultrafast mode. At each voltage point, run a two-stage binary search:

**Short Test**: use exponential stepping (`2^n × minimum step`) to quickly converge close to the stable upper frequency limit.

**Long Test**: based on the value found in the short test, perform single-step endurance validation to confirm true stability.

During each round of testing, the tool periodically applies **frequency fluctuations**: it raises or lowers the current set frequency periodically to simulate dynamic workload changes and prevent the GPU from passing at a static frequency but crashing when switching in practice. A watchdog also monitors whether the stress-test process is still alive to prevent the scan from hanging.

If thermal / power throttling is detected (`thrm_or_pwr_limit_flag`), ~~the test resolution will be lowered automatically to reduce GPU load~~ (TODO since the load was changed to CLI-based testing), ensuring the result reflects overclocking capability rather than TDP bottlenecks.

After each voltage point finishes, the result is appended to `vfp-tem.csv` in real time and recorded in `vfp.log` (resume scanning supported).

### Phase 3: Video Memory Frequency Scanning (Optional)

Enabled with `-m`. After core scan finishes, the tool locks at the highest voltage point and scans the memory OC ceiling using a similar binary-search approach.

### Phase 4: fix_result Post-processing

`start.bat` automatically calls after `autoscan`:

```bat
nvoc-auto-optimizer.exe set vfp fix_result -m 1
```

**Principle**: Since Pascal (10 series), NVIDIA V-F curves exhibit a small difference between light load and heavy load — similar to CPU Load-Line Calibration (LLC). If the stable maximum frequency found under full load is written back without correction, instability may occur when the operating point shifts during load transitions.

fix_result uses `vfp-init.csv` dynamic measurement's `margin_bin` (difference between loaded frequency and static default frequency, converted to step bins) to conservatively adjust each point from `autoscan`:
- `margin_bin > 5`: reduce by `(5 + minus_bin)` steps.
- `|margin_bin| < 2`: reduce by `(1 + minus_bin)` steps.
- otherwise: reduce by `(|margin_bin| + 1 + minus_bin)` steps.

In ultrafast mode, the missing voltage points between the 4 key points are first filled in by linear interpolation.

### Phase 5: Import and Export Final Curve

```bat
nvoc-auto-optimizer.exe set vfp import .\ws\vfp.csv
nvoc-auto-optimizer.exe set vfp export .\ws\vfp-final.csv
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
set vfp autoscan -b traditional    # force traditional
set vfp autoscan -b aggressive     # force aggressive
```

---

## Breakpoint Resumption

Results are appended to `ws\vfp.log` after each voltage point finishes. Next run, the tool parses:

- `minimum_voltage_point` / `maximum_voltage_point`: skip probing.
- Last `Finished core OC on point` entry: continue from next point.
- Last success/fail offsets: restore binary search convergence directly.

Whether exit, manual interrupt, or crash, **just rerun `start.bat` to resume**.

To start over, run `start.bat 1` to clear log.

---

## Legacy GPU Mode (Maxwell / Pascal)

NVAPI for GTX 9 series (Maxwell) lacks point-by-point VFP writing; uses global P0 offset via `autoscan_legacy`:

1. Write global offset through `set_pstates` (`ClockDomain::Graphics`, `PState::P0`).
2. Scan max stable offset with binary search + endurance test.
3. Not supported: ultrafast interpolation, memory segment scans, per-point fix_result.

For voltage control, Maxwell uses `SetPstates20` `baseVoltages` (`--voltage-delta` in μV) instead of VoltRails boost.

Legacy OverVolt often fails in practice; VBIOS editing often recommended for pre-900 series for max performance.

---

## Working Files Description

| File               | Purpose               | Notes                                                |
|--------------------|-----------------------|------------------------------------------------------|
| `ws\vfp.log`       | Scan log              | Basis for resumption; deleting restarts from scratch |
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

**BSOD / black screen / Linux drop / ERR during scanning is normal** — indicates frequency limit exceeded. Tool will backtrack. If system freezes > 3 min, force reboot and rerun `start.bat`. On Linux, exit GPU-using processes (including GUI) and run `linux_oc_recover.sh` to reset; reboot if deadlocked. Recovery tolerance on Linux lower than Windows TDR; save work.

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
- archives include `LICENSE`.

---

*nvoc-auto-optimizer v0.0.3 — by Skyworks*
