# Autoscan 自动扫描流程

Autoscan 是 NVOC 的核心功能，自动扫描 GPU 每个电压点的稳定超频上限，生成最优 V-F 曲线。

## 原理

### 什么是 V-F 曲线？

NVIDIA 从 Pascal（10 系）起引入 GPU Boost 3.0，核心是一张电压-频率（VFP）查找表。出厂曲线是保守标定，不同硅片的实际稳定极限差异很大。

### 超频的本质

对 VFP 表中每个电压点施加正频率偏移（`KilohertzDelta`）。偏移过大时 GPU 会触发 TDR 恢复或崩溃。autoscan 的目标：**找出每个电压点能稳定通过压力测试的最大频率偏移**。

## 快速开始

### 标准扫描（RTX 20 系及以上）

以**管理员身份**运行：

```bat
start.bat
```

自动执行：检测 GPU → 重置曲线 → 导出出厂曲线 → autoscan → fix_result → 导入最终曲线

### 超快速扫描

```bat
start_ultrafast.bat
```

仅扫描 4 个关键电压点，其余线性插值，速度显著加快。

### Legacy GPU 扫描（GTX 9 系）

```bat
start_legacy.bat
```

使用全局 P0 频率偏移扫描，不写入 VFP 曲线。

### 重新扫描（清除断点）

```bat
start.bat 1
```

传入 `1` 清空 `ws/vfp.log` 和 `ws/vfp-tem.csv`，从头开始。

## 扫描流程详解

### 阶段 0：准备工作

1. `info` — 打印 GPU 信息并识别世代
2. `reset pstate` — 清零 P-State 偏移
3. `reset vfp` — VFP 曲线所有偏移归零
4. `set nvapi --reset-vfp-locks` — 解除电压/频率锁定
5. `set vfp export` — 保存出厂原始曲线（`ws/vfp-init.csv`）

### 阶段 1：电压范围探测

从预设起始点出发，逐步推进电压锁定来确定 GPU 个体可达的电压范围（上限和下限）。结果记录到 `vfp.log`，断点续扫时跳过此阶段。

### 阶段 2：逐点核心频率扫描

对范围内每隔 3 个点取测试点（标准模式），在每个电压点执行双阶段二分搜索：

- **短测试**：指数步进快速收敛
- **长测试**：单步耐久性验证

期间会定期施加**频率涨落**（Fluctuation），模拟动态负载变化。

### 阶段 3：显存频率扫描（可选）

使用 `-m` 参数启用，锁定最高电压点扫描显存超频上限。

### 阶段 4：fix_result 后处理

根据出厂曲线的 `margin_bin`（轻重载频率差异）对扫描结果做保守化修正，避免负载变化时不稳定。

### 阶段 5：导入并导出最终曲线

```bash
nvoc-auto-optimizer.exe set vfp import ./ws/vfp.csv
nvoc-auto-optimizer.exe set vfp export ./ws/vfp-final.csv
```

## Ultrafast 模式

假设各电压点超频幅度随电压升高单调递减，只测 4 个关键点后线性插值。

**50 系 GPU 的 Max-Q 阶梯**：检测静态/负载曲线频率跳变处和 margin_bin 变化点，精确定位 4 个关键点。

**其他 GPU**：在扫描范围内均匀四等分。

## 崩溃恢复机制

| 模式 | 行为 | 适用 |
|---|---|---|
| **Traditional** | 等待 Windows TDR 自动恢复 | 大多数 GPU |
| **Aggressive** | 主动触发 BSOD 强制重启，配合开机自启从断点继续 | 50 系（TDR 无法自动恢复的驱动 Bug） |

```bash
set vfp autoscan -b traditional   # 强制传统模式
set vfp autoscan -b aggressive    # 强制激进模式
```

## 断点续扫

每个电压点扫描完成后实时写入 `ws/vfp.log`。下次运行时自动从断点继续。无论是正常退出、手动中断还是崩溃重启，只需再次运行 `start.bat`。

## 工作文件

| 文件 | 用途 |
|---|---|
| `ws/vfp.log` | 扫描日志，断点续扫核心依据 |
| `ws/vfp-init.csv` | 出厂原始曲线快照 |
| `ws/vfp-tem.csv` | autoscan 临时结果，每点实时写入 |
| `ws/vfp.csv` | fix_result 输出，经轻重载补偿的最终曲线 |
| `ws/vfp-final.csv` | import 后再次 export 的确认快照 |

## 测试环境建议

- 室温 20–25°C
- 确保 GPU 散热良好
- 扫描期间核心温度 > 82°C 应检查散热
- 关闭其他 GPU 负载程序
- 确保电源充足（笔记本需接入外接电源）
- **扫描期间 BSOD / 黑屏是正常现象**，工具会自动恢复

## 免责声明

- 超频使 GPU 超出出厂规格工作，存在系统不稳定风险
- 扫描期间崩溃和 BSOD 是设计行为，不会造成 GPU 永久损坏
- 超频设置在系统重启后不自动保留，需重新 import
- 本工具不对因使用不当导致的硬件损坏、数据丢失负责
