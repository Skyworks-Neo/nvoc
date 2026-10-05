# xOCD 2.0.0 能力改动增量审计（2026-10-05）

目标：`reverse/xocd/xOCD.exe`（用户提供的新版本，版本串 **2.0.0**；上一版 1.4.33 的审计见 [`xocd-oc-tool-audit.md`](./xocd-oc-tool-audit.md)（主仓 557b62e））。
方法：与上一版同一流水线（PE 切分 → ilspycmd 反编译 → 新旧源码/二进制对拍）；本次另开两个子代理分别深挖 NVAPI 驱动面与 `xOCD.ExtendedLimits`/PMX 新子系统。
对照基线：主仓 `cli-more-reversing`，nvapi-rs 子模块当前工作树。

**一句话结论**：2.0.0 是一次**主版本级能力扩张**——它第一次引入**签名内核驱动**（pmxdrv.sys）去直改 `nvlddmkm.sys` 的内存策略对象，从而突破用户态 NVAPI 不可及的功率 125% 墙与 GDDR7 显存 +3000 MHz 偏移墙；同时补齐时钟微调（XBAR/SYS/NVD 偏移）、保留限制跨会话恢复（DPAPI 回滚胶囊）、只读诊断报告与 A/B/A 基准举证协议。上一版审计中「无驱动安装、无内核组件」的结论**仅对 1.4.33 有效，且只对 NVAPI 读写面继续有效**：新的超顶能力必须走内核。

## 1. 产物锁定与跨机复现

| 产物 | SHA256 | 大小 |
|---|---|---|
| xOCD.exe（2.0.0 原始，本机文件） | `a434aaa60a947b2a90af6a5f68077524e4285850b2ed6438f5ced31f3147e4ab` | 6,276,239 |
| xocd-app-new.exe（切出的托管主程序） | `322f2ade4bf656d5acbe285971daf405415df9c833b270ac13e9c485950d7840` | 6,008,832 |
| xOCD.PmxDriver（工具内嵌的内核驱动资源，**带签名完整件**） | `b1a8ee1222eea5f199028d90b9b77c2acf46d6d84a9e125403b2888c6f681c72` | 43,632 |
| xocd-worker-new.exe（D3D12 基准 worker） | `2c87e773c12559735a071d44045571e8a3db9dde7ab933d16c9c7e94c0bcbc5b` | 343,040 |

关键对拍事实：

- **D3D12 基准 worker 与 1.4.33 逐字节相同**（SHA256 与上一版 `xocd-native2.bin` 相等，代码节表/时间戳一致）——所有能力改动都在托管层与新的 PMX 内核件里，基准负载本身未动。
- `xOCD.PmxDriver` 的 SHA256 与工具自身的部署校验常数 `PmxHelperDeployment.ExpectedHash`（`xOCD.ExtendedLimits/PmxHelperDeployment.cs:10`）相等，即**该资源就是运行时部署的 `pmxdrv.sys` 本体**（PE32+ x86-64、subsystem Native、9 节、仅导入 `ntoskrnl.exe`/`HAL.dll`、Authenticode 目录在位；内嵌字符串 `\Device\Pmxdrv`、SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)`、PDB `pmx-cse-new\x64\Release\pmxdrv.pdb`）。切分脚本另切出一份 23,040 字节同名截短拷贝（丢 `.reloc` 与签名尾），以 ilspy 提取的 43,632 字节完整件为准。
- 复现：`reverse/xocd/split-new.py`（PE overlay 切分）+ `ilspycmd -p`（托管层 300 个 .cs：xOCD 215 + ExtendedLimits 68 + FlatUI 10 + 生成源等，未混淆）+ 新版反编译树本机在 `~/ida-scratch/xocd-decomp-new/src/`。

改动面统计（旧→新逐文件 diff）：**25 个新文件 / 70 个修改文件**（该统计只覆盖 `xOCD/` 目录）；热点 `MainForm.cs` +3762/−509、`NvidiaController.cs` +1565/−284、`Program.cs` +728/−270、`NvApiSource.cs` +686/−152、`FanCurveForm.cs` 重写 +406/−188。此外新增整个命名空间 `xOCD.ExtendedLimits`（68 文件 / 10,366 行，v1.4.33 完全没有）与 `xOCD.FlatUI`（主题控件 10 文件）。

## 2. 新子系统：Extended NVIDIA Limits（内核补丁式超顶）

激活是选项里的 opt-in 开关（"Allow Extended NVIDIA Limits"）；`NvidiaController.SetExtendedPower` 硬门控：RTX 3080/3070 Ti → Ampere 会话、名称含 "RTX 40" → Ada、含 "RTX 50" → Blackwell 会话，其余 `NotSupportedException`。设置本身不持久化（每次会话重新启用），跨会话由带 **DPAPI 保护回滚胶囊**的 `RetainedLimitsRecord` 承接（见 §3.2）。

**访问机制（决定性二进制证据）**：`xOCD.PmxDriver` 是**签名内核驱动**，不是用户态 helper——PE subsystem=1(Native)，仅导入 `ntoskrnl.exe`（`IoCreateDevice/Secure`、`ZwOpenSection`、`ZwMapViewOfSection/Unmap`、`MmUnlockPages`、`IoFreeMdl` 等）。经 SCM 加载为内核服务 `xOCD_ExtendedPower`（`sc.exe create … type= kernel start= demand`，`PmxService.cs:125,158,248`）；用户态打开 `\\.\pmxdrv` 设备（`PmxPhysicalMemory.cs:94`），以 METHOD_BUFFERED IOCTL `0x222AB8/0x222ABC`（Map/Unmap）批量映射 `\Device\PhysicalMemory` 物理页。`NvidiaKernelReader` 用 `NtQuerySystemInformation(11)`+SeDebugPrivilege 找到 `nvlddmkm.sys` 基址，扫描物理空间匹配掩码根签名，做 4 级页表（39/30/21/12）翻译，直接读写驱动对象；**无 CR3 回退**。

**它到底改了什么**：

- **Blackwell 显存（GDDR7）**：把驱动时钟面 "E2" 能力字从 3000 抬到 `max(5000, VBIOS BIT C PLL 推算值)`，配合 `pnputil /restart-device` 重启 GPU；再补丁原始物理上界 `rawPhysical` 3000 → `RawMaximumMhz = min((PllMaximum*2000 − FactoryKhz)/1000, 65535)`。工具自述（`BlackwellMemoryGeometry.cs:1448`）：*"GDDR7 driver offset range extended to +N MHz. Running memory frequency above +3000 MHz has not been verified"* —— 扩的是驱动面，>+3 GHz 的实机稳定**未背书**。
- **Blackwell 功率**：按 role 补丁 board/shared/root 贡献策略字段并置 policy+68 dirty 字节，随后通过板卡策略 SET 强制发布；若 INFO 读回不变，**回退到 power-command 租约**（channel/command `0xFE`/`0xF8`）。上限 `min(默认 125%, stockMax+allowance, stockMax 125%)`，绝对夹 2,000,000 mW；自定义档 = stock..min(250% stock, 2500 W)，0.5 W 步进。
- **输入电流**（policy 9，字段 264/276）：上限抬到 125%。
- **Ada（全部 RTX 40）**：board policy max + 镜像字段补丁，经 `_native.Write(_originalNative)` 发布，绝对上限 1,250,000 mW；除名称门控还有实时验证（`VerifiedAdaPowerSession.cs:377`）。
- **Ampere（仅两档 VBIOS 锁定目标）**：RTX 3080 `94.02.26.48.1f` → 900 W；RTX 3070 Ti `94.04.5A.00.F2` → 550 W。

**安全/回滚模型**：写前 compare-and-validate（所有权链、PCI/BAR、物理背衬、镜像 SHA256 与时间戳）；`LimitTransaction` 预检读比对、按序写、失败即恢复、驱动换代时拒绝缓存地址；回滚胶囊 DPAPI 加密、Windows 启动 GUID 变化即作废；关闭时 fail-closed（清理未验证就取消退出或标记 `_shutdownCleanupFailure` 要求重启；无法验证的关机写 `AllowExtendedNvidiaLimits=false` 而不是去碰挂死驱动）；黑名单组合崩溃护栏（RTX 5090 / 617.14 / VBIOS 98.02.2e.80.c9）。工具明确不动 Secure Boot / 内存完整性 / 易受攻击驱动黑名单——它的逃生门是 pmxdrv.sys 本身是**已签名**的 legacy 驱动。

**对 nvoc 的直接结论**：这**实证闭环**了我们 NvpwrControl 审计中「用户态抬顶结构性不可达」的判断——nvapi-rs 能写的 125%/OCP 上限就是驱动发布的策略值本身，再往上只能改驱动内存。ExtendedLimits 用到的 NVAPI ID 全部已在 `nvid.rs` 注册：板卡策略 GET/SET `0x70916171/0xAD95F5ED`（ClientPowerPoliciesGet/SetStatus）、功率命令 `0x33AB0353/0x17695269`（ClientPwrPoliciesGet/SetControl）、VF 表 `0x21537AD4/0x23F1B133`；差异从来不在 ID，而在写前先改内核对象。

## 3. 用户可见能力增量（UI/功能面）

### 3.1 设置与解锁面

- 新增 `UiTheme`（System/Light/Dark 三态，`ThemeMode.cs`）、`TesterName`（诊断报告身份）、`DriverRecoveryManualApplyRequired`。
- OCP 解锁**双轨独立**：`UnlockNvvddOcpLimits` / `UnlockMsvddOcpLimits`（旧 `UnlockPowerChannelLimits` 向后兼容回落）；校验报错分轨（"…requests X A above the firmware default…"）；UI 明示 *"The 5001 A raw sentinel is never exposed as a usable ceiling"*（5001 A 原始哨兵值不当作可用上限）；RTX 3080/Ampere 特案：即使无高于默认的余量也允许 arm（`OptionsCapabilityPolicy.IsLegacyAmpere`）。
- `AllowExtendedNvidiaLimits` **永不持久化**（`AppSettingsStore.ForPersistence`/`Deserialize` 双处强制 false），保证「重启后必须显式重新启用」。
- 设置落盘改**原子替换**（临时文件 + `File.Replace` + flushToDisk），配置损坏不再半写。
- 主题默认色 `#168BFF` → `#FF8A2B`。

### 3.2 保留限制生命周期（KeepGpuSettingsOnExit / RetainedLimits）

- `RetainedLimitsRecord`：Uuid+PCI+Driver+Vbios+PowerMaximumW(±1.1 W 容差)+MemoryMaximumKhz+Power/Memory 标志+`WindowsBootId`+`RollbackCapsule`。`Matches/MatchesIdentity/IdentityMismatch/HasReturnedToStock` 四判据：身份任一不符、**跨 Windows 启动**（`NtQuerySystemInformation` class 90 的 BootIdentifier GUID）、回滚胶囊缺失、或功率已回落到 stock 判定 → 拒绝重放/判定已回 stock。
- `RetainedRollbackProtection`：DPAPI（`CryptProtectData/UnprotectData`）密封回滚胶囊（后端/镜像哈希/显存几何/功率补丁 before-after/原始字节/所有权映射/功率角色基线/命令通道），载入时篡改即抛 `IOException`。
- `ShutdownDefaultsWorkflow.RestoreAsync`：退出清理顺序硬校验——先禁扩展限制 → **读回确认已禁** → 恢复驱动默认 → 再确认限制未复启；任一步失败抛异常（fail-closed）。
- `RetainedPowerVerification.UseControl(verify, request, baseline, cap)`：三态决策——验证通过且请求=基线 → 用；请求≠基线且低于 cap → 用；三态未知且介于其间 → `IOException`（拒绝含混）。
- UI 侧三态只读呈现（`OptionsForm.ShowRetainedState`）："state unverified (read-only)" / "retained in driver (uncheck to restore)" / "readback verified; read-only"。

### 3.3 只读诊断与举证

- `XbarDiagnosticReport.Save`：XBAR 能力/控制布局 JSON（`ReadOnly = true`，自述 *"no new ratio or curve write is authorized by this report"*），字段含 XbarClock/XbarVoltageDemand/偏移窗/实测 MHz。
- `GpuCompatibilityReport`：兼容性 ZIP + 会话目录归档（"unavailable interfaces are recorded independently. No unlock, reset or hardware write is attempted."）。
- `NvidiaRecoveryEvents`：经 `wevtapi` 查询 Display 提供程序 **EventID 4101（TDR，nvlddmkm）**，用于崩溃/驱动恢复归因；配套反例自测（"Another vendor was classified as an NVIDIA crash."、"An unexpected shutdown was classified as a driver recovery."）。
- `ErrorReporter`：错误自动落盘（error.log + 全屏截图 + `ReportIdentity`）到 `Desktop\xOCD-Reports`；未处理异常挂接（UI 线程 + AppDomain）。`PowerObservation`/`TweakRunEvidence`/`TweakSample` 记录 Requested/Published/Enforced/实测四层功率与 XBAR/SYS/NVD 时钟采样。

### 3.4 时钟微调（XBAR/SYS/NVD）

- `ClockTweakPolicy`：私有记录 `176 + slot*1072`，privateId 1/2/21 = XBAR/SYS/NVD；范围来自驱动 min/max 且须过 Coherence 检查（min≤0 且 ≥−10000、max>0 且 ≤10000）。
- `ClockControlTransaction`：RMW + 全状态回滚；自测证明**单字段字节级写**（offset 8 频率 kHz，其余字节不动）、无变化零写、失败后恢复原始字节、驱动换代即重读（`ClockTweakTests.cs:105-161`）。
- UI 新增 "VIDEO / NVD CLOCK OFFSET"、"SYS/XBAR clock offset"、"Supporting clocks" 分组；多处说明 *"Driver NVD domain offset; shares MSVDD with XBAR and SYS"*（NVD 与 XBAR/SYS 共享 MSVDD 约束）。

### 3.5 V/F 曲线新能力

- `VfCurveEditingPolicy.RampUpperVoltagePoints`：**上段斜坡**——选定一个 >1100 mV 的已发布点为目标，从实时 1100 mV 锚点起线性插值生成各点偏移（越界 ±1 GHz 或驱动未发布锚点/目标点即抛），编辑器按钮 "Ramp upper curve"（同时释放电压锁）。
- `NvVfApi.CaptureRawCurveDiagnostics`：**裸表诊断 dump**（只读）——读 `0x21537AD4`(7208B) 与 `0x23F1B133`(9248B)，按 28 字节/条目导出 Type@+4、FrequencyKhz@+8、VoltageUv@+12，标出 >1100 mV 的 plausible slots；无需写即取证。
- 直读证据：28 字节条目布局与我们私有 VF 表记录字段吻合（freq/volt 偏移与我们的解析一致）。

### 3.6 风扇曲线窗体重写

FlatUI 重写：可拖拽绘图（键盘方向键微调）、预设 Silent/Balanced/Aggressive、实时温度标线（"Now NN °C"）、2–10 点硬校验、单调不减校验、安全警示（"never reaches 100% fan speed" / "below 50% at 80 °C" 变红）、Add/Remove point。

### 3.7 遥测

- 新增 **Temps 页**：GPU 温度/热点/显存结温的 Current + 会话 Average/Minimum/Maximum 统计（`TemperatureHistory`），"Reset telemetry" 清历史。
- Clocks 页同样带会话统计，域列表含 **XBAR/SYS/"Video / NVD clock"**；`ObserveSnapshot` 采集 core/mem/XBAR/SYS/Video。
- 通知页切换/GPU 切换即清理历史（`ClearForGpuSwitch`）。

### 3.8 基准 A/B/A 举证协议（TweakComparison）

三份 BenchmarkForm JSON（Baseline/Tweak/Restored）强校验后才出对比：GPU 身份/驱动/VBIOS/负载配置全等、每跑内 reported tuning state 不得变化、遥测中断必须为 0、**恰一个设置改变**、恢复后设置须等于基线、时间序校验。26 项设置面全量 diff（core/mem/XBAR/SYS/VIDEO/HUB/UPROC 偏移、四域电压 demand、NVVDD/MSVDD 上下限与 ratio、功率、双 OCP、boost lock、时钟范围、风扇、温度目标）。输出内置免责声明：*"Clock-control readback confirms a stored request, not effective V/F adoption or an independent hardware clock measurement."*、*"XBAR, SYS and NVD share MSVDD constraints; a requested offset need not produce an equal live-clock change."*、*"MSVDD demand/floor/ceiling is not physical MSVDD measurement."* ——与我们在 SDC/readback 上的纪律同源，可直接借用于 nvoc 报告口径。

### 3.9 UI 与稳健性

- 新 `xOCD.FlatUI` 主题控件 + `Theme/ThemeMode/ThemedForm/ThemedMessageBox`（深/浅/系统主题，可运行时切换）；`Palette` 全面换色。
- `TelemetryBubble` 绘制失败安全回退（一次性 `RecordFatal` 上报 + 简单矩形回退绘制）、仪表渲染自测；`HiddenTogglesPopup` 不可用项显示**具体原因**（"Unavailable from GPU / driver"+reason）。
- `ProfileStore` 文件名校验加固（保留设备名 CON/PRN/AUX/NUL/COM1-9/LPT1-9、非法字符、`.`/`..` 拒绝）。
- `Program.cs` 新增 15+ 个自检/报告 CLI（`--flat-ui-self-test`、`--clock-tweak-self-test`、`--compare-tweaks`、`--rtx3080-vf-policy-capture`、`--rtx3080-power-evidence-self-test`、`--helper-deployment-report`、`--extended-power-session-test`（需 `--confirm-hardware-writes`）、`--shutdown-defaults-self-test`、`--xbar-report-self-test`、`--gauge-render-self-test` 等）。

### 3.10 RTX 3080 特案与方法论模块

- `Rtx3080DynamicAssistPolicy`（NVVDD 1118–1200 mV 窗）、`MsvddExperimentalRequestPolicy`（vbios `94.02.26.48.1f` 精确匹配）、`BlackwellBoardPowerFallback`（"Native board-power policy write" 收到 `NVAPI_INVALID_ARGUMENT (-5)` 时重试回退）。
- `Rtx3080VoltageEvidence`：显式区分「发布曲线 >1100 mV 点数」「实测电压是否 >1100 mV」，并**恒置** `DynamicScalingAbove1100MvVerified=false`，判词：单次捕获不足以证明动态 V/F 选择或性能收益，须同负载对比时钟/功率/限制原因。
- `Rtx3080PowerEvidence`：Requested/Published/Enforced/Draw 四层 + `EnforcedLimitBelowRequest`（enforced+5 < requested）+ ≥85% 负载下 power-cap 时钟事件计数；判词 *"The published ceiling alone is not the effective board budget."* ——与我们 TGP 发布/执行分离的认知同构。

## 4. NVAPI 驱动面增量（NvApiSource/NvVfApi/NvidiaController）

基线：新旧 `xOCD/`、`xOCD.ExtendedLimits/` 源码对拍（旧树 `~/ida-scratch/xocd-decomp` ↔ 新树 `~/ida-scratch/xocd-decomp-new/src`），对照 `nvapi-rs/sys/src/nvid.rs`。**核心结论：核心 NVAPI 封装的调用面（ReadRaw/GetStruct 的 (id,version,size) 三元组）新旧一致，无任何 `NvAPI_*` 符号名新增**——增量全部在封装之上的调用点、几何与新 ID。

### 4.1 新增 ID 与几何（v1.4.33 从未调用）

| ID | 符号（nvid.rs） | 读写 | 戳/几何 | 证据 | 用途 |
|---|---|---|---|---|---|
| `0x33AB0353` | ClientPwrPoliciesGetControl | 读 | query 0x10528（v1\|1320） | `xOCD.ExtendedLimits/BlackwellPowerCommand.cs:8,27-32`；`BlackwellNativePort.cs:45,365` | 读每通道 32 位功率命令（mask 1<<ch@+4，value@40*(ch+1)，cmd@+4） |
| `0x17695269` | ClientPwrPoliciesSetControl | 写 | 0x10528 | `BlackwellPowerCommand.cs:10`；`BlackwellNativePort.cs:377` | 命令 `0xFE`=抬顶、`0xF8`=保留目标；写后有读回验证（`BlackwellLimitSession.cs:2422,2492`） |
| `0xFA579A0F` | EnableDynamicPstates | 写 | enable u32 | `BlackwellNativePort.cs:187-190` | 解锁显存前冻结动态 pstate |
| `0x57F7CAAC` | GetRamType | 读 | out==16（GDDR7） | `BlackwellNativePort.cs:170` | 显存解锁门控 |

新增几何（ID 本就注册/已知，但 xOCD 首次给出确定性 buffer 布局）：

| ID | 用途 | 几何 |
|---|---|---|
| `0x67F31384` | PowerChannels INFO（内核补丁计划输入） | Blackwell 2,727,984 B / 戳 0x2BA030；回退 347,124 B / 0xF4BF4（`BlackwellNativePort.cs:123,132`） |
| `0x8B3E7343/0xAFFC2279` | PwrPolicies Get/SetControl（输入电流请求） | Modern 2,393,824 B / 0x2786E0、条目 2664；Legacy 307,376 B / 0x5B0B0、条目 2608；mask 512@+136；policy idx 9 / type 25，request==amps*1000（`BlackwellLimitSession.cs:30-32,405-429,442-456`） |
| `0x6FF81213/0x0F4DAE6B` | GetPstates20/SetPstates20 | GET 戳 0x21CF8（v2, 7416）；SET 最小包 `{+0=0x21CF8,+8=1,+12=1,+28=4,+40=kHz}`；P0 条目基址 20+i*456、clock type 4（`BlackwellNativePort.cs:194,256-299`） |

新封装 `NvidiaBoardPolicyPort`（板卡策略端口）：公共 Init/Unload/EnumPhysicalGPUs/BusId/BusSlotId/GetFullName + `0x70916171/0xAD95F5ED`，72 B v1 戳 0x10048、count@+4∈[1,4]、P0 type==0@8+i*16，且要求写入后**逐字节保持**（`NvidiaBoardPolicyPort.cs:42-69,74-134`）。六个公共 ID 在 1.4.33 即已使用，72 B 布局与旧 `NvApiSource.cs:490` 一致。

### 4.2 行为变化（写序/回滚纪律）

| 面 | 旧（1.4.33） | 新（2.0.0） | 证据 |
|---|---|---|---|
| rail 重置 | 把控制字 +4/+8/+16 清 0 | 恢复捕获的原字节，逐字节比对+失败回滚（ExactRailDefault） | 旧 `NvApiSource.cs:2509-2524` → 新 `xOCD/NvApiSource.cs:2891-2946` |
| XOC 上限 | 用驱动上限 | `verifiedXocCeilingMv` 覆写；RTX 3080 驱动说 1100 时实验性 1200 mV | 新 `NvApiSource.cs:2848-2851,2951,2999-3002` |
| P0 电压条目 | 硬编码 DomainId==0 | 扫描 BaseVoltages、±500 mV 合理域校验、携带 DomainId/EntryFlags/PstateFlags | 旧:950-964,1028-1039 → 新:874-908,1062-1085 |
| 时钟写 | 直写+读回 | 事务化 Commit + 经 `0x57B5A5DF` 复核路由身份；失败置持久 fail-closed 标志 | 新:4206-4241,3514-3520,353,4238；`ClockTweakPolicy.cs:9-37` |
| 时钟边界 | 硬编码 switch | 从 ClkDomains 路由表读驱动上报 | 旧:3794-3804 → 新:4233-4245 |
| OCP 解锁 | 单全局标志 | 双轨 UnlockNvvdd/MsvddOcpLimits + RTX 3080 Ampere 回退 | `AppSettings.cs:27-29`、`ProfileOptionPolicy.cs:43-68`、`PowerChannelLimitPolicy.cs:8-44`、`OptionsCapabilityPolicy.cs:29-60` |
| 诊断 | — | 只读原始捕获：NVVDD clamp、热目标、RTX50 rails、VF 裸表 | 新:1641,4712,4816；`NvVfApi.cs:146-193` |
| VF 编辑 | — | `RampUpperVoltagePoints`（>1100 mV 上段斜坡） | `VfCurveEditingPolicy.cs:63-100` |

未变：`0x0733E009` VF SET（`NvVfApi.cs:228`）、`0xE440B867` BoostLock 780 B v2（`NvApiSource.cs:1204`）、`0xCBFF71D0/0xEF3D20EA` TopRels、Boost/VoltBoost、风扇 `0x814B209F/0xA58971A5`、ThermChannel、I2CReadEx。

### 4.3 对 nvapi-rs 的落点

- **已注册零封装**：`0x33AB0353/0x17695269`（仅 nvid.rs 注册表，无 sys 结构）——xOCD 给出了完整几何（0x10528、通道 mask/值/命令偏移），值得补 sys 类型+安全封装：这是 Blackwell 功率抬顶的**回退通道**。
- **几何扩展候选**：`0x67F31384` INFO 2,727,984 B/0x2BA030 与 347,124 B/0xF4BF4；PwrPolicies Control Modern 2,393,824 B/0x2786E0、Legacy 307,376 B/0x5B0B0——与我们既有「同 ID 多布局」警告一致，dispatch 应把这些尺寸并入。
- 已封装无需动：`0xFA579A0F`（`src/gpu.rs:1363`）、`0x57F7CAAC`（`sys/gpu/mod.rs:867`）。
- 未采用面：`0x3CC2D181/0xEB44E8AA`（FanCooler）与 `0xA38ACF9D` VoltVoltDevices 两版 xOCD 均未用，本次 diff 无交叉验证价值。

### 4.4 注意（子代理告警）

- 戳是**不透明版本阶梯**：header version 高字≠buffer 尺寸高字（如戳 0x2BA030 vs 尺寸 0x29A030），不能对全族假设 `(ver<<16)|size`——与 `nvapi-struct-magic-idioms` 记忆一致。
- FE/F8 命令语义来自租约/读回代码推断（非文档）；通道编号来自发现的 policy channel（中置信）。
- 真正的能力改动在 `ExtendedLimits` 内核面（§2），NVAPI 在此只是「端口与回退」——再次印证：**抬顶不可由 NVAPI 面达成**。

## 5. 与 nvapi-rs 的对照结论

| xOCD 2.0.0 能力 | nvapi-rs 现状 | 差距性质 |
|---|---|---|
| ExtendedLimits 超顶（>125% 功率、GDDR7 offset >+3000 MHz） | 不可达（用户态） | **结构性**：需内核/物理内存补丁（pmxdrv.sys 路线或 PawnIO 类），NVAPI 面无法表达 |
| 板卡策略 GET/SET `0x70916171/0xAD95F5ED`、功率命令 `0x33AB0353/0x17695269` | 均已注册（命令族无 sys 封装） | 仅差封装；几何已由 xOCD 给出（§4.3），且封了也改不动顶——只差「内核改顶」 |
| 时钟微调 XBAR/SYS/NVD offset | 已有域控制（0xD14B69CF 等） | 语义已覆盖；xOCD 的 RMW 单字段+回滚纪律值得对照 |
| V/F 上段斜坡 / 裸表 dump | 未封装 | 可低成本补（读 0x21537AD4/0x23F1B133 已有；斜坡=纯算法） |
| RetainedLimits/DPAPI 回滚胶囊 | 无 | 设计可借鉴：跨启动失效 + 失败闭合 + 认证胶囊 |
| A/B/A 举证协议与免责口径 | 无 | **方法论可借鉴**（尤其 readback≠生效 的显式声明） |
| Temps/Clocks 会话 min/max/avg 统计 | CLI 无 | 可低成本补（纯统计） |
| TDR EventID 4101 查询 | 无 | 可低成本补（wevtapi；Linux 侧对照 Xid 台账） |

## 6. 待实机验证清单（只读优先）

1. 本机（P100/470 等旧卡）：2.0.0 是否仍能起、`--status`/兼容性报告是否正常（ExtendedLimits 门控应全部 unavailable，走 fail-closed）。
2. 4060L/2070：跑 `--compare-tweaks` 三连与 XbarDiagnosticReport，核对 XBAR/SYS/NVD 列是否与我们的域表一致（NVD 在我们侧是 Video 别名）。
3. 有 RTX 30/40/50 实机时：扩展限制开关启用前后对照 `0x70916171` INFO 读回与 nvidia-smi 功率墙（验证「发布 vs 执行」分离）。
4. 内核件观察：安装后确认服务 `xOCD_ExtendedPower` 的加载/卸载与 `\\.\pmxdrv` 设备生命周期；退出后确认服务已删、策略回读为默认。

## 7. 来源与置信度

- 高：产物哈希/同一性、文件级改动统计、PMX 驱动身份与 IOCTL、生成门控与上限公式、保留限制/回滚胶囊语义、UI 能力清单（均有二进制或代码直证）。
- 中：Ada/Ampere 具体瓦数结果（硬编码已验证表，未跨机复核）；「>+3000 MHz 未验证」是 xOCD 自述而非独立裁决。
- 低：无（本节所有条目均为 2.0.0 静态证据；E-matrix 式实机探针列于 §6）。
