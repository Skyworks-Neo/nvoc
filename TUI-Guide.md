# TUI Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC-TUI is a Python Textual-based terminal interface frontend using `pynvoc` native bindings ([[PyNvoc-API]]).

### Running

```bash
cd tui
uv sync
uv run nvoc-tui
```

### What's new (2026-06 → 2026-09)

| Area | What changed |
|------|--------------|
| Dashboard | Expanded metric lines: video decode clock on the CLK line (fc0acc0), ECLK effective clocks, FCLK fabric-clock line with V2 server domain canonicalization + MSD (393b7ae/7fadfcb), multi-rail VOLT line (7367a1c), core/hotspot/MEM temps, PWR draw/limit, RAILS per-rail power, PCIE Gen/bandwidth/replay, PERF LIMIT reasons; duplicated FAN prefix dropped (9740476) |
| Overclock | ClkDomains fabric offset rows — Xbar/Sys/Msd/Host with controllable-mask gates + coupling read-modify-write (73cb800); Mobile Power pane only on mobile GPUs (d250ab1) with TGP input anchored at the effective PPAB ceiling (2d80a12) and a P0-bounded Volt Limit; fan policy verdict — modern cards restricted to continuous/manual (0c79f35/66a5abe) |
| VF Curve | P0 workable-region bar + brown effective-wall line (6573279), rail-aware crosshair incl. GPC (21c8fe1/802bc15), EXT-section per-domain overlays (1541be1), effective-curve synthesis fallback (7be32ad), read-only vBIOS VF ladder overlay with Maxwell/Kepler fallback (65ca778), per-rail P0 bounds (760cda9) |
| Console & stability | Identical-consecutive console line collapse (e8528df), serialized read-only query workers, GPU hotplug recovery, onefile payload trim — PIL/aiohttp/ssl excluded (~measured, 9153de3) |

### Features

- **Dashboard**: Real-time polling GPU status. The metrics pane now renders, when the GPU exposes them:
  - `CLK` — GPU | MEM | VID (video decode clock) MHz
  - `ECLK` — effective (actually running) GPU/MEM clocks
  - `FCLK` — internal fabric clocks (Xbar, Sys, Msd, Hub, Host, …) from the GetAllClocks V2 breakdown; server parts reporting V2-suffixed domain names (`Gpc2`, `Xbar2`, …) are canonicalized onto the same line, and MSD is shown as a first-class OC domain
  - `VOLT` — multi-rail on multi-rail parts: `GPC 1050 mV | MEM 681 mV` (HBM compute cards) or `… | MSVDD …` (50-series fabric rail), read from the real rail currents instead of the coarse rounded status voltage
  - `TEMP` — core plus hot-spot and memory temps when present, in `live / limit` form
  - `PWR` — live draw and the enforced power limit; `RAILS` — per-rail power breakdown (BOARD/CHIP/MEM/PCIE…)
  - `PCIE` — link Gen, lanes, ↑Tx/↓Rx bandwidth, replay counter warning; `PERF LIMIT` — decoded limit reasons; `PSTATE`; `ARCH`
  - Polling controls (interval, Pause/Resume, Now, Info/Status/Get); a GPU that stops answering enters an offline backoff that re-runs discovery until the dGPU returns, and hotplug changes recover without a restart

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/efeae01a-8958-44c0-bbc5-93294a46f60e" />

- **Overclock**: Overclocking and fan control operations.
  - **Fabric offset rows**: alongside Core/Mem (pstate20 with a −104 → ClkDomains-bit fallback), the Clock pane gains Xbar / Sys / Msd / Host offset rows. Rows are gated by the private ClockClient controllable mask (unsupported domains disabled, e.g. Host without mask bit 9); Sys applies as a read-modify-write that stacks on the −f SYS cancel coupled by Xbar writes; MSD is disabled on Pascal
  - **Mobile Power pane** (shown only on mobile GPUs): PPAB toggle, D-Notifier level, TGP (W) input anchored at the *effective* ceiling — min of requested TGP and the active D-Notifier cap, clamped into the TGP range — plus Target Temp and a Volt Limit (mV) input bounded by the volt-rail P0 walls
  - **Fan policy verdict**: modern cards offer only `contin.` (continuous — other policies silently no-op on modern cooler paths); legacy (≤ Kepler) cards get `default`/`manual` at discovery time; fanless server cards gray out the fan pane
  - **PState From / To** lock with idle-first roster validation and a native single-P-State pin fallback on pre-Kepler parts

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/01146c52-c5a5-4576-8e66-e4f8367ac637" />

- **VF Curve**: Static VF curve export/import/edit with terminal plotting.
  - **Multi-curve chart**: GPC / XBAR / MSD (plus HBM MEM and unknown 50-series curves), current + default series, visibility checkboxes, canonical order GPC → XBAR → MSD → others, and **EXT-section per-domain overlays** (display-only series from the record EXT slots)
  - **P0 boundaries**: deep-red floor/ceiling lines per the active curve's rail (gpc/msd → primary, xbar/mem → secondary MSVDD/HBM rail), a blue **workable-region bar** spanning floor → ceiling just above the x-axis, and the live effective wall drawn as a **brown** line (256-color 130, distinct from the immutable walls and the orange lock marker)
  - **Rail-aware crosshair**: the live point reads the real rail current — GPC included — while fabric curves direct-read their private MEASURE_FREQ domain (MSD on bit 21); a 0 mV "pseudo-lock" from a broken VFP plane is never rendered, and a public read zeroed except the #0 sentinel falls back to the private view
  - **Read-only vBIOS VF ladder** (Maxwell/Kepler fallback): when the driver curve is unavailable, the vBIOS GPU Boost 2.0 ladder (BIOS Range / BIOS Max / BIOS Min) overlays the chart — or becomes the plot, titled "vBIOS VF ladder (read-only)". For these legacy GPUs the curve is view-only: editing targets are downgraded, the ladder is never an edit target

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/3b84f5a4-3aee-492c-b41c-5dd386903625" />

- **Output Console**: Native operation output display; identical consecutive lines collapse into a "(previous line repeated N×)" summary so a per-second poll cannot flood the pane.

### Legacy GPU behavior

- **Maxwell/Kepler (and older)**: the vBIOS VF ladder is the curve representation and is **view-only** — the plot renders read-only and edit/apply targets are downgraded; the ladder is never an edit target (65ca778)
- **Pre-Kepler**: the P-State memory-clock-range lock cannot derive its window and falls back to the native single-P-State pin
- **Pre-Pascal**: no fabric offset rows (Core/Mem only); MSD offset row is disabled on Pascal
- Fan policy on ≤ Kepler cards is restricted to `default`/`manual` (modern `continuous` CoolerPolicy types are rejected by old drivers)
- Unsupported surfaces gray out with an explicit verdict instead of erroring

### Stability & packaging notes

- Read-only query workers are serialized and V/F refresh data is kept in memory; GPU hotplug changes are recovered; an FD leak when multiple refresh ops compete is fixed
- The onefile payload was trimmed after measuring the archive (~81.8 MB total): PIL (13.4 MB, pulled in by plotext's static imports but only used by its unused image plotting), `ssl` + `_hashlib` + libcrypto (no networking/crypto), and `aiohttp`/`msgpack`/`textual.dev` (the textual devtools client stack) are excluded (9153de3). numpy/OpenBLAS stays — plotext (VF curve) hard-requires it

### Testing

```bash
cd tui
uv run pytest
```

### Packaging

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

### Note

> Most of the code is generated by CodeX; functionality is still being improved.

*Maintained from: `gui/`, `tui/` source, CHANGELOG via git log.*

---

<a id="chinese"></a>

## 中文

NVOC-TUI 是基于 Python Textual 的终端界面前端，使用 `pynvoc` 原生绑定（[[PyNvoc-API]]）。

### 运行

```bash
cd tui
uv sync
uv run nvoc-tui
```

### 新变化（2026-06 → 2026-09）

| 领域 | 变化 |
|------|------|
| Dashboard | 指标行扩充：CLK 行增加视频解码时钟（fc0acc0）、ECLK 有效时钟、FCLK fabric 时钟行（V2 服务器域名规范化 + 显示 MSD，393b7ae/7fadfcb）、多 rail VOLT 行（7367a1c）、核心/热点/显存温度、PWR 功耗/上限、RAILS 分 rail 功耗、PCIE Gen/带宽/重放计数、PERF LIMIT 限频原因；去掉重复的 FAN 前缀（9740476） |
| 超频 | ClkDomains fabric 偏移行——Xbar/Sys/Msd/Host，带可控制掩码门控 + 耦合读改写（73cb800）；Mobile Power 面板仅在移动 GPU 上显示（d250ab1），TGP 输入锚定有效 PPAB 上限（2d80a12）、Volt Limit 受 P0 边界约束；风扇策略判定——新卡仅限 continuous/manual（0c79f35/66a5abe） |
| VF Curve | P0 可工作区横条 + 棕色有效墙线（6573279）、含 GPC 的 rail 感知十字线（21c8fe1/802bc15）、EXT 区段分域叠加线（1541be1）、有效曲线合成回退（7be32ad）、只读 vBIOS VF 阶梯叠加（Maxwell/Kepler 回退，65ca778）、分 rail P0 边界（760cda9） |
| 控制台与稳定性 | 相同连续控制台行折叠（e8528df）、只读查询工作线程串行化、GPU 热插拔恢复、onefile 载荷瘦身——剔除 PIL/aiohttp/ssl（实测，9153de3） |

### 功能

- **Dashboard**：实时轮询 GPU 状态。GPU 支持时，指标面板现在会渲染：
  - `CLK` — GPU | MEM | VID（视频解码时钟）MHz
  - `ECLK` — 实际运行的有效 GPU/MEM 时钟
  - `FCLK` — 内部 fabric 时钟（Xbar、Sys、Msd、Hub、Host 等，来自 GetAllClocks V2 全量分解）；上报 V2 后缀域名（`Gpc2`、`Xbar2` 等）的服务器卡片会规范化到同一行，MSD 作为一等超频域显示
  - `VOLT` — 多 rail 卡片显示多 rail：`GPC 1050 mV | MEM 681 mV`（HBM 计算卡）或 `… | MSVDD …`（50 系 fabric rail），读取真实 rail 电流而非粗粒度的状态电压
  - `TEMP` — 核心温度，有热点/显存温度时一并显示，`实时 / 上限` 形式
  - `PWR` — 实时功耗与当前生效的功耗上限；`RAILS` — 分 rail 功耗分解（BOARD/CHIP/MEM/PCIE…）
  - `PCIE` — 链路 Gen、通道数、↑Tx/↓Rx 带宽、重放计数告警；`PERF LIMIT` — 解码后的限频原因；`PSTATE`；`ARCH`
  - 轮询控制（间隔、Pause/Resume、Now、Info/Status/Get）；GPU 失联时进入离线退避并反复重新发现，直到 dGPU 回归；热插拔变化无需重启即可恢复

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/efeae01a-8958-44c0-bbc5-93294a46f60e" />

- **Overclock**：超频与风扇控制操作。
  - **Fabric 偏移行**：在 Core/Mem（pstate20，−104 时回退 ClkDomains 位）之外，Clock 面板新增 Xbar / Sys / Msd / Host 偏移行。各行由私有 ClockClient 可控制掩码门控（不支持的域禁用，如无掩码位 9 的 Host）；Sys 以读-改-写应用，叠加在 Xbar 写入耦合出的 −f SYS 取消量之上；Pascal 上 Msd 行禁用
  - **Mobile Power 面板**（仅在移动 GPU 上显示）：PPAB 开关、D-Notifier 档位、锚定*有效*上限的 TGP (W) 输入——取请求 TGP 与当前 D-Notifier 上限的较小值并钳制进 TGP 量程——以及 Target Temp 和受 volt-rail P0 墙约束的 Volt Limit (mV) 输入
  - **风扇策略判定**：新卡仅提供 `contin.`（continuous——其他策略在现代 cooler 路径上静默无效）；旧卡（≤ Kepler）在发现时提供 `default`/`manual`；无风扇服务器卡将风扇面板置灰
  - **PState From / To** 锁定，带空闲优先名单校验，pre-Kepler 部件回退为原生单 P-State 钉住

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/01146c52-c5a5-4576-8e66-e4f8367ac637" />

- **VF Curve**：静态 VF 曲线导出/导入/编辑，支持终端绘图。
  - **多曲线图**：GPC / XBAR / MSD（另有 HBM MEM 与 50 系未知曲线），实时 + 默认两条序列、可见性复选框，规范顺序 GPC → XBAR → MSD → 其他，以及 **EXT 区段分域叠加线**（来自记录 EXT 槽位的仅显示序列）
  - **P0 边界**：按活动曲线的 rail 绘制深红下限/上限线（gpc/msd → 主 rail，xbar/mem → 次级 MSVDD/HBM rail）、x 轴上方横跨下限→上限的蓝色**可工作区横条**，以及绘制为**棕色**（256 色 130，与不可变墙线和橙色锁标记区分）的实时有效墙线
  - **Rail 感知十字线**：实时点读取真实 rail 电流——包括 GPC——而 fabric 曲线直读各自的私有 MEASURE_FREQ 域（MSD 在 bit 21）；坏 VFP 平面报出的 0 mV "伪锁" 不会渲染，除 #0 哨兵外全零的公共读回退为私有视图
  - **只读 vBIOS VF 阶梯**（Maxwell/Kepler 回退）：驱动曲线不可用时，vBIOS GPU Boost 2.0 阶梯（BIOS Range / BIOS Max / BIOS Min）叠加在图上——或直接成为整幅图，标题 "vBIOS VF ladder (read-only)"。对这些旧 GPU，曲线只读：编辑目标被降级，阶梯绝不是编辑目标

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/3b84f5a4-3aee-492c-b41c-5dd386903625" />

- **Output Console**：原生操作输出显示；相同的连续行折叠为 "(previous line repeated N×)" 摘要，逐秒轮询不再刷屏。

### 旧 GPU 行为

- **Maxwell/Kepler（及更早）**：vBIOS VF 阶梯是曲线形态且**只读**——绘图以只读渲染，编辑/应用目标被降级；阶梯绝不是编辑目标（65ca778）
- **Pre-Kepler**：P-State 内存时钟范围锁无法推导窗口，回退为原生单 P-State 钉住
- **Pre-Pascal**：没有 fabric 偏移行（仅 Core/Mem）；Pascal 上 Msd 偏移行禁用
- ≤ Kepler 卡片的风扇策略限定为 `default`/`manual`（现代 `continuous` CoolerPolicy 类型会被旧驱动拒绝）
- 不支持的界面会带明确判定置灰，而不是报错

### 稳定性与打包说明

- 只读查询工作线程串行化、V/F 刷新数据驻留内存；GPU 热插拔变化可恢复；修复了多个刷新操作竞争时的 FD 泄漏
- onefile 载荷经实测归档构成后瘦身（总计约 81.8 MB）：剔除 PIL（13.4 MB，由 plotext 静态导入引入，但其图像绘图功能未被使用）、`ssl` + `_hashlib` + libcrypto（无网络/加密需求）、以及 `aiohttp`/`msgpack`/`textual.dev`（textual devtools 客户端栈）（9153de3）。numpy/OpenBLAS 保留——plotext（VF 曲线）硬依赖它

### 测试

```bash
cd tui
uv run pytest
```

### 打包

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

### 注意

> 代码大部分由 CodeX 生成，功能尚在完善中。

*Maintained from: `gui/`, `tui/` source, CHANGELOG via git log.*
