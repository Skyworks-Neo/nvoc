# NVML.dll vs nvml-wrapper-sys coverage audit

Companion to the NVAPI work. Question: the `nvml-wrapper-sys` 0.9.1 crate (which
nvoc depends on) was generated against **NVML v11** (~2021). The on-disk
`temp/nvml.dll` is a **2024-05** build (driver R550+, ~NVML v12). How many NVML
functions are *unbound* (present in the DLL, missing from the Rust bindings),
and of those, how many have enough public information to be usable?

## Method

1. Dumped all exports of `temp/nvml.dll` via `objdump -p` → **417** unique
   `nvml*` symbols.
2. Extracted all `extern "C" { pub fn nvml* }` from
   `nvml-wrapper-sys-0.9.1/src/bindings.rs` → **373** bound symbols.
3. Diffed: **44** symbols are exported by the DLL but not bound by the wrapper.
   (0 symbols bound by the wrapper are absent from the DLL — NVML is
   backwards-compatible, as documented.)
4. Cross-referenced the 44 against `nvml.h` from **CUDA 12.8** (`NVML_API_VERSION 12`)
   — the authoritative public signature source.

## Headline result

**All 44 unbound symbols are absent from the public `nvml.h`.** They are private
exports (or aliases). None carries a documented signature in the shipped header.

## Reclassification by usability

The 44 are not equally opaque. Grouped by "how much information is available":

### [A] Public alias — trivially bindable (2)

| Export | = | Public function (already bound) |
|---|---|---|
| `nvmlGetBlacklistDeviceCount` | alias of | `nvmlGetExcludedDeviceCount` |
| `nvmlGetBlacklistDeviceInfoByIndex` | alias of | `nvmlGetExcludedDeviceInfoByIndex` |

`nvml.h` defines these as `#define nvmlGetBlacklistDeviceCount nvmlGetExcludedDeviceCount`
(undocumented rename of the older "excluded device" API). Signature fully known;
binding them adds nothing the existing wrapper doesn't already expose.

### [B] Versioned variant of a PUBLIC base function — signature inferable (5)

These `_v1`/`v2` exports are newer revisions of functions whose base name **is**
declared in `nvml.h`. The signature is the base signature + whatever the bump
changed (usually a struct version field or an added out-param). Bindable with
light RE/confidence:

- `nvmlDeviceGetAccountingStats_v2`  (base public)
- `nvmlDeviceGetRemappedRows_v2`     (base public)
- `nvmlDeviceGetVgpuSchedulerLog_v2` (base public)
- `nvmlDeviceGetVgpuSchedulerState_v2` (base public)
- `nvmlDeviceSetVgpuSchedulerState_v2` (base public)

### [C] Private — no public signature, RE required (37)

Everything else. Their base names are themselves absent from `nvml.h`, so there
is no public contract. Sub-themes:

- **vGPU scheduler / heterogeneous / placement** (datacenter vGPU only, ~16):
  `nvmlGpuInstanceGetActiveVgpus`, `...GetCreatableVgpus`,
  `...GetVgpuHeterogeneousMode`, `...SetVgpuHeterogeneousMode`,
  `...GetVgpuTypeCreatablePlacements`, `nvmlVgpuTypeGetMaxInstancesPerGpuInstance`,
  `nvmlDeviceVgpuForceGspUnload`, the `_v1` SchedulerLog/State variants, etc.
  Irrelevant to nvoc (single desktop GPU, no vGPU).
- **System event sets** (5): `nvmlSystemEventSetCreate/Free/Wait`,
  `nvmlSystemRegisterEvents`, `nvmlSystemGetCPER_v1`. Newer telemetry/event API.
- **Internal / escape hatch** (several): `nvmlInternalGetExportTable` is the
  classic NVIDIA pattern — a private function that hands back a *different*
  function-pointer table (often the "real" internal API). `nvmlDeviceGetHandles`,
  `nvmlDeviceGetHandleByUUIDV`, `nvmlDeviceGetPdi`, `nvmlDeviceReadWritePRM_v1`,
  `nvmlDeviceReadPRMCounters_v1`, `nvmlDeviceGetNvLinkInfo`,
  `nvmlDeviceGetAddressingMode`, `nvmlDeviceGetRepairStatus`,
  `nvmlDeviceGetSramUniqueUncorrectedEccErrorCounts`, `nvmlDeviceGetBBXTimeData_v1`,
  `nvmlDeviceSetRusdSettings_v1`, `nvmlDeviceWorkloadPowerProfileUpdateProfiles_v1`.
  These are the genuinely interesting unknowns but have zero public info — each
  would need full static RE of `nvml.dll` to bind safely.

## Conclusion for nvoc

- **Coverage is effectively complete for documented functionality.** The wrapper
  binds all 373 publicly-documented NVML functions; the 44 gaps are either
  aliases (already covered), vGPU/datacenter private APIs (out of scope), or
  undocumented internals (no contract to bind against).
- **No low-hanging monitoring fruit.** Unlike the NVAPI side (where
  `GetComputeCapabilities` was undocumented-but-RE-able and added real value),
  the NVML gaps here are either redundant (aliases), irrelevant (vGPU), or
  opaque-internal (would each need RE with no public reference).
- **The one structurally interesting export** is `nvmlInternalGetExportTable` —
  NVIDIA's usual door to a hidden function-pointer table. If a future need
  arises to reach an NVML feature not in the public API, that is the entry point
  to RE first (mirror of the NVAPI `QueryInterface` dispatch work already done).

Data artifacts: `temp/nvml.dll`, `temp/nvml.h` (CUDA 12.8). Analysis is
reproducible from `objdump -p nvml.dll` + the bindings.rs/header diffs.

---

# Appendix: `nvmlInternalGetExportTable` deep-dive (static RE)

Followed up on the "one structurally interesting export." Static RE of
`temp/nvml.dll` (R610 `r610_45` driver branch) in IDA.

## Mechanism

`nvmlInternalGetExportTable(void **out, void *guid16)` @ `0x180002390` — a clean
3-iteration walker over a registry at `0x18027C910` (3 entries × 16 bytes, each
`[guid_ptr, table_ptr]`). Compares the caller's first two qwords (full 16-byte
GUID) against each registered GUID; on match returns `*out = table_ptr`,
otherwise returns `2`. Only called via the export table (no internal callers) —
a pure public escape hatch, exactly mirroring NVAPI's `QueryInterface`.

## The three registered interfaces

The first qword of each table is the **byte-size of the whole structure**
(header + pointer array), not an entry count.

| GUID (16 bytes) | Table addr | Size field | Slots | Non-null ptrs | Role |
|---|---|---|---|---|---|
| `c4fe3e6c-c98f-6c4e-a327-ee696e12f7c4` | `0x180281700` | 2376 | 294 | **232** | device-API dispatch table (294 = exactly the `nvmlDevice*` count in the binary) |
| `b81dd730-e4d2-8144-9cd0-e1de3504fa27` | `0x180282050` | 64 | 7 | **7** | small system-object + device interface |
| `9e8c7a58-123a-b54f-b9e3-f4ba3945dd5f` | `0x180282090` | 16 | 1 | **1** | single legacy/stub method (writes `*out=0`, returns success) |

The three pointer sets are **disjoint** (zero overlap, different address bands) —
three genuinely different interface contracts, not nested views.

## What the shims actually do

Every slot is a C++-style **vtable-dispatch shim** sharing one skeleton:
log at DEBUG≥5 to `apps/nvml/entry_points.h:<line>` with the entry's integer ID
and arg signature → take a global lock → validate the device handle → dispatch
via `*(device+164680) -> vtable -> method_at_offset`. Example (table0 slot 0,
ID 1921, sig `(%p,%d,%d,%p)`): `*(dev+164680) -> *(+320) -> fn@+120`. Each shim
has exactly 3 data xrefs (the table slot, a reloc, and a `{shim_rva, impl_rva,
name_hash}` triad) — they are a registration-only dispatch surface, never called
by the public exports.

## Does it expose anything the 417 public exports don't? — No

- table0's 294 slots mirror the 294 `nvmlDevice*` functions 1:1 (232 populated);
  it is a **parallel implementation layer, not a superset**.
- The public `nvml*` exports (`nvmlDeviceGetTemperature` @ `0x180049BF0`,
  `nvmlDeviceGetClockInfo` @ `0x180034D50`, …) are the **client-side** wrappers;
  these GUID-keyed tables are the **server-side** interface that the privileged
  nvml daemon registers (nvml.dll is split client/daemon in modern driver builds).
  Same device object (`dev+164680`), same APIs, different dispatch contract.
- The metadata block at `0x180282118+` lists the API names in order — every one
  (`GetClockInfo`, `GetTemperature`, `GetPowerUsage`, `GetFanSpeed`,
  `GetUtilizationRates`, …) is a known public NVML API. **No private/undocumented
  monitoring name appears.**

## GUID identification

The three GUIDs are **not** any documented NVIDIA interface GUID (not the
public D3D/CUDA UMD interface GUIDs, not the NVAPI/NVML v2/v3 version GUIDs).
They appear *only* at their registration slots in the binary — no symbol names
them, and they are not in NVIDIA's open-source kernel-module repo (which is
RM/kernel-side, not UMD nvml.dll-side). They are private, undocumented
client/daemon "export-table" interface IDs. **Unmatched to any public reference.**

## Conclusion (definitive)

There is **no wrapping opportunity** in `nvmlInternalGetExportTable` for nvoc.
All three GUID-keyed tables reach the same `nvmlDevice*` device methods the 417
public exports already expose, just via the internal client/daemon dispatch
contract. No additional monitoring capability (temperature, power, clock, fan,
utilization, memory, pstate) is hidden behind it. The earlier hypothesis
("internal table may hide richer APIs, like NVAPI's GetComputeCapabilities did")
is **falsified** for NVML: unlike NVAPI, the NVML internal surface is exactly
the public surface.
