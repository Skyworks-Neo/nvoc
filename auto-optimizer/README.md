# nvoc-auto-optimizer

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](../LICENSE)


[中文](./README.md) | [English](./README-en.md)

本项目采用 [Apache License 2.0](../LICENSE) 许可发布，英文版说明见 `README-en.md`。

> **NVIDIA GPU VF 曲线自动超频优化器**  
> 基于 Rust 编写，通过 NVAPI / NVML 接口操控 GPU，配合 `cli-stressor` 压力测试进行逐点电压-频率（V-F Curve）自动扫描，为
> NVIDIA GPU 找出每个电压点的稳定超频上限并生成最优化曲线。

## nvoc-auto-optimizer——真正懂超频的开发的N卡超超爆妙妙工具

## 目录

- [背景与原理](#背景与原理)
- [支持的 GPU 世代](#支持的-gpu-世代)
- 兼容性一览（接口 × GPU 世代 / 基础功能）
- [依赖与环境要求](#依赖与环境要求)
- [目录结构](#目录结构)
- [快速开始](#快速开始)
- [命令参考](#命令参考)
  - [全局参数](#全局参数)
  - [命令](#命令)
    - [export-vfp](#export-vfp)
    - [import-vfp](#import-vfp)
    - [sync-vfp-memory-pstate](#sync-vfp-memory-pstate)
    - [nvoc-cli VFP lock / reset](#nvoc-cli-vfp-lock--reset)
    - [autoscan-vfp](#autoscan-vfp)
    - [autoscan-vfp-legacy](#autoscan-vfp-legacy)
    - [fix-vfp-result](#fix-vfp-result)
    - [nvoc-cli set-vfp-point-delta-mhz](#nvoc-cli-set-vfp-point-delta-mhz)
- [扫描流程详解](#扫描流程详解)
  - [阶段 0：准备工作](#阶段-0准备工作)
  - [阶段 1：电压范围探测](#阶段-1电压范围探测)
  - [阶段 2：逐点核心频率扫描](#阶段-2逐点核心频率扫描)
  - [阶段 3：显存频率扫描（可选）](#阶段-3显存频率扫描可选)
  - [阶段 4：fix_result 后处理](#阶段-4fix_result-后处理)
  - [阶段 5：导入并导出最终曲线](#阶段-5导入并导出最终曲线)
- [Ultrafast 模式](#ultrafast-模式)
- [崩溃恢复机制](#崩溃恢复机制)
- [断点续扫](#断点续扫)
- [Legacy GPU 模式（Maxwell / Pascal）](#legacy-gpu-模式maxwell--pascal)
- [工作文件说明](#工作文件说明)
- [测试环境建议](#测试环境建议)
- [从源码构建](#从源码构建)
- [免责声明](#免责声明)

---

## 背景与原理

### 什么是 V-F 曲线？

NVIDIA 从 Pascal（10 系）起引入了 **GPU Boost 3.0**，其核心是一张电压-频率（Voltage-Frequency，简称 VFP）查找表：GPU 会根据当前核心电压，在表中查出对应的目标频率，并在运行时实时跟踪。  
出厂默认曲线是 NVIDIA 针对最差硅片留有充裕余量后标定的保守值。不同晶圆批次、不同硅片个体的实际稳定极限差异很大，优质硅片的真实可用频率往往远高于出厂值。

### 超频的本质

超频本质是对 VFP 表中每个电压点施加一个正频率偏移（`KilohertzDelta`），使该电压点对应的目标频率提高。偏移过大时，芯片内部时序违例，GPU 会触发 TDR（Timeout Detection and Recovery）恢复或直接崩溃。  
本工具的目标是：**对曲线上每个电压点，找出该电压下能稳定通过压力测试的最大频率偏移值**。

### 压力测试负载：cli-stressor

`cli-stressor` 用于在给定电压/频率下提供稳定性压力。判据采用**进程返回码**：返回 `0` 视为通过，非 `0` 视为失败。

> **警告**：不要将 OpenCL stressor 作为 autoscan 结果的最终通过判据。它的压力不足，OpenCL-only 通过可能让扫描接受高于硬件真实稳定上限的频率偏移。直接应用这些偏高结果可能导致驱动重置、系统不稳定、数据损坏或硬件故障；请将其视为临时结果，并使用 CUDA stressor 或更重的真实负载重新验证。

### Maxwell / 9 系 Legacy 模式

Maxwell（GM 代号，9xx 系列）及更早的 GPU 不支持逐点 V-F 曲线写入，只能通过 `SetPstates20` 对 P0 状态施加全局频率偏移。本工具对这类 GPU 使用 `autoscan-vfp-legacy` 流程，仅扫描单一全局偏移值。

---

## 支持的 GPU 世代

| 世代                     | 代号前缀   | 模式              | 说明                                    |
|------------------------|--------|-----------------|---------------------------------------|
| RTX 50 系（Blackwell）    | `GB`   | VF 曲线           | 默认使用 aggressive BSOD 恢复；支持 Max-Q 阶梯标定 |
| RTX 40 系（Ada Lovelace） | `AD`   | VF 曲线           | —                                     |
| RTX 30 系（Ampere）       | `GA`   | VF 曲线           | —                                     |
| RTX 20 系（Turing TU10x） | `TU10` | VF 曲线（轻重载差异小）   | —                                     |
| GTX 16 系（Turing TU11x） | `TU11` | VF 曲线（轻重载差异小）   | —                                     |
| GTX 10 系（Pascal）       | `GP1`  | VFP 曲线（79 点）    | 轻重载差异小；fix_result 仍推荐执行               |
| GTX 9 系（Maxwell）       | `GM`   | **Legacy 全局偏移** | 不支持逐点 VFP；通过 `autoscan-vfp-legacy` 扫描     |
| Volta 计算卡              | `GV`   | Legacy          | 同上                                    |

> **移动端 GPU**（名称包含 `Laptop`）无法修改 TDP / 温度墙 / VDDQ boost，工具在检测到时会自动跳过这些设置。

---

<h2 id="compatibility-interfaces-gpu-generations">兼容性一览（接口 × GPU 世代 / 基础功能）</h2>

注意：在 Linux NV proprietary 驱动测试的结果是，Linux 上 NVAPI 本质是一个 /lib/x86_64-linux-gnu/libnvidia-api.so.1的兼容层，直接导向 /lib/x86_64-linux-gnu/libnvidia-ml.so.1；
换言之，在 Linux 上，实际上只存在 NVML 接口。但由于 NVAPI “转译” NVML的特性，对于专业卡（如 P100 V100等）——由于本项目的主 GPU ID 索引采用 NVAPI 接口——会出现 Linux 上的支持性大于 Windows 的情况。
此外，NVML 的核心频率范围锁定实际上底层是电压范围锁定，我们已经通过动态调节进行了验证——在调节频率偏置后，频率范围锁定实际仍然在设定时生效频率范围对应的电压范围工作点内生效，而非原始频率范围。

### **NVAPI接口+桌面消费级GPU:**

|                 功能                 | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9/部分7 (GM) |              备注              |
|:----------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:--------------:|:----------------------------:|
|           VF curve 编辑+导出           |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|            自动超频autoscan            |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|       旧版自动超频autoscan-vfp-legacy        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      核心频率偏置 (`--core-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      显存频率偏置 (`--mem-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        | 保留 Graphics VFP 曲线（回写逐点偏置） |
|        功耗墙(`--power-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|       温度墙(`--thermal-limit`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速控制(NVAPI cooler)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速重置(NVAPI cooler)        | ✅  (Linux❓) | ✅  (Linux❓) | ✅  (Linux❓) |  ✅  (Linux❌)  |  ✅  (Linux❌)  | ✅  (Linux❌)  |  ✅  (Linux❌)   |                              |
|      电压点锁定 (--locked-voltage)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|      电压点解锁 (reset-volt-locks)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|  核心频率范围锁定 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|   核心频率范围解锁 (--reset-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|   显存频率范围锁定 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |      部分支持      |                              |
|   显存频率范围解锁 (--reset-mem-clocks)    |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |      部分支持      |                              |
| Boost电压Voltboost( --voltage-boost) |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|    过电压Overvolt(--voltage-delta)    |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ✅        |                              |

### **NVML接口+桌面消费级GPU:**

|                 功能                 | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9/部分7 (GM) |              备注              |
|:----------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:--------------:|:----------------------------:|
|           VF curve 编辑+导出           |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|            自动超频autoscan            |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |       ❌        |      需要基于频率锁定和偏置进行新的算法       |
|       旧版自动超频autoscan-vfp-legacy        |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |      TODO      |      需要基于频率锁定和偏置进行新的算法       |
|      核心频率偏置 (`--core-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      显存频率偏置 (`--mem-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        | NVML 无 VFP 曲线 |
|        功耗墙(`--power-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|       温度墙(`--thermal-limit`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速控制(NVML cooler)         |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速重置(NVML cooler)         |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      电压点锁定 (--locked-voltage)      |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|      电压点解锁 (--locked-voltage)      |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|  核心频率范围锁定 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❓        |                              |
|   核心频率范围解锁 (--reset-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❓        |                              |
|   显存频率范围锁定 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|   显存频率范围解锁 (--reset-mem-clocks)    |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|  应用频率范围锁定 ( --locked-app-clocks)   |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|   应用频率范围解锁 (--reset-app-clocks)    |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
| Boost电压Voltboost( --voltage-boost) |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|    过电压Overvolt(--voltage-delta)    |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |

### **NVAPI接口+服务器级GPU:**

|                 功能                 | Blackwell (GB) |  Hopper (GH)  |  Ampere (GA)  |  Turing (GT)  |  Volta (GV)   |  Pascal (GP)  |            备注            |
|:----------------------------------:|:--------------:|:-------------:|:-------------:|:-------------:|:-------------:|:-------------:|:------------------------:|
|           VF curve 编辑+导出           |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|            自动超频autoscan            |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|      核心频率偏置 (`--core-offset`)      |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|      显存频率偏置 (`--mem-offset`)       |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|        功耗墙(`--power-limit`)        |       ✅        |       ✅       |       ✅       |       ✅       |       ✅       |       ✅       |    一般由驱动锁定，需通过NVML尝试     |
|       温度墙(`--thermal-limit`)       |       ❓        |       ❓       |       ❓       |       ❓       |       ❓       |       ❓       |  有return code 0但不生效的情况   |
|        风扇转速控制(NVAPI cooler)        |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |      无板载风扇或由主板/系统控制      |
|      电压点锁定 (--locked-voltage)      |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|  核心频率范围锁定 (--locked-core-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   核心频率范围解锁 (--reset-core-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   显存频率范围锁定 (--locked-mem-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) |       ❌       |       ❌       |       ❌       | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   显存频率范围解锁 (--reset-mem-clocks)    | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) |       ❌       |       ❌       |       ❌       | 驱动模型不同+ Linux 有 NVAPI 转译 |
| Boost电压Voltboost( --voltage-boost) |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |

### **NVAPI接口+工作站级GPU:**

|                 功能                 | Blackwell (GB) | Ada (AD) | Ampere (GA) | Turing (TU) | Pascal (GP) | 备注 |
|:----------------------------------:|:--------------:|:--------:|:-----------:|:-----------:|:-----------:|:--:|
|           VF curve 编辑+导出           |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |    |
|            自动超频autoscan            |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |    |
|      核心频率偏置 (`--core-offset`)      |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |    |
|      显存频率偏置 (`--mem-offset`)       |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |    |
|        功耗墙(`--power-limit`)        |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |    |
|       温度墙(`--thermal-limit`)       |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |    |
|        风扇转速控制(NVAPI cooler)        |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |    |
|      电压点锁定 (--locked-voltage)      |       ✅        |    ✅     |      ✅      |      ❓      |      ❓      |    |
|  核心频率范围锁定 (--locked-core-clocks)   |       ✅        |    ✅     |      ✅      |      ✅      |      ✅      |    |
|   显存频率范围锁定 (--locked-mem-clocks)   |       ✅        |    ✅     |      ✅      |      ❓      |      ❌      |    |
| Boost电压Voltboost( --voltage-boost) |       ❌        |    ❌     |      ❌      |      ❌      |      ❌      |    |

### **NVML接口+服务器级GPU:**

|                 功能                 | Blackwell (GB) |  Hopper (GH)  |  Ampere (GA)  |  Turing (GT)  |  Volta (GV)   |  Pascal (GP)  |            备注            |
|:----------------------------------:|:--------------:|:-------------:|:-------------:|:-------------:|:-------------:|:-------------:|:------------------------:|
|           VF curve 编辑+导出           |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|            自动超频autoscan            |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|       旧版自动超频autoscan-vfp-legacy        |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|      核心频率偏置 (`--core-offset`)      |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|      显存频率偏置 (`--mem-offset`)       |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|        功耗墙(`--power-limit`)        |       ✅        |       ✅       |       ✅       |       ✅       |       ✅       |       ✅       |                          |
|       温度墙(`--thermal-limit`)       |       ❓        |       ❓       |       ❓       |       ❓       |       ❓       |       ❓       |  有return code 0但不生效的情况   |
|        风扇转速控制(NVML cooler)         |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |      无板载风扇或由主板/系统控制      |
|        风扇转速重置(NVML cooler)         |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |      无板载风扇或由主板/系统控制      |
|      电压点锁定 (--locked-voltage)      |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|      电压点解锁 (--locked-voltage)      |       ❌        |       ❌       |       ❌       |       ❌       |       ❌       |       ❌       |                          |
|  核心频率范围锁定 (--locked-core-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   核心频率范围解锁 (--reset-core-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | ✅  (Windows❌) | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   显存频率范围锁定 (--locked-mem-clocks)   | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) |       ❌       |       ❌       |       ❌       | 驱动模型不同+ Linux 有 NVAPI 转译 |
|   显存频率范围解锁 (--reset-mem-clocks)    | ✅  (Windows❌)  | ✅  (Windows❌) | ✅  (Windows❌) |       ❌       |       ❌       |       ❌       | 驱动模型不同+ Linux 有 NVAPI 转译 |
|  应用频率范围锁定 ( --locked-app-clocks)   |       ❌        |      ❌       |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|   应用频率范围解锁 (--reset-app-clocks)    |       ❌        |      ❌       |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
| Boost电压Voltboost( --voltage-boost) |       ❌        |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|    过电压Overvolt(--voltage-delta)    |       ❌        |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |


### **NVML接口+工作站级GPU:**

|                 功能                 | Blackwell (GB) | Ada  (AD) | Ampere (GA) | Turing (TU) | Pascal (GP) | 备注 |
|:----------------------------------:|:--------------:|:---------:|:-----------:|:-----------:|:-----------:|:--:|
|           VF curve 编辑+导出           |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|            自动超频autoscan            |      TODO      |   TODO    |    TODO     |    TODO     |    TODO     |    |
|       旧版自动超频autoscan-vfp-legacy        |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|      核心频率偏置 (`--core-offset`)      |       ❓        |     ❓     |      ❓      |      ❓      |      ❓      |    |
|      显存频率偏置 (`--mem-offset`)       |       ❓        |     ❓     |      ❓      |      ❓      |      ❓      |    |
|        功耗墙(`--power-limit`)        |       ✅        |     ✅     |      ✅      |      ✅      |      ✅      |    |
|       温度墙(`--thermal-limit`)       |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|        风扇转速控制(NVML cooler)         |       ✅        |     ✅     |      ✅      |      ✅      |      ✅      |    |
|        风扇转速重置(NVML cooler)         |       ✅        |     ✅     |      ✅      |      ✅      |      ✅      |    |
|      电压点锁定 (--locked-voltage)      |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|      电压点解锁 (--locked-voltage)      |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|  核心频率范围锁定 (--locked-core-clocks)   |       ✅        |     ✅     |      ✅      |      ✅      |      ✅      |    |
|   核心频率范围解锁 (--reset-core-clocks)   |       ✅        |     ✅     |      ✅      |      ✅      |      ✅      |    |
|   显存频率范围锁定 (--locked-mem-clocks)   |       ✅        |     ✅     |      ✅      |      ❌      |      ❌      |    |
|   显存频率范围解锁 (--reset-mem-clocks)    |       ✅        |     ✅     |      ✅      |      ❌      |      ❌      |    |
|  应用频率范围锁定 ( --locked-app-clocks)   |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|   应用频率范围解锁 (--reset-app-clocks)    |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
| Boost电压Voltboost( --voltage-boost) |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |
|    过电压Overvolt(--voltage-delta)    |       ❌        |     ❌     |      ❌      |      ❌      |      ❌      |    |

### 移动端 GPU / 工作站 GPU / 服务器 GPU ：

一般不支持**Boost电压、风扇相关控制、温度墙相关控制、功耗墙相关控制**

## 依赖与环境要求

### 运行时依赖

| 依赖                                                    | 说明                                                                                                                          |
|:------------------------------------------------------|:----------------------------------------------------------------------------------------------------------------------------|
| Windows 10/11 或任一 Linux 发行版                           | 工具通过 NVAPI/NVML 调用，支持 Windows 10/11 和*任何使用 nvidia-open-dkms 的 linux 发行版*（理论上，已测试 ArchLinux + KDE、Ubuntu22.04 、Debian 12/13） |
| NVIDIA 驱动（≥ 537，Linux用nvidia-open-dkms 或 proprietary） | 需支持目标 GPU 的 NVAPI/NVML 接口，已知395版本完全不行，因此难以支持驱动太老的kepler和之前的GPU                                                              |
| `cli-stressor` 压力测试运行脚本（自动超频用）                        | Windows 默认调用 `test\test_cuda_windows.bat`，Linux 默认调用 `test/test_cuda_linux.sh`                                           |
| Administrator权限/Sudo权限                                | 超频参数写入接口需要管理权限，而大部分读取不需要                                                                                                    |

### 压测脚本位置（默认）

```
auto-optimizer/
├── test/
│   ├── test.bat
│   ├── test_cuda_windows.bat
│   ├── test_cuda_linux.sh
│   ├── cli-stressor-cuda-rs-minload.sh
│   └── dyn_load_export_cuda_rs_linux.sh
└── ws/
```

### 编译依赖（仅从源码构建时需要）

- Rust 工具链
- 目标架构对应构建工具
- Linux 默认 CUDA RS autoscan 需要带 CUDA 支持的 release 二进制；使用默认 minload/Vulkan 配置时请构建：
  `cargo build --release -p cli-stressor-cuda-rs --features cuda,vulkan`

---

## 目录结构

```
auto-optimizer/
├── src/
│   ├── main.rs               # 入口，命令路由
│   ├── arg_help.rs           # clap 命令行参数定义
│   ├── basic_func.rs         # GPU 世代检测、分辨率工具、handle_info/list/status/get/reset
│   ├── nvidia_gpu_type.rs     # GPU 世代识别与分型参数
│   ├── oc_get_set_function_nvapi.rs # NVAPI: VFP lock/reset、cooler、电压/频率设置
│   ├── oc_get_set_function_nvml.rs  # NVML: 功耗墙、时钟锁定、P-State 锁定
│   ├── oc_profile_function.rs# VFP export/import、fix_result、autoscan 辅助函数
│   ├── oc_scanner.rs         # autoscan_gpuboostv3 / legacy scanner 核心扫描循环
│   ├── autoscan_config.rs    # 扫描参数结构体（统一解析 ArgMatches）
│   ├── types.rs              # OutputFormat、ResetSettings、VfpResetDomain 枚举
│   ├── conv.rs               # 枚举字符串互转
│   ├── error.rs              # 统一错误类型
│   ├── human.rs              # 人类可读输出格式化
│   └── lib.rs
├── ws/                       # 运行时自动创建，存放扫描中间文件
│   ├── vfp.log               # 扫描过程日志（断点续扫依赖此文件）
│   ├── vfp-init.csv          # 首次扫描前导出的出厂原始曲线
│   ├── vfp-tem.csv           # autoscan 每点实时保存的临时结果
│   └── vfp.csv / vfp-final.csv  # fix_result 后处理结果 / 最终导出确认文件
├── test/
│   └── test.bat              # 调用 cli-stressor 的压力测试封装脚本
├── start.bat                 # 标准扫描启动脚本
├── start_ultrafast.bat       # 超快速扫描启动脚本
├── start_legacy.bat          # Legacy GPU 扫描启动脚本
├── GpuTdrRecovery.reg        # TDR 恢复相关注册表
├── recover.bat               # 手动恢复脚本
└── Cargo.toml
```

---

## 快速开始

### 标准扫描（推荐，适用于 RTX 20 系及以上）

以**管理员身份**运行：

```bat
start.bat
```

脚本会自动：
1. 检测并列出系统中的 GPU，提示输入目标 GPU ID
2. 重置 VFP 曲线并解锁电压锁定
3. 导出并保存出厂原始曲线（`ws\vfp-init.csv`）
4. 执行 autoscan 扫描全部电压点
5. 执行 fix_result 轻重载补偿后处理
6. 将最优曲线导入 GPU 并导出最终文件（`ws\vfp-final.csv`）

### 超快速扫描（推荐用于节省时间）

```bat
start_ultrafast.bat
```

仅扫描 4 个关键电压点，其余点线性插值，速度显著加快。详见 [Ultrafast 模式](#ultrafast-模式)。

### Legacy GPU 扫描（GTX 9 系及之前 / 目前仅测试了 Maxwell）

```bat
start_legacy.bat
```

使用全局 P0 频率偏移扫描，不写入 VFP 曲线。

### 重新扫描（清除历史记录）

若需从头开始（丢弃断点续扫状态）：

```bat
start.bat 1
```

传入参数 `1` 将清空 `ws\vfp.log` 和 `ws\vfp-tem.csv`。

---

## 命令参考

`nvoc-auto-optimizer` 现在只保留 VFP 优化工作流命令。会修改 GPU 状态的命令需以管理员/root 权限运行：

```text
nvoc-auto-optimizer.exe [--gpu GPU_ID] [--no-color] <command> [command options]
```

GPU 发现、状态查询、通用超频写入、风扇控制、功耗限制、锁定和通用重置操作请使用 `nvoc-cli`，例如 `nvoc-cli list-gpus`、`nvoc-cli get-status`、`nvoc-cli set-core-offset-mhz` 或 `nvoc-cli reset-vfp-lock`。

### 全局参数

| 参数 | 简写 | 说明 |
|------|------|------|
| `--gpu <GPU_ID>` | `-g` | 指定目标 GPU。接受十进制或十六进制，可多次指定。 |
| `--no-color` | - | 禁用 ANSI 彩色输出。 |

### 命令

| 命令 | 说明 |
|------|------|
| `export-vfp [OPTIONS] [OUTPUT]` | 将当前 VFP 曲线导出为 CSV。 |
| `export-vfp-log [OPTIONS]` | 从 autoscan 日志解析并导出 VFP 点。 |
| `import-vfp [OPTIONS] [INPUT]` | 从 CSV 导入修改后的 VFP 曲线。 |
| `sync-vfp-memory-pstate` | 将次高可调显存 VFP 档位同步到 P0 显存频率。 |
| `fix-vfp-result [OPTIONS]` | 对 autoscan 结果进行后处理。 |
| `autoscan-vfp [OPTIONS]` | 自动扫描 VFP 曲线。 |
| `autoscan-vfp-legacy [OPTIONS]` | 对旧 GPU 扫描全局 P-State 超频偏移。 |
| `reset-vfp --vfp-domain all\|core\|memory` | 重置 VFP 曲线偏移。 |

---

#### export-vfp

将当前 VFP 曲线导出为 CSV 文件。

```bat
nvoc-auto-optimizer.exe export-vfp .\ws\vfp-init.csv
nvoc-auto-optimizer.exe export-vfp --quick .\ws\vfp-quick.csv
```

默认导出 Graphics（core）曲线；可使用域参数导出其他 VFP 表。

| 参数          | 简写   | 说明                  |
|-------------|------|---------------------|
| `<OUTPUT>`  | —    | 输出路径（`-` 表示 stdout） |
| `--tabs`    | `-t` | 使用 Tab 作为分隔符（默认逗号）  |
| `--quick`   | `-q` | 跳过动态负载测量，仅导出静态曲线    |
| `--nocheck` | `-n` | 跳过动态测量结果的合理性校验      |
| `--memory`  | —    | 导出 Memory 域 VFP 曲线          |
| `--processor` | —  | 导出 Processor 域 VFP 曲线       |
| `--video`   | —    | 导出 Video 域 VFP 曲线           |
| `--undefined` | — | 导出 Undefined 域 VFP 曲线       |

**CSV 列格式（完整动态导出）：**

| 列名                       | 说明                  |
|--------------------------|---------------------|
| `voltage`                | 电压点（μV）             |
| `frequency`              | 当前设定频率（kHz）         |
| `delta`                  | 相对出厂频率的偏移（kHz）      |
| `default_frequency`      | 出厂静态默认频率（kHz）       |
| `default_frequency_load` | 动态负载下实测频率（kHz）      |
| `margin`                 | 负载频率与静态默认频率之差（kHz）  |
| `margin_bin`             | margin 换算为最小步进数（整数） |

> 动态导出会启动压力测试负载运行约 45 秒后读取曲线，请确保默认压力测试脚本可执行。

---

#### import-vfp

从 CSV 文件将修改后的曲线写入 GPU。

```bat
nvoc-auto-optimizer.exe import-vfp .\ws\vfp.csv
```

默认导入 Graphics（core）曲线；Memory 域按点序号对齐（建议基于 export 文件修改），其他域按电压匹配。

| 参数        | 简写   | 说明                 |
|-----------|----|--------------------|
| `<INPUT>` | —    | 输入路径（`-` 表示 stdin） |
| `--tabs`  | `-t` | Tab 分隔符            |
| `--memory`  | —  | 导入 Memory 域 VFP 曲线  |
| `--processor` | — | 导入 Processor 域 VFP 曲线 |
| `--video` | —    | 导入 Video 域 VFP 曲线   |
| `--undefined` | — | 导入 Undefined 域 VFP 曲线 |

---

#### sync-vfp-memory-pstate

将显存 VFP 表中次高档位同步到 P0 频率（便于 Windows 上 P2/P3 频率对齐 P0）。

```bat
nvoc-auto-optimizer.exe sync-vfp-memory-pstate
```

---

#### nvoc-cli VFP lock / reset

将 GPU 电压锁定到 VFP 点位或显式电压，或解除 VFP lock 状态。

```bat
nvoc-cli set-vfp-voltage-lock 68
nvoc-cli set-vfp-voltage-lock 850mV
nvoc-cli set-vfp-voltage-lock 850000uV
nvoc-cli set-locked-clocks-mhz 210 2100 --domain core
nvoc-cli set-locked-clocks-mhz 5000 9501 --domain memory
nvoc-cli reset-locked-clocks --domain core
nvoc-cli reset-locked-clocks --domain memory
nvoc-cli reset-vfp-lock
```

| 命令                                          | 说明                                    |
|---------------------------------------------|---------------------------------------|
| `set-vfp-voltage-lock <POINT_OR_VOLTAGE>`   | 裸数字按 point；电压必须显式单位（`mV` / `uV`）      |
| `set-locked-clocks-mhz <MIN> <MAX> --domain core` | 锁定 NVAPI Graphics 核心频率范围（MHz）         |
| `set-locked-clocks-mhz <MIN> <MAX> --domain memory` | 锁定 NVAPI Memory 显存频率范围（MHz）           |
| `reset-locked-clocks --domain core`         | 解除 NVAPI 核心频率锁定                                       |
| `reset-locked-clocks --domain memory`       | 解除 NVAPI 显存频率锁定                                    |
| `reset-vfp-lock`                            | 清除 NVAPI VFP lock（电压锁定 / 核心/显存频率锁定状态）                    |

---

#### autoscan-vfp

**核心功能**：对当前 GPU 执行完整的 VFP 曲线自动扫描。

```bat
nvoc-auto-optimizer.exe autoscan-vfp
nvoc-auto-optimizer.exe autoscan-vfp -u
nvoc-auto-optimizer.exe autoscan-vfp -u -b aggressive
```

| 参数            | 简写   | 默认                  | 说明                                                          |
|---------------|------|---------------------|-------------------------------------------------------------|
| `--ultrafast` | `-u` | 关                   | 启用超快速模式（仅扫 4 个关键点，其余插值）                                     |
| `--test-exe <路径>` | `-w` | `./test/test_cuda_windows.bat` / `./test/test_cuda_linux.sh` | CLI 压力测试封装脚本路径 |
| `--log <路径>` | `-l` | `./ws/vfp.log`      | 扫描日志路径                                                      |
| `-q <序列>`     | —    | `-`                 | 自定义扫描点序列（`-` 为自动）                                           |
| `-t <次数>`     | —    | `30`                | CLI 压力测试时长/重试循环次数；封装脚本会据此设置压力测试 duration                    |
| `-o <路径>`     | —    | `./ws/vfp-tem.csv`  | 每点结果实时保存的 CSV 路径                                            |
| `-i <路径>`     | —    | `./ws/vfp-init.csv` | 参考原始曲线 CSV 路径                                               |
| `-m`          | —    | 关                   | 同时扫描显存超频                                                    |
| `-b <方式>`     | —    | 按 GPU 世代自动选择        | 崩溃恢复方式：`aggressive`（主动 BSOD 重启）或 `traditional`（等待 TDR 自动恢复） |

---

#### autoscan-vfp-legacy

适用于 Maxwell（GTX 9 系）及更早 GPU 的全局偏移自动扫描。

```bat
nvoc-auto-optimizer.exe autoscan-vfp-legacy
nvoc-auto-optimizer.exe autoscan-vfp-legacy -b aggressive
```

参数与 `autoscan` 基本相同，但不支持 `--ultrafast`、`-m`（显存扫描）、`-q`（点序列）、`-o`（临时结果 CSV）及 `-i`（初始曲线），因为 Legacy 模式只有单一全局偏移。`-w`、`-l` 和 `-t` 仍可用于调整 CLI 压力测试封装脚本、扫描日志和压力测试时长/重试循环。

---

#### fix-vfp-result

对 `autoscan` 产生的临时 CSV 执行**轻重载补偿后处理**，生成最终稳定曲线。

```bat
nvoc-auto-optimizer.exe fix-vfp-result -m 1
nvoc-auto-optimizer.exe fix-vfp-result -m 1 -u
```

| 参数            | 简写   | 说明                                            |
|---------------|------|-----------------------------------------------|
| `-m <整数>`     | —    | 额外保守偏移 bin 数（推荐 `1`，即在 margin 修正基础上额外降低 1 步进） |
| `--ultrafast` | `-u` | 关                                             | 对仅有 4 个关键点的 ultrafast 结果执行插值补全                |
| `-v <路径>`     | —    | `./ws/vfp-tem.csv`                            | 输入：autoscan 产出的临时 CSV                         |
| `-o <路径>`     | —    | `./ws/vfp.csv`                                | 输出：补偿后的最终曲线 CSV                               |
| `-i <路径>`     | —    | `./ws/vfp-init.csv`                           | 参考：出厂原始曲线 CSV                                 |
| `-l <路径>`     | —    | `./ws/vfp.log`                                | 扫描日志（用于读取 ultrafast 关键点）                      |
| `-d <整数>`     | —    | `3`                                           | 参考频率差值 bin（内部参数）                              |

---

#### nvoc-cli set-vfp-point-delta-mhz

手动将 VFP 曲线某一点设置为指定频率偏移，用于手动调试。

```bat
nvoc-cli set-vfp-point-delta-mhz 50 150000
```

| 参数            | 说明           |
|---------------|--------------|
| `<POINT>`     | VFP 点索引       |
| `<DELTA_KHZ>` | 频率偏移（kHz） |

---

## 扫描流程详解

### 阶段 0：准备工作

`start.bat` 在调用 `autoscan-vfp` 前会依次执行：

1. `nvoc-cli get-info`：打印 GPU 信息并识别世代
2. `nvoc-cli reset-pstate-clock-offsets`：清零 P-State 频率偏移，确保干净起点
3. `reset-vfp`（或 `reset-vfp --vfp-domain all`）：将 VFP 曲线所有点偏移归零
   - 若仅需清除核心超频，可改用 `reset-vfp --vfp-domain core`
   - 若仅需清除显存超频，可改用 `reset-vfp --vfp-domain memory`
4. `nvoc-cli reset-vfp-lock`：解除电压锁定 / 频率锁定
5. `export-vfp .\ws\vfp-init.csv`（若不存在）：保存出厂原始曲线作为全程参考
6. 为压力测试可执行文件添加防火墙规则（可选，避免网络活动干扰测试）

### 阶段 1：电压范围探测

`autoscan` 启动时首先调用 `handle_test_voltage_limits`，从根据 GPU 世代预设的起始点出发，通过逐步推进电压锁定（`handle_lock_vfp`）来确定该 GPU 个体真实可达的电压范围：

- **上限探测**：从预设上限点向上逐点推进，直到锁定失败（曲线到达平坦区或超出范围）
- **下限探测**：从预设下限点向下逐点推进，找出最低可用电压点

结果记录到 `vfp.log` 中（`minimum_voltage_point` / `maximum_voltage_point`），断点续扫时直接从日志读取，跳过此阶段。

### 阶段 2：逐点核心频率扫描

对 `[lower_voltage_point, upper_voltage_point]` 范围内每隔 3 个点取一个测试点（标准模式），或取 4 个关键点（ultrafast 模式），在每个电压点执行以下双阶段二分搜索：

**短测试阶段（Short Test）**：用指数步进（2^n × 最小步进）快速收敛到接近稳定上限的频率值。

**长测试阶段（Long Test）**：以短测试找到的值为基础，用单步进行耐久性验证，确认真实稳定性。

每轮测试中会定期施加**频率涨落**（Fluctuation）：在当前设定频率的基础上周期性地上调或下调，模拟实际使用中的动态负载变化，避免
GPU 在静态频率下通过而在动态切换时崩溃。同时通过进程守护（watchdog）监控压力测试进程是否存活，防止扫描卡死。

若检测到温度墙 / 功耗墙频率压制（`thrm_or_pwr_limit_flag`），~~会自动降低测试分辨率以减少 GPU 负载~~
（由于测试压力改为CLI，该部分TODO），确保测试结果反映超频能力而非 TDP 瓶颈。

每个电压点扫描完成后，结果实时追加到 `vfp-tem.csv`，并记录到 `vfp.log`（支持断点续扫）。

### 阶段 3：显存频率扫描（可选）

使用 `-m` 参数启用，在核心扫描完成后，锁定在最高电压点，用类似的二分搜索方式扫描显存超频上限。

### 阶段 4：fix_result 后处理

`start.bat` 在 `autoscan` 完成后自动调用：

```bat
nvoc-auto-optimizer.exe fix-vfp-result -m 1
```

**原理**：从 Pascal（10 系）起，NVIDIA GPU 的 V-F 曲线在轻载和重载下会有微小差异——类似 CPU 的 Load-Line Calibration（LLC）现象。autoscan 在满载下得到的稳定极限频率，如果不加修正直接写入，在负载变化的瞬间可能因实际工作点偏移而导致不稳定。

fix_result 根据 `vfp-init.csv` 中动态测量记录的每个点的 `margin_bin`（负载频率与静态默认频率之差，换算为步进数），对 autoscan 得到的每个点的频率偏移做保守化修正：
- `margin_bin > 5`：降低 `(5 + minus_bin)` 步
- `|margin_bin| < 2`：降低 `(1 + minus_bin)` 步
- 其他：降低 `(|margin_bin| + 1 + minus_bin)` 步

ultrafast 模式下还会先用线性插值对 4 个关键点之间的空缺电压点做补全。

### 阶段 5：导入并导出最终曲线

```bat
nvoc-auto-optimizer.exe import-vfp .\ws\vfp.csv
nvoc-auto-optimizer.exe export-vfp .\ws\vfp-final.csv
```

将修正后的曲线写入 GPU 并快照保存为最终文件。

---

## Ultrafast 模式

Ultrafast 模式的核心假设是：**在原厂标定曲线下，各电压点可超频幅度随电压升高而单调递减**，因此可以只测少数几个点然后线性插值，大幅减少扫描轮次。

**关键点选取逻辑（针对有 Max-Q 阶梯的 50 系 GPU）：**

NVIDIA 50 系 GPU 的出厂曲线中存在一个"Max-Q 阶梯"——在某个中间电压处，频率会突然上跳，形成台阶。该台阶的位置在轻载和重载下不同。为了正确处理这个阶梯，ultrafast 模式会从 `vfp-init.csv`（带动态负载列）中检测以下 4 个关键点：

| 关键点 | 检测依据                          |
|-----|-------------------------------|
| p1  | 静态曲线频率最大跳变处（阶梯下边界，轻载）         |
| p2  | 负载曲线频率最大跳变处（阶梯下边界，重载）         |
| p3  | margin_bin 首次由 0 变为负值处（阶梯上边界） |
| p4  | margin_bin 最大负值处（曲线重载压制最强处）   |

扫描只在 p1、p2、p3、p4 四点各测一次，fix_result 阶段再在各段之间线性插值，并对阶梯区域的超频幅度做额外的安全降幅保护。

**无 Max-Q 阶梯的 GPU**（30 系、40 系等）：p1～p4 均匀分布在扫描范围内的四等分点，直接线性插值。

---

## 崩溃恢复机制

扫描过程中难免触发超频不稳定，工具内置了两种恢复策略：

### Traditional（传统模式）

等待 Windows TDR（Timeout Detection and Recovery）机制自动检测 GPU 挂起并恢复驱动。适用于大多数 GPU。

### Aggressive（激进模式，50 系默认）

部分 RTX 50 系 GPU 的驱动存在 Bug，TDR 触发后无法自动恢复驱动，会导致系统永久卡死，自动扫描流程无法继续。  
Aggressive 模式会主动触发系统内核崩溃（BSOD），强制重启，配合 Windows 启动自启动项实现重启后自动从断点继续扫描。

> **注意**：主动 BSOD 是正常的流程控制手段，**不代表系统故障，不会损坏 GPU 或操作系统**。看到蓝屏重启后无需担心，扫描会自动恢复。

可通过 `-b` 参数手动覆盖：
```bat
autoscan-vfp -b traditional    # 强制使用传统模式
autoscan-vfp -b aggressive     # 强制使用激进模式
```

---

## 断点续扫

每个电压点扫描完成后，结果实时追加到 `ws\vfp.log`。下次运行时，工具会解析日志中的：

- `minimum_voltage_point` / `maximum_voltage_point`：跳过电压范围探测阶段
- 最后一次 `Finished core OC on point` 记录：从该点的下一个点继续
- 最后成功/失败的频率偏移值：直接恢复二分搜索的收敛状态

因此，无论是正常退出、手动中断还是崩溃重启，**只需再次运行 `start.bat`，扫描即可从中断处继续**，无需重新开始。

若需从头开始，运行 `start.bat 1` 清除日志。

---

## Legacy GPU 模式（Maxwell / Pascal）

GTX 9 系（Maxwell，GM 代号）的 NVAPI 不支持 VFP 曲线逐点写入，只能对 P0 状态设置一个统一的全局频率偏移。`autoscan-vfp-legacy` 的流程：

1. 通过 `set_pstates`（`ClockDomain::Graphics`，`PState::P0`）写入全局偏移
2. 用与 VFP 模式相同的二分搜索 + 耐久性测试逻辑扫描最大稳定偏移值
3. 不支持：ultrafast 插值、显存分段扫描、逐点 fix_result

电压控制方面，Maxwell 使用 `SetPstates20` 的 `baseVoltages` 字段（`--voltage-delta` 参数，单位 μV），而非 Pascal 及以后的 VoltRails boost。

Legacy OverVolt实测经常不生效，NV的支持性做得不太好，对于900系之前的卡，鉴于NV没有对VBIOS作校验，建议在保证核心、显存、VRM散热的条件下直接编辑VBIOS以追求最大性能。

---

## 工作文件说明

| 文件                 | 用途            | 说明                            |
|--------------------|---------------|-------------------------------|
| `ws\vfp.log`       | 扫描过程日志        | 断点续扫的核心依据；删除此文件会导致从头开始        |
| `ws\vfp-init.csv`  | 出厂原始曲线快照      | 首次扫描前自动导出；是 fix_result 的参考基准  |
| `ws\vfp-tem.csv`   | autoscan 临时结果 | 每点完成后实时写入；fix_result 的输入      |
| `ws\vfp.csv`       | fix_result 输出 | 经轻重载补偿的最终曲线；`import` 的输入      |
| `ws\vfp-final.csv` | 最终确认快照        | import 后再次 export 保存，可用于比对和备份 |

---

## 测试环境建议

- **室温**：建议 20–25°C，不超过 30°C，不低于 15°C
- **散热**：确保 GPU 散热良好；笔记本建议使用支架，避免软质表面
- **GPU 温度**：扫描期间若核心温度持续 > 82°C，应检查散热设置或等待冷机后继续
- **系统状态**：扫描前关闭其他 GPU 负载程序（游戏、视频编码等）；动态 VFP 导出时尤其需要干净的负载环境
- **电源**：确保笔记本接入外接电源，桌面机电源供应充足。

**扫描期间系统崩溃（BSOD / 画面黑屏 / Linux掉卡/ERR）是正常现象**，说明当前测试点的频率偏移超出了 GPU
稳定极限。工具会自动退回并从更保守的值继续。若系统卡死超过 3 分钟，请手动强制重启，然后重新运行 `start.bat`
。在Linux上，您可能需要将所有在该GPU上的程序（如图形界面）退出，然后使用linux_oc_recover.sh重置（如果出现程序死锁，一般只能重启）。Linux的不稳定超频恢复限度一般低于Windows
TDR，使用时注意保存工作。

---

## 从源码构建

```bat
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc/auto-optimizer
cargo build --release
```

编译产物：`target\release\nvoc-auto-optimizer.exe`

> 依赖 `nvapi-rs` 的私有 git 仓库分支，构建时需要能访问 GitHub。

---

## 免责声明

- 超频操作会使 GPU 在超出出厂规格的条件下工作，存在导致系统不稳定的风险
- 扫描过程触发 GPU 崩溃和系统 BSOD 是本工具设计行为的一部分，**不会造成 GPU 永久损坏**，但使用者需自行评估风险
- 本工具产生的超频设置在系统重启后不会自动保留，需要重新 `import` 曲线方可生效
- **本工具不对因使用不当导致的任何硬件损坏、数据丢失或其他损失负责**

---

## 许可协议

本项目采用 **Apache License 2.0** 许可发布，完整条款请参见仓库根目录的 `LICENSE` 文件。

- SPDX 标识：`Apache-2.0`
- Crate 清单已通过 `license-file = "LICENSE"` 明确指向许可文本
- 发布包与源码仓库会保留 `LICENSE` 文件，便于分发与二次引用

---

*nvoc-auto-optimizer v0.0.3 — by Skyworks*
