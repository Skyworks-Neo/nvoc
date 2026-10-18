# Afterburner 4.6.7.19412 NVAPI 私有接口全表面 + 命名偏差验证 + P0~Pn 功能差异 Tier 表

- 日期：2026-10-18
- 样本：`reverse/afterburner2/RTHAL.dll`（4.6.7.19412，sha256 fd2cdf34…84aa）
- 方法：`nvapi_QueryInterface` 全 74 个解析桩字节级提取（`push imm32; call [QI]`），
  经惰性缓存全局→forwarder→具名 `CNVAPIWrapper::*` 方法三跳挂接语义；MSVC 符号
  保留完好，映射为静态证据（非猜测）。
- 上游：[oow-mode-verification.md](oow-mode-verification.md)（OOW 数据库侧验证）

## 1. ID → AB 方法全表

### 1.1 OOW 核心：电压轨 + 风扇 Coolers 客户端族

| NVAPI ID | RTHAL 桩 | AB 方法 | 备注 |
|---|---|---|---|
| `0x465F9BCF` | 0x10002950 | `ClientVoltageRail_GetStatus(hPhysical, pStatus)` | 76B 表（版本字 0x1004C），40B/轨条目，µV 域（0x10624DD3 魔数 ÷1000 转 mV） |
| `0x9DF23CA1` | 0x10002A20 | `ClientVoltageRail_GetControl(pHandleIdx)` | 轨控制面读 |
| `0xB9306D9B` | 0x10002AF0 | `ClientVoltageRail_SetControl(pHandleIdx)` | 轨控制面写（OOW "独立电压偏移"通道） |
| `0xFB85B01E` | 0x10003990 | `ClientFanCoolers_GetInfo` | Coolers 信息 |
| `0x35AED5E8` | 0x10003A60 | `ClientFanCoolers_GetStatus` | Coolers 状态 |
| `0x814B209F` | 0x10003B30 | `ClientFanCoolers_GetControl/GetStatus` | Coolers 控制读 |
| `0xA58971A5` | 0x10003C00 | `ClientFanCoolers_SetControl` | Coolers 控制写（cooler policy table 客户端通道） |

### 1.2 V/F 曲线与 boost 族

| NVAPI ID | AB 方法 | 备注 |
|---|---|---|
| `0xE440B867` | `VFCurve_GetCurve(VOLTAGE_FREQUENCY_CURVE_DESC*)` | 曲线描述符读 |
| **`0x39442CFB`** | **`VFCurve_SetCurve`** | **命名偏差 #1：nvoc 叫 PerfClientLimits SET**（7 字段锁表写器）。AB 把同一接口当 V/F 曲线主写入器。两个名字、一个接口 |
| `0x64B43A6A` | `VFCurve_GetClockBoostRanges` | 各域 boost 范围查询 |
| `0x507B4B59` | `VFCurve_GetClockBoost/GetCurve/SetClockBoost`（10 xrefs） | boost 读/写 |
| `0x0733E009` | `VFCurve_GetClockBoost/GetCurve/SetClockBoost/SetCurve` | |
| `0x9ABDD40D` | `VFCurve_SetClockBoost/SetCurve` + 2 个未名函数 | |
| `0x23F1B133` | `VFCurve_GetCurve/SetCurve/InitCaps` | |

### 1.3 PS20（PStates 2.0）家族

| NVAPI ID | AB 方法 | 备注 |
|---|---|---|
| `0x843C0256` | `GetPstate` | 公开已知 GetPStates20 |
| `0x6FF81213` | `InitCaps / PS20_GetClock / PS20_GetClockBoost` | 公开已知 SetPStates20（同 ID 多用途） |
| `0xCDF27911` | `SetPstate` | |
| `0xBA94C56E` | `GetPstate / SetPstate` | |
| `0x927DA4F6` | `GetPstate` | 公开已知 SetVFPCurve（AB 用于 PState 读路径） |
| `0x0F4DAE6B` | **`PS20_SetClockBoost / PS20_SetRelVoltageBoost / PS20_SetVoltageBoost`** | 公开已知 SetPerfClocks；AB 的 CoreVoltageBoost%（电压 boost）写通道。命名偏差 #2：nvoc 叫 SetPerfClocks，AB 主用其为 boost 写器 |
| `0x2A0A350F` | `GetPerfClocks`（+`InitLocations`） | 公开已知 GetPerfClocks |
| `0x1EA54A3B` | `GetPerfClocks`（第二形态，per-pstate 表） | `ApplyClockDomainIdWorkaround` 的 NV_GPU_PERF_CLOCK_TABLE 来源 |

### 1.4 策略（热/功/性能钳制源）

| NVAPI ID | AB 方法 | 备注 |
|---|---|---|
| `0x409D9841` | **`GetActivePerfPoliciesMask / GetSupportedPerfPoliciesMask`** | "哪个钳制源在生效"读数面（性能策略：thermal/power/voltage/…） |
| `0x3D358A0C` | `GetActivePerfPoliciesMask` | |
| `0x0D258BB5` | **`GetThermalControl / GetThermalControlRange / SetThermalControl`** | 热控制器（slowdown/shutdown 阈值族），≠ nvoc 的 sensor temperature limit |
| `0xE9C425A1` | `GetThermalControl / VFCurve_Get/SetCurve` | |
| `0x34C0B13D` | `SetThermalControl / VFCurve_SetCurve` | |
| `0x34206D86` | **`GetPowerControl / GetPowerControlRange / SetPowerControl`** | NVAPI 功率控制族（nvoc 功率写走 kmd 0x470807/NVML，此为第三通道） |
| `0x70916171` | `GetPowerControl / VFCurve_Get/SetCurve` | |
| `0xAD95F5ED` | `SetPowerControl / VFCurve_SetCurve` | |
| `0xE3640A56` | `GetThermalSettings` | 公开已知 |
| `0xEDCF624E` | `GetRelPower` | 相对功率读数 |

### 1.5 监控/枚举/I2C/杂项

| NVAPI ID | AB 方法 | 备注 |
|---|---|---|
| `0xCEEE8E9F` | `GetFullName`（经 0x1004BDA0） | 公开已知，抽样校验点 |
| `0x1BE0B8E5` | `GetAllClocks` | 公开已知 |
| `0x5F608315` | `GetFanSpeedRPM` | 公开已知 GetTachReading |
| `0x2DFDB66E` | `GetPCIIdentifiers` | |
| `0xA561FD7D` | `GetVbiosVersionString` | |
| `0x60DED2ED` | `GetUtilizations` | |
| `0xDCB616C3` | `GetClocks` | |
| `0x07F9B368` / `0x774AA982` | `GetDisplayDriverMemoryInfo` | |
| `0x0078DBA2` | `GetConnectedDisplays` | |
| `0x20DE9260` / `0x22A78B05` / `0x4888D790` / `0x175167E9` | 显示器枚举族 | |
| `0x0694D52E` | `GetCustomDisplay` | |
| `0x283AC65A` / `0x4D7B0709` | `I2CWrite / I2CRead / I2CWriteAndRead` | **VRM 直写传输层**（24 款 `CHALVRMImplementation*` 芯片驱动走此通道） |
| `0x0694D52E` `0xDAD9CFF8` `0x375DBD6B` `0xFCBC7E14` `0x577DD202` `0x73BF8338` `0xE4A26362` `0xDA8466A0` | `Get/SetFramerateLimit`、`IsFramerateLimitControlSupported` | **勘误（§4.2）**：这 8 个 ID 不是独立 FPS 接口族，而是 **DRS 驱动设置注册表**（NvAPI_DRS_Create/DestroySession/Load/SaveSettings/Set/Get/DeleteProfileSetting/GetBaseProfile）。AB 的帧率限制 = DRS 全局设置：setting id `0x10834FEE`，值 `fps\|0xF0000000`，GET 读回 u16 半字 |
| `0xF951A4D1` `0x2926AAAD` `0xE5AC921F` `0xD9930B07` `0xBAABBFCC` | Init/SelectDevices/GetSystemType 路径 | |
| `0xD22BDD7E` | `Uninit` | 公开已知 NvAPI_Unload，抽样校验点 |

## 2. 命名偏差验证（AB 名 vs nvoc 名）

| # | 接口 | AB 命名 | nvoc/nvapi-rs 现名 | 裁决 |
|---|---|---|---|---|
| N1 | `0x39442CFB` | **VFCurve_SetCurve**（V/F 曲线主写入器） | PerfClientLimits SET（7 字段锁表） | **同名异职需合并**：同一接口。AB 语义证明该接口是"客户端 VFP 写通道"总入口——锁表只是其一种载荷形态。nvapi-rs 文档/命名应记双名；我们 3060 "SET 先应用后报错"的实测与 AB 的 SetCurve 前置读（先 GetCurve/GetThermalControl/GetPowerControl 后写）行为对齐 |
| N2 | `0x0F4DAE6B` | PS20_SetVoltageBoost/SetRelVoltageBoost/SetClockBoost | nvapi 公开名 SetPerfClocks | AB 把它当 boost 写器用；nvoc 若接此 ID 应按 AB 语义暴露为 boost 写入而非 clocks 写入 |
| N3 | 电压轨族 | **ClientVoltageRail**（NVAPI QueryInterface 层，µV，多轨表） | **VoltRails**（escape 0x07000191 / RM 0x2080B201/B202/F214 层） | 同域两层：AB 走 NVAPI 客户端族，nvoc 走更深的 RM/escape 族。语义对齐（轨枚举+控制+状态），补全时以 AB 版本字 0x1004C/40B 条目结构为参考实现 |
| N4 | 风扇策略 | **ClientFanCoolers**（cooler policy table 客户端通道） | nvoc fan-curve（SetFanCurve，曲线语义） | 互补非冲突：policy（每 cooler 模式表）vs curve（温度-RPM 点） |
| N5 | 热策略 | **ThermalControl**（slowdown/shutdown 控制器） | nvoc thermal（SetTemperatureLimit，sensor limit） | 不同域：ThermalControl 是"控制器阈值"，TemperatureLimit 是"传感器上限"；文章"thermal policies 检查"落到 ThermalControl 族 |
| N6 | 功率 | **PowerControl**（NVAPI 族） | nvoc power（kmd 0x470807 / NVML） | 三通道并存：NVAPI PowerControl 是文章"power policies"的 NVAPI 面，nvoc 未接 |

## 3. 功能差异 P0~Pn Tier 表（补全清单）

| Tier | 项 | 接口 | nvoc 现状 | 补全方案 | 风险 |
|---|---|---|---|---|---|
| **P0** | 电压轨 NVAPI 客户端通道（get/set control + status） | `0x465F9BCF` / `0x9DF23CA1` / `0xB9306D9B` | VoltRails 走 escape/RM（get-volt-rail-info 已实机验证），无 NVAPI 客户端族 | nvapi-rs 新增 `client_voltage_rail_*` 三函数（76B/0x1004C 结构已解码）；CLI `get-volt-rail-info --client` 对照读 | 写侧需 `--force` + pre-ampere 门（沿用 GpuType 纪律） |
| **P0** | 活跃钳制源掩码 | `0x409D9841` / `0x3D358A0C` | get-lock-status 有 PerfClientLimits 状态，无"当前哪个 policy 在钳"读数 | `get-perf-policies`（supported+active 掩码解码），诊断价值最高：三机功耗钳源谜团的直读面 | 只读，零风险 |
| **P0** | 0x39442CFB 双名归档 + SetCurve 前置读序列 | 同上 N1 | PerfClientLimits 命名未记 AB 语义 | 文档修正 + `set-vfp-locks` 复刻 AB 的写前读序列（GetCurve→写），提升 Ampere 钳制诊断 | 文档+流程，无新写通道 |
| **P1** | 电压 boost %（PS20） | `0x0F4DAE6B`（boost 语义） | profile `voltage.boost_percent` 域存在但写通道单薄 | `set-voltage-boost`（PS20_SetVoltageBoost/Rel 变体），对照 AB `CoreVoltageBoost` | 中：boost 写后必须回读验证 |
| **P1** | 热控制器（slowdown/shutdown） | `0x0D258BB5` 族 | thermal 域仅 sensor limit | `get/set-thermal-control` + range；与 temperature-limit 并列暴露 | 中高：shutdown 阈值误写风险，默认只读 |
| **P1** | 功率控制 NVAPI 通道 | `0x34206D86` / `0x70916171` / `0xAD95F5ED` | 功率写走 kmd 0x470807（毒写通道，SET 先应用后报错） | `set-power-limit --channel nvapi` 第三通道对照实验，验证是否绕开 kmd 钳制 | 实验性，`#[ignore]` 硬件门 |
| **P1** | 风扇 Coolers policy 表 | `0xFB85B01E` / `0x35AED5E8` / `0x814B209F` / `0xA58971A5` | fan-curve 已有（曲线语义） | `get-fan-coolers`（info/status/control 读全走）+ `set-fan-cooler-control`（模式表写） | 写侧需恢复路径（restore 现值） |
| **P2** | boost 范围查询 / 相对功率 | `0x64B43A6A` / `0x507B4B59` / `0xEDCF624E` | 无 | 只读补全：`get-vf-boost-ranges`、`get-rel-power` | 只读 |
| **P2** | I2C 直写 VRM 通道 | `0x283AC65A` / `0x4D7B0709` | 无（kmd MMIO 另有通道） | **不实现**，仅接口 + 24 芯片清单建档（CHALVRM 族表）；留待有明确仲裁需求再立项 | 高：卡级硬件写 |
| **P3** | FramerateLimit 族 / 显示器族 / GetCustomDisplay | 8+5 IDs | 无 | 不实现（非 OC 域） | — |

## 4. 实现勘误（2026-10-18，补全计划落地时核实）

逆向产出后对 nvapi-rs 子模块与主仓库的现状核查表明：**本表 P0/P1 的 9 个接口 ID
全部已在 nvapi-rs 绑定**，第 3 节的缺口认定据此修正：

| 本表原判定 | 实际现状 | 真实缺口 → 落点 |
|---|---|---|
| P0 电压轨 Client 族（0x465F9BCF/0x9DF23CA1/0xB9306D9B） | sys `NV_GPU_CLIENT_VOLT_RAILS_*` + 中层 `core_voltage/voltage_boost` + hi `GpuStatus.voltage`/`GpuSettings.voltage_boost` + CLI `get/set/reset-public-gpc-rail-volt-boost` 全通 | 无 |
| P0 钳制源掩码（0x409D9841/0x3D358A0C） | sys/中层 `perf_info/perf_status` + hi `GpuInfo.perf`/`GpuStatus.perf` 已通；0x550 的 active 掩码此前裸透传 | 已落地：`PerfStatus.active/timer/policy_timers` 深解码（nvapi-rs 209457b）+ CLI `get-perf-policies` 位名化诊断 |
| P1 热/功策略 | sys+中层+hi+CLI 全通：`thermal_limit_info/thermal_limit/set_thermal_limit`（V3/V2 回退）、`power_limit_info/power_limit/set_power_limit`（V2/V1 回退）；写侧 `SetPublicTgpPercent`（power SET）、`SetNvapiTargetTemp`/`SetNvapiSensorLimits`（thermal SET） | 无 |
| P1 风扇 Coolers | sys cooler.rs:533-627 + 中层 + CLI `set-fan-speed --rpm`/`set-fan-curve --activate` 在用 | 无 |
| P1 电压 boost（0x0F4DAE6B） | CLI `set-public-gpc-rail-volt-boost` | 无 |

### AB 名 ↔ nvapi-rs 名对位（N1–N6 的最终裁决）

| AB 19412 RTHAL 符号 | 实际 ID | nvapi-rs 名 | 备注 |
|---|---|---|---|
| `VFCurve_SetCurve/GetCurve` | 0x39442CFB / 0xE440B867 | `PerfClientLimitsSet/GetStatus` | 双名一接口；SetCurve 写前先拉 ThermalPolicies/PowerPolicies GET（已记入 sys doc comment） |
| `Get/SetPowerControl`、`GetPowerControlRange` | 0x34206D86/0x70916171/0xAD95F5ED | `ClientPowerPolicies*`（`power_limit_info/power_limit/set_power_limit`） | AB 名义上的 "PowerControl" 就是 PowerPolicies 族；nvoc 的 kmd 0x470807 通道是另一条臂 |
| `Get/SetThermalControl`、`GetThermalControlRange` | 0x0D258BB5/0xE9C425A1/0x34C0B13D | `ClientThermalPolicies*`（`thermal_limit_info/thermal_limit/set_thermal_limit`） | nvoc 另有 private GET-prime 0xC4554575 处理 mobile GetStatus 瞬态值 |
| `GetRelPower` | 0xEDCF624E | `ClientPowerTopologyGetStatus` | AB 把 PowerTopology 状态当相对功率读 |
| `ClientVoltageRail_Get/Get/SetControl` | 0x465F9BCF/0x9DF23CA1/0xB9306D9B | `ClientVoltRails*`（`core_voltage`/`voltage_boost`） | 同域同名异拼 |
| `GetFramerateLimit` 族（8 ID） | 0xDAD9CFF8 等 | 未绑 | P3 维持不实现 |

另记：nvapi-rs 同时存在 `...Private` 后缀的同名 ID 变体（如
`ClientPowerPoliciesGetInfoPrivate` 0x67F31384）——两代平行 ID 族；AB 19412 走的是
**非 Private** 族（0x34206D86 等），与 nvid.rs:329-331/743-745 一致。

## 5. 对既有结论的修正/佐证

1. **宽表抬窗的第三方量级佐证**：AB 用同代接口把 GDDR7 显存抬到 3999 MHz（oem3
   `MemoryClockBoostMax`），与 Turing 610.47 宽表 Max 槽同抬同量级。
2. **"SET 先应用后报错"**：AB `VFCurve_SetCurve`（0x39442CFB）写前先拉
   GetThermalControl/GetPowerControl/GetPowerControlRange——与我们在 3060 上观测的
   "Applied 212000 但被钳"行为互证：钳制发生在驱动侧策略层，不是客户端传参错误。
3. **XBAR/SYS 域无字面串的解释补强**：时钟域走 `0x1EA54A3B`（per-pstate
   NV_GPU_PERF_CLOCK_TABLE）+ `ClockDomainIdWorkaround` 按**数字域 ID** 处理——
   AB 从不按名引用域，文章的 XBAR/SYS 说法只能是外部解读，19412 包内无法证实。

## 局限

- SetControl 侧（电压轨/fan coolers）参数块未逐字段解码——补全实现时需在
  idalib 中反编译 `0x1004dcf0`/`0x1004da30` 提取控制结构（plan 内已列为第一步）。
- `.oem/.oem1` 加密 blob 未解密；oem2（设备能力表）仅结构层面确认。

## 6. 勘误 v2（2026-10-18，P2/P3 补全落地时核实）

### 6.1 §1.5 "FPS 限制器族" 更名 DRS

交叉比对 nvapi-rs `nvid.rs:595-631` 证实上表 8 个 ID 全部是 `NvAPI_DRS_*`（第一行
原文误写 `0xF020614A`/`0xDAD9CFF8` 顺序亦有误，实为 CreateSession `0x0694D52E` 领衔）。
AB 19412 的 `Get/SetFramerateLimit`/`IsFramerateLimitControlSupported` 是 **DRS 设置
读写** 的薄封装：`CreateSession→LoadSettings→GetBaseProfile→Get/SetSetting→SaveSettings
→DestroySession`，全局（无 GPU 句柄）。

### 6.2 SetControl 参数块逐字段解码完成（原"局限"销账）

- **`ClientVoltageRail_SetControl`**（RTHAL 0x1004dcf0）：40B 控制块，version 0x10028，
  控制值写 +4 的**低字节**；GetControl（0x1004dba0）读同一字节。与 nvapi-rs
  `NV_GPU_CLIENT_VOLT_RAILS_CONTROL_V1.percent`（u32 @+4，AmpereOC/HYDRA 交叉）
  对齐——AB 用 u8 是实现取舍，字段语义同为 boost 百分比。
- **`ClientFanCoolers_SetControl`**（RTHAL 0x1004da30）：RMW（先 GET 再改再 SET 全块），
  version 0x105AC，count 取 +8 低字节；每 cooler 条目 44B stride，`level` @条目+4、
  `flags` bit0=manual @条目+8——与 nvapi-rs
  `NV_GPU_CLIENT_FAN_COOLERS_CONTROL_V1`（44B 条目：cooler_id/level/flags）逐字段吻合。

### 6.3 P2/P3 塌缩与落地清单（相对第 3 节）

| 第 3 节项 | 落地核查/结果 |
|---|---|
| P2 boost 范围（0x64B43A6A） | 已在中层+hi：`vfp_ranges` → `GpuInfo.vfp_limits`（`get-info` 自动携带）——无需新命令 |
| P2 相对功率（0xEDCF624E） | 已在 hi：`GpuStatus.power: BTreeMap<PowerTopologyChannelId, Percentage>`（`get-status` 携带） |
| P2 I2C 通道 | 中层 `i2c_read/i2c_write` 已有；本轮补 CLI `get-i2c`（读）+ `set-i2c --force`（写，卡级硬件写门控） |
| P3 FRL 族 | 本轮落地：nvapi-rs sys `drs.rs`（8 FFI + NvDRSSetting 0x13020/12320B）+ 中层 `framerate_limit/set_framerate_limit` + CLI `get/set-framerate-limit --force` + pynvoc `query_framerate_limit` |
| P3 显示器族/GetCustomDisplay | 维持不实现（非 OC 域） |
| P0/P1（§4 勘误节） | 同前：全部已绑定；本轮另落 `get-perf-policies` 位名化诊断与 PerfStatus `_ns` 单位后缀 |

### 6.4 单位记法

PerfPolicies status 的计时器字段统一 SI 记法：类型字段 `timer_ns`/`policy_timers_ns`，
CLI 输出键同名——全仓库无 `Ns` 拼写（单位大小写混用零容忍）。
