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
