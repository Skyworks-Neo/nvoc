# GPU-Z 2.71.0.0 传感器面全量逆向审计（对照 nvapi-rs / nvoc）

| 项 | 值 |
|---|---|
| 目标 | TechPowerUp GPU-Z 2.71.0.0（win32/PE，UPX 壳） |
| 方法 | upx -d 脱壳 → idalib 9.0 建库（32 位无反编译器）+ capstone 5.0.7 离线反汇编/字节扫描 |
| 基线 | 主仓 `cli-more-reversing@557b62e`（xOCD 审计之上）；nvapi-rs nvid.rs 注册 2188 IID |
| 证据台账 | `reverse/gpuz/gpuz271-recon-ledger.md`（untracked）+ `~/ida-scratch/gpuz-decomp/`（脚本/JSON/ASM） |
| 前作 | `docs/reverse-engineering/gpu-z/per-rail-power.md`（WinRing0 结案，2026-09）；dev-memory-digest 旧工具审计 GPU-Z=99 ID |
| 车道 | 2026-10-05 GPU-Z 车道（cli-more-reversing 分支，docs-only） |

---

## 1. 版本锁定与复现流水线

```
packed  reverse/gpuz/GPU-Z.exe            SHA256 a2412d6720e091f65925622561c3f274a25ecd0b9fb0f50ab3b910918f01dee9
unpacked GPU-Z.exe (upx 5.2.1 -d)         SHA256 6d7234b4a7a0f010357ba994c9585c1ed2ff74131dce50629d31587652c5a301
PE32 x86 machine=0x014C  base=0x400000   12,428,576 → 53,758,240 bytes（.text 仅 1.9MB，.rsrc ≈49MB 内嵌载荷）
FileVersion 2.71.0.0                      "(c) 2007-2026 TechPowerUp"
```

复现：`upx -d` → `idalib db_open`（75.1s 自动分析）→ `scan_gpuz.py`（已知 ID 全文件 4B LE 扫描 + UTF-16 串提取）→ `capstone` 定向反汇编（resolver/轮询器/getter）。脚本与产物清单在台账。

## 2. 工具概览与驱动访问通道

1. **NVAPI（主通道）**：`LoadLibraryW("nvapi.dll")` → `GetProcAddress("nvapi_QueryInterface")`（字符串 @0x720630/0x720644）→ 主解析器 `sub_6246F0`（0x6246F0..0x6252FD）内 **122 个 `push <ID>; call eax`**，函数指针缓存进 ctx 结构（+0x0c..+0x1a8，完整 ID→偏移表见台账）。第二 display 解析器 @~0xc73300 复解 22 个 display/生命周期 ID，全部 ⊆ 主集 → **122 即全部接口面，完备性成立**。
2. **GDI 显示驱动逃逸（温度回退）**：`CreateDCW`+`ExtEscape`（sub_5FE030，escape 号 **0x7037**）——绕开 NVAPI 直连内核的私有回退。
3. **低阶内核驱动（第 8 代接口）**：'Loading low level driver...'（sub_42BB50）+ **'GPU-Z-v8'** 命名簇 + XOR 加密 blob（0x7e1820 起）+ CreateServiceW/DeviceIoControl 静态导入。旧 per-rail-power.md 的 WinRing0/IOCTL 0x800064A0 结论继续有效，2.71 仍走加密命名（'GPUZShm'、'WinRing0' 明文均为零命中）。
4. **进程内 I2C 引擎（功率直读）**：256B 分页缓存读（sub_64A080，vtable[0] 块读）+ 32 位组装（sub_636050）+ `/1000` 魔数 0x10624DD3 —— INA3221 型 shunt 监视器计算（详见 §5）。
5. **零 NVML**：`nvml.dll` ASCII/UTF-16 全零引用——GPU-Z 从不用 NVML。
6. **内嵌载荷**：.rsrc 49MB 含 nvpowerapi.dll 字样 ×2 与 NVIDIA Firmware Update Utility（NVFlash）载荷，自带小型 NVAPI 子集（lifecycle+display+EnumPhysicalGPUs），无新 ID。

## 3. NVAPI ID 交叉验证总表

**总量 122，全部已在我们 nvid.rs 注册，零未知 ID**（与 xOCD 审计结论同形）。旧工具审计封盘 99 → 2.71 为 122（+23；逐条 delta 重建列为 E6）。

### 3.1 分类总表

| 类 | 含义 | 条目 |
|---|---|---|
| **D 双方一致** | 已注册+已封装+双方在用 | 枚举/身份（EnumPhysicalGPUs、GetFullName、GetPCIIdentifiers、GetDisplayDriverVersion…）、GetThermalSettings、GetAllClockFrequencies、GetPstates20、GetCurrentPstate、GetMemoryInfo、GetDisplayDriverMemoryInfo、GetTachReading、GetCoolerSettings、ClientFanCoolers GetInfo/GetStatus/GetControl/SetControl、ClientPowerPolicies GetInfo/GetStatus/SetStatus、ClientThermalPolicies GetInfo/GetStatus/SetStatus、ClientPowerTopology GetInfo/GetStatus、GetVoltages、GetVoltageDomainsStatus、ClientVoltRails GetStatus/GetControl/SetControl、ClockClientClkDomains/ClkVfPoints Get 族、PerfPolicies/PerfClientLimits、GetDynamicPstatesInfoEx、GetVbiosVersionString/GetVbiosImage、I2CReadEx/I2CWriteEx（解析在面，用途见 §5/§9） |
| **C 布局/语义增量** | 双方都有但 GPU-Z 给出新布局证据 | ① **PowerMonitorGetStatus 状态缓冲通道索引 = 通道 ID×0x2C**（sub_654390 `imul …,0x2c`；魔数 0x1059C=ver1\|0x59C）——我们目前只有 ch0+0x44 与启发式偏移吻合；② **ThermChannelGetInfo 用 ver1\|0x44 请求 ≤15 通道**（sub_6243F0 `mov [esi],0x10044; push 0xf`），20B/条记录 + 通道类型 0x10/0x11 过滤——我们的 ThermChannelGetStatus(0x65FE3AAD) 之外，GPU-Z 主用 **GetInfo(0x0BC8163D)**；③ **ClientFanCoolersGetStatus V2 = 0x20AB0**（ver2\|0xAB0，sub_625CB0）——风扇 % 的正式读面 |
| **B 已注册但未封装/未作为读面** | GPU-Z 在用、我方无对应 CLI 面 | GetPowerMizerInfo/SetPowerMizerInfo、GetECCStatusInfo/GetECCErrorInfo、GetSerialNumber、GetRamMaker、GetFBWidthAndLocation、GetValidGpuTopologies、GetPerGpuTopologyStatus、ClockClkDomainsMeasureFreq(0x527FC458)、引擎计数族（GetVPECount/GetShaderPipeCount/GetShaderSubPipeCount/GetTotalSMCount/GetTotalSPCount/GetTotalTPCCount/GetGpuCoreCount/GetPartitionCount/GetRasterBackendCount）、GetHybridPadInfo、GetActiveOutputs、GetRamType（部分 get-info 已有） |
| **A 写路径呼应（xOCD 交叉印证）** | GPU-Z 也解析的 SET | **0x0F4DAE6B SetPstates20 与 0x0733E009 ClockClientClkVfPointsSetControl = xOCD 审计「四个未封装 SET」之二**（两工具独立佐证）；另有 SetPstatesInfo(0xCDF27911)、SetPerfClocks(0x07BCF4AC)、SetPowerMizerInfo(0x50016C78)、ClientThermalPoliciesSetStatus/ClientPowerPoliciesSetStatus |
| **未用** | GPU-Z 不用 | 0x2AD3DBAB PowerMonitorGetStatusV45（YOFOO sibling）不在 122 内 |

完整 122 ID 清单（含解析地址与 vtable 偏移）见台账 `reverse/gpuz/gpuz271-recon-ledger.md`。

## 4. Sensors 面板逐行机制表

主轮询器 = `sub_5FF800`（0x5FF800..0x60326E，3.9k 指令），78 个传感器名块逐块 call 集合见台账 `sensor_blocks.txt`。摘要（★=块内含直证调用）：

| 簇 | GPU-Z 显示名（2.71 实测字符串） | 后端 | 证据 VA |
|---|---|---|---|
| 时钟 | GPU/Memory/Shader/Crossbar/SYS/L2/Video Clock | 时钟族 getter（0x61F300/0x625CB0/0x625D50/0x61F9C0；NVAPI GetAllClockFrequencies/GetAllClocks/PerfClocks 在 ctx 面） | 0x5FF923..0x6002E1 |
| 温度 | GPU Temperature / PCB Temperature | **ThermChannelGetInfo ver1\|0x44（15 通道×20B）** → 回退 **ExtEscape 0x7037** | 0x600405、0x5FF700、0x6243F0、0x5FE030 |
| 温度 | Hot Spot / Memory Temperature / Hottest Memory Chip / GPU Temp.#1-3 / (DISIO/MEMIO/SHADERCORE) | ThermChannel 派生（0x61E660/0x61E6D0）；字符串 @0x766054..0x766160 | 0x600CE3、0x600EE9 |
| 温度 | VRM/PCB/SOC/Water/PLX/12V-2x6/GC-HPWR Temperature、VDDC/MVDDC/VDDCI Phase #%d | 通道化温度簇（字符串 @0x71BDDC、0x71CFB0、0x71FD78、0x73665C…） | — |
| 风扇 | Fan Speed (%) | **ClientFanCoolersGetStatus V2（0x20AB0）** | 0x600846、0x625CB0 |
| 风扇 | Fan Speed (RPM) / Fan %d Speed | Tach/Cooler 族（0x5FEC20/0x61E320/0x6253E0） | 0x600B95、0x601164 |
| 利用率 | GPU Load / Memory Controller Load / Video Engine Load / Bus Interface Load | GetDynamicPstatesInfoEx 簇 | 0x601892..0x601AE8 |
| 显存 | Memory Used (Dedicated/Dynamic)、System Memory Used | GetMemoryInfo+GetDisplayDriverMemoryInfo；**Force_WDDM_Mem_Sensor** 特例路径（0x64E4B0） | 0x601548、0x601C1D、0x601CBE |
| 总线 | PCIe TX / PCIe RX（GB/s） | 自有 **NVPcieThroughput** 模块（GET_PROBED_IDS/devInst 枚举，字符串 @0x7360C8+） | 0x60169A、0x601764 |
| 功率 | Board Power Draw | **I2C INA3221 直读（0x634D90）** 优先 + PowerMonitor 通道表 | 0x601F41、0x601E9D |
| 功率 | GPU Chip Power Draw / MVDDC / MVDDQ | **PowerMonitorGetStatus 通道 246/247（0x6542D0→0x654390→0x623A60，0x1059C）** | 0x60202B、0x60210E、0x6021F1 |
| 功率 | PWR_SRC Power Draw / PWR_SRC Voltage | PowerMonitor 通道 | 0x6022E1、0x6023A5 |
| 功率 | PCIe Slot / 6-Pin #1-2 / 8-Pin #1-6 / 16-Pin 功率与输入电压 | 通道描述符表（0x5FC3B0 构造器 + 0x654340/0x654390） | 0x6023EB..0x6025BB |
| 功率 | USB-C Power Draw | I2C 路（0x634D90） | 0x6029BC |
| 功率 | Power Consumption (W/%) | 0x634D90 + 0x623770 / 0x626120 | 0x602B05、0x602D50 |
| 限因 | PerfCap Reason（Pwr/Thrm/VRel/VOp/Idle 文案） | 0x5FC3E0（@0x71D65E..0x71D776） | 0x602E33 |
| 其他 | EVGA iCX | vt+0x24 派生 | 0x603091 |

## 5. 核心发现一：每轨功率双路径拓扑（与我们 4060L 车道互证）

```
'GPU Chip Power Draw'(0x60202B) ←push 0xf6(246)→ sub_6542D0 通道表匹配
'MVDDC Power Draw'(0x60210E)    ←push 0xf7(247)→      │
                                       ▼
              sub_654390: 0x59C 缓冲 → sub_623A60 写魔数 0x1059C(ver1|0x59C)
                                       │   = PowerMonitorGetStatus 状态结构
                                       ▼
              通道值 = 状态缓冲[通道ID × 0x2C]（imul 直接索引）
```

- **主路径 = NVAPI PowerMonitor**（GetInfo 描述符 + GetStatus 状态），**通道号与我方 `gpu_z_rail_name` 映射同号**（246→Chip、247→MVDDC；Board/16-Pin 走 0x654340 带外支路）。`GPUZ_OFFSET_LABELS` 的 4060L 软门设计由此获得跨工具佐证。
- **副路径 = 进程内 I2C 直读**（sub_634D90）：配置查表 → 分页缓存读（256B/页）→ 寄存器组装 → `/1000`（0x10624DD3）→ V×I = **INA3221 型 shunt 监视器语义**；Board Power（0x601E9D 优先分支）、USB-C、Power Consumption (W) 吃这条。
- 对 2026-09 per-rail-power.md 的修正粒度：2.71 里 **每轨主读面已迁移到 NVAPI PowerMonitor**，WinRing0/低阶驱动承担的是 I2C 传输层与回退（传输后端归属待 E3 裁决：`NvAPI_I2CReadEx` vs 自带驱动 IOCTL）。
- "Power Consumption (W)/(%)" 两个通用传感器名（0x71D5A8/0x71D5E4）= 笔记本/无描述符系统的回退功率面。

## 6. 核心发现二：温度三级链与风扇正式读面

1. **ThermChannelGetInfo（0x0BC8163D）ver1|0x44**：请求 ≤15 通道，每通道 20B 记录（值+类型+min/max），类型 **0x10/0x11** 被过滤采用（0x5FF700 内 `cmp 0x10/0x11`）→ GPU Temperature/Hot Spot/Memory Temperature 的来源。我方已注册但 CLI 走的是 ThermChannelGetStatus——**这是 C 类布局增量，E4 可 A/B**。
2. **ExtEscape 0x7037 回退**：NVAPI 热面不可用时 `CreateDCW("\\\\.\\DISPLAY…")+ExtEscape` 直连显示驱动（0x5FE030）——GPU-Z 保底手段，我们结构性不做（§9 分类）。
3. **风扇 % = ClientFanCoolersGetStatus V2（0x20AB0）**：cooler 句柄数组在 provider+0x1DC，与我方 `FanCoolerGetStatus(0x3CC2D181)` 封装同族；RPM 另走 Tach/Cooler 族。

## 7. 读面/遥测摘要（GPU-Z 独有工程）

- **NVPcieThroughput 模块**：自有 PCIe 吞吐探测（GET_PROBED_IDS → devInst/subInst 枚举 → 根 client），不依赖 NVAPI——对应我方 `pcie_tx/rx_mibps`（NVAPI 路）。
- **Force_WDDM_Mem_Sensor**：配置键强制显存传感器走 WDDM 路径（0x601C1D）——传感器强制通道设计（§11 可借鉴）。
- **Hottest Memory Chip**：从 ThermChannel 多通道 argmax 派生的合成传感器（0x601021）。
- **字符串级 PerfCap 文案**：Pwr/Thrm/VRel/VOp/Idle 五位（0x71D65E..0x71D776）与我方 PerfLimits 位语义一致。
- **每连接器功率/电压**（PCIe Slot/6-Pin/8-Pin #1-6/16-Pin/USB-C）：全部来自通道描述符表 + INA3221 直读，是我方 power_rails_w 的超集显示。

## 8. 概念命名映射表

| GPU-Z 2.71 | 我方 | 判定 |
|---|---|---|
| Crossbar Clock | ClockDomainId::Xbar | 同物异名（clock.rs:50-89 已有 advisory 注释） |
| Video Clock | Msd（域 20/21 裁定） | 同物异名（nvclocks 审计已注 GPU-Z/nvidia-smi 叫 video clock） |
| SYS Clock | Sys | 一致 |
| L2 Clock | Hub（域 4） | ⚠ 疑同物异名，待 A/B |
| GPU Load / Memory Controller Load / Video Engine Load / Bus Interface Load | GetDynamicPstatesInfoEx percent 条目 | 同物异名 |
| Board Power Draw | InputTotalBoard / "Total Board Power Draw" | 同物（rail 245/223 同号互证） |
| GPU Chip Power Draw | InputNvvdd / "NVIDIA GPU VDD" | 同物（通道 246 同号互证） |
| MVDDC Power Draw | MVDDC | 同名同物（通道 247 同号互证） |
| PWR_SRC Power Draw | "PWR PP SRC"（ampereoc 层） | 同物异名（微差） |
| Hot Spot / Memory Temperature | hotspot_index / memory_index | 一致 |
| PCIe TX/RX（**GB/s**） | pcie_tx/rx_**mibps**（**MiB/s**） | 同物**异单位** |
| Memory Used (Dedicated/Dynamic) | memory_info dedicated/… | 同物异键名 |
| PerfCap Reason 五位 | PerfLimits 位 | 一致 |
| Fan Speed (%) | Fan1/Fan2 percent | 一致 |

## 9. 查漏补缺清单（我方缺口，按可达性分类）

**NVAPI 可补（ID 已注册，缺封装或 CLI 面）**
1. `ClockClkDomainsMeasureFreq(0x527FC458)`——域频率测量（呼应 clock.rs:1452 MEASURE 未接线 TODO）。
2. `GetECCStatusInfo/GetECCErrorInfo`——ECC 错误计数读取面（get-info 增量）。
3. `GetSerialNumber`、`GetRamMaker`——身份面补全（RamMaker 与我方 vBIOS 侧 RAM 厂商解析可交叉）。
4. `GetValidGpuTopologies`/`GetPerGpuTopologyStatus`——拓扑读取面（呼应 fabric 车道 99e0e4a）。
5. `NvAPI_I2CReadEx/I2CWriteEx`——通用 I2C 总线面（高价值+高危；GPU-Z 用它做 INA3221/PMBus 直达，见 §5；写面须提权门控+白名单设备）。
6. 引擎计数族九件（VPE/ShaderPipe/ShaderSubPipe/SM/SP/TPC/GpuCore/Partition/RasterBackend）——get-info 计算能力补全。
7. `GetFBWidthAndLocation`、`GetHybridPadInfo`、`GetActiveOutputs`——低频补全。

**结构性不做（维持既有设计决策）**
8. ExtEscape 0x7037 内核回退——GDI escape 通道不符合我方抽象层路线（4060L 结论）。
9. 低阶驱动 v8 直读——不做内核驱动（WinRing0 边界结论维持；Microsoft 脆弱驱动黑名单侧原因同旧档）。

**xOCD 交叉印证（写路径）**
10. 0x0F4DAE6B SetPstates20 与 0x0733E009 ClkVfPointsSetControl：xOCD 审计四个未封装 SET 中的两个，GPU-Z 独立解析佐证其真实在用——决断后可与 xOCD 卡合并处理。

## 10. 命名冲突 E-matrix（供裁决）

| # | 矛盾 | GPU-Z 2.71 主张 | 我方现状 | 建议裁决卡 |
|---|---|---|---|---|
| ① | 时钟域显示名 | Crossbar Clock | Xbar | P0：CLI 渲染层加显示别名 `Xbar (Crossbar)`，枚举名不动 |
| ② | 视频域显示名 | Video Clock | Msd（域 20/21） | P0：显示别名 `MSD (Video)`——RTSS/nvidia-smi/GPU-Z 三方一致用 video，我们孤例用 MSD |
| ③ | L2 域归属 | L2 Clock（独立显示） | Hub（域 4） | P1：E7 实测 Hub 频率 vs GPU-Z L2 后改名；在此之前不动 |
| ④ | PCIe 吞吐单位 | GB/s | MiB/s（pcie_tx_mibps） | P0：键名/显示二选一——建议显示层换算 GB/s 并保留 JSON 原值 |
| ⑤ | PWR 轨名 | PWR_SRC Power Draw | PWR PP SRC | P0：显示层统一 PWR_SRC（与 241 通道 GPU-Z 名对齐） |
| ⑥ | Board 轨名 | Board Power Draw | InputTotalBoard/"Total Board Power Draw" | 维持现状（描述符名为权威，GPU-Z 名已在 gpu_z_rail_name 层）——无冲突 |
| ⑦ | Chip 轨名 | GPU Chip Power Draw | InputNvvdd/"NVIDIA GPU VDD" | 维持现状（246 同号互证语义一致）；可选别名 |
| ⑧ | 显存占用键名 | Memory Used (Dedicated)/(Dynamic) | get-info memoryInfo 键 | P1：对齐键名或在 README 记映射 |
| ⑨ | 利用率键名 | GPU Load / Memory Controller Load / Video Engine Load / Bus Interface Load | dynamic pstates 百分比条目 | P1：CLI 渲染层采用 GPU-Z 同款四名 |
| ⑩ | 通道类型 0x10/0x11 | ThermChannel 有效温度类型 | 我方 ThermChannelGetStatus 未做类型过滤 | P0（技术性）：ThermChannel 读取补类型过滤，防非温度通道串入 |
| ⑪ | PowerMonitor 通道索引 | 通道 ID×0x2C 直接索引 | ch0+0x44 + 启发式偏移+置信度 | P1：E5 A/B——若 0x2C 通则成立，disambiguate_power_rails 可删启发式 |
| ⑫ | 风扇状态结构 | V2 = 0x20AB0 | 我方 FanCoolerGetStatus 封装戳记需复核 | P0（技术性）：核对我方戳记是否 ver2\|0xAB0 |

## 11. 可借鉴设计

1. **传感器强制通道键**（Force_WDDM_Mem_Sensor）：用户级配置强制某传感器走特定后端——可用于我方 per-rail 置信度覆盖。
2. **派生合成传感器**（Hottest Memory Chip）：多通道 argmax 上屏，降低读表成本。
3. **传感器对象统一模型**：0xD8 字节对象 + 名称/单位/回调三元组 + 统一注册器（sub_5FF800 块模式），类似我方 GpuOperation 全触点清单的结构化方向。
4. **加密命名 + 代际化驱动接口**（GPU-Z-v8）：低阶驱动的版本化命名空间设计（仅记录，不复刻）。

## 12. 跨机验证 E-matrix

| 实验 | 内容 | 命令/探针 | 卡 | 期望回填 |
|---|---|---|---|---|
| E1 | GPU-Z 2.71 与 `nvoc get-status` 并排快照（同屏同刻） | 手动并排 + 截图 | 本机 1070/K4000 | §8 表逐行核实 |
| E2 | 2.71 共享内存导出面 | GPU-Z 运行时枚举 Section 对象（Process Explorer / `handle.exe GPU-Z`） | 本机 | 导出面现名（GPUZShm 后继？）与记录布局 |
| E3 | I2C 传输后端归属 | windbg 断 NvAPI_I2CReadEx 入口 + GPU-Z 读 Board Power | 本机 | 0x64A080 块读走 NVAPI 还是自带 IOCTL |
| E4 | ThermChannelGetInfo vs GetStatus A/B | nvoc 探针：0x0BC8163D ver1\|0x44 15 通道 | 1070 | 类型 0x10/0x11 语义 + 20B 记录布局 |
| E5 | PowerMonitor 通道×0x2C 通则 | nvoc 探针：状态缓冲按 0x2C 步进扫 | 4060L | 0x2C 通则成立→删启发式 |
| E6 | 旧 99→122 delta 重建 | 归档 02-conversations GPU-Z 会话日志深挖 | — | 逐条新增 ID 清单 |
| E7 | Hub == L2？ | nvoc all_clocks 域 4 vs GPU-Z L2 Clock | 1070 | §8 ③ 裁决 |

## 13. 决断清单（供裁决）

- **P0（静态证据足，直接落）**：§10 ①②④⑤＋技术性 ⑩⑫；§9-10 xOCD 双佐证 SET 并卡。
- **P1（A/B 后落）**：§9-1 MeasureFreq 接线、§9-2/3/4 get-info 补全、§9-5 I2CReadEx 封装评估（含安全设计）、§10 ③⑧⑨⑪。
- **P2（设计借鉴，另开任务）**：§11-1/2。
- **不做**：§9-8/9（escape 回退、内核驱动）。

## 14. 来源与置信度

- **实证（反汇编直证）**：122 ID 清单及完备性（双 resolver+噪声过滤+字节扫描三角验证）；Sensors 78 块名字与调用集合；每轨功率双路径（0x1059C/0x2C/246/247）；温度三级链（0x10044/0x7037）；风扇 0x20AB0；nvml 零引用；GPUZShm 零明文；GPU-Z-v8 低阶驱动语境。
- **推断（需 E 卡）**：0x64A080 传输层归属；GPU-Z-v8 完整命名构成；共享内存导出面现状；GetThermalSettings 调用点逐层归因；Hub=L2。
- **局限**：静态 only（无运行时 trace）；32 位无反编译器，全程 capstone 手动数据流；旧 99 ID 清单未逐条重建（E6）。
- **关联**：per-rail-power.md（WinRing0 结案）§2/§5 与本报告 §5 互补；xOCD 审计 §3 表 A 类与本报告 §3-A 同源互证。
