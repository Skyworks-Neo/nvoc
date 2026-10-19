# MODS 量产检验工具（520.220.2 / 570.278）逆向 → Blackwell RTX PRO 6000 审计交叉参考

**日期**：2026-10-11
**来源**：两套 NVIDIA MODS（Modular Diagnostics & Exerciser Suite）量产检验包的 idalib + 解密联合分析：
- **4060 包** `520.220.2_PG173SKU371_TTR_4060`（RTX 4060 / AD107，MODS 520.220，构建 linuxmfg 2023-04-10，内嵌 RM = r520_00-634 / 520.89）
- **5050 包** `570.278 - GB207 RTX5050/659-1G152-0050-MFG-43071`（RTX 5050 / GB207 Blackwell，MODS 570.278 / 框架 4.30，2025-04）

**目的**：回应 [`blackwell-pro6000-580-173-02-audit.md`](../nvapi/blackwell-pro6000-580-173-02-audit.md) 的三个悬案（TopRels 唯一活写面的机制语义、`RmVFPointCheckIgnore` 杠杆的精确配方、"记录面可写/行为面脱钩"的候选三=缺 commit 步骤），并提供 MODS x86-64 内嵌 RM ↔ GSP-RM 固件的桥接方法与 GB202 数据资产。

**可信度**：本文所有 JS 引用均出自**已解密明文文件**（磁盘可核验，路径见 §6）；解密算法经 zlib 全解压 + 头部 u64 精确匹配双重自证。C++ 侧结论标注（⚠️=子代理反编译产出，内部一致未逐条复核）。

---

## 0. MODS 是什么（一段话）

NVIDIA 产线/返修 GPU 诊断框架 = **用户态完整 RM + 最小内核直通层**。内核侧 `/dev/mods`（misc 设备，GPL-2.0，`driver.tgz` 附完整 C 源码）只提供特权原语：任意物理地址 mmap、PCI cfg/BAR、DMA/IOMMU、用户态服务的中断接管、任意 ACPI 方法求值；GPU 全部知识（寄存器、时钟树、Volt3x、AVFS、内嵌 NVRM）在用户态 `mods` 主程序（248-393MB 静态 ELF）里，Blackwell 上自举 GSP-RM 固件。测试逻辑用 JS 编写（SpiderMonkey 内嵌），`.jse/.he` 加密分发——**加密已完全破解**（§6）。

## 1. TopRels = 时钟传播拓扑关系（审计唯一活写面的机制语义）

审计发现 `0xEF3D20EA` SetControl 是服务器卡第一条行为活写面（XBAR = ratio × GPC，下行自由、上行钳死 devinit 0.9），但机制未知。MODS 产线脚本给出完整语义模型——它是 **clock propagation topology relationship** 机制：

**证据（`jse_dec_4060/gpuargs.jse.dec`，4060 包解密件）**：
- `:1678` — CLI 参数注册：
  ```js
  AddArg("set_prop_top_rel_ratio", FUNC2, "ArgSetClkPropTopRelRatio", null, OS_KERNELRM,
         "Set the ratio for a clock propagation topology relationship", ParamConst.PARAM_MULTI_OK);
  ```
- `:2122-2126` — 实参结构：`{"ratioIdx": <Index>, "propTopRelRatio": <Ratio>}`（**按关系 index 设比率**，多关系可批量）。
- `:8130-8132` — 关系枚举入口：`GpuSubdev.Perf.GetClkTopPropRelsInfo(clockTopPropRelsInfo)` 遍历返回的关系表。
- `:8092-8093` — 关系表条目字段：`TopologyId: %u, clkPropTopRelMask: 0x%x`（`propTopInfo.topId` / `propTopInfo.clkPropTopRelMask`）。
- `:137-141` — 同族调试参数：`-dump_clk_propogation_regimes` / `-dump_clk_propogation_topology` / `-forced_propagation_topology` / `-dump_clk_propogation_topology_rels`。
- `:8385-8392` — regime 与 2x↔1x 域换算：`GpuSubdev.Perf.Is2xClockDomain(clk)` / `ConvertTo1xClockDomain(clk)` / `SetRegime(clk, regime)`。

**对审计的推论**：
1. 0.9"devinit 上限墙"是**每条拓扑关系自身的初始比率**，不是全局策略钳位 → 审计 v4 遗留的"细边界扫描"应先 `GetClkTopPropRelsInfo` 式枚举（对应 shim 读面），逐关系测绘各自的下限/上限，而非对单一 index 扫。
2. `clkPropTopRelMask` 暗示一个拓扑 id 下挂多条关系（XBAR/GPC 之外还有 SYS/uProc 等派生域，与审计实测 index 0/1/2/20/21/22 的测量宇宙一致）。
3. MODS 把它归在 `OS_KERNELRM` 权限级 → 与审计 root 门一致。

## 2. `RmVFPointCheckIgnore` 配方（审计最有希望杠杆的活体用法模板）

审计从 GSP 固件字符串锁定该键为最有希望的宿主侧杠杆，但只有键名。MODS 产线脚本 `fpf.jse.dec`（4060 包，post-SLT 熔丝烧写流程）给出**完整配方**——使能 → 写电压限 → 回读校验 → 恢复：

```js
// fpf.jse.dec:88-112（ProgramNvvdd）
if (IsSplitRailSupported(this.subdev))                 // :85  VoltRailMask 位计数 >1 = split-rail 卡
    this.subdev.Volt3x.SplitRailConstraintAuto = true; //     分轨卡先开自动约束

Registry.ResourceManager.RmVFPointCheckIgnore = true;  // :90 "Allow setting voltage limits"

let voltageLimitsMv = {low: 800, high: 880};           // :94  目标窗口 800-880 mV
let nvvddToSetMv = (voltageLimitsMv.low + voltageLimitsMv.high) / 2;
let voltToSet = [{railIdx: Gpu.VOLTAGE_LOGIC, voltMv: nvvddToSetMv}];
CHECK_RC(this.subdev.Volt3x.SetVoltagesMv(voltToSet)); // :99  批量轨电压（内部 ×1000 转 uV）

let currentNvvddMv = [];
CHECK_RC(this.subdev.Volt3x.GetVoltageMv(Gpu.VOLTAGE_LOGIC, currentNvvddMv)); // :104 回读校验
if (currentNvvddMv < voltageLimitsMv.low || currentNvvddMv > voltageLimitsMv.high)
    return RC.VOLTAGE_OUT_OF_RANGE;                    // :111 不落窗即报错
```

恢复纪律（`fpf.jse.dec:139-148`，Cleanup）：
```js
Registry.ResourceManager.RmVFPointCheckIgnore = false;         // :141 用完即关
this.subdev.Volt3x.SplitRailConstraintAuto = this.oriSplitRailConstraint;  // :144
CHECK_RC(this.subdev.Volt3x.ClearSetVoltage(Gpu.VOLTAGE_LOGIC));           // :147 清电压设定
CHECK_RC(this.subdev.Perf.SetPerfPoint(this.origPP));                      // :148 还原 perf 点
```

**键语义**（⚠️ C++ 侧，5050 包 A4 报告 §3.4）：`SendVoltLimitsToRM`（sub_17062F0）在打包 80,904B 参数（cmd `0x20801ACF`，7 逻辑轨 × 316B/轨）前，对每轨做 **over-voltage limit / reliability limit 预检**，`rmkey byte a1+88100`（即该键）控制越限时是放行还是告警。运行时设置通道 = `gpuargs.jse.dec:953` 的 `-rmkey` 参数（`AddArg("rmkey", FUNC2, "ArgRmRegistryKey", null, OS_KERNELRM, "Set a registry key for ResourceManager.")`）。

**对审计的推论**：
1. 服务器卡实验路径：`NVreg_RegistryDwords=RmVFPointCheckIgnore=1`（模块参数，需 root + 重载）≈ MODS 运行时 `Registry.ResourceManager.RmVFPointCheckIgnore=true`——两者是同一个 RM 注册表键（MODS 的 `-rmkey` 与 NVreg_RegistryDwords 最终都落在 RM init/registry 读取）。
2. **预期效果边界**：该键放宽的是 **VF 点校验 / 电压限预检**，不是引擎策略——写仍需走正确的 RM cmd（MODS 用 `0x20801ACF` 而非 NVAPI 面 0x2080D01C/D02D）。它解释不了记录面惰性，只解决"写了会被拒/告警"。
3. 配套键（MODS JS/固件双侧在位）：`RMImpPerfLimitMask`（`gpuargs.jse.dec:958`）、`PerfPmaControlReg`（`:959`）、`g_RMKeyMaxMclkFbstopTimePresent`（`:136`）。

## 3. "记录面可写、行为面脱钩"的候选三 —— MODS 产线行为化序列

审计疑"需要 shim 之外的 commit 步骤"。MODS 让时钟/电压设置**行为化**的产线序列如下（全部解密件可核）：

| 步骤 | API（JS 面） | 证据 | 语义 |
|---|---|---|---|
| ① regime/拓扑 | `SetRegime(clk, regime)` + 2x→1x 换算 | gpuargs.jse.dec:8385-8392 | 时钟传播 regime 设置（与 §1 同机制族） |
| ② perf 点提交 | `SetPerfPoint({PStateNum, LocationStr})` | boostbase.jse.dec:837 `{"PStateNum":0,"LocationStr":"max"}`；:1035 `"tdp"`；:654 退出前还原 | **行为化的核心提交步**——LocationStr 支持 `max/intersect/tdp` |
| ③ backdoor 注入 | `InjectPerfPoint({Clks:[{Domain, FreqHz}]})` | mods.jse.dec:1315-1322 | **仅当 `Subdev.Perf.NumPStates === 0`**（"PStates 3.0 supported by RM but disabled in VBIOS"）时用注入后门强制频率 |
| ④ 电压轨 | `Volt3x.SetVoltagesMv` → `GetVoltageMv` 回读 | fpf.jse.dec:99/104 | 见 §2 |
| ⑤ 负载验证 | heater/stress（FGHeater、test 2/23/92） | boostbase.jse.dec:839-847；tunevolt.jse.dec 全文 | 写后**行为校验**而非仅记录回读 |
| ⑥ 还原 | SetPerfPoint(orig) / ClearSetVoltage | boostbase.jse.dec:654；fpf.jse.dec:147-148 | — |

关键判读（对照审计三候选）：
- **MODS 从不直接依赖 ClkDomains/VF 记录 SET 面让频率行为化**——它走 `SetPerfPoint`/`InjectPerfPoint`（perf 点 API，另一族 RM cmd）。审计实测 0x2080D01C/0x2080D02D 是纯记录存储器，与此完全自洽：**记录面本来就是 GET 镜像/调参面，行为面在 perf 点 + 引擎策略对象**。
- `InjectPerfPoint` 的触发条件（VBIOS 禁 PStates 时）暗示 RM 内部存在"perf 点 → 时钟树"的独立附接路径——审计的遗留①（root+7408 槽 544/545/600 读取者穷举）应把 **InjectPerfPoint/SetPerfPoint 的 RM impl** 列为已知读者候选。
- ⚠️ 二进制侧补充 op 面（A3 字符串表，4060 包）：`EnableArbitration/DisableArbitration/LockArbiter/SetArbiterMinMode`、`SendPerfLimitsToRM`、`DisablePerfLimit/DisableAllPerfLimits/RestoreDefaultLimits`、`SetFmonTestPoint(gpcclk_MHz, margin_MHz, margin_uV)`、`ScaleSpeedo/OverrideKappa/OverrideVfeEquOutputRange`——审计在服务器卡上做"解除钳制"实验时，这些是比裸 SET 记录更接近引擎的入口（各自对应私有 RM cmd，需在 shim/GSP 侧定位）。

## 4. MODS x86-64 内嵌 RM ↔ GSP-RM 固件：assert 字符串桥接法

审计 K-6/T4-3 已立项 GSP 固件逆向（`gsp_ga10x.bin` 内层 ELF，75MB 未剥符号 RISC-V，27,985 函数）。MODS 提供一条加速路径：

**原理**：MODS 570.278（GB207 量产版）内嵌**同源 RM 的 x86-64 宿主编译**（A4 §5：GspRmControl=sub_6B6AB80 自举 GSP-RM；CLK_DOMAIN 代际对象、Volt3x、PMGR 字符串全套在位）。两个二进制共享 NVRM 的 assert/format 字符串——审计在 GSP 里已用 assert 锚定（softFloor.c/PERF-CF/PMGR），同样字符串在 MODS x86-64 里存在且**反编译质量远高于 RISC-V**。

**操作法**：取审计已锚定的 GSP assert 串（如 `"Invalid CLK_DOMAIN_50_PROG_PHYSICAL for the LOGICAL clkIdx %u"`）→ 在 MODS 二进制 `find_regex` 同串 → 顺 xref 找到 x86-64 impl → Hex-Rays 解出完整对象图/字段布局 → 按字符串+结构形状映射回 GSP RISC-V 地址。审计算法（"逐 finding 对抗验证"）不变，只换一个更好解的平行样本。

**本机已有资产**（未入库，见 §6）：4060 包 1.5GB IDA 库（214,094 函数）+ 34 个反编译件；5050 包 IDA 会话含 Volt3x/SetClock 全链反编译结论（A4 §3.4/§4.1/§7）。

## 5. 数据资产（GB202 直接证据）

| 资产 | 内容 | 对审计的价值 |
|---|---|---|
| `jse_dec/cvb_ist_gb202_1.yme.dec`、`cvb_ist_gb202_2.yme.dec` | **GB202 CVB 电压表**（YAML）：`id: 6, usage: slt`；`voltage_setting: {atpg, cas} × {baseline, offset} × {msvdd, nvvdd} × C0..C5`（本文件 baseline C0=0.6、余 0） | GB202 电压模型系数结构真值；注意 `usage: slt`（SLT/ATPG 档，非产品 curve 档——解读时看 `usage` 字段分档） |
| `jse_dec/ovrmcalibration.jse.dec` | AVFS/超频校准流程（Blackwell 独有脚本） | 引擎侧校准顺序参照 |
| `jse_dec/pstate.jse.dec`（5050）与 `jse_dec_4060/pstate.jse.dec` | P-state 表构建/切换流程 | 与审计 T2-5 V/F points 判别互补 |
| `jse_dec_4060/gshmoo.jse.dec` | V/F shmoo 扫描方法（扫频找边） | 服务器卡钳制边界测绘的方法学 |
| `jse_dec_4060/boarddb.spe.dec`、`boards.jse.dec` | 板级定义/板库 | 服务器 SKU 板级项对照 |
| `jse_dec_4060/tunevolt.jse.dec` | 电压调优全流程（IDDq/Speedo 判据 + 压测验证 + patch 文件） | §2/§3 的完整版参照 |

## 6. 解密方法与产物位置（供活机复现/补全）

**算法**（5050 由 IDA 证据链破解；4060 由密钥扫描独立复现，双侧互相印证）：
```
容器: [0..3) magic F1 1A 80 ｜ [3..11) u64 LE 解压后大小 ｜ [11..27) 16B CTR IV ｜ [27..) 密文
fileKey = keyblob[0:16] XOR AES-128-ECB(key=keyblob[16:32], blk=keyblob[32:48])
plain   = zlib.decompress( AES-128-CTR(fileKey, IV).decrypt(body) )    # BE128 计数器
```
- 5050(570.278) keyblob @VA `0x9A7C660` → fileKey `63e4a2085186311e6e6175d15ca0b079`
- 4060(520.220.2) keyblob @VA `0x543B800`（FO 0x503B800）→ fileKey `ba090e3eb883c260f7b5be8a2300edee`（全 RX 段纯 Python 扫描、zlib 全解压唯一验证命中）
- 变体：`F1 1A 81`=加密日志、`F1 1A 82`=需预处理变体（vkstress.jse 属此类，待解）、`2B D4 FF`=免解压变体

**产物（本机 `reverse/mods_analysis/`，不入库）**：`jse_dec_4060/`（77 文件）、`jse_dec/`（24 文件）、`verify_4060_key.py`/`find_key_4060{,b}.py`/`jse_verify2.py`、`a3_work/mods4060.i64` + `a3_work/dump/*.c`（34 个反编译）、分报告 `A1_driver.md`（内核 ioctl 全清单）/`A2_specs.md`（.spc 明文规格：guardband -8.88%/-25mV、MinVoltuV 650000、NVVDD OVR I2C 解锁序列）/`A3_mods4060.md`/`A4_mods5050.md`/`A5_tools.md`（nvflash BIT 表、mats、nvpex）/`REPORT.md`（总报告）。未解残留：`mods.he`/`mods.jse`（解密时被 idalib worker 锁定，重跑即得）、`vkstress.jse`（F1 1A 82）。

## 7. 边界（MODS 帮不上的）

1. **服务器 SKU 策略钳制**（450W max=default、VfOffset 范围 [0,0]、TopRels 0.9 devinit 墙）在 VBIOS/SKU 策略层——MODS 解释机制，不提供解锁。
2. `/dev/mods` 直写通道与运行中的 580.173.02 驱动**互斥**（Linux PCI 单绑定 + GSP 所有权），在 PRO 6000 活机不可用作旁路；它的用途是 ground-truth oracle（多卡分治时作对照）。
3. MODS 5050 = GB207 口径；GB202 特有行为仍以活机审计 + GSP 固件逆向为准（同 arch 0x1B、同 CLK_DOMAIN_50/Volt3x 代际，机制可平移，数值不可）。
