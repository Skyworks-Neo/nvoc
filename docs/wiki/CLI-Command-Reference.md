# nvoc-cli Command Reference

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Per-command reference for `nvoc-cli` (see [[CLI-Guide]] for the global argument model, backends, and privileges). 101 GPU commands plus the `list` meta command, organized into the same 9 families the CLI itself uses (`nvoc-cli list <group>`). Purpose tables are bilingual (Purpose / 用途).

Conventions: `[arg]` = required positional, `<x>` = option value, `…` = repeatable. Every command accepts the global options (`--gpu/-g`, `--nvapi|--nvml`, `--output/-O`, `--no-color`) and `--help`. Flags marked **NVML** or **NVAPI** are backend-restricted; unmarked commands with both backends default to auto (NVAPI first, NVML fallback). "Private" = undocumented NVAPI surface — structure versions vary by driver; read-only use recommended.

> ⚠️ Writes are high-risk: read first, record the original value, verify the readback, know the reset ([[Safety-and-Recovery]]).

### Info family (`info`, 14 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `list [GROUP]` | List all commands grouped by family (meta command, no GPU access) | 按族列出全部命令（元命令，不访问 GPU） |
| `get-gpu-list` | List discovered GPUs and available backends (NVML/NVAPI) | 列出发现的 GPU 与可用后端 |
| `get-uuid` | Read GPU UUID | 读取 GPU UUID |
| `get-info` | Read NVAPI GPU identity/capability info + NVAPI interface version + PCIe gen + BAR info | 读取 NVAPI GPU 身份/能力信息 + NVAPI 接口版本 + PCIe 代际 + BAR 信息 |
| `get-status [--verbose]` | Read NVAPI live GPU status (clocks, temps, power, utilization) | 读取 NVAPI 实时 GPU 状态（频率、温度、功率、占用率） |
| `get-settings` | Read NVAPI overclock settings (per-P-State deltas) | 读取 NVAPI 超频设置（各 P-State 偏移） |
| `get-throttle-reasons` **NVML** | Read NVML throttle (performance-limit) reasons | 读取 NVML 降速（性能限制）原因 |
| `get-vbios [--out F] [--dump] [--maxwell-vftable-decode]` | Read the VBIOS image + BIT summary (Windows-only) | 读取 VBIOS 镜像 + BIT 摘要（仅 Windows） |
| `get-display-list [--all]` | List NVAPI display IDs for EDID operations | 列出 NVAPI 显示器 ID（供 EDID 操作） |
| `get-edid DISPLAY_ID` | Read display EDID through NVAPI | 经 NVAPI 读取显示器 EDID |
| `set-edid DISPLAY_ID EDID_HEX` | Set display EDID through NVAPI | 经 NVAPI 写入显示器 EDID |
| `clear-edid DISPLAY_ID` | Clear display EDID through NVAPI | 经 NVAPI 清除显示器 EDID |
| `set-ecc-configuration ENABLED [--immediate]` | Set NVAPI ECC memory configuration (persists in NVRAM) | 设置 NVAPI ECC 内存配置（持久化到 NVRAM） |
| `restart-display-driver` | Restart the display driver (legacy apply-OC trigger) | 重启显示驱动（旧式"应用超频"触发器） |

#### get-info

```bash
nvoc-cli get-info --gpu 0
```

Human output promotes the NVAPI interface version (e.g. `R580`) to a banner line — it marks the driver's API generation, which gates which private/stamp-gated families the driver honors. JSON adds `nvapi_interface_version`, `max_pcie_link_gen` (from NVML), and `bar_info` (GetBarInfo, best-effort).

#### get-status

```bash
nvoc-cli -O json get-status --gpu 0
nvoc-cli get-status --verbose        # + VFP table, raw power_monitor, D-Notifier D1–D5 table
```

Includes the `all_clocks_detailed` section (GetAllClocks V2): per-domain effective frequency plus the driver's `ratio_domain`/`ratio` parent link and `reserved` dwords when non-zero. Also augments NVML live data: `power_draw_w`, `power_limit_w`, and the mobile `d_notifier` level (private ClientPowerPoliciesGetInfo) that silently clamps the TGP wall.

#### get-vbios

```bash
nvoc-cli get-vbios --gpu 0                      # summary: version/size/BIT
nvoc-cli get-vbios --out vbios.rom              # write raw image
nvoc-cli get-vbios --dump                       # full BIT token table + Fermi-model raw blocks
nvoc-cli get-vbios --maxwell-vftable-decode     # GPU Boost 2.0 V/F ladder dump
```

Windows-only (`NvAPI_GPU_GetVbiosImage`, escape-gated). Best-effort companions: the VBIOS **security word** (QueryVbiosSecurityInfo) and **status string** (QueryVbiosStatusString).

- The built-in BIT-family parser locates the ASCII `BIT` signature (the classic 0x48 pointer does not hold on all images), anchor-scans the 6-byte token table, and auto-detects the data base (BIT-relative on Fermi-era images, image-relative on Pascal — Pascal's `'P'` token is a u32 pointer directory).
- `--dump` surfaces the perf table (`'P'`) and voltage blocks (`'M'`/`'V'`/`'U'`) as raw bytes where layouts are undecoded (Fermi = discrete pstate × clock × VID, no V/F curve).
- `--maxwell-vftable-decode` decodes the **Maxwell GPU Boost 2.0 ladder** (BIT `'P'`+0x34, v0x10 — RE'd off MaxwellBiosTweaker; applies to GM10x/GM20x): 79 GPC points, each with u16 half-MHz frequency and a vmap index; voltage (min/max mV) resolves through the vmap table; printed as an `Id | V_min | V_max | Freq` table plus P-State boundary marks. Other generations error with "no boost-ladder table".

#### set-ecc-configuration

```bash
nvoc-cli set-ecc-configuration on                # deferred to next reboot (default)
nvoc-cli set-ecc-configuration on --immediate    # apply now (hardware-dependent support)
```

`NvAPI_GPU_SetECCConfiguration` — persists in non-volatile memory; post-SET readback from GetECCConfigurationInfo. GeForce consumer GPUs typically return `NotSupported`. Check `get-status`'s `Ecc` section first.

### Power family (`power`, 14 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `get-public-power-limit` | Power-limit range — NVML watts (min/current/max) or NVAPI TDP percent + mobile tgp_range | 功率限制范围——NVML 瓦特（min/current/max）或 NVAPI TDP 百分比 + 移动端 tgp_range |
| `set-power-limit WATT [--policy-index N]` | Set TGP in watts (NVAPI mobile slider / NVML power-management limit) | 按瓦特设置 TGP（NVAPI 移动端滑条 / NVML 功率管理限制） |
| `reset-power-limit [--policy-index N]` | Reset NVAPI TGP to rated/default (mobile) | 将 NVAPI TGP 复位到额定/默认（移动端） |
| `get-power-ceiling` | Effective power wall on PPAB mobiles: requested TGP + D-Notifier cap, ceiling = min | PPAB 移动端的有效功率墙：请求 TGP + D-Notifier 上限，ceiling = 二者取小 |
| `set-public-tgp-percent PCT` | Set NVAPI power limit in percent (ClientPowerPolicies) | 按百分比设置 NVAPI 功率限制 |
| `reset-public-tgp-percent` | Reset NVAPI power limits | 复位 NVAPI 功率限制 |
| `get-rated-tdp` | Rated-TDP readback trio | 额定 TDP 三路回读 |
| `get-dnotifier` / `set-dnotifier LEVEL` | D-Notifier (D0-notify) level + D1–D5 power-cap table (mobile) | D-Notifier（D0-notify）级别 + D1–D5 功率上限表（移动端） |
| `set-ppab-status on/off` | NVAPI PPAB / Dynamic-Boost enable | NVAPI PPAB / Dynamic-Boost 开关 |
| `get-power-mode` / `set-power-mode max\|balanced` | NVIDIA App power mode (with support gate) | NVIDIA App 功率模式（含支持性门控） |
| `set-whispermode2-status on/off [--mode M]` | Whisper Mode 2.0 + acoustic mode (mobile) | Whisper Mode 2.0 + 声学模式（移动端） |
| `set-batteryboost2-status on/off` | Battery Boost 2.0 enable (mobile) | Battery Boost 2.0 开关（移动端） |

#### get-public-power-limit (dual backend — absorbed get-power-limit)

```bash
nvoc-cli get-public-power-limit              # auto → NVML: min/current/max watts
nvoc-cli get-public-power-limit --nvapi      # TDP percent range + tgp_range when exposed
```

NVML branch = nvidia-smi `-pl` territory. NVAPI branch reads the ClientPowerPolicies TDP percent range and adds the private mobile `tgp_range` (watts, per `--policy-index`) when the driver answers; missing entries are `null`. The old `get-power-limit` / `get-tdp-temp-limits` names are gone — this command (plus `get-public-temp-limit`) absorbed them.

#### get-power-ceiling

```bash
nvoc-cli get-power-ceiling
```

The **effective PPAB wall**: ceiling = min(requested TGP, active D-Notifier cap). Human output shows Ceiling / Requested TGP / D-Notifier Cap / Default TGP in watts; unsupported GPUs return `{"supported": false}`.

#### set-power-limit

```bash
nvoc-cli set-power-limit 140              # auto: NVAPI first
nvoc-cli set-power-limit 140W --nvml      # NVML power-management limit
```

Watts positional accepts a `W` suffix. The driver clamps to the valid range — record `get-public-power-limit` output first.

### Thermal family (`thermal`, 9 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `get-temp-thresholds` | Temperature thresholds — NVML (Shutdown/Slowdown/…) by default; `--nvapi` exposes target-temp policy | 温度阈值——默认 NVML；`--nvapi` 暴露 target-temp 策略 |
| `get-public-temp-limit` | NVAPI public temp-limit range (min/default/max °C + throttle curve); missing = `null` | NVAPI 公开温度限制范围（缺失项为 `null`） |
| `set-temp-limit C [--domain gpu\|acoustic]` | Set thermal limit — NVAPI sensor limit; NVML GPU max-temp, or acoustic with `--domain acoustic` | 设置温度墙——NVAPI 传感器限制；NVML 为 GPU 最高温阈值，`--domain acoustic` 为声学目标 |
| `reset-temp-limit` | Reset NVAPI sensor limits | 复位 NVAPI 传感器限制 |
| `set-private-target-temp-limit C [--policy-index N]` | NVAPI target-temp policy slot in °C (mobile) | NVAPI target-temp 策略槽（移动端） |
| `get-legacy-temp-sensor` | NVAPI legacy 3-sensor view (GPU/Memory/Board, live + physical range) | NVAPI 旧式三传感器视图 |
| `get-temp-sim` / `set-temp-sim C` / `reset-temp-sim` | Temperature-simulation state (Secured-Overrides gated) | 温度仿真状态（受 Secured-Overrides 门控） |

```bash
nvoc-cli get-temp-thresholds
nvoc-cli set-temp-limit 83
nvoc-cli set-temp-limit 78 --domain acoustic --nvml   # Linux channel; Windows rejects NVML threshold writes
```

On Windows the NVML threshold setter is rejected — use `set-private-target-temp-limit` there. `set-temp-sim` fakes the driver-visible temperature: a **DANGEROUS research tool** (fan-control experiments only, restore with `reset-temp-sim`).

### Fan family (`fan`, 8 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `get-fan-info` | Fan/cooler info — NVML count + min/max %; NVAPI per-cooler private info | 风扇/cooler 信息——NVML 数量与 min/max%；NVAPI 私有 per-cooler 信息 |
| `set-fan-speed VALUE [--percent] [--rpm] [--fan F] [--policy P] [--cooler N]` | Set fan speed: % (NVAPI SetCoolerLevels / NVML) or RPM (NVAPI-only private) | 设置风扇转速：百分比或 RPM（仅 NVAPI 私有路径） |
| `reset-fan-speed [--rpm] [--fan F] [--cooler N]` | Restore fan control — NVAPI clears the cooler level override / NVML default | 恢复风扇控制——NVAPI 清除 cooler level 覆盖 / NVML 恢复默认 |
| `get-fan-curve` / `set-fan-curve` / `reset-fan-curve` | NVAPI fan-curve table (ClientFanPolicies, desktop-only) | NVAPI 风扇曲线表（仅桌面卡） |
| `set-fanstop-status on/off [--curve N]` | Fan-stop / zero-RPM per curve slot (FanArbiterSet) | 每个曲线槽的停转/零转开关 |
| `get-legacy-fan-policy` | Fan-policy capabilities (V2 raw block on modern drivers; legacy V1 decoded on R391-era) | 风扇策略能力（新驱动为 V2 原始块；R391 时代解码 V1） |

#### set-fan-speed / reset-fan-speed

```bash
nvoc-cli set-fan-speed 60 --fan all --policy manual             # percent (default)
nvoc-cli set-fan-speed 1800 --rpm --nvapi                       # physical RPM (private FanCoolerSetControl)
nvoc-cli reset-fan-speed                                        # unpin the fan
nvoc-cli reset-fan-speed --rpm --cooler 0                       # disable fan-speed simulation + clear enable bit
```

- `--policy` valid values are enumerated in help (default `manual`): **NVML** `manual`, `auto` (= `continuous`, temperature-driven). **NVAPI**: those plus `perf`, `discrete`, `hybrid`, `software` (silent-ODT), `default`, `default32`.
- Live A/B (GUI/TUI work): on **modern cards only `continuous` (TemperatureContinuous) actually applies the manual % level via NVAPI**; the rest of the old dropdown no-ops or is rejected. Legacy (≤ Kepler, e.g. GT730/Fermi R391) lands on `manual`.
- `reset-fan-speed` default path clears the NVAPI cooler level override (control-block bit0) — the **only reset that unpins modern cards**. A specific `--fan` requires `--nvml` (NVAPI resets all coolers).
- RPM is clamped to the cooler `[min, max]` from `get-fan-info --nvapi`.

#### fan curves

```bash
nvoc-cli get-fan-curve
nvoc-cli set-fan-curve 0 40:800,60:1200,75:1800   # slot idx + 3 strictly-monotonic temp:rpm points
nvoc-cli reset-fan-curve --curve 0                # factory slot restore (works where NVAPI reset is NOT_SUPPORTED)
nvoc-cli set-fanstop-status on --curve 0
```

Points must be strictly increasing in temperature AND RPM. `reset-fan-curve` goes through the private FanPolicy path (0x2B2A2A45).

### Clock family (`clock`, 20 commands) — offsets, locks, P-States

| Command | Purpose | 用途 |
|---|---|---|
| `get-pstate-global-freq-offset [--domain] [--pstate]` | Read clock offset in MHz (NVAPI/NVML) | 读取 MHz 频率偏移 |
| `set-pstate-global-freq-offset OFFSET [--domain] [--pstate]` | Set clock offset in MHz for any domain | 设置任意时钟域的 MHz 频率偏移 |
| `reset-pstate-global-freq-offset [--domain]` | Reset offsets for all touched pstate/domain pairs, or filter with `--domain` | 复位全部已触达 (pstate, domain) 对，或按 `--domain` 过滤 |
| `get-pstate-freq-range` **NVML** | Read NVML P-State clock ranges | 读取 NVML P-State 频率范围 |
| `set-freq-lock MIN MAX [--domain]` | Lock core/memory clocks to a MHz range (NVAPI VFP lock / NVML hard floor+ceiling) | 将核心/显存频率锁定到 MHz 区间 |
| `reset-freq-lock [--domain]` | Reset locked clocks | 复位频率锁 |
| `get-pstate-lock [--domain N]` | Read the native NVAPI P-State level table | 读取原生 NVAPI P-State 级别表 |
| `set-pstate-lock PSTATE` | Lock the native NVAPI P-State (admin) | 锁定原生 NVAPI P-State（需管理员） |
| `reset-pstate-lock` | Clear all native NVAPI P-State locks | 清除全部原生 P-State 锁 |
| `set-pstate-lock-via-mem-range FIRST [SECOND]` | Lock one P-State or a contiguous range via memory freq range (NVAPI/NVML) | 经显存频率范围锁定单个或连续 P-State 区间 |
| `set-private-forced-pstate-lock-user PSTATE [--set-type T]` | Force a P-State (private SetForcePstate) | 强制锁定 P-State（私有 SetForcePstate） |
| `reset-private-forced-pstate-lock-user` | Release the forced lock (SetForcePstate sentinel 16) | 释放强制锁 |
| `set-private-permanent-pstate-lock-user LEVEL` | Admin-free permanent P-State lock (SetPerfLevel; index-based; reboot clears) | 免管理员永久 P-State 锁（按索引；重启清除） |
| `get-private-legacy-pstates20-freq-domain-info` | Read-only dump of the private pstates-2.0 delta table | 私有 pstates-2.0 delta 表只读转储 |
| `set-private-legacy-pstates20-freq-domain-global-offset DELTA [--pstate] [--domain] [--flags]` | RMW one delta in the private pstates table (DANGEROUS) | 改写私有 pstates 表中的一个 delta（危险） |
| `set-overclocked-pstates on/off` | Toggle the overclocked-pstate unlock before a delta write | 在 delta 写入前开启/关闭 overclocked-pstate 解锁 |
| `get-supported-legacy-application-freq` **NVML** | Read NVML supported application clocks | 读取 NVML 支持的 application clocks |
| `set-legacy-application-freq-lock MEM_MHZ GFX_MHZ` **NVML** | Set NVML application clocks | 设置 NVML application clocks |
| `reset-legacy-application-freq-lock` **NVML** | Reset NVML application clocks | 复位 NVML application clocks |
| `set-legacy-freq MHZ [--domain core\|mem]` | Absolute clock for legacy (Kepler) GPUs (legacy SetClocks) | 老卡（Kepler）绝对频率设置 |

#### Global clock offsets (the everyday OC knob)

```bash
nvoc-cli get-pstate-global-freq-offset --domain core --pstate P0
nvoc-cli set-pstate-global-freq-offset 150 --domain core --pstate P0        # +150 MHz
nvoc-cli set-pstate-global-freq-offset -100 --domain memory --nvml          # NVML path
nvoc-cli reset-pstate-global-freq-offset --domain core
```

`--domain` takes the public 4-value vocabulary (`core`/`memory`/`processor`/`video`); `--pstate` such as `P0` (repeatable). Units: `125MHz`-style suffixes accepted.

#### P-State locks — three tiers, by decreasing privilege

```bash
nvoc-cli get-pstate-lock --domain 0            # 0=GPC/core, 2=memory; shows Locked: P0, P3 summary
nvoc-cli set-pstate-lock P0                    # admin, highest priority
nvoc-cli reset-pstate-lock
nvoc-cli set-private-forced-pstate-lock-user 0 # SetForcePstate (0x025BFB10); set_type 0/1/2 all force
nvoc-cli reset-private-forced-pstate-lock-user # pstate=16 is the release sentinel
nvoc-cli set-private-permanent-pstate-lock-user 4   # index into the GPU's REAL pstate list (4060 laptop: 4=P0)
```

`set-private-permanent-pstate-lock-user` takes an **INDEX** into this GPU's available P-State list (see `get-pstate-lock`), not a fixed P8..P0 enum; there is **no release value** — the lock survives both resets above and only a reboot/driver reload clears it. `set-pstate-lock-via-mem-range` warns (does not reject) on overlapping range pins and falls back to a runtime pin.

#### Private pstates-2.0 family (renamed 2026-08-31)

The probe workflow pairs the unlock with the table read:

```bash
nvoc-cli get-private-legacy-pstates20-freq-domain-info     # dump delta table (GetPstates20Private, stamp 81044)
nvoc-cli set-overclocked-pstates on                        # unlock the OC pstate range first (EnableOverclockedPstates)
nvoc-cli set-private-legacy-pstates20-freq-domain-global-offset 500 --pstate 0 --domain gpc
nvoc-cli set-overclocked-pstates off                       # restore
```

- The dump shows `caps_editable` (caps bit0 = kernel "editable"), flags, table dimensions, and per-(P-State, domain) `delta_raw` rows in the 32-domain RTSS-space ids.
- **`--domain` accepts names** — `gpc`, `xbar`, `m`/`mem`, `gpc2`, … or the raw id the dump prints (RTSS space: gpc=0, xbar=1, sys=2, hub=3, m=4, host=5, disp=6, …, msd=21, pciegen=31).
- `DELTA` is a **raw table word**, not live-calibrated: the public-path marshalling stores `100*delta_kHz/domainMax` (percent of domain max) but kernel-side interpretation is unverified — on P100 writes are not retained. `--flags` ORs bits into the byte@+4 flags word (bit1 = RM apply flag).
- One-sided range rendering: `get-pstate-freq-range` prints min-only/max-only ranges correctly instead of blanking one side.
- These commands are private/experimental — structure versions vary by driver; read-only use recommended.

### Voltage family (`voltage`, 13 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `get-volt-rail-info` | Private VoltRails: rail mask + per-rail offsets + live voltages + VRM windows | 私有 VoltRails：rail 掩码 + 每 rail 偏移 + 实时电压 + VRM 窗口 |
| `set-volt-rail-limit RAIL_BIT VALUE [--offset] [--target] [--expect-type N]` | Set a volt-rail limit in mV (melonVolt write path) | 设置 volt-rail 限值（mV） |
| `get-public-gpc-rail-volt-boost` / `set-…` / `reset-…` | NVAPI voltage boost percent trio | NVAPI 电压提升百分比三件套 |
| `get-legacy-gpc-rail-overvolt-limit [--pstate]` / `set-… DELTA_UV` / `reset-…` | P-State base voltage delta in µV (alias: `set-legacy-overvolt-uv`) | P-State 基础电压 delta（µV） |
| `get-legacy-gpc-rail-volt-range [--pstate]` | Legacy core overvolt ranges (per-pstate min/current/max) | 旧式核心过压范围 |
| `set-overvolt-uv DELTA_UV` | Global over-voltage offset (PSTATES20 V2 OV array) | 全局过压偏移 |
| `get-core-voltage-control` / `set-core-voltage-control VALUE` | Raw core-voltage control object (units uncalibrated — read first) | 原始 core-voltage control（单位未标定——先读取） |
| `set-gpc-volt-lock TARGET [--feedback]` | Lock VFP by point index or voltage (`42`, `900mV`, `900000uV`) | 按点索引或电压锁定 VFP |

#### get-volt-rail-info

```bash
nvoc-cli get-volt-rail-info --gpu 0
```

Rail mask, per-rail live voltages, and per-rail P0 bounds (current/target/effective/vbios/vrm-max walls + offset ceiling). Multi-rail parts (GB10 / 50-series: core + Xbar) report each rail tagged by `rail_bit` (status matched by rail bit, per-rail P0 blocks).

#### set-volt-rail-limit

```bash
nvoc-cli set-volt-rail-limit 0 1125 --target        # absolute mV target on rail 0
nvoc-cli set-volt-rail-limit 1 56.25 --offset       # direct mV offset (5090 MSVDD = rail 1, type 3)
```

Both modes take **millivolts by default** (one decimal allowed — 10/20-series hardware step is 12.5 mV); a `uv` suffix means raw µV. `--target` derives the offset from the live control/status snapshot. `--expect-type N` guards the rail control entry type. The driver clamps the effective wall to `min(target, vbios_wall, vrm_max_wall)` regardless of mode. High-risk write — record `get-volt-rail-info` first.

#### set-gpc-volt-lock

```bash
nvoc-cli set-gpc-volt-lock 42            # VFP point index
nvoc-cli set-gpc-volt-lock 900mV         # one-decimal mV accepted (981.25mV is valid)
nvoc-cli set-gpc-volt-lock 900000uV      # raw µV
```

Reset with `reset-public-vftable-gpc-lock` (same PerfClientLimits line).

### V/F curve tables family (`vfp`, 14 commands)

| Command | Purpose                                                          | 用途                                         |
|---|------------------------------------------------------------------|----------------------------------------------|
| `get-public-vftable [--domain] [--indexed] [--infer-missing-field] [--output-csv P]` | Public V-F curve (VfPoints line); all domains by default         | 公开 V-F curve 表（默认转储全部域）          |
| `set-public-vftable-point-offset POINT DELTA [--domain] [--import-csv P]` | Set one VFP point delta in MHz, or apply a whole CSV curve       | 设置单个 VFP 点偏移，或导入整条 CSV 曲线     |
| `set-public-vftable-range-offset START END DELTA` | Set a VFP point range delta in MHz                               | 设置 VFP 点区间偏移                          |
| `reset-public-vftable-offset [--domain]` | Reset VFP deltas                                                 | 复位 VFP delta                               |
| `reset-public-vftable-gpc-lock` | Reset the VFP voltage lock                                       | 复位 VFP 电压锁                              |
| `sync-vfp-memory-pstate` | Copy the memory VFP second-pstate curve onto P0                  | 将 第二档pstate 显存的目标频率设置为和P0相同 |
| `get-private-vftable [--bank] [--domain] [--infer-missing-field] [--dump-records]` | Private ClockClient V/F-points (voltage-indexed, per-bank)       | 私有 ClockClient V/F 点                      |
| `set-private-vftable-point-offset BANK INDEX VALUE [--freq-mode] [--raw-converted] [--raw]` | Write one private V/F point (dangerous)                          | 写一个私有 V/F 点（危险）                    |
| `set-private-vftable-range-offset BANK START END VALUE [… modes]` | Batch private V/F edit, one RMW cycle (dangerous)                | 批量私有 V/F 编辑（危险）                    |
| `reset-private-vftable-offset BANK [--domain] [--mode] [--freq] [--volt] [--slot]` | Clear private V/F overrides the public resets can't reach        | 清除公开 reset 路径够不到的私有覆盖          |
| `get-private-freq-domain-info` | ClkDomains control block: controllable mask + per-domain records | ClkDomains 控制块：可控掩码 + 各域记录       |
| `get-private-freq-domain-status [DOMAIN]` | Measure domain physical clock (two-sample MEASURE_FREQ)          | 测量时钟域物理频率（两次采样）               |
| `set-private-freq-domain-global-offset DOMAIN OFFSET [--freq] [--volt] [--slot] [--temporary]` | Write a per-domain global offset (dangerous XBar write)          | 写入域级全局偏移（危险的 XBar 写入）         |
| `reset-private-freq-domain-global-offset [--domain] [--slot] [--freq] [--volt]` | Reset ClkDomains offsets to stock                                | 复位 ClkDomains 偏移到默认                   |

Public-table commands use the private NVAPI VfPoints line (`0x21537AD4` family read, VfSetControl `0x0733E009` write per the naming decisions doc). `--import-csv`/`--output-csv` exchange the curve as `voltage,frequency,delta,default_frequency`.

#### ClkDomains: three domain universes (do not mix them)

The private clock-domain tools deal with **three different bit universes**, all confirmed in source comments:

1. **WRITE/record space** (ClkDomains control records — used by `set/reset-private-freq-domain-global-offset` names and `get-private-freq-domain-info`): certified via 2026-08/09 A/B sweeps (Pascal/Turing/Ampere/Ada/Volta) plus 2026-09-06 cross-certification against FreqsEnum — **gpc=0, xbar=1, mem=2, sys=3, hub=4, msd=5, bit6 unattributed, disp=7, pciegen=8, host=9**. Pascal has no MSD domain (bit 5 SET unsupported).
2. **MEASURE space** (`get-private-freq-domain-status` argument, RTSS order): gpc=0, xbar=1, sys=2, hub=3, m=4, host=5, disp=6, …
3. **FreqsEnum selector space** (legal-frequency enumeration; same object space as the records): 0=Gpc, 1=Xbar, 2=M, 3=Sys, 4=Hub, 5=Msd, 6 unsupported, 7=Disp, 8=PcieGen, 9=Host.

For cross-generation A/B work address records by **bare integer** — name attribution is per-generation: on Ada (4060) bit1 moves SYS+XBAR, bit2 memory, bit3 pure SYS, bit5 MSD, bit9 Host; on Pascal 1080 bit 5 moves the SYS clock; bit1 is pure Xbar only on Pascal/Volta (Ampere+ couples Sys into bit1).

#### set-private-freq-domain-global-offset

```bash
nvoc-cli get-private-freq-domain-info                       # see the records + slots first
nvoc-cli set-private-freq-domain-global-offset xbar --freq -60
nvoc-cli set-private-freq-domain-global-offset gpc --volt +25.5 --temporary
nvoc-cli set-private-freq-domain-global-offset 5 --slot 0 -15000            # raw record bit + raw slot
nvoc-cli reset-private-freq-domain-global-offset                            # EVERY domain × both plane slots
nvoc-cli reset-private-freq-domain-global-offset --domain xbar --freq
```

- Positional 1: WRITE-map name (`xbar`, `gpc`/`core`, `msd`, `disp`, `host`, …) or raw bit 0–31. Positional 2: with `--freq` (default) a signed MHz offset, one decimal allowed (`-60`, `+15.5`; an explicit `khz` suffix keeps the legacy unit); with `--volt` a per-domain V/F-curve voltage addend in mV (one decimal allowed).
- **Plane slots are generation-dependent**: `--freq`/`--volt` resolve to slot 0/1 on 10–40 series and slot 2/3 on Blackwell 50 series. `--slot` writes the RAW dword (0–7) and is never remapped; slots 2–7 are driver-opaque on 10–40 series.
- Single-rail arbitration model (user-verified): final voltage at a frequency = MAX over domains of (built-in curve + domain volt offset). Note a GPC volt shift makes `get-public-vftable` return near-all-zero garbage (the public table can't represent shifted curves) while `get-private-vftable` is unaffected.
- The medium layer snapshots the control block, version-gates, SETs, readbacks, and restores on mismatch. `--temporary` additionally restores the snapshot before returning (the reversible-experiment recipe from the XBar safety notes referenced in the code).
- Readback caveat: a global offset does **not** project into the per-point V/F control readback (`get-private-vftable`'s `offset:` field) — it surfaces only in `get-private-freq-domain-info` slot 0 and as `freq_current` shifting away from `freq_default`.
- Write-record unit JSON: the info dump tags each record slot with its unit per generation (slot unit JSON), and domains the driver refuses during reset are reported as warnings while the reset continues.
- **Dangerous** — read-only probe first, one domain at a time.

#### get-private-vftable

```bash
nvoc-cli get-private-vftable                            # bank 0, all segments, human table
nvoc-cli get-private-vftable --domain gpc               # gpc|xbar|msd|disp|mem (sys/host alias msd; bare 0-4 = hint ordinal)
nvoc-cli get-private-vftable --bank 1                   # pstate-class records
nvoc-cli get-private-vftable --dump-records             # + raw-record slot map (layout discovery)
nvoc-cli get-private-vftable --infer-missing-field      # PASCAL ONLY field inference
```

Human output is an aligned bare-number table (voltages mV, frequencies MHz; blank cell = not reported). Per point: `voltage_uV` axis, `volt_current_mv` — the **current voltage read from V/F point record slot +0x68** (Ada-verified; legacy/Blackwell report 0 → blank), `domain_currents` (extended-section per-domain `[freq, volt]` slots, Turing-verified), `volt_offset_mv` (**Blackwell** records only — their +0x64 slot is a signed µV term), `freq_default_mhz` / `freq_current_mhz`, and the raw control-override readback (`mode`/`offset`, GetControl `0xDA025C3E`).

`--dump-records` attaches per-offset dword statistics (value range, distinct count, i32 range, cross-slot correlations) plus a hex dump of the first record — records are 488 B (modern) or 76 B (Volta-legacy). This is the tool that identified Blackwell's +0x64 signed µV voltage slot (a −45 mV experiment read back as 2³²−45000).

Private reads are faithful by default; `--infer-missing-field` is **Pascal-only** (voltage borrowed from the public grid, default/current semantic remap). Turing's private table is fully populated. Private/experimental — read-only use recommended.

#### reset-private-vftable-offset

```bash
nvoc-cli reset-private-vftable-offset 0                          # bank 0, both planes, all domains
nvoc-cli reset-private-vftable-offset 0 --domain xbar            # one segment group (gpc|xbar|msd|disp|mem)
nvoc-cli reset-private-vftable-offset 0 --volt                   # only the voltage plane
nvoc-cli reset-private-vftable-offset 0 --mode freq --slot 0     # equivalent plane selectors
nvoc-cli reset-private-vftable-offset 1                          # pstate-class records bank
```

`--freq`/`--volt` (or `--mode freq|raw`, `--slot 0/1`) pick one plane: freq = mode-0 kHz offsets, volt = mode-1 raw values. Default clears both. Plane-selector aliases were added to this command and the ClkDomains reset (2026-09-01).

### Performance policies family (`perf`, 9 commands)

| Command | Purpose | 用途 |
|---|---|---|
| `get-autoboost-status` / `set-autoboost-status on/off` / `reset-autoboost-status on/off` **NVML** | NVML auto-boost state (reset = set default) | NVML 自动加速状态 |
| `get-autoboost-support API` / `set-autoboost-support API STATE` **NVML** | NVML API restriction (`app-clocks`\|`auto-boost` × `open`\|`restricted`) | NVML API 限制状态 |
| `get-pmgr-arbiter` | PMGR voltage-request arbiter values (raw NVAPI status on supported:no) | PMGR 电压仲裁值 |
| `set-pmgr-arbiter CSV` | Set 11 dwords (admin; GET-patch-SET RMW recommended) | 写入 11 个 dword（需管理员） |
| `set-perf-freq-caps MAX_MHZ [--min MIN_MHZ]` | GPU frequency perf-cap (PerfLimitsSetStatus) | GPU 频率 perf-cap |
| `reset-perf-freq-caps` | Clear the perf-cap | 清除 perf-cap |

```bash
nvoc-cli set-perf-freq-caps 2100 --min 300      # clamped by the driver
nvoc-cli reset-perf-freq-caps
```

### OC scanner family (`scanner`, 1 command)

| Command | Purpose | 用途 |
|---|---|---|
| `oem-oc-scanner [--start] [--stop] [--revert] [--status] [--background-on/off] [--incomplete]` | Driver-side (OEM) OC Scanner control, drivers ≥ 455.00 | 驱动侧 OEM OC Scanner 控制 |

```bash
nvoc-cli oem-oc-scanner --start      # driver scans in background and applies V/F offsets itself
nvoc-cli oem-oc-scanner --status
nvoc-cli oem-oc-scanner --revert     # restore the pre-scan curve
```

No console progress output. `--incomplete` queries partial results of an INCOMPLETE run.

### Reset inventory (recovery cheat sheet)

| Reset command | Clears |
|---|---|
| `reset-pstate-global-freq-offset [--domain]` | P-State global freq offsets |
| `reset-freq-lock [--domain]` | Locked clocks (NVAPI/NVML) |
| `reset-pstate-lock` / `reset-private-forced-pstate-lock-user` | Native / forced P-State locks (NOT the permanent lock — reboot clears that) |
| `reset-public-vftable-offset` / `reset-public-vftable-gpc-lock` | Public V/F deltas / voltage lock |
| `reset-private-vftable-offset BANK […]` | Private V/F overrides (freq+volt planes) |
| `reset-private-freq-domain-global-offset […]` | ClkDomains global offsets (every domain × both slots by default) |
| `reset-power-limit` / `reset-public-tgp-percent` | TGP watt / percent |
| `reset-temp-limit` / `reset-temp-sim` | Sensor limits / temperature simulation |
| `reset-fan-speed` / `reset-fan-curve` | Fan override (control-block bit0) / curve slot |
| `reset-legacy-application-freq-lock` | NVML application clocks |
| `reset-legacy-gpc-rail-overvolt-limit` / `reset-public-gpc-rail-volt-boost` | Base-voltage delta / boost percent |
| `reset-autoboost-status on/off` | NVML auto-boost default |
| `reset-perf-freq-caps` | Frequency perf-caps |

### Renamed commands (2026-08 normalization — old scripts)

Normalization per `cli/RENAME_DECISIONS.md` (offsets not deltas, `temp-limit` not `thermal-limit`, symmetric get/set/reset stems). Notable renames: `get-vfp`→`get-public-vftable`, `set-core-offset-mhz`/`set-memory-offset-mhz`→`set-pstate-global-freq-offset --domain`, `set-locked-clocks-mhz`→`set-freq-lock`, `get/set-tgp-watt`→`get/set-power-limit`, `set-dynamic-boost`→`set-ppab-status`, `get/set-pstate-native`→`get/set/reset-pstate-lock`, `get-clk-domains`→`get-private-freq-domain-info`, `get-clk-vf-points`→`get-private-vftable`, `thermal-sim`→`temp-sim`, `get-tdp-temp-limits`→`get-public-power-limit`+`get-public-temp-limit`. One alias remains: `set-legacy-overvolt-uv` → `set-legacy-gpc-rail-overvolt-limit`. The `--policy` fan selector and `--domain` vocabulary above are current as of 2026-09.

> Note: some `cli/README.md` usage examples still show pre-rename names (`get-power-watt`, `set-core-offset-mhz`, `set-locked-clocks-mhz`, `get-vfp`) — those are stale; the names in this page are verified against `cli/src/lib.rs`.

<a id="chinese"></a>

## 中文

`nvoc-cli` 的逐命令参考（全局参数模型、后端与权限见 [[CLI-Guide]]）。共 101 条 GPU 命令外加 `list` 元命令，按 CLI 自身的 9 个族组织（`nvoc-cli list <group>`）。用途表为双语（Purpose / 用途）。

约定：`[arg]` = 必选位置参数，`<x>` = 选项取值，`…` = 可重复。所有命令都接受全局选项（`--gpu/-g`、`--nvapi|--nvml`、`--output/-O`、`--no-color`）与 `--help`。标注 **NVML** 或 **NVAPI** 的为后端限定命令；双后端命令默认 auto（先 NVAPI，失败回退 NVML）。"私有" = 未公开文档的 NVAPI 面——结构随驱动变化，建议只读使用。

> ⚠️ 写入属于高风险：先读取、记录原始值、回读验证，并明确 reset 路径（[[Safety-and-Recovery]]）。

### Info 族（`info`，14 条命令）

| 命令 | 用途 |
|---|---|
| `list [GROUP]` | 按族列出全部命令（元命令，不访问 GPU） |
| `get-gpu-list` | 列出发现的 GPU 与可用后端（NVML/NVAPI） |
| `get-uuid` | 读取 GPU UUID |
| `get-info` | 读取 NVAPI GPU 身份/能力信息 + NVAPI 接口版本 + PCIe 代际 + BAR 信息 |
| `get-status [--verbose]` | 读取 NVAPI 实时 GPU 状态（频率、温度、功率、占用率） |
| `get-settings` | 读取 NVAPI 超频设置（各 P-State 偏移） |
| `get-throttle-reasons` **NVML** | 读取 NVML 降速（性能限制）原因 |
| `get-vbios [--out F] [--dump] [--maxwell-vftable-decode]` | 读取 VBIOS 镜像 + BIT 摘要（仅 Windows） |
| `get-display-list [--all]` | 列出 NVAPI 显示器 ID（供 EDID 操作） |
| `get-edid DISPLAY_ID` | 经 NVAPI 读取显示器 EDID |
| `set-edid DISPLAY_ID EDID_HEX` | 经 NVAPI 写入显示器 EDID |
| `clear-edid DISPLAY_ID` | 经 NVAPI 清除显示器 EDID |
| `set-ecc-configuration ENABLED [--immediate]` | 设置 NVAPI ECC 内存配置（持久化到 NVRAM） |
| `restart-display-driver` | 重启显示驱动（旧式"应用超频"触发器） |

#### get-info

```bash
nvoc-cli get-info --gpu 0
```

human 输出把 NVAPI 接口版本（如 `R580`）提升为横幅行——它标记驱动导出的 API 代际，决定驱动接受哪些私有/stamp 门控族。JSON 额外提供 `nvapi_interface_version`、`max_pcie_link_gen`（来自 NVML）与 `bar_info`（GetBarInfo，尽力而为）。

#### get-status

```bash
nvoc-cli -O json get-status --gpu 0
nvoc-cli get-status --verbose        # 额外：VFP 表、原始 power_monitor、D-Notifier D1–D5 表
```

包含 `all_clocks_detailed` 分节（GetAllClocks V2）：各域有效频率，非零时附带驱动的 `ratio_domain`/`ratio` 父域关系与 `reserved` dword。同时补充 NVML 实时数据：`power_draw_w`、`power_limit_w`，以及移动端 `d_notifier` 级别（私有 ClientPowerPoliciesGetInfo——它会悄悄钳制 TGP 墙）。

#### get-vbios

```bash
nvoc-cli get-vbios --gpu 0                      # 摘要：version/size/BIT
nvoc-cli get-vbios --out vbios.rom              # 写出原始镜像
nvoc-cli get-vbios --dump                       # 完整 BIT token 表 + Fermi 模型原始块
nvoc-cli get-vbios --maxwell-vftable-decode     # GPU Boost 2.0 V/F 阶梯转储
```

仅 Windows（`NvAPI_GPU_GetVbiosImage`，走内核 escape）。尽力而为的伴随读取：VBIOS **security word**（QueryVbiosSecurityInfo）与 **status string**（QueryVbiosStatusString）。

- 内置 BIT 族解析器定位 ASCII `BIT` 签名（经典的 0x48 指针在部分镜像上不成立），锚定扫描 6 字节 token 表，并自动探测数据基址（Fermi 时代镜像为 BIT 相对，Pascal 为镜像相对——Pascal 的 `'P'` token 是 u32 指针目录）。
- `--dump` 将 perf 表（`'P'`）与电压块（`'M'`/`'V'`/`'U'`）按原始字节呈现（未解码的布局：Fermi = 离散 pstate × clock × VID，无 V/F curve）。
- `--maxwell-vftable-decode` 解码 **Maxwell GPU Boost 2.0 阶梯**（BIT `'P'`+0x34、v0x10——逆向自 MaxwellBiosTweaker；适用于 GM10x/GM20x）：79 个 GPC 点，每点 u16 半 MHz 频率 + vmap 索引；电压（min/max mV）经 vmap 表解析；以 `Id | V_min | V_max | Freq` 表输出并附 P-State 边界标记。其他代际报错"no boost-ladder table"。

#### set-ecc-configuration

```bash
nvoc-cli set-ecc-configuration on                # 默认推迟到下次重启
nvoc-cli set-ecc-configuration on --immediate    # 立即生效（是否支持取决于硬件）
```

`NvAPI_GPU_SetECCConfiguration`——写入非易失存储；SET 后经 GetECCConfigurationInfo 回读。GeForce 消费卡通常返回 `NotSupported`。先查看 `get-status` 的 `Ecc` 分节。

### Power 族（`power`，14 条命令）

| 命令 | 用途 |
|---|---|
| `get-public-power-limit` | 功率限制范围——NVML 瓦特（min/current/max）或 NVAPI TDP 百分比 + 移动端 tgp_range |
| `set-power-limit WATT [--policy-index N]` | 按瓦特设置 TGP（NVAPI 移动端滑条 / NVML 功率管理限制） |
| `reset-power-limit [--policy-index N]` | 将 NVAPI TGP 复位到额定/默认（移动端） |
| `get-power-ceiling` | PPAB 移动端的有效功率墙：请求 TGP + D-Notifier 上限，ceiling = 二者取小 |
| `set-public-tgp-percent PCT` | 按百分比设置 NVAPI 功率限制（ClientPowerPolicies） |
| `reset-public-tgp-percent` | 复位 NVAPI 功率限制 |
| `get-rated-tdp` | 额定 TDP 三路回读 |
| `get-dnotifier` / `set-dnotifier LEVEL` | D-Notifier（D0-notify）级别 + D1–D5 功率上限表（移动端） |
| `set-ppab-status on/off` | NVAPI PPAB / Dynamic-Boost 开关 |
| `get-power-mode` / `set-power-mode max\|balanced` | NVIDIA App 功率模式（含支持性门控） |
| `set-whispermode2-status on/off [--mode M]` | Whisper Mode 2.0 + 声学模式（移动端） |
| `set-batteryboost2-status on/off` | Battery Boost 2.0 开关（移动端） |

#### get-public-power-limit（双后端——吸收原 get-power-limit）

```bash
nvoc-cli get-public-power-limit              # auto → NVML：min/current/max 瓦特
nvoc-cli get-public-power-limit --nvapi      # TDP 百分比范围 + tgp_range（如驱动暴露）
```

NVML 分支即 nvidia-smi `-pl` 领域。NVAPI 分支读取 ClientPowerPolicies 的 TDP 百分比范围，并在私有 ClientTgpWatt 族应答时附加移动端 `tgp_range`（瓦特，按 `--policy-index`）；缺失条目为 `null`。旧的 `get-power-limit` / `get-tdp-temp-limits` 名称已移除——由本命令（加 `get-public-temp-limit`）吸收。

#### get-power-ceiling

```bash
nvoc-cli get-power-ceiling
```

**有效 PPAB 墙**：ceiling = min(请求 TGP, 活动中的 D-Notifier 上限)。human 输出以瓦特显示 Ceiling / Requested TGP / D-Notifier Cap / Default TGP；不支持的 GPU 返回 `{"supported": false}`。

#### set-power-limit

```bash
nvoc-cli set-power-limit 140              # auto：NVAPI 优先
nvoc-cli set-power-limit 140W --nvml      # NVML 功率管理限制
```

瓦特位置参数接受 `W` 后缀。驱动会钳制到有效范围——先记录 `get-public-power-limit` 输出。

### Thermal 族（`thermal`，9 条命令）

| 命令 | 用途 |
|---|---|
| `get-temp-thresholds` | 温度阈值——默认 NVML（Shutdown/Slowdown/…）；`--nvapi` 暴露 target-temp 策略 |
| `get-public-temp-limit` | NVAPI 公开温度限制范围（min/default/max °C + throttle curve）；缺失项为 `null` |
| `set-temp-limit C [--domain gpu\|acoustic]` | 设置温度墙——NVAPI 传感器限制；NVML 为 GPU 最高温阈值，`--domain acoustic` 为声学目标 |
| `reset-temp-limit` | 复位 NVAPI 传感器限制 |
| `set-private-target-temp-limit C [--policy-index N]` | NVAPI target-temp 策略槽（°C，移动端） |
| `get-legacy-temp-sensor` | NVAPI 旧式三传感器视图（GPU/Memory/Board，实时 + 物理范围） |
| `get-temp-sim` / `set-temp-sim C` / `reset-temp-sim` | 温度仿真状态（受 Secured-Overrides 门控） |

```bash
nvoc-cli get-temp-thresholds
nvoc-cli set-temp-limit 83
nvoc-cli set-temp-limit 78 --domain acoustic --nvml   # Linux 通道；Windows 拒绝 NVML 阈值写入
```

Windows 上 NVML 阈值写入被拒绝——此时使用 `set-private-target-temp-limit`。`set-temp-sim` 伪造驱动可见温度：**危险的研究工具**（仅用于风扇实验，用 `reset-temp-sim` 恢复）。

### Fan 族（`fan`，8 条命令）

| 命令 | 用途 |
|---|---|
| `get-fan-info` | 风扇/cooler 信息——NVML 数量与 min/max%；NVAPI 私有 per-cooler 信息 |
| `set-fan-speed VALUE [--percent] [--rpm] [--fan F] [--policy P] [--cooler N]` | 设置风扇转速：百分比（NVAPI SetCoolerLevels / NVML）或 RPM（仅 NVAPI 私有路径） |
| `reset-fan-speed [--rpm] [--fan F] [--cooler N]` | 恢复风扇控制——NVAPI 清除 cooler level 覆盖 / NVML 恢复默认 |
| `get-fan-curve` / `set-fan-curve` / `reset-fan-curve` | NVAPI 风扇曲线表（ClientFanPolicies，仅桌面卡） |
| `set-fanstop-status on/off [--curve N]` | 每个曲线槽的停转/零转开关（FanArbiterSet） |
| `get-legacy-fan-policy` | 风扇策略能力（新驱动为 V2 原始块；R391 时代解码 V1） |

#### set-fan-speed / reset-fan-speed

```bash
nvoc-cli set-fan-speed 60 --fan all --policy manual             # 百分比（默认）
nvoc-cli set-fan-speed 1800 --rpm --nvapi                       # 物理 RPM（私有 FanCoolerSetControl）
nvoc-cli reset-fan-speed                                        # 解除风扇钉死
nvoc-cli reset-fan-speed --rpm --cooler 0                       # 关闭风扇转速仿真 + 清除使能位
```

- `--policy` 的有效取值已在帮助中枚举（默认 `manual`）：**NVML** `manual`、`auto`（= `continuous`，按温度自动）。**NVAPI**：另加 `perf`、`discrete`、`hybrid`、`software`（silent-ODT）、`default`、`default32`。
- 实机 A/B（GUI/TUI 工作）：**现代卡上只有 `continuous`（TemperatureContinuous）能经 NVAPI 真正施加手动百分比**；旧下拉列表的其余项要么无效要么被拒。老卡（≤ Kepler，如 GT730/Fermi R391）落在 `manual`。
- `reset-fan-speed` 默认路径清除 NVAPI cooler level 覆盖（控制块 bit0）——**这是唯一能解开现代卡钉死状态的 reset**。指定 `--fan` 需要 `--nvml`（NVAPI 重置全部 cooler）。
- RPM 会钳制到 `get-fan-info --nvapi` 报告的 cooler `[min, max]`。

#### 风扇曲线

```bash
nvoc-cli get-fan-curve
nvoc-cli set-fan-curve 0 40:800,60:1200,75:1800   # 槽位 idx + 3 个严格单调 temp:rpm 点
nvoc-cli reset-fan-curve --curve 0                # 恢复出厂槽位（NVAPI reset 报 NOT_SUPPORTED 时可用）
nvoc-cli set-fanstop-status on --curve 0
```

温度与 RPM 都必须严格递增。`reset-fan-curve` 走私有 FanPolicy 路径（0x2B2A2A45）。

### Clock 族（`clock`，20 条命令）——偏移、锁与 P-State

| 命令 | 用途 |
|---|---|
| `get-pstate-global-freq-offset [--domain] [--pstate]` | 读取 MHz 频率偏移（NVAPI/NVML） |
| `set-pstate-global-freq-offset OFFSET [--domain] [--pstate]` | 设置任意时钟域的 MHz 频率偏移 |
| `reset-pstate-global-freq-offset [--domain]` | 复位全部已触达 (pstate, domain) 对，或按 `--domain` 过滤 |
| `get-pstate-freq-range` **NVML** | 读取 NVML P-State 频率范围 |
| `set-freq-lock MIN MAX [--domain]` | 将核心/显存频率锁定到 MHz 区间（NVAPI VFP 锁 / NVML 硬性下限+上限） |
| `reset-freq-lock [--domain]` | 复位频率锁 |
| `get-pstate-lock [--domain N]` | 读取原生 NVAPI P-State 级别表 |
| `set-pstate-lock PSTATE` | 锁定原生 NVAPI P-State（需管理员） |
| `reset-pstate-lock` | 清除全部原生 P-State 锁 |
| `set-pstate-lock-via-mem-range FIRST [SECOND]` | 经显存频率范围锁定单个或连续 P-State 区间（NVAPI/NVML） |
| `set-private-forced-pstate-lock-user PSTATE [--set-type T]` | 强制锁定 P-State（私有 SetForcePstate） |
| `reset-private-forced-pstate-lock-user` | 释放强制锁（SetForcePstate 哨兵值 16） |
| `set-private-permanent-pstate-lock-user LEVEL` | 免管理员永久 P-State 锁（SetPerfLevel；按索引；重启清除） |
| `get-private-legacy-pstates20-freq-domain-info` | 私有 pstates-2.0 delta 表只读转储 |
| `set-private-legacy-pstates20-freq-domain-global-offset DELTA [--pstate] [--domain] [--flags]` | 改写私有 pstates 表中的一个 delta（危险） |
| `set-overclocked-pstates on/off` | 在 delta 写入前开启/关闭 overclocked-pstate 解锁 |
| `get-supported-legacy-application-freq` **NVML** | 读取 NVML 支持的 application clocks |
| `set-legacy-application-freq-lock MEM_MHZ GFX_MHZ` **NVML** | 设置 NVML application clocks |
| `reset-legacy-application-freq-lock` **NVML** | 复位 NVML application clocks |
| `set-legacy-freq MHZ [--domain core\|mem]` | 老卡（Kepler）绝对频率设置 |

#### 全局时钟偏移（日常超频旋钮）

```bash
nvoc-cli get-pstate-global-freq-offset --domain core --pstate P0
nvoc-cli set-pstate-global-freq-offset 150 --domain core --pstate P0        # +150 MHz
nvoc-cli set-pstate-global-freq-offset -100 --domain memory --nvml          # NVML 路径
nvoc-cli reset-pstate-global-freq-offset --domain core
```

`--domain` 使用公开的 4 值词汇（`core`/`memory`/`processor`/`video`）；`--pstate` 如 `P0`（可重复）。单位接受 `125MHz` 风格后缀。

#### P-State 锁——按权限递减的三级

```bash
nvoc-cli get-pstate-lock --domain 0            # 0=GPC/core，2=memory；汇总显示 Locked: P0, P3
nvoc-cli set-pstate-lock P0                    # 需管理员，最高优先级
nvoc-cli reset-pstate-lock
nvoc-cli set-private-forced-pstate-lock-user 0 # SetForcePstate（0x025BFB10）；set_type 0/1/2 均为强制
nvoc-cli reset-private-forced-pstate-lock-user # pstate=16 是释放哨兵
nvoc-cli set-private-permanent-pstate-lock-user 4   # 指向该 GPU 真实 pstate 列表的索引（4060 笔记本：4=P0）
```

`set-private-permanent-pstate-lock-user` 接收**指向该 GPU 可用 P-State 列表的索引**（见 `get-pstate-lock`），不是固定的 P8..P0 枚举；**没有释放值**——该锁能熬过上面两个 reset，只有重启/重载驱动才能清除。`set-pstate-lock-via-mem-range` 对重叠的范围锁定采取警告而非拒绝，并回退到运行期钉住。

#### 私有 pstates-2.0 族（2026-08-31 重命名）

探测工作流将解锁与表读取配对：

```bash
nvoc-cli get-private-legacy-pstates20-freq-domain-info     # delta 表转储（GetPstates20Private，stamp 81044）
nvoc-cli set-overclocked-pstates on                        # 先解锁 OC pstate 范围（EnableOverclockedPstates）
nvoc-cli set-private-legacy-pstates20-freq-domain-global-offset 500 --pstate 0 --domain gpc
nvoc-cli set-overclocked-pstates off                       # 恢复
```

- 转储显示 `caps_editable`（caps bit0 = 内核"可编辑"）、flags、表维度，以及按 32 域 RTSS 空间 id 的每 (P-State, domain) `delta_raw` 行。
- **`--domain` 接受名称**——`gpc`、`xbar`、`m`/`mem`、`gpc2` 等，或转储打印的原始 id（RTSS 空间：gpc=0、xbar=1、sys=2、hub=3、m=4、host=5、disp=6、…、msd=21、pciegen=31）。
- `DELTA` 是**原始表字**，未经实时标定：公开路径的编组存储 `100*delta_kHz/domainMax`（域最大值的百分比），但内核侧解释未验证——P100 上写入不保留。`--flags` 将位 OR 进 byte@+4 的 flags 字（bit1 = RM apply 旗标）。
- 单侧范围渲染：`get-pstate-freq-range` 能正确打印只有 min 或只有 max 的范围，不再留空白。
- 这些命令属于私有/实验性质——结构随驱动变化，建议只读使用。

### Voltage 族（`voltage`，13 条命令）

| 命令 | 用途 |
|---|---|
| `get-volt-rail-info` | 私有 VoltRails：rail 掩码 + 每 rail 偏移 + 实时电压 + VRM 窗口 |
| `set-volt-rail-limit RAIL_BIT VALUE [--offset] [--target] [--expect-type N]` | 设置 volt-rail 限值（mV，melonVolt 写路径） |
| `get-public-gpc-rail-volt-boost` / `set-…` / `reset-…` | NVAPI 电压提升百分比三件套 |
| `get-legacy-gpc-rail-overvolt-limit [--pstate]` / `set-… DELTA_UV` / `reset-…` | P-State 基础电压 delta（µV；别名 `set-legacy-overvolt-uv`） |
| `get-legacy-gpc-rail-volt-range [--pstate]` | 旧式核心过压范围（每 pstate min/current/max） |
| `set-overvolt-uv DELTA_UV` | 全局过压偏移（PSTATES20 V2 OV 数组） |
| `get-core-voltage-control` / `set-core-voltage-control VALUE` | 原始 core-voltage control（单位未标定——先读取） |
| `set-gpc-volt-lock TARGET [--feedback]` | 按点索引或电压锁定 VFP（`42`、`900mV`、`900000uV`） |

#### get-volt-rail-info

```bash
nvoc-cli get-volt-rail-info --gpu 0
```

rail 掩码、每 rail 实时电压，以及每 rail 的 P0 边界（current/target/effective/vbios/vrm-max 墙 + 偏移余量）。多 rail 部件（GB10 / 50 系：核心 + Xbar）按 `rail_bit` 逐 rail 上报（状态按 rail bit 匹配，逐 rail P0 块）。

#### set-volt-rail-limit

```bash
nvoc-cli set-volt-rail-limit 0 1125 --target        # rail 0 的绝对 mV 目标
nvoc-cli set-volt-rail-limit 1 56.25 --offset       # 直接 mV 偏移（5090 MSVDD = rail 1，type 3）
```

两种模式**默认均以毫伏为单位**（允许一位小数——10/20 系硬件步进为 12.5 mV）；`uv` 后缀表示原始 µV。`--target` 从实时控制/状态快照推导偏移。`--expect-type N` 对 rail 控制条目类型加防护。无论哪种模式，驱动都会把有效墙钳制到 `min(target, vbios_wall, vrm_max_wall)`。高危写入——先记录 `get-volt-rail-info`。

#### set-gpc-volt-lock

```bash
nvoc-cli set-gpc-volt-lock 42            # VFP 点索引
nvoc-cli set-gpc-volt-lock 900mV         # 接受一位小数的 mV（981.25mV 合法）
nvoc-cli set-gpc-volt-lock 900000uV      # 原始 µV
```

用 `reset-public-vftable-gpc-lock` 复位（同属 PerfClientLimits 线）。

### V/F curve 表族（`vfp`，14 条命令）

| 命令 | 用途 |
|---|---|
| `get-public-vftable [--domain] [--indexed] [--infer-missing-field] [--output-csv P]` | 公开 V-F curve 表（VfPoints 线；默认转储全部域） |
| `set-public-vftable-point-offset POINT DELTA [--domain] [--import-csv P]` | 设置单个 VFP 点偏移（MHz），或导入整条 CSV 曲线 |
| `set-public-vftable-range-offset START END DELTA` | 设置 VFP 点区间偏移（MHz） |
| `reset-public-vftable-offset [--domain]` | 复位 VFP delta |
| `reset-public-vftable-gpc-lock` | 复位 VFP 电压锁 |
| `sync-vfp-memory-pstate` | 将显存 VFP 二段曲线复制到 P0 |
| `get-private-vftable [--bank] [--domain] [--infer-missing-field] [--dump-records]` | 私有 ClockClient V/F 点（按电压索引，分 bank） |
| `set-private-vftable-point-offset BANK INDEX VALUE [--freq-mode] [--raw-converted] [--raw]` | 写一个私有 V/F 点（危险） |
| `set-private-vftable-range-offset BANK START END VALUE [… 模式]` | 批量私有 V/F 编辑，单次 RMW（危险） |
| `reset-private-vftable-offset BANK [--domain] [--mode] [--freq] [--volt] [--slot]` | 清除公开 reset 路径够不到的私有覆盖 |
| `get-private-freq-domain-info` | ClkDomains 控制块：可控掩码 + 各域记录 |
| `get-private-freq-domain-status [DOMAIN]` | 测量时钟域物理频率（两次采样 MEASURE_FREQ） |
| `set-private-freq-domain-global-offset DOMAIN OFFSET [--freq] [--volt] [--slot] [--temporary]` | 写入域级全局偏移（危险的 XBar 写入） |
| `reset-private-freq-domain-global-offset [--domain] [--slot] [--freq] [--volt]` | 复位 ClkDomains 偏移到默认 |

公开表命令走私有 NVAPI VfPoints 线（读 `0x21537AD4` 族，写 VfSetControl `0x0733E009`，见命名决策文档）。`--import-csv`/`--output-csv` 以 `voltage,frequency,delta,default_frequency` 交换曲线。

#### ClkDomains：三个域宇宙（切勿混用）

私有时钟域工具涉及**三个不同的位宇宙**，均已在源码注释中确认：

1. **WRITE/记录空间**（ClkDomains 控制记录——`set/reset-private-freq-domain-global-offset` 的名称与 `get-private-freq-domain-info` 使用）：经 2026-08/09 的 A/B 扫描（Pascal/Turing/Ampere/Ada/Volta）与 2026-09-06 对 FreqsEnum 的交叉认证——**gpc=0、xbar=1、mem=2、sys=3、hub=4、msd=5、bit6 未归因、disp=7、pciegen=8、host=9**。Pascal 没有 MSD 域（bit 5 不支持 SET）。
2. **MEASURE 空间**（`get-private-freq-domain-status` 参数，RTSS 顺序）：gpc=0、xbar=1、sys=2、hub=3、m=4、host=5、disp=6、…
3. **FreqsEnum 选择器空间**（合法频率枚举；与记录同一对象空间）：0=Gpc、1=Xbar、2=M、3=Sys、4=Hub、5=Msd、6 不支持、7=Disp、8=PcieGen、9=Host。

跨代际 A/B 时请用**裸整数**寻址记录——名称归因随代际变化：Ada（4060）上 bit1 移动 SYS+XBAR、bit2 显存、bit3 纯 SYS、bit5 MSD、bit9 Host；Pascal 1080 上 bit 5 移动 SYS 时钟；bit1 仅在 Pascal/Volta 上是纯 Xbar（Ampere 起把 Sys 耦合进 bit1）。

#### set-private-freq-domain-global-offset

```bash
nvoc-cli get-private-freq-domain-info                       # 先看记录 + 槽位
nvoc-cli set-private-freq-domain-global-offset xbar --freq -60
nvoc-cli set-private-freq-domain-global-offset gpc --volt +25.5 --temporary
nvoc-cli set-private-freq-domain-global-offset 5 --slot 0 -15000            # 裸记录位 + 裸槽位
nvoc-cli reset-private-freq-domain-global-offset                            # 全部域 × 两个平面槽
nvoc-cli reset-private-freq-domain-global-offset --domain xbar --freq
```

- 位置参数 1：WRITE 映射名称（`xbar`、`gpc`/`core`、`msd`、`disp`、`host` 等）或裸位 0–31。位置参数 2：`--freq`（默认）为带符号 MHz 偏移，允许一位小数（`-60`、`+15.5`；显式 `khz` 后缀保留旧单位）；`--volt` 为该域 V/F curve 的电压加数，单位 mV（允许一位小数）。
- **平面槽位随代际变化**：`--freq`/`--volt` 在 10–40 系解析到槽 0/1，在 Blackwell 50 系解析到槽 2/3。`--slot` 写原始 dword（0–7），永不重映射；10–40 系上槽 2–7 对驱动不透明。
- 单 rail 仲裁模型（用户验证）：某频率下的最终电压 = 各域（内建曲线 + 域电压偏移）的最大值。注意 GPC 电压偏移会让 `get-public-vftable` 返回近乎全零的垃圾（公开表无法表达平移后的曲线），而 `get-private-vftable` 不受影响。
- 中间层会快照控制块、做版本门控、SET、回读，不匹配则恢复。`--temporary` 额外在返回前恢复快照（代码引用的 XBar 安全笔记中的可逆实验配方）。
- 回读注意事项：全局偏移**不会**投影到逐点 V/F 控制回读（`get-private-vftable` 的 `offset:` 字段）——它只出现在 `get-private-freq-domain-info` 的槽 0，以及 `freq_current` 相对 `freq_default` 的移动上。
- 写记录的单位 JSON：info 转储按代际给每个记录槽标注单位（slot unit JSON）；reset 时驱动拒绝的域会以警告形式报告并继续。
- **危险**——先只读探测，一次只动一个域。

#### get-private-vftable

```bash
nvoc-cli get-private-vftable                            # bank 0，全部分段，human 表
nvoc-cli get-private-vftable --domain gpc               # gpc|xbar|msd|disp|mem（sys/host 别名 msd；裸 0-4 = hint 序数）
nvoc-cli get-private-vftable --bank 1                   # pstate 类记录
nvoc-cli get-private-vftable --dump-records             # 附加原始记录槽位图（布局发现）
nvoc-cli get-private-vftable --infer-missing-field      # 仅 PASCAL 的字段推断
```

human 输出为对齐的裸数字表（电压 mV、频率 MHz；空白格 = 未上报）。每个点包含：`voltage_uV` 轴、`volt_current_mv`——**从 V/F 点记录槽 +0x68 读到的当前电压**（Ada 验证；legacy/Blackwell 报 0 → 空白）、`domain_currents`（扩展分节的每域 `[freq, volt]` 槽，Turing 验证）、`volt_offset_mv`（**仅 Blackwell** 记录——其 +0x64 槽为带符号 µV 项）、`freq_default_mhz` / `freq_current_mhz`，以及原始控制覆盖回读（`mode`/`offset`，GetControl `0xDA025C3E`）。

`--dump-records` 附加逐偏移 dword 统计（值范围、去重计数、i32 范围、跨槽相关性）与首记录十六进制转储——记录为 488 B（现代）或 76 B（Volta legacy）。Blackwell +0x64 带符号 µV 电压槽正是靠它定位的（−45 mV 实验回读为 2³²−45000）。

私有读取默认忠实呈现；`--infer-missing-field` **仅限 Pascal**（电压借自公开网格 + default/current 语义重映射）。Turing 的私有表是完整填充的。私有/实验性质——建议只读使用。

#### reset-private-vftable-offset

```bash
nvoc-cli reset-private-vftable-offset 0                          # bank 0，两个平面，全部域
nvoc-cli reset-private-vftable-offset 0 --domain xbar            # 单一分段组（gpc|xbar|msd|disp|mem）
nvoc-cli reset-private-vftable-offset 0 --volt                   # 仅电压平面
nvoc-cli reset-private-vftable-offset 0 --mode freq --slot 0     # 等价的平面选择器
nvoc-cli reset-private-vftable-offset 1                          # pstate 类记录 bank
```

`--freq`/`--volt`（或 `--mode freq|raw`、`--slot 0/1`）选择一个平面：freq = mode-0 kHz 偏移，volt = mode-1 原始值。默认两者都清。平面选择器别名于 2026-09-01 加入本命令与 ClkDomains reset。

### Performance policies 族（`perf`，9 条命令）

| 命令 | 用途 |
|---|---|
| `get-autoboost-status` / `set-autoboost-status on/off` / `reset-autoboost-status on/off` **NVML** | NVML 自动加速状态（reset = 设为默认） |
| `get-autoboost-support API` / `set-autoboost-support API STATE` **NVML** | NVML API 限制（`app-clocks`\|`auto-boost` × `open`\|`restricted`） |
| `get-pmgr-arbiter` | PMGR 电压仲裁值（supported:no 时附原始 NVAPI 状态） |
| `set-pmgr-arbiter CSV` | 写入 11 个 dword（需管理员；建议 GET-改-SET 的 RMW） |
| `set-perf-freq-caps MAX_MHZ [--min MIN_MHZ]` | GPU 频率 perf-cap（PerfLimitsSetStatus） |
| `reset-perf-freq-caps` | 清除 perf-cap |

```bash
nvoc-cli set-perf-freq-caps 2100 --min 300      # 驱动会钳制
nvoc-cli reset-perf-freq-caps
```

### OC scanner 族（`scanner`，1 条命令）

| 命令 | 用途 |
|---|---|
| `oem-oc-scanner [--start] [--stop] [--revert] [--status] [--background-on/off] [--incomplete]` | 驱动侧（OEM）OC Scanner 控制，驱动 ≥ 455.00 |

```bash
nvoc-cli oem-oc-scanner --start      # 驱动后台扫描并自行应用 V/F 偏移
nvoc-cli oem-oc-scanner --status
nvoc-cli oem-oc-scanner --revert     # 恢复扫描前曲线
```

无控制台进度输出。`--incomplete` 查询 INCOMPLETE 运行的部分结果。

### Reset 清单（恢复速查）

| Reset 命令 | 清除对象 |
|---|---|
| `reset-pstate-global-freq-offset [--domain]` | P-State 全局频率偏移 |
| `reset-freq-lock [--domain]` | 频率锁（NVAPI/NVML） |
| `reset-pstate-lock` / `reset-private-forced-pstate-lock-user` | 原生 / 强制 P-State 锁（**不含**永久锁——那需要重启） |
| `reset-public-vftable-offset` / `reset-public-vftable-gpc-lock` | 公开 V/F delta / 电压锁 |
| `reset-private-vftable-offset BANK […]` | 私有 V/F 覆盖（freq+volt 平面） |
| `reset-private-freq-domain-global-offset […]` | ClkDomains 全局偏移（默认全部域 × 两个槽） |
| `reset-power-limit` / `reset-public-tgp-percent` | TGP 瓦特 / 百分比 |
| `reset-temp-limit` / `reset-temp-sim` | 传感器限制 / 温度仿真 |
| `reset-fan-speed` / `reset-fan-curve` | 风扇覆盖（控制块 bit0）/ 曲线槽 |
| `reset-legacy-application-freq-lock` | NVML application clocks |
| `reset-legacy-gpc-rail-overvolt-limit` / `reset-public-gpc-rail-volt-boost` | 基础电压 delta / 提升百分比 |
| `reset-autoboost-status on/off` | NVML auto-boost 默认值 |
| `reset-perf-freq-caps` | 频率 perf-cap |

### 命令重命名（2026-08 规范化——老脚本对照）

依据 `cli/RENAME_DECISIONS.md`（offset 取代 delta、`temp-limit` 取代 `thermal-limit`、get/set/reset 词干对称）。重要改名：`get-vfp`→`get-public-vftable`、`set-core-offset-mhz`/`set-memory-offset-mhz`→`set-pstate-global-freq-offset --domain`、`set-locked-clocks-mhz`→`set-freq-lock`、`get/set-tgp-watt`→`get/set-power-limit`、`set-dynamic-boost`→`set-ppab-status`、`get/set-pstate-native`→`get/set/reset-pstate-lock`、`get-clk-domains`→`get-private-freq-domain-info`、`get-clk-vf-points`→`get-private-vftable`、`thermal-sim`→`temp-sim`、`get-tdp-temp-limits`→`get-public-power-limit`+`get-public-temp-limit`。仅存一个别名：`set-legacy-overvolt-uv` → `set-legacy-gpc-rail-overvolt-limit`。上文 `--policy` 风扇选择器与 `--domain` 词汇截至 2026-09 均为最新。

> 注意：部分 `cli/README.md` 用法示例仍是改名前的旧名（`get-power-watt`、`set-core-offset-mhz`、`set-locked-clocks-mhz`、`get-vfp`）——这些已过期；本页名称均对照 `cli/src/lib.rs` 验证。

---

*Maintained from: cli/README.md, cli/src/main.rs, cli/src/lib.rs, cli/src/output.rs, cli/src/vbios.rs, cli/RENAME_DECISIONS.md, cli-common/.*
