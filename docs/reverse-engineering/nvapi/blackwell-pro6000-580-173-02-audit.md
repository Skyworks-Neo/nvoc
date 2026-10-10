# Blackwell RTX PRO 6000（GB202）× Linux 580.173.02 全量逆向补全审计

**日期**：2026-10-11（真机首测）
**硬件**：2× NVIDIA RTX PRO 6000 Blackwell Server Edition，PCI `10DE:2BB5`（subsystem `10DE:204E`），VBIOS 98.02.8D.00.01，96 GB GDDR7，PCIe Gen5（本机 host Gen3）
**驱动**：580.173.02 Linux **open kernel module**（DKMS 全符号构建，`r582_69` 分支），GSP 架构；CUDA 13.0
**目标**：本机首次拥有 Blackwell 硬件。此前分支全部 50 系知识来自 NvpwrControl 1.8.0 静态逆向（Windows 616.92）与 Linux 610.57.04 覆盖审计；本文记录 580.173.02 + GB202 真机上的全量核对结果，按 **tier 分级列出对 nvoc 仓库的补全改动清单**，供逐条核对。全程只读，无任何 GPU 写入。

**方法**：4 个 idalib MCP 会话（`nv-kmd-580`=nvidia.ko 18574 全命名函数、`nv-api-580`=libnvidia-api.so、`nv-ml-580`=libnvidia-ml.so、`nv-modeset-580`）× 13 探测维度多代理并行 + 逐 finding 对抗验证 + completeness critic + 定向补漏。开源地面真值：`reverse/open-gpu-kernel-modules` tag **580.173.02**（与二进制 1:1 核对通过，无漂移）。原始 probe 输出归档于 `reverse/linux-580.173.02/probe/`（不入库）。

**5 行总结**
1. **身份链全解**：`0x2BB5` → PMC_BOOT_42 `{arch 0x1B, impl 0x2}` → chipId 0x1B2 → `HAL_IMPL_GB202`(61)；SKU 名在 `sChipsReleased[]`（6793×16B，符号级定位），**0x2BB5 名下 102 条里混着两条 GeForce 陷阱行**——repo 必须按 device+subsystem 精确匹配。repo 现状：无 PCI 表但名称+codename 识别路径可归 `WorkstationBlackwell`（真缺口在 Linux `GetShortName` 是否返回 "GB202" 式 codename，见集成节 I-2）。
2. **裁决 nvpwrcontrol-audit §4.3 之争**：0x527FC458 的 +4 在 NVAPI 结构层是 **index 语义**——Linux shim（libnvidia-api.so）内部把 index 翻译成 RM bitmask（反编译 0x2bf00 + 0xaed0 全 switch + 逆映射 0xac20）；工具传 0x2 实测的是 SYS（读法 A），"mask 语义"之说不成立。
3. **裁决 §4.1 0x0F 锚点**：wire 域子型为 1..16，本构建 **0x0F 合法但无值**，实际扩展子型是 **0x0B 与 0x10**；freq@rec+0x114 / MSVDD@rec+0x11C 锚点只对 type-{2,4..10,0x0B} 成立，**type 0x10 窗口 +8B**；且 Linux shim 写入 rec+0 的是**映射后域号 0..15**（非 Windows 的原始 wire 子型字节）——naive marker 扫描在 Linux 上会扑空。
4. **OC 私有面 8 个未注册 handler 全部解码**（此前 critic 列为最大缺口）：含一个**未绑定的单槽 VF 写路径** `0x28DCA3F0`（GET 0x208090A2 / SET 0x2080D0A3）、`0xFB8F61EC` 的 RM cmd 归属纠正（0x20809018，非 nvapi-rs 注释里的 0x20809006——那属于 0x527FC458）。
5. **GSP-only 结构 + Windows kmd 路线 Linux 终审**：CPU 侧 nvidia.ko 无任何 clk/volt/VFT 对象模型，`RMCFG_MODULE_PMGR 0`——Windows 功率策略对象模型（PowerRoot/BoardObj）在 Linux **不存在**，全部功率控制 GSP 常驻（本地仅特权门+RPC 桩）；等价构建器在本机 `/lib/firmware/nvidia/580.173.02/gsp_ga10x.bin`（75MB **未剥符号** RISC-V ELF，后续逆向直接目标）。Linux 表面 = libnvidia-api shim（547 条 QI，610.57.04 为 590）+ NVML 404 导出（470:255 / 610:417）。

---

## Tier 1 — 新卡识别（代码可直接落）

| # | 发现 | 证据 | 建议改动 |
|---|---|---|---|
| T1-1 | 身份四元组：`device 0x2BB5 + subsystem vendor 0x10DE + subsystem 0x204E 或 0x220B` → "NVIDIA RTX PRO 6000 Blackwell Server Edition" | `sChipsReleased[]` 表项 @IDA `0xAF19D0`/`0xAF19E0`，字符串 @`0xADA E30`；`g_nv_name_released.h:5438-5439`；消费入口 `rm_get_device_name` @`0x1444C0`（6793 条，exact-subsystem 优先、`{0,0}` 表项 device-only 回退） | repo 增加 Blackwell 识别常量（device+subsystem 二元组，勿用裸 device ID） |
| T1-2 | **命名陷阱**：`0x2BB5` 名下共 102 条名称表项 = 2 条 bare-metal Server Edition + 98 条 vGPU `DC-*` + **2 条 GeForce 陷阱**（subsystem `0x21A6`→"GeForce RTX 3050"、`0x21A7`→"GeForce RTX 3060"，`g_nv_name_released.h:6732-6733`） | strings-sweep 修正 + ogkm 源码核对 | 任何"按 device ID 查名"的逻辑必须带 subsystem 精确匹配 |
| T1-3 | 芯片识别链：`PMC_BOOT_0 → halmgrGetHalForGpu_IMPL @0x177010 → chipID[] @0xB02280 (81×12B) → HAL_IMPL_GB202 = 61 → halIface_GB202 @0xB01D00`；chipId = `(0x1B<<4)\|0x2 = 0x1B2`（`gpuInitChipInfo_IMPL @0x217520`） | 二进制字节 + `g_hal_archimpl.h:91` + `gpu.c:5922-5950` + `nv_ref.h:183` | 若 repo 需要 chip 判别：GB202 = arch `0x1B` impl `0x2`；RM 控制面编码 `NV2080_CTRL_MC_ARCH_INFO_IMPLEMENTATION_GB202 = 0x2`（`ctrl2080mc.h:141`） |
| T1-4 | 内核 PCI 探测表**无 per-device 白名单**（`PCI_ANY_ID` + class 0x0300/0x0302/0x0680），识别完全在 RM 内部 | `nv-pci-table.c:39-92` | 佐证 T1-1：repo 侧同样不能指望"驱动层有表"，要自带 |
| T1-5 | 同代工作站家族（同一套识别机制）：`2BB1`(+/204B 四厂商块)、`2BB3`、`2BB4 Max-Q`、`2BB5`、`2BB9`、`2C02`；GB20x 全谱：GB202/203/205/206/207/20B/20C（arch 0x1B）+ GB100/102/10B/110/112（0x1A） | IDA 表段 `0xAF18D0-0xAF1A40` 逐项解码 + `g_hal_archimpl.h:87-98` | 识别表一次建全，不必逐卡补 |
| T1-6 | **repo 现状：无 PCI ID 表，但有名称+codename 识别路径**——全仓 grep `0x2BB5` 字面量无命中；`detect_gpu_type`（`core/src/gpu_type.rs:120`）按 name+codename 判定，本卡可归 `WorkstationBlackwell`（详见集成节 I-1/I-2，代理维度的"零识别"结论已修正） | followup + chip-tables grep + 主循环人工核对 | 真缺口在 codename 来源（I-2）而非 ID 表 |

## Tier 2 — 布局/版本漂移（相对 610.74/616.92 文档，改 probe 参数可用）

| # | 发现 | 证据 | 与既有文档的差异 |
|---|---|---|---|
| T2-1 | **XBAR 测量语义裁决（§4.3 收官）**：NVAPI `0x527FC458` 结构 `+4` 是 **index**；shim `sub_AED0 @0xaed0` 全 switch 把 index → RM bitmask（可测集 `{0..18, 20..25, 31}`，19 与 26-30 拒绝），INFO 侧逆映射 `sub_AC20 @0xac20` 把 mask → index | `nv-api-580` 0x2bf00/0xaed0/0xac20 反编译 | nvpwrcontrol-audit §4.3 两种读法**和解**：结构层 index 语义（我们 40 系记录对），工具传 0x2 = 测 SYS（工具的 "physical XBAR" 标注错） |
| T2-2 | **ClockDomains V2 几何确认**：magic `0x261A4`（V2）/`0x10964`（V1）在 580 shim 双双在位；stride 772B 重推导精确吻合（`v13 += 193` dwords）；GET @`0x2c6e0`、SET 镜像 @`0x2ca70` | 反编译常量 `156068=0x261A4`、`67940=0x10964` | 与 clk_ctrl_entry_v2::STRIDE=772 一致，无漂移 |
| T2-3 | **0x0F 锚点精确化（§4.1 收官）**：wire 子型 1..16；本构建 case 表 `10→7, 15→14, 16→15`（`0x9d10`）；**0x0B 与 0x10 是本代实际扩展子型**（0x0F 合法但无值）；freq/MSVDD 锚点（+0x114/+0x11C）仅对 type-{2,4..10,0x0B} 成立，**type 0x10 整体 +8B**（`v13[69..76]` 窗口） | GET/SET 反编译分支逐 dword 对照 | nvpwrcontrol §4.1 的 `BW_TYPE=0x0F` 常量**不可直接用**；按本文锚点修正 nvapi-rs clock.rs |
| T2-4 | **Linux rec+0 陷阱**：shim 写入 ClockDomains 记录 +0 的是**映射后域号（0..15）**，不是 Windows 观测的原始 wire 子型（Ada `{0x0A×7, 0x02}` / BW marker `0x0F`） | `0x2c6e0: sub_9D10(v19, *v18)` | Linux 上 marker-scan 策略必须先过映射表（`0x9d10` case 表） |
| T2-5 | **V/F points 新版本**：GET_CONTROL 同时接受 `0x12420` 与 **`0x22420`**（`(*a2-74784)&0xFFFEFFFF`）；wire entry 判别 `{0,2,4}`=空 vs `{1,3,5,6,7}`=有值；INFO 仅 `0x1182C` | `0x38830`/`0x37fa0`/helper `0x29b50` | nvpwrcontrol §4.5 的 Blackwell magic 得到双源确认 + 新增 v2 |
| T2-6 | **ADC/voltRail**：INFO `0x209F0` 在位（另有旧布局 `0x10348`）；STATUS（验证轮补全）接受 `0x10340`（旧）+ `0x109C8`（先前文档猜测值**确认**），RM cmd `0x208090xx` 族 | `0x32a00`、STATUS `0x33d00` | 616.92 magic 与 580.173.02 一致——ADC 面跨代稳定 |
| T2-7 | **QI 表规模漂移**：580.173.02 = **547 条**（base `0xAE280`，16B/条，终止符 idx 547），610.57.04 = 590（少 43）；3 条 Linux-only IID 与 610 完全相同（`0x4E0B05ED`、`0x7B9681DA`、`0xBF75A81E`） | 全表脚本解码 + 逐 handler 反编译 | linux-version-coverage-audit 的 590 基线需按版本分叉记录 |
| T2-8 | **PowerMonitor Linux 版本门**：QI `0x2AD3DBAB` @`0x751D0` 仅 version `0x50188` 触发 RM GET（cmd `0x2080A036`，先前的 `0x20802036` 为笔误）；V4 `0x40188` → **-9**；>0x50188 → 0 空操作（向前兼容直通） | 反编译 + 表项 idx321 | unregistered-handler-triage 的 Windows 结论在 Linux 收紧为单版本门 |
| T2-9 | **0xFB8F61EC RM cmd 归属纠正**：580 Linux 上该 handler（@`0x2c010`）用 `0x20809018`（magics 0x10020/0x20020/0x30178）；`0x20809006` 属于 `0x527FC458`（@`0x2bf79`）——nvapi-rs `sys/src/gpu/clock.rs:1975` 注释张冠李戴 | 反编译 + 59 处 0x2080xxxx 立即数全扫 | 修 nvapi-rs 注释/归属 |
| T2-10 | **NVML VfOffset 通道漂移**：`nvmlDeviceSetGpcClkVfOffset` → RM `0x2080D031`（610 文档记录 `0x2080F031`；params 0x608 不变） | `nv-ml-580` sub_1053B0 @`0x105669` 字节 `b9 31 d0 80 20` | kmd/文档中 0x2080F031→按驱动版本分叉 |
| T2-11 | NDA 对 `0x4E0B05ED`/`0x7B9681DA`：selector `0xC3`(195)/`0xBF`，struct 880B/12B——与 610 的 `0xC4/0xC0`、888B 已漂移 | `0x574C0`/`0x12B40` 反编译 | 若 repo 未来绑定 NDA 面，版本门按 580 记 |
| T2-12 | TGP GET/SET 在表头 idx21/22 与 610 同位；version block 比文档 610 记录落后一代（case `0x12720/0x11F10/0x10A4C/0x106DC/0x10298`） | `0x48FF0` 反编译 | TGP-watt 探针的 magic 级联需按 580 收敛 |
| T2-13 | `0x991E343D` = GET `0x2080901C`（SET `0x2080D01C` 的读孪生）：Windows triage-2 把它标为非 OC 是**误标**；32 项循环，src stride 24B、dst stride 548B（137 dwords） | `0x2cdb0` 反编译 | 读通道白捡，无需写权限即可枚举当前 V/F 控制 |

## Tier 3 — 新能力/新表面（版本信息、新域、新遥测）

| # | 发现 | 证据 |
|---|---|---|
| T3-1 | **8 个私有 clock/VF handler 全解码**（critic 缺口清零）：`0xFB8F61EC`、`0x991E343D`、`0x0C1EF2CA`(GET 0x208090A2，v1 10×68B / v2 32×68B)、`0x28DCA3F0`(GET 0x208090A2 + **SET 0x2080D0A3**)、`0x2BC9F805`/`0xAFA4113C`(NAFLL 对，后者收 magic 0x652C8/0x752C8)、`0x21537AD4`(GetVFPCurve 复合：0x20809029+0x20188+0x2080902B → v3 0x35B0C)、`0xC4733F19`(锁定时钟状态，transport selector 0xB8，4 flag 位)；全部 IID 在 Win610.74 QI 表同样在位 | followup 维度逐个反编译 + r610-74 表交叉 |
| T3-2 | 上述私有 RM cmd（0x20809018/1C/2A/2B/90A2/D0A3/90B0/90B1…59 处）**不在开源 SDK 头文件**、也不在 nvidia.ko 立即数中——纯 GSP-forwarded 内部控制 | ogkm 全树 grep 0 命中 + nvidia.ko find immediate 0 命中 |
| T3-3 | **wrapper-gap ID 在 Linux 的存在性裁决**：在位 = thermal-sim(idx321)、core-voltage(idx332)、PMGR-arbiter(idx339)、PowerMonitor V4/V5(idx379)；**缺席 = SetPowerMizerInfo `0x50016C78`、PCF DynamicBoostGet `0xC80068A1`、`0xAD298D3F`、Windows seeds `0x33C7358C`/`0x593E8644`** | 全文件字节扫 + 表项反编译 | wrapper-gap-audit 的 9 项中 Linux 可_actionable_ 面大幅收缩 |
| T3-4 | nvapi-rs ID 覆盖率：注册 1210 个 ID 中 286 个可在 547 表解析 | qi_nvidrs 脚本 | 绑定面按 547 收敛 |
| T3-5 | NVML 404 导出（470:255 / 610:417 之间）；nvml-wrapper-sys 0.10.0 绑 396/404，8 个未绑（Get/SetHostname_v1、GetRemappedRows_v2、GetUnrepairableMemoryFlag_v1、ReadPRMCounters_v1、GetBlacklistDeviceCount 等） | nm -D 全量 + bindings.rs 比对 |
| T3-6 | **RUSD（class 0x00DE RM_USER_SHARED_DATA）**：整页只读电源/时钟遥测（throttle 位、RUSD_CLK_PUBLIC_DOMAIN_*、RUSD_POWER_LIMIT_INFO、avg/inst power、policy status tgpmW、HBM temp type）——无需 ctrl 调用 | `cl00de.h:92-416` | repo 只读遥测可走此路，绕开权限面 |
| T3-7 | **Blackwell 新动态功率控制族（开源 SDK 命名！）**：`CONFIGURE_TGP_MODE 0x20800AD2`、`CONFIGURE_TURBO_V2 0x20800AD3`、`SET_VPSTATE 0x20800AD6`、`UPDATE_EDPP_LIMIT 0x20800AD0` / `GET_EDPP_LIMIT_INFO 0x20800AFD`（dispatch 在 `g_subdevice_nvoc.c` 在册） | `ctrl2080internal.h:3272-3602` | 命名控制 = 可直接绑定的合法面（读侧先行） |
| T3-8 | RATED_TDP 仲裁模型在树内完整文档化：clients {RM, WAR_BUG, GLOBAL, OS, PROFILE} × actions {DEFAULT, FORCE_EXCEED, FORCE_LIMIT, FORCE_LOCK, FORCE_FLOOR}；权限门：MIG 需 admin、profiling TurboBoost 需 PERFMON | `ctrl2080perf.h:208-310`、`kern_perf_pwr.c:79-91` |
| T3-9 | 新遥测导出：`nvmlDeviceGetMarginTemperature` → 0x2080852B；`nvmlDevicePowerSmoothingSetState` → 0x2080E63B（PMGR 特权）；`GetC2cModeInfoV` → 0x2080182B + C2C LPWR 对 0x20801831/32；fabric 已 V3（0x208001A3）；DramEncryptionMode 对 → `NV2080_CTRL_CMD_FB_QUERY_DRAM_ENCRYPTION_STATUS 0x20801358`（SDK 命名确认） | nvml-exports 逐个 objdump |
| T3-10 | **电压/轨测量存在但无公开导出**：服务端实现 @`nv-ml-580` sub_A90D0（RM 0x2080B201/B202），导出面 0 命中 rail/volt；nvidia-smi 无 voltage 查询字段；新出现 **GPMVDD/GPCVDD 轨电压监控故障串**（`0xb094e0` 等） | objdump + strings | 电压读面 = 私有 QI handler 路线（0xDC2BD4A6 在位，idx332） |
| T3-11 | 真机读数基线：power default=max=enforced **450W**、min 300W（NVML 无用户 headroom）；时钟 max Graphics/SM 2430 / Mem 12481(GDDR7) / Video 2107；Graphics 阶梯 ~323 步 @~7.5MHz；温度面 GPU-only（mem N/A）；新查询节：GPU Memory Power / Module Power / Power Smoothing / Workload Power Profiles（本机全 N/A）；dmon 新列 jpg/ofa | `probe/smi-*.txt` |
| T3-12 | 测量宇宙（shim 层）：可测 index `{0..18, 20..25, 31}`——RTSS 序非连续，与 CLI 位表（msd=21、pwr=20、utils=22、host1x=28）**冲突待活体核对**（见 Tier 4 任务书） | `0xaed0` switch 全解码 |
| T3-13 | 版本指纹：nvidia.ko 580.173.02（DKMS vermagic 6.8.0-41-generic）、NVML 内部 `13.580.173.02`、QI idx3 handler 内嵌分支串 **`r582_69`**、GSP 固件文件族 `gsp_ga10x.bin`（Blackwell 落入 ga10x 镜像族，GB202 专属 ucode 以 BinArchive 内嵌于 nvidia.ko，kgspBinArchive GB202 FMC 0x81000B） | strings/modinfo/nm -S |
| T3-14 | ioctl 面与 610.57.04 完全一致（RM_CONTROL 0x2A/32B、ALLOC 0x2B/48B、XFER 0xD3、INTR 0xD5；原始常量 `0xC020462A` 等已解码）；RM 版本检查协议 STRICT/RELAXED/QUERY（不匹配时回写 "580.173.02" 并返 0xFFFF）；客户端开关 `__RM_NO_VERSION_CHECK`/`__RM_ENABLE_VERBOSE_OUTPUT` | nv_escape.h + `0x145bd0`/`0x8B280` 反编译 |

## Tier 4 — 存疑 / 需写权限 / 未覆盖（本轮不做，附后续协议）

| # | 事项 | 现状 | 后续协议 |
|---|---|---|---|
| T4-1 | VF **SET** 侧线格式：`0x38b80`(0x0733E009) 已定位（RM 0x2080902A@0x38d89 + **0x2080D02D**@0x38ee3）但未逐字段反编译 | GET 镜像推断 | 需要时按 GET 镜像 + nvpwrcontrol fail-closed 三重门先静态终审 |
| T4-2 | **`0x28DCA3F0` 未绑定单槽 VF 写路径**（GET 0x208090A2 / SET 0x2080D0A3）：repo 无绑定、开源头无定义 | 已解码结构，未活体验证 | 读侧先探 GET 响应性；写侧需单独评审（对照 kmd-power-policy 安全规范） |
| T4-3 | GSP-RM 固件未分析——真正的 VFT/VfPoint/ClkDomain/VoltRail/BoardObj 构建器在其中 | **目标已具体化**：`/lib/firmware/nvidia/580.173.02/gsp_ga10x.bin` = 75MB 未剥符号 RISC-V ELF（K-6），idb_open 直接可用（架构选 RISC-V） | 单独立项：GSP 固件逆向（PowerRoot 等价物、时钟表构建器、PMGR FINN 全貌） |
| T4-4 | 测量 index 物理归属活体定名（0..18/20..25/31 全枚举 + 负载态 GPC 比率自检） | 静态映射表已有 | 跑 `blackwell_recon_live`（只读）+ TopRels GPC:XBAR 比率 0.9 自检（§4.3 协议，结构层已裁决为 index） |
| T4-5 | Module/GPU-Memory Power、Power Smoothing、Workload Power Profiles 真实性：本机全 N/A——是 SKU 无传感器还是需特权/激活 | live-smi | 对比 DC 型号或开启 MIG/fabric 后复测 |
| T4-6 | ACPI-GPS PRH（thermal limit/EDPpeak/TGP-turbo 同步）与 C2C Lpwr 行为：SBIOS 相关，本机未触发 | strings+函数在位 | 需平台事件触发，只读观察即可 |
| T4-7 | RMCTRL per-command 特权/路由 flags 未全量解码（决定哪些文档化功率控制在 open kmod 上可用户态直发） | ogkm 生成表部分解码 | critic 遗留：dump `g_subdevice_nvoc.c` flags 字段全量 |
| T4-8 | vGPU 面（0x55A00000 heap、100 条 DC-* chip-flags、48-VM 分区表）：非 vGPU 主机不可验证 | 静态记录 | tier 4 挂起 |

---

## Windows kmd 功率策略路线在 Linux 的终审（power-policy 维度，高价值否定结论）

| # | 发现 | 证据 |
|---|---|---|
| K-1 | **OBJPMGR/BoardObj/PowerRoot 对象层在 Linux nvidia.ko 中不存在**：函数名 regex 全扫 0 命中（仅 nvswitch 的 pmgr 初始化）；`rmconfig.h:759` `#define RMCFG_MODULE_PMGR 0`——开源构建直接关闭了 PMGR 引擎模块，只编译了 ctrl 桩 | nv-kmd-580 符号全扫 + `srcs.mk:603/852/299` |
| K-2 | Windows 功率策略 RM 命令 `0x2080A61A`/`0x2080E61B` 解码为 PMGR FINN 接口，开源头文件截断（`ctrl2080pmgr.h:53,66` 只有框架）；编译出的唯一 PMGR dispatch 项 **pFunc=NULL**（@`nv-kmd-580 0xa777f0`，methodId 在册但无处理函数）。验证轮补强：PMGR 并非全死——`NV2080_CTRL_CMD_PMGR_GET_MODULE_INFO 0x20802609` 被 `_gpuDetectNvswitchSupport` **活跃下发**（`mov ecx,20802609h` @`0x214ddf`） | get_bytes + g_subdevice_nvoc.c:7485-7509 + 验证轮 0x214ddf 复核（probe 十进制换算有误已纠正：545302042=0x2080A61A、545318427=0x2080E61B） |
| K-3 | **全部 NV2080 功率控制 GSP 常驻**：本机路径 = `rpcRmApiControl_GSP`（@`0x38e850`）；本地 handler 一律是特权门 + RPC 转发桩（`kern_perf_pwr.c:55-105`，SMC 下非特权客户端拒时钟控制） | 反编译 + 源码行核对 |
| K-4 | **kmd-power-policy-layout-r610-74.md 的偏移链在 Linux 上完全不适用**：无 DriverGlobal 等价物、无 per-GPU 静态池/表全局（globals 扫 54 命中全是字符串/params/fops） | globals 实体查询 |
| K-5 | 内核侧真正存在的功率状态物：OBJGPUBOOSTMGR（49 函数，`0x434c0-0x348970`）、SyncGpuBoost、GC6/RTD3（`kern_gpu_power.c:251-288`）；本地（非 GSP）客户端资源功率控制 `cliresCtrlCmdSyncGpuBoost{Info,GroupCreate,GroupDestroy,GroupInfo}_IMPL` @`0x3520e0-0x3522f0` | entity_query + 验证轮补全 |
| K-6 | **GSP-RM 固件即"新 kmd"**：`/lib/firmware/nvidia/580.173.02/gsp_ga10x.bin` = **75MB 未剥符号 RISC-V ELF**（Blackwell 落 ga10x 镜像族），BoardObj/PowerRoot/VFT 构建器全在其中——后续逆向的直接目标（idb_open 即用，无需解包 BinArchive） | file 探测；注意 `/lib/firmware/nvidia/gb202/gsp/` 是旧 570.144 残留，非本驱动加载物 |

**结论**：Windows 上的"内核写功率顶"配方（Nvpwr/sys 门禁 + RVA 签名 + PowerRoot 字段写）在 Linux 开源驱动栈上**没有对应物**——等价物在 GSP-RM 固件里。kmd-power-policy-layout-r610-74.md 应加"Linux: N/A（GSP-only）"横向批注；Linux 侧的用户态可达功率面 = T3-7 的命名控制族（RATED_TDP/EDPP/TGP-mode，带特权门）。

## 活体实验记录（2026-10-11，`blackwell_recon_live` 首跑 + CLI 采样）

T4-4 任务在真机执行（只读），文档等待已久的三个 50 系悬案一次收官：

**1. §4.3 测量语义裁决 — index 语义活体钉死**
直测 0x527FC458 扫表（+4=0..28）+ TopRels 比率自检：

| +4 | 读数 | 归属 | 自检 |
|---|---|---|---|
| 0 | 2432.801 MHz | GPC | =Graphics 时钟 ✓ |
| 1 | 2180.953 MHz | **XBAR** | **0.8965×GPC ≈ TopRels 比率 0.8999 ✓✓** |
| 2 | 2212.428 MHz | SYS（GPC 派生） | 0.9094×GPC |
| 4 | 12479.384 MHz | MCLK | =Memory ✓ |
| 20 | 1991.483 MHz | **uProc**（见下） | 0.8186×GPC |
| 21 | 2107.001 MHz | **Msd/Vid** | =video max 2107 分毫不差 |
| 22 | 107.936 MHz | Utils | 定频 108，与 Ada 一致 |
| 3,5-18,23-25 | -104 | 本代无测量槽 | |
| 19,26-28 | -5 | 非法 | |

XBAR 自检通过 = **index 语义 + 域归因同时活体确认**（与 shim 反编译裁决 T2-1 双源一致）。NvpwrControl 工具传 +4=0x2 实测的是 SYS，其 "physical XBAR" 展示错域——结案。

**2. §4.1 V2 记录子型 — 活体读数与 shim 静态解三源互锁**
ClockDomains V2 GetControl（magic 0x261A4）：controllable mask 0xFF，**dom 0-7 全部 type=0x0F**。这个 0x0F 不是 NvpwrControl 字面的 wire 值，而是 shim 映射的输出：wire 子型 0x10 → `sub_9D10` case 16→15(0x0F) 写入 rec+0（若 wire 真是 0x0F=15，映射后应为 0x0E≠观测）——静态 T2-3/T2-4 与活体三源互锁。BW 锚点 freq@+0x114 / MSVDD@+0x11C 结构上可读但**值全零**（空闲态无 populate）→ "唯一活跃条目"启发式正确返回 None 不误触发。

**3. TopRels 比率记录 — 首次活体命中**
GetControl seed A（+4=0xFF 工具式种子）status=0，比率 **raw=0xE660 (0.8999) @rec+0x68**——tag3+0xE660 是 Blackwell 特有的推断（nvpwrcontrol-audit §9.1 在 Ada 上观察到 tag5/payload0）获活体证实；本机 GPC:XBAR = 0.9。§9.1 的"Ada 边是 enum=2 无比率"对照成立。

**4. 域 21（Msd）显示问题 — 用户悬案定论**
"50 系 MSD 显示一直是错"的活体终审：直测 +4=21 单次读 **2107.001 MHz = video 时钟 max 分毫不差**；同时段 GetAllClocks[21] 空闲漂浮 41-211 MHz（媒体子系统空闲门控，真值非错读）。**域 21 = video/media 时钟在 50 系活体确认**——xOCD 的 "Video/NVD" 归因正确，2026-10-05 "不权威"裁决翻案；`Msd/Vid` 双名标签保留。深度空闲读数漂浮是门控现象，不是显示 bug。

**5. uProc 数值确认**
+4=20 直测 1991.483 MHz（0.8186×GPC），与 All Clocks 面板 1990.6-1991.5 一致——域 20 在 GB202 上为核心邻近可变时钟（Ada 上为定频 540），uProc（微处理器/GSP 类）释义成立。

**遗留**：视频负载确认（需 NVENC 客户端，本机无 ffmpeg/torch）——装齐后跑一次 encode，预期 Msd/Vid 跳至 2107 档且 VideoEngine>0；ECC/负载下 bit 32 归因实验同批。

**补充实锤（同日，ffmpeg + h264_nvenc 4K120 负载，`pro6000_msd_load_live` 双接口采样器）**：
- NVENC 负载下 **GetAllClocks[21] 从空闲的 41-211 MHz 爬升到 2015-2066 MHz**，与直测 direct(21) 的 2093-2103 MHz 同步跟踪（恒差 ~3% = 有效频率 vs 瞬时频率的采样差）——**两接口读的是同一个 video 时钟，GetAllClocks 的 Msd 槽没有错**；此前的"明显不符"是空闲门控值 vs 单次 max 采样的错位。
- Msd 归属、双标签、空闲漂浮解释三项全部获得负载态交叉证实；域 21 案卷彻底关闭。
- 顺带发现并修复：Utils(22) 直测可测（定频 107.9-108.0）但 GetAllClocks 槽恒零——pynvoc `normalize_status` 现对 FCLK 域集中 GetAllClocks 缺席的域做 `QueryNvapiClkDomainFreqsBatch` 回退直测，TUI FCLK 行从 5 域恢复到 6 域（+UTILS）。

## 文档裁决记录（对既有文档的修订指令）

1. `nvpwrcontrol-blackwell-tuner-audit.md` §4.3：裁决为**读法 A**（index 语义）；工具的 "XBAR physical"= SYS；§9.3 裁决协议可关闭。
2. 同文 §4.1：`BW_TYPE=0x0F` 改为 `{0x0B, 0x10}`（本构建），锚点 `+0x114/+0x11C` 限定 type-{2,4..10,0x0B}，type 0x10 +8B；Linux rec+0 为映射域号。
3. 同文 §3 / `wrapper-gap-audit.md`：Linux 580.173.02 存在性按本文 T3-3 收缩；PowerMonitor cmd 笔误 `0x20802036`→`0x2080A036`，版本门 `0x50188`-only。
4. `unregistered-handler-triage-2.md`：`0x991E343D` 误标翻案（= SET 0x2080D01C 的读孪生，OC 相关）。
5. `nvapi-rs sys/src/gpu/clock.rs:1975`：0xFB8F61EC ↔ RM cmd 归属纠正（0x20809018；0x20809006 属 0x527FC458）。
6. `linux-version-coverage-audit.md`：QI 基线按版本分叉（610.57.04=590 / 580.173.02=547）；NVML 基线 255/404/417 三点。
7. （活体补强 2026-10-11）§4.3 从"结构层裁决"升级为"活体双源裁决"：XBAR 比率自检 0.8965≈0.8999 通过，index 语义+域归因同时钉死，§9.3 协议正式关闭。
8. （活体补强 2026-10-11）域 21 = video/media 时钟在 50 系活体确认（+4=21 读 2107.001 = video max 分毫不差）；2026-10-05 "xOCD Video 标注不权威"的裁决翻案；`Msd/Vid` 双名保留，深度空闲漂浮值 = 媒体子系统门控真值。
9. （活体补强 2026-10-11）§4.1 V2 记录：GB202 真机 dom 0-7 的 rec+0 全部 type=0x0F——这**正是 T2-3/T2-4 shim 映射的输出**（wire 子型 0x10 → `sub_9D10` case 16→15 = 0x0F 写入 rec+0；若 wire 是 NvpwrControl 字面的 0x0F=15，映射后应是 0x0E）。三源互锁：NvpwrControl 的 0x0F marker（Windows 写 wire 原值或同映）+ shim 16→15 case + 真机 rec+0=0x0F。BW 锚点结构可读但空闲态值全零。

## repo 集成点（主循环人工核对，修正了代理维度的"零识别"误报）

| # | 集成点 | 现状 | 对本卡的判定 |
|---|---|---|---|
| I-1 | `core/src/gpu_type.rs:120` `detect_gpu_type`：**名称+codename** 判定（非 PCI ID） | 本卡 name="NVIDIA RTX PRO 6000 Blackwell Server Edition" 含 "RTX"+"6000" 无 "GeForge"→`is_rtx_professional`；codename `GB202` 前缀 GB → **`WorkstationBlackwell`**（"Server Edition" 字样不在 `is_server` 关键词表——当前行为可能是有意的：该 SKU 是工作站级核心） | 分类可用，无需新设备 ID 表（代理维度 T1-6 "repo 零识别"的修正：repo 无 PCI 表是设计使然） |
| I-2 | **codename 来源风险**：`GpuInfo.codename = gpu.short_name()`（`nvapi-rs/src/hi/gpu.rs:247`）→ `NvAPI_GPU_GetShortName`（QI `0xD988F0F3`，580 表 idx131 @`0xc020`） | handler 是 transport selector 0x21 调用，静态无法确定 Linux 返回的是 "GB202" 式短名还是全名/失败。若失败 → codename 空 → `detect_gpu_type` 落 **Unknown**，全部 Blackwell 门失效 | **最高优先活体测试**：跑 repo 现有查询确认 classification |
| I-3 | `GpuType::is_blackwell()`（`gpu_type.rs:636`）：ClkDomains WRITE 槽位 50 系平移（slot2=频率 kHz、slot3=µV 电压偏移）、私有 V/F +0x64=带符号 µV | 注释明言"服务器 Blackwell 未实测，按同代口径归入"——本文 T2-3/T2-5 即该口径的 Linux 佐证任务 | WorkstationBlackwell 命中 → `cli/src/lib.rs:5555/5704` 的 slot 2/3 映射自动生效 |
| I-4 | VFP ext 槽解码器（`gpu_type.rs:1043/1160`"Blackwell today ext slots undecoded → 拒写"） | fail-closed 已就位 | 本文 T2-5 的 wire 子型判别（{0,2,4}空 vs {1,3,5,6,7}有值）是解码器补全的直接输入 |
| I-5 | 工作站 OC 先验（`gpu_type.rs:495-520`）：WorkstationBlackwell 落通用保守参数（`core_oc_safe_limit 300000`、`is_50_series: false`） | 450W 服务器卡按 300k 安全限起步是合理保守；`is_50_series:false` 是否会错过 50 系扫描逻辑需逐消费者核对 | 低风险，留观察 |
| I-6 | 驱动版本门：全仓 grep 无 `"580."` 字面量门 | 版本分叉点目前只在文档层（kmd-power-policy 610.74 布局、NVML VfOffset cmd 等） | T2-7~T2-12 的分叉记录落文档即可，暂无需代码门 |
| I-7 | Python/GUI/TUI 同构：`gui/src/tabs/vfcurve/tab.py:5639/5672` 与 `gui/src/tabs/dashboard/sections/overclock.py:1308` 的 `_is_blackwell_from_info`、`tui/nvoc_tui/controllers/vfcurve.py:1262` 都按 slot 2/3 平移；pynvoc 已导出 `set_volt_rail_slot_offset`/`reset_volt_rail_limit` | 检测函数与 Rust 侧同源（name+codename 口径） | I-2 的 codename 风险同样决定 GUI/TUI 的槽位映射是否生效——一个修复点覆盖全栈 |

## live-nvml 补跑结果（主循环完成，`probe/nvml-sweep2.txt`）

- 双卡 parity 确认：brand=14、**arch=10（Blackwell 的 NVML 枚举值，T3 级新常量）**、cc 12.0、power 450/450/450、P8。
- **VfOffset 全零**：`GetGpcClkVfOffset/GetMemClkVfOffset = 0`，`MinMaxVfOffset = [0,0]`——服务器 SKU 上公共 VF 偏移路径名义存在但**范围锁死**（与 450W max=default 一致，无用户 headroom）。
- 温度：sensor 0（GPU）唯一可读（39/38°C）；sensor 1-15 全 NOT_SUPPORTED——内存温度面确认不存在。
- NVML 时钟域枚举仅 4 个（0=Graphics/1=SM→2430、2=Mem→12481、3=Video→2107）；`SupportedPerformanceStates = [P0, P8]` 仅两档；throttle 支持集 0x1ff。
- SupportedMemoryClocks=[12481,405] 双档，graphics 阶梯 301+95 步（~7.5MHz 粒度）。
- 首轮 sweep（`nvml-sweep.txt`）在 GetClockOffsets 后 core dump；`nvml_probe2.py` 子进程隔离补齐。field-values 扫描 struct 布局有误作废（与"无电压面"结论不冲突）。

## 后续任务书

1. `blackwell_recon_live` 只读探针跑本机（settles T4-4；需用户批准 cargo build nvapi-rs live tests）。
2. GSP 固件逆向立项（T4-3/K-6）：`idb_open /lib/firmware/nvidia/580.173.02/gsp_ga10x.bin`（75MB RISC-V 全符号）——Windows kmd 路线的 Linux 等价物（BoardObj/PowerRoot/VFT 构建器）全在里面。
3. `0x28DCA3F0` 写路径安全评审（T4-2，对照两阶段连贯写协议）。
4. ~~NVML 活体域枚举~~（已完成：`probe/nvml-sweep2.txt`，结果已回填）。
5. RPC 侧功率控制序列化面枚举（验证轮遗留）：`rpcCtrlPerfRatedTdpSetControl_{STUB,v1A_1F}` @`0x176780`/`0x387470` 及 `rpcRmApiControl_*` 族。

## 证据与产物索引

- IDA 会话：`nv-kmd-580`（nvidia.ko，全符号）、`nv-api-580`、`nv-ml-580`、`nv-modeset-580`（工作副本 + .i64 均在 `reverse/linux-580.173.02/`，不入库）
- 源码地面真值：`reverse/open-gpu-kernel-modules` tag 580.173.02（二进制 1:1 核对通过）
- live probe 归档：`reverse/linux-580.173.02/probe/{smi-full,smi-features,smi-supported-clocks,smi-query,query-gpu,proc-gpu-info,dmon-sample,nvml-exports,nvml-sweep,nvml-sweep2}.txt` + `nvml_sweep.py`/`nvml_probe2.py`
- 多代理工作流：13 探测维度 × 逐 finding 对抗验证 × completeness critic × 3 followup；两轮共 39 个代理（25 首轮 + 5 个 429 限流死者中 2 个重跑成功，3 个由主循环人工补齐），journal: 会话目录 `wf_996f8571-8d0`；digest 快照 `probe/workflow-digest-r1.md`
- modeset-survey / wrapper-gaps 维度未被独立代理完成（429），已分别由 strings-sweep+versions 修正、qi-table T3-3 覆盖关闭
