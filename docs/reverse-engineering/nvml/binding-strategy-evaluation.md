# NVML 绑定策略与 RM 直写评估

- 日期：2026-09-10
- 结论性质：策略决策记录（无代码变更）
- 关联：`nvml/export-coverage-audit.md`、`../nvapi/linux-version-coverage-audit.md`、`../nvapi/woa-arm64-coverage-audit.md`、`reverse/kernel-rm-index/`（open-gpu-kernel-modules 对账产物）

## 1. NVML 绑定：是否需要像 nvapi-rs 一样自己 fork

### 现状盘点

- 依赖：workspace 钉 `nvml-wrapper = "0.12.1"`；`nvml-wrapper-sys = "0.9.1"` 在代码中**零直接使用**（钉版仅为锁传递版本）。
- 使用面：`core/src/nvml.rs` 高层封装约 25 个函数——UUID、功耗（draw/limit/ensemble）、PCIe 遥测、auto-boost 三态、API restriction、功耗墙 SET、时钟偏移 get/set（×2 缩放）、温度阈值/墙/声学、throttle reasons、violation status、pstate 区间、applications clocks、风扇三件——**不只有遥测，含一组 SET**。
- 缺口策略：wrapper 未绑定/别扭的导出（fan-speed v1、nvmlShutdown、GetNumFans 回退）走自建 libloading shim；覆盖对照表见 `nvml/export-coverage-audit.md`（R550 nvml.dll 417 导出 vs sys 绑定 373，44 个未绑定全为私有）。
- 平台：WOA 三件套（System32 nvml.dll=nvml_loader 改名 / DriverStore nvml_arm64ec.dll）的架构感知已在 `core/src/dll_path.rs` 解决（2026-09-10 补丁），不需要动 wrapper 内部。

### 上游状态（与 nvapi-rs 处境相反）

[rust-nvml/nvml-wrapper](https://github.com/rust-nvml/nvml-wrapper) 活跃维护：0.12.1 / sys 0.9.1 就是上游最新（2026 年中发布），Debian/Ubuntu/Fedora 均在打包；针对 NVML 12 开发，且 NVIDIA 对 NVML 公共 ABI 有**向后兼容承诺**（`ver<<24|size` 版本纪律）。我们已经在最新版上。

### 决策矩阵

| nvapi-rs 当年的 fork 动因 | NVML 现状 |
|---|---|
| 原作者停更 3 年 | 上游活跃，我们已在最新版 |
| 需要大量私有面（ID 注册表 + proc-macro 大改） | 我们只用公共面；私有/缺口走自建 shim，fork 不改变这一层 |
| 上游无人合 PR | 上游接收 PR、发版正常 |
| NVAPI 私有面无版本兼容承诺 | NVML 公共 ABI 有官方向后兼容承诺 |

**结论：不 fork。** 触发器（满足任一才重议）：
1. 上游停滞 >12 个月，且 NVML 新导出为 nvoc 所需；
2. 需要批量绑定私有 NVML 面（现策略 shim 够用则不动）；
3. 上游架构变更破坏 nvoc（如移除 `Nvml::builder().lib_path()` 这类定制点）;
4. 平台特化需求侵入 wrapper 内部（WOA 已证明可在 dll_path 层解决，无需）。

## 2. RM 直写：是否纳入本项目

### 认知校正

"RM CONTROL 是绕过 nvapi/nvml 直接跟内核对话"这个理解需要修正：**NVAPI/NVML 本身就是 RM CONTROL 的薄客户端**。Linux 实测（610.57.04）：libnvidia-api 与 libnvidia-ml 是两条**平行的独立 RM 客户端**，各自直接对 `/dev/nvidiactl` 发 ioctl `0xC020462A`（NV_ESC_RM_CONTROL），中间没有任何共享层；Windows 上 nvapi64 的 QueryInterface handler 内部同样组 RM escape（0x0700_01xx 私有族等）。所以"RM 直写"不是打开了一条更危险的新通道，而是**跳过厂商客户端包装、复用同一内核 ABI**——代价是放弃厂商库里的参数校验、版本协商、重试/重连和按特性门控，收益是不受厂商用户态库的功能覆盖面限制。

用户类比"给 v520 之前的 Linux 自己写一个 nvapi"——**准确**。自建 RM 客户端要补齐的正是 libnvidia-api 免费提供的那套：client/subdevice 分配（NV_ESC_RM_ALLOC 0xC030462B）、rmapi 版本握手、NVOS54_PARAMS 控制封装、错误分类学、逐分支 cmd 验证矩阵。这是个完整客户端，不是一个函数调用。

### 真实缺口在哪里（这个决策才有的谈）

| 环境 | 控制面现状 | RM 直写的边际价值 |
|---|---|---|
| Windows（全分支） | nvapi64 恒在，私有面 nvoc 已全覆盖 | **无** |
| Linux ≥ v525 | libnvidia-api 存在，610 实测 590 项覆盖 nvoc 所需（TGP/pstate/power 族齐） | **很小**（仅 525–570 中间版本覆盖未实测，见 Phase 0.5） |
| Linux < v525（470 legacy 等） | 无 libnvidia-api；NVML 公共面缺 `SetGpcClkVfOffset`/`GetClockOffsets`/`GetNumFans`（470 dynsym 实测） | **这是唯一真缺口**：老 Linux 上的 V/F 偏移一族 |

即：需要"自写 nvapi"的场景=Maxwell/Pascal 时代卡 + 470 legacy Linux + 想要 V/F 偏移控制。窄，但确实是本项目 legacy 工作线（TITAN X / 定制超频社区）的用户群。

### 三条路径

- **A. 维持现状**：老 Linux 只给 NVML 公共面能给的控制；V/F 缺席。
- **B. 借道厂商半公开通道**：470 的 libnvidia-ml 导出 **`nvmlRetry_NvRmControl`**（470 dynsym 实测存在、610 已移除）——厂商自带的 RM 直通口，客户端上下文（client/alloc/版本握手）由厂商库管理，我们只传 cmd。拿它先验证 V/F 一族在 470 内核上是否可用，**不需要自建客户端**。Windows R550 的 nvml.dll 私有导出分组（export-coverage-audit §未绑定表）也有同类口子可查。
- **C. 自建 RM 客户端**：完整"自写 nvapi"。成本=NVOS54 封装+alloc+握手+错误学+逐分支验证矩阵，并且**分支可验证性分层极不均匀**：
  - R515+：内核模块开源（本仓 `reverse/open-gpu-kernel-modules/`），cmd→handler→结构可溯源——**但 oracle 是残缺的**：同版本（610.57.04，`version.mk:2`）的 open 树把 0x90/0xD0/0xF0/0xC6/0xA6 五个时钟/功率管控组 define 全部剥离（VOLT 接口 FINN 0x208032 在 `g_finn_rm_api.h:420` 挂名、ctrl2080volt.h 仅 38 行空壳；AVFS 同为空壳），"接口在、命令剥离"是政策行为（同版本闭源二进制实测在用 0x2080F031/0x2080C637/0x2080852F）。可验证的只剩公共族（PERF/GPU/BIF/BUS/FB/GSP/热/电源事件）。
  - 470 及以下：内核模块**闭源**（nv-kernel blob），零源码 oracle，只能拿同分支 NVML 二进制 mimic——恰是"520 之前"类比所处的最差档。
  - ⇒ 修正后的判定：**自建客户端在任何分支都拿不到时钟/功率管控族的强 oracle**——开源分支的剥离面与闭源分支的黑箱面，验证难度同级。

### 结论：不纳入主线；按需求门控分阶段

- **Phase 0（现在）**：不写 RM 直写层。本评估文档 + kernel-rm-index（内核 cmd 索引）作为地基留存。
- **Phase 0.5（廉价实证）**：下载 535/550/570 各一个 Linux 包，跑 `qi_table.py`+IDA 把 libnvidia-api 覆盖面实测出来，界定"525 以下才有缺口"这个断言的真实窗口。
- **Phase 1（仅当老 Linux 用户确有需求）**：走路径 B（`nvmlRetry_NvRmControl`），窄面验证 V/F 偏移一族（Windows 侧已知的 0x20809021/22/23、0x2080D024、0x2080F031）在 470 内核的行为。通道是厂商的，风险敞口只有 cmd/参数本身。
- **Phase 2（仅当 Phase 1 证明需求持续）**：才考虑自建客户端，且**先做 R515+ 开源内核分支**（有完整 oracle），470 闭源分支最后做、只做 Phase 1 已验证过的窄面。

## 3. RM CONTROL 风险模型（两条线共用）

内核侧四层防御（open-gpu-kernel-modules 610.57.04，file:line 已核）：
1. cmd 必须在注册表内：`_rmapiRmControl`（`src/nvidia/src/kernel/rmapi/control.c:343`）→ `rmapiutilGetControlInfo`（`rmapi/rmapi_utils.c:161-196`，class→FINN 导出表线性查 methodId）→ 未知 cmd 干净返回 INVALID_COMMAND；
2. 每-cmd params layout：`paramsSize` **精确等值门**（`control.c:447-451`）——尺寸/版本不符返回兼容性错误而非未定义行为（与 nvapi-rs "stamp 即 ABI" 主张同构）；
3. 句柄校验：`serverControl`（`rmapi/resource.c:162`）的 client/subdevice 上下文查找；
4. 特权 SET 门：权限检查**先于 handler**（`control.c:683-697`，调用点 `control.c:830`；`NV_ERR_INSUFFICIENT_PERMISSIONS` = nvstatuscodes.h:56）——对账唯一被内核确认的 nvoc 主张之一，与我们实测的 -137/-104 权限语义互证；另有 `control.h:167-344` 权限宏与 `control.c:789` 白名单。

真实风险排序（与缓解）：
1. **跨分支版本漂移**（私有 cmd/结构/stamp 变动；已付过学费：0x57B5A5DF 尺寸 0x9B8→0x9D8 修正）——缓解：逐分支覆盖审计、stamp 纪律、healthy-read hybrid、`allowable_result` 优雅降级，全部已存在。
2. **绕过厂商客户端侧钳制的硬件风险**——与一切 OC 工具同级（Afterburner 的限幅也是客户端侧的）；遵守 AGENTS.md 高危写入原则：只读验证先行、恢复路径可见。
3. **NDA 面无契约**——实例：Linux 0xC0/0xC4 死桩、WOA 103 缺 ID；缓解：能力探测，永不假设。

**总评**：RM 直达的风险形态是**维护性**（版本漂移、无契约），不是**危险性**（内核不会因坏参数崩溃）；是否值得承担，取决于缺口场景的真实需求密度——这正是 §2 分阶段门控要回答的问题。

## 4. 内核对账结果摘要（2026-09-10，详见 `reverse/kernel-rm-index/reconciliation.md`）

对 nvapi-rs/nvoc 的 165 条可验证主张逐条对账：**内核确认 2 / 内核无此面 161 / 冲突 0 / 待复核 2**（nvoc 自知的两处内部不一致，见 claims §冲突标记）。

- **零冲突**：所有私有面要么被 open 树剥离、要么位于内核概念外（Windows escape 是 nvlddmkm 私有分发号，与内核 FINN 接口表零编号对应）——开源内核**既不能证实也不能证伪** nvapi-rs 的私有编号。
- 被确认的两条是**机制级**：权限门先于 handler、GC6 功能面（内核对应物 0x2080270D/E，`kern_gpu_power.c:690/711`；escape 号本身仍 Windows 私有）。
- CLK 域专项：open 头仅 Tegra 3 值，nvoc 9 域口径（0..9 + mask 0x3FF）在开源侧**完全不可验证**，裁决出路是闭源二进制/GSP 固件。
- 对 §2 的影响：oracle 残缺使 Phase 2（自建客户端）的性价比进一步下降；对 §1 无影响（NVML 公共面照常受内核分发覆盖）。
