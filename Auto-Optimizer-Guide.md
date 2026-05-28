# Auto-Optimizer Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

auto-optimizer is the Rust CLI core of NVOC, controlling NVIDIA GPUs through NVAPI/NVML interfaces.

### Basic Usage

```
nvoc-auto-optimizer.exe [global options] <subcommand> [subcommand options]
```

#### Global Options

| Option | Short | Description |
|---|---|---|
| `--gpu <GPU_ID>` | `-g` | Target GPU (decimal/hex/index), can be specified multiple times. Default: all GPUs |
| `--output-format <FMT>` | `-O` | `human` (default) or `json` |

### Commands

#### info — GPU Details

Displays model, codename, performance state, power limits, sensors, etc.

```bash
nvoc-auto-optimizer.exe info
nvoc-auto-optimizer.exe -O json info -o gpu_info
```

#### list — List All GPUs

```bash
nvoc-auto-optimizer.exe list
```

#### status — GPU Runtime Status

```bash
nvoc-auto-optimizer.exe status
nvoc-auto-optimizer.exe status --all
nvoc-auto-optimizer.exe status --monitor 2.0   # refresh every 2 seconds
```

| Option | Short | Default | Description |
|---|---|---|---|
| `--all` | `-a` | — | Show all information |
| `--clocks <on\|off>` | `-c` | `on` | Clock frequencies |
| `--coolers <on\|off>` | `-C` | `off` | Fan information |
| `--sensors <on\|off>` | `-S` | `off` | Temperature sensors |
| `--vfp <on\|off>` | `-v` | `off` | Current VFP curve values |
| `--pstates <on\|off>` | `-P` | `off` | P-State configuration |
| `--monitor <sec>` | `-m` | — | Continuous monitoring |

#### get — Current Overclock Settings

Displays VFP offsets, P-State offsets, power limits, frequency boundaries, and detailed NVML state.

```bash
nvoc-auto-optimizer.exe get
```

#### reset — Restore Default Settings

```bash
# Reset all
nvoc-auto-optimizer.exe reset

# Reset specific items
nvoc-auto-optimizer.exe reset voltage-boost power nvapi-cooler

# Reset VFP, clear core frequency offsets only
nvoc-auto-optimizer.exe reset vfp --vfp-domain core

# Reset NVML fan
nvoc-auto-optimizer.exe reset nvml-cooler
```

**Resettable items**: `voltage-boost` · `thermal` · `power` · `nvapi-cooler` · `nvml-cooler` · `vfp` · `lock` · `pstate` · `overvolt`

#### set — Overclock Settings

##### set nvml-cooler — Fan Control

```bash
nvoc-auto-optimizer.exe set nvml-cooler --id 1 --policy manual --level 60
```

##### set nvapi — NVAPI Interface Settings

```bash
nvoc-auto-optimizer.exe set nvapi --voltage-boost 100 --thermal-limit 90
nvoc-auto-optimizer.exe set nvapi --core-offset 150000 --mem-offset 500000
nvoc-auto-optimizer.exe set nvapi --locked-voltage 68
nvoc-auto-optimizer.exe set nvapi --locked-core-clocks 210 2100
```

##### set nvml — NVML Interface Settings

```bash
nvoc-auto-optimizer.exe set nvml --core-offset 150 --mem-offset 1000
nvoc-auto-optimizer.exe set nvml -P 350
```

##### set nvapi-cooler — NVAPI Fan Control

```bash
nvoc-auto-optimizer.exe set nvapi-cooler --id 1 --policy continuous --level 60
```

##### set vfp — V-F Curve Operations

| Subcommand | Description |
|---|---|
| `set vfp export <file>` | Export current VFP curve as CSV |
| `set vfp import <file>` | Import curve from CSV to GPU |
| `set vfp autoscan` | Execute autoscan (see [[Autoscan-Workflow]]) |
| `set vfp autoscan_legacy` | Legacy GPU global offset scan |
| `set vfp fix_result` | Light/heavy load compensation post-processing |
| `set vfp single_point_adj` | Manual single-point frequency offset adjustment |
| `set vfp sync_mem_pstate_as_p0` | Sync memory P-State to P0 |

### Output Format

`-O human` (default) provides human-readable tables; `-O json` outputs structured JSON, usable with `-o <prefix>` to save to files.

---

<a id="chinese"></a>

## 中文

auto-optimizer 是 NVOC 的 Rust CLI 核心，通过 NVAPI/NVML 接口控制 NVIDIA GPU。

### 基本用法

```
nvoc-auto-optimizer.exe [全局参数] <子命令> [子命令参数]
```

#### 全局参数

| 参数 | 简写 | 说明 |
|---|---|---|
| `--gpu <GPU_ID>` | `-g` | 目标 GPU（十进制/十六进制/序号），可多次指定。缺省操作所有 GPU |
| `--output-format <FMT>` | `-O` | `human`（默认）或 `json` |

### 命令一览

#### info — GPU 详细信息

显示型号、代号、性能状态、功耗限制、传感器等。

```bash
nvoc-auto-optimizer.exe info
nvoc-auto-optimizer.exe -O json info -o gpu_info
```

#### list — 列出所有 GPU

```bash
nvoc-auto-optimizer.exe list
```

#### status — GPU 运行状态

```bash
nvoc-auto-optimizer.exe status
nvoc-auto-optimizer.exe status --all
nvoc-auto-optimizer.exe status --monitor 2.0   # 每 2 秒刷新
```

| 参数 | 简写 | 默认 | 说明 |
|---|---|---|---|
| `--all` | `-a` | — | 显示全部信息 |
| `--clocks <on\|off>` | `-c` | `on` | 时钟频率 |
| `--coolers <on\|off>` | `-C` | `off` | 风扇信息 |
| `--sensors <on\|off>` | `-S` | `off` | 温度传感器 |
| `--vfp <on\|off>` | `-v` | `off` | VFP 曲线当前值 |
| `--pstates <on\|off>` | `-P` | `off` | P-State 配置 |
| `--monitor <秒>` | `-m` | — | 持续监控 |

#### get — 当前超频设置

显示 VFP 偏移量、P-State 偏移、功耗墙、频率边界等详细 NVML 状态。

```bash
nvoc-auto-optimizer.exe get
```

#### reset — 恢复默认设置

```bash
# 重置全部
nvoc-auto-optimizer.exe reset

# 重置指定项
nvoc-auto-optimizer.exe reset voltage-boost power nvapi-cooler

# 重置 VFP，仅清除核心频率偏移
nvoc-auto-optimizer.exe reset vfp --vfp-domain core

# 重置 NVML 风扇
nvoc-auto-optimizer.exe reset nvml-cooler
```

**可重置项**：`voltage-boost` · `thermal` · `power` · `nvapi-cooler` · `nvml-cooler` · `vfp` · `lock` · `pstate` · `overvolt`

#### set — 超频设置

##### set nvml-cooler — 风扇控制

```bash
nvoc-auto-optimizer.exe set nvml-cooler --id 1 --policy manual --level 60
```

##### set nvapi — NVAPI 接口设置

```bash
nvoc-auto-optimizer.exe set nvapi --voltage-boost 100 --thermal-limit 90
nvoc-auto-optimizer.exe set nvapi --core-offset 150000 --mem-offset 500000
nvoc-auto-optimizer.exe set nvapi --locked-voltage 68
nvoc-auto-optimizer.exe set nvapi --locked-core-clocks 210 2100
```

##### set nvml — NVML 接口设置

```bash
nvoc-auto-optimizer.exe set nvml --core-offset 150 --mem-offset 1000
nvoc-auto-optimizer.exe set nvml -P 350
```

##### set nvapi-cooler — NVAPI 风扇控制

```bash
nvoc-auto-optimizer.exe set nvapi-cooler --id 1 --policy continuous --level 60
```

##### set vfp — V-F 曲线操作

| 子命令 | 说明 |
|---|---|
| `set vfp export <文件>` | 导出当前 VFP 曲线为 CSV |
| `set vfp import <文件>` | 从 CSV 导入曲线到 GPU |
| `set vfp autoscan` | 执行自动扫描（详见 [[Autoscan-Workflow]]） |
| `set vfp autoscan_legacy` | Legacy GPU 全局偏移扫描 |
| `set vfp fix_result` | 轻重载补偿后处理 |
| `set vfp single_point_adj` | 手动调整单点频率偏移 |
| `set vfp sync_mem_pstate_as_p0` | 同步显存 P-State 到 P0 |

### 输出格式

`-O human`（默认）提供人类可读表格；`-O json` 输出结构化 JSON，可配合 `-o <前缀>` 保存到文件。
