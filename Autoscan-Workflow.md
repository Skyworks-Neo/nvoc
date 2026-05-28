# Autoscan Workflow

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Autoscan is NVOC's core feature, automatically scanning each GPU voltage point's stable overclock limit and producing an optimized V-F curve.

### Theory

> For complete theoretical foundations (V-F curves, Loadline mechanism, power/thermal/voltage wall constraints, signal integrity, etc.), see [[Theory]].

#### What is a V-F Curve?

Starting with Pascal (10-series), NVIDIA introduced GPU Boost 3.0, whose core is a voltage-frequency (VFP) lookup table. The factory curve is conservatively calibrated — stable limits vary significantly across individual silicon samples.

#### The Essence of Overclocking

Apply a positive frequency offset (`KilohertzDelta`) to each voltage point in the VFP table. Excessive offsets cause GPU TDR recovery or crashes. Autoscan's goal: **find the maximum stable frequency offset at each voltage point**.

### Quick Start

#### Standard Scan (RTX 20-series and above)

Run as **Administrator**:

```bat
start.bat
```

Auto-executes: detect GPU → reset curve → export factory curve → autoscan → fix_result → import final curve

#### Ultra-Fast Scan

```bat
start_ultrafast.bat
```

Scans only 4 critical voltage points, linear interpolation for the rest — significantly faster.

#### Legacy GPU Scan (GTX 9-series)

```bat
start_legacy.bat
```

Uses global P0 frequency offset scan, no VFP curve writes.

#### Re-scan (Clear Breakpoints)

```bat
start.bat 1
```

Pass `1` to clear `ws/vfp.log` and `ws/vfp-tem.csv`, starting fresh.

### Detailed Scan Workflow

#### Phase 0: Preparation

1. `info` — Print GPU info and identify generation
2. `reset pstate` — Clear P-State offsets
3. `reset vfp` — Zero all VFP curve offsets
4. `set nvapi --reset-vfp-locks` — Unlock voltage/frequency locks
5. `set vfp export` — Save factory original curve (`ws/vfp-init.csv`)

#### Phase 1: Voltage Range Detection

Starting from a preset point, progressively advance voltage locks to determine the GPU's reachable voltage range (upper and lower bounds). Results recorded in `vfp.log`; skipped on resume.

#### Phase 2: Per-Point Core Frequency Scan

Tests every 3rd point within the range (standard mode). Two-stage binary search per voltage point:

- **Short test**: Exponential stepping for fast convergence
- **Long test**: Single-step endurance verification

Periodically applies **frequency fluctuation** to simulate dynamic load changes.

#### Phase 3: Memory Frequency Scan (Optional)

Enabled with `-m` flag. Locks at the highest voltage point to scan memory overclock limits.

#### Phase 4: fix_result Post-Processing

Conservative correction based on the factory curve's `margin_bin` (light/heavy load frequency difference) to prevent instability during load transitions.

#### Phase 5: Import and Export Final Curve

```bash
nvoc-auto-optimizer.exe set vfp import ./ws/vfp.csv
nvoc-auto-optimizer.exe set vfp export ./ws/vfp-final.csv
```

### Ultrafast Mode

Assumes overclock headroom decreases monotonically with voltage, testing only 4 key points with linear interpolation.

**50-series Max-Q Steps**: Detects frequency jumps on static/load curves and margin_bin change points to precisely locate 4 key points.

**Other GPUs**: Evenly divides the scan range into quarters.

### Crash Recovery Mechanism

| Mode | Behavior | Applies To |
|---|---|---|
| **Traditional** | Wait for Windows TDR auto-recovery | Most GPUs |
| **Aggressive** | Actively trigger BSOD for forced restart, auto-resume from breakpoint | 50-series (driver bug where TDR can't auto-recover) |

```bash
set vfp autoscan -b traditional   # Force traditional mode
set vfp autoscan -b aggressive    # Force aggressive mode
```

### Breakpoint Resume

Each voltage point's scan result is written to `ws/vfp.log` in real time. Next run auto-resumes from the breakpoint. Whether normal exit, manual interruption, or crash/reboot — just run `start.bat` again.

### Work Files

| File | Purpose |
|---|---|
| `ws/vfp.log` | Scan log, core basis for breakpoint resume |
| `ws/vfp-init.csv` | Factory original curve snapshot |
| `ws/vfp-tem.csv` | Autoscan temp results, real-time per-point writes |
| `ws/vfp.csv` | fix_result output, final curve with load compensation |
| `ws/vfp-final.csv` | Confirmation snapshot after import + re-export |

### Environment Recommendations

- Room temperature 20–25°C
- Ensure good GPU cooling
- Core temp > 82°C during scan warrants a cooling check
- Close other GPU-loading programs
- Ensure adequate power (laptops must be plugged in)
- **BSOD / black screens during scan are normal** — the tool auto-recovers

### Disclaimer

- Overclocking runs GPU beyond factory spec; system instability risk exists
- Crashes and BSODs during scanning are by design and won't cause permanent GPU damage
- Overclock settings are not persisted across system restarts; re-import is required
- This tool is not responsible for hardware damage or data loss from improper use

---

<a id="chinese"></a>

## 中文

Autoscan 是 NVOC 的核心功能，自动扫描 GPU 每个电压点的稳定超频上限，生成最优 V-F 曲线。

### 原理

> 完整的理论基础（V-F 曲线、Loadline 机制、功耗墙/温度墙/电压墙约束、信号完整性等）请参阅 [[Theory]]。

#### 什么是 V-F 曲线？

NVIDIA 从 Pascal（10 系）起引入 GPU Boost 3.0，核心是一张电压-频率（VFP）查找表。出厂曲线是保守标定，不同硅片的实际稳定极限差异很大。

#### 超频的本质

对 VFP 表中每个电压点施加正频率偏移（`KilohertzDelta`）。偏移过大时 GPU 会触发 TDR 恢复或崩溃。autoscan 的目标：**找出每个电压点能稳定通过压力测试的最大频率偏移**。

### 快速开始

#### 标准扫描（RTX 20 系及以上）

以**管理员身份**运行：

```bat
start.bat
```

自动执行：检测 GPU → 重置曲线 → 导出出厂曲线 → autoscan → fix_result → 导入最终曲线

#### 超快速扫描

```bat
start_ultrafast.bat
```

仅扫描 4 个关键电压点，其余线性插值，速度显著加快。

#### Legacy GPU 扫描（GTX 9 系）

```bat
start_legacy.bat
```

使用全局 P0 频率偏移扫描，不写入 VFP 曲线。

#### 重新扫描（清除断点）

```bat
start.bat 1
```

传入 `1` 清空 `ws/vfp.log` 和 `ws/vfp-tem.csv`，从头开始。

### 扫描流程详解

#### 阶段 0：准备工作

1. `info` — 打印 GPU 信息并识别世代
2. `reset pstate` — 清零 P-State 偏移
3. `reset vfp` — VFP 曲线所有偏移归零
4. `set nvapi --reset-vfp-locks` — 解除电压/频率锁定
5. `set vfp export` — 保存出厂原始曲线（`ws/vfp-init.csv`）

#### 阶段 1：电压范围探测

从预设起始点出发，逐步推进电压锁定来确定 GPU 个体可达的电压范围（上限和下限）。结果记录到 `vfp.log`，断点续扫时跳过此阶段。

#### 阶段 2：逐点核心频率扫描

对范围内每隔 3 个点取测试点（标准模式），在每个电压点执行双阶段二分搜索：

- **短测试**：指数步进快速收敛
- **长测试**：单步耐久性验证

期间会定期施加**频率涨落**（Fluctuation），模拟动态负载变化。

#### 阶段 3：显存频率扫描（可选）

使用 `-m` 参数启用，锁定最高电压点扫描显存超频上限。

#### 阶段 4：fix_result 后处理

根据出厂曲线的 `margin_bin`（轻重载频率差异）对扫描结果做保守化修正，避免负载变化时不稳定。

#### 阶段 5：导入并导出最终曲线

```bash
nvoc-auto-optimizer.exe set vfp import ./ws/vfp.csv
nvoc-auto-optimizer.exe set vfp export ./ws/vfp-final.csv
```

### Ultrafast 模式

假设各电压点超频幅度随电压升高单调递减，只测 4 个关键点后线性插值。

**50 系 GPU 的 Max-Q 阶梯**：检测静态/负载曲线频率跳变处和 margin_bin 变化点，精确定位 4 个关键点。

**其他 GPU**：在扫描范围内均匀四等分。

### 崩溃恢复机制

| 模式 | 行为 | 适用 |
|---|---|---|
| **Traditional** | 等待 Windows TDR 自动恢复 | 大多数 GPU |
| **Aggressive** | 主动触发 BSOD 强制重启，配合开机自启从断点继续 | 50 系（TDR 无法自动恢复的驱动 Bug） |

```bash
set vfp autoscan -b traditional   # 强制传统模式
set vfp autoscan -b aggressive    # 强制激进模式
```

### 断点续扫

每个电压点扫描完成后实时写入 `ws/vfp.log`。下次运行时自动从断点继续。无论是正常退出、手动中断还是崩溃重启，只需再次运行 `start.bat`。

### 工作文件

| 文件 | 用途 |
|---|---|
| `ws/vfp.log` | 扫描日志，断点续扫核心依据 |
| `ws/vfp-init.csv` | 出厂原始曲线快照 |
| `ws/vfp-tem.csv` | autoscan 临时结果，每点实时写入 |
| `ws/vfp.csv` | fix_result 输出，经轻重载补偿的最终曲线 |
| `ws/vfp-final.csv` | import 后再次 export 的确认快照 |

### 测试环境建议

- 室温 20–25°C
- 确保 GPU 散热良好
- 扫描期间核心温度 > 82°C 应检查散热
- 关闭其他 GPU 负载程序
- 确保电源充足（笔记本需接入外接电源）
- **扫描期间 BSOD / 黑屏是正常现象**，工具会自动恢复

### 免责声明

- 超频使 GPU 超出出厂规格工作，存在系统不稳定风险
- 扫描期间崩溃和 BSOD 是设计行为，不会造成 GPU 永久损坏
- 超频设置在系统重启后不自动保留，需重新 import
- 本工具不对因使用不当导致的硬件损坏、数据丢失负责
