# PowerMizer / CoreVoltageControl / PMGR-Arbiter semantics (R610.74)

> **Driver-specific evidence:** handler addresses and layouts below were derived
> from the analyzed R610.74 `nvapi64_impl.dll` (nvamsi) + `nvlddmkm.sys`. Re-derive
> before use on another build. All three SET entries are admin-gated on this build.

Deep-dive follow-up to `oc-gap-layouts-r610-74.md` (2026-08-28). Method: user-mode
handler decompile of every caller of the shared kernel-transport wrapper
`sub_180389320(escape, buf, size, ...)` + kernel-side string/regdb survey +
live readback on the RTX 4060 Laptop.

## Transport (shared by all three families)

`sub_180389320` → `sub_180389620`: builds the NVAPI kernel header
`[0]=0x4E565F41, [1]=0x10002, [2]=size, [3]=0x4E56452A, [4]=escape` then issues
the admin ioctl `0x8DE0010`. Escape ids are **1:1 with user-mode handlers** — an
exhaustive immediate sweep found no second handler per escape (apparent
collisions were adjacent constants: `PerfCheckDefaultMode`/`PerfDebugModeGetStatus_LEGACY`
use the sibling escape 0x70001BA, `CheckIfDriverHackedForSLI` uses 0x70001C5).

Kernel side: none of the escape ids appear anywhere in `nvlddmkm.sys` (bytes or
immediates). The dispatch is not statically visible on this GSP-era driver — the
semantic handlers live behind the RM/GSP boundary. Kernel-visible residue that
ties the families to RM subsystems: `RmPmgrOverride`/`RmPmgrSpeedo*`/`RmPmgrIddq*`
regdb params (PMGR = RM power manager), an `EnableCoreVoltage` regdb DWORD gating
a byte in a perf/volt struct, and 7+ all-caps `POWERMIZER` regdb descriptor rows.

## 1. PowerMizer — escape 0x700003A, 0x48-byte struct

IDs: GetPowerMizerInfo 0x76BFA16B @0x1802392A0, SetPowerMizerInfo 0x50016C78
@0x180261BC0 (SET admin-gated → -104).

Struct field map (validated in both handlers):

| off | field | GET | SET |
|---|---|---|---|
| +0x28 | hGpu | in | in |
| +0x34 | status | out (kernel writes; `86` → user maps to -120) | out (86 → -120) |
| +0x38 | source-1 | 1→0, 2→1 (only {1,2} accepted) | same |
| +0x3C | mode | out (internal 0/1) | in (6→0, 7→1) |
| +0x44 | op / queryType | 3 | 2 |

`queryType` other than 3 is rejected pre- or post-call (-5) — 3 is the only
mode-query on this build. Output is strictly internal {0,1}, published as 6/7.

**Empirical verdict (2026-08-28, live experiments on the 4060L): GET is NOT a
readback and this is not a runtime state.**

- User-measured: toggling the NVCP power-management dropdown does NOT change the
  GET; AC/battery transitions do NOT change the GET either.
- Elevated SET experiment (`build/probe_pmizer.ps1`, P/Invoke on 0x50016C78):
  `SET source=1 mode=6` and `SET source=2 mode=6` both return NVAPI_OK, yet GET
  still reports 7 for both sources afterwards; nvidia-smi shows no behavioral
  change (P8, 405 MHz, idle watts constant). Restore SETs (mode=7) also rc=0.
- So the GET returns a **boot-time constant / configuration report** (internal
  1, published 7) on this system — the same "GET is not a readback" pattern as
  the SetPerfLevel companion 0x77D8F573 (constant (1,1,0)). The SET is accepted
  but has no runtime effect; at best it writes a store that is only sampled at
  driver/GSP init, or nothing reads it at all.

**What PowerMizer is:** NVIDIA's classic DVFS/performance governor (mobile-born:
battery-vs-performance P-state/clock/voltage policy engine). The name survives as
an RM subsystem (kernel regdb has 7+ `POWERMIZER` descriptor rows plus a
`POWERMIZER_HARD` variant) and in Linux nvidia-settings (`GPUPowerMizerMode`),
but on GSP-era Windows drivers the DVFS decisions live in the GSP firmware and
this NVAPI surface is vestigial. The `{1,2}` selector most plausibly mirrors the
classic stored-mode pair (PowerMizerMode / PowerMizerMode_AC) — two slots, not a
measured AC/DC condition. Related IDs on sibling escape 0x70001BA:
PerfCheckDefaultMode 0x8AA0E961 (iterates heads, flags non-default V/F curves via
private perf tables), PerfDebugModeGetStatus_LEGACY 0xB16235C5 — perf-mode
neighborhood, "LEGACY" suffix and all. Distinct from SetPerfLevel 0x75DD3E6A (a
pstate *lock* with its own 4th lock store).

**Use:** none found. `get-power-mizer` is a constant probe on this GPU; the SET
has no observable effect. Surface value ≈ 0 — recommend withdrawing both from
CLI/pynvoc (keep the sys bindings + this note for the record).

## 2. CoreVoltageControl — escapes 0x07000043/44/45, 0x38-byte struct

IDs (nvVoltage.spec): GetCoreVoltage 0x58337FA3 @0x1801C9CE0 (escape 0x43),
GetCoreVoltageControl 0xA91F88EB @0x1801C9E30 (escape 0x45),
SetCoreVoltageControl 0xDC2BD4A6 @0x1801CB300 (escape 0x44, admin-gated).

All three share one shape: hGpu @+0x28, scalar @+0x34, no version magic.
GetCoreVoltage alone sets the transport-fallback flag (a5=1, may use the admin
device if the primary path is unavailable). The SET performs **zero user-mode
validation** — the u32 passes through raw.

**Live (4060L):** GetCoreVoltageControl = **1**, while the actual core rail
(rail 0) reads 625000 µV via the VoltRails/Pmumon surfaces. The control scalar is
therefore **not a voltage value** — most plausibly a state/capability word of the
core-voltage control object (1 = default/auto, or capability const).

**Identity (medium confidence):** the RM VOLT module's core-voltage *control
object* — a request/override slot distinct from the measurement path
(GetCoreVoltage = scalar read, sibling in the same spec). Corroborating context:

- The whole family sits in nvVoltage.spec beside VoltVoltRails Get/SetControl
  (the melonVolt µV-offset path, RM 0x07000191) and
  VoltPmumonVoltRailsGetSamples (measurement). On this Ada-mobile part the
  VoltVoltRails control entry reads type=0 = "no offset control"; CVC=1 likely
  marks the same "not controllable here" state.
- Kernel `EnableCoreVoltage` regdb DWORD (0/1, unset on this machine) gates a
  core-voltage feature byte — the family has an opt-in enable knob.

Historical plausibility: the CoreVoltageControl pair is the modern residue of the
old partner-tool core-voltage override surface (Fermi-era EVGA/Precision voltage
unlock went through NVAPI); on contemporary parts the writable offset moved to
VoltVoltRails. Treat the SET as **likely inert on this SKU, semantics unconfirmed**
— never blind-write it.

## 3. PMGR voltage-request arbiter — escape 0x0700019F, 0x70-byte struct

IDs (nvVoltage.spec): GetPMGRVoltageRequestArbiterValues 0x717648FD
@0x1801C9F80 (get-flag 0), SetPMGRVoltageRequestArbiterValues 0x9C4BB8D0
@0x1801CB480 (flag 1, admin-gated).

Layout: gpuSelector @+0x30, get/set flag @+0x34; v1 magic 0x10024 (8 payload
dwords @ +0x38..0x44, +0x50..0x5C), v2 0x20030 (+3 dwords @ +0x48..0x4C, +0x60).
The GET copies back exactly the payload dwords; SET forwards them. The values are
opaque to the user-mode layer (no validation, no scaling).

**Verdict (2026-08-28, closed): the method is not registered on consumer
drivers — permanently unsupported there.**

- User sweep: RTX 20 desktop, RTX 30 desktop and RTX 40 laptop all return
  `supported: no` from `get-pmgr-arbiter`.
- Raw-status probe (`build/probe_pmgr.ps1`, P/Invoke 0x717648FD): wrong version
  magic → -9 (call reaches the kernel), correct v1/v2 → **-104
  (NVAPI_NOT_SUPPORTED)**, identical elevated and non-elevated → NOT the admin
  gate; the kernel-side method table has no entry for interface 7 / method
  0x19F on this driver.
- Kernel dispatcher (nvlddmkm sub_1418F4BA0): the NvApi kernel API is
  interface-numbered (0x0700xxxx ⇒ interface 7, method < 0x1000) and resolves
  methods through a runtime-populated table; consumer builds don't carry this
  one.
- GSP firmware evidence (gsp_ga10x.bin, uncompressed ELF, 84 MB): PMGR = RM
  Power Manager — `PMGR_PWR_CHANNEL_*` (incl. `OUTPUT_VOLTAGE`),
  `PMGR_PWR_MONITOR_*`, `PMGR_PWR_POLICY_*` with `WORKLOAD_DIE` /
  `WORKLOAD_PHYSICAL_SINGLE` types (the workload power-capping machinery
  PPAB/Dynamic-Boost sits on), `PmgrIddq*` / `PmgrIsense*` current-sense
  overrides, `PmgrOverride` / `PmgrPmuOverride` knobs. But the string
  "arbiter" (any case) has **zero occurrences**, and no VoltRequest enums —
  the voltage-request arbitration component is not compiled into consumer
  GSP-RM at all.

**Identity:** the NDA nvVoltage.spec surface for PMGR's per-client voltage
request arbitration (combining requests from clock clients / VFE tables / rail
offsets into the final core-rail request). Its natural home is the server/
datacenter GSP builds with multi-rail telemetry; consumer SKUs will never
register it. Practical value on consumer hardware: none.

## CLI / pynvoc surface status

| family | CLI | pynvoc | verdict |
|---|---|---|---|
| get-power-mizer | `get-power-mizer` | `get_power_mizer` | withdraw — constant (7) on this GPU, no information |
| set-power-mizer | rolled back | — | stays out — SET accepted (rc=0 elevated) with zero effect |
| get-core-voltage-control | `get-core-voltage-control` | `get_core_voltage_control` | keep — harmless capability/state read |
| set-core-voltage-control | `set-core-voltage-control` | `set_core_voltage_control` | demote/hide (RENAME_DECISIONS "待确认" resolves to: likely inert on Ada mobile, no validation, do not expose) |
| get-pmgr-arbiter | `get-pmgr-arbiter` | `get_pmgr_arbiter` | keep as capability probe — reports the raw status now (`status_code: -104 NotSupported` on consumer SKUs) |
| set-pmgr-arbiter | `set-pmgr-arbiter` | `set_pmgr_arbiter` | keep but experimental-only (unreachable on this GPU) |

`GetCoreVoltage` 0x58337FA3 (measurement twin) exists in nvapi-rs
(`core_voltage_scalar`) but is not surfaced by core/CLI/pynvoc; the measurement
dimension is already covered by VoltRails/Pmumon — leave unwrapped.
