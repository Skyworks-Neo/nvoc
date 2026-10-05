# xOCD 逆向审计 × nvapi-rs 查漏补缺（2026-10-05）

目标：`reverse/xocd/xOCD.exe`（第三方超频工具，UI 见会话截图：NVVDD/MSVDD 轨窗、XBAR/SYS/UPROC 域控制、OCP 电流限、XOC 解锁）。
方法：ilspycmd 全源码反编译 + idalib（外层启动器）静态逆向，与我们 nvapi-rs 逐 ID 对照。
对照基线：主仓 main@fd7c43a（本报告落 `cli-more-reversing` 分支），nvapi-rs 子模块 v0.2.x@acb8a1d。
详细证据台账（不入库）：`reverse/xocd/agentA-control.md`（写路径）、`reverse/xocd/agentB-telemetry.md`（读路径）。

## 1. 版本锁定与跨机复现流水线

| 产物 | SHA256 |
|---|---|
| xOCD.exe（原始） | `21ea98a2c592e82fa81d63f4a6c3d0f5e6c5e87edbc17ee6df6db66621d10a1f` |
| xocd-app.exe（切出的托管主程序） | `6c21244a53aa9b3a2c5a7c6cecb74bd0f80dcd15dab06726ca6a2df45e3dc073` |
| xocd-native2.bin（切出的 D3D12 基准 worker） | `2c87e773c12559735a071d44045571e8a3db9dde7ab933d16c9c7e94c0bcbc5b` |

复现步骤（任何机器，仅需 Python3 + .NET SDK + ilspycmd）：
1. xOCD.exe 为 **原生启动器（332KB PE，6 节）+ 4.77MB overlay**；节表结束于文件偏移 0x4BE00。
2. overlay@0x4BE00 起内含两个 PE：托管主程序 @overlay+0x200（≈4.9MB，含唯一 BSJB），D3D12 worker @overlay+0x432358（≈343KB，导入 d3d12/dxgi/D3DCOMPILER_47）。按 MZ+`PE\0\0` 校验切出。
3. `ilspycmd -p -o <dir> xocd-app.exe` → 190 个 .cs，**未混淆、符号全保留**，命名空间 `xOCD` 扁平单目录。本报告所有 `文件:行号` 引用均指该反编译根（本机在 `C:/Users/YLW-XLAB/ida-scratch/xocd-decomp`）。

## 2. 工具概览与驱动访问通道

技术栈：C# WinForms（.NET 框架依赖式单文件发布），关键文件 `NvApiSource.cs`(4331 行) / `NvVfApi.cs` / `NvidiaController.cs` / `NvmlSource.cs` / `MainForm.cs`。

写路径四通道（按优先级）：
1. **NVAPI 私有接口直调**（主体）：`nvapi64.dll!nvapi_QueryInterface(id)` + Cdecl 函数指针，版本戳 `(ver<<16)|size` 手工写入缓冲区 dword0；全部调用被一把 `_sync` 锁串行化（NvApiSource.cs:338）。
2. **nvidia-smi 子进程**：仅 GPU Clock Range（`-lgc/-rgc`，NvidiaController.cs:1840-1862）与常规卡板功率（`-pl`，:3669）。
3. **NVML**：core/mem 时钟 offset 首选路径（:3897-3953），显存做「公共 NVML offset + 私有域 kHz 补充」双段拆分。
4. **PawnIO 签名模块**（`xOCD.Nvidia.bin` 资源）：只读 Blackwell 热点（PCI 直读，绕过驱动），不在写路径。

读路径额外三通道（外设电流）：NVAPI I2CReadEx（板载 ITE IT8915FN，仅 ROG Astral 白名单）→ MSI Ai1300/1600TS PSU USB HID（PMBus Linear11）→ Thermal Grizzly WireView Pro II 串口（100B 帧，任意卡可用）。**全程无 0x0700xxxx RM escape 直调、无驱动安装、无内核组件**；I2C write 接口只做能力探测从未真写。

## 3. NVAPI ID 交叉验证总表

**xOCD 使用的 55 个 NVAPI ID 在我们 `nvid.rs` 中 55/55 已注册，无一个未知 ID**（反编译中另一大数 0x53695748 是 HWiNFO 共享内存魔数，非 NVAPI）。

| 增量类型 | 条目 |
|---|---|
| A. 已注册 + **安全层未封装的写路径** | `0xD14B69CF` ClockClkDomainsSetControl（nvid.rs:1892；src/ 0 引用）；`0xCBFF71D0`/`0xEF3D20EA` TopRels Get/SetControl（:1890/:1900；0 引用）；`0x0733E009` ClkVfPointsSetControl（:770；仅 GET 封装）；`0x0F4DAE6B` SetPstates20（:294；0 引用） |
| B. 已封装但**语义需修正** | `0x8B3E7343`/`0xAFFC2279`：我们只注为 TGP-watt（nvid.rs:1404-1466）；xOCD 证明同接口条目 (policyId,subtype)=(19,13)/(19,12) 是 **NVVDD/MSVDD OCP 电流（raw mA）**，legacy fallback (13,19)/(14,19) |
| C. 布局/语义增量 | `0xE440B867` BoostLock 七域表（type2@domain0/1=时钟范围锁、type3@domain6=电压锁，780B v2，count@8==7、entry@12+i*24）；`0x65FE3AAD` ThermChannel 索引语义（0=GPU、1=hotspot、显存=2(RTX50)/7(RTX40)/9(其它)，i32/256=°C）；`0x57B5A5DF` v4 34476B 域路由发现（mask@8、slot id@176+i*1072，id 0..255 须唯一）；`0x6FF81213` **v2** 7416B（RTX50 isolated-P0 时钟写，ReferenceClock.cs）；轨族新锚点 status.values[7]@+32=rail 最低电压、ctrl.values[0/1/3]=maxDelta/maxCorr/minCorr 标定组 |
| D. 双侧一致（复核通过） | 0x64B43A6A（2344B 域范围）、0x507B4B59（6188B mask）、0x21537AD4（7208B 曲线状态）、0xDCB616C3、风扇/温度族、ClientVoltRails boost、PerfClientLimits v2 0x30C |

## 4. 控制面逐控件机制（UI → 驱动落点）

Apply 通用模型（NvidiaController.ApplyCoreAsync :1387-1989）：**staging 全在 UI/Profile 层**，点 Apply 才按固定顺序落驱动：
`voltage policy → boost → nvvdd → msvdd → msvdd_ratio → core → memory → core_voltage → 各域时钟 → 各域电压 demand → 温度目标 → 功率 → OCP×2 → clock_range → 风扇 → boost 收尾 → 总读回`；每步压入回滚闭包，任一步失败逆序全回滚 + 汇总异常；undo 可重放；重复 Apply 幂等跳过。

| UI 控件 | 通道/ID | 单位 | 机制要点 |
|---|---|---|---|
| NVVDD/MSVDD range (mV) | VoltVoltRails 0xA3070DB0→0x87C55C8A | µV | 写 ctrl 三标定字段（+76/+80/+88）；范围=驱动 status（非 vBIOS）；硬夹 250-2000mV |
| Core Voltage offset | ClkDomains 0xF58938F5→0xD14B69CF 电压字段 | µV | UI −25..+50（XOC 扩展 −500..+500）；驱动侧固有 ±500mV |
| Core/Mem Clock offset | NVML→Pstates20 0x0F4DAE6B fallback；RTX50 走 v2 7416B 隔离 P0 | kHz | 显存双段拆分（先降私段→写公共→升私段） |
| GPU Clock Range | **nvidia-smi -lgc** | MHz | 已有 BoostLock 时钟锁时拒绝再写电压锁 |
| XBAR/SYS/HUB/UPROC Clock | 0xD14B69CF 时钟字段 | kHz | 绝对频率表述；xOCD 硬包络 −100..+1000（Video/Uproc −100..+500） |
| MSVDD Clock (7000) | TopRels 0xCBFF71D0→0xEF3D20EA | 16.16 定点 | ratio 0.7–1.2，raw=ratio*65536/10000，值@+104（与我们 rec0+0x68 同一字段） |
| XBAR/SYS/(Video) Voltage | 0xD14B69CF 电压字段 | µV | RTX50 校验 PrivateId1→slot1、tag==15 |
| Video / NVD | 0xD14B69CF + 公开读 0xDCB616C3 slot8 | kHz | **NVD=Video 别名**（归一化合并，冲突拒载） |
| Memory Clock (420) | Pstates20 P0 domain4 | DDR 显示单位=raw/2 | 上限用「试写→读回→恢复」探测 |
| Power Limit 125% | smi `-pl`；RTX50: 0x70916171+0xAD95F5ED | raw=W/default×100000 | 读回误差>1.1W 判失败 |
| NVVDD/MSVDD OCP (A) | 0x67F31384+0x8B3E7343+0xAFFC2279 | **raw mA**（UI×1000） | 见 §5 |
| Voltage Boost 100% | 0x9DF23CA1/0xB9306D9B；无原生 boost 卡→轨窗映射 | % | `ceiling=StandardMax+(BoostMax−StandardMax)·%/100`（5mV 取整）；**boost 最后落、轨窗后补写**（boost 写会重置轨窗；RTX3080 1118mV 组合序列为实证） |
| V/F 曲线编辑器 | 0x507B4B59+0x23F1B133+0x0733E009+0x21537AD4 | kHz×deltaScale | 点 0 锚点禁改、逐点读回；deltaScale=2 当 arch∈[0x130,0x140) 且 range=±2MHz（Pascal 约定判据，可抄） |
| 温度目标 | 0x0D258BB5+0xE9C425A1+0x34C0B13D | ℃×256 | 恰好一条 P0 未激活策略才写 |
| 风扇 | ClientFan 0x814B209F+0xA58971A5（legacy 三件套兜底） | % | 读回 ±2% 容差 |

## 5. 核心发现一：OCP 真身

- OCP = 私有 power-channel 接口（**就是我们标注 TGP-watt 的同一三件套**）里 `(policyId=19, subtype)` 的电流条目：**NVVDD=(19,13)、MSVDD=(19,12)**，legacy fallback (13,19)/(14,19)。
- info（0x67F31384，v4 2672B，entry@56+i*88）：`min@+16 / default@+20 / max@+24`，raw 单位 **mA**；default 即 UI 文案里的 "firmware default"（驱动/vBIOS 报告）。写值硬钳 1000..5001000 mA。
- control（0x8B3E7343 GET / 0xAFFC2279 SET）：xOCD 常规布局 2636B v1（戳 68172，entry stride40@28、value@i*40+32）；RTX50 另有 **Large 变体**（控制 2393824B、条目 stride 9288@2664、value@+68；info 2859056B）。我们 4060L 实测的 10016B magic 属第三种布局——同 ID 多代际布局并存，封装需按戳分派。
- **"Unlock NVVDD/MSVDD OCP limits" 不写任何驱动解锁位**：开=UI 把滑条顶从 default 放宽到驱动报 max；关=把超默认 profile 值替换回 default 并加入下次 Apply 的「default-释放」步骤（ConstrainForRecovery + StagePowerChannelDefaultsAboveFirmware）。staging 回落语义即此。
- 与 VoltVoltRails 的 values[3]「VRM 墙」**不是**同一概念：那是电压墙（1.2V 类），OCP 是电流限（mA）。

## 6. 核心发现二：三个安全开关全部是 UI/策略门（零驱动状态）

1. **Enable XOC voltage range (up to 1.20 V)**：可用性=驱动自报 OvervoltageMax（轨 status@176 校准）> StandardMax；作用=允许 NVVDD/MSVDD 目标顶到 OV 墙。"1.20V" 是文案非硬编码，真边界永远来自驱动报告 + 2000mV 硬夹。
2. **Extend voltage offsets (−500…+500 mV)**：只改 4 个电压 demand 滑条范围（−25..+50 ↔ −500..+500）；±500000µV 是接口固有钳位，不改任何结构。
3. **Unlock OCP**：见 §5。
4. Profile 层防呆：ProfileOptionPolicy 按值域推断所需开关（profile 值>+50 即需扩展开关），开关显式关闭则拒载。

## 7. 读面/遥测摘要

- 双速轮询：每 tick 轻量链（NVML 时钟/功耗/负载/显存 → NVAPI 电压/温度/hotspot → nvidia-smi → 沿用旧值，四级兜底+合理性钳位），全量能力发现 30s 一次（游戏进程 5min）；默认 2000ms（钳 250ms–1000s）+ 500ms 看门狗。
- **实时轨电压只有 NVVDD 一条**（0x465F9BCF 76B v1，µV@+40，0.25–2V 合理窗）；MSVDD 等其它轨 xOCD 也不读实时值。
- **实时电流（A）完全不来自 NVAPI**：ITE IT8915FN（0x4D7B0709 I2CReadEx，addr 0x56 reg 0x80，24B=6×(大端 mV+_mA)，仅 Astral 白名单 9 个 subsystem ID）→ MSI PSU HID（Linear11，VID 0x0DB0）→ WireView Pro II 串口（@12+j*12：i16 mV / u32 mA / u32 mW，6 针+总计）。我们要的「电流读数」xOCD 也是靠外部硬件补的。
- CSV 仅 9 列（timestamp+8 指标），无 hotspot/外设。
- 无 vBIOS 解析：所有滑条端点=驱动 GET 结构（VoltRailsStatus min@192/max@168 µV、PowerPolicies info、Pstates20 delta 范围、NVML offsets、smi max）+ 少量硬编码安全包络（GPC min 210MHz 纯 fallback：NvidiaController.cs:1212）。
- nvcuda 13 处=内嵌 cubin "bounded_load" 有界负载生成器（1024×256 网格），服务「带载自检」，非遥测。
- xOCD.Nvidia.bin=PawnIO 签名模块（LibreHardwareMonitor 同款 NVIDIA 热点模块）：无签名驱动做安全 PCI/IO 读的可行范式。

## 8. 概念命名映射表（同概念不同名全对齐）

| xOCD | 我们（ClockDomainId） | 裁决状态 |
|---|---|---|
| Core=0 / XBAR=1 / SYS=2 / HUB=3 / Memory=4 | Gpc=0 / Xbar=1 / Sys=2 / Hub=3 / M=4 | 一致 |
| Processor=公开 slot7 | Hotclk=7（文档称 PROCESSOR(7)） | 一致 |
| **UPROC=私有 20** | **Pwr=20** | **用户裁决（2026-10-05）：我方命名维持**；xOCD 的 UPROC 记为 50 系侧叫法 |
| **Video/NVD=私有 21；实时频率走公开 slot8** | **Msd=21；slot8=Pclk0** | **用户裁决：Msd = media subsystem domain，命名正确**——GPU-Z 与 nvidia-smi 均将 MSD 显示为 "video clock"；xOCD 的 Video/NVD=21 判定为 50 系兼容层命名（未考虑老卡），我方不改名（sys 域表注释已落） |
| NVVDD=轨 bit0 / MSVDD=轨 bit1 | power.rs rail_mask bit0/bit1 | 一致（xOCD 硬编码无动态名表） |
| Board Policy Get（0x70916171） | ClientPowerPoliciesGetStatus | 同 ID 不同名 |
| MSVDD clock ratio | 我们无对应物 | 新概念（TopRels 边比率） |
| BoostLock 七域表 | PerfClientLimits（我们无域级解码） | 布局补全 |

## 9. 查漏补缺清单（我们缺的，按证据强度排序）

1. **OCP 电流限读/写**（§5）：sys 层同 ID 已有结构壳，缺 policyId=19 通道解析（mA）与 (19,13)/(19,12)/(13,19)/(14,19) 命名、RTX50 Large 变体。P100 不可验证；4060L/50 系可验证。
2. **域绝对时钟 + 域电压 demand 写**（0xD14B69CF）：XBAR/SYS/VIDEO/HUB/UPROC 时钟与全部轨 demand 电压——我们 0 封装。字段偏移：时钟 slot*772+(tag15?568:560)（PrivateId0:564/556），电压 slot*772+(tag15?576:564)（PrivateId0:572:560）；core/xbar/sys 电压字段 560/572、1336/1348、2880/2892。
3. **TopRels 控制**（0xCBFF71D0/0xEF3D20EA，MSVDD 时钟比率 0.7–1.2，值@+104 16.16 定点）：0 封装；我们已有 GetInfo 侧 U16.16 解析可复用。
4. **V/F 曲线 SET**（0x0733E009）：仅 GET 封装；xOCD 给出条目基 100/stride 36/delta@+24 布局与逐点读回协议。
5. **SetPstates20**（0x0F4DAE6B）v2 7416B RTX50 isolated-P0 时钟写 + P0 offset fallback：0 引用。
6. **BoostLock 七域表解码**（0xE440B867/0x39442CFB）：结构已有，缺域语义（type2@0/1 时钟锁、type3@6 电压锁）与「外部时钟范围锁→拒绝电压锁」消费逻辑。
7. **ThermChannel 索引语义**（0=GPU/1=hotspot/显存=2(RTX50)/7(RTX40)/9(其它)，/256°C）：纯注释级增量。
8. **NVML 增量**：GetClockInfo 三域、Utilization/MemoryInfo、TemperatureV（hotspot，`ver=size|0x1000000`）、ClockOffsets 探测/写入、DeviceGetArchitecture。
9. 反向优势确认：我们的 PowerMonitor per-rail 功率、TopRels 全树、32 域表、V/F 私有曲线族是 **xOCD 没有的**；xOCD 无 per-rail 功率遥测、无 vBIOS 解析、无 escape 直调。

## 10. 差异与矛盾清单（需实机 A/B 裁决，按风险排序）

| # | 矛盾 | 我方现状 | xOCD 主张 | 建议裁决卡 |
|---|---|---|---|---|
| ① | V/F 控制表 delta 偏移 | freqDeltaKHz@entry+20（clock.rs:396 一带） | delta@条目+24（条目基 100/64，stride 36/28） | P100（有 V/F 曲线，本机即可） |
| ② | VoltVoltRails status v2 槽索引 | 注释「按 RAIL BIT」 | 「按 DENSE 序」（base=160+ordinal*172） | 4060L（双轨在线） |
| ③ | TopRels rec0+0x68 比率语义 | GPC→XBAR 比率（0.9） | MSVDD:memory 比率（0.7–1.2） | 4060L（已有 TopRels 树） |
| ④ | 私有域 20/21 命名 | Pwr=20 / Msd=21 | UPROC=20 / Video-NVD=21 | 4060L MEASURE_FREQ 负载行为 |
| ⑤ | 0x8B3E7343/0xAFFC2279 条目语义 | TGP-watt（mW） | OCP 电流（mA，policyId=19 通道） | 4060L 读 default 值数量级即可判 |
| ⑥ | Pascal deltaScale | clock.rs 注释「R610.74 实测纯 kHz」 | arch∈[0x130,0x140) 且 range ±2MHz 时 ×2 | P100 一条读数可定 |

## 11. 可借鉴设计（按采纳成本排序）

1. **写协议范式**：GET 快照→RMW→SET→逐字段读回→失败回滚原快照→二次验证→聚合异常；Apply 级联回滚栈 + undo 重放 + 幂等跳过。应成为 nvapi-rs 写路径封装的标准模式（我们高风险写目前只有部分 readback-verify）。
2. **双速遥测 + 四级兜底**：轻量链每 tick、能力发现 30s/5min，全链合理性钳位——直接对应我们 GUI/TUI 轮询设计，P100 全兼容。
3. **硬件绑定 Profile 存储**：`Profiles/<gpuId|vbios>/*.json` + 版本字段迁移 + 原子写 + 可导出包。
4. **opt-in 安全门控**：三开关按能力禁用 + profile 校验拒绝 + 恢复自动钳回 firmware default + 全屏风险确认（可抑制标记）——与 AGENTS.md「恢复行为可见」原则完全同向。
5. **deltaScale 判据**：arch+range 组合判定 V/F 步进（替代我们按代际硬编码）。
6. **boost 顺序学**：boost 最后落、轨窗后补写（boost 写重置轨窗）+ RTX3080 组合序列。
7. **self-test CLI 档**：单二进制 `--*-self-test` 家族，硬件探针须显式 `--confirm-hardware-writes`——契合我们 xtask ci 分层。
8. **进程隔离基准 worker**：D3D12 独立进程 + stdout `PROGRESS|phase|elapsed|fps` 协议 + JSON 评分（Avg/1%Low/0.1%Low/Consistency）——stressor 评分可借鉴。
9. **PawnIO 路线**：签名用户态模块做无驱动安全 PCI/IO 读（Blackwell hotspot 直读实证）。
10. 启动基线保护：首读即存原始字节，周期比对漂移自动恢复（EnsureNvvddControlBytes）。

## 12. 跨机验证 E-matrix（只读优先；全部为 GET/探测，禁写）

前置：0x0700 系写接口在无裁决前不得实装写路径；E1/E6 只读即可定案。
**探针已就位**（nvapi-rs v0.2.x@6f14581）：E1/E3/E4/E5/E6 跑 `cargo test -p nvapi --test xocd_gap_probe_live -- --nocapture --ignored`（GET-only，JSON 落 `reverse/xocd/`）；E2 用 `--test volt_rails_raw_dump`。

| 实验 | 内容 | 命令/探针 | 卡 | 期望回填 |
|---|---|---|---|---|
| E1 | V/F 表 delta 偏移 ①⑥ | 现有 `get-public-vftable`/私有 VFP GET raw dump（p100 车道 E0 探针） | **P100（本机）** | delta 值与已知好曲线对齐的偏移方（+20 vs +24）；rangeMax/Min 是否 ±2000000 |
| E2 | 轨 status 槽索引 ② | VoltVoltRailsGetStatus 0x5D0634EE raw dump，按 bit 序/dense 序各解一次 | 4060L | 哪组解出合理 vBIOS 墙（≈1050-1100mV 级） |
| E3 | OCP 语义 ⑤ | 0x67F31384 info + 0x8B3E7343 control raw dump，读 (19,13)/(19,12) 与 default@+20 | 4060L | default 数量级=A×1000（OCP 成立）还是 mW（TGP 成立） |
| E4 | 域 20/21 命名 ④ | MEASURE_FREQ 域 20/21 空闲+负载读数 | 4060L | 20 随微码负载变=UPROC/Pwr 判别；21 vs Msd |
| E5 | TopRels 边语义 ③ | 4060L 现有 TopRels 树 + 0xCBFF71D0 GET @104 | 4060L | rec0 边两端域号 |
| E6 | BoostLock 七域表 | 0xE440B867 GET，count 是否 7 | 4060L/P100 | 表存在性与 mode/value 解码 |
| E7（远期） | RTX50 Large OCP / isolated-P0 v2 / Astral ITE | — | 50 系实机（用户暂无） | 搁置，静态结论已足 |

## 13. 决断清单（供裁决；2026-10-05 用户裁决后状态）

- **P0（✅ 已实施，v0.2.x@6f14581，见 §14 台账）**：OCP 通道解析+限值写（双几何检测）、0xD14B69CF 类型化封装、TopRels 控制封装、SetPstates20（offset RMW + RTX50 隔离模板）、BoostLock/ThermChannel/MSD 注释与解码增量、deltaScale 判据。**注意**：写路径已实装但未实机验收——按安全规约，首次实机使用前先跑对应 E 实验裁决布局矛盾。
- **P1（A/B 后落）**：①⑥ V/F 偏移与 deltaScale 判据、④ 域 20/21 更名、TopRels 语义注释。
- **P2（设计借鉴，GUI/TUI/core 层另立任务）**：回滚栈+undo、双速遥测、Profile 存储、opt-in 门控、self-test 档、基准 worker 协议。
- **不做**：外设电流级联（ITE/PSU/WireView）依赖特定硬件白名单，价值低；smi 通道（我们已有原生 NVAPI 等价物）；PawnIO（当前无绕驱动需求）。

## 14. 落地台账（2026-10-05，用户裁决后实施）

用户裁决：①「补充查漏 2 和 3」（四个 SET 写路径 + OCP 真身）→ nvapi-rs **直接提交 v0.2.x（不开分支）**；②域 20/21 维持我方命名（Msd = media subsystem domain）；③可借鉴设计后续再做；④命名等矛盾实验放到另一台机器设计。

**nvapi-rs v0.2.x@6f14581**（直接提交，fmt/clippy/单测全绿，sys 45 + safe 26）：
- `0xD14B69CF` 类型化封装：sys 语义槽位表 `clk_ctrl_entry_v2_semantics`（0x0F→freq=VALUES[2]/volt=VALUES[4]，非 0x0F→[0]/[1]，GPC 类 privateId==0 偏移已文档化）+ safe `set_clk_domain_freq_offset` / `set_clk_domain_voltage_demand`（µV，±500 mV 接口钳位，走既有 RMW 配方）。
- TopRels 封装：`top_rels_ratio` / `set_top_rels_ratio(_raw)`（唯一关系门控 + 快照/回写/读回/回滚全配方，0.7–1.2 包络，0.9 保留 0xE660 硬件字面量；语义命名分歧③注释保留）。
- `SetPstates20`：`set_pstate_clock_offset`（戳级联 3→2，[min,max] 钳位，RMW+读回+回滚）+ `set_p0_reference_clock_isolated`（RTX50 隔离模板写）。
- **OCP**：sys `NV_GPU_CLIENT_POWER_CHANNELS_INFO_V4`（2672B，policyId/subtype/min/default/max，OCP 四元组常量）+ 0x10A4C 控制**双几何检测**（xOCD 紧凑 40B@28 vs 我们 46296 RE 的 136B@1756——同戳两代布局，实现期新发现）+ safe `power_channel_policies` / `power_channel_control` / `ocp_channels` / `set_power_channel_value`（mA 硬钳 1000..5001000）。
- `boost_lock_snapshot`（七域表解码 + 时钟范围锁/电压锁谓词）+ `vf_delta_scale`（arch∈[0x130,0x140) 且 range ±2MHz ⇒ ×2 判据）+ MSD/media subsystem 与 ThermChannel 索引语义注释。
- 探针：`tests/xocd_gap_probe_live.rs`（**GET-only**，`--ignored`）：E1（V/F 双几何对照）、E3（OCP 通道+几何检测）、E4（域槽位实况）、E5（TopRels 边判别+比率窗口扫描）、E6（七域表）；E2 复用既有 `tests/volt_rails_raw_dump.rs`。JSON 落 `reverse/xocd/` 供跨机 diff。

**实施期修正（agentA 误判）**：0x0733E009 V/F 曲线 SET **并非未封装**——`set_vfp_table`（src/gpu.rs:1393）早已走 `NvAPI_GPU_ClockClientClkVfPointsSetControl`（nvapioc 几何：条目基 40/delta@+20，R610.74 实测纯 kHz）。真正缺口只有：①xOCD 几何（基 100/delta@+24）与 nvapioc 几何的矛盾待 E1 裁决；②deltaScale 判据（已落 `vf_delta_scale`）。§3/§9 相应条目作废。

**剩余未做**：查漏清单 #8（NVML 增量：GetClockInfo 三域/Utilization/MemoryInfo/TemperatureV/ClockOffsets/Architecture，属主仓 core 层）；可借鉴设计（用户指示后续再做）；CLI/GUI 命令面暴露（另批）；写路径的实机验收（先跑对应 E 实验）。

## 15. 来源与置信度

- 反编译源：`C:/Users/YLW-XLAB/ida-scratch/xocd-decomp`（本机；复现见 §1，xocd-app.exe SHA256 锁定）。未混淆，全部结论有 `文件:行号` 引用，收录于 `reverse/xocd/agentA-control.md` 与 `agentB-telemetry.md`（untracked 台账）。
- 我方基线：`nvapi-rs/sys/src/nvid.rs`（注册比对 55/55）、`sys/src/gpu/{clock,power}.rs`、`src/gpu.rs`；关键「未封装」结论经主线程独立 grep 复核（0xD14B69CF/0xCBFF71D0/0xEF3D20EA/0x0733E009 在 src/ 0 引用；PowerPolicyId 仅 Default=0；TGP-watt 注释在 nvid.rs:1404-1466）。
- 置信度：通道定性、55 ID 对照、三开关=UI 门、OCP 通道映射=**实证**（反编译直读）；V/F 偏移矛盾、status 索引、域 20/21 命名、RTX50 Large 布局单位=**推断**（E-matrix 待裁）。
- 局限：xOCD 面向 RTX 30/40/50，P100 适用面见 agentB §10（电压/温度/公开频率/偏移/曲线可用；Blackwell 专属路径不适用）；未运行 xOCD、未做任何动态验证。

## 16. 首测判读（2026-10-05，RTX 2070 + RTX 3060 + Tesla P100 + RTX 4060 Laptop）

四台机器跑 `tests/xocd_gap_probe_live.rs`（GET-only 轮；e6/e1 的 JSON 写失败系测试 CWD=workspace 根，c3ead23 已修）。

### 16.1 E3 PowerChannels —— 大半定案
- info v4（264816）**四代全通**（Pascal/Turing/Ampere+Ada）。
- **policyId 0 = 板功率（raw mW）四代通用**，默认值与卡规格分毫不差：2070 (0,9)=175000/219000（175W 规格）、3060 (0,0)=170000/212000（170W）、P100 (0,0)=250000=250W（Tesla 锁定，min 125W）、4060L (0,0)=5000/100000/140000（100W 基础 +140W DB）。
- **OCP 电流（mA）身份按代际轮换**：50 系 (19,13)/(19,12)（xOCD 配对）；**Ampere 与 Ada 共用 (13,19)**（3060 def 135000/max 138068 ≈ +2.3% 余量；4060L def=max=135000 笔记本无余量）；**Turing (6,19)**（2070 def 215860/max 240000）；**Pascal 无电流 OCP 通道**（(6,1)/(3,7) 均为哨兵；剩余 (3,x)/(4,x) 电流通道 66/240/35/13A 身份待定）。5001000/1001000 = 「无上限」哨兵。
- **紧凑控制 0x10A4C 在非 50 系全部 -9 被拒** → ~~50 系以下 OCP 写需走 10016B（0x12720）布局~~ **已于 round-5 推翻：那是版本魔数笔误（0x0010_0A4C 写成 v16|2636；正确为 0x0001_0A4C = 68172 = v1|2636）。修正后 P100/582.41 实机 status=0 且紧凑几何满载返回（详见 §16.9）。** e3 的 0x12720 只读扫描保留为第二几何仪器。

### 16.2 E6 BoostLock —— 双向实证定案
- 2070 **电压锁**（用户施加）：id=6/mode=3/value=800000 µV ✓；2070 **频率范围锁**（-lgc）：**id=0 与 id=1 同时 mode=2**，value=1995000/1935000 kHz（上/下界）✓；P100/4060L 无锁态：七项全 mode=0 ✓。
- xOCD DecodeBoostLock 语义（id∈{0,1}+mode2=范围锁、id6+mode3=电压锁）与 `is_clock_range_lock`/`is_voltage_lock` 谓词**实证成立，不改**。

### 16.3 E5 TopRels —— 50 系专属确证
- info 双机 OK；gpc_xbar：2070=[]、3060/4060L=[0]（唯一边，门控可过）、P100=[]。
- control (0x1075c)：Pascal/Turing **-103**，Ampere/Ada **-1**（与 nvpwrcontrol 首测 4060L "GetControl Ada=-1" 一致）→ 只有 50 系可写；矛盾③（比率语义命名）只能等 50 系机器。

### 16.4 E4 记录类型 —— 驱动版本漂移发现
- 代际普查：Pascal {0x4,0x5}、Turing {0x8,0x9}、Disp 0x02 跨代、Ampere 3060 与 **Ada 4060L 当前驱动均报 0x10** —— 同一张 4060L 在旧驱动报 0x0A（0x10 = internal 0x0F 的 +1 协议重映射）→ **类型字节是驱动版本依赖的**，0x0F/0x10 应视为同一现代家族解读；xOCD 的 tag==15 分派键与我们的 `record_type_blackwell` 一致，无需改代码（注释已更新）。
- 2070 两次运行各见一处 slot0=-1000 kHz（首报 bit8/type2、次报 bit0/type8；输出交错位归属待复测），含义未定。
- 空闲态 freq/volt 值全 0 → 语义槽位图未被带值验证。

### 16.5 E1 V/F 几何 —— 静态线索不定案，e1b 待跑
- magic 0x22420 四代接受 ✓；+20 处 2070=31、3060=15、4060L=15、**P100=0**（非跨代稳定的点数）。
- P100 表非全零：部分 mask + 两处非零 dword（+68=1、+104=0x100000）——**都不落在两种候选几何的 delta 槽**（nvapioc 60+36i / xOCD 124+36(i-1)）→ 静态判别不成立。
- **裁决手段已交付**：`e1b_vf_points_write_read`（`#[ignore]` + `NVOC_ALLOW_VF_WRITE_PROBE=1` 双门控）：RMW 单 dword +15000 kHz → 双几何对照 → 恢复 → 逐字节校验。在任一卡上跑一轮即可定案矛盾①。

### 16.6 E-matrix 状态总览
| 实验 | 状态 |
|---|---|
| E2 轨 status 槽索引 | 未跑（`volt_rails_raw_dump`，4060L 待跑） |
| E3 OCP 语义/几何 | **定案**（单位/身份/哨兵四代实锤 + round-5 戳修正后 P100 实机控表满载返回、几何检测 Some(true)；写路径待 40 系/610 实机复验） |
| E4 域 20/21 命名 | 用户裁决结案（MSD=media subsystem）；类型普查归档 |
| E5 TopRels 边语义 | 确证仅 50 系可裁，搁置 |
| E6 BoostLock | **定案**（双向实证） |
| E1 V/F 几何+deltaScale | **定案**（真几何 88+36i 双机实证 + 点阵写入白名单/entry+0 只读双机闭环，见 §16.8） |

### 16.7 第二轮判读（同日，4060L + 2070，探针 c3ead23→b96a8be）

- **E1b 证伪两个候选几何**：SET 被接受（status=0）但 +15000 kHz 在 nvapioc 槽（4060L@564、2070@1140）与 xOCD 槽均不保留，恢复逐字节校验 OK ×2。结论：delta 字段不在 60+36i 也不在 124+36(i-1)——**矛盾①升级为「字段位置未知」**，clock.rs:384 的 nvapioc「R610.74 live round-trip」记载与本次读回矛盾，待 e1c 定位后回改。
- **e1c 已交付（字段发现扫描）**：扫 [40,204] 每 4 字节——仅原值为 0 的偏移补丁 15000 → SET → GET → **全表 diff**（若值被驱动搬到别的槽，diff 直接暴露真字段）→ 每步恢复+校验，失配即中止。双门控同 e1b。
- **E3 round-2 扫描作废**：两机 0x12720 GET 返回全零载荷（命中 [0] 均为版本魔数 75552 的宽区间假阳性）。根因：探针未做 set_tgp_watt 的「私有 GetInfo 预热 + dword1 mask 播种」。round-3 已修（预热+播种 0x7FFF、跳过头部、精确默认值/严格区间内匹配、载荷非零 dword 计数）。**round-5 再修正**：种子必须恰好是 info 返回的 mask（见 §16.9）——0x7FFF 超集在 P100 上被 -1 拒收，0x12720 的 -1 即此。**round-5 复跑：同机 0x12720 仍 -1（其种子固定 0x7FFF）；修正播种后 0x10A4C v1 即 status=0——第 2 轮的'全零载荷'根因是种子而非布局。**
- **E2 首批佐证**：2070 VoltRailsStatus(v1|2760) +72=712500/+76=1125000 µV **= UI 截图 NVVDD range 712–1125**；4060L +80=1200000（1.2V VRM/OV 墙，与 xOCD status.values[3] 语义吻合）；2070 control +72/+76=32000（32mV 标定窗，xOCD ctrl.values[0/1] 呼应）。两卡均单轨 → **bit 序 vs dense 序仍需 50 系双轨卡**。
- 2070 E4 稳定复现：bit0/type0x8/slot0 = **-1000 kHz**（非输出交错，含义未定，与 -lgc 范围锁是否相关待查）。
- e3 探针的未使用变量警告已修；JSON 路径修复生效（本轮 e1/e6/e3 JSON 均落盘成功）。

### 16.8 E1 终局结案（2026-10-05 深夜，offset_of 实证 + 2070 实测写-读差量）

- **根因=探针手算偏移错误，不是结构体/驱动问题**：`NV_GPU_CLOCK_CLIENT_CLK_VF_POINTS_CONTROL_V1` 头部实为 version(4)+ClockMask<8>(32)+unknown(32)=68B，points@68、entry 36B、freqDeltaKHz@entry+20 ⇒ **delta(i)=88+36i**（size_of=9248 的唯一解）。e1/e1b 的「nvapioc 60+36i」是 40B 头假设的产物；xOCD 的 124+36(i-1) 对 i≥1 与其为同一族（off-by-one 贴错标签）。
- **原始证据重解释**：e1c 保留位 {88,124,160,196} 即真槽位；68/104/140/176 被拒 -1 = 那正是各 entry 的 clock_type 字段（驱动枚举校验拒绝 15000）；e1b 的 60+36×30 落在 entry29 尾 padding（驱动重建置零，故"不保留"）。§16.7 的「字段位置未知」结论作废。
- **e7 探针（v0.2.x@ec0e654→7252a85）实证**：
  - 2070/610.47（用户 OC +50×0..70 生效态）：A2 普查 `50000: n=71 [88+36k k=0..70]`；A4 ours 列 pt0..7=50000、old60 列=0；**B1 `set_vfp_table(p5,+80MHz)` 差量恰 1 dword：abs 268(=88+36×5) 50000→80000**；B2 raw 补丁 268=55555 保留；两臂恢复逐字节校验 OK。
  - **4060L/610 跨机复现**：A2 同为 `50000: n=71 [88+36k k=0..70]`，B1/B2 与 2070 逐字相同（差值、保留、恢复全 OK）。
  - P100（本机，只读）：status 0、点阵 80 位、type=1@68+36k k=0..79、delta 全 0（写臂需提权，-137 优雅跳过）。
- **旧证据再更正**：§16.5 的「+20 处 2070=31/3060=15/4060L=15/P100=0 非跨代稳定点数」= 点阵 dword[4]（位 128..），实即各卡点表长度尾部：2070=133 位、4060L=132 位、P100=80 位；并无独立 count 字段。
- **B3/B4 双机裁决（v0.2.x@7252a85，4060L+2070 提权实跑，两机输出逐字一致）**：
  - **② 结案：点阵位就是写入白名单**。表外点 140 的 raw delta 槽（两机均 abs 5128）补丁 60001 → SET **被接受（status 0）但驱动重建表时该槽归零**（retention 0、全表 diff 0 dword）→ 表外 delta 被静默丢弃、不被消费；走 API 路径（`set_vfp_table` 顺手置 bit140）则整个 SET 被拒 **-1**。实践含义：写点索引必须落在 GET 点阵位数内（2070=133、4060L=132），越界 -1 是 fail-closed，可接受、无需改码。
  - **④ entry+0 = `clock_type`（VfPointType）字段，驱动逐点声明、只读** —— 用户判读定案：值 1 = `NV_GPU_CLOCK_CLIENT_CLK_VF_POINT_TYPE_FIXED`（enum Prog=0/Fixed=1/Dyn=2，sys/gpu/clock.rs:480-485；即 hi 层 `VfpPoint.point_type`/`is_editable()` 与 core「Pascal 公共表全 Fixed 只读」注释所依据的同一字段）。分布自洽：2070/4060L 的 Fixed 尾段（128..130 / 127..130）即 memory 段锚点（CLI 文档 "trailing memory entries 127..131"），用户 OC 差量只落 Prog 区 0..70；P100 全表=1（整表 Fixed=公共只读，与 core/src/nvapi.rs Pascal 分支一致；e7 本地烟测 B5 在 P100 上 pick p0 正因此）。B4 事实：entry10（当前 0）试写 {1,2,8,9,255} → **双机五值全 -1**（连表内他处合法的 1 也被拒）；结合 e1c 与 P100 非提权跑（v=1=原值只过值校验、只撞 -137）→ **点类型由驱动声明，SET 只接受原值回显**。我们的写路径 RMW 从不改动它，天然安全。
  - **B5 裁决（4060L 实跑）**：p127（in-bitmap、clock_type=1 Fixed）raw 补 33333 → SET 0 接受但 retention 0、全表 diff 空 → **Fixed 点不消费 delta**（与表外点同态）。结合 B1/B2（Prog 点保留）与类型门既有经验：**可消费 delta 的面 = 点阵内 ∩ Prog 类型点**。B6（类型门 vs 域范围门判别臂）未跑即删——用户裁决按类型门结案。写侧含义：set-public-vftable 对 non-Prog 点写入会被驱动静默归零（回读即见 0），CLI 可考虑依 `is_editable()` 提示；我们的写入本身安全（RMW+点阵不动）。
  - **探针清理（同日）**：e1/e1b/e1c（错误 40B 头偏置算术的遗留档案）已删除；e7 为唯一 V/F 探针（v0.2.x@后随提交）；e1c 的"保留位 {88,124,160,196}"证据已由 §16.8 吸收。
  - ③ rsvd[4]/unknown[8]/padding[3]：仍全零、从未被带值触发；继续保持"勿动"（RMW 保原值）即可。**公共 VF 表至此全字段身份定案：可写面=delta(88+36i)+点阵位数（≤GET 值），其余全部驱动所有只读。**
- **结论**：写路径（`set_vfp_table` 及 CLI 公开 VF 写）与 struct 几何自始正确；`get-public-vftable` 的 delta 列虽来自 curve/status 面（current−默认），但其与 raw 表 88+36i 一致。**矛盾①⑥结案**；clock.rs 与探针注释中的 60+36i 算术为化石错误（e7 模块注释已留勘误）。
- 余留观察（不影响结论）：clock_type=Fixed 仅落尾段（2070 128..130、4060L 127..130，即 memory 段锚点），P100 全表 Fixed——分布语义已对齐现有 VfPointType 解析（get-public-vftable 本就输出 point_type 列），无需改码；仅剩"Fixed 点是否消费 delta"由 B5 收尾。

### 16.9 OCP 控制 -9 结案（2026-10-05，用户报告的 40 系/610 `set-ocp-limit` 失败根因）

用户实测 `nvoc-cli set-ocp-limit nvvdd 1000` 在 40 系/610 驱动报 `NvAPI_GPU_ClientTgpWattGetStatus failed: NVAPI_INCOMPATIBLE_STRUCT_VERSION (-9)`。根因**不是代际门，是版本魔数笔误**：

- **戳笔误（根因）**：生产码与探针把控制 GET/SET 的魔数写成字面量 `0x0010_0A4C`（v16|2636）。xOCD `ReferenceOcp.Layout Small.Header` = **68172 = 0x00010A4C = v1|2636**（五位数写法 "0x10A4C" 被误读成 0x00100A4C）。任何版本 nibble ≠ 驱动表项都在用户态 nvapi64 版本门即 -9——**与代际无关**；此前"pre-50 全 -9"的观测实为该笔误的产物（没有任何一台机器、包括 50 系，成功调用过）。修正后 P100/582.41 实机 `status=0`（下方证据），v16 字面量行同机仍 -9 作对照。
- **同批修正的三处几何/契约缺陷**：
  1. 紧凑 value 偏移：sys 原 `ENTRY_BASE 28 + VALUE_OFF 32` 重复计了 entry 基址 ⇒ buffer 60+40i 错误；xOCD `PowerChannelControlValueOffset = index*40+32`（NvApiSource.cs:2007/2036，`ReferenceOcp` ValueDelta=4）⇒ **buffer 32+40i**。审计 §5 行 71 的 "value@i*40+32" 本为正确，代码转写时引入。
  2. 紧凑条目数 6→**15**（0x7FFF 掩码契约覆盖 bit 0..14，xOCD `WriteMask`/`VerifyValues` 逐位迭代）。
  3. **GET 播种**：驱动只填 mask 位对应条目，且**播种超集（0x7FFF）被 -1 拒收**——必须恰好用 info 返回的 mask。P100 实机对照：同一调用 shape，seed 0x7FFF ⇒ -1，seed 0x449f（=info mask & 0x7FFF）⇒ 0。xOCD 每次控制 GET/SET 前都先 info GET 再以其 mask 播种，此步曾被我们省略。
  4. 几何检测/取值改为**按 mask 位索引**（可稀疏，P100 mask=0xc49f 有 gap）而非位置序。
- **P100/582.41 实机证据（e3 探针修正后）**：`control 0x10A4C v1|2636 (seed 0x449f): status=0`；mask 回显 0x449f；紧凑 values 按 info 位读回 = info 默认值（5001000/1001000/250000/66000/240000/35000/13000/5001000）；r465 136B 区全 0；**几何检测 Some(true)（xOCD 紧凑）**；戳矩阵全部 v1 行（0x10298/0x106DC/0x10A4C/0x11F10/0x12720）status=0、17 个非零 dword（同一对象别名），唯 v16 行 -9——版本门隔离性成立。
- **代码状态**：nvapi-rs v0.2.x@55aafb5 直提（sys `STAMP` 常量 + 偏移/条目数修正 + 索引化检测器；safe 层快照/写/读回/回滚全链按 xOCD 非 50 `SetPowerChannelLimit` 配方：mask 播种 GET → 几何检测 → 补丁 → **写 mask=1<<index** → SET → 1<<index 播种读回 → 失配回滚）；探针 e3 更新（v16 对照行 + info-mask 播种）；CLI help/core 注释/本节文本同步。
- **残留验证**：写路径的实机闭环（SET 接受 + 保留 + 回滚）在 40 系/610 上复跑 `set-ocp-limit` 即决；P100 本机无提权，写臂 -137 优雅跳过。0x12720（10016B）备用布局的只读扫描保留，但按本结论它已非 pre-50 写路径的必选项。

## 17. nvoc-core / cli 封装建议清单（2026-10-05，逆向与测试暂停点）

### 17.1 手头结果汇总（截至本节）

已落地（nvapi-rs v0.2.x@b96a8be，全部带 RMW+读回+回滚配方）：
ClkDomains 类型化 freq/volt-demand 写（0xD14B69CF，代际槽位分派）、TopRels 比率读/写（0xCBFF71D0/0xEF3D20EA，唯一关系门控）、SetPstates20 偏移 RMW + RTX50 隔离 P0 模板、OCP PowerChannels 读/写（info v4 + 0x10A4C 双几何检测）、BoostLock 七域快照与谓词、vf_delta_scale 判据、GET-only 探针族 e1/e1b/e1c/e3/e4/e5/e6。

四代实证（2070 Turing / 3060 Ampere / P100 Pascal / 4060L Ada）：
policyId 0 = 板功率 mW（默认值=卡规格，四代通用）；OCP 电流 mA 身份代际轮换（50 系 (19,13)(19,12)、Ampere+Ada (13,19)、Turing (6,19)、Pascal 无）；5001000/1001000 哨兵=无上限；BoostLock 语义双向实证（电压锁 id6/mode3 µV、范围锁 id0+id1 mode2 kHz 界、无锁全 mode0）；VoltRailsStatus µV/墙序模型佐证（712–1125 UI 对齐、1.2V VRM 墙）；TopRels 控制仅 50 系（Pascal/Turing -103、Ampere/Ada -1）；ClkDomains 记录类型字节随驱动版本漂移（Ada 0x0A→0x10）。

未定案（探针就绪，按用户指示暂停）：E2 双轨槽位序（需 50 系）；TopRels 比率语义（需 50 系）。已随 §16.9 出局：pre-50 OCP 写偏移（0x12720 扫描转为备用布局仪器，戳修正后紧凑几何四代即通）。V/F 表已从列表移出——全字段定案（真几何 88+36i、点阵=写入白名单、entry+0 只读，B3/B4 双机裁决见 §16.8）。

### 17.2 封装建议（按优先级）

P0 —— 读侧、证据实锤、零写风险，可直接实施：

| # | 交付 | nvoc-core | cli | 价值/证据 |
|---|---|---|---|---|
| 1 | 功率通道表读取 | QueryNvapiPowerChannels（operation.rs 新 Kind，包 power_channel_policies + ocp_channels + power_channel_control） | get-power-channels | OCP/板功率/电流通道/哨兵一览，四代可用；渲染 A 与 mA 双列；is_board_power/is_ocp_current 已备 |
| 2 | BoostLock 快照 | QueryBoostLocks（包 boost_lock_snapshot） | get-boost-locks | 「频率为何被钳」排查 + 一切 V/F/电压写操作的前置安全检查（见 #7）；双向实证 |
| 3 | 精细温度通道暴露 | 无需新 Kind（thermal_channel_info 已封装） | get-thermal-channels | hotspot/显存结温（索引语义 0/1/2(50系)/7(40系)/9(其它) 已注释）；xOCD 遥测面板同款数据 |

P1 —— 写侧、nvapi-rs 原语就绪且配方内置，需 CLI 风险标注（部分有前置依赖）：

| # | 交付 | nvoc-core | cli | 依赖/风险 |
|---|---|---|---|---|
| 4 | OCP 限值写 | SetNvapiPowerChannelValue | set-ocp-limit（CLI 文档的 `--confirm-disable-protection` 二次确认为设计建议） | 全代际同一紧凑几何（戳 v1\|2636 + info-mask 播种，xOCD 非 50 配方；原 pre-50 -9 系戳笔误，§16.9）；高危（解除保护），恢复路径=写回 default |
| 5 | 域电压 demand 写 | SetNvapiDomainVoltageDemand | set-domain-voltage --domain xbar/sys/video --uv | set_clk_domain_voltage_demand 就绪（±500mV 钳 + RMW）；4060L 即可验证；中危 |
| 6 | 域时钟类型化偏移 | SetNvapiDomainClockOffsetTyped（或给既有 set-private-freq-domain-global-offset 加 --auto-slot） | 同左 | set_clk_domain_freq_offset 就绪（记录类型分派槽位）；新驱动卡（0x10 记录）需要；中危 |
| 7 | 写前外部锁检查（pre-flight） | CheckExternalBoostLocks：V/F/电压类写操作执行前查 boost_lock_snapshot，范围锁激活则拒绝电压锁（xOCD 安全语义） | 不单独暴露，作为写操作的内置 gate + --force 旁路 | 纯读检查；把 2070 实证的保护语义变成产品行为 |

P1.5 —— 读侧增强（独立可做）：

| # | 交付 | 层 | 说明 |
|---|---|---|---|
| 8 | NVML 增量 | core/src/nvml.rs | GetClockInfo 三域 / Utilization / MemoryInfo / TemperatureV(hotspot) / ClockOffsets 探测 / GetArchitecture——xOCD 当主力遥测源的整套；补齐后 get-info 可跨驱动兜底 |

P2 —— 依赖未定案或无实机，暂缓：

| # | 交付 | 前置 |
|---|---|---|
| 9 | TopRels 比率写 CLI（set-top-rels-ratio） | 仅 50 系可验证 |
| 10 | RTX50 隔离 P0 写 CLI（set-p0-reference-clock） | 仅 50 系 |
| 11 | V/F 公共表写修正（set_vfp_table 偏移/文档回改 + deltaScale 接入 CLI 曲线写） | 等 e1c 定位真字段 |
| 12 | 板功率 mW 直写（set-power-channel-value 走 (0,subtype) 通道） | ~~与既有 set-power-limit 重叠，价值存疑~~ 已实施为 set-ocp-limit 数值 index（全表入口），与 set-power-limit 同源不同门，见 §18 |

P3 —— 设计借鉴（用户已裁决后续再做）：Profile 体系、Apply 级联回滚栈+undo、双速遥测、opt-in 安全门控、self-test 命令档、基准 worker 协议。

### 17.2.1 实施状态（2026-10-05 深夜，用户勾选后落地）

已实施（用户裁决：P0 全部 + P1 仅 #4 不加保护 + TopRels 比率写 CLI；#5/#6 缓做、#7 不采纳）：
- nvoc-core：OperationKind 新增 6 变体（QueryNvapiPowerChannels/QueryNvapiBoostLocks/QueryNvapiThermalChannels/SetNvapiPowerChannelValue/QueryNvapiTopRelsRatio/SetNvapiTopRelsRatio，写侧入 is_nvapi_write GC6 预热门）+ 6 个 GpuOperation（读侧包装 None-降级，OCP 写直通 nvapi-rs 内建钳位/RMW/回滚）。
- cli：6 命令落地——get-power-channels（mA/A 双列 + 哨兵标注 + 几何检测）、get-boost-locks（含谓词与 hint 行）、get-thermal-channels（primary 类型表 + 逐通道实测 °C）、set-ocp-limit <nvvdd|msvdd|INDEX> <A|ma|W|mw>（按用户要求无确认门；代际解析 + 驱动窗钳位，xOCD 紧凑几何全代际通用，§16.9；任意 index 与 TGP 同源判定见 §18）、get-top-rels-ratio、set-top-rels-ratio <0.7-1.2>（0.9=0xE660 字面量）。
- nvapi-rs hi 层：7 个透传（power_channel_policies/ocp_channels/power_channel_control/boost_lock_snapshot/top_rels_ratio + 2 写）。
- 门禁：fmt/clippy 全绿，cli 68 / core 71 / nvapi 全绿（含 specs 排序与穷举守卫）。

### 17.3 实施注意

- core 每条写操作走既有 OperationKind + is_nvapi_write() GC6 预热门；OCP 写额外要求二次确认语义（xOCD RiskAcknowledgement 的最小版）。
- cli 新命令入 Groups（Power/Clock/Voltage/Thermal）+ output.rs 渲染管线约定（BTreeMap 序、mA/A 双列、uV 命名 allow 惯例）。
- ~~pre-50 系 OCP 写在 E3-r3 定位前保持 NotSupported 拒绝~~（已随 §16.9 撤销：戳笔误修正后全代际同走紧凑几何；写侧失败仍由 nvapi-rs 内建几何检测/RMW 回滚兜底，core/cli 无需额外判断）。
- 全部 P0 读命令在 P100 上即可验收（info v4/BoostLock/ThermChannel 四代可用）。

## 18. TGP-watt 与 OCP 通道的同源判定（2026-10-05 用户提问）+ 任意 index 写入落地

### 18.1 结论：`set-power-limit`(--nvapi) 与 `set-ocp-limit` 是**同一个底层控制对象**

用户观察"这里的 id 也包含了功耗墙的设置"成立，且不止是"包含"——是同一个 GET/SET 函数对、同一个 RMW 缓冲、同一张条目表：

- **同一函数对**：两者最终都打 `0x8B3E7343`（ClientTgpWattGetStatus / 别名 PowerPolicyGetControl）与 `0xAFFC2279`（ClientTgpWattSetStatus / PowerPolicySetControl）。`set-power-limit --nvapi`（SetNvapiTgpWatt→set_tgp_watt）写"目标 mW 在 buf+0x8A0+40*index"的 **v1|10016 (0x12720)** 全表视图（ref tool 配方；538+ 驱动才有此戳）；`set-ocp-limit` 写 **v1|2636 (0x10A4C)** 紧凑视图（xOCD 配方；`COMPACT_ENTRY_BASE 28/STRIDE 40/VALUE_OFF 4`）。两者是同 handler 版本 switch 的不同表项——R465 IDA 实证接受 `{0x10298, 0x106DC, 0x10A4C, 0x11F10}`（§16.9，另 0x12720 为 538+），同一对象的多版本视图。
- **同一信息源**：`0x67F31384`（ClientPowerPoliciesGetInfoPrivate）既是 TGP 范围（348KB 私有结构，`policy_index` 默认 2）又是通道表（v4 6312B 通道视图）的唯一来源；两条 CLI 命令的"policy index"来自同一个表的同一索引空间。
- **公开/NVML 面同条目**：ClientPowerPoliciesGetInfo/GetStatus/SetStatus（0x34206D86/0x70916171/0xAD95F5ED）与 NVML 功耗上限是该**板功率政策条目的公开门**。
- **P100/582.41 实机数值三点一致（毫瓦级）**：NVML `get-public-power-limit` 当前/最小/最大 = **250/125/250 W**；私有 TGP 范围 = **250/125/250 W @policy_index 2**；通道表 index 2 = policyId 0 板功率 **250000/125000/250000 mW**，控制缓冲当前值 250000。三面同值即同对象。
- **对写路径的硬约束（两条路径均已是 mask-scoped RMW）**：TGP 与 OCP 写共用一张表，任何写入必须 `mask=1<<index` 只动自己那行——我们的 OCP 全链如此（§16.9 配方），现有 `set_power_mw` 亦 OR 同一位。否则 TGP 写会顺手把 OCP 行清成零。
- **未实测**：公开/NVML 写的实机闭环（本机非提权，`nvmlDeviceSetPowerManagementLimit` 返回 NoPermission）；读侧三面一致性已闭环。可裁决点：下次提权时 `set-power-limit --nvml <x>` 后回读通道表 index 2 的 current_raw 应随动——即公开写落在同一控制缓冲。

### 18.2 `set-ocp-limit` 任意 index 写入（本次落地）

- 位置参数 `<nvvdd|msvdd|INDEX>`：INDEX = `get-power-channels` 打印的条目 index（= info mask 位 = 控制条目 = 写掩码位，四者在四代机器上与 xOCD 驱动一致）。单位随行解析：OCP 电流条目 A/`ma`、板功率条目 W/`mw`，`ma`/`mw` 后缀（大小写不敏感）= 原始驱动整数。越界 index 报错并列出可用集合。
- 驱动 `[min,max]` 窗口钳位在 nvapi-rs 内完成（OCP 电流另有 1000..=5001000 硬包络）；当实写值 ≠ 请求值时输出新增 `requested_mA/mW` + `clamped_to_driver_window`，用户可见（例：40 系 nvvdd 求 140 A、窗口 135 A 上限 → applied 135000）。
- `get-power-channels` 每行新增 `current_raw`（与 index 配对的活体控制值）；P100 实机 = 各行默认值（股票卡 250000 等，8 行逐项吻合）。
- 渲染修正（用户报告"Raw MA"）：`format_label` 补 `mA/mW` SI 大小写映射与 `OCP/NVVDD/MSVDD/TGP` 缩写词表；`parse_power_channel_value` 后缀大小写不敏感。cli 72 测试绿（新增 value 语法与渲染回归）。
- 关联：§17.2 P2 #12"板功率 mW 直写，价值存疑"**解除存疑**——与 set-power-limit 同源但入口不同（同表任意行 vs TGP 专用视图），保留两条入口（前者=全表/诊断，后者=常规 TGP 操作）。

### 18.3 两视图几何对齐（2026-10-05 用户要求"v1 全表应与紧凑表对齐"）——已实证

用户要求先做视图对齐：理论上 0x12720 全表（v1|10016，`set-power-limit` 用）与 0x10A4C 紧凑表（v1|2636，`set-ocp-limit` 用）应逐条对齐。**结论：两视图是同一张表的两个基址，逐条完全对齐，已由活体实证。**

- **几何（代码已发布并对齐）**：`NV_GPU_CLIENT_TGP_WATT_STATUS_V1` 现公开 `ENTRY_BASE=0x8A0`/`ENTRY_STRIDE=40`/`ENTRY_VALUE_OFF=4`；紧凑视图 `NV_GPU_CLIENT_TGP_WATT_STATUS_10A4C_V1` 的 `COMPACT_STRIDE=40`/`COMPACT_VALUE_OFF=4`/`COMPACT_ENTRY_BASE=28`。**两视图共享 stride 与 value 偏移（entry+4），唯一差异是表基址**（0x8A0 vs 28）；行索引 = info 掩码位 = 控制条目 = 写掩码位，写契约 `1<<index` 两视图一致。sys 单测 `tgp_full_and_compact_views_align` 断言该几何恒等式（stride/value 相等、基址不等、buffer dword = base+value_off+40i），已绿。
- **活体实证（P100/582.41，探针 e8）**：同一 info 掩码播种（`seed=0x449f`）下，两戳 GET 均 `status=0`；8 个已填充条目（bit 0/1/2/3/4/7/10/14，含板功率 bit2 id(0,0)、Turing OCP 家族的 (6,1)/(3,x) 等）**compact 值 == full 值 == info 默认值，逐条 ALIGNED，0 MISMATCH**。快照 `reverse/xocd/e8-tgp-alignment.json`；探针 `e8_tgp_full_compact_alignment`（GET-only）。
- **对写路径的含义**：`set-power-limit`(--nvapi) 现已落到**共享的紧凑核心**（见 §18.4），0x12720 视图在代码中统一标记为 V2（新宽表视图，538+ 才有此戳）；紧凑 0x10A4C 是**兼容面更宽**的同一对象视图（R465 起即接受）。

### 18.4 TGP 写路径落到共享紧凑核心；0x12720 全表标记为 V2（2026-10-05 用户指示）

用户指示"TGP 写落到这个共享紧凑核心，原来的 0x12720 全部标记为 V2"。落地：

- **单一共享核心**：`PhysicalGpu::set_channel_raw_core(all, index, value_raw)`（`vapi-rs/src/gpu.rs`）承载 §16.9 的完整紧凑 RMW 配方——info 播种（stamp `0x0001_0A4C`）→ 几何检测 → 按 `[min,max]` 钳位（`value_raw==u32::MAX` 为"复位/不钳"哨兵）→ 写掩码恰 `1<<index` → SET `0xAFFC2279` → 掩码播种 `1<<index` 读回 → 不符则 SET 回滚并报 `ArgumentRange`。`set_power_channel_value`（OCP 任意 index）与 `set_tgp_watt`/`reset_tgp_watt` **全部委托**同一核心，写契约只有一处实现。
- **类型统一**：`set_tgp_watt`/`reset_tgp_watt` 返回类型由 `crate::NvapiResult` 改为 `crate::Result`，与其余写面一致（`hi/gpu.rs` 包装去掉冗余 `map_err`；core `SetNvapiTgpWatt` 文档同步）。
- **命名标记**：sys `NV_GPU_CLIENT_TGP_WATT_STATUS_V1` → `NV_GPU_CLIENT_TGP_WATT_STATUS_V2`（含 nvversion 别名重定向），文档注明"V2 = 538+ 新宽表视图（0x12720），不是版本 nibble；是紧凑核心的第二视图"。所有 0x12720 相关标识/注释统一以 V2 命名。
- **不做**：不改写入语义（仍 `1<<index` 掩码 scoped）、不改几何（§18.3 已证对齐）；`get-power-channels`/`set-ocp-limit`/`set-power-limit` 全部走同一核心的收益=pre-538 驱动也归一到兼容面更宽的 0x10A4C 戳。

### 18.5 功率表 index 上限核查：不是 15，已放宽到 32（2026-10-05 用户提问"确定只有上限 15 个吗"）

用户问功率表索引是否真的封顶 15，是否应再往上探测。**答案：不是 15；代码原先的 `0..15` 是错的（丢真实条目），已全部放宽到 32，并活体复探。**

- **掩码是 32 位**，表物理可容 32 条（紧凑表 entry 31 的值落在缓冲字节 1272，远在 2636B 结构内）。xOCD 的 `WriteMask`/`VerifyValues` 只迭代 `0..=14`（0x7FFF），但那只是它的自限，不是驱动契约。
- **P100/582.41 活体（探针 e3，放宽后重跑）**：info 掩码 = `0xc49f`，**bit 15 已置位**——旧 `0..15` 循环静默丢掉了它。全掩码播种紧凑 GET（`status=0`）返回条目位 0/1/2/3/4/7/10/14/**15**；其中 bit 14、15 均为 `id=(6,1) def=max=5001000` 的"无界"哨兵条目。稠密 0..31 扫描：未置位 bit 返回 0（驱动只填掩码内条目），**16..31 无任何条目填充**，与掩码 `0xc49f` 完全自洽。
- **对齐确认**：放宽后 e8 复跑为 **9/9 ALIGNED**（含 bit 15），0 MISMATCH——即新恢复的 bit 15 在两视图上同样对齐（快照 `reverse/xocd/e8-tgp-alignment.json`）。
- **代码变更**：`power_channel_policies`（`src/gpu.rs`）、`find_channel`（`sys/src/gpu/power.rs`）循环 `0..15`→`0..32`；`COMPACT_ENTRIES` 15→32；`NV_GPU_CLIENT_POWER_CHANNELS_INFO_V4.mask` 文档改为"扫描全部 32 位"；探针 e3/e8 的 `0..15` 与 `info_mask & 0x7FFF` 播种全部改为全掩码 `0..32`/`info_mask`。**结论：本机会话驱动（P100）16..31 无填充，但封顶 15 确实会丢 bit 15，放宽正确且安全。**

### 18.6 ExtendedLimits 面的活体测试流程（2026-10-05 用户提问）

用户问 xOCD 2.0 ExtendedLimits 面（§4 的 `power_graph_roles`/`power_command`/`power_control_input`）有没有测试流程。**此前只有离线字节回放单测（`xocd2_extended_limits_tests`、`xocd2_power_graph_decode_tests`），无活体探针。** 本次把 GET-only `e9_extended_limits_surface` 扩为**全函数覆盖探针**（`tests/xocd_gap_probe_live.rs`）：`power_graph_roles`（0x2BA030，-9 时按 xOCD 方式合成 347124B 布局）+ `power_command` **ch 0..31 × cmd {0xF8,0xFE} 双向扫描** + 由 graph role 派生的 `power_control_input` + **`set_power_command` 双门控自恢复写臂**（identity→扰动→恢复，恢复失败响亮报错；只写 0xF8，哨兵 baseline 跳过）。完整分级验收流程（L0 离线 / L1-L2 只读 / L3 写入 / L4 封装裁决门）独立成文：`docs/reverse-engineering/nvapi/xocd-extendedlimits-test-plan.md`。

- **实机判读（P100/582.41 + 4060L）**：两台 `power_graph_roles` 均 `Err(ArgumentRange)`，诊断行显示**两台 Modern 0x2BA030 都被 -9**、fallback 0xF4BF4 成功但角色解码失败——即 pre-50（含 Ada 笔记本）都不走 Modern 图，角色/输入面需接受 Modern 图的机器（Blackwell 50 系优先）。`power_command` **两台均仅 ch0 有数据**：0xF8=`0xFFFFFFFF`(unset 哨兵)，**0xFE=板功率 mW**（P100=250000、4060L=100000，与 NVML/通道表三面一致）——这是本面唯一跨代有真实数据的通道。`power_control_input` 因无 graph/shared role 跳过。**写臂 4060L 提权实测 PASS**（ch0/0xFE identity SET 接受+读回、扰动 100001 接受+读回、恢复 baseline=100000 成功：写通路成立、无粒度钳位/静默丢弃）；P100 非提权 SET=-137。探针已扩展为全函数覆盖 + Modern/fallback 原始 status 诊断行 + 0xF8→0xFE 回退自恢复写臂；分级验收台账见 `docs/reverse-engineering/nvapi/xocd-extendedlimits-test-plan.md` §3b。**结论：`power_command` ch0/0xFE（板功率 mW）读写均实证可用，是唯一可封装候选；graph/输入面 pre-50 与 4060L 均不可用。** 快照 `reverse/xocd/e9-extended-limits.json`。

### 18.7 CLI 出口合并：`set-power-limit` + `set-ocp-limit` → `set-pwr-cur-limit`（2026-10-05 用户指示）

§18.1/§18.3 已证两条写命令是同一个底层控制对象（同一 GET/SET 函数对、同一张表、写入契约同为 `1<<index` 掩码 scoped、几何共享），§18.4 已把两条写路径归一到同一个 `set_channel_raw_core`。用户据此指示"确认是同表就可以把 set-tgp-limit 淘汰掉，和 set-ocp-limit 合并封装到 set-pwr-cur-limit 命令"。落地：

- **单一命令**：`set-pwr-cur-limit <TARGET> <VALUE> [--policy-index N]`（power 族，双后端 BOTH_BACKENDS，arity 2）。TARGET 语义：`tgp`/`board` = 板功率/TGP 政策行（`--policy-index` 与 `--nvml` 仅在此生效；auto 优先 NVAPI 紧凑核心，`--nvml` 走 nvidia-smi `-pl` 路径即 `SetNvmlPowerLimit`），`nvvdd`/`msvdd` = OCP 电流通道（仅 NVAPI），或 `get-power-channels` 打印的任意数值 index（值单位随后行解析：电流行 A/`ma`，板功率行 W/`mw`；`ma`/`mw` 后缀 = 原始驱动整数）。渲染键由旧 `rail` 改为 `target`。
- **核心 op 不动**：`SetNvapiTgpWatt`/`SetNvmlPowerLimit`/`SetNvapiPowerChannelValue` 三个 core op 全部保留（仅 CLI 命令变体合并）；TGP 分支委托 `set_tgp_watt`（已落 §18.4 共享核心），OCP/任意 index 分支委托 `set_power_channel_value`。
- **旧名删除**（非别名）：`set-power-limit` 与 `set-ocp-limit` 两个 CLI 名字直接移除，clap 解析旧名即报错。决策记录在 `cli/RENAME_DECISIONS.md`。
- **测试**：新增 `set_pwr_cur_limit_parses_all_targets`（tgp/board/nvvdd/msvdd/数值 index、`--policy-index`/`--nvml` 解析、旧名报错）；`commands_listed_in_lexicographic_order` 门禁通过（spec 置于 `set-public-vftable-range-offset` 与 `set-temp-limit` 之间）；cli 73 测试绿。
- **风险面不变**：抬升 OCP 上限仍是解除一项安全网（无确认门），`--nvml` 主功率墙仍为常规操作。本机 P100 非提权，写臂照旧优雅跳过（NVML NoPermission / NVAPI -137）。

### 18.8 封装：`get-power-command` / `set-power-command` + `get-pwr-cur-info` 改名（2026-10-05 用户指示）

用户问"power_command 这条写入是之前没有的吗？和 set-pwr-cur-limit 不是同一个？"——**是两条完全不同的 NVAPI 族**：

| | `set-pwr-cur-limit`（§18.7 合并体） | `power_command`（本次新封装） |
|---|---|---|
| GET | `0x8B3E7343`（ClientTgpWattGetStatus / PowerPolicyGetControl） | **`0x33AB0353`**（NvAPI_GPU_ClientPwrPoliciesGetControl） |
| SET | `0xAFFC2279`（ClientTgpWattSetStatus / PowerPolicySetControl） | **`0x17695269`**（NvAPI_GPU_ClientPwrPoliciesSetControl） |
| 缓冲戳 | v1\|2636（0x10A4C 紧凑）/ v1\|10016（0x12720 全表 V2） | **v1\|1320（0x0001_0528，1320B 租约包）** |
| 寻址 | 32 位掩码下的通道 index（=info mask 位） | (channel 0..31, command 0xF8\|0xFE) 40B 槽 @40*(ch+1) |
| 语义 | 驱动 [min,max] 窗钳的通道限值（mA/mW） | 内核功率上限请求租约：0xFE=可写 request/lease（活体单位 mW，=板功率），0xF8=observed-only 读回（活体哨兵 0xFFFFFFFF） |
| 写入契约 | mask-scoped RMW+读回+回滚 | 值拒 0/0xFFFFFFFF 哨兵，写后读回验证，**无隐式恢复**（调用方自持基线） |

power_command 的 FFI 与 hi 封装（`power_command`/`set_power_command`）在本车道早已落库（§18.6 E9 探针的底层），此前缺的是 core/CLI 出口。本次按用户指示补齐：

- **core**：`QueryNvapiPowerCommand { channel, command }` / `SetNvapiPowerCommand { channel, command, value }` 两个 GpuOperation + OperationKind 变体（写侧进 `is_nvapi_write` 清单触发 GC6 预唤醒）+ lib.rs 再导出。
- **CLI**：`get-power-command [--channel N] [--command observed|request]`（默认 ch0/request；request 渲染 mW/W 视图，0xFFFFFFFF 渲染 unset）、`set-power-command <VALUE> [--channel N] [--command observed|request]`（VALUE 裸数=瓦特×1000，`mw` 后缀=原始整数；与 `set-pwr-cur-limit` 共用 `parse_power_channel_value`）。均 Group::Power、NVAPI_ONLY。高危注记：request=0xFE 直驱内核功率上限请求，无确认门。
- **`get-power-channels` → `get-pwr-cur-info`**（用户指示，旧名直接删除无别名）：与 `set-pwr-cur-limit` 构成同一功率/电流面的读/写对。**行单位分类法（用户判读法）**：min=1→电流通道（mA 渲染 A）、min≥1000→功率通道（mW 渲染 W，含全零退化行）；"unbounded (sentinel)" 分支取消，所有行带单位渲染（4060L 的 (15,3) 38 W、(4,5) 12.4 W 每轨功率行就此可读）。分类谓词 `PowerChannelPolicy::is_current_channel` 落 nvapi-rs 数据模型，`set-pwr-cur-limit` 写后渲染键同步用同一分类。
- **测试**：`power_command_commands_parse`（选项/选择器解析、ch 32 拒绝、0xAA 拒绝、旧名 `get-power-channels` 报错）+ 后端断言；cli 74 测试绿，core/nvapi/clippy/fmt 全绿。
- **边界**：活体仅 ch0/0xFE 有数据（两代实测=板功率 mW，与 NVML/通道表三面一致）；0xF8 全哨兵、ch1..31 空的作用仍待 Blackwell 实机（§18.6 L4 判据）。
