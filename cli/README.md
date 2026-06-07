# nvoc-cli

`nvoc-cli` is a focused command-line wrapper over `nvoc-core`.

## Compatibility Overview (Interface × GPU Generation / Basic Functions)

This matrix is the canonical compatibility reference for NVOC backend commands exposed by `nvoc-cli`. Autoscan-specific rows describe the `auto-optimizer` flows that depend on the same NVAPI/NVML backend capabilities.

Note: Testing on Linux NV proprietary drivers shows that NVAPI on Linux is essentially a compatibility layer for /lib/x86_64-linux-gnu/libnvidia-api.so.1, directly leading to /lib/x86_64-linux-gnu/libnvidia-ml.so.1. In other words, on Linux, only the NVML interface actually exists. However, due to NVAPI's "translation" of NVML, for professional cards (such as P100, V100, etc.)—since NVOC's primary GPU ID index uses the NVAPI interface—support on Linux may be better than on Windows.
Additionally, the core frequency range lock of NVML is actually a voltage range lock at the bottom. We have verified through dynamic adjustment—after adjusting the frequency offset, the frequency range lock still takes effect within the working point of the voltage range corresponding to the frequency range at the time of setting, rather than the original frequency range.

### **NVAPI Interface + Desktop Consumer GPUs:**

|                      Function                      | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) |  GTX 9/Part 7 (GM)  |                          Remarks                           |
|:--------------------------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:-------------------:|:----------------------------------------------------------:|
|               VF curve Edit + Export               |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|                  Auto OC autoscan                  |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ❌          |                                                            |
|           Legacy Auto OC autoscan-vfp-legacy           |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|      Core Frequency Offset (`--core-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          |                                                            |
|      Memory Frequency Offset (`--mem-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |          ✅          | Preserves Graphics VFP curve (restores per-point deltas) |
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
|           Legacy Auto OC autoscan-vfp-legacy           |    TODO     |    TODO     |    TODO     |     TODO      |     TODO     |     TODO     |       TODO        | Requires new algorithm based on frequency lock and offset  |
|      Core Frequency Offset (`--core-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         |                                                            |
|      Memory Frequency Offset (`--mem-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |         ✅         | NVML has no VFP curve |
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
|               VF curve Edit + Export               |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|                  Auto OC autoscan                  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|      Core Frequency Offset (`--core-offset`)       |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|      Memory Frequency Offset (`--mem-offset`)      |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|            Power Wall (`--power-limit`)            |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ✅       |  Usually locked by driver, needs NVML attempt   |
|        Temperature Wall (`--thermal-limit`)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❓       |   Cases where return code 0 but not effective   |
|          Fan Speed Control (NVAPI cooler)          |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | No onboard fan or motherboard/system controlled |
|       Voltage Point Lock (--locked-voltage)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|  Core Frequency Range Lock (--locked-core-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Core Frequency Range Unlock (--reset-core-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Lock (--locked-mem-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Unlock (--reset-mem-clocks) |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | Diff driver model + Linux has NVAPI translation |
|     Boost Voltage Voltboost (--voltage-boost)      |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|             Overvolt (--voltage-delta)             |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |

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
|               VF curve Edit + Export               |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|                  Auto OC autoscan                  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|           Legacy Auto OC autoscan-vfp-legacy           |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|      Core Frequency Offset (`--core-offset`)       |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|      Memory Frequency Offset (`--mem-offset`)      |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|            Power Wall (`--power-limit`)            |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ✅       |                                                 |
|        Temperature Wall (`--thermal-limit`)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❓       |   Cases where return code 0 but not effective   |
|          Fan Speed Control (NVML cooler)           |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | No onboard fan or motherboard/system controlled |
|           Fan Speed Reset (NVML cooler)            |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | No onboard fan or motherboard/system controlled |
|       Voltage Point Lock (--locked-voltage)        |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|      Voltage Point Unlock (reset-volt-locks)       |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       |                                                 |
|  Core Frequency Range Lock (--locked-core-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Core Frequency Range Unlock (--reset-core-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       | ✅ (Windows❌) | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Lock (--locked-mem-clocks)  |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | Diff driver model + Linux has NVAPI translation |
| Memory Frequency Range Unlock (--reset-mem-clocks) |       ❓        |      ❓       |      ❓       |      ❓       |      ❓       |      ❌       | Diff driver model + Linux has NVAPI translation |
|   App Frequency Range Lock (--locked-app-clocks)   |       ❓        |      ❓       |      ❓      |       ❓       |       ❓       |      ❌       |                                                            |
|  App Frequency Range Unlock (--reset-app-clocks)   |       ❓        |      ❓       |      ❓      |       ❓       |       ❓       |      ❌       |                                                            |
|     Boost Voltage Voltboost (--voltage-boost)      |       ❓        |      ❓       |      ❓      |       ❓       |       ❓       |      ❌       |                                                            |
|             Overvolt (--voltage-delta)             |       ❓        |      ❓       |      ❓      |       ❓       |       ❓       |      ❌       |                                                            |

### **NVML Interface + Workstation Grade GPUs:**

|                      Function                      | Blackwell (GB) | Ada (AD) | Ampere (GA) | Turing (TU) | Pascal (GP) | Remarks |
|:--------------------------------------------------:|:--------------:|:--------:|:-----------:|:-----------:|:-----------:|:-------:|
|               VF curve Edit + Export               |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
|                  Auto OC autoscan                  |      TODO      |   TODO   |    TODO     |    TODO     |    TODO     |         |
|           Legacy Auto OC autoscan-vfp-legacy           |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |
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
|             Overvolt (--voltage-delta)             |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |         |

### Mobile GPUs / Workstation GPUs / Server GPUs:

Generally do not support **Boost Voltage, fan-related controls, temperature wall related controls, power wall related controls**.

## Usage

```text
nvoc-cli [--gpu GPU_ID] [--nvapi|--nvml] [--output human|json] <function-name> [args] [named args]
```

Named arguments can be placed before or after the function name. Use
`nvoc-cli <function-name> --help` to see the named arguments supported by a
specific function.

Examples:

```text
nvoc-cli get-vfp --gpu 0
nvoc-cli --domain memory get-vfp --output json
nvoc-cli --nvml get-power-watt
nvoc-cli set-core-offset-mhz 150 --gpu 0
nvoc-cli set-locked-clocks-mhz 210 2100 --domain core
```

When neither `--nvapi` nor `--nvml` is provided, commands that support both
backends try NVAPI first and fall back to NVML if the NVAPI attempt fails.
