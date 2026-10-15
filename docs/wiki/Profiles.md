# Profiles: multi-slot overclock save / load / import / export

[English](#english) | [中文](#中文)

<a id="english"></a>

## English

`nvoc-core`'s `profile` module (surfaced as the `nvoc-cli profile-*` command family and the `pynvoc.profile_*` bridge) captures the live overclocking state into named, versioned JSON slots bound to **one physical GPU**, and replays them back with identity checks, read-back verification and rollback. See [[Safety-and-Recovery]] for the general write-safety model; this page documents the profile-specific guarantees.

### Why identity binding

A stable V/F curve on one card can crash an identical model. Every profile records the GPU's **NVML UUID** (primary key) plus a **hardware fingerprint** (PCI device/subsystem IDs, VBIOS version, model name, P-State roster — the driver version is recorded but never compared). `profile-apply` refuses to run unless both agree; a VBIOS flash or subsystem change requires the explicit `--rebase` re-bind. The PCI bus position is recorded for humans only and never matched (keying on bus position breaks when a card is reseated — the MSI Afterburner failure mode).

### Storage layout

```
<root>/profiles/<uuid-slug>/
    stock.json          # `profile-capture-stock` reference snapshot
    backup-last.json    # pre-apply snapshot, written before every apply
    active              # name of the last successfully applied slot
    <slot-slug>.json    # one named profile per file
```

Default root: `%PROGRAMDATA%\nvoc\profiles` (Windows) / `$XDG_CONFIG_HOME/nvoc/profiles` or `/etc/nvoc/profiles` (Linux) — the same tier as the `nvoc-srv` config. Override per invocation with `--profile-root`. Every write is atomic (temp file + rename); the directory listing is the source of truth (no index file to corrupt).

### Schema (v1)

One JSON document per slot (`schema_version: 1`, `deny_unknown_fields`):

- `gpu` — UUID + fingerprint identity block (see above).
- `domains` — only the captured settings families appear; **an absent domain is never written on apply** (the MSI Afterburner `ProfileContents` mask idea, expressed as omitted fields):
  `clock_offsets` (per-PState per-domain kHz deltas), `public_vftable` (per-point V/F deltas, graphics + memory), `voltage` (boost %, overvolt µV), `power` (watts and/or percent), `thermal` (sensor limits), `fan` (per-cooler curves), plus `locked_clocks` / `vfp_locks` which are schema-reserved and **never default-captured** (opt-in via `profile-save --domains`).
- `baseline` — the stock reference values at capture time (used by `profile-diff` and future rebinding).
- `validation` — optional user-recorded stress evidence.

Units are schema-fixed (kHz / µV / W / %); frontends convert for display only. Value types are reused verbatim from the `GpuSettings` snapshot type, so capture ↔ apply round-trips bit-exactly.

### Apply pipeline

`profile-apply NAME` runs a fixed sequence with no unplanned writes:

1. **Identity verdict** — UUID + fingerprint match or refuse (four-tier matrix; driver drift alone only warns and forces verification).
2. **Plan** — each captured domain gets an explicit verdict: `apply`, `skip` (generation gate — e.g. no editable V/F curve on pre-Pascal; reported, never silently dropped) or `deferred` (lock families in v1). `--dry-run` prints the plan and stops.
3. **Backup** — the live `GpuSettings` + fan curves + power bounds are snapshotted into `backup-last.json` before the first write.
4. **Write** — fixed order: pstate offsets → public V/F deltas (reset-first) → voltage → power → thermal → fan.
5. **Verify** — read back and compare (watts with a ±2 W NVML quantization tolerance). Any failed domain triggers rollback from the snapshot unless `--no-rollback`.
6. On full success the slot is recorded in the `active` marker; `profile-restore-last` replays the snapshot at any time.

### Portability

`profile-export [--out FILE] [NAME…]` bundles slots into one portable `*.nvocprofile.json` package (schema-versioned, host/purpose metadata). `profile-import` validates with a rejection ladder (wrong kind → unsupported version → empty → too many → aggregated per-slot errors) and never re-binds slots to a different card; apply-time identity checks still guard the hardware. Name conflicts resolve via `--on-conflict skip|overwrite|rename`.

### Command summary

```bash
nvoc-cli profile-capture-stock              # run at stock: reference snapshot
nvoc-cli profile-save daily [--domains clock-offsets,power] [--note "..."]
nvoc-cli profile-list / profile-show daily / profile-diff daily
nvoc-cli profile-apply daily --dry-run      # print the plan only
nvoc-cli profile-apply daily                # backup -> write -> verify (rollback on failure)
nvoc-cli profile-restore-last               # replay backup-last.json
nvoc-cli profile-rename old new / profile-copy old new / profile-delete old
nvoc-cli profile-export --out daily.nvocprofile.json daily
nvoc-cli profile-import daily.nvocprofile.json --on-conflict rename
```

Same operations from Python via `pynvoc`: `profile_capture_stock`, `profile_save`, `profile_list`, `profile_show`, `profile_diff`, `profile_apply`, `profile_restore_last`, `profile_export`, `profile_import` (all take `gpu`, most take `profile_root`; `profile_apply` exposes `dry_run` / `no_verify` / `no_rollback` / `rebase`).

### Out of scope (v1)

Boot-time auto-apply (srv integration needs crash-loop protection — a readiness handshake like xOCD's startup channel or an in-progress marker), voltage-rail and clock-lock appliers, and the auto-optimizer `vfp-final.csv` → profile bridge.

<a id="中文"></a>

## 中文

`nvoc-core` 的 `profile` 模块（CLI 命令族 `nvoc-cli profile-*`、Python 桥 `pynvoc.profile_*`）把实时超频状态捕获成**绑定单张物理卡**的具名版本化 JSON 槽位，并在回放时做身份校验、回读验证与回滚。通用写安全模型见 [[Safety-and-Recovery]]。

### 为什么要身份绑定

同一型号的两张卡，一张稳定的 V/F 曲线可能在另一张上直接死机。每个 profile 记录 **NVML UUID**（主键）+ **硬件指纹**（PCI device/subsystem ID、VBIOS 版本、型号、P-State 花名册；驱动版本只记录、永不参与比对）。`profile-apply` 要求两者全部一致，否则拒绝；刷 VBIOS 或换卡后的重绑必须显式 `--rebase`。PCI 总线号仅作记录、绝不参与匹配——按总线位置做键，挪槽即失配（MSI Afterburner 的教训）。

### 存储布局

```
<root>/profiles/<uuid-slug>/
    stock.json          # profile-capture-stock 的出厂参考快照
    backup-last.json    # 每次 apply 前的现场快照
    active              # 最近一次成功应用的槽名
    <slot-slug>.json    # 每槽一文件
```

默认根目录：Windows `%PROGRAMDATA%\nvoc\profiles`；Linux `$XDG_CONFIG_HOME/nvoc/profiles` 或 `/etc/nvoc/profiles`——与 `nvoc-srv` 配置同级。可用 `--profile-root` 覆盖。所有写入均为原子写（临时文件+改名）；目录列表即真相（没有会损坏的索引文件）。

### Schema（v1）

每槽一份 JSON（`schema_version: 1`，`deny_unknown_fields`）：

- `gpu` —— UUID + 指纹身份块。
- `domains` —— 只序列化被捕获的域；**缺席的域在 apply 时绝不触碰**（Afterburner `ProfileContents` 位掩码思想，用"字段缺席"表达）：
  `clock_offsets`（各 PState 各域 kHz 偏移）、`public_vftable`（逐点 V/F 偏移，核心+显存）、`voltage`（boost %、overvolt µV）、`power`（瓦特和/或百分比）、`thermal`（传感器限制）、`fan`（各风扇曲线）；另有 `locked_clocks` / `vfp_locks` 为 schema 预留，**默认永不捕获**（需 `profile-save --domains` 显式点名）。
- `baseline` —— 捕获时刻的出厂参考值（供 `profile-diff` 与后续重绑）。
- `validation` —— 可选的用户压测记录。

单位在 schema 层固定（kHz / µV / W / %），换算只发生在前端展示层。取值类型直接复用 `GpuSettings` 快照类型，捕获 ↔ 应用按位精确往返。

### Apply 管线

`profile-apply NAME` 按固定顺序执行，绝无计划外写入：

1. **身份裁决** —— UUID+指纹全等才放行（四档裁决矩阵；仅驱动版本差异只降级为警告并强制回读验证）。
2. **计划** —— 每个已捕获域给出显式裁决：`apply`、`skip`（代际门——如 pre-Pascal 无可编辑 V/F 曲线；明确上报，绝不静默跳过）或 `deferred`（v1 的锁族）。`--dry-run` 只打印计划。
3. **备份** —— 首次写入前把现场 `GpuSettings`+风扇曲线+功率界快照进 `backup-last.json`。
4. **写入** —— 固定顺序：PState 偏移 → 公共 V/F 偏移（先复位）→ 电压 → 功率 → 温度 → 风扇。
5. **验证** —— 回读比对（瓦特按 NVML 量化粒度留 ±2 W 容差）。任一域失败即从快照回滚，除非 `--no-rollback`。
6. 全部成功后把槽名写入 `active` 标记；`profile-restore-last` 可随时重放快照。

### 可移植性

`profile-export [--out FILE] [NAME…]` 把槽位打包成单个可移植 `*.nvocprofile.json`（带 schema 版本与主机/用途元数据）。`profile-import` 按拒绝阶梯校验（kind 不对 → 版本不支持 → 空 → 超上限 → 逐槽错误聚合），且**绝不**把槽位重绑到别的卡；硬件仍由 apply 期身份检查把关。命名冲突用 `--on-conflict skip|overwrite|rename` 处理。

### 设计出处

机制对比逆向结论（2026-10，issue #360）：MSI Afterburner 以 `ProfileContents` 位掩码选捕获域、按"含总线号的硬件串"分目录（反面教材）；xOCD 以版本化 JSON 包 + 逐控制项 readback 兼容裁决 + apply→verify→restore 生命周期；两者与 #360 的 UUID 绑定方案共同构成本模块的骨架。

### v1 范围外

开机自动应用（srv 集成需崩溃循环保护——类似 xOCD 启动通道的 readiness 握手或 in-progress 标记）、电压轨/锁频 applier、auto-optimizer `vfp-final.csv` → profile 桥。
