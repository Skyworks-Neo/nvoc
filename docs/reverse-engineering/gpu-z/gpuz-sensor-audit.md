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

## 9. 查漏补缺清单（裁决后实施状态）

> 用户裁决（2026-10-05）：「其他读取面全补」。实施前逐项核对发现 §9 首版**高估了缺口**——`clk_domain_freq_direct`（0x527FC458，hi/gpu.rs:890）、ECC 双 GET（sys/src/gpu/ecc.rs + GpuStatus.ecc）、GetSerialNumber（含 4060L 活体注释）、GetRamMaker、GetFBWidthAndLocation、core/shader_pipe/shader_sub_pipe/partition 计数均为**既有实现，误报撤销**。

**本批已补（nvapi-rs sys/mid/hi + CLI `get-info` 的 `compute_caps` 节）**
1. `NvAPI_GPU_GetVPECount`、`GetRasterBackendCount`、`GetTotalTPCCount`、`GetTotalSMCount`、`GetTotalSPCount`——五个新计数 FFI + mid 方法 + `PhysicalGpu::compute_caps()` 聚合（NotSupported/NoImplementation → None）。
2. `NvAPI_GPU_GetActiveOutputs`——FFI + mid + compute_caps。
3. `NvAPI_GetValidGpuTopologies`——FFI + 静态方法（系统级无句柄参数）；`GetPerGpuTopologyStatus` 已有未再包。
4. 分区计数 `partition_count` mid 方法补齐（sys FFI 已在，hi 层此前无入口）。

**评估后维持不封装**
5. `NvAPI_I2CReadEx/I2CWriteEx`——已注册不封装维持（GPU-Z 用它做 INA3221/PMBus 直达；任意设备/寄存器写面 = 高危，评估为 P2 独立任务：提权门控 + 设备白名单 + 只读先行）。
6. `GetHybridPadInfo`——随本批未做（低频，OEM 二合一混合垫信息，无消费方）。

**结构性不做（维持既有设计决策）**
7. ExtEscape 0x7037 内核回退——GDI escape 通道不符合我方抽象层路线（4060L 结论）。
8. 低阶驱动 v8 直读——不做内核驱动（WinRing0 边界结论维持；Microsoft 脆弱驱动黑名单侧原因同旧档）。

**xOCD 交叉印证（写路径）**
9. 0x0F4DAE6B SetPstates20 与 0x0733E009 ClkVfPointsSetControl：GPU-Z 独立解析佐证；xOCD 车道后续判读修正 0x0733E009 本有 set_vfp_table 封装（真实缺口=几何矛盾），以该判读为准。

## 10. 命名冲突 E-matrix（已裁决，2026-10-05）

| # | 矛盾 | GPU-Z 2.71 主张 | 我方现状 | **用户裁决与实施** |
|---|---|---|---|---|
| ① | 时钟域显示名 | Crossbar Clock | Xbar | **不改**（维持 Xbar） |
| ② | 视频域显示名 | Video Clock | Msd（域 20/21） | **已实施**：`Msd/Vid` 双标签（nvenum_display + CLI 两处 domain_name） |
| ③ | L2 域归属 | L2 Clock（独立显示） | Hub（域 4） | **已实施**：`Hub/L2C` 双标签（用户指定；E7 归因实验不再阻塞改名） |
| ④ | PCIe 吞吐单位 | GB/s | MiB/s（pcie_tx_mibps） | **不改**（维持 MiB/s） |
| ⑤ | PWR 轨名 | PWR_SRC Power Draw | PWR PP SRC | **已实施**：ampereoc_rail_name 241 → `PWR_SRC` |
| ⑥ | Board 轨名 | Board Power Draw | InputTotalBoard | **维持现状** |
| ⑦ | Chip 轨名 | GPU Chip Power Draw | InputNvvdd | **维持现状** |
| ⑧ | 显存占用键名 | Memory Used (Dedicated)/(Dynamic) | memoryInfo 键 | **维持现状** |
| ⑨ | 利用率键名 | GPU Load 等四名 | dynamic pstates 条目 | **维持现状** |
| ⑩ | 通道类型 0x10/0x11 | ThermChannel 有效温度类型 | 未做类型过滤 | **不采纳** |
| ⑪ | PowerMonitor 通道索引 | 通道 ID×0x2C 直接索引 | ch0+0x44 + 启发式 | **进一步确认**：E5 探针首证已落（见 §12），跨代再验后删启发式 |
| ⑫ | 风扇状态结构 | V2 = 0x20AB0 | 我方封装 0x210A8 | **进一步确认**：静态核对完成（同 ID 异 size），活体仲裁挂 E-⑫ |

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
| E5 | PowerMonitor 通道×0x2C 通则 | `cargo test -p nvapi --release --test gpuz_powermonitor_0x2c_live -- --ignored --nocapture` | 本机卡**已跑**（2026-10-05）+ 待 4060L | **首证成立**：v1\|0x59C 戳被接受（OK），活通道落 0x2C 网格（+0x2C/+0xB0/+0xDC≈12.2M），启发式偏移（+0x44 等）全零——待 4060L 复验后删 disambiguate_power_rails 启发式 |
| E6 | 旧 99→122 delta 重建 | 归档 02-conversations GPU-Z 会话日志深挖 | — | 逐条新增 ID 清单 |
| E7 | Hub == L2？ | nvoc all_clocks 域 4 vs GPU-Z L2 Clock | 1070 | 归因确认（改名已按用户裁决先行落地为 Hub/L2C 双标签） |
| E-⑫ | FanCoolerGetStatus size 仲裁 | 探针：同 ID 分别传 0x210A8 / 0x20AB0 | 活体卡 | 驱动接受哪个（或双收）→ 决定我方戳记是否改 0x20AB0 |

## 13. 决断清单（裁决后状态）

- **已落**：§10 ②③⑤（命名双标签/轨名）、§9-1~4（compute_caps 补全，nvapi-rs@439ea37 + 主仓 459a26e 卷入的 CLI 节）、§10 ⑪⑫ 的静态/首证部分（探针 gpuz_powermonitor_0x2c_live.rs + cooler.rs 戳记注释）。
- **用户不改**：§10 ①④⑥⑦⑧⑨。
- **不采纳**：§10 ⑩。
- **待确认后落**：§10 ⑪（4060L 复验 0x2C → 删启发式）、⑫（size 仲裁）。
- **P2（另开任务）**：§9-5 I2CReadEx 封装评估（提权+白名单）、§11-1/2 设计借鉴。
- **不做**：§9-7/8（escape 回退、内核驱动）。

## 14. 来源与置信度

- **实证（反汇编直证）**：122 ID 清单及完备性（双 resolver+噪声过滤+字节扫描三角验证）；Sensors 78 块名字与调用集合；每轨功率双路径（0x1059C/0x2C/246/247）；温度三级链（0x10044/0x7037）；风扇 0x20AB0；nvml 零引用；GPUZShm 零明文；GPU-Z-v8 低阶驱动语境。
- **实证（活体，裁决批新增）**：E-⑪ 首证（v1|0x59C 接受 + 0x2C 网格活通道 + 启发式偏移全零，本机卡 2026-10-05）；GetSerialNumber 4060L 既有活体注释（二进制字节非字符串）。
- **推断（需 E 卡）**：0x64A080 传输层归属；GPU-Z-v8 完整命名构成；共享内存导出面现状；GetThermalSettings 调用点逐层归因；0x2C 通则跨代普适性（仅本机一卡）；0x210A8 vs 0x20AB0 驱动接受度。
- **局限**：静态 only（无运行时 trace）；32 位无反编译器，全程 capstone 手动数据流；旧 99 ID 清单未逐条重建（E6）；§9 首版缺口清单含误报（已在裁决批 §9 修正）。
- **实施批**：nvapi-rs v0.2.x@439ea37（E-⑪ 探针 + E-⑫ 戳记注释 + clippy 全绿修复）；CLI compute_caps 节与命名双标签由并行 xOCD 车道卷入主仓 459a26e；本报告修订与子模块指针 bump 为 cli-more-reversing 实施批 commit。
- **关联**：per-rail-power.md（WinRing0 结案）§2/§5 与本报告 §5 互补；xOCD 审计 §3 表 A 类与本报告 §3-A 同源互证。
