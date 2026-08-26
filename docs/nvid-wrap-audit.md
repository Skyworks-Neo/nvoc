# nvid.rs Wrap Audit — 2026-08-25

Complete audit of `nvapi-rs/sys/src/nvid.rs` (1211 enum entries, 2177 distinct
ID-name references incl. comments) against the full project to identify
**overclock-relevant IDs not yet wrapped/fully reversed**, and to flag
duplicate/alias-annotation issues.

## Summary counts

| category | count |
|---|---|
| total `Api` enum entries | 1211 |
| referenced anywhere in project (non-nvid.rs) | 194 distinct names |
| OC-relevant names in nvid.rs | 431 |
| OC-relevant names **never referenced** in project | ~280 (bulk = YOFOO spec-block reservations) |
| wrapped-in-hi (full high-level API) | ~95 |
| sys-only (FFI declared, no hi wrapper) | ~20 |
| example-only (probe scripts) | ~25 |
| **high-value unwrapped OC control IDs** (real gaps) | **9** |
| duplicate hex IDs (enum-level) | **0** (build would fail) |
| alias-annotation inconsistencies | **2** (documented, benign) |

## 1. High-value unwrapped OC control IDs (real gaps)

These are genuine overclock/voltage/thermal **setter or control** surfaces —
not telemetry, not YOFOO bulk reservations, not already-wrapped. Ordered by
likely value.

### 1a. Voltage offset (core/MVDD) — **highest value**

| hex | name | role | evidence |
|---|---|---|---|
| 0xDC2BD4A6 | `NvAPI_GPU_SetCoreVoltageControl` | core voltage control SET | nvVoltage.spec YOFOO; paired with GetCoreVoltageControl 0xA91F88EB (also unwrapped) |
| 0x9C4BB8D0 | `NvAPI_GPU_SetPMGRVoltageRequestArbiterValues` | PMGR voltage-request arbiter SET | nvVoltage.spec; the PMGR (power-management microcontroller) voltage path — distinct from the wrapped VoltRails (0x87C55C8A) family |

**Note**: nvoc already wraps the **melonVolt** `VoltVoltRails` SET (0x87C55C8A,
per-rail µV offset) + the legacy `ClientVoltRailsSetControl` (0xB9306D9B,
voltage-boost percent). `SetCoreVoltageControl`/`SetPMGRVoltageRequestArbiterValues`
are a THIRD, lower-level voltage path (direct VRM/PMGR) — may overlap in effect
with VoltRails on some GPUs. Worth RE before wrapping to confirm it's not
redundant.

### 1b. Thermal simulation (3-piece) — **medium value, GPUMon Thermspy gap**

| hex | name | status | role |
|---|---|---|---|
| 0x8CD42541 | `NvAPI_GPU_SetThermalSimulationMode` | **sys-declared, UNWRAPPED** | temp-simulation SET (basic) |
| 0xAF97FE75 | `NvAPI_GPU_GetThermalSimulationMode` | wrapped (medium nvcall) | temp-simulation GET |
| 0x95E71AB6 | `NvAPI_GPU_SetExtendedThermalSimulationMode` | wrapped (medium nvcall) | temp-simulation SET (extended/VBIOS-secured) |

Memory `thermspy-reversed.md` flags the **3-piece thermal-sim trio** as the
real gap. Current state: 2 of 3 wrapped (GET + Extended-SET). The basic
`SetThermalSimulationMode` (0x8CD42541) is sys-declared but has no medium/hi
wrapper. Filling it completes the trio. **However** the Thermspy gap is
`0x8CD42541` + `0x95E71AB6` + `0xAF97FE75` as a *typed 4-arg* call
`(hGpu, flags, enable, temp)` — confirm the SET signature matches before
wrapping.

### 1c. PowerMizer — **medium value**

| hex | name | status | role |
|---|---|---|---|
| 0x50016C78 | `NvAPI_GPU_SetPowerMizerInfo` | **UNWRAPPED** | PowerMizer mode SET (perf/power-tradeoff policy) |
| 0x76BFA16B | `NvAPI_GPU_GetPowerMizerInfo` | wrapped | PowerMizer GET |

PowerMizer GET is wrapped; the SET (mode: adaptive/prefer-max-perf/prefer-
max-battery) is not. A real user-facing OC/power lever. Memory
`thermspy-reversed.md` lists PowerMizer as a gap.

### 1d. PerfRatedTdp family (4 IDs) — **medium value, possibly redundant**

| hex | name | status | role |
|---|---|---|---|
| 0xC9E9BB33 | `NvAPI_GPU_ClientRatedTdpControl` | **WRAPPED** | rated-TDP enable/disable (mode 3/0) |
| 0xED2BEA09 | `NvAPI_GPU_PerfRatedTdpGetControl` | UNWRAPPED | rated-TDP control GET (perf-nvPstate spec) |
| 0x87BD35EF | `NvAPI_GPU_PerfRatedTdpGetInfo` | UNWRAPPED | rated-TDP info |
| 0xFCBDF642 | `NvAPI_GPU_PerfRatedTdpGetStatus` | UNWRAPPED | rated-TDP status |

The SET/enable (`ClientRatedTdpControl`) is wrapped. The three PerfRatedTdp
GETs are the *info/status/control-snapshot* reads — likely redundant with the
wrapped enable (the SET is the only lever). Lower priority; wrap only if a
readback is needed.

### 1e. PCF DynamicBoost GET — **low value (SET already wrapped)**

| hex | name | status | role |
|---|---|---|---|
| 0x1504FC3D | `NvAPI_GPU_ClientDynamicBoostSetStatus` | **WRAPPED** | PPAB enable SET |
| 0xC80068A1 | `NvAPI_PCF_DynamicBoostGetStatus` | UNWRAPPED | PPAB enable GET (readback) |

SET wrapped, GET not. Low priority — the SET already verifies via readback
elsewhere.

### 1f. QBoost controller (3 IDs) — **DO NOT WRAP (ruled out)**

| hex | name | status | reason |
|---|---|---|---|
| 0xB4C5D8BA | `NvAPI_GPU_ClientQboostGetInfo` | unwrapped, **intentionally** | wrong PPAB-slider hypothesis (memory `dynamicboost-ppab-wrapped`) |
| 0xB78734AB | `NvAPI_GPU_ClientQboostSetStatus` | unwrapped, **intentionally** | same — QBoost ≠ PPAB path |
| 0xC9E9BB33 | (ClientRatedTdpControl) | wrapped | real path kept |

### 1g. PerfVfeEqu / PerfVfeVar (6 IDs) — **unknown, possibly legacy**

| hex | name | role |
|---|---|---|
| 0x4C75C9FE | `NvAPI_GPU_PerfVfeEquGetControl` | VFE equation control GET |
| 0x5D387298 | `NvAPI_GPU_PerfVfeVarGetControl` | VFE variable control GET |
| (+Set/Info variants) | | VFE = voltage-frequency-equation |

These appear to be a legacy/advanced V/F-equation editor family. Not seen in
any RE'd tool (GPUMon/MSI/EVGA/Thermspy). Likely internal-tuning only.
**Defer** — would need IDA RE to confirm it's not just a different view of the
already-wrapped VfPoints families.

### 1h. OC Scanner completeness (2 IDs)

| hex | name | status | role |
|---|---|---|---|
| 0xBC4AEE25 | `NvAPI_GPU_ClientStartOcScanner` | WRAPPED | scan start |
| 0xC28B73DE | `NvAPI_GPU_ClientStopOcScanner` | WRAPPED | scan stop |
| 0xCC727B22 | `NvAPI_GPU_ClientRevertOc` | WRAPPED | revert |
| 0x593E8E72 | `NvAPI_GPU_ClientGetLastOcScannerResults` | WRAPPED | last results |
| 0x1CB41116 | `NvAPI_GPU_ClientRegisterForOcScannerStatusUpdates` | WRAPPED (FFI) | callback register |
| 0x06DC7CE8 | `NvAPI_GPU_ClientEnableBackgroundOcScanner` | **UNWRAPPED** | background-scan enable |
| 0xBE371D0A | `NvAPI_GPU_GetLastIncompleteOcScannerResults` | **UNWRAPPED** | incomplete-scan results |

Two minor OC-Scanner completeness gaps. Low value unless background scanning
is needed.

## 2. Aliases / annotation consistency

### 2a. Documented alias IDs (same hex, two names — intentional, correct)

These are the YOFOO-alias pattern: the RE'd GPUMon name is the enum variant;
the YOFOO public name is in the doc comment. **19 total**, all correct. Key
ones:

| RE'd enum name (kept) | YOFOO alias | hex |
|---|---|---|
| ClientDynamicBoostSetStatus | PCF_DynamicBoostSetStatus | 0x1504FC3D |
| ClientTgpWattGetStatus | PowerPolicyGetControl | 0x8B3E7343 |
| ClientTgpWattSetStatus | PowerPolicySetControl | 0xAFFC2279 |
| ClientThermalTargetGetStatus | ThermalPolicyGetControl | 0xC4554575 |
| ClientThermalTargetSetStatus | ThermalPolicySetControl | 0xE097144F |
| ClientExternPowerStateSet | SetExternPowerState | 0x48E0847D |
| ClientPStateLimitStatus | GetPstateActiveLimits | 0x9962C97C |
| ClientRatedTdpControl | PerfRatedTdpSetControl | 0xC9E9BB33 |
| PerfPstatesGetInfoPrivate | PerfPstatesGetInfo | 0x7B30AE0D |
| ClientPowerPoliciesGetInfoPrivate | PowerPolicyGetInfo | 0x67F31384 |
| ClientThermalPoliciesPrivateGetInfo | ThermalPolicyGetInfo | 0x2F69F8E5 |
| GetVoltageStep | GetVoltageDomainsInfo | 0x28766157 |
| GetUsages | GetDynamicPstatesInfo | 0x189A1FDF |
| GetRamMaker | GetRamVendorID | 0x42AEA16A |
| GetManufacturingInfo | ManufacturingInfo | 0xA4218928 |
| GetComputeCapabilities | QueryComputeCaps | 0xB7BCF50D |
| ClientQboostGetInfo | PCF_ControllerGetInfo | 0xB4C5D8BA |
| ClientQboostSetStatus | PCF_ControllerSetControl | 0xB78734AB |
| ClientPowerPoliciesSetInfoPrivate | (no alias, private-only) | 0xAD9A2E6D |

### 2b. The `GetClockBoostLock`/`SetClockBoostLock` alias — **annotation only, NO duplicate enum entry**

```
771: NvAPI_GPU_PerfClientLimitsGetStatus = 0xe440b867, // aka ... NvAPI_GPU_GetClockBoostLock
772: NvAPI_GPU_PerfClientLimitsSetStatus = 0x39442cfb, // aka ... NvAPI_GPU_SetClockBoostLock
```

The YOFOO block (nvClocks.spec) does **not** separately register
`GetClockBoostLock`/`SetClockBoostLock` as enum variants — the only entry is
the `PerfClientLimits*` pair. The "aka" comments correctly document the alias.
**No action needed** — this is the intended single-source-of-truth pattern.

### 2c. `ClientTgpWattSetStatus` vs `ClientTgpWattSetStatus_GpuMonExe` — **two enum entries, two IDs (correct)**

```
0xAFFC2279  ClientTgpWattSetStatus          (CLI variant — live on 4060L)
0xBFF09E59  ClientTgpWattSetStatus_GpuMonExe (GUI variant — NULL on 4060L)
```

Two DIFFERENT hex IDs for the same role across the two GPUMon binaries. This
is correctly documented (memory `prefer-gpumoncmd-over-gpumon-exe`).
**No merge needed** — they're genuinely distinct IDs.

## 3. Wrap-status by family (OC-relevant)

| family | GetInfo | GetStatus | GetControl | SetControl | notes |
|---|---|---|---|---|---|
| ClientFanPolicies | ✅ hi | ✅ (0xCF6CEF26, unwrapped) | ✅ hi | ✅ hi | curve + reset + fan-stop all wrapped |
| ClientFanCoolers | ✅ hi | ✅ hi (0x3CC2D181) | ✅ hi | ✅ hi | wrapped (gpumon-fancurve family) |
| ClientFanArbiters | ✅ hi | ✅ hi | ✅ hi | ✅ hi | wrapped |
| ClientPowerTopology | ✅ hi | ✅ hi | — | — | read-only, wrapped |
| ClientPowerPolicies (public) | ✅ hi | ✅ hi | — | ✅ hi | wrapped |
| ClientPowerPolicies (private) | ✅ hi (0x67F31384) | — | — | ✅ hi (0xAD9A2E6D) | D-notifier + TGP wrapped |
| ClientTgpWatt | — | ✅ hi (0x8B3E7343) | — | ✅ hi (0xAFFC2279) | wrapped (dynamicboost-ppab) |
| ClientThermalPolicies (public) | ✅ hi | ✅ hi | — | ✅ hi | wrapped |
| ClientThermalPolicies (private) | ✅ hi (0x2F69F8E5) | — | — | ✅ hi (0xE097144F) | target-temp wrapped |
| ClockClientClkVfPoints (public) | ✅ hi | ✅ hi | ✅ hi | ✅ hi | V/F boost-table wrapped |
| ClockClkVfPoints (private) | ✅ hi | ✅ hi | ✅ hi | ✅ hi + **reset_vfp_private** | fully wrapped + reset |
| ClockClkDomains (XBar) | ✅ hi | — | ✅ hi | ✅ hi | clk-domain-xbar wrapped |
| VoltVoltRails (melonVolt) | ✅ hi (0x2C73AFDC) | ✅ hi (0x5D0634EE) | ✅ hi (0xA3070DB0) | ✅ hi (0x87C55C8A) | fully wrapped |
| ClientVoltRails (legacy) | ✅ hi | ✅ hi | ✅ hi | ✅ hi (0xB9306D9B) | wrapped |
| PowerMonitor | ✅ hi (0xC12EB19E) | ✅ hi (0xF40238EF) | — | — | per-rail power wrapped |
| ThermChannel | ✅ hi (0x0BC8163D) | ✅ hi (0x65FE3AAD) | — | — | thermal sensors unified |
| PerfLimits (freq cap) | ✅ hi (0xE63AE22B) | ✅ hi (0xEFCEDD1F) | — | ✅ hi (0x32CA4983) | gpuclk wrapped |
| PerfClientLimits | ✅ hi (0xE440B867) | — | — | ✅ hi (0x39442CFB) | wrapped |
| OC Scanner | — | — | — | start/stop/revert/results ✅ | callback FFI ✅ |
| ThermalSim | — | ✅ medium (0xAF97FE75) | — | ✅ medium (0x95E71AB6) + **0x8CD42541 unwrapped** | gap: basic SET |
| PowerMizer | ✅ hi | — | — | ❌ **0x50016C78 unwrapped** | gap: SET |

## 4. Conclusion

**Real actionable gaps (9 IDs)**, priority-ordered:
1. `SetPowerMizerInfo` 0x50016C78 — PowerMizer mode SET (real user lever)
2. `SetThermalSimulationMode` 0x8CD42541 — completes thermal-sim trio
3. `PCF_DynamicBoostGetStatus` 0xC80068A1 — PPAB readback
4. `SetCoreVoltageControl` 0xDC2BD4A6 — core voltage (RE first, may overlap VoltRails)
5. `SetPMGRVoltageRequestArbiterValues` 0x9C4BB8D0 — PMGR voltage (RE first)
6. `PerfRatedTdpGetControl/Info/Status` ×3 — rated-TDP readback (low priority)
7. `ClientEnableBackgroundOcScanner` 0x06DC7CE8 — background scan
8. `GetLastIncompleteOcScannerResults` 0xBE371D0A — incomplete results
9. `PerfVfeEqu`/`PerfVfeVar` ×6 — legacy VFE editor (defer, needs RE)

**No duplicate enum entries** (build-verified — 0 dupes would compile-fail).
**No annotation merges needed** — the 19 YOFOO aliases are correctly
documented as comments on the single RE'd enum variant; the 2
`*ClockBoostLock` alias-comments are annotation-only (no separate entry).
