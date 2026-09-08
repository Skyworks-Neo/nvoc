# Auto-Optimizer Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

`nvoc-auto-optimizer` is the autoscan orchestration CLI core of NVOC. It owns four key workflows:

- `autoscan` — the automatic stability scan loop over core/memory frequency deltas (`autoscan-vfp`, `autoscan-vfp-legacy`, wrapped by the one-shot `optimize` workflow)
- `vfp` — V-F curve read/export/import (`export-vfp`, `export-vfp-log`, `import-vfp`, `sync-vfp-memory-pstate`)
- `reset` — return clocks to a safe baseline (`reset-vfp`)
- `fix_result` — normalize and patch scan outputs for downstream use (`fix-vfp-result`)

Direct NVAPI/NVML control (GPU discovery, live status, fan, power limits, locks, generic overclock writes) has moved to `nvoc-cli` — see [[CLI-Guide]]. This page documents only what the optimizer itself exposes today.

### Basic Usage

```
nvoc-auto-optimizer.exe [--gpu GPU_ID] [--no-color] <command> [command options]
```

A subcommand is required. Mutating commands (`reset-vfp`, `import-vfp`, `autoscan-vfp`, `autoscan-vfp-legacy`, `optimize`) require Administrator (Windows) or root (Linux). `export-vfp` and `fix-vfp-result` are read-side and work without elevation; `export-vfp-log` is fully offline.

#### Global Options

| Option | Short | Description |
|---|---|---|
| `--gpu <GPU_ID>` | `-g` | Target GPU selector; accepts decimal or hex, can be repeated. Default: all GPUs |
| `--no-color` | — | Disable ANSI color output (also honors the `NO_COLOR` environment variable) |

### Command Inventory

| Command | Elevation | Description |
|---|---|---|
| `optimize` | yes | Run the complete VFP optimization workflow (reset → export → autoscan → fix → import → final export) |
| `autoscan-vfp` | yes | Auto-scan a new VFP curve point by point |
| `autoscan-vfp-legacy` | yes | Auto-scan legacy GPUs (Maxwell/Volta-class) using a global P-State OC offset |
| `export-vfp [OUTPUT]` | no | Export the current VFP curve as CSV (dynamic load sampling) |
| `export-vfp-log` | no (offline) | Export VFP points parsed from an autoscan JSONL log |
| `import-vfp [INPUT]` | yes | **Internal optimize-workflow driver (hidden)**; user imports go through `nvoc-cli set-public-vftable-point-offset --import-csv <PATH>` |
| `fix-vfp-result` | no | Post-process autoscan results (light/heavy-load compensation) |
| `reset-vfp` | yes | Reset VFP curve offsets to zero |

#### optimize — End-to-End Workflow

```bash
nvoc-auto-optimizer.exe optimize
nvoc-auto-optimizer.exe -g 1 optimize --mode ultrafast
nvoc-auto-optimizer.exe optimize --mode legacy
nvoc-auto-optimizer.exe optimize --fresh --yes
```

| Option | Default | Description |
|---|---|---|
| `--mode <MODE>` | `standard` | `standard`, `ultrafast`, or `legacy` workflow |
| `--fresh` | off | Discard the resumable scan log (`vfp.jsonl`) and temporary results |
| `-y`, `--yes` | off | Acknowledge the safety warning without prompting |
| `--workspace <PATH>` | `GPUScan-<UUID>` | Relative per-GPU scan workspace path (absolute paths and `..` are rejected) |

Bundled-stressor builds (default `stressor-bundled` feature) additionally accept:

| Option | Description |
|---|---|
| `--stressor-profile <PROFILE>` | `auto` (default), `low-vram`, `standard`, or `40-50` |
| `--stressor-config <PATH>` | Custom stressor TOML config (conflicts with `--stressor-profile`) |

External-stressor builds (`stressor-external` without `stressor-bundled`) use `--test-exe <PATH>` / `--minload-exe <PATH>` (default `cli-stressor-cuda-rs`). Builds with both features add `--stressor-backend <bundled\|external>` (default `bundled`).

`optimize` prints a warning that V/F optimization intentionally probes unstable settings and requires an interactive `yes` (or `--yes`). It selects one GPU (prompt or `-g`), creates the workspace, then chains: reset P-State offsets → reset VFP offsets → reset GPC voltage lock → export factory curve → autoscan → `fix-vfp-result -m 1` → import → final export.

#### autoscan-vfp — Core Scan Loop

```bash
nvoc-auto-optimizer.exe autoscan-vfp
nvoc-auto-optimizer.exe autoscan-vfp -u
nvoc-auto-optimizer.exe -g 1 autoscan-vfp --stressor-profile low-vram
```

| Option | Short | Default | Description |
|---|---|---|---|
| `--log <LOG>` | `-l` | `./ws/vfp.jsonl` | Structured JSONL scan log (basis for resume) |
| `--ultrafast` | `-u` | off | Only scan 4 key points; interpolate the rest in `fix-vfp-result` |
| `-q <POINT_SEQ>` | — | `-` | Custom point sequence (`-` = automatic; kept for CLI compatibility) |
| `-o <OUTPUTCSV>` | — | `./ws/vfp-tem.csv` | Per-point result CSV, written in real time |
| `-i <INITCSV>` | — | `./ws/vfp-init.csv` | Reference factory curve CSV |
| `-m`, `--Vmem_scan_switch` | — | off | Also scan the memory OC ceiling |
| `-t <TIMEOUT_LOOPS>` | — | `30` | Kept for CLI compatibility; phase durations are currently fixed by the scanner |
| `-b <METHOD>` (`--recovery_method_switch`) | — | per GPU generation | `aggressive` or `traditional`; parsed but recovery handling is currently generation-driven |
| `--cuda-device <INDEX>` | — | derived | CUDA device ordinal for the stressor (auto-derived from a single numeric `-g`) |
| `--stressor-extra-args <ARG>...` | — | — | Extra arguments appended verbatim to every stressor invocation |
| `--stressor-profile <PROFILE>` | — | `auto` | Bundled CUDA stress profile: `auto`, `low-vram`, `standard`, `40-50` |
| `--stressor-config <PATH>` | — | — | Custom stressor TOML (overrides the profile) |
| `--stressor-backend <BACKEND>` | — | `bundled` | `bundled` worker or `external` executable (builds with both features) |

External-backend builds use `-w, --test-exe <PATH>` and `--minload-exe <PATH>` instead of the profile flags. A few voltage/frequency lock flags (`--locked-voltage`, `--locked-core-clocks`, `--locked-mem-clocks`, `--clock`, `--voltage`, `--point`, `--domain`) exist but are hidden: they are internal plumbing used by the scan loop itself.

Each stressor invocation runs the bundled CUDA worker for `5 × loops` seconds (short test 10 loops, long endurance test ×2, ultrafast +50%); a watchdog force-kills it at `15 × loops` and treats the point as failed.

#### autoscan-vfp-legacy — Maxwell/Volta Path

Same common flags as `autoscan-vfp` (`-l`, `-t`, `-b`, `--cuda-device`, `--stressor-*`), but no `--ultrafast`, `-q`, `-o`, `-m`, or `-i` — legacy GPUs only take a single global P0 graphics offset. The result is a global offset, not a per-point curve. Generation applicability: see [[GPU-Support-Matrix]].

```bash
nvoc-auto-optimizer.exe autoscan-vfp-legacy
nvoc-auto-optimizer.exe autoscan-vfp-legacy -b aggressive
```

#### export-vfp — Curve Export

```bash
nvoc-auto-optimizer.exe export-vfp .\GPUScan-xxxx\vfp-init.csv
nvoc-auto-optimizer.exe export-vfp --memory .\mem.csv
```

| Option | Short | Description |
|---|---|---|
| `<OUTPUT>` | — | Output path; `-` (default) prints the static table to stdout |
| `--quick` | `-q` | **Deprecated**: static export moved to `nvoc-cli get-public-vftable --output-csv <PATH> --domain <DOMAIN>` |
| `--nocheck` | `-n` | Skip the plausibility check of dynamic results |
| `--memory` / `--processor` / `--video` / `--undefined` | — | Export another VF table domain (mutually exclusive; default Graphics) |

The default (dynamic) export runs the bundled stressor with its `dynamic-export` profile for ~45 s, then writes comma-delimited CSV with the columns `voltage, frequency, delta, default_frequency, default_frequency_load, margin, margin_bin`. The `margin_bin` column feeds `fix-vfp-result` and ultrafast key-point detection.

#### export-vfp-log — Rebuild a Curve from the Log

```bash
nvoc-auto-optimizer.exe export-vfp-log -l .\GPUScan-xxxx\vfp.jsonl -i .\GPUScan-xxxx\vfp-init.csv -o .\from-log.csv
```

Parses finished core-scan points out of the JSONL log and writes them as CSV. Works offline (no GPU required).

#### fix-vfp-result — Margin Compensation

```bash
nvoc-auto-optimizer.exe fix-vfp-result -m 1
nvoc-auto-optimizer.exe fix-vfp-result -m 1 -u
```

| Option | Short | Default | Description |
|---|---|---|---|
| `-m <MINUS_BIN>` | — | `1` | Extra conservative bins to subtract (integer, −50..50; negative values relax) |
| `-v <TMPCSV>` | — | `./ws/vfp-tem.csv` | Input: autoscan temporary CSV |
| `-o <OUTPUTCSV>` | — | `./ws/vfp.csv` | Output: compensated final curve CSV |
| `-i <INITCSV>` | — | `./ws/vfp-init.csv` | Reference factory curve |
| `--ultrafast` | `-u` | off | Interpolate the 4 ultrafast key points first |
| `-l <VFPLOG>` | — | `./ws/vfp.jsonl` | JSONL log (key points for ultrafast interpolation) |
| `-d <DELTA_REF>` | — | `3` | Internal reference delta |

Per-point correction based on the exported `margin_bin` (with `m = -m` value, `step` = the GPU's minimum frequency step): `margin_bin > 5` subtracts `(3 + m)·step`; `|margin_bin| < 2` subtracts `m·step`; otherwise `( |margin_bin| + m )·step`. Deltas are clamped at 0 and a "SP score" (sum of final vs. factory frequencies) is printed.

#### reset-vfp — Safe Baseline

```bash
nvoc-auto-optimizer.exe reset-vfp                          # all domains
nvoc-auto-optimizer.exe reset-vfp --vfp-domain core
nvoc-auto-optimizer.exe reset-vfp --vfp-domain memory
```

`--vfp-domain` accepts `all` (default), `core`, or `memory`. Locks are *not* cleared here — use `nvoc-cli reset-public-vftable-gpc-lock` / `nvoc-cli reset-freq-lock` (the `optimize`/autoscan exit path clears both automatically, see [[Safety-and-Recovery]]).

#### import-vfp — Internal Only

`import-vfp` still applies a CSV curve (memory rows are matched by point index, other domains by voltage) but is hidden and prints a warning: user-facing import is `nvoc-cli set-public-vftable-point-offset --import-csv <PATH> [--domain <DOMAIN>]`. The `optimize` workflow drives it internally. `sync-vfp-memory-pstate` (second-highest memory VF stage → P0) likewise exists in both binaries.

### Stressor Integration

- Default builds embed the CUDA stress worker (`cli-stressor-cuda-rs`) in the same binary. The optimizer re-executes itself with a hidden `--nvoc-stressor-cuda-rs-worker` argv marker, so CUDA runs only in an isolated child process (a fatal CUDA failure cannot poison the scan).
- `--stressor-profile auto` resolves by GPU: GeForce RTX 40/50 series → `40-50`; otherwise ≤ 8 GiB VRAM → `low-vram`, more → `standard`. Auto selection requires ≥ 6 GiB VRAM (pass `--stressor-profile` explicitly on smaller cards).
- The pass/fail criterion is the stressor process exit code: `0` pass, non-zero fail. Windows Event Log GPU events (FECS exceptions, TDRs) and Linux `dmesg` Xid events are checked around each run and can force a failure.
- OpenCL loads are **not** a final acceptance gate — see [[Stress-Testing]].

### Workspace and Files

`optimize` uses one workspace per GPU, `GPUScan-<UUID>` (override with `--workspace`; Linux also accepts a legacy `Scan-<UUID>` directory). Standalone subcommands default to `./ws/...` paths instead.

| File | Purpose |
|---|---|
| `vfp.jsonl` | Structured scan log (`voltage_range`, `scan_mode`, `key_points`, `test_result`, `point_finished`, `scan_completed` events); core basis for resuming |
| `vfp-init.csv` | Factory curve snapshot (dynamic export, with margin columns) |
| `vfp-tem.csv` | Autoscan per-point results, written in real time |
| `vfp.csv` | `fix-vfp-result` output; the curve to import |
| `vfp-final.csv` | Confirmation snapshot exported after import |

### What Changed Recently (since 2026-05-28)

- **Command surface narrowed; renames** (#204 refactor, `0213c9b` naming normalization): the old `info`/`list`/`status`/`get`/`set ...`/`reset ...` subcommands moved to `nvoc-cli` (e.g. `nvoc-cli set-clock-offset-mhz` → `set-pstate-global-freq-offset`, `set-vfp-voltage-lock` → `set-gpc-volt-lock`, `set-locked-clocks-mhz` → `set-freq-lock`, `reset-vfp-lock` → `reset-public-vftable-gpc-lock`, `get-tdp-temp-limits` → `get-public-power-limit` + `get-public-temp-limit`). See [[CLI-Guide]].
- **`optimize` + bundled stressor** (#245/`411cdbb`): one command runs the whole workflow in a `GPUScan-<UUID>` workspace; the CUDA stressor is compiled into the binary and launched as an isolated child; the old `start.bat` / `start_ultrafast.bat` / `start_legacy.bat` wrappers were removed (use `--mode ultrafast` / `--mode legacy`).
- **Resilient scan resume** (#246/`aa65760`): schema-versioned JSONL with pending/finished test records; binary-search bounds and the current point are restored on rerun; corrupt records require interactive confirmation before resume. Details in [[Autoscan-Workflow]].
- **autoscan-vfp panic fix, tab CSV removed** (#217/`1684241`): scanning no longer aborts after the first point; all CSV I/O is comma-delimited only.
- **Scanner refactor** (#202/`74556b1`): scan loop split into `oc_scanner/{phases,pressure,runtime,windows_events}` modules with structured phase/test logging.
- **Structured TDP/VF outputs** (#186/`3c0d3e0`): the autoscan profile reads structured TDP/temp-limit tables and VF points; scan profile application (VDDQ boost, TDP/temp max-out, cooler levels) uses those structures.
- **Missing TDP/temp limits reported as null** (`703172a`): unreadable limits are no longer fabricated placeholders — the optimizer skips maxing them out with an explicit error instead.
- **Native Windows GPU event query** (#259/`4fa5838`): FECS/TDR detection queries the Windows Event Log via the native `EvtQuery` API (replacing the PowerShell helper); Linux reads Xid events from `dmesg`.
- **Development manual override + single-binary child spawn fix** (#248/`8968e25`): debug builds accept `p`/`pass` or `f`/`fail` typed into the scan terminal to force the current stressor result; the bundled worker spawn is routed through an argv marker.
- **Colorized warning text** (#256/`f9392af`): warnings and scan status use the shared color helpers; disable with `--no-color` or `NO_COLOR`.

### See Also

[[Autoscan-Workflow]] · [[CLI-Guide]] · [[GPU-Support-Matrix]] · [[Safety-and-Recovery]] · [[Build-and-Install]] · [[Frontends]] · [[Stress-Testing]]

---

<a id="chinese"></a>

## 中文

`nvoc-auto-optimizer` 是 NVOC 的 autoscan 编排 CLI 核心，承担四类关键工作流：

- `autoscan` —— 针对核心/显存频率偏移的自动稳定性扫描循环（`autoscan-vfp`、`autoscan-vfp-legacy`，并由一键式 `optimize` 工作流封装）
- `vfp` —— V-F 曲线读取/导出/导入（`export-vfp`、`export-vfp-log`、`import-vfp`、`sync-vfp-memory-pstate`）
- `reset` —— 恢复安全基线（`reset-vfp`）
- `fix_result` —— 规范化并修正扫描输出（`fix-vfp-result`）

直接的 NVAPI/NVML 控制命令（GPU 发现、实时状态、风扇、功耗墙、各类锁、通用超频写入）已移至 `nvoc-cli` —— 见 [[CLI-Guide]]。本页只记录优化器自身当前暴露的命令。

### 基本用法

```
nvoc-auto-optimizer.exe [--gpu GPU_ID] [--no-color] <command> [command options]
```

必须指定子命令。修改 GPU 状态的命令（`reset-vfp`、`import-vfp`、`autoscan-vfp`、`autoscan-vfp-legacy`、`optimize`）需要 Windows 管理员 / Linux root 权限。`export-vfp` 与 `fix-vfp-result` 属于读取侧，无需提权；`export-vfp-log` 完全离线。

#### 全局参数

| 参数 | 简写 | 说明 |
|---|---|---|
| `--gpu <GPU_ID>` | `-g` | 目标 GPU 选择器；接受十进制或十六进制，可重复指定。缺省操作所有 GPU |
| `--no-color` | — | 禁用 ANSI 彩色输出（同时遵循 `NO_COLOR` 环境变量） |

### 命令一览

| 命令 | 提权 | 说明 |
|---|---|---|
| `optimize` | 需要 | 运行完整 VFP 优化工作流（重置 → 导出 → autoscan → fix → 导入 → 最终导出） |
| `autoscan-vfp` | 需要 | 逐点自动扫描新 VFP 曲线 |
| `autoscan-vfp-legacy` | 需要 | Legacy GPU（Maxwell/Volta 级）全局 P-State 偏移扫描 |
| `export-vfp [OUTPUT]` | 不需要 | 将当前 VFP 曲线导出为 CSV（动态负载采样） |
| `export-vfp-log` | 不需要（离线） | 从 autoscan JSONL 日志解析并导出 VFP 点 |
| `import-vfp [INPUT]` | 需要 | **内部 optimize 工作流驱动（隐藏）**；用户导入请用 `nvoc-cli set-public-vftable-point-offset --import-csv <PATH>` |
| `fix-vfp-result` | 不需要 | autoscan 结果后处理（轻重载补偿） |
| `reset-vfp` | 需要 | 将 VFP 曲线偏移归零 |

#### optimize —— 端到端工作流

```bash
nvoc-auto-optimizer.exe optimize
nvoc-auto-optimizer.exe -g 1 optimize --mode ultrafast
nvoc-auto-optimizer.exe optimize --mode legacy
nvoc-auto-optimizer.exe optimize --fresh --yes
```

| 参数 | 默认 | 说明 |
|---|---|---|
| `--mode <MODE>` | `standard` | `standard`、`ultrafast` 或 `legacy` 工作流 |
| `--fresh` | 关 | 丢弃可续扫日志（`vfp.jsonl`）与临时结果 |
| `-y`，`--yes` | 关 | 跳过安全警告确认 |
| `--workspace <PATH>` | `GPUScan-<UUID>` | 相对路径的单 GPU 扫描工作目录（拒绝绝对路径与 `..`） |

内置压力测试构建（默认 `stressor-bundled` feature）额外接受：

| 参数 | 说明 |
|---|---|
| `--stressor-profile <PROFILE>` | `auto`（默认）、`low-vram`、`standard`、`40-50` |
| `--stressor-config <PATH>` | 自定义压力测试 TOML 配置（与 `--stressor-profile` 互斥） |

外部压力测试构建（`stressor-external` 且无 `stressor-bundled`）使用 `--test-exe <PATH>` / `--minload-exe <PATH>`（默认 `cli-stressor-cuda-rs`）。同时启用两个 feature 的构建增加 `--stressor-backend <bundled\|external>`（默认 `bundled`）。

`optimize` 会提示"V/F 优化会主动试探不稳定设置"并要求交互式输入 `yes`（或 `--yes`）。它选择一块 GPU（交互选择或 `-g`）、创建工作目录，然后依次执行：重置 P-State 偏移 → 重置 VFP 偏移 → 重置 GPC 电压锁 → 导出出厂曲线 → autoscan → `fix-vfp-result -m 1` → 导入 → 最终导出。

#### autoscan-vfp —— 核心扫描循环

```bash
nvoc-auto-optimizer.exe autoscan-vfp
nvoc-auto-optimizer.exe autoscan-vfp -u
nvoc-auto-optimizer.exe -g 1 autoscan-vfp --stressor-profile low-vram
```

| 参数 | 简写 | 默认 | 说明 |
|---|---|---|---|
| `--log <LOG>` | `-l` | `./ws/vfp.jsonl` | 结构化 JSONL 扫描日志（断点续扫依据） |
| `--ultrafast` | `-u` | 关 | 只扫 4 个关键点，其余由 `fix-vfp-result` 插值 |
| `-q <POINT_SEQ>` | — | `-` | 自定义扫描点序列（`-` 为自动；保留兼容） |
| `-o <OUTPUTCSV>` | — | `./ws/vfp-tem.csv` | 每点结果实时写入的 CSV |
| `-i <INITCSV>` | — | `./ws/vfp-init.csv` | 参考出厂曲线 CSV |
| `-m`，`--Vmem_scan_switch` | — | 关 | 同时扫描显存超频上限 |
| `-t <TIMEOUT_LOOPS>` | — | `30` | 保留兼容参数；当前各阶段时长由扫描器固定 |
| `-b <METHOD>`（`--recovery_method_switch`） | — | 按 GPU 世代 | `aggressive` 或 `traditional`；已解析但当前恢复处理由世代决定 |
| `--cuda-device <INDEX>` | — | 自动推导 | 压力测试的 CUDA 设备序号（单一数字 `-g` 时自动推导） |
| `--stressor-extra-args <ARG>...` | — | — | 逐字追加到每次压力测试调用的额外参数 |
| `--stressor-profile <PROFILE>` | — | `auto` | 内置 CUDA 压力档位：`auto`、`low-vram`、`standard`、`40-50` |
| `--stressor-config <PATH>` | — | — | 自定义压力测试 TOML（覆盖档位） |
| `--stressor-backend <BACKEND>` | — | `bundled` | `bundled` 内置 worker 或 `external` 外部可执行（双 feature 构建） |

外部后端构建改用 `-w, --test-exe <PATH>` 与 `--minload-exe <PATH>`。另有若干电压/频率锁参数（`--locked-voltage`、`--locked-core-clocks`、`--locked-mem-clocks`、`--clock`、`--voltage`、`--point`、`--domain`）已隐藏：它们是扫描循环自身的内部通道。

每次压力测试运行内置 CUDA worker `5 × 循环数` 秒（短测试 10 循环、长耐久测试 ×2、ultrafast +50%）；守护在 `15 × 循环数` 秒强制结束并判该点失败。

#### autoscan-vfp-legacy —— Maxwell/Volta 路径

与 `autoscan-vfp` 共用公共参数（`-l`、`-t`、`-b`、`--cuda-device`、`--stressor-*`），但不支持 `--ultrafast`、`-q`、`-o`、`-m`、`-i` —— Legacy GPU 只接受单一全局 P0 图形偏移，结果是全局偏移而非逐点曲线。适用世代见 [[GPU-Support-Matrix]]。

```bash
nvoc-auto-optimizer.exe autoscan-vfp-legacy
nvoc-auto-optimizer.exe autoscan-vfp-legacy -b aggressive
```

#### export-vfp —— 曲线导出

```bash
nvoc-auto-optimizer.exe export-vfp .\GPUScan-xxxx\vfp-init.csv
nvoc-auto-optimizer.exe export-vfp --memory .\mem.csv
```

| 参数 | 简写 | 说明 |
|---|---|---|
| `<OUTPUT>` | — | 输出路径；`-`（默认）将静态表打印到 stdout |
| `--quick` | `-q` | **已废弃**：静态导出改用 `nvoc-cli get-public-vftable --output-csv <PATH> --domain <DOMAIN>` |
| `--nocheck` | `-n` | 跳过动态结果的合理性校验 |
| `--memory` / `--processor` / `--video` / `--undefined` | — | 导出其他 VF 表域（互斥；默认 Graphics） |

默认（动态）导出会以内置压力测试的 `dynamic-export` 档位运行约 45 秒，然后写出逗号分隔 CSV，列为 `voltage, frequency, delta, default_frequency, default_frequency_load, margin, margin_bin`。`margin_bin` 列供 `fix-vfp-result` 与 ultrafast 关键点检测使用。

#### export-vfp-log —— 从日志重建曲线

```bash
nvoc-auto-optimizer.exe export-vfp-log -l .\GPUScan-xxxx\vfp.jsonl -i .\GPUScan-xxxx\vfp-init.csv -o .\from-log.csv
```

从 JSONL 日志解析已完成的核心扫描点并写成 CSV。离线可用（无需 GPU）。

#### fix-vfp-result —— 裕量补偿

```bash
nvoc-auto-optimizer.exe fix-vfp-result -m 1
nvoc-auto-optimizer.exe fix-vfp-result -m 1 -u
```

| 参数 | 简写 | 默认 | 说明 |
|---|---|---|---|
| `-m <MINUS_BIN>` | — | `1` | 额外保守下压的 bin 数（整数，−50..50；负值放宽） |
| `-v <TMPCSV>` | — | `./ws/vfp-tem.csv` | 输入：autoscan 临时 CSV |
| `-o <OUTPUTCSV>` | — | `./ws/vfp.csv` | 输出：补偿后的最终曲线 CSV |
| `-i <INITCSV>` | — | `./ws/vfp-init.csv` | 参考出厂曲线 |
| `--ultrafast` | `-u` | 关 | 先对 4 个 ultrafast 关键点插值 |
| `-l <VFPLOG>` | — | `./ws/vfp.jsonl` | JSONL 日志（ultrafast 插值的关键点来源） |
| `-d <DELTA_REF>` | — | `3` | 内部参考差值 |

基于导出的 `margin_bin` 做逐点修正（`m` 为 `-m` 值，`step` 为该 GPU 最小频率步进）：`margin_bin > 5` 下压 `(3 + m)·step`；`|margin_bin| < 2` 下压 `m·step`；其余下压 `( |margin_bin| + m )·step`。偏移下限钳到 0，并输出"SP score"（最终频率与出厂频率之和的比值）。

#### reset-vfp —— 安全基线

```bash
nvoc-auto-optimizer.exe reset-vfp                          # 所有域
nvoc-auto-optimizer.exe reset-vfp --vfp-domain core
nvoc-auto-optimizer.exe reset-vfp --vfp-domain memory
```

`--vfp-domain` 接受 `all`（默认）、`core`、`memory`。此命令**不**清除锁 —— 请用 `nvoc-cli reset-public-vftable-gpc-lock` / `nvoc-cli reset-freq-lock`（`optimize`/autoscan 退出路径会自动清除两者，见 [[Safety-and-Recovery]]）。

#### import-vfp —— 仅内部使用

`import-vfp` 仍可应用 CSV 曲线（memory 域按点序号对齐，其他域按电压匹配）但已隐藏并会打印警告：面向用户的导入命令是 `nvoc-cli set-public-vftable-point-offset --import-csv <PATH> [--domain <DOMAIN>]`。`optimize` 工作流内部调用它。`sync-vfp-memory-pstate`（将次高显存 VF 档同步到 P0）在两个二进制中均存在。

### 压力测试集成

- 默认构建将 CUDA 压力 worker（`cli-stressor-cuda-rs`）编入同一二进制。优化器以隐藏的 `--nvoc-stressor-cuda-rs-worker` argv 标记重新执行自身，CUDA 只在隔离子进程中运行（致命 CUDA 故障不会污染扫描进程）。
- `--stressor-profile auto` 按 GPU 解析：GeForce RTX 40/50 系 → `40-50`；否则显存 ≤ 8 GiB → `low-vram`，更多 → `standard`。自动选择要求 ≥ 6 GiB 显存（更小的卡请显式传 `--stressor-profile`）。
- 通过/失败判据是压力测试进程退出码：`0` 通过，非零失败。每次运行前后会检查 Windows 事件日志 GPU 事件（FECS 异常、TDR）与 Linux `dmesg` Xid 事件，可直接判失败。
- OpenCL 负载**不是**最终验收判据 —— 见 [[Stress-Testing]]。

### 工作目录与文件

`optimize` 为每块 GPU 使用独立工作目录 `GPUScan-<UUID>`（可用 `--workspace` 覆盖；Linux 亦兼容旧 `Scan-<UUID>` 目录）。独立子命令的默认路径仍为 `./ws/...`。

| 文件 | 用途 |
|---|---|
| `vfp.jsonl` | 结构化扫描日志（`voltage_range`、`scan_mode`、`key_points`、`test_result`、`point_finished`、`scan_completed` 事件）；断点续扫核心依据 |
| `vfp-init.csv` | 出厂曲线快照（动态导出，含 margin 列） |
| `vfp-tem.csv` | autoscan 每点结果，实时写入 |
| `vfp.csv` | `fix-vfp-result` 输出；待导入的曲线 |
| `vfp-final.csv` | 导入后再次导出的确认快照 |

### 近期变更（2026-05-28 之后）

- **命令面收窄与改名**（#204 重构、`0213c9b` 命名归一化）：旧 `info`/`list`/`status`/`get`/`set ...`/`reset ...` 子命令移至 `nvoc-cli`（如 `nvoc-cli set-clock-offset-mhz` → `set-pstate-global-freq-offset`、`set-vfp-voltage-lock` → `set-gpc-volt-lock`、`set-locked-clocks-mhz` → `set-freq-lock`、`reset-vfp-lock` → `reset-public-vftable-gpc-lock`、`get-tdp-temp-limits` → `get-public-power-limit` + `get-public-temp-limit`）。见 [[CLI-Guide]]。
- **`optimize` + 内置压力测试**（#245/`411cdbb`）：一条命令在 `GPUScan-<UUID>` 工作目录完成整个工作流；CUDA 压力测试编译进二进制并作为隔离子进程启动；旧 `start.bat` / `start_ultrafast.bat` / `start_legacy.bat` 包装脚本已移除（改用 `--mode ultrafast` / `--mode legacy`）。
- **强韧的断点续扫**（#246/`aa65760`）：带 schema 版本的 JSONL，含 pending/finished 测试记录；重跑时恢复二分搜索边界与当前点；损坏记录需交互确认后才续扫。详见 [[Autoscan-Workflow]]。
- **autoscan-vfp 首点后 panic 修复、移除 tab CSV**（#217/`1684241`）：扫描不再在第一个点后中止；所有 CSV 读写仅支持逗号分隔。
- **扫描器重构**（#202/`74556b1`）：扫描循环拆分为 `oc_scanner/{phases,pressure,runtime,windows_events}` 模块，带结构化阶段/测试日志。
- **结构化 TDP/VF 输出**（#186/`3c0d3e0`）：autoscan 档位读取结构化 TDP/温度墙表与 VF 点；扫描档位（VDDQ boost、TDP/温度墙拉满、风扇档位）基于这些结构。
- **缺失 TDP/温度墙上报为 null**（`703172a`）：读不到的限值不再伪造占位值 —— 优化器会跳过拉满并给出明确错误。
- **原生 Windows GPU 事件查询**（#259/`4fa5838`）：FECS/TDR 检测改为原生 `EvtQuery` API 查询 Windows 事件日志（取代 PowerShell 辅助脚本）；Linux 从 `dmesg` 读取 Xid 事件。
- **开发用手动覆写 + 单二进制子进程修复**（#248/`8968e25`）：debug 构建可在扫描终端键入 `p`/`pass` 或 `f`/`fail` 强制当前压力测试结果；内置 worker 的派生改经 argv 标记路由。
- **告警文本着色**（#256/`f9392af`）：警告与扫描状态使用统一着色助手；`--no-color` 或 `NO_COLOR` 可关闭。

### 另见

[[Autoscan-Workflow]] · [[CLI-Guide]] · [[GPU-Support-Matrix]] · [[Safety-and-Recovery]] · [[Build-and-Install]] · [[Frontends]] · [[Stress-Testing]]

---

*Maintained from: auto-optimizer/README.md, auto-optimizer/src/*, git log*
