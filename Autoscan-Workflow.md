# Autoscan Workflow

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Autoscan is NVOC's core feature: it automatically probes each GPU voltage point's stable overclock limit and produces an optimized V-F curve. Since the stressor-bundling rework (#245), the whole pipeline is driven by a single command — `nvoc-auto-optimizer optimize` — with resumable state (see [[Auto-Optimizer-Guide]] for the full command inventory).

### Theory

> For complete theoretical foundations (V-F curves, Loadline mechanism, power/thermal/voltage wall constraints, signal integrity, etc.), see [[Theory]].

#### What is a V-F Curve?

Starting with Pascal (10-series), NVIDIA introduced GPU Boost 3.0, whose core is a voltage-frequency (VFP) lookup table. The factory curve is conservatively calibrated — stable limits vary significantly across individual silicon samples.

#### The Essence of Overclocking

Apply a positive frequency offset (`KilohertzDelta`) to each voltage point in the VFP table. Excessive offsets cause GPU TDR recovery or crashes. Autoscan's goal: **find the maximum stable frequency offset at each voltage point**.

### Quick Start

Run as **Administrator**:

```bat
nvoc-auto-optimizer.exe optimize                      :: standard scan (RTX 20 series and above)
nvoc-auto-optimizer.exe optimize --mode ultrafast     :: 4 key points + interpolation
nvoc-auto-optimizer.exe optimize --mode legacy        :: GTX 9 series / Maxwell (global P0 offset)
nvoc-auto-optimizer.exe optimize --fresh              :: discard resume state and start over
```

`optimize` auto-executes: select GPU → workspace (`GPUScan-<UUID>`) → baseline resets → export factory curve → resumable autoscan → `fix-vfp-result` → import → final export snapshot. Non-interactive runs need `--yes`.

> The old `start.bat` / `start_ultrafast.bat` / `start_legacy.bat` wrappers were removed when the stressor was bundled into the optimizer (#245). Passing `1` to clear logs is likewise replaced by `--fresh`.

### End-to-End Pipeline

#### Phase 0: Preparation

Read-only checks first; treat overclocking writes as high risk ([[Safety-and-Recovery]]).

1. Identify the GPU and generation: `nvoc-cli get-info` / `nvoc-cli get-gpu-list`
2. Establish a clean baseline: no other GPU-load programs; laptops plugged in; room temperature 20–25°C; core temp should stay below ~82°C during the scan
3. Confirm power headroom — the scan maxes out TDP and the temp limit on desktop cards
4. Prepare the recovery strategy **before** starting: TDR registry settings (`GpuTdrRecovery.reg`), the startup-folder relaunch entry (`test/registerStartup.bat`) for 50-series aggressive recovery, or `systemd/nvoc-vfp.service` + `test/linux_oc_recover.sh` on Linux

#### Phase 1: Workspace, Confirmation, Baseline Resets

`optimize` picks one GPU (interactive prompt or `--gpu`; the GPU must expose a UUID), prints the safety warning (confirm with `yes` or `--yes`), creates `GPUScan-<UUID>/`, and resets the GPU to a clean baseline:

1. Reset P-State global frequency offsets (`ResetPstateGlobalFreqOffset`)
2. Reset all public V-F table offsets (`reset-vfp --vfp-domain all` equivalent)
3. Reset the GPC voltage lock (`ResetPublicVftableGpcLock`)

#### Phase 2: Export the Initial VFP

If `vfp-init.csv` does not exist yet, `export-vfp` runs the bundled stressor's `dynamic-export` profile for ~45 s under load and saves the factory curve with the columns:

`voltage, frequency, delta, default_frequency, default_frequency_load, margin, margin_bin` (comma-delimited CSV; tab-delimited support was removed in #217).

This snapshot is the reference for `fix-vfp-result` and for ultrafast key-point detection.

#### Phase 3: Unlock and Probe the Voltage Range

The scan probes the GPU's reachable voltage range (`ProbeVoltageLimits`) — upper bound where the curve flattens, lower bound of usable voltage — and records a structured `voltage_range` event (with per-GPU min/max voltages) into `vfp.jsonl`. On resume this phase is skipped by reading the log.

#### Phase 4: Scan Loop (with the Bundled Stressor)

Before the first point, the autoscan profile is applied (desktop cards only; laptops skip it): VDDQ/VoltRails voltage boost to +100% (legacy Maxwell cards get a P0 base-voltage delta instead), TDP to max, temp limit to max (TDP limit removed), cooler levels raised, a small memory OC applied, and memory P-State synced to P0. If the driver does not report the TDP/temp-limit tables, the optimizer refuses to fabricate values and reports them as null with an explicit error (missing limits are skipped, never guessed).

For each voltage point (every 3rd point in standard mode, 4 key points in ultrafast mode):

1. **Voltage lock** at the point (`SetGpcVoltLock`); on Optimus laptops (30/50 series mobile), a `MinLoadPulse` minimal Vulkan load wakes the power-gated GPU first
2. **Short test** — two-stage binary search with exponential stepping (`2^n ×` minimum step) to converge quickly
3. **Long test** — single-step endurance verification at twice the short-test duration
4. **Frequency fluctuation** is applied continuously during each test (periodic up/down offsets plus live V/F checks) to expose points that pass statically but fail on dynamic load transitions; a watchdog kills the stressor if it outlives the time budget
5. The point's result is appended to `vfp.jsonl` (`test_result` records) and `vfp-tem.csv` in real time; intermediate points are linearly interpolated in standard mode

The stressor is the **bundled CUDA worker** (`cli-stressor-cuda-rs` compiled into the optimizer binary, executed as an isolated child process). Each invocation runs for a fixed per-phase duration; exit code `0` = pass, non-zero = fail.

##### Stressor Selection

- **CUDA-Rust worker (default)** — the real validation load; also what the scan itself uses. Integer workloads (int8/16/32) plus GEMM/Vulkan mixed loads were added in #246 to close "fake pass" gaps at low frequency ranges.
- **OpenCL loads** are only a first-pass smoke check. They are *not* sufficient as the final acceptance gate: OpenCL-only passes can accept offsets above the hardware's true limit. Treat autoscan output as provisional until revalidated — see [[Stress-Testing]].

#### Phase 5: Acceptance Criteria

A point passes only when **all** of these hold:

- Stressor process exits with code `0`
- No target-GPU FECS exception (>3 forces fail) or TDR (>6 forces fail) events in the Windows Event Log during the run; any new non-critical GPU event for the target also fails the run (native `EvtQuery` polling every ~3 s, #259). On Linux, new `NVRM: Xid` events in `dmesg` fail the run
- In-test V/F checks stay precise; thermal/power capping above ~30% of checks raises a warning
- Critical event bursts trigger an automatic PnP disable/enable recovery cycle (`test/windows_oc_pnp_recover.ps1`) before continuing

#### Resuming an Interrupted Scan

Resume state lives entirely in `vfp.jsonl` (schema-versioned, one JSON event per line):

| Event | Role on resume |
|---|---|
| `voltage_range` | Skips Phase 3 |
| `scan_mode` | Restores normal vs. ultrafast mode |
| `key_points` | Restores the ultrafast 4-point selection |
| `test_result` | Restores the binary-search bounds (`last succeeded` / `last failed` deltas); a pending (unfinished) record counts as a failure, so a crash mid-test backtracks safely |
| `point_finished` | Continues from the next point |
| `scan_completed` | Marks the scan as done (no resume data) |

Whether the scan exited normally, was interrupted with Ctrl+C, or the machine crashed/rebooted — **just rerun `nvoc-auto-optimizer optimize`** and it continues from the exact point, including converged search bounds. After finishing a point, the resumed bounds are shifted by one safe-elasticity step for extra margin.

If the log contains corrupt records (e.g. a torn write during a crash), the optimizer recovers all valid records, then asks for interactive confirmation (`y/N`) before resuming from recovered state; non-interactive runs refuse to resume from a corrupt log. Use `optimize --fresh` to discard the log and `vfp-tem.csv` and start from scratch.

#### Phase 6: fix_result Post-Processing

After the scan, `optimize` runs `fix-vfp-result -m 1`. Since Pascal, V-F curves shift slightly between light and heavy load (similar to CPU Load-Line Calibration). The fix uses each point's exported `margin_bin` to conservatively lower the scanned delta — `margin_bin > 5`: subtract `(3 + m)` bins; `|margin_bin| < 2`: subtract `m` bins; otherwise `(|margin_bin| + m)` bins, where `m = 1` by default (`-m`). In ultrafast mode the 4 key points are first linearly interpolated (with extra safety reduction around a 50-series Max-Q step). Output: `vfp.csv`, plus a printed "SP score".

#### Phase 7: Import and Apply

`optimize` imports `vfp.csv` into the GPU (the internal `import-vfp` path) and exports the confirmation snapshot `vfp-final.csv`. For manual application of a curve file, use the CLI:

```bash
nvoc-cli set-public-vftable-point-offset --import-csv .\GPUScan-xxxx\vfp.csv
```

Overclock settings do not persist across reboots — re-import after every boot (Windows startup entry or `systemd/nvoc-vfp.service` on Linux can automate this).

#### Phase 8: Validation with Stress Testing

The scanned curve reflects the bundled CUDA worker's pass criterion. Before daily use, validate with heavier real workloads (games, renderers, dedicated [[Stress-Testing]] suites). If a load-type instability appears, rerun `fix-vfp-result` with a larger `-m` (e.g. `-m 2`) and re-import.

#### Phase 9: Recovery

- Both the success and error exit paths automatically reset VFP offsets (`all` domains), the GPC voltage lock, and both graphics/memory frequency locks; the error path additionally forces all fans to 100%
- **TDR strategy**: traditional mode waits for Windows TDR to auto-recover the driver (most GPUs); some RTX 50-series drivers cannot auto-recover, so the aggressive scheme force-reboots via a deliberate BSOD and resumes from the breakpoint through the startup-folder relaunch. `GpuTdrRecovery.reg` raises `TdrDelay`/`TdrDdiDelay` and removes TDR count/time limits to make recovery reliable. The `-b traditional|aggressive` override is still accepted on `autoscan-vfp`, but recovery handling is currently generation-driven
- If the system freezes for more than ~3 minutes, force a power-off reboot and rerun `optimize`; the JSONL resume logic picks up where it left off
- On Linux, exit GPU-using programs (including the GUI), run `test/linux_oc_recover.sh`, and reboot if deadlocked; Linux recovery tolerance is generally lower than Windows TDR — save your work

See [[Safety-and-Recovery]] for the full recovery playbook.

### Ultrafast Mode

Core assumption: under the factory curve, overclock headroom decreases monotonically with voltage, so only 4 key points are tested and the rest are interpolated in `fix-vfp-result`.

**RTX 50 series (Max-Q step)**: the factory curve contains a frequency step whose position differs between light and heavy load. The 4 key points are detected from `vfp-init.csv` dynamic columns: max static-frequency jump (p1), max loaded-frequency jump (p2), first `margin_bin` transition from 0 to negative (p3), and the most negative `margin_bin` (p4), sorted and clamped into the scan range.

**Other GPUs** (30/40 series etc.): the 4 points are the even quarter divisions of the scan range.

### Legacy GPU Path (Maxwell / Volta)

GTX 9 series (Maxwell) and Volta-class cards do not support per-point VFP writes. `optimize --mode legacy` (or `autoscan-vfp-legacy`) binary-searches a single global P0 graphics offset with the same short/long stress-test machinery, then restores stock. No ultrafast interpolation, memory segment scan, or per-point `fix_result` applies. Generation applicability: [[GPU-Support-Matrix]].

### Work Files

All paths are relative to the workspace (`GPUScan-<UUID>/` for `optimize`; standalone subcommands default to `./ws/`).

| File | Purpose |
|---|---|
| `vfp.jsonl` | Structured scan log; core basis for breakpoint resume (deleting it restarts the scan) |
| `vfp-init.csv` | Factory curve snapshot (dynamic, with margin columns); reference for fix_result |
| `vfp-tem.csv` | autoscan per-point results, written in real time; input to fix_result |
| `vfp.csv` | fix_result output; the curve to import |
| `vfp-final.csv` | Confirmation snapshot exported after import |

### Environment Recommendations

- Room temperature 20–25°C
- Ensure good GPU cooling; use stands for laptops
- Core temp > 82°C during the scan warrants a cooling check
- Close other GPU-load programs (especially during the dynamic `vfp-init.csv` export)
- Ensure adequate power (laptops must be plugged in)
- **BSOD / black screens during the scan are expected** — the tool recovers and resumes automatically

### Disclaimer

- Overclocking runs the GPU beyond factory spec; system instability risk exists
- Crashes and BSODs during scanning are by design and will not permanently damage the GPU
- Overclock settings are not persisted across reboots; re-import is required
- This tool is not responsible for hardware damage or data loss from improper use

---

<a id="chinese"></a>

## 中文

Autoscan 是 NVOC 的核心功能：自动探测 GPU 每个电压点的稳定超频上限，生成最优 V-F 曲线。自压力测试内置化重构（#245）后，整条流水线由单条命令 —— `nvoc-auto-optimizer optimize` —— 驱动，且支持断点恢复（完整命令清单见 [[Auto-Optimizer-Guide]]）。

### 原理

> 完整的理论基础（V-F 曲线、Loadline 机制、功耗墙/温度墙/电压墙约束、信号完整性等）请参阅 [[Theory]]。

#### 什么是 V-F 曲线？

NVIDIA 从 Pascal（10 系）起引入 GPU Boost 3.0，核心是一张电压-频率（VFP）查找表。出厂曲线是保守标定，不同硅片的实际稳定极限差异很大。

#### 超频的本质

对 VFP 表中每个电压点施加正频率偏移（`KilohertzDelta`）。偏移过大时 GPU 会触发 TDR 恢复或崩溃。autoscan 的目标：**找出每个电压点能稳定通过压力测试的最大频率偏移**。

### 快速开始

以**管理员身份**运行：

```bat
nvoc-auto-optimizer.exe optimize                      :: 标准扫描（RTX 20 系及以上）
nvoc-auto-optimizer.exe optimize --mode ultrafast     :: 4 个关键点 + 插值
nvoc-auto-optimizer.exe optimize --mode legacy        :: GTX 9 系 / Maxwell（全局 P0 偏移）
nvoc-auto-optimizer.exe optimize --fresh              :: 丢弃续扫状态，从头开始
```

`optimize` 自动执行：选择 GPU → 工作目录（`GPUScan-<UUID>`）→ 基线重置 → 导出出厂曲线 → 可续扫 autoscan → `fix-vfp-result` → 导入 → 最终导出快照。非交互运行需 `--yes`。

> 旧 `start.bat` / `start_ultrafast.bat` / `start_legacy.bat` 包装脚本已随压力测试内置化（#245）移除。传参 `1` 清空日志的做法也由 `--fresh` 取代。

### 端到端流水线

#### 阶段 0：准备

先做只读检查；超频写入属高风险操作（[[Safety-and-Recovery]]）。

1. 识别 GPU 与世代：`nvoc-cli get-info` / `nvoc-cli get-gpu-list`
2. 建立干净基线：关闭其他 GPU 负载程序；笔记本接入外接电源；室温 20–25°C；扫描期间核心温度应保持在 ~82°C 以下
3. 确认功耗余量 —— 桌面卡扫描会把 TDP 和温度墙拉满
4. **开始前**准备好恢复策略：TDR 注册表（`GpuTdrRecovery.reg`）、50 系激进恢复的开机自启项（`test/registerStartup.bat`），Linux 上的 `systemd/nvoc-vfp.service` + `test/linux_oc_recover.sh`

#### 阶段 1：工作目录、确认与基线重置

`optimize` 选择一块 GPU（交互选择或 `--gpu`；GPU 必须能提供 UUID），打印安全警告（输入 `yes` 或 `--yes` 确认），创建 `GPUScan-<UUID>/`，并将 GPU 重置到干净基线：

1. 重置 P-State 全局频率偏移（`ResetPstateGlobalFreqOffset`）
2. 重置全部公开 V-F 表偏移（等价 `reset-vfp --vfp-domain all`）
3. 重置 GPC 电压锁（`ResetPublicVftableGpcLock`）

#### 阶段 2：导出初始 VFP

若 `vfp-init.csv` 尚不存在，`export-vfp` 会在负载下运行内置压力测试的 `dynamic-export` 档位约 45 秒，保存带下列列的出厂曲线：

`voltage, frequency, delta, default_frequency, default_frequency_load, margin, margin_bin`（逗号分隔 CSV；#217 已移除 tab 分隔支持）。

该快照是 `fix-vfp-result` 与 ultrafast 关键点检测的参考基准。

#### 阶段 3：解锁并探测电压范围

扫描探测 GPU 可达的电压范围（`ProbeVoltageLimits`）—— 上限到曲线平坦区、下限到最低可用电压 —— 并将结构化 `voltage_range` 事件（含各 GPU 最小/最大电压）记入 `vfp.jsonl`。续扫时直接读日志跳过此阶段。

#### 阶段 4：扫描循环（使用内置压力测试）

首个测试点之前会应用 autoscan 档位（仅桌面卡；笔记本跳过）：VDDQ/VoltRails 电压提升 +100%（legacy Maxwell 卡改为 P0 基础电压增量）、TDP 拉满、温度墙拉满（同时移除 TDP 限制）、提高风扇档位、施加小幅显存超频并同步显存 P-State 到 P0。若驱动未上报 TDP/温度墙表，优化器拒绝伪造数值，而是将其上报为 null 并给出明确错误（缺失限值会被跳过，绝不猜测）。

对每个电压点（标准模式每隔 3 个点取一个，ultrafast 模式取 4 个关键点）：

1. **电压锁定**到该点（`SetGpcVoltLock`）；Optimus 笔记本（30/50 系移动端）先以 `MinLoadPulse` 极小 Vulkan 负载唤醒休眠的 GPU
2. **短测试** —— 双阶段二分搜索，指数步进（`2^n ×` 最小步进）快速收敛
3. **长测试** —— 以短测试结果为基础做单步耐久验证，时长为短测试两倍
4. 每轮测试期间持续施加**频率涨落**（周期性升降偏移加实时 V/F 校验），暴露"静态能过、动态切换就崩"的点；守护计时器超时则强杀压力测试进程
5. 该点结果实时追加到 `vfp.jsonl`（`test_result` 记录）与 `vfp-tem.csv`；标准模式下中间点做线性插值

压力测试为**内置 CUDA worker**（`cli-stressor-cuda-rs` 编译进优化器二进制，作为隔离子进程执行）。每次调用运行固定时长；退出码 `0` = 通过，非零 = 失败。

##### 压力测试选择

- **CUDA-Rust worker（默认）** —— 真实验证负载，也是扫描本身使用的负载。#246 新增整数负载（int8/16/32）与 GEMM/Vulkan 混合负载，弥补低频段的"假通过"缺口。
- **OpenCL 负载**只配做第一轮粗筛。它**不足以**作为最终验收判据：仅 OpenCL 通过可能接受超出硬件真实上限的偏移。在重新验证前，请将 autoscan 结果视为临时值 —— 见 [[Stress-Testing]]。

#### 阶段 5：验收判据

一个点只有在**全部**满足以下条件时才算通过：

- 压力测试进程以退出码 `0` 结束
- 运行期间 Windows 事件日志无目标 GPU 的 FECS 异常（>3 强制失败）或 TDR（>6 强制失败）事件；目标 GPU 出现任何新的非关键 GPU 事件同样判失败（原生 `EvtQuery` 约 3 秒轮询一次，#259）。Linux 上 `dmesg` 出现新的 `NVRM: Xid` 事件判失败
- 测试中 V/F 校验保持精确；温度/功耗压制占比超过约 30% 会给出警告
- 关键事件爆发会先触发自动 PnP 禁用/启用恢复循环（`test/windows_oc_pnp_recover.ps1`）再继续

#### 恢复被中断的扫描

续扫状态完全存于 `vfp.jsonl`（带 schema 版本，每行一条 JSON 事件）：

| 事件 | 续扫时的作用 |
|---|---|
| `voltage_range` | 跳过阶段 3 |
| `scan_mode` | 恢复普通/ultrafast 模式 |
| `key_points` | 恢复 ultrafast 的 4 点选择 |
| `test_result` | 恢复二分搜索边界（`last succeeded` / `last failed` 偏移）；pending（未完成）记录按失败计，因此测试中途崩溃也会安全回退 |
| `point_finished` | 从下一个点继续 |
| `scan_completed` | 标记扫描已完成（无续扫数据） |

无论是正常退出、Ctrl+C 中断，还是崩溃/重启 —— **只需重新运行 `nvoc-auto-optimizer optimize`**，它会从确切的点继续，包括已收敛的搜索边界。完成一个点后续扫时，恢复的边界会额外移动一个安全弹性步进作为余量。

若日志含损坏记录（例如崩溃时的半行写入），优化器会恢复全部有效记录，并在从恢复状态续扫前请求交互确认（`y/N`）；非交互运行拒绝从损坏日志续扫。使用 `optimize --fresh` 丢弃日志与 `vfp-tem.csv`、从头开始。

#### 阶段 6：fix_result 后处理

扫描完成后，`optimize` 运行 `fix-vfp-result -m 1`。自 Pascal 起，V-F 曲线在轻重载间存在微小偏移（类似 CPU 的 Load-Line Calibration）。fix 依据每点导出的 `margin_bin` 对扫描偏移做保守下压 —— `margin_bin > 5`：下压 `(3 + m)` 个 bin；`|margin_bin| < 2`：下压 `m` 个 bin；其余下压 `(|margin_bin| + m)` 个 bin，其中 `m` 默认为 1（`-m`）。ultrafast 模式先对 4 个关键点线性插值（50 系 Max-Q 阶梯附近额外加大安全降幅）。输出 `vfp.csv`，并打印"SP score"。

#### 阶段 7：导入与应用

`optimize` 将 `vfp.csv` 写入 GPU（内部 `import-vfp` 路径）并导出确认快照 `vfp-final.csv`。手动应用曲线文件请用 CLI：

```bash
nvoc-cli set-public-vftable-point-offset --import-csv .\GPUScan-xxxx\vfp.csv
```

超频设置重启后不保留 —— 每次开机后需重新导入（Windows 自启项或 Linux 的 `systemd/nvoc-vfp.service` 可自动化）。

#### 阶段 8：压力测试验证

扫描结果反映的是内置 CUDA worker 的通过判据。日常使用前，请用更重的真实负载（游戏、渲染器、专用 [[Stress-Testing]] 套件）验证。若某类负载出现不稳，用更大的 `-m` 重跑 `fix-vfp-result`（如 `-m 2`）并重新导入。

#### 阶段 9：恢复

- 成功与错误退出路径都会自动重置 VFP 偏移（`all` 域）、GPC 电压锁和图形/显存频率锁；错误路径还会把所有风扇强制到 100%
- **TDR 策略**：传统模式等待 Windows TDR 自动恢复驱动（大多数 GPU）；部分 RTX 50 系驱动无法自动恢复，因此激进方案通过主动 BSOD 强制重启，并借助开机自启项从断点继续。`GpuTdrRecovery.reg` 提高 `TdrDelay`/`TdrDdiDelay` 并移除 TDR 次数/时间限制以保证恢复可靠。`autoscan-vfp` 仍接受 `-b traditional|aggressive` 覆写，但当前恢复处理由世代自动决定
- 若系统卡死超过约 3 分钟，请强制断电重启后重跑 `optimize`；JSONL 续扫逻辑会从中断处接续
- Linux 上请退出所有占用 GPU 的程序（含图形界面），运行 `test/linux_oc_recover.sh`，若死锁则重启；Linux 的不稳定超频恢复容忍度一般低于 Windows TDR —— 注意保存工作

完整恢复手册见 [[Safety-and-Recovery]]。

### Ultrafast 模式

核心假设：出厂曲线下各电压点的可超频幅度随电压升高单调递减，因此只测 4 个关键点，其余在 `fix-vfp-result` 中插值。

**RTX 50 系（Max-Q 阶梯）**：出厂曲线存在一个轻重载位置不同的频率台阶。4 个关键点从 `vfp-init.csv` 动态列中检测：静态频率最大跳变处（p1）、负载频率最大跳变处（p2）、`margin_bin` 首次由 0 变负处（p3）、`margin_bin` 最大负值处（p4），排序后钳制到扫描范围内。

**其他 GPU**（30/40 系等）：4 个点取扫描范围的均匀四等分。

### Legacy GPU 路径（Maxwell / Volta）

GTX 9 系（Maxwell）与 Volta 级计算卡不支持逐点 VFP 写入。`optimize --mode legacy`（或 `autoscan-vfp-legacy`）用相同的短/长压力测试机制对单一全局 P0 图形偏移做二分搜索，完成后恢复原值。不适用 ultrafast 插值、显存分段扫描与逐点 `fix_result`。适用世代见 [[GPU-Support-Matrix]]。

### 工作文件

所有路径相对于工作目录（`optimize` 为 `GPUScan-<UUID>/`；独立子命令默认 `./ws/`）。

| 文件 | 用途 |
|---|---|
| `vfp.jsonl` | 结构化扫描日志；断点续扫核心依据（删除则从头开始） |
| `vfp-init.csv` | 出厂曲线快照（动态导出，含 margin 列）；fix_result 参考基准 |
| `vfp-tem.csv` | autoscan 每点结果，实时写入；fix_result 输入 |
| `vfp.csv` | fix_result 输出；待导入的曲线 |
| `vfp-final.csv` | 导入后再次导出的确认快照 |

### 测试环境建议

- 室温 20–25°C
- 确保 GPU 散热良好；笔记本建议使用支架
- 扫描期间核心温度 > 82°C 应检查散热
- 关闭其他 GPU 负载程序（动态导出 `vfp-init.csv` 时尤其需要）
- 确保电源充足（笔记本需接入外接电源）
- **扫描期间 BSOD / 黑屏属预期现象** —— 工具会自动恢复并续扫

### 免责声明

- 超频使 GPU 超出出厂规格工作，存在系统不稳定风险
- 扫描期间崩溃和 BSOD 是设计行为，不会造成 GPU 永久损坏
- 超频设置在系统重启后不自动保留，需重新导入
- 本工具不对因使用不当导致的硬件损坏、数据丢失负责

---

*Maintained from: auto-optimizer/README.md, auto-optimizer/src/*, git log*
