# Afterburner "out of warranty"（过保模式）二进制差分验证

- 日期：2026-10-18
- 样本：`reverse/afterburner/` = MSI Afterburner **4.6.7.16935**（OOW 公布前），
  `reverse/afterburner2/` = **4.6.7.19412**（Beta 5，OOW 首发版，2026-10-07 构建）
- 触发：微信文章《什么叫"过保模式"：微星Afterburner正式开放新功能，让用户可以对GPU底层控制》
  （https://mp.weixin.qq.com/s/Xg4t4615j-nkZK8UGOfQzg ，转述 videocardz
  https://videocardz.com/newz/msi-afterburner-unlocks-hidden-nvidia-gpu-controls-through-out-of-warranty-mode ）
- 方法：两版本全树字节级关键词扫描（ASCII/UTF-16LE）+ RTHAL.dll C++ 符号面差分 +
  `.oem*` 数据库解析。二进制本体不入库（见 `../README.md` 政策）；按用户指令仅入库
  RTCore 驱动两个 blob：RTCore64.sys = `bbe7afcf`，RTCore32.sys = `dd15c364`
  （两目录完全同 blob，2024-05-13 构建，未随 OOW 变更）。

## 裁决总表

| 文章论断 | 16935（前） | 19412（OOW） | 裁决 |
|---|---|---|---|
| GPU 名旁显示 "out of warranty" 标签 | 无此串 | exe 内 `%DescShort% out of warranty` UI 格式串 | ✅ 实锤 |
| build 19412 随附 Afterburner.oem3 | 无 .oem3 | 明文 469B，标题即 "Out of warranty hardware control path" | ✅ 实锤 |
| 底层控制"一直存在，只是被要求隐藏" | RTHAL 符号类集合与 19412 **零差异** | — | ✅ 直接实锤（差分无新增类） |
| 电压轨访问 / 独立电压偏移 | 24 款 VRM 芯片直写实现 + `PS20RelVoltageControl` | oem3 增 `VDDC/AUX_Generic_Detection` + `AuxDesc=MSVDD` 第二轨；exe 新增 `MVDDCDesc/AuxDesc/Aux2Desc` 轨描述符 | ✅ 底座早就在，OOW 放开轨描述 |
| 热/功耗策略检查 | `IsPerfPolicyActive/Supported`、`GpuGet/Set/RestoreCoolerPolicyTable` | 同（无新增） | ✅ 一直在 |
| MMIO 直访 | `MMIOUserMode/MMIOPageSize/PCIAccessMode/PCIBusFilter` 配置 + RTCore64.sys `MmMapIoSpace` | 同 | ✅ 一直在 |
| 私有时钟域监控/调节（XBAR/SYS 字样） | 无 "XBAR" 字面串 | 同样无 | ⚠️ 无字面证据；域机制（`ApplyClockDomainIdWorkaround`/`FindClockDomainIndex` + `NV_GPU_PERF_CLOCK_TABLE`）按数字 ID 处理任意域，oem3 未含 XBAR 条目，该子功能应在后续下发数据库/数值接口中 |
| SPEEDO 系数 / 漏电流诊断报告 | 无 | 无（`Help/Properties/General/GPU_TYPE` 的 leakage 是 legacy 高/低漏电选择器文案，两版相同，非 OOW 诊断） | ❌ 19412 包内不存在；推断走 NVAPI 数值 ID 或后续下发的 oem/插件 |

## `.oem3` 全文（OOW 解锁数据库本体）

```ini
;OEM

; Version 1.0, last edited on 07.10.2026

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
; Out of warranty hardware control path
; for reference design NVIDIA series
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

[VEN_10DE&DEV_????&SUBSYS_????????&REV_??]
Priority				= 1
MemoryClockBoostMax			= 3999000
VDDC_Generic_Detection 			= 0100h
AUX_Generic_Detection			= 0300h
AuxDesc					= MSVDD
;DefaultCoreVoltageBoost		= 0
;DefaultAuxVoltageBoost			= -50
```

要点：

- 通配段 `[VEN_10DE&DEV_????&SUBSYS_????????&REV_??]` 覆盖**所有 NVIDIA 公版卡**——
  OOW 不是逐卡白名单，而是"公版一律解锁"。
- `Priority = 1` 使其压过 `.oem/.oem1/.oem2` 的逐设备条目。
- `MemoryClockBoostMax = 3999000` kHz：显存偏移上限放开到 ~4 GHz（GDDR7 量级）。
- `VDDC_Generic_Detection=0100h` / `AUX_Generic_Detection=0300h`：通用（非逐卡校准）
  电压轨检测方式码；`AuxDesc = MSVDD` 给第二轨命名。
- 加载器是版本通用的：两版 exe 均含 `%s\MSIAfterburner.oem%d` 格式串（按序号枚举
  加载 .oem/.oem1/.oem2/.oem3…）。16935 差的不是加载器，是这份解锁数据。

## RTHAL.dll 符号面（两版本一致，即"功能一直在"的物证）

NV 硬件抽象层 `CNVAPIWrapper`（经 `nvapi_QueryInterface` 调私有 ID）：

- 时钟域：`ApplyClockDomainIdWorkaround(NV_GPU_PERF_CLOCK_TABLE*)`、
  `SetClockDomainIdWorkaround(DWORD)`、`FindClockDomainIndex(...)`；
  监控侧 16935 `PLLClockMonitoring` → 19412 更名 `EffectiveClockMonitoring`（唯一 cfg 键级差异）。
- 电压：`Get/SetPS20RelVoltageControl`（PState 2.0 相对电压控制）；
  exe 侧 `UnlockVoltageControl/UnlockVoltageMonitoring/ForceConstantVoltage` 配置键。
- 策略：`IsPerfPolicyActive/IsPerfPolicySupported`（性能策略）、
  `GpuGetCoolerPolicyTable/GpuSetCoolerPolicyTable/GpuRestoreCoolerPolicyTable`
  （`NV_GPU_COOLER_POLICIES_STATUS_PARAMS_V1`、`NV_GPU_SETCOOLER_LEVEL_V2`）。
- 功率：`NvAPI_GPU_PowerMonitorGetInfo/GetStatus`。

VRM 直写家族 `CHALVRMImplementation*` —— 24 款控制器芯片，I2C/PMBus 级独立电压
控制路径：CHL8214/8228/8266/8318（Chil）、IR3567B/IR3595A、L6788A、
MP2888A/MP29816A/MP2988（MPS）、NCP4206/NCP81022（ON）、
UP1637/1816/6204/6208/6218/6262/6266/9512（uPI）、VT1165/VT1556、Generic。

MMIO 通道：RTCore64.sys（`MmMapIoSpace`/`MmUnmapIoSpace`）+ 用户态
`MMIOUserMode/MMIOPageSize/PCIAccessMode/PCIBusFilter` 配置面，两版一致。

## 与 nvoc 工作的衔接

1. **第三方旁证**：OOW 的"公版全解锁 + 电压轨 + 显存抬窗 3999 MHz"与我们 Turing
   610.47 宽表抬窗（Max 槽同抬）属同一攻击面量级；AB 的 PrivateCaps 走的也是
   NVAPI 私有 ID 面，可交叉比对我们的 PerfClientLimits/VoltRails 偏移表。
2. **`AuxDesc` 第二/第三电压轨（MVDDC/Aux/Aux2）**：与 nvapi-rs VoltRails
   逐 rail 布局解码（escape 0x07000191 / RM 0x2080B2xx 族）语义对齐，佐证
   多轨描述符是消费级工具可达面。
3. **oem 数据库 = AB 版的 domain-mask manifest**：逐设备能力表 + 优先级门控，
   与 nvoc profile 的域掩码设计同构；`Priority` 字段先于内容匹配。
4. OOW 未给 HAL 新增任何代码路径——解锁纯粹是**数据驱动**（数据库 + 配置键），
   这与我们"驱动能力早已存在，缺的是正确的传参模态"的系列结论一致。

## 局限

- 只验证了安装包随附文件；文章宣称的 XBAR/SYS 域调节、SPEEDO/漏电流诊断在
  19412 包内无字面证据，不能据此否定其存在（数值 ID 无字符串痕迹属常态，
  且 oem3 版本号 1.0 表明后续会增量下发）。
- `.oem/.oem1` 为同头加密 blob，未解密；`MSIAfterburner.dat`（RTUI 资源容器）
  未深挖。
