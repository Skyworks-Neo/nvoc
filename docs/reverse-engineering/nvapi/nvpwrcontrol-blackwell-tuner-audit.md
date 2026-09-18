# NvpwrControl 1.8.0 (616.92 Blackwell Laptop Tuner) 逆向审计

**5 行总结**
1. 三层工具：测试签名内核驱动 `Nvpwr.sys`（解析已加载 nvlddmkm 616.92 的活动 RM 电源策略对象，**只写数据字段+调原生 setter，不改任何代码页**）+ XMG v8 协作驱动后端（19 目标 250 W 契约）+ 用户态 NVAPI 调优器（GET→validate→SET→readback→rollback 全链）。
2. RM 电源策略对象模型全解：PowerRoot{base/amount/elig/UPPER/LOWER} + Board selector 槽（src 0xFE=MAX、0xF7=生成器输出）+ F7 生成器公式 `min(C+A, U)`——解释了笔电功率顶为何 API 层抬不动。
3. Blackwell 笔记本布局补充（无实机条件下的静态来源）：ClockDomains V2 控制块与 Ada 完全同构（772=0x304 步长），50 系记录 type 0x0A→0x0F、freq@rec+0x114、MSVDD@rec+0x11C；TopRels GPC:XBAR 比率语义 0xE660=0.9；Pstates20 5070Ti 实测范围。
4. 可达性结论：抬顶（UPPER/Board MAX/Type07/NVPCF maxima）的 mutator 是 RM 内部函数，未发现命令面出口——用户态 scope 结构性做不到；备选路线=vBIOS 功率表修改（工具链现成）或内核驱动（配方已解析，代价不可接受）。
5. 一处测量语义冲突待实机裁决：工具用 0x527FC458 测 "XBAR physical" 时 +4 传 0x2，与我们的 green-curve 5070 实证 XBAR=1 矛盾（index 语义下它测的是 SYS，或 50 系该字段改 mask 语义）。

来源：`reverse/NvpwrControl_61692_v1_8_0_unified_blackwell_tuner/`（完整源码树 ~5.6k 行：`driver/driver.c` 1318 行内核驱动 + `app/nvapi_tuner.cpp`/`xmg_probe.cpp`/`nvapi_probe.cpp`/`main.cpp` + `cli/nvpwrctl.cpp` + `shared/nvpwr_ioctl.h` + 8 份 md 文档）。参考机：Lenovo Legion Pro 5 Gen 10 / RTX 5070 Ti Laptop / vBIOS 98.05.4E.00.07 / 驱动 616.92。全部结论为**静态分析**，未经本机验证（我们无 Blackwell 硬件）。

---

## 1. 工具架构与安全机制链分析

### 1.1 它到底改了什么（以及没改什么）

Nvpwr.sys **不是**魔改版 nvlddmkm。它是一个独立编写的 WDM 驱动，加载后：

1. `AuxKlibQueryModuleInformation` 枚举已加载内核模块找到 nvlddmkm.sys 基址；
2. **精确构建门禁**：PE TimeDateStamp == `0x6A9B4070` 且 SizeOfImage == `0x06D3E000`（616.92 指纹），再逐字节比对 6 处机器码签名（见 §2.4）——任一不符即拒绝工作（fail-closed）；
3. 解析活动对象链（§2.1），对策略**数据字段**做 InterlockedExchange 写入，并**调用 nvlddmkm 内部已有的函数**（原生 setter/生成器）；
4. 全程不触碰 nvlddmkm 的代码页，不在磁盘上修改任何 NVIDIA 文件。

### 1.2 签名链：为什么要关 Secure Boot / 开 testsigning

与"改没改 NVIDIA 驱动"无关，链条是：

```
自签名新驱动 Nvpwr.sys（仓库内有 Nvpwr.cer + signtool tlog）
  → Windows DSE（CI.dll 加载时签名校验）拒载：自签证书不在微软信任链
  → 出路只有 testsigning 模式（bcdedit /set testsigning on，信任微软测试根）
  → testsigning 仅在 Secure Boot 关闭时生效（UEFI 设置里关，即"改 EFI 启动"的实质）
```

微软 WHQL/attestation 签名路线对一个"写别的厂商驱动的内部状态"的工具不可能通过审查，所以 test-signing 是唯一路径。

### 1.3 内核完整性校验拦的是加载，不是数据写

- **HVCI（内存完整性）/CI 校验对象是代码页与加载时签名**，不是跨驱动的数据区。Nvpwr 不会被"检测到数据被改"而拦截——但 HVCI 开启时 test-signed 驱动一律加载失败，故必须连带关闭内核隔离。是"签名等级达不到"，不是"改数据导致校验失败"。
- **PatchGuard** 只看守内核自身关键结构（SSDT/IDT/关键全局），nvlddmkm 的功率策略数据不在看守清单——所以写数据不触发 0x109 蓝屏，这条路技术上才能走通。
- README 明确声明不实现 DSE/Secure Boot bypass——需要用户手动配置环境。

### 1.4 为什么过不了游戏反作弊：三层独立拒绝

1. **环境层**：testsigning on / Secure Boot off 本身就是 Vanguard、FACEIT、ESEA 及部分 BattlEye/Ricochet 配置的拒绝或封禁条件——查的是启动标志，与装了什么驱动无关。
2. **存在层**：Nvpwr.sys 无微软签名、不在任何白名单，内核模块枚举即被标记。
3. **行为层**：在内核里**读写另一个驱动的内存 + 调用其内部私有函数**——手法与作弊器完全同款，行为检测不区分改的是"功率策略"还是"伤害值"。部分反作弊还监控 nvlddmkm 完整性/功率读数合理性。

**一句话**：不改代码只免了"文件完整性"这一关；进内核需要过不了签名链的自签驱动（Secure Boot/testsigning/HVCI 要求的来源），改他驱运行时数据+调内部函数（反作弊行为层拒绝的来源）。

---

## 2. RM 电源策略对象模型（616.92 特定，全新知识）

### 2.1 对象解析链

```
nvlddmkm + RVA 0x013B2E18          → DriverGlobal（全局状态指针）
DriverGlobal + 0x0208              → GPU 表
GPU 表 + 0x48C48                   → 注册表 GPU 数量（≤32）
GPU 表 + 0x48A48 + i*0x10          → Major 对象指针
GPU 表 + 0x48A50 + i*0x10          → GPU ID
Major + 0x25B0                     → PowerRoot（电源策略根）
PowerRoot + 0x1CC0                 → registry；+ 0x1CF8 → lookup 函数（须在镜像内）
Board = lookup(root+0x1CC0, key)   → Board 对象（须 board+0x2D0 == 基址+0x8D6960）
```

有效性门：root+0x3D10(init) == 1、root+0x3D1C(policy key) < 0x40。

### 2.2 PowerRoot 字段（root+）

| 偏移 | 语义 | 备注 |
|---|---|---|
| 0x3D10 | init 标志 | ==1 才有效；原生 setter 内部检查 |
| 0x3D11 | eligibility | Dynamic Boost 资格；setter 调用会**重跑生成器** |
| 0x3D12 | amountActive | 动态量激活标志 |
| 0x3D14 | base / cTGP 输入 | 生成器的 C 项（mW） |
| 0x3D18 | PPAB amount | 生成器的 A 项（mW），DB 预算 |
| 0x3D1C | policy key | lookup Board 用 |
| 0x3D20 | LOWER | 生成器的 B 项 |
| 0x3D24 | UPPER | **饱和顶**，F7 生成的钳制值——抬顶的核心字段 |
| 0x3D28/0x3D2C | aux | 未破译 |

### 2.3 Board 对象与 selector 槽

- Board+0x2D0：type-0 setter 函数指针，原型 `(Major, Root, Board, Selector, Source, Value) → nvStatus`。
- Board+0x104：**selector2 = MAX 槽**，出厂 source=`0xFE`（静态策略源）。
- Board+0x1F4：**selector3 = CURRENT 槽**，source 列表含 `0xF7`（生成器输出）。
- selector 槽布局：`mode@+0, count@+1, effective@+4(u32), secondary@+8(u32), source 数组@+0xC`，数组项 stride 8 = `{source_id u8, pad, value u32}`，count ≤ 8。

**跨证**：XMG 包的 Type07 selector 为 24 字节 `{mode, count, effective, reference, tag, tagged}`——与 Nvpwr 的 Board 槽是同族"功率策略 selector"对象（§5）。

### 2.4 原生函数 RVA 与生成器语义（616.92）

| RVA | 函数 | 签名字节（门禁用） |
|---|---|---|
| 0x004E3EF0 | F7 生成器 | —（分支见下） |
| 0x004E4610 | SetAmount(Major, amount) | `48 83 EC 38 48 8B 81 B0 25 00 00 ...`（`mov rax,[rcx+0x25B0]` 重取 root，`cmp byte [rax+0x3D10],0` init 检查） |
| 0x004E4680 | SetEligibility(Major, elig) | 同构；**调用即重跑 0x4E3EF0 生成器** |
| 0x008D6960 | BoardSet(Major, Root, Board, Sel, Src, Val) | prologue 三连 push |
| 0x004E3FBF | UPPER 加载指令位 | `44 8B 8B 24 3D 00 00`（`mov r9d,[rbx+0x3D24]`） |
| 0x004E4088 | F7 记录写点 | `66 C7 44 24 48 F7 03`（`mov word [rsp+0x48], 0x3F7`，即 source=0xF7+mode=3 打包） |

**F7 生成器有限 cTGP 分支**（严格主动态）：要求 `amountActive=1 ∧ elig=1 ∧ U>B ∧ A≤U−B`，此时

```
F7 = min(C, U−A) + A  ≡  min(C+A, U)
```

**这就是桌面式功率接口在笔电失效的数学原因**：只动生成器输入（C/A，即我们已知的 PPAB 封装面）永远被 U 钳制——Nvpwr 的 Phase A 正是利用这一点"檐下整备"自证连贯性。

### 2.5 出厂态 vs DB 激活态（补 PPAB 底层语义）

| 状态 | elig | amountActive | base(+3D14) | amount(+3D18) | F7 |
|---|---|---|---|---|---|
| 出厂 OEM（coherent baseline） | 0 | 0 | ==UPPER | 0 | ==UPPER |
| DB 激活（Nvpwr applied） | 1 | 1 | target−25k | 25000 | ==UPPER==target |

即 RM 层 Dynamic Boost = (base 降至 cTGP) + (amount=DB 预算) + elig=1；出厂时 base 字段==最大功率。我们 `nvoc-power-thermal-wrapped-families` 里的 TGP/PPAB 封装接口就是这组字段的命令面出口——**但只有生成器输入侧，没有顶侧**。

### 2.6 三阶段转移 + 回滚方法论（工程范式可复用）

```
Phase A 檐下整备：BoardSet(sel2,0xFE,saveOEM) + UPPER=OEM + base=target−25k
                  + SetAmount(25k) + SetElig(1) → 回读要求 state=Armed
Phase B 抬双顶：  BoardSet(sel2,0xFE,target) + UPPER=target（只抬一个不够）
Phase C 重生成：  SetElig(1) 触发原生生成器 → 全量回读要求 state=Applied
任一阶段失败 → 回滚：恢复 root 四字段 → SetElig(saved) 重生成 → BoardSet(sel2,0xFE,saved) → 验证 StockBaseline
```

安全设计要点（值得搬进我们的写路径规范）：同会话首改前保存基线（root/board 身份+五字段）；回滚拒绝对象身份变化；对"外部遗留修改态"只认极窄恢复族（UPPER 145–160k、5k 对齐、amount ∈{25k,30k@150W,40k@160W}、全链一致），其余 MIXED 一律拒绝；profile 门（5050/5060/5070 须 OEM==115W 才许 120–140W；5070Ti 须 OEM==140W 才许 145–180W；5080/5090 须 OEM 150–175W 才许 175–225W）。

---

## 3. NVAPI ID 交叉验证表

工具 14 个 ID vs 我们 `nvapi-rs` 注册 vs `reverse/nvapi-interface-table-46296.txt`（462.96 QI 表，13/14 在位）：

| ID | 工具用途 | nvapi-rs | 462.96 表 | 备注 |
|---|---|---|---|---|
| 0x6FF81213 | Pstates20 GET | ✅ nvid.rs:293 | ✅ | V2 magic 0x21CF8(7416B)/V3 0x31CF8 级联我们已有 |
| 0x0F4DAE6B | Pstates20 SET | ✅ nvid.rs:294 | ✅ | 工具确认最小 V1(0x11C94,7316B) 请求可用 |
| 0x57B5A5DF | ClockDomains INFO | ✅ clock.rs:1860 | ✅ | 616.92 接受 0x486AC，与我们 610+ 审计表吻合 |
| 0xF58938F5 | ClockDomains GET_CONTROL | ✅ clock.rs:1865（RM 0x2080901b） | ✅ | Ada magic 0x10964 / Blackwell 0x261A4 |
| 0xD14B69CF | ClockDomains SET_CONTROL | ✅ clock.rs:1872（RM 0x2080d01c） | ✅ | 同上 |
| 0x527FC458 | 直接频率测量 | ✅ clock.rs:1582 | ❌（晚于 462 世代） | 语义冲突见 §4.3 |
| 0xE826E4F0 | TopRels GET_INFO | ❌（结构在 nvclocks-audit） | ✅ | 工具补语义 §4.4 |
| 0xCBFF71D0 | TopRels GET_CONTROL | ❌ 同上 | ✅ | magic 0x1075C 双方一致 |
| 0xEF3D20EA | TopRels SET_CONTROL | ❌ 同上 | ✅ | rec+0x68=比率字段 |
| 0x507B4B59 | V/F points INFO | ✅ nvid.rs:757 | ✅ | Blackwell magic 0x1182C(6188B) |
| 0x23F1B133 | V/F points GET_CONTROL | ✅ nvid.rs:758 | ✅ | Blackwell magic 0x12420(9248B) |
| 0x0733E009 | V/F points SET_CONTROL | ❌ | ✅ | 工具 presence-check 过、刻意不调 |
| 0x68789E2A | ADC/rail INFO | ✅ clock.rs:5039 等 | ✅ | Blackwell magic 0x209F0(2544B)，mask@+4 |
| 0x43D9B26A | ADC/rail STATUS | ✅ clock.rs:4403 等 | ✅ | Blackwell magic 0x109C8(2504B) |

（另有 NVAPI_INITIALIZE 0x0150E828 / ENUM 0xE5AC921F / UNLOAD 0xD22BDD7E 三个常规项，双方一致。）

---

## 4. Blackwell 50 系布局补充（核心价值：无实机静态来源）

> 背景：我们的 measure/control 域序在 ≤40 系已实证，但 50 系辅助域（Msd 等）顺序全错，且手头长期无 Blackwell 卡——本工具是 50 系笔记本知识的唯一静态来源之一。

### 4.1 ClockDomains V2 控制块：与 Ada 完全同构 + 三个新锚点

- magic **0x261A4** = ver2 | 0x61A4(24996B)——正是我们 clock.rs 已注册的 `NV_GPU_CLOCK_CLIENT_CLK_DOMAINS_CONTROL_V2`（工具写 buffer 0x13000 ≥ magic size，与我们一致）。
- **mask@+8 = 0xFF 种子**：与我们 `set_mask(0xFF)` 逐位一致（u32::MAX 被拒、0xFF 接受）。
- **记录步长 0x304 = 772 字节——与 Ada V2 完全相同**（我们的 `clk_ctrl_entry_v2::STRIDE=772`）。同构确认，非新家族。
- Blackwell 差异三点：
  1. 记录 type 判别字（rec+0 低字节）**0x0A（Ada 实测）→ 0x0F（Blackwell）**；
  2. freq（kHz）@ **rec+0x114** = 我们的 VALUES[2]（VALUES 基 = rec+268）；
  3. MSVDD 请求（µV）@ **rec+0x11C** = VALUES[4]。
- 工具的发现策略（值得采纳）：不硬编码域索引，扫描 ≥0x100 起重复出现 marker 0x0F 的唯一 run 且要求步长恰为 0x304（"audited Blackwell entry stride"，其余拒收），再在 32 槽中找**唯一非零 freq/MSVDD 条目**作为 XBAR（回落 NvAPI 枚举默认 1）——这是对 50 系辅助域序号错乱问题的工程幸存方案。
- 附加：ClockDomains 专用测量 ID 0x527FC458 V1(0x1000C, 12B)，XBAR 测量掩码 0x2、输出 kHz@+8。

### 4.2 Pstates20 @5070Ti/616.92 实测范围

- GET V2 0x21CF8 可用；布局：header 20B（@8=np states、@12=n clocks、@16=n voltages），pstate 记录 456B（P0 判别@+0），clock 项 44B×8（domain@0、cur@+12、min@+16、max@+20；domain 0=graphics、4=memory），voltage 项 24B（在 +8+8×44 处，同 cur/min/max 偏移）。
- 参考机范围：**core −1000..+1000 MHz、mem −1000..+3000 MHz**；**无 per-pstate 基准电压条目 → NVVDD delta 被禁用**（不伪造不存在的控制）。与我们 set_overvolt 走的 voltages[] OV 数组（@+7332）是不同字段，注意区分。
- SET 用最小 V1（0x11C94）请求：仅 P0 + 目标域，保留字段全零——与我们 set_pstates 的"零结构+只填目标项"同款，互证。

### 4.3 测量语义冲突（待实机裁决）★

工具 `MeasureXbar`：12B 缓冲，ver 0x1000C@+0，**0x2@+4**，读 kHz@+8，标注 "Physical XBAR clock"。
我们的记录（green-curve 在 RTX 5070/610.88 差分写实证 + 4060L 四域 <2% 交叉验证）：+4 为域**索引**，GPC=0、**XBAR=1**、SYS=2、MCLK=4。

两种读法静态不可裁决：
- **读法 A**：+4 仍是索引 → 工具传 0x2 实测的是 SYS，其 "physical XBAR" 展示值是错域（工具作者的显示未必被人眼核过）；
- **读法 B**：50 系该字段改 **mask 语义**（bit = 1<<域索引 → XBAR 恰为 0x2）→ 工具对、我们 40 系索引语义在 50 系变化。

实测方案（有 Blackwell 机时）：GPC 负载下分别传 1 和 2 读值，用 **TopRels GPC:XBAR 比率 0.9 做自检**——XBAR_phys ≈ 0.9 × GPC_phys 者为真 XBAR。

### 4.4 TopRels 语义层（补齐我们已有结构）

我们 nvclocks-audit 已解 TopRels 全族结构（INFO 0x15798=0x8E8+255×0x150、control 0x1075C=0x64+255×0x108、tag 3..7 负载形状）；工具补上**语义**：

- INFO 中的语义关系记录：`{u32 0, byte 0, byte 1, byte 1, u32 0xE660}` = 源 GPC(0) → 目标 XBAR(1)、bidirectional=1、默认比率 raw 0xE660；
- 比率为 **U16.16**：0xE660 = 0.9001…（工具刻意把 0.9 直接编码为 0xE660 避免 one-LSB 回读不匹配）；
- GET/SET_CONTROL 记录内比率字段 @ **rec+0x68**（我们的 tag-3 负载 u32@wire+0x64→rec+0x68 正是它）；写入范围门 0.0..2.0；
- 工具先验 INFO 关系（恰一条 GPC→XBAR 记录）再 SET——语义门+结构门双层校验。

### 4.5 V/F points 与 ADC（Blackwell 尺寸锚点）

- V/F：INFO 0x507B4B59 magic **0x1182C**(6188B)；GET_CONTROL 0x23F1B133 magic **0x12420**(9248B)；SET_CONTROL 0x0733E009 在位。info 响应 **@+4 起 32B mask** 作 GET_CONTROL @+4 的种子。工具对 616.92 的 V/F 状态布局判定为"远大于公开参考布局"，**拒绝启用通用 V/F 写入器**（"SET ID 存在"≠"可安全写"的 fail-closed 判断，值得写进我们的能力门规范）。
- ADC：INFO 0x68789E2A magic **0x209F0**(2544B)，**mask@+4**；STATUS 0x43D9B26A magic **0x109C8**(2504B)。链路在 5070Ti 可用。

---

## 5. XMG v8 250 W 契约（第二后端，19 目标）

XMGPowerPatch（未随包分发，`\\.\XMGPowerPatch`）是另一独立内核驱动，走**语义发现**路线：

- **硬件门**：VEN 10DE ∧ DEV 2C18(5090L)/2C19(5080L) ∧ subsystem vendor **1D05** ∧ 恰一 GPU/布局/对象 ∧ generation 稳定。
- **协议**：QUERY IOCTL `0x00226000`（in `{8, ver8}` → 2056B）；APPLY/ROLLBACK `0x0022A004`（in `{16, ver8, op, 0}`，op 1=Apply 2=Rollback → 568B）。用户态不能提供目标值——固定 225k base + 25k DB = 250k。
- **19 写目标**（出厂→目标，mW）：
  - Core 7：StateMaximum 175k→250k；MainChannel Selector2 Effective/Tagged 175k→250k；NVPCF MaximumA/B/Alias 175k→250k；NVPCF **Base 150k→225k**（注意出厂 base≠出厂 max）。
  - Type07 Entry13（数组 idx 13，ID **0x1B**）6 目标：selectors 1/2/3 effective+tagged，210k→250k。
  - Type07 Entry14（数组 idx 14，ID **0x1C**）6 目标：60k→100k（语义未破译，疑副轨/平台预算）。
- **状态机**：FULL_STOCK(1) / FULL_TARGET(2) / LEGACY_CORE_TARGET_TYPE07_STOCK(3)，其余混合/部分态**零写入**；每写即回读，失败逆序回滚；Apply 后独立全量 QUERY 复验。
- **275 W 结论**：契约不可外推（"250000→275000"不成立），须独立建立 Core/NVPCF/E13/E14 值与不变量——工具明确锁死，这个判断本身值得记录。
- QUERY 输出 2056B 布局要点（偏移=word×4）：overall@2、platform gate@4、PCI id@5-7、NVPCF/NVIDIA status@8/29、safety 计数器@49-55、generation@58、resolver profile@219/220、layout 证据块@223-286、capability flags@287（bit0/1/2=core250/entry13/entry14）、state@288、Entry13@1164、Entry14@1252（各含 idx/embedded/id/identity + 3×24B selector）。

---

## 6. 可达性评估：nvoc 用户态 scope 能否抬顶

**结论：不能。结构性限制，不是知识缺口。**

字段分两组：

1. **生成器输入**（root base/amount/elig）：有用户态封装出口——即我们已知的 TGP-watt/PPAB 封装面。但 F7=min(C+A,U)（§2.4），工具 Phase A 实证：不抬 U，F7 恒被钳在出厂顶。桌面滑条同样走这条且被 UPPER 校验。
2. **顶本身**（root+0x3D24 UPPER、Board selector2 越界值、Type07 Entry13/14、NVPCF maxima）：mutator 是 RM 内部函数（0x4E4610/0x4E4680/0x8D6960），**未发现任何 RM 命令面出口**（escape 0x0700xxxx / RM 0x2080xxxx 均无"写顶"）。

四重佐证：①原作者实测桌面功率路径 → INVALID_ARGUMENT；②Nvpwr 与 XMGPowerPatch 两个独立作者**都选择下内核**而非找接口；③Afterburner 类工具普遍上限=OEM max；④我们 public power limit 双后端经验一致。

**备选路线盘点**：

| 路线 | 状态 | 代价/风险 |
|---|---|---|
| 静态终审：idalib 枚举 616.92 中 0x3D24/selector2 全部写入者并验 escape 可达性 | **我们 scope 内可做**，一次审死 | 无风险；若发现可写命令面则 API 路线打开（概率低非零） |
| vBIOS 功率表修改（MPT 传统：Pascal/Turing 时代改表抬笔电 TGP） | 工具链现成（功率表 min_mw、mod-256 校验、P 指针重定位），**Blackwell 未验证** | 平台 EC 可能重钳；闪改签名/变砖风险 |
| 自写内核驱动（本工具配方已完整解析） | 技术可行 | testsigning+关 SecureBoot+HVCI 关+反作弊全拒——对可分发工具不可接受 |
| WinRing0 类签名驱动做内核写 | 绕过 testsigning | 微软易受攻击驱动黑名单拦截 + 反作弊行为层照样抓 |

---

## 7. 对 nvoc 的行动项（记录，本期不实施）

1. `nvapi-rs` clock.rs：为 V2 控制块补 Blackwell type-0x0F 锚点常量（`BW_TYPE=0x0F, BW_FREQ=+0x114, BW_MSVDD=+0x11C`）+ "唯一活跃条目" XBAR 发现助手。
2. TopRels：按 §4.4 加 GPC:XBAR 比率语义访问器（U16.16，0.9↔0xE660 特判），挂在 nvclocks-audit 已有结构上。
3. CLI 探针：XBAR/辅助域定位改用 marker-scan + 唯一活跃条目策略，弃硬编码索引（50 系域序错乱的防呆）。
4. 50 系测量域序实测探针任务书（有 Blackwell 机时）：§4.3 方案 + 逐域差分写识别 Msd/Hub/Disp/Host 的 50 系真序。
5. 可达性终审任务书：`db_open` 616.92 nvlddmkm → xref 枚举 root+0x3D24 与 Board selector2 写入者 → 逐一溯源是否 RM 命令面（0x2080xxxx escape handler）可达 → 出"API 层抬顶是否可行"的最终结论。
6. 能力门规范：任何"SET ID 存在"不等于"可安全写"——须布局/范围/回读三重门（工具的 V/F 禁用判断是范例）。

## 8. 来源与置信度

- 全部为**静态源码分析**；工具自称 5070Ti 上 145/150/160W 实机验证、XMG 2C18 live-verified / 2C19 pending（其自带标注，未独立验证）。
- 所有 RVA/偏移/魔数严格限定 nvlddmkm **616.92**（timestamp 0x6A9B4070）；跨驱动版本不成立（工具自己的门禁哲学）。
- 与我们的交叉验证项（772 步长、mask@+8、Pstates20 布局、magic 公式）为双源一致，置信度高；工具独有断言（RM 对象偏移、XMG 契约值）为单源，标注待验证。

---

## 9. 40 系首测（2026-09-18，RTX 4060 Laptop，`blackwell_recon_live`）

工具落地当天在 4060L（R610 世代驱动）首测，本节为活体判读：

### 9.1 TopRels 拓扑树首次活体填充——域号表被独立表面交叉验证

GetInfo 返 0，count=15、mask 11 条，raw bytes=`{src,dst,bidir}`：

```
rec0  GPC(0)→XBAR(1)      rec5  XBAR(1)→Hub(4)     rec8  Mem(2)→Host(9)
rec1  XBAR(1)→Sys(3)      rec6  Mem(2)→XBAR(1)     rec9  Mem(2)→Msd(5)
rec2  XBAR(1)→Host(9)     rec7  Mem(2)→Sys(3)      rec10 XBAR(1)→Disp(7)
rec3  XBAR(1)→Msd(5)      rec4  Mem(2)→PcieGen(8)
```

边集与物理常识完全自洽（GPC→XBAR→外设群；Mem→XBAR/Sys/Host/Msd/PcieGen），域号 0/1/2/3/4/5/7/8/9 与 nvclocks 审计最终域表**逐一吻合**——`clkprop-tops-toprels-regimes.md` 的"本机树空"悬念在 Ada 上以满树形态收官，域号表获得第三方表面实证。

**代际差异（重要）**：Ada 的 GPC→XBAR 边是 **enum=2（wire tag 5）、payload=0——没有比率字段**。工具的严格五元组门（enum==0 + payload==0xE660）在 Ada 必然失配，比率记录（tag 3 + 0xE660）是 **Blackwell/616.92 特有**。我们 `find_gpc_xbar_records()` 采用仅字节匹配 `[0,1,1]` 的宽松门（payload 另读）是正确设计。GetControl 返 -1：Ada 上比率控制未填充，与 payload=0 自洽。

### 9.2 ClockDomains V2：代际探针验证 + Ada 布局再确认

status=0、controllable mask=0xFF，**8 条记录在 bit 0..7**：dom 0-5/7 = type **0x0A**、dom 6 = type **0x02**。代际探针验证成立（Ada=0x0A ↔ 50 系预期 0x0F）；BW 锚点（freq@+0x114/MSVDD@+0x11C）在 Ada 全读 0——符合预期（Ada 双平面值在 VALUES[0]/[1]，见 vf-curve-families 记忆），锚点常量不影响 Ada 读路径。`find_unique_populated_entry()` 在全零 Ada 块上正确返 None（不误触发）。

### 9.3 测量域 40 系基线表（50 系对照的 diff 基准）

空闲期 0x527FC458 五槽全通（status=0）：

| +4 | 读数 | 判读 |
|---|---|---|
| 0 | 26.26 MHz | GPC 深空闲门周期伪影（与既有 4060L 记录一致；负载下重测应 ≈2000） |
| 1 | 1054.16 MHz | XBAR（≈GPC/2，轻载） |
| 2 | 1004.73 MHz | SYS（与 XBAR 接近，负载下重测分辨：XBAR 随 GPC 走、SYS 定频） |
| 3 | 449.90 MHz | **新数据点**：测量空间 3 未在我们域表归因，读定频 ~450（疑 Disp/Hub） |
| 4 | 7009.71 MHz | MCLK 铁证（GDDR 有效频率） |

≤40 系 index 语义再获确认（4=MCLK 无歧义）。**50 系裁决协议**：同表重跑，若 +4=2 变为 ≈0.9×GPC 而非 ~1000 定频 → mask 语义定案（工具对）；若仍 ~1000 定频 → index 语义延续，工具的 "XBAR physical" 实为 SYS。负载下跑 +4∈{0,1,2} 可同时钉死 0/1/2 归属。

**负载态补测（同日）**：0=2220.69（GPC）／1=1962.60（XBAR，**GPC 比 0.8838**——非 0.9，0.9 是 Blackwell 工具默认，代际有移）／2=1879.47（SYS，比 0.8464，与 XBAR 比值两态恒 ≈1.045，同为 GPC 派生域）／3=449.81（**定频不变**，用户假设 HUB——待差分写确认）／4=7993.53（MCLK=16Gbps 满档）。≤40 系 index 语义负载态复核成立。slot 上界未知，探针已扩到 +4=0..15（K4000 全 -3=族不支持，属预期；40/50 系可枚举完整可测集）。裁决协议不变：50 系看 +4=2——≈0.9×GPC ⇒ mask 语义定案；跟随 SYS 特征 ⇒ index 语义延续。接口本身 nvapi-rs 早已接线（`Gpu::clk_domain_freq_direct`，green-curve 集成），探针是 sys 层直调扫表。

**机器归因纠正（用户指正）**：§9.3 的 +4=0..15 扫表、TUI FCLK 对照、V2 块（0x08/0x09）三组数据**全部来自 RTX 2070（Turing）**，非 4060L。修正后：测量 7 槽映射 {0=GPC,1=XBAR,2=SYS,3=HUB,4=MEM,5=HOST,6=DISP,7+=-104} 为 **Turing 实证**（2070 扫表 + 2070 TUI GetAllClocks 同源对照，HUB=slot3/MSD 无槽由此定案），与 4060L 已证 0..4 子集跨代一致（Ada 的 5..15 待扫）。MSD 无测量槽为 Turing 观测。V2 记录子型：Turing={08×3,09×4,02}、Ada={0A×7,02}、Blackwell={0F}(工具)——type=记录子型非纯代际戳。XBAR:GPC=0.8838 为 4060L 负载态事实，不受此纠正影响。
