# GUI Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC-GUI is a Python (CustomTkinter) graphical interface backed by the native `pynvoc` bindings ([[PyNvoc-API]]) for short queries/settings and the `nvoc-auto-optimizer` CLI ([[CLI-Guide]]) for long-running workflows. It provides a Dashboard (which hosts the overclock and fan-control panels), a VF Curve page (which hosts the Autoscan section), and a standalone output console window.

### Running

```bash
cd gui
uv sync
uv run python main.py
```

The GUI auto-detects `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`.

### What's new (2026-06 → 2026-09)

| Area | What changed |
|------|--------------|
| Layout | The standalone Overclock tab is gone — its panels are hosted on the Dashboard; Autoscan is now a section inside the VF Curve tab; the console opens as its own window |
| VF Curve | EXT-section per-domain current curves (1541be1), P0 workable-region shading (4ff0493), rail-aware crosshair incl. GPC (21c8fe1/802bc15), Shift+Reset whole-bank clear (ee28dd1), effective-curve synthesis fallback (7be32ad), read-only BIOS VF ladder + legacy interaction downgrade (e733e98) |
| Overclock | ClkDomains fabric/uncore offset pages with pager, mask gates, coupling RMW, per-domain resets (907493f), Blackwell plane-slot routing (0f99043), core/mem sliders with MHz/mV toggle (4fada95), mobile TGP slider anchored at the effective PPAB ceiling (927f120), fan-policy verdict (0c79f35/66a5abe), P-State lock warn-not-reject + pin fallback (0e19d3f) |
| Dashboard & stability | dGPU offline backoff re-probe and hotplug recovery, startup marshal-race fixes + slider deferred-range redraw + packaged diagnostics (ddc2b89), startup geometry auto-pin / chart UI-scale floor / boot phase markers / slim onefile payload (b01fadd) |

### Feature Tabs

#### Dashboard

- GPU info and real-time status (frequency, temperature, fans, VFP) as gradient metric rows (GPU / MEM / VOLT / TEMP / PWR) with a VFP lock indicator
- Current overclock settings overview
- Hosts the **Overclock** and **Fan Control** panels (the former standalone tabs) plus a 🖥 Console button for the output window
- When no GPU answers at startup (dGPU disabled), polling enters an offline backoff that re-runs discovery until the GPU returns; GPU hotplug changes are recovered without a restart

<img alt="image" width="50%" src="https://github.com/user-attachments/assets/7cfefc10-47e3-40aa-aeeb-f7d37f527c6f"/>

#### Autoscan (section of the VF Curve tab)

One-click automated VF curve optimization workflow (see [[Autoscan-Workflow]]):

1. Click **Export Init VFP** to save factory curve
2. Click **Reset & Unlock VFP** to prepare for scanning
3. Configure parameters (mode, score threshold, etc.)
4. Click **Start Autoscan** — output streams to console in real time
5. After scan completes, click **Fix Results** for post-processing
6. Click **Import Final VFP** to apply the optimized curve

Supports Standard / Ultrafast / Legacy modes.

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/5b7ca629-e6b2-4bab-a586-41bc50670cb9" />

#### Overclock (hosted on the Dashboard)

- Core frequency offset (`--core-offset`)
- Memory frequency offset (`--mem-offset`)
- Power limit (`-P`)
- Thermal limit (`-T`)
- Voltage Boost (`-V`)
- Manual fan speed control
- Preset profiles and slider adjustment
- NVAPI / NVML dual interface support

New since 2026-06:

- **Core/Mem rows are sliders with a MHz/mV unit toggle** (4fada95): the unit chip switches the row between the frequency-offset plane (MHz) and the ClkDomains slot-1 voltage plane (mV, Pascal and newer); an mV row remembers its MHz range for the toggle back
- **ClkDomains fabric/uncore offset pages** (907493f): the ⚡ V/F Offsets card gains a ◂ ▸ pager — page 1 **Xbar/Sys**, page 2 **Msd/Host** (pre-Pascal cards keep Core/Mem only and never show the pager)
  - An Xbar write couples SYS with a cancelling −f on 30-series+; the **Sys** row is a read-modify-write that stacks on top instead of overwriting that cancel
  - Rows are gated by the private ClockClient *controllable mask* — unsupported domains (e.g. MSD on Pascal, Host without mask bit 9) are disabled instead of erroring
  - Each row has its own ↺ reset next to the ✓ apply; on Blackwell (50-series) all offset paths route through the slot-2/3 plane slots automatically and force a VF-curve refresh (0f99043)
- **API semantics**: NVAPI offset values are kHz, NVML values are MHz; NVAPI-only pages gray out under NVML
- **Mobile panel**: PPAB (Dynamic Boost) toggle, D-Notifier level selector, and a TGP slider anchored at the *effective* ceiling — min of the requested TGP and the active D-Notifier cap (nvidia-smi's "PPAB Ceiling: Current"), clamped into the TGP range (927f120); plus a target-temp slider and a Volt Limit slider bounded by the private VoltRails P0 walls
- **Fan-control verdicts** (0c79f35/66a5abe): the NVAPI cooler family decides the policy list — modern cards are restricted to `continuous` (other policies silently no-op on modern cooler paths); legacy (≤ Kepler) cards get `default`/`manual`. Fanless server cards (NVML reports zero coolers) and mobile GPUs gray out the fan pane (760cda9)
- **P-State lock**: the roster is normalized idle-first (p8 → p0, b7c3fb8); a range lock that overlaps other P-States with identical memory clocks now applies with a **warning** instead of being rejected; on pre-Kepler parts the lock falls back at runtime to the native single-P-State pin (0e19d3f/ae5f3f8); the "Lock target: Px" caption is anchored to the active selection (65722a5)

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/debbe0de-25e7-42d6-811c-6c2670cc7bd5" />

#### VF Curve

- Export / import VFP curves (CSV format)
- Lock / unlock voltage points
- Single-point frequency adjustment
- Terminal plot preview

New since 2026-06 — the chart is multi-curve and rail-aware:

- **Per-domain curves**: GPC, XBAR, MSD (the third curve, relabeled from SYS — it is the domain the ClkDomains bit-5 record drives), and the HBM MEM curve on Pascal-HBM compute cards; a 50-series fourth curve the ordinal table cannot name displays as UNKn. Each has current (solid) / default (dashed) series and a visibility checkbox
- **EXT-section per-domain current curves** (1541be1): domain-current series packed in the private record EXT slots (Turing: XBAR/SYS/MSD/HOST; Ampere: SYS/MSD/HOST; Ada: SYS/HOST) plot as display-only overlays, attributed by the segments actually present in the loaded table
- **P0 workable-voltage region** (4ff0493, 72bc45a): the band between the hardware floor (min hold) and the ceiling min(vBIOS wall, VRM max wall) is shaded; deep-red lines label **P0 floor** / **P0 ceiling**, and the light-red **P0 eff volt lim** line follows every Volt Limit apply. The effective wall is **draggable** via a triangle handle in the margin — the handle grays out on non-GPC curves of single-rail parts, where the wall write would target a rail the curve does not own (b949f3c)
- **Per-rail P0 bounds** (760cda9): walls follow the active curve's rail — gpc/msd ride the primary (core) rail, xbar/mem the secondary MSVDD/HBM rail; single-rail parts fall back to the primary for every curve
- **Rail-aware crosshair** (21c8fe1/802bc15): the live point reads the rail current from VoltRails — GPC included — while the fabric curves (xbar/msd/mem) direct-read their private MEASURE_FREQ domain (MSD measures on bit 21, not the SYS channel); one true voltage source, no 5–20 mV split between curves
- **Shift+Reset** (ee28dd1): holding Shift while clicking 🔁Reset on a private curve clears the WHOLE private VFP bank in one read-modify-write (instead of a slow per-point loop) and zeroes every domain's ClkDomains global offset; a normal Reset also clears the active curve's own domain global offset, because curve-point offsets and domain global offsets are separate storage (6130566)
- **Honest axes under volt offsets** (7be32ad, 581eb79/7585cfb): the private GPC segment is the default-axis authority; a healthy public read still supplies the live current curve on its own shifted grid (public = private + slot-1 offset, reported as "+X mV vs private default axis"); when ClkDomains offsets are readable, an **effective curve is synthesized** (base shifted by the µV voltage addend + kHz frequency offset) as a display fallback
- **Broken-read guards** (41b26e4, 71b5ba8): a public read that zeroed out except the #0 sentinel point (old driver + positive offset, e.g. live V100) is detected and replaced by the private view; the 0 mV "pseudo-lock" reported by a broken VFP plane is never rendered as a lock
- **Read-only BIOS VF ladder** (e733e98): when the driver curve is unavailable — Maxwell/Kepler with fallback — the vBIOS GPU Boost 2.0 ladder is fetched once per GPU and drawn as the curve, strictly view-only (see Legacy GPU behavior below)
- Switching GPUs clears the chart (no lingering foreign curve) and an explicit "not supported" verdict appears when the private VFP family is absent (6e2c091)

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/e7f79884-abbd-4ea6-9a54-9584e0c2f20e" />

#### Output Console

- Real-time CLI/native output for all operations, in its own window (opened from the Dashboard's 🖥 Console button); log lines buffer until first open
- Identical consecutive lines collapse into one line plus a "(previous line repeated N×)" summary (e8528df)
- In packaged builds the console mirrors to `%LOCALAPPDATA%\nvoc-gui\console.log`, and Tk callback tracebacks land in `%LOCALAPPDATA%\nvoc-gui\callback_tracebacks.log` (ddc2b89)

### Legacy GPU behavior

- **Maxwell/Kepler (and older)**: the vBIOS VF ladder is the only curve representation and is **view-only** — drag-edit, wall-drag and apply targets are downgraded; the ladder can still be inspected, exported and compared, but it is never an edit target (e733e98 GUI, 65ca778 TUI)
- **Pre-Kepler**: the P-State memory-clock-range lock cannot derive its window (NVML mem-clock ranges unsupported) and falls back to the native single-P-State pin
- **Pre-Pascal**: the overclock pager never appears — Core/Mem rows only, no ClkDomains fabric pages
- Fan policy on ≤ Kepler cards is restricted to `default`/`manual` (the modern `continuous` CoolerPolicy type is rejected by old drivers); legacy fan parsing covers Fermi
- Unsupported surfaces gray out with an explicit verdict instead of erroring

### Stability & diagnostics notes

- Startup: worker→UI marshal races fixed — a background result is retried instead of dropped while the UI loop is still coming up (ddc2b89); the startup window geometry is pinned and re-applied after CTk's DPI re-apply (b01fadd); boot phase markers with wall-clock stamps go to the support log; BLAS/OpenMP thread pools are capped (~700 MB less commit)
- The 100%-display UI-scale floor is scoped to the chart package only: on 100%-scaling 1080p screens the VF-curve chart renders at ≥1.25× effective scale without inflating the rest of the UI (b01fadd, 5b8f741)
- Slider/range redraws are deferred while a resize or window-move session is active and flushed once at settle (ddc2b89); hidden-tab and tray-minimized dashboard polling pauses; unchanged VF-curve and slider redraws are skipped
- Packaged diagnostics live under `%LOCALAPPDATA%\nvoc-gui\` (`console.log`, `callback_tracebacks.log`, matplotlib cache under `mpl\`)
- The onefile payload trims `ssl`/`jinja2`/`pyparsing.diagram`/AVIF codecs and matplotlib's sample data / non-TTF fonts (b01fadd)

### Architecture

```
main.py → src/app.py → src/tabs/dashboard (+ sections/overclock, sections/fan)
                     → src/tabs/vfcurve (+ sections/autoscan)
                     → src/widgets/*
         → src/backend/native.py → pynvoc ([[PyNvoc-API]])
         → src/cli_runner.py → nvoc-auto-optimizer CLI ([[CLI-Guide]])
         → src/config.py
```

- **`src/backend/`**: adapts GUI operations to native `pynvoc` calls (short queries/settings) or CLI arguments
- **`src/cli_runner.py`**: invokes the CLI as a subprocess, passes arguments and parses output (Autoscan, VFP export)
- **`src/config.py`**: JSON-based configuration management
- **`src/tabs/`**: dashboard (with overclock/fan sections) and vfcurve (with autoscan section)

### Configuration

- Config file: `nvoc_gui_config.json`
- Customizable CLI path, GPU selection, etc.

### Packaging

```powershell
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

---

<a id="chinese"></a>

## 中文

NVOC-GUI 是基于 Python（CustomTkinter）的图形界面前端：短耗时查询/设置走原生 `pynvoc` 绑定（[[PyNvoc-API]]），长耗时工作流走 `nvoc-auto-optimizer` CLI（[[CLI-Guide]]）。窗口由 Dashboard（承载超频与风扇控制面板）、VF Curve 页（承载 Autoscan 区块）和独立的输出控制台窗口组成。

### 运行

```bash
cd gui
uv sync
uv run python main.py
```

GUI 会自动检测 `../auto-optimizer/target/release/nvoc-auto-optimizer.exe`。

### 新变化（2026-06 → 2026-09）

| 领域 | 变化 |
|------|------|
| 布局 | 独立的 Overclock 页签已取消——其面板由 Dashboard 承载；Autoscan 成为 VF Curve 页内的区块；控制台改为独立窗口 |
| VF Curve | EXT 区段分域实时曲线（1541be1）、P0 可工作电压区着色（4ff0493）、含 GPC 的 rail 感知十字线（21c8fe1/802bc15）、Shift+Reset 整库清除（ee28dd1）、有效曲线合成回退（7be32ad）、只读 BIOS VF 阶梯 + 旧卡交互降级（e733e98） |
| 超频 | ClkDomains fabric/uncore 偏移分页（分页器、掩码门控、耦合读改写、分域重置，907493f）、Blackwell plane-slot 路由（0f99043）、核心/显存滑块的 MHz/mV 切换（4fada95）、移动端 TGP 滑块锚定有效 PPAB 上限（927f120）、风扇策略判定（0c79f35/66a5abe）、P-State 锁重叠警告 + 运行时回退（0e19d3f） |
| Dashboard 与稳定性 | dGPU 离线退避重探与热插拔恢复、启动 marshal 竞态修复 + 滑块延迟重绘 + 打包诊断（ddc2b89）、启动几何自动钉住 / 图表 UI 缩放下限 / 启动阶段标记 / 瘦身 onefile 载荷（b01fadd） |

### 功能页签

#### Dashboard

- GPU 信息与实时状态（频率、温度、风扇、VFP），以渐变指标行（GPU / MEM / VOLT / TEMP / PWR）和 VFP 锁定指示展示
- 当前超频设置一览
- 承载 **Overclock** 与 **Fan Control** 面板（原独立页签），并提供 🖥 Console 按钮打开输出窗口
- 启动时无 GPU 应答（dGPU 被禁用）时，轮询进入离线退避并反复重新发现，直到 GPU 回归；GPU 热插拔变化无需重启即可恢复

<img alt="image" width="50%" src="https://github.com/user-attachments/assets/7cfefc10-47e3-40aa-aeeb-f7d37f527c6f"/>

#### Autoscan（VF Curve 页内区块）

一键自动优化 VF 曲线工作流（见 [[Autoscan-Workflow]]）：

1. 点击 **Export Init VFP** 保存出厂曲线
2. 点击 **Reset & Unlock VFP** 准备扫描
3. 配置参数（模式、分数阈值等）
4. 点击 **Start Autoscan** — 输出实时流式显示到控制台
5. 扫描完成后点击 **Fix Results** 后处理
6. 点击 **Import Final VFP** 应用优化曲线

支持 Standard / Ultrafast / Legacy 三种模式。

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/5b7ca629-e6b2-4bab-a586-41bc50670cb9" />

#### Overclock（由 Dashboard 承载）

- 核心频率偏移（`--core-offset`）
- 显存频率偏移（`--mem-offset`）
- 功耗墙（`-P`）
- 温度墙（`-T`）
- Voltage Boost（`-V`）
- 手动风扇转速控制
- 预设方案与滑块调节
- NVAPI / NVML 双接口支持

2026-06 以来的新变化：

- **核心/显存行变为带 MHz/mV 单元切换的滑块**（4fada95）：单元芯片在频率偏移平面（MHz）与 ClkDomains slot-1 电压平面（mV，Pascal 及更新）之间切换；切到 mV 的行会记住 MHz 量程以便切回
- **ClkDomains fabric/uncore 偏移分页**（907493f）：⚡ V/F Offsets 卡片新增 ◂ ▸ 分页器——第 1 页 **Xbar/Sys**，第 2 页 **Msd/Host**（pre-Pascal 卡片只有 Core/Mem，不显示分页器）
  - 30 系及以上，Xbar 写入会以 −f 耦合 SYS；**Sys** 行是读-改-写，叠加在该取消量之上而非覆盖
  - 各行由私有 ClockClient *可控制掩码* 门控——不支持的域（如 Pascal 的 MSD、无掩码位 9 的 Host）直接禁用而非报错
  - 每行在 ✓ 应用旁有独立 ↺ 重置；Blackwell（50 系）上所有偏移路径自动改走 slot-2/3 plane slots 并强制刷新 VF 曲线（0f99043）
- **API 语义**：NVAPI 偏移值单位为 kHz，NVML 为 MHz；NVAPI 专属页面在 NVML 下置灰
- **移动端面板**：PPAB（Dynamic Boost）开关、D-Notifier 档位选择器，以及锚定在*有效*上限的 TGP 滑块——取请求 TGP 与当前 D-Notifier 上限的较小值（即 nvidia-smi 的 "PPAB Ceiling: Current"），并钳制进 TGP 量程（927f120）；另有目标温度滑块和以私有 VoltRails P0 墙为边界的 Volt Limit 滑块
- **风扇控制判定**（0c79f35/66a5abe）：由 NVAPI cooler 家族决定策略列表——新卡仅限 `continuous`（其他策略在现代 cooler 路径上静默无效）；旧卡（≤ Kepler）提供 `default`/`manual`。无风扇服务器卡（NVML 风扇数为 0）与移动端 GPU 会将风扇面板置灰（760cda9）
- **P-State 锁**：名单规范化为空闲优先（p8 → p0，b7c3fb8）；范围锁与其他内存时钟相同的 P-State 重叠时改为**警告后照常应用**而非拒绝；pre-Kepler 部件运行时自动回退为原生单 P-State 钉住（0e19d3f/ae5f3f8）；"Lock target: Px" 标题锚定到当前选择（65722a5）

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/debbe0de-25e7-42d6-811c-6c2670cc7bd5" />

#### VF Curve

- 导出 / 导入 VFP 曲线（CSV 格式）
- 锁定 / 解锁电压点
- 单点频率调整
- 绘图预览

2026-06 以来的新变化——图表支持多曲线并具备 rail 感知：

- **分域曲线**：GPC、XBAR、MSD（第三条曲线，由 SYS 更名——它对应 ClkDomains bit-5 记录驱动的域），以及 Pascal-HBM 计算卡上的 HBM MEM 曲线；50 系第四条无法命名的曲线显示为 UNKn。每条曲线都有实时（实线）/ 默认（虚线）两条序列和可见性复选框
- **EXT 区段分域实时曲线**（1541be1）：打包在私有记录 EXT 槽位中的分域电流序列（Turing：XBAR/SYS/MSD/HOST；Ampere：SYS/MSD/HOST；Ada：SYS/HOST）以仅显示的叠加线绘制，归因依据本次表中实际存在的段
- **P0 可工作电压区**（4ff0493，72bc45a）：硬件下限（min hold）与上限 min(vBIOS 墙, VRM 最大墙) 之间的区带被着色；深红线标注 **P0 floor** / **P0 ceiling**，浅红 **P0 eff volt lim** 线随每次 Volt Limit 应用移动。有效墙可通过页边三角形手柄**拖拽**——单 rail 卡片的非 GPC 曲线上手柄置灰，因为此时写墙会落到该曲线并不拥有的 rail（b949f3c）
- **分 rail 的 P0 边界**（760cda9）：墙跟随活动曲线的 rail——gpc/msd 走主（核心）rail，xbar/mem 走次级 MSVDD/HBM rail；单 rail 卡片对所有曲线回退到主 rail
- **Rail 感知十字线**（21c8fe1/802bc15）：实时点从 VoltRails 读取 rail 电流——包括 GPC——而 fabric 曲线（xbar/msd/mem）直读各自的私有 MEASURE_FREQ 域（MSD 测量在 bit 21，而非 SYS 通道）；单一电压真源，曲线之间不再有 5–20 mV 分裂
- **Shift+Reset**（ee28dd1）：在私有曲线上按住 Shift 点击 🔁Reset，一次读-改-写清除整个私有 VFP 库（取代缓慢的逐点循环），并清零所有域的 ClkDomains 全局偏移；普通 Reset 也会清除活动曲线自身域的全局偏移，因为曲线点偏移与域全局偏移是独立存储（6130566）
- **电压偏移下的诚实坐标轴**（7be32ad，581eb79/7585cfb）：私有 GPC 段是默认轴权威；健康的公共读仍在其自身漂移网格上提供实时曲线（public = private + slot-1 偏移，控制台报告 "+X mV vs private default axis"）；当 ClkDomains 偏移可读时，会**合成有效曲线**（基准曲线按 µV 电压加数 + kHz 频率偏移平移）作为显示回退
- **坏读保护**（41b26e4，71b5ba8）：除 #0 哨兵点外全部为零的公共读（旧驱动 + 正偏移，如实测 V100）会被识别并替换为私有视图；坏 VFP 平面报出的 0 mV "伪锁" 不会作为锁渲染
- **只读 BIOS VF 阶梯**（e733e98）：当驱动曲线不可用时——Maxwell/Kepler 及回退场景——vBIOS GPU Boost 2.0 阶梯每次 GPU 查询一次并作为曲线绘制，严格只读（见下方"旧 GPU 行为"）
- 切换 GPU 会清空图表（不残留他卡曲线），私有 VFP 家族缺失时显示明确的 "not supported" 判定（6e2c091）

<img width="50%" alt="image" src="https://github.com/user-attachments/assets/e7f79884-abbd-4ea6-9a54-9584e0c2f20e" />

#### 输出控制台

- 所有操作的 CLI/原生输出实时展示，独立成窗（从 Dashboard 的 🖥 Console 按钮打开）；日志行在首次打开前先缓冲
- 相同的连续行折叠为一行加 "(previous line repeated N×)" 摘要（e8528df）
- 打包构建中，控制台镜像到 `%LOCALAPPDATA%\nvoc-gui\console.log`，Tk 回调堆栈写入 `%LOCALAPPDATA%\nvoc-gui\callback_tracebacks.log`（ddc2b89）

### 旧 GPU 行为

- **Maxwell/Kepler（及更早）**：vBIOS VF 阶梯是唯一的曲线形态且**只读**——拖拽编辑、拖墙与应用目标全部降级；阶梯仍可查看、导出和对比，但绝不是编辑目标（e733e98 GUI，65ca778 TUI）
- **Pre-Kepler**：P-State 内存时钟范围锁无法推导窗口（NVML 显存时钟范围不支持），回退为原生单 P-State 钉住
- **Pre-Pascal**：超频分页器不出现——仅有 Core/Mem 行，没有 ClkDomains fabric 页
- ≤ Kepler 卡片的风扇策略限定为 `default`/`manual`（现代 `continuous` CoolerPolicy 类型会被旧驱动拒绝）；旧风扇解析覆盖 Fermi
- 不支持的界面会带明确判定置灰，而不是报错

### 稳定性与诊断说明

- 启动：修复 worker→UI marshal 竞态——UI 循环尚未起来时后台结果会重试而不是丢弃（ddc2b89）；启动窗口几何被钉住并在 CTk 的 DPI 重应用后恢复（b01fadd）；带毫秒时间戳的启动阶段标记写入支持日志；BLAS/OpenMP 线程池被限制（少提交约 700 MB）
- 100% 显示的 UI 缩放下限仅作用于图表包：100% 缩放的 1080p 屏幕上 VF 曲线图以 ≥1.25× 有效缩放渲染，不再放大其余 UI（b01fadd，5b8f741）
- 滑块/量程重绘在调整大小或移动窗口会话期间延迟、落定后统一刷新（ddc2b89）；隐藏页签与托盘最小化时暂停仪表轮询；未变化的 VF 曲线与滑块重绘被跳过
- 打包诊断位于 `%LOCALAPPDATA%\nvoc-gui\`（`console.log`、`callback_tracebacks.log`，matplotlib 缓存在 `mpl\` 下）
- onefile 载荷剔除 `ssl`/`jinja2`/`pyparsing.diagram`/AVIF 编解码器以及 matplotlib 的示例数据 / 非 TTF 字体（b01fadd）

### 架构

```
main.py → src/app.py → src/tabs/dashboard（含 sections/overclock、sections/fan）
                     → src/tabs/vfcurve（含 sections/autoscan）
                     → src/widgets/*
         → src/backend/native.py → pynvoc（[[PyNvoc-API]]）
         → src/cli_runner.py → nvoc-auto-optimizer CLI（[[CLI-Guide]]）
         → src/config.py
```

- **`src/backend/`**：将 GUI 操作适配到原生 `pynvoc` 调用（短查询/设置）或 CLI 参数
- **`src/cli_runner.py`**：以子进程方式调用 CLI，传递参数并解析输出（Autoscan、VFP 导出）
- **`src/config.py`**：JSON 格式配置管理
- **`src/tabs/`**：dashboard（含超频/风扇区块）与 vfcurve（含 autoscan 区块）

### 配置

- 配置文件：`nvoc_gui_config.json`
- 可自定义 CLI 路径、GPU 选择等

### 打包

```powershell
uv sync --group build
uv run pyinstaller nvoc_gui.spec
```

*Maintained from: `gui/`, `tui/` source, CHANGELOG via git log.*
