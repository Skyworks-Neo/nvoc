# NVOC-AutoOptimizer

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)


[中文](./README.md) | [English](./README-en.md)

本项目采用 [Apache License 2.0](LICENSE) 许可发布，英文版说明见 `README-en.md`。

> **NVIDIA GPU VF 曲线自动超频优化器**  
> 基于 Rust 编写，通过 NVAPI / NVML 接口操控 GPU，配合 `cli-stressor` 压力测试进行逐点电压-频率（V-F Curve）自动扫描，为
> NVIDIA GPU 找出每个电压点的稳定超频上限并生成最优化曲线。

## NVOC-AutoOptimizer——真正懂超频的开发的N卡超超爆妙妙工具

## 配套产品——使用所有配套产品以达到最好体验

[NVOC-AUTOOPTIMIZER](https://github.com/Skyworks-Neo/NVOC-AutoOptimizer)：核心模块。

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor)：压力测试模块，用于自动超频扫描部分。没有该模块仍可以使用自动扫描之外的所有功能。（NVOC-AutoOptimizer开放任何你的自定义压力测试模块接入，只需满足return
code定义即可。）

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI)：跨平台超频图形界面，直接对标MSI Afterburner。 （为了避免GPU超炸带走图形界面，使用CPU渲染，在低端机器如遇到性能问题，建议使用NVOC-TUI）；

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI)：跨平台超频命令行界面，用于没有图形界面的机器，兼容性好，性能要求低；

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV)：client-server架构控制模块，用于机房、服务器、工作站等场景的 Web 管理、~~远程超频~~（TODO）

## 目录

- [背景与原理](#背景与原理)
- [支持的 GPU 世代](#支持的-gpu-世代)
- 兼容性一览（接口 × GPU 世代 / 基础功能）
- [依赖与环境要求](#依赖与环境要求)
- [目录结构](#目录结构)
- [快速开始](#快速开始)
- [命令参考](#命令参考)
  - [顶层参数](#顶层参数)
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

### Maxwell / 9 系 Legacy 模式

Maxwell（GM 代号，9xx 系列）及更早的 GPU 不支持逐点 V-F 曲线写入，只能通过 `SetPstates20` 对 P0 状态施加全局频率偏移。本工具对这类 GPU 使用 `autoscan_legacy` 流程，仅扫描单一全局偏移值。

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
| GTX 9 系（Maxwell）       | `GM`   | **Legacy 全局偏移** | 不支持逐点 VFP；通过 `autoscan_legacy` 扫描     |
| Volta 计算卡              | `GV`   | Legacy          | 同上                                    |

> **移动端 GPU**（名称包含 `Laptop`）无法修改 TDP / 温度墙 / VDDQ boost，工具在检测到时会自动跳过这些设置。

---

<h2 id="compatibility-interfaces-gpu-generations">兼容性一览（接口 × GPU 世代 / 基础功能）</h2>

### **NVAPI接口+桌面消费级GPU:**

|                 功能                 | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9/部分7 (GM) |              备注              |
|:----------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:--------------:|:----------------------------:|
|           VF curve 编辑+导出           |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|            自动超频autoscan            |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|       旧版自动超频autoscan_legacy        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      核心频率偏置 (`--core-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      显存频率偏置 (`--mem-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        | 和VF Curve不独立，应用会导致VF Curve重置 |
|        功耗墙(`--power-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|       温度墙(`--thermal-limit`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速控制(NVAPI cooler)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速重置(NVAPI cooler)        |  ✅(Linux❓)  |  ✅(Linux❓)  |  ✅(Linux❓)  |   ✅(Linux❓)   |   ✅(Linux❌)   |  ✅(Linux❌)   |   ✅(Linux❌)    |                              |
|      电压点锁定 (--locked-voltage)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|      电压点解锁 (--locked-voltage)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|  核心频率范围锁定 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|  核心频率范围解锁 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|   显存频率范围锁定 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |      部分支持      |                              |
|   显存频率范围解锁 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |      部分支持      |                              |
| Boost电压Voltboost( --voltage-boost) |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❌        |                              |
|    过电压Overvolt(--voltage-delta)    |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ✅        |                              |

### **NVML接口+桌面消费级GPU:**

|                 功能                 | RTX 50 (GB) | RTX 40 (AD) | RTX 30 (GA) | RTX 20 (TU10) | GTX 16 (TU11) | GTX 10 (GP1) | GTX 9/部分7 (GM) |              备注              |
|:----------------------------------:|:-----------:|:-----------:|:-----------:|:-------------:|:-------------:|:------------:|:--------------:|:----------------------------:|
|           VF curve 编辑+导出           |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|            自动超频autoscan            |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |       ❌        |      需要基于频率锁定和偏置进行新的算法       |
|       旧版自动超频autoscan_legacy        |    TODO     |    TODO     |    TODO     |     TODO      |     TODO      |     TODO     |      TODO      |      需要基于频率锁定和偏置进行新的算法       |
|      核心频率偏置 (`--core-offset`)      |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      显存频率偏置 (`--mem-offset`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        | 和VF Curve不独立，应用会导致VF Curve重置 |
|        功耗墙(`--power-limit`)        |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|       温度墙(`--thermal-limit`)       |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速控制(NVML cooler)         |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|        风扇转速重置(NVML cooler)         |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ✅        |                              |
|      电压点锁定 (--locked-voltage)      |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|      电压点解锁 (--locked-voltage)      |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|  核心频率范围锁定 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❓        |                              |
|  核心频率范围解锁 (--locked-core-clocks)   |      ✅      |      ✅      |      ✅      |       ✅       |       ✅       |      ✅       |       ❓        |                              |
|   显存频率范围锁定 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|   显存频率范围解锁 (--locked-mem-clocks)   |      ✅      |      ✅      |      ✅      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
| Boost电压Voltboost( --voltage-boost) |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |
|    过电压Overvolt(--voltage-delta)    |      ❌      |      ❌      |      ❌      |       ❌       |       ❌       |      ❌       |       ❌        |                              |

### 移动端GPU：

一般不支持**Boost电压、风扇相关控制、温度墙相关控制、功耗墙相关控制**

## 依赖与环境要求

### 运行时依赖

| 依赖                                      | 说明                                                                                                                          |
|:----------------------------------------|:----------------------------------------------------------------------------------------------------------------------------|
| Windows 10/11                           | 工具通过 NVAPI/NVML 调用，支持 Windows 10/11 和*任何使用 nvidia-open-dkms 的 linux 发行版*（理论上，已测试 ArchLinux + KDE、Ubuntu22.04 、Debian 12/13） |
| NVIDIA 驱动（≥ 537，Linux用nvidia-open-dkms） | 需支持目标 GPU 的 NVAPI/NVML 接口，已知395版本完全不行，因此难以支持驱动太老的kepler和之前的GPU                                                              |
| `cli-stressor` 压力测试运行脚本（自动超频用）          | 默认由 `test\test_cuda_windows.bat` / `test\test_opencl_linux.sh` 调用                                                           |
| Administrator权限/Sudo权限                  | 超频参数写入接口需要管理权限，而大部分读取不需要                                                                                                    |

### 压测脚本位置（默认）

```
NVOC-AutoOptimizer/
├── test/
│   ├── test.bat
│   ├── test_cuda_windows.bat
│   └── test_opencl_linux.sh
└── ws/
```

### 编译依赖（仅从源码构建时需要）

- Rust 工具链
- 目标架构构建工具

---

## 目录结构

```
NVOC-AutoOptimizer/
├── src/
│   ├── main.rs               # 入口，命令路由
│   ├── arg_help.rs           # clap 命令行参数定义
│   ├── basic_func.rs         # GPU 世代检测、分辨率工具、handle_info/list/status/get/reset
│   ├── nvidia_gpu_type.rs     # GPU 世代识别与分型参数
│   ├── oc_get_set_function_nvapi.rs # NVAPI: VFP lock/reset、cooler、电压/频率设置
│   ├── oc_get_set_function_nvml.rs  # NVML: 功耗墙、时钟锁定、P-State 锁定
│   ├── oc_profile_function.rs# VFP export/import、fix_result、autoscan 辅助函数
│   ├── oc_scanner.rs         # autoscan_gpuboostv3 / autoscan_legacy 核心扫描循环
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

所有命令均需以管理员权限运行：

```
NVOC-Auto-Optimizer.exe [全局参数] <子命令> [子命令参数]
```

### 顶层参数

| 参数                         | 简写   | 说明                                                                              |
|-----------------------------|------|------------------|---------------------------------------------------------------------------------|
| `--gpu <GPU_ID>`            | `-g` | 指定目标 GPU。接受十进制或十六进制（`0x0800`），或从 `list` 看到的序号（0, 1, 2…）。可多次指定以选中多卡。缺省时操作所有 GPU。 |
| `--output-format <OFORMAT>` | `-O` | 输出格式：`human`（默认，人类可读）或 `json`                                                   |

---

### info

显示 GPU 详细信息（型号、代号、性能状态、功耗限制、传感器限制等）。

```bat
NVOC-Auto-Optimizer.exe info
NVOC-Auto-Optimizer.exe -O json info -o gpu_info
```

| 参数            | 说明                                            |
|---------------|-----------------------------------------------|
| `-o <OUTPUT>` | JSON 输出时指定文件路径前缀（会生成 `<OUTPUT>_gpu<ID>.json`） |

---

### list

列出系统中所有 NVIDIA GPU，显示序号、ID、PCI 信息及 UUID。

```bat
NVOC-Auto-Optimizer.exe list
```

---

### status

显示 GPU 当前运行状态（频率、电压、温度、风扇、VFP 曲线当前值等）。

```bat
NVOC-Auto-Optimizer.exe status
NVOC-Auto-Optimizer.exe status --all
NVOC-Auto-Optimizer.exe status --monitor 2.0
```

| 参数                    | 简写   | 默认    | 说明            |
|-----------------------|------|-------|---------------|
| `--all`               | `-a` | —     | 显示全部信息        |
| `--status <on\|off>`  | `-s` | `on`  | 显示基本状态        |
| `--clocks <on\|off>`  | `-c` | `on`  | 显示时钟频率        |
| `--coolers <on\|off>` | `-C` | `off` | 显示风扇信息        |
| `--sensors <on\|off>` | `-S` | `off` | 显示传感器温度       |
| `--vfp <on\|off>`     | `-v` | `off` | 显示 VFP 曲线当前值  |
| `--pstates <on\|off>` | `-P` | `off` | 显示 P-State 配置 |
| `--monitor <秒>`       | `-m` | —     | 持续监控，每隔指定秒数刷新 |

---

### get

显示当前生效的超频设置（VFP 偏移量、P-State 偏移等）以及详细的 NVML 状态信息。

```bat
NVOC-Auto-Optimizer.exe get
```

> **补充说明（NVML 状态）**：
> 现在 `get` 命令会输出详细的底层 NVML 设定，包括：
> - 功耗墙限制范围（Min / Current / Max，单位 W）
> - 各个 P-State 对应的核心与显存频率运行边界
> - 针对每一个 P-State 独设的 **核心 / 显存超频偏移量（Offset）**
> - 该显卡所有受支持的可用应用程序频率组合（Supported Applications Clocks），并自动整理出最小/最大频率边界与分度值。

---

### reset

恢复超频设置到默认值。支持灵活的 `setting` 选择器、`--domain` 别名，以及 VFP 重置时的细粒度 `--vfp-domain` 控制；另外提供 `reset nvml-cooler` 子命令用于恢复 NVML 风扇默认控制。

```bat
重置全部设置
NVOC-Auto-Optimizer.exe reset

只重置指定设置
NVOC-Auto-Optimizer.exe reset voltage-boost power nvapi-cooler

使用 --domain 别名（等同于上例）
NVOC-Auto-Optimizer.exe reset --domain voltage-boost --domain power --domain nvapi-cooler

重置 NVML 风扇到默认控制模式
NVOC-Auto-Optimizer.exe reset nvml-cooler

指定 NVML 风扇 ID 重置默认控制模式
NVOC-Auto-Optimizer.exe reset nvml-cooler --id 1

只重置 VFP 曲线，并仅清除核心频率偏移
NVOC-Auto-Optimizer.exe reset vfp --vfp-domain core

只清除 VFP 显存频率偏移
NVOC-Auto-Optimizer.exe reset --domain vfp --vfp-domain memory

仅给 --vfp-domain，默认作为 reset vfp 处理
NVOC-Auto-Optimizer.exe reset --vfp-domain core
```

**可指定的重置项（可多选）：**

| 值                           | 简称或别名 | 说明                                               |
|-----------------------------|-------|--------------------------------------------------|
| `voltage-boost`             | —     | 电压 Boost 归零                                      |
| `thermal` 或 `sensor-limits` | —     | 温度墙恢复默认                                          |
| `power` 或 `power-limits`    | —     | 功耗墙恢复默认                                          |
| `nvapi-cooler`              | —     | NVAPI 风扇控制恢复自动                                   |
| `nvml-cooler`               | —     | NVML 风扇控制恢复自动                                    |
| `vfp` 或 `vfp-deltas`        | —     | VFP 曲线所有偏移归零（可配合 `--vfp-domain`）                 |
| `lock` 或 `vfp-lock`         | —     | 解除电压锁定                                           |
| `pstate` 或 `pstate-deltas`  | —     | P-State 频率偏移归零                                   |
| `overvolt`                  | —     | 清零 legacy GPU 的 baseVoltage delta（Maxwell / 9 系） |

**VFP 重置域选项（仅在 `--vfp-domain` 指定时生效）：**

| 值        | 说明                  |
|----------|---------------------|
| `all`    | 重置核心与显存频率偏移（默认）     |
| `core`   | 仅重置核心频率（Graphics）偏移 |
| `memory` | 仅重置显存频率（Memory）偏移   |

---

### set

超频设置入口，现在区分了 `nvapi` 和 `nvml` 两套接口，支持以下子命令和参数。

#### set nvml-cooler

设置 NVML 风扇策略和转速。

```bat
NVOC-Auto-Optimizer.exe set nvml-cooler --id 1 --policy manual --level 60
NVOC-Auto-Optimizer.exe set nvml-cooler --policy continuous --level 80
```

| 参数         | 说明                                        |
|------------|-------------------------------------------|
| `--id`     | 风扇 ID（`1` / `2` / `all`，默认 `all`）         |
| `--policy` | 风扇策略（例如 `continuous` / `manual` / `auto`） |
| `--level`  | 风扇转速百分比                                   |

#### set nvapi

通过官方 NVAPI 接口进行超频，主要用于 VFP 曲线锁定、电压 Boost 和核心/显存频率范围锁定。

```bat
NVOC-Auto-Optimizer.exe set nvapi --voltage-boost 100 --thermal-limit 90
NVOC-Auto-Optimizer.exe set nvapi --core-offset 150000 --mem-offset 500000
NVOC-Auto-Optimizer.exe set nvapi --locked-voltage 68
NVOC-Auto-Optimizer.exe set nvapi --locked-core-clocks 210 2100
NVOC-Auto-Optimizer.exe set nvapi --locked-mem-clocks 5000 9501
NVOC-Auto-Optimizer.exe set nvapi --reset-vfp-locks
```

| 参数                                 | 简写   | 说明                                                  |
|------------------------------------|------|-----------------------------------------------------|
| `--voltage-boost <0-100>`          | `-V` | 设置电压 Boost 百分比（桌面 GPU）                              |
| `--thermal-limit <℃>`              | `-T` | 设置温度墙（摄氏度）                                          |
| `--power-limit <%>`                | `-P` | 设置功耗墙百分比                                            |
| `--voltage-delta <μV>`             | `-U` | 核心电压偏移（单位 μV，适用于 Maxwell / 900 系及更早）                |
| `--pstate <PSTATE>`                | `-z` | 配合 `--voltage-delta` 及 Offset 指定目标 P-State（默认 `P0`） |
| `--core-offset <kHz>`              | —    | 通过 NVAPI 设置特定 P-State 的核心频率偏移（kHz）                  |
| `--mem-offset <kHz>`               | —    | 通过 NVAPI 设置特定 P-State 的显存频率偏移（kHz）                  |
| `--locked-voltage <POINT_OR_VOLT>` | —    | 锁定 VFP 电压。数字按 point（如 `68`）；电压带单位（如 `850mV`）        |
| `--locked-core-clocks <MIN> <MAX>` | —    | 锁定 NVAPI Graphics 核心频率范围（MHz）                       |
| `--locked-mem-clocks <MIN> <MAX>`  | —    | 锁定 NVAPI Memory 显存频率范围（MHz）                         |
| `--reset-core-clocks`              | —    | 解除 NVAPI 核心频率锁定                                       |
| `--reset-mem-clocks`               | —    | 解除 NVAPI 显存频率锁定（别名：`--pstate-unlock`）               |
| `--reset-vfp-locks`                | —    | 解除 NVAPI VFP lock（电压锁定 / 频率锁定状态）                    |

#### set nvml

通过官方 NVML 接口进行超频，主要用于 P-State 级别的 Offset 设置、功耗限制（瓦特）和显存锁窗。

```bat
NVOC-Auto-Optimizer.exe set nvml --core-offset 150 --mem-offset 1000
NVOC-Auto-Optimizer.exe set nvml -P 350
NVOC-Auto-Optimizer.exe set nvml --pstate-lock P0
```

| 参数                                 | 说明                                          |
|------------------------------------|---------------------------------------------|
| `--pstate <ID>`                    | 指定目标 P-State 序号（`0` 表示 P0，`2` 表示 P2，默认 `0`） |
| `--core-offset <MHz>`              | 设置特定 P-State 的核心频率偏移（MHz）                   |
| `--mem-offset <MHz>`               | 设置特定 P-State 的显存频率偏移（MHz）                   |
| `-T, --thermal-limit <℃>`          | 温度墙兼容参数（别名：`--thermal-gpu-max`），当前版本仅解析不写入 |
| `--thermal-shutdown <℃>`           | NVML `Shutdown` 兼容参数，当前版本仅解析不写入                    |
| `--thermal-slowdown <℃>`           | NVML `Slowdown` 兼容参数，当前版本仅解析不写入                    |
| `--thermal-memory-max <℃>`         | NVML `MemoryMax` 兼容参数，当前版本仅解析不写入                   |
| `--thermal-acoustic-min <℃>`       | NVML `AcousticMin` 兼容参数，当前版本仅解析不写入                 |
| `--thermal-acoustic-curr <℃>`      | NVML `AcousticCurr` 兼容参数，当前版本仅解析不写入                |
| `--thermal-acoustic-max <℃>`       | NVML `AcousticMax` 兼容参数，当前版本仅解析不写入                 |
| `--thermal-gps-curr <℃>`           | NVML `GpsCurr` 兼容参数，当前版本仅解析不写入                     |
| `-P, --power-limit <W>`            | 精确设置功耗墙限制（瓦特）                               |
| `--app-clock <Mem> <Core>`         | 锁定应用程序时钟频率，依次传入显存（MHz）与核心（MHz）              |
| `--locked-core-clocks <Min> <Max>` | 锁定 GPU 核心频率范围（MHz）                          |
| `--reset-core-clocks`              | 解除 GPU 核心频率锁定                               |
| `--locked-mem-clocks <Min> <Max>`  | 锁定显存频率范围（MHz）                               |
| `--pstate-lock <ID> [<ID>]`        | 通过显存锁窗将 GPU 锁到单个或连续 NVML P-State 区间（如 `P0`） |
| `--reset-mem-clocks`               | 解除显存频率锁定（包含 P-State 锁定解除）                   |

`get` / `status` 人类可读输出中会显示 NVML Temperature Thresholds 的细分值（不可用项显示 `N/A`）；当前版本不执行 NVML 温度阈值写入。

#### set nvapi-cooler

设置风扇策略和转速。

```bat
NVOC-Auto-Optimizer.exe set nvapi-cooler --id 1 --policy continuous --level 60
NVOC-Auto-Optimizer.exe set nvapi-cooler --policy manual --level 80
```

| 参数         | 说明                                |
|------------|-----------------------------------|
| `--id`     | 风扇 ID（`1` / `2` / `all`，默认 `all`） |
| `--policy` | 风扇策略（例如 `continuous` / `manual`）  |
| `--level`  | 风扇转速百分比                           |

---

#### set vfp export

将当前 VFP 曲线导出为 CSV 文件。

```bat
NVOC-Auto-Optimizer.exe set vfp export .\ws\vfp-init.csv
NVOC-Auto-Optimizer.exe set vfp export --quick .\ws\vfp-quick.csv
```

| 参数          | 简写   | 说明                  |
|-------------|------|---------------------|
| `<OUTPUT>`  | —    | 输出路径（`-` 表示 stdout） |
| `--tabs`    | `-t` | 使用 Tab 作为分隔符（默认逗号）  |
| `--quick`   | `-q` | 跳过动态负载测量，仅导出静态曲线    |
| `--nocheck` | `-n` | 跳过动态测量结果的合理性校验      |

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

#### set vfp import

从 CSV 文件将修改后的曲线写入 GPU。

```bat
NVOC-Auto-Optimizer.exe set vfp import .\ws\vfp.csv
```

| 参数        | 简写   | 说明                 |
|-----------|----|--------------------|
| `<INPUT>` | —    | 输入路径（`-` 表示 stdin） |
| `--tabs`  | `-t` | Tab 分隔符            |

---

#### set nvapi lock / reset

将 GPU 电压锁定到 VFP 点位或显式电压，或解除 VFP lock 状态。

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

| 参数                                          | 说明                                    |
|---------------------------------------------|---------------------------------------|
| `--nvapi-locked-voltage <POINT_OR_VOLTAGE>` | 裸数字按 point；电压必须显式单位（`mV` / `uV`）      |
| `--nvapi-locked-core-clocks <MIN> <MAX>`    | 锁定 NVAPI Graphics 核心频率范围（MHz）         |
| `--nvapi-locked-mem-clocks <MIN> <MAX>`     | 锁定 NVAPI Memory 显存频率范围（MHz）           |
| `--nvapi-reset-core-clocks`                 | 解除 NVAPI 核心频率锁定                         |
| `--nvapi-reset-mem-clocks`                  | 解除 NVAPI 显存频率锁定（别名：`--pstate-unlock`） |
| `--nvapi-reset-vfp-locks`                   | 清除 NVAPI VFP lock（电压锁定 / 核心/显存频率锁定状态） |

---

#### set vfp autoscan

**核心功能**：对当前 GPU 执行完整的 VFP 曲线自动扫描。

```bat
NVOC-Auto-Optimizer.exe set vfp autoscan
NVOC-Auto-Optimizer.exe set vfp autoscan -u
NVOC-Auto-Optimizer.exe set vfp autoscan -u -b aggressive
```

| 参数            | 简写   | 默认                  | 说明                                                          |
|---------------|------|---------------------|-------------------------------------------------------------|
| `--ultrafast` | `-u` | 关                   | 启用超快速模式（仅扫 4 个关键点，其余插值）                                     |
| `-w <路径>`     | —    | `./test/test.bat`   | 压力测试脚本路径                                                    |
| `-l <路径>`     | —    | `./ws/vfp.log`      | 扫描日志路径                                                      |
| `-q <序列>`     | —    | `-`                 | 自定义扫描点序列（`-` 为自动）                                           |
| `-t <次数>`     | —    | `30`                | 单次测试超时检测循环次数                                                |
| `-o <路径>`     | —    | `./ws/vfp-tem.csv`  | 每点结果实时保存的 CSV 路径                                            |
| `-i <路径>`     | —    | `./ws/vfp-init.csv` | 参考原始曲线 CSV 路径                                               |
| `-m`          | —    | 关                   | 同时扫描显存超频                                                    |
| `-b <方式>`     | —    | 按 GPU 世代自动选择        | 崩溃恢复方式：`aggressive`（主动 BSOD 重启）或 `traditional`（等待 TDR 自动恢复） |

---

#### set vfp autoscan_legacy

适用于 Maxwell（GTX 9 系）及更早 GPU 的全局偏移自动扫描。

```bat
NVOC-Auto-Optimizer.exe set vfp autoscan_legacy
NVOC-Auto-Optimizer.exe set vfp autoscan_legacy -b aggressive
```

参数与 `autoscan` 基本相同，但不支持 `--ultrafast`、`-m`（显存扫描）、`-q`（点序列）及 `-i`（初始曲线），因为 Legacy 模式只有单一全局偏移。

---

#### set vfp fix_result

对 `autoscan` 产生的临时 CSV 执行**轻重载补偿后处理**，生成最终稳定曲线。

```bat
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1 -u
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

#### set vfp single_point_adj

手动将 VFP 曲线某一点设置为指定频率偏移，用于手动调试。

```bat
NVOC-Auto-Optimizer.exe set vfp single_point_adj -s 50 -d 150000
```

| 参数         | 简写 | 默认       | 说明        |
|------------|----|----------|-----------|
| `-s <索引>`  | —  | `50`     | 起始点索引     |
| `-d <kHz>` | —  | `150000` | 频率偏移（kHz） |

---

## 扫描流程详解

### 阶段 0：准备工作

`start.bat` 在调用 `autoscan` 前会依次执行：

1. `info`：打印 GPU 信息并识别世代
2. `reset pstate`：清零 P-State 频率偏移，确保干净起点
3. `reset vfp`（或 `reset --domain vfp --vfp-domain all`）：将 VFP 曲线所有点偏移归零
   - 若仅需清除核心超频，可改用 `reset --domain vfp --vfp-domain core`
   - 若仅需清除显存超频，可改用 `reset --domain vfp --vfp-domain memory`
4. `set --nvapi-reset-vfp-locks`：解除电压锁定 / 频率锁定
5. `set vfp export .\ws\vfp-init.csv`（若不存在）：保存出厂原始曲线作为全程参考
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
NVOC-Auto-Optimizer.exe set vfp fix_result -m 1
```

**原理**：从 Pascal（10 系）起，NVIDIA GPU 的 V-F 曲线在轻载和重载下会有微小差异——类似 CPU 的 Load-Line Calibration（LLC）现象。autoscan 在满载下得到的稳定极限频率，如果不加修正直接写入，在负载变化的瞬间可能因实际工作点偏移而导致不稳定。

fix_result 根据 `vfp-init.csv` 中动态测量记录的每个点的 `margin_bin`（负载频率与静态默认频率之差，换算为步进数），对 autoscan 得到的每个点的频率偏移做保守化修正：
- `margin_bin > 5`：降低 `(5 + minus_bin)` 步
- `|margin_bin| < 2`：降低 `(1 + minus_bin)` 步
- 其他：降低 `(|margin_bin| + 1 + minus_bin)` 步

ultrafast 模式下还会先用线性插值对 4 个关键点之间的空缺电压点做补全。

### 阶段 5：导入并导出最终曲线

```bat
NVOC-Auto-Optimizer.exe set vfp import .\ws\vfp.csv
NVOC-Auto-Optimizer.exe set vfp export .\ws\vfp-final.csv
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
set vfp autoscan -b traditional    # 强制使用传统模式
set vfp autoscan -b aggressive     # 强制使用激进模式
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

GTX 9 系（Maxwell，GM 代号）的 NVAPI 不支持 VFP 曲线逐点写入，只能对 P0 状态设置一个统一的全局频率偏移。`autoscan_legacy` 的流程：

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
git clone https://github.com/your-org/NVOC-AutoOptimizer.git
cd NVOC-AutoOptimizer
cargo build --release
```

编译产物：`target\release\NVOC-Auto-Optimizer.exe`

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

*NVOC-AutoOptimizer v0.0.3 — by Skyworks*

