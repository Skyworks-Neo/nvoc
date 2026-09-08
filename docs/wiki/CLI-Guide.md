# nvoc-cli Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

`nvoc-cli` is the focused command-line wrapper over `nvoc-core`. It exposes GPU discovery, live status, read-only telemetry, and the NVAPI/NVML write surfaces (clock offsets, V-F curve edits, power/thermal walls, fan control, P-State locks, and private NVIDIA control families) as flat, function-style commands. Per-command behavior details live in [[CLI-Command-Reference]]; this page covers the argument model, backends, output, privileges, and safety workflow.

> ⚠️ **Safety first**: overclocking writes are high-risk. Read state before you write, record the original values, verify after every write, and know your recovery path ([[Safety-and-Recovery]]). Commands backed by *private* NVAPI surfaces are driver- and generation-dependent — prefer read-only use.

### Install and build

`nvoc-cli` is a workspace member of the Rust monorepo (edition 2024, toolchain pinned in `rust-toolchain.toml`). Build without the CUDA stressor crate:

```bash
cargo build --workspace --exclude cli-stressor-cuda-rs
# or just the CLI:
cargo build -p nvoc-cli --release
```

The binary lands at `target/release/nvoc-cli` (Linux) or `target\release\nvoc-cli.exe` (Windows):

```powershell
.\target\release\nvoc-cli.exe --help
```

### Global argument model

```text
nvoc-cli [--gpu GPU_ID] [--nvapi|--nvml] [--output human|json] <function-name> [args] [named args]
```

Named arguments can be placed before or after the function name. Use `nvoc-cli <function-name> --help` to see the named arguments supported by a specific function. Invoking a command with missing required arguments prints the **full** per-command help, not just a usage line.

Global options (defined in `cli/src/lib.rs`):

| Option | Value | Meaning |
|---|---|---|
| `-g`, `--gpu` | `GPU_ID` | GPU selector; repeat for multiple GPUs |
| `--nvapi` | flag | Force the NVAPI backend (conflicts with `--nvml`) |
| `--nvml` | flag | Force the NVML backend (conflicts with `--nvapi`) |
| `--nvml-path` | `PATH` | Explicit `nvml.dll` path — legacy drivers keep NVML outside the DLL search path (e.g. `...\NVSMI\nvml.dll`); overrides `NVOC_NVML_PATH` |
| `--nvapi-path` | `PATH` | Directory (or `nvapi64.dll` file) inserted into the DLL search path before NVAPI loads; overrides `NVOC_NVAPI_PATH` |
| `-O`, `--output` | `human` \| `json` | Output format, default `human` |
| `--no-color` | flag | Disable ANSI color (the `NO_COLOR` environment variable is honored too) |
| `-h`, `--help` / `-V`, `--version` | — | Help / version |

Per-command selectors registered globally — their meaning depends on the command:

| Selector | Used for |
|---|---|
| `--domain` | Clock domain (`core`/`memory`/`processor`/`video`), `gpu\|acoustic` on `set-temp-limit` (NVML path), `core\|mem` on `set-legacy-freq`, `gpc\|xbar\|msd\|disp\|mem` on the private V/F tools, or a 0–3 clock-domain index on `get-pstate-lock` |
| `--pstate` | A P-State such as `P0` or `P2` (repeatable where noted) |
| `--fan` | Fan/cooler target: `all`, `0`, `1`, or `2` |
| `--policy` | Fan control policy, default `manual` |
| `--policy-index` | TGP power-policy table index (default 2) |

One deliberate exception to "named args anywhere": `--freq` / `--volt` on `set-private-freq-domain-global-offset` and the two private resets are **subcommand-only**. The root-level form is rejected outright so a dropped `--volt` can never silently turn a +25 mV voltage intent into a +25 MHz frequency write.

### Command discovery

```bash
nvoc-cli list                  # all commands grouped by family
nvoc-cli list power            # one family: info|power|thermal|fan|clock|voltage|vfp|perf|scanner
nvoc-cli get-private-vftable --help
```

The bare root help is hand-rendered and grouped into 9 families (the command surface itself stays flat): **info** (14), **power** (14), **thermal** (9), **fan** (8), **clock** (20), **voltage** (13), **vfp** (14), **perf** (9), **scanner** (1) — 101 GPU commands plus the `list` meta command.

### Backend selection: NVML vs NVAPI vs private NVAPI

- **Auto (no flag)** — commands that support both backends try NVAPI first and fall back to NVML if the NVAPI attempt fails. Per-command exceptions exist where the other backend is the more useful default: `get-public-power-limit` and `get-temp-thresholds` prefer NVML in auto mode, while `set-power-limit` prefers NVAPI.
- **`--nvml`** — the stable, public NVIDIA Management Library surface. Best for power limits, temperature thresholds, fan control, P-State clock ranges, throttle reasons.
- **`--nvapi`** — the Windows driver API, including *private* control families (private V/F tables, ClkDomains writes, volt rails, pstates-2.0 internals, D-Notifier, PMGR arbiter). Private surfaces are undocumented, **structure versions vary by driver generation**, and some stamps only exist on specific driver branches — read-only use is recommended until you have verified behavior on your own GPU/driver.
- **Linux note** (from `cli/README.md`): NVAPI on Linux is essentially a compatibility layer — `libnvidia-api.so.1` translates to `libnvidia-ml.so.1`, so only the NVML interface truly exists. Because NVOC's primary GPU index uses NVAPI, support on Linux can be better than expected for professional cards (P100, V100, …).
- Unsupported backend + command combinations are rejected at parse time (e.g. `get-throttle-reasons --nvapi` errors out), so a wrong flag never reaches the GPU.

Generation-by-generation compatibility per function is maintained in [[GPU-Support-Matrix]].

### Output formats

- **human** (default): colored, sectioned output — grouped root help, aligned V/F tables, `Key: value` status blocks.
- **json** (`--output json` / `-O json`): one machine-readable document per run. Missing values are reported as JSON `null`, never fabricated placeholders (e.g. a driver reporting no TDP/temp-limit entries yields `"min_tdp_percent": null`).
- Unsupported private features return `{"supported": false}` (sometimes with the raw NVAPI `status_code` + `status_name`) instead of erroring, so scripts can branch cleanly.
- `--no-color` / `NO_COLOR` strip ANSI styling for logging.

### Privilege requirements

| Environment | Reads | Writes / private NVAPI |
|---|---|---|
| Windows | mostly unprivileged | **elevated (Administrator) shell** required for most writes and private families |
| Linux | NVML reads usually fine unprivileged | **root** required for writes |
| Container | NVML reads work with `--gpus all` | root **+** `--cap-add SYS_ADMIN` **+** bind-mount `libnvidia-api.so.1` for NVAPI |

Container details, tested write paths, and the exact `docker run` invocations live in [[Container-Usage]]. Note that the examples there predate the 2026-08 command renaming — map old names (e.g. `set-power-watt`) to current ones (e.g. `set-power-limit`) using [[CLI-Command-Reference]]. Container isolation does **not** isolate GPU hardware state: any write inside the container hits the host GPU.

### The 64 MiB-stack worker thread

`main()` runs the whole CLI on a spawned worker thread with a **64 MiB stack**. Reason: debug builds keep large stack temporaries (multi-hundred-KB NVAPI structs, wide output tables) un-inlined on the 1 MiB main-thread stack, which overflows on commands like `get-private-vftable` / `get-status`. Release builds fit fine, but the worker removes the debug-mode cliff entirely. Panics from the worker are resumed on the main thread, so crash semantics are unchanged.

### Safety workflow (read → verify → write → readback → recovery)

1. **Discover and read first.** `get-gpu-list`, `get-info`, `get-status` — confirm the target GPU and current state.
2. **Record the original state** (JSON output is handy): offsets, power limit, fan policy, locked clocks.
3. **Write small.** Prefer reversible values (`--temporary` on the private ClkDomains write restores the pre-write snapshot before returning).
4. **Read back** the same value with the matching `get-` command; JSON makes diffs trivial.
5. **Know the recovery**: every `set-` family has a symmetric `reset-`; keep [[Safety-and-Recovery]] at hand. If the display dies, the classic escape is a driver restart (`restart-display-driver`) or reboot — locks written via `set-private-permanent-pstate-lock-user` survive resets and only a reboot/driver reload clears them.

Example round trip:

```bash
nvoc-cli -O json get-pstate-global-freq-offset --domain core --pstate P0   # record (0 MHz)
nvoc-cli set-pstate-global-freq-offset 150 --domain core --pstate P0       # +150 MHz
nvoc-cli get-pstate-global-freq-offset --domain core --pstate P0           # readback: 150
nvoc-cli reset-pstate-global-freq-offset                                    # back to stock
```

High-risk families flagged in the source itself: private V/F point writes (`set-private-vftable-*`, "dangerous V/F edit"), ClkDomains writes (`set-private-freq-domain-global-offset`, "dangerous XBar clock write"), temperature simulation (`set-temp-sim`, "DANGEROUS research tool"), raw volt-rail limits, PMGR arbiter writes, and EDID replacement. Treat each as a bench experiment, not a daily driver setting.

### Exit codes and errors

| Exit code | Meaning |
|---|---|
| `0` | Success — including `--help` and `list` |
| `1` | Runtime error (backend failure, driver refusal) — or **any** per-GPU failure in a multi-GPU run (other GPUs still execute; results mark the failures) |
| `2` | Usage error: unknown command, wrong positional count, option not valid for that command |
| clap codes | Parse-level errors (clap's own exit codes, typically `2`) |

Error text goes to stderr with an `Error:` prefix; `Run 'nvoc-cli --help' for usage.` accompanies usage errors. Driver rejections surface as nvoc-core error strings or raw NVAPI status names (`NotSupported`, `NoPermission`, …).

<a id="chinese"></a>

## 中文

`nvoc-cli` 是构建在 `nvoc-core` 之上的精简命令行封装，以扁平的函数式命令暴露 GPU 发现、实时状态、只读遥测，以及 NVAPI/NVML 写入面（频率偏移、V-F curve 编辑、功率/温度墙、风扇控制、P-State 锁，以及 NVIDIA 私有控制族）。各命令的详细行为见 [[CLI-Command-Reference]]；本页介绍参数模型、后端选择、输出格式、权限要求与安全流程。

> ⚠️ **安全第一**：超频写入属于高风险操作。写入前先读取状态、记录原始值，每次写入后回读验证，并明确恢复路径（[[Safety-and-Recovery]]）。基于 *private* NVAPI 面的命令依赖驱动与 GPU 代际——私有结构随驱动版本变化，建议优先只读使用。

### 安装与构建

`nvoc-cli` 是 Rust monorepo 的 workspace 成员（edition 2024，工具链版本锁定在 `rust-toolchain.toml`）。构建时不包含 CUDA 压测 crate：

```bash
cargo build --workspace --exclude cli-stressor-cuda-rs
# 或只构建 CLI：
cargo build -p nvoc-cli --release
```

二进制位于 `target/release/nvoc-cli`（Linux）或 `target\release\nvoc-cli.exe`（Windows）：

```powershell
.\target\release\nvoc-cli.exe --help
```

### 全局参数模型

```text
nvoc-cli [--gpu GPU_ID] [--nvapi|--nvml] [--output human|json] <function-name> [args] [named args]
```

命名参数可以放在函数名之前或之后。使用 `nvoc-cli <function-name> --help` 查看某个函数支持的命名参数。命令缺少必选参数时会打印**完整**的命令帮助，而不只是 usage 行。

全局选项（定义于 `cli/src/lib.rs`）：

| 选项 | 取值 | 说明 |
|---|---|---|
| `-g`, `--gpu` | `GPU_ID` | GPU 选择器；可重复以选择多块 GPU |
| `--nvapi` | 开关 | 强制 NVAPI 后端（与 `--nvml` 互斥） |
| `--nvml` | 开关 | 强制 NVML 后端（与 `--nvapi` 互斥） |
| `--nvml-path` | `PATH` | 显式指定 `nvml.dll` 路径——老驱动常把 NVML 放在 DLL 搜索路径之外（如 `...\NVSMI\nvml.dll`）；覆盖 `NVOC_NVML_PATH` |
| `--nvapi-path` | `PATH` | 在 NVAPI 加载前插入 DLL 搜索路径的目录（或 `nvapi64.dll` 文件）；覆盖 `NVOC_NVAPI_PATH` |
| `-O`, `--output` | `human` \| `json` | 输出格式，默认 `human` |
| `--no-color` | 开关 | 关闭 ANSI 着色（同时遵循 `NO_COLOR` 环境变量） |
| `-h`, `--help` / `-V`, `--version` | — | 帮助 / 版本 |

全局注册的按命令选择器——具体含义取决于命令：

| 选择器 | 用途 |
|---|---|
| `--domain` | 时钟域（`core`/`memory`/`processor`/`video`）；`set-temp-limit` 的 NVML 路径用 `gpu\|acoustic`；`set-legacy-freq` 用 `core\|mem`；私有 V/F 工具用 `gpc\|xbar\|msd\|disp\|mem`；`get-pstate-lock` 用 0–3 的时钟域索引 |
| `--pstate` | P-State，如 `P0`、`P2`（部分命令可重复） |
| `--fan` | 风扇/cooler 目标：`all`、`0`、`1`、`2` |
| `--policy` | 风扇控制策略，默认 `manual` |
| `--policy-index` | TGP 功率策略表索引（默认 2） |

"命名参数任意位置放置"有一个刻意的例外：`set-private-freq-domain-global-offset` 与两个私有 reset 命令上的 `--freq` / `--volt` **只能写在子命令之后**。根级别的写法会被直接拒绝，这样漏掉的 `--volt` 绝不会把 +25 mV 的电压意图悄悄变成 +25 MHz 的频率写入。

### 命令发现

```bash
nvoc-cli list                  # 按族列出全部命令
nvoc-cli list power            # 单个族：info|power|thermal|fan|clock|voltage|vfp|perf|scanner
nvoc-cli get-private-vftable --help
```

根帮助为手工渲染、按 9 个族分组（命令面本身保持扁平）：**info**（14）、**power**（14）、**thermal**（9）、**fan**（8）、**clock**（20）、**voltage**（13）、**vfp**（14）、**perf**（9）、**scanner**（1）——共 101 条 GPU 命令外加 `list` 元命令。

### 后端选择：NVML vs NVAPI vs 私有 NVAPI

- **auto（不加旗标）**——同时支持两个后端的命令先尝试 NVAPI，失败后回退 NVML。个别命令的 auto 默认例外：`get-public-power-limit` 与 `get-temp-thresholds` 优先 NVML，`set-power-limit` 优先 NVAPI。
- **`--nvml`**——稳定的公开 NVIDIA Management Library 面。适合功率墙、温度阈值、风扇控制、P-State 频率范围、throttle reasons。
- **`--nvapi`**——Windows 驱动 API，包含*私有*控制族（私有 V/F 表、ClkDomains 写入、volt rails、pstates-2.0 内部结构、D-Notifier、PMGR arbiter）。私有面未公开文档化，**结构版本随驱动代际变化**，部分 stamp 只存在于特定驱动分支——在自己验证之前建议只读使用。
- **Linux 说明**（来自 `cli/README.md`）：Linux 上的 NVAPI 本质上是一个兼容层——`libnvidia-api.so.1` 直接转发到 `libnvidia-ml.so.1`，即真正存在的只有 NVML 接口。由于 NVOC 的主 GPU 索引走 NVAPI，专业卡（P100、V100 等）在 Linux 上的支持反而可能比 Windows 更好。
- 不支持的后端 + 命令组合在解析期即被拒绝（例如 `get-throttle-reasons --nvapi` 直接报错），错误旗标不会触达 GPU。

各功能 × GPU 代际的兼容性维护在 [[GPU-Support-Matrix]]。

### 输出格式

- **human**（默认）：彩色、分节的输出——分组根帮助、对齐的 V/F 表、`Key: value` 状态块。
- **json**（`--output json` / `-O json`）：每次运行输出一份机器可读文档。缺失值输出 JSON `null`，绝不伪造占位符（例如驱动未上报 TDP/temp-limit 条目时输出 `"min_tdp_percent": null`）。
- 不受支持的私有功能返回 `{"supported": false}`（有时附带原始 NVAPI `status_code` + `status_name`），而不是报错，便于脚本分支处理。
- `--no-color` / `NO_COLOR` 去除 ANSI 样式，方便写日志。

### 权限要求

| 环境 | 读取 | 写入 / 私有 NVAPI |
|---|---|---|
| Windows | 大多无需提权 | 大多数写入与私有族需要**管理员 shell** |
| Linux | NVML 读取通常普通用户即可 | 写入需要 **root** |
| 容器 | `--gpus all` 下 NVML 读取可用 | 需要 root **+** `--cap-add SYS_ADMIN` **+** 绑定挂载 `libnvidia-api.so.1`（NVAPI） |

容器细节、已验证的写入路径与完整 `docker run` 命令见 [[Container-Usage]]。注意该页示例早于 2026-08 的命令重命名——用 [[CLI-Command-Reference]] 将旧名（如 `set-power-watt`）映射到现名（如 `set-power-limit`）。容器隔离**不会**隔离 GPU 硬件状态：容器内的任何写入都作用于宿主 GPU。

### 64 MiB 栈工作线程

`main()` 将整个 CLI 跑在一个 **64 MiB 栈**的派生工作线程上。原因：debug 构建会把大型栈上临时对象（几百 KB 的 NVAPI 结构体、宽输出表）保留在内联之外，放在 1 MiB 的主线程栈上，`get-private-vftable` / `get-status` 等命令会栈溢出。release 构建本可以放下，但工作线程彻底消除了 debug 模式的悬崖。工作线程的 panic 会在主线程上重新展开，崩溃语义不变。

### 安全流程（读取 → 验证 → 写入 → 回读 → 恢复）

1. **先发现、先读取。** `get-gpu-list`、`get-info`、`get-status`——确认目标 GPU 与当前状态。
2. **记录原始状态**（JSON 输出很方便）：偏移、功率墙、风扇策略、频率锁。
3. **小步写入。** 优先使用可逆的值（私有 ClkDomains 写入的 `--temporary` 会在返回前恢复写入前快照）。
4. **用对应的 `get-` 命令回读**；JSON 便于 diff。
5. **牢记恢复路径**：每个 `set-` 族都有对称的 `reset-`；随时备好 [[Safety-and-Recovery]]。显示崩溃时的经典逃生通道是 `restart-display-driver` 或重启——但 `set-private-permanent-pstate-lock-user` 写入的锁能熬过 reset，只有重启/重载驱动才能清除。

完整往返示例：

```bash
nvoc-cli -O json get-pstate-global-freq-offset --domain core --pstate P0   # 记录（0 MHz）
nvoc-cli set-pstate-global-freq-offset 150 --domain core --pstate P0       # +150 MHz
nvoc-cli get-pstate-global-freq-offset --domain core --pstate P0           # 回读：150
nvoc-cli reset-pstate-global-freq-offset                                    # 回到默认
```

源码中自带高危标记的族：私有 V/F 点写入（`set-private-vftable-*`，"dangerous V/F edit"）、ClkDomains 写入（`set-private-freq-domain-global-offset`，"dangerous XBar clock write"）、温度仿真（`set-temp-sim`，"DANGEROUS research tool"）、裸 volt-rail 限值、PMGR arbiter 写入、EDID 替换。请把这些当作台架实验，而不是日常驱动设置。

### 退出码与错误

| 退出码 | 含义 |
|---|---|
| `0` | 成功——包括 `--help` 与 `list` |
| `1` | 运行时错误（后端失败、驱动拒绝）——多 GPU 运行中**任意**单卡失败（其余 GPU 仍会执行，结果中标记失败） |
| `2` | 用法错误：未知命令、位置参数数量不对、该命令不支持的选项 |
| clap 码 | 解析级错误（clap 自身退出码，通常为 `2`） |

错误文本写入 stderr 并带 `Error:` 前缀；用法错误伴随 `Run 'nvoc-cli --help' for usage.`。驱动拒绝会以 nvoc-core 错误串或原始 NVAPI 状态名（`NotSupported`、`NoPermission` 等）呈现。

---

*Maintained from: cli/README.md, cli/src/main.rs, cli/src/lib.rs, cli/src/output.rs, cli-common/, docs/wiki/Container-Usage.md.*
