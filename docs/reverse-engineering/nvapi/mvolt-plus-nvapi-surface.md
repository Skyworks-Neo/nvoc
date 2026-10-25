# mVolt+ (0.50.5) NVAPI 接口面全量逆向与 nvoc 封装缺口对照

- 样本:`reverse/mvolt+/mVolt+.exe`,PE32+ x64 原生(非 .NET),3.7 MB
  - SHA-256 `7ef2614769a6d438198bc4e6c3053469d351574515abb7a934b4dc7358047dcb`
- 工具:idalib(headless,session `05ccfd4a`)+ 磁盘侧 Python 解码器
  `reverse/mvolt+/decode_ids.py`(从 PE 文件原始字节解码,不依赖 IDA 会话,可独立复算)
- 结论速览:**mVolt+ 的 NVAPI 表面是我们 nvapi-rs 情报库的真子集——42/42 ID 已知、42/42 有
  raw 类型化封装、41/42 有 safe 封装;唯一未封装的 0x57B5A5DF 是我们 RE 后有意跳过的
  (R575.74 上 UNRESOLVED,GetControl 已取代)。真正的缺口在业务消费层,不在封装层。**

## 1. 程序身份

- 自述 `application_version 0.50.5`,报告 schema `mVolt+.compat.v2`。
- 定位:GET-only 兼容性/遥测报告工具。报告 JSON 自我声明:
  `"safety":{"get_only":true,"setters_resolved_but_never_called":true,"address_scan_performed":false,"i2c_performed":false}`,
  并声明不采集序列号/用户名/文件路径。
- 双栈:NVAPI(`LoadLibraryExW("nvapi64.dll")` + `nvapi_QueryInterface`)+ NVML
  (`nvmlInit_v2` 等,同样整表解析;`nvmlDeviceSetGpuLockedClocks` 仅出现在解析表中)。

## 2. 接口面的组织方式(逆向方法)

报告生成器 `sub_140013E90` 遍历两张接口描述表输出 JSON(每条 `{name,id,access,resolved}`):

1. **明文表**(VA `0x1402C4BB8`,7 条):`{name:QWORD, id:u32, access:QWORD}`,24 字节步长,直接
   QueryInterface。
2. **混淆表**(表体 VA `0x1402C4860`,35 条;`{name:QWORD, idx:u8, access:QWORD}`):
   - ID 由一台 18 字节小程序 VM 计算:`prog = VA 0x1402EE6E0 + 18*idx`,`key = u32 @VA 0x1402EE460 + 4*idx`;
   - 密钥流:`ks(i) = splitmix64_finalizer(seed − 0x61C8864680B583EB·i) >> 56`,`seed = key | 0xA53B112800000000`,
     位置 p 的字节与 `ks(p+1)` 异或;
   - VM 指令:`0x09 acc=imm32` / `0x58 acc=rol32(acc,u8)` / `0x93 acc+=imm32` / `0xCE acc^=imm32` / `0xCF 返回`;
   - imm32 为 4 字节 LE,逐字节同一密钥流解码(解码器实现见 `decode_ids.py`,35/35 解码成功,结果全部落回
     已知 NVAPI ID,构成自证)。
- `access` 只有两值:`GET` 与 `SET_NOT_CALLED`(字符串 VA `0x1402C3BC8` / `0x1402C3C90`)。
- 逐卡遥测在 `sub_1400112F0`(JSON per-GPU);`sub_14019EF60` 是 GUI 侧轻量 GPU 名解析路径。
- 另:静态核对报告生成器内联的 Initialize(0x0150E828)/EnumPhysicalGPUs(0xE5AC921F)/驱动版本查询均
  为公开 ID。

## 3. 全量接口面(7 + 35)与 nvoc 各层覆盖对照

覆盖列:`ID`=nvid.rs 收录;`raw`=sys 层 nvapi! 类型化封装;`safe`=medium/safe 层真实调用(非注释);
`biz`=nvoc 业务层(core/tui/gui/pynvoc)已消费。

### 3.1 明文表(全部 GET,全部公开文档化 API)

| ID | 名称 | ID | raw | safe | biz |
|---|---|---|---|---|---|
| 0xCEEE8E9F | NvAPI_GPU_GetFullName | ✓ | gpu/mod.rs:77 | ✓ | ✓(GPU info) |
| 0xA561FD7D | NvAPI_GPU_GetVbiosVersionString | ✓ | gpu/mod.rs:155 | ✓ | ✓ |
| 0x2DDFB66E | NvAPI_GPU_GetPCIIdentifiers | ✓ | gpu/mod.rs:239 | ✓ | ✓ |
| 0x1BE0B8E5 | NvAPI_GPU_GetBusId | ✓ | gpu/mod.rs:312 | ✓ | ✓ |
| 0x2A0A350F | NvAPI_GPU_GetBusSlotId | ✓ | gpu/mod.rs:319 | ✓ | ✓ |
| 0xD8265D24 | NvAPI_GPU_GetArchInfo | ✓ | gpu/mod.rs:508 | ✓ | ✓ |
| 0x46FBEB03 | NvAPI_GPU_GetPhysicalFrameBufferSize | ✓ | gpu/mod.rs:124 | ✓ | ✓ |

> 注:`GetPCIIdentifiers = 0x2DDFB66E` 与我们 nvid.rs 独立记录一致(社区旧表常写作 0x2DFDB66E)。
> mVolt+ 运行时解析该 ID 并输出 resolved 状态,构成第二个独立来源的交叉验证。

### 3.2 混淆表(35 条,VM 解码)

| # | ID | 名称 | access | ID | raw | safe | biz |
|---|---|---|---|---|---|---|---|
| 00 | 0x2C73AFDC | VoltVoltRailsGetInfo | GET | ✓ | power.rs:468 | ✓ | ✓(operation.rs volt_rails) |
| 01 | 0x5D0634EE | VoltVoltRailsGetStatus | GET | ✓ | power.rs:529 | ✓ | ✓ |
| 02 | 0xA3070DB0 | VoltVoltRailsGetControl | GET | ✓ | power.rs:521 | ✓ | 库内可用,biz 未消费 |
| 03 | 0x87C55C8A | VoltVoltRailsSetControl | SET_NOT_CALLED | ✓ | power.rs:537 | ✓ | 未消费(我们写电压走 ClientVoltRails 0xB9306D9B/0xB9306D9B 族) |
| 04 | 0x68789E2A | ClockAdcDevicesGetInfo | GET | ✓ | clock.rs:5740 | ✓ | 未消费 |
| 05 | 0x43D9B26A | ClockAdcDevicesGetStatus | GET | ✓ | clock.rs:5770 | ✓ | 未消费 |
| 06 | 0x6FF81213 | NvAPI_GPU_GetPstates20 | GET | ✓ | pstate.rs:376 | ✓ | ✓ |
| 07 | 0x0F4DAE6B | NvAPI_GPU_SetPstates20 | SET_NOT_CALLED | ✓ | pstate.rs:493 | ✓ | ✓(核心写路径) |
| 08 | 0x34206D86 | ClientPowerPoliciesGetInfo | GET | ✓ | power.rs:900 | ✓ | ✓ |
| 09 | 0x70916171 | ClientPowerPoliciesGetStatus | GET | ✓ | power.rs:949 | ✓ | ✓(TGP 限制读取链) |
| 10 | 0xAD95F5ED | ClientPowerPoliciesSetStatus | SET_NOT_CALLED | ✓ | power.rs:953 | ✓ | ✓(set_power_limits) |
| 11 | 0x507B4B59 | ClockClientClkVfPointsGetInfo | GET | ✓ | clock.rs:536 | ✓ | ✓ |
| 12 | 0x21537AD4 | ClockClientClkVfPointsGetStatus | GET | ✓ | power.rs:826 | ✓ | ✓(VFP 曲线) |
| 13 | 0x23F1B133 | ClockClientClkVfPointsGetControl | GET | ✓ | clock.rs:448 | ✓ | ✓(core/src/nvapi.rs:1118) |
| 14 | 0x0733E009 | ClockClientClkVfPointsSetControl | SET_NOT_CALLED | ✓ | clock.rs:453 | ✓ | ✓(core/src/nvapi.rs:1142) |
| 15 | 0xC12EB19E | PowerMonitorGetInfo | GET | ✓ | power.rs:2299 | ✓ | **未消费** |
| 16 | 0xF40238EF | PowerMonitorGetStatus | GET | ✓ | power.rs:2380 | ✓ | **未消费** |
| 17 | 0x927DA4F6 | NvAPI_GPU_GetCurrentPstate | GET | ✓ | pstate.rs:117 | ✓ | ✓ |
| 18 | 0xEDCF624E | ClientPowerTopologyGetStatus | GET | ✓ | power.rs:1987 | ✓ | **未消费** |
| 19 | 0x7F7F4600 | NvAPI_GPU_GetPerfDecreaseInfo | GET | ✓ | gpu/mod.rs:412 | ✓ | 未消费(NVML throttle reasons 已有) |
| 20 | 0x64B43A6A | ClockClientClkDomainsGetInfo | GET | ✓ | clock.rs:496 | ✓ | ✓ |
| 21 | 0x1BD69F49 | NvAPI_GPU_GetAllClocks | GET | ✓ | clock.rs:214 | ✓ | ✓ |
| 22 | 0x57B5A5DF | ClockClkDomainsGetInfo(私有) | GET | ✓ | clock.rs:2090 | **✗(有意)** | 未消费 |
| 23 | 0xF58938F5 | ClockClkDomainsGetControl(私有) | GET | ✓ | clock.rs:2097 | ✓ | ✓(clk_domains_control) |
| 24 | 0xD14B69CF | ClockClkDomainsSetControl(私有) | SET_NOT_CALLED | ✓ | clock.rs:2106 | ✓ | ✓(set_clk_domain_offset) |
| 25 | 0x527FC458 | ClockClkDomainsMeasureFreq | GET | ✓ | clock.rs:2124 | ✓ | ✓(clk_domain_freq_direct) |
| 26 | 0xE440B867 | PerfClientLimitsGetStatus | GET | ✓ | clock.rs:636 | ✓ | ✓(VFP 锁状态) |
| 27 | 0x39442CFB | PerfClientLimitsSetStatus | SET_NOT_CALLED | ✓ | clock.rs:643 | ✓ | ✓(set_vfp_lock) |
| 28 | 0xE826E4F0 | ClockClkPropTopRelsGetInfo | GET | ✓ | clock.rs:2417 | ✓ | 未消费 |
| 29 | 0xCBFF71D0 | ClockClkPropTopRelsGetControl | GET | ✓ | clock.rs:2425 | ✓ | 未消费 |
| 30 | 0xEF3D20EA | ClockClkPropTopRelsSetControl | SET_NOT_CALLED | ✓ | clock.rs:2438 | ✓ | 未消费(RE 注释:SET 无特权门,高危) |
| 31 | 0xFB85B01E | ClientFanCoolersGetInfo | GET | ✓ | cooler.rs:535 | ✓ | 未消费 |
| 32 | 0x35AED5E8 | ClientFanCoolersGetStatus | GET | ✓ | cooler.rs:569 | ✓ | 未消费 |
| 33 | 0x814B209F | ClientFanCoolersGetControl | GET | ✓ | cooler.rs:626 | ✓ | 未消费 |
| 34 | 0xA58971A5 | ClientFanCoolersSetControl | SET_NOT_CALLED | ✓ | cooler.rs:632 | ✓ | 未消费(我们写风扇走 set_cooler_levels 旧族) |

## 4. Tier 表(以"nvoc 侧缺口"分级)

- **Tier 0 — 已封装且已进业务层,无缺口**:见上表 biz=✓ 的 24 项(含全部 4 条核心写路径:
  SetPstates20、ClockClientClkVfPoints Set/GetControl、PerfClientLimitsSetStatus、
  ClockClkDomainsSetControl,以及 volt_rails / clk_domain_freq* 遥测链)。**无需动作。**
- **Tier 1 — nvapi-rs 已完整封装(raw+safe),业务层未消费;mVolt+ 运行时解析佐证其可用性**:
  1. PowerMonitorGetInfo/GetStatus(0xC12EB19E/0xF40238EF)— 实际功耗遥测(rail 分解瓦数)
  2. ClientPowerTopologyGetStatus(0xEDCF624E)— 供电来源拓扑(内建/外接/connector),对笔记本
     TGP 策略与 617.42 TGP TDR 归因直接有用
  3. NvAPI_GPU_GetPerfDecreaseInfo(0x7F7F4600)— 降频/节流原因位图(NVAPI 侧;NVML 侧已有)
  4. ClockAdcDevicesGetInfo/GetStatus(0x68789E2A/0x43D9B26A)— 电压 ADC 采样点枚举与读数
  5. ClientFanCoolersGetInfo/GetStatus/GetControl(0xFB85B01E/0x35AED5E8/0x814B209F)— 新族风扇遥测
  6. VoltVoltRailsGetControl(0xA3070DB0)— volt-boost 当前控制状态
  7. ClockClkPropTopRelsGetInfo/GetControl(0xE826E4F0/0xCBFF71D0)— GPC→XBAR 拓扑关系/频率比
  8. ClientPowerPoliciesGetStatus(0x70916171)已进 TGP 链、列此存档
- **Tier 2 — 有 raw、无 safe,属有意跳过(非缺口)**:
  - ClockClkDomainsGetInfo 0x57B5A5DF(clock.rs:2090 注释:R575.74 返回 UNRESOLVED,
    GetControl 0xF58938F5 已取代其发现功能)。维持现状;仅在 hi 层文档补一句互相引用。
- **Tier 3 — mVolt+ 有而我们无知的接口:0 个。**

## 5. 补齐规划(2026-10-20 已实施,见 §7 落点)

1. **P0(零代码)**:无 ID/raw 缺口需要补。双源交叉验证(0x2DDFB66E)已补进
   `nvapi-rs/sys/src/nvid.rs` 的 GetPCIIdentifiers 行注释。✅
2. **P1(消费已有封装)**:实施中发现 Tier-1 名单需修正——PowerMonitor、PowerTopology、
   GetPerfDecrease、ClientFanCoolers 四族**早已在 `hi::GpuStatus` 中聚合**(hi/gpu.rs),真正的
   缺口是 pynvoc 序列化层丢弃了它们。已补:✅
   - pynvoc `normalize_status` 新增 `perf_decrease`(raw 位图 + 解码标志)、`power_topology`
     (按通道百分比)、`power_monitor_channels`(描述符 + 友好轨名 + raw 读数)三个键。
   - 唯一没有任何 hi 路径的 ADC 族:新增 `hi::Gpu::adc_devices()`(组合 info+status,拒绝→None)
     → core `QueryNvapiAdcDevices` operation → pynvoc `query_adc_devices()`。
   - FanCoolers 遥测原本就经 `status.coolers` 序列化,无需动作。
3. **P2(doctor 自检探针)**:✅ 三层落地——
   - `core/src/doctor.rs`:`nvapi_doctor_report()` 对 54 个精选 ID(nvoc 全部写路径 + dashboard
     读族,mVolt+ 同测者标 `(mVolt+)`)逐个 QueryInterface 判空;含唯一性单测。
   - pynvoc `nvapi_doctor()`:`{interface_version, load_error, all_resolved, missing[], entries[]}`。
   - CLI `nvoc-cli doctor`:Human/JSON 双输出,有缺失时退出码非零(可做驱动升级冒烟门)。
4. **P3(明确不做)**:ClockClkDomainsGetInfo 0x57B5A5DF 保持不封装;VoltVoltRailsSetControl /
   ClockClkPropTopRelsSetControl 已封装但维持业务层不接入(高危写,现无需求)。

## 6. 复算指引

```bash
cd reverse/mvolt+ && python decode_ids.py
```

输出两表全量(明文 7 + 解码 35)及 VM 程序样本。解码正确性自证:35/35 程序均以合法 opcode 序列
`09/93/CE → 58 → CF` 收尾且结果全部命中已知 NVAPI ID——错误的密钥流几乎不可能同时满足这两点。

## 7. 字面/字段名差异清单(2026-10-20 补查)

### 7.1 mVolt+.compat.v2 schema 全字段(供 doctor 输出对齐参考)

顶层:`schema`、`application_version`、`created_utc`、`safety{get_only, setters_resolved_but_never_called,
address_scan_performed, i2c_performed}`、`privacy{usernames_collected, serial_number_api_called,
filesystem_paths_collected, opaque_numeric_driver_buffers_included}`、`host{os, architecture}`、
`driver{status, version, display, branch}`、`interfaces[{name,id,access,resolved}]`、`gpus[]`、
`native_limit_diagnostics`。

GPU 对象:`index`、`name{status,value}`、`vbios{status,value}`、`pci{status, device_id, subsystem_id,
extended_device_id, revision_id}`、`location{bus_status, bus, slot, slot_status}`、`architecture{status,
architecture_id, implementation_id}`、`framebuffer{status, size_kib}`、`raw_get_probes[{name, id, status,
size, raw_base64, called}]`、`volt_legacy_sweep[]`。

**已采纳**:nvoc doctor 的 `{name, id, resolved}` 条目形制与 mVolt+ `interfaces[]` 对齐(便于两份报告
并排 diff);raw_get_probes 的"把 opaque 缓冲区 base64 收进报告"模式未采纳(nvoc 的 raw dump 已有
专用命令)。**命名无争议项**:我们的 pynvoc 键(power_topology / perf_decrease / power_monitor_channels)
与既有键族(power_w / coolers)一致,未引入 mVolt+ 命名。

### 7.2 意外发现:mVolt+ 主程序是一个 vBIOS limit 编辑器(与 vbios-parser-enhanced 直接相关)

compat 报告的 `native_limit_diagnostics` 牵出了它 GUI 主体的完整 vBIOS 功率/时钟限制编辑子系统:

- 方法草稿:`mvolt.limit-method-draft.v1..v6`,两种方法 **`bios`**(写 vBIOS 镜像,需重启让驱动
  重新消费)与 **`direct`**(运行时直写);"The driver has not consumed the confirmed working input"、
  "Restart Windows to complete override removal"。
- 可编辑 bound 类别字面名:`Invalid power bound`、`Invalid current bound`、`Invalid OCP bound`、
  `Invalid clock minimum/maximum/endpoint`、`Invalid memory bound/maximum`、`Invalid PWM frequency
  ownership`("Choose 25 or 27 kHz")、`Invalid fan channel evidence`、Speedo 电压校准输入
  ("Speedo is not a BIOS image field"、original/confirmed/staged/reverted 四态)。
- 事务化恢复存储:recovery record(checksum、size bound、transaction revision、boot identity、
  适配器绑定校验,D3DKMT 枚举 + `windows-device-v1:` 身份前缀 + `SUBSYS_` 匹配),"The recovery
  record belongs to another adapter"、"Reload recovery storage before saving"。
- BIOS profile 感知:"The confirmed input changed the original BIOS profile selection"。

这与本分支(vbios-parser-enhanced)的 parser 工作同一领域:mVolt+ 已把"改 vBIOS power table →
驱动消费 → 可恢复"的完整工程问题做了一遍,其错误字符串枚举等价于一张**边界条件检查清单**。

### 7.3 争议项(交由用户决断,未实施)

1. **是否追 vBIOS limit 编辑器线索**(§7.2):价值高(与 vbios 分支直接互补,相当于一份已上市的
   边界条件规格书),但 scope 大(需要逆向它的 limit-method-draft v1→v6 演进与 recovery 存储
   格式)。建议单独任务。
2. **是否给 nvoc 写路径引入 mVolt+ 式持久化恢复 journal**:我们写路径有 snapshot→SET→readback→
   restore,但没有跨重启的持久化 recovery record。mVolt+ 的错误串表明它为"写到一半断电/驱动未
   消费"建了完整状态机。若 nvoc 要做 vBIOS 级写入,这层绕不开;仅 NVAPI 运行时写则现状够用。
3. **PowerMonitor raw_status 单位未验证**(power.rs 注释自述 units unconfirmed):doctor/遥测已
   透出原始值,是否安排一次上机标定(对齐 GPU-Z 读数)由用户定。
4. **0x57B5A5DF 维持不封装**:我们的 RE 结论(UNRESOLVED on R575.74)与 mVolt+ 仍将其列入探针表
   不矛盾(解析≠可用),维持 Tier 2 现状;若用户希望对齐 mVolt+ 的"全量列出"风格,可以只补 raw
   注释,不建议加 safe 层。
