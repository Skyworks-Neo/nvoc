# nvid.rs OC-Relevance Audit

**Scope**: `nvapi-rs/sys/src/nvid.rs` — 1211 NVAPI IDs (2906 lines), audited 2026-08-25.
**Goal**: enumerate every overclocking-relevant ID, flag which are unwrapped/under-reversed, detect duplicates, and note mergeable comments.

## Summary

| Metric | Count |
|---|---|
| Total registered IDs | 1211 |
| OC-relevant IDs (this audit) | ~180 |
| OC-relevant IDs with FFI wrap / used | ~75 |
| OC-relevant IDs **not wrapped** (candidates) | ~105 |
| True duplicate enum entries | **0** |
| Comment-hex collisions (false-positive "dupes") | 28 (version magics, RM escapes, handler addresses in doc comments — not enum entries) |
| Mergeable comment clusters | 6 |

---

## 1. OC-Relevant ID Catalog

### Wrapped & live (no action needed)

| ID | Name | Function | Category |
|---|---|---|---|
| 0x1bd69f49 | GetAllClocks | All-domain effective clock read (V2) | clock |
| 0xdcb616c3 | GetAllClockFrequencies | 32-domain clock frequencies (V3) | clock |
| 0x64b43a6a | ClockClientClkDomainsGetInfo | Clock domain ranges | clock |
| 0x507b4b59 | ClockClientClkVfPointsGetInfo | Public V/F boost mask | clock/vf |
| 0x23f1b133 | ClockClientClkVfPointsGetControl | Public V/F boost table GET | clock/vf |
| 0x0733e009 | ClockClientClkVfPointsSetControl | Public V/F boost table SET | clock/vf |
| 0x21537ad4 | ClockClientClkVfPointsGetStatus | Public V/F curve read | clock/vf |
| 0x8895b510 | ClockClkVfPointsGetInfo | Private V/F point masks | clock/vf |
| 0xda025c3e | ClockClkVfPointsGetControl | Private V/F snapshot GET | clock/vf |
| 0xfec00d04 | ClockClkVfPointsSetControl | Private V/F SET (reset_vfp_private) | clock/vf |
| 0x7fee9032 | ClockClkVfPointsGetStatus | Private V/F records | clock/vf |
| 0x57b5a5df | ClockClkDomainsGetInfo | XBar clock-domain info | clock |
| 0xf58938f5 | ClockClkDomainsGetControl | XBar clock-domain control | clock |
| 0xd14b69cf | ClockClkDomainsSetControl | XBar clock-domain SET | clock |
| 0xfb8f61ec | ClockCounterMeasureAvgFreq | Counter-based avg freq measure | clock |
| 0x527fc458 | ClockClkDomainsMeasureFreq | Direct per-domain kHz measure | clock |
| 0x927da4f6 | GetCurrentPstate | Current P-state | pstate |
| 0x6ff81213 | GetPstates20 | P-states20 table GET | pstate |
| 0x0f4dae6b | SetPstates20 | P-states20 table SET (offset apply) | pstate |
| 0x7b30ae0d | PerfPstatesGetInfoPrivate | P-state info (native get) | pstate |
| 0x025bfb10 | SetForcePstate | Force P-state | pstate |
| 0x39442cfb | PerfClientLimitsSetStatus | P-state lock SET (offset) | pstate/perflimit |
| 0xe440b867 | PerfClientLimitsGetStatus | P-state lock status | pstate/perflimit |
| 0xe63ae22b | PerfLimitsGetInfo | Perf limit info | perflimit |
| 0xefcedd1f | PerfLimitsGetStatus | Perf limit status | perflimit |
| 0x32ca4983 | PerfLimitsSetStatus | GPU freq perf-cap SET | perflimit |
| 0xfa579a0f | EnableDynamicPstates | Enable dynamic P-states (unlock) | pstate |
| 0xe3640a56 | GetThermalSettings | Thermal sensor readings | thermal |
| 0x0bc8163d | ThermChannelGetInfo | Thermal channel topology (hotspot) | thermal |
| 0x65fe3aad | ThermChannelGetStatus | Thermal channel readings | thermal |
| 0xc4554575 | ClientThermalTargetGetStatus | Target temp GET (mobile wall) | thermal |
| 0xe097144f | ClientThermalTargetSetStatus | Target temp SET | thermal |
| 0x0d258bb5 | ClientThermalPoliciesGetInfo | Thermal policy info | thermal |
| 0xe9c425a1 | ClientThermalPoliciesGetStatus | Thermal policy status | thermal |
| 0x34c0b13d | ClientThermalPoliciesSetStatus | Thermal policy SET | thermal |
| 0x2f69f8e5 | ClientThermalPoliciesPrivateGetInfo | Private thermal policy info (target temp) | thermal |
| 0x76bfa16b | GetPowerMizerInfo | PowerMizer state | power |
| 0xc12eb19e | PowerMonitorGetInfo | Per-rail power descriptor | power |
| 0xf40238ef | PowerMonitorGetStatus | Per-rail power mW | power |
| 0xa4dfd3f2 | ClientPowerTopologyGetInfo | Power topology info | power |
| 0xedcf624e | ClientPowerTopologyGetStatus | Power topology status | power |
| 0x34206d86 | ClientPowerPoliciesGetInfo | Power policy info | power |
| 0x70916171 | ClientPowerPoliciesGetStatus | Power policy status | power |
| 0xad95f5ed | ClientPowerPoliciesSetStatus | Power policy SET | power |
| 0x8b3e7343 | ClientTgpWattGetStatus | TGP watt GET | power |
| 0xaffc2279 | ClientTgpWattSetStatus | TGP watt SET | power |
| 0x1504fc3d | ClientDynamicBoostSetStatus | PPAB / Dynamic-Boost enable | power |
| 0x48e0847d | ClientExternPowerStateSet | D-Notifier SET | power |
| 0x67f31384 | ClientPowerPoliciesGetInfoPrivate | D-Notifier/TGP policy GET | power |
| 0xad9a2e6d | ClientPowerPoliciesSetInfoPrivate | TGP cap SET (VelocityX) | power |
| 0xf21c2d56 | ClientPowerModesGetInfo | Power mode info (Balanced/Max) | power |
| 0x180a9468 | ClientPowerModesGetControl | Power mode control | power |
| 0x3cc8c552 | ClientPowerModesSetControl | Power mode SET | power |
| 0xdb9ed906 | PowerDeviceGetInfo | Power sensor info (32-ch) | power |
| 0x465f9bcf | ClientVoltRailsGetStatus | Voltage rail status | voltage |
| 0x9df23ca1 | ClientVoltRailsGetControl | Voltage boost percent GET | voltage |
| 0xb9306d9b | ClientVoltRailsSetControl | Voltage boost percent SET | voltage |
| 0x2c73afdc | VoltVoltRailsGetInfo | Voltage rail builder info | voltage |
| 0x5d0634ee | VoltVoltRailsGetStatus | Voltage rail data (µV) | voltage |
| 0xa3070db0 | VoltVoltRailsGetControl | Voltage rail offset GET | voltage |
| 0x87c55c8a | VoltVoltRailsSetControl | Voltage rail offset SET | voltage |
| 0xc16c7e2c | GetVoltageDomainsStatus | Voltage domains status | voltage |
| 0x28766157 | GetVoltageStep | Voltage step | voltage |
| 0xda141340 | GetCoolerSettings | Cooler settings GET | cooler/fan |
| 0x891fa0ae | SetCoolerLevels | Cooler levels SET (fan speed) | cooler/fan |
| 0xe543c540 | ClientFanPoliciesGetControl | Fan curve GET | fan |
| 0x52b76d12 | ClientFanPoliciesGetInfo | Fan policy info | fan |
| 0xc181947a | ClientFanPoliciesSetControl | Fan curve SET | fan |
| 0x0fe87b7f | FanPolicyGetControl | Fan policy reset GET | fan |
| 0x2b2a2a45 | FanPolicySetControl | Fan policy reset SET | fan |
| 0x600f612e | ClientFanArbitersGetControl | Fan arbiter control | fan |
| 0xdddfda38 | ClientFanArbitersGetInfo | Fan arbiter info | fan |
| 0xcde021b9 | ClientFanArbitersGetStatus | Fan arbiter status | fan |
| 0x44cd3014 | ClientFanArbitersSetControl | Fan stop SET | fan |
| 0x814b209f | ClientFanCoolersGetControl | Fan cooler control | fan |
| 0xfb85b01e | ClientFanCoolersGetInfo | Fan cooler info | fan |
| 0x35aed5e8 | ClientFanCoolersGetStatus | Fan cooler status | fan |
| 0xa58971a5 | ClientFanCoolersSetControl | Fan cooler SET | fan |
| 0xbc4aee25 | ClientStartOcScanner | OC Scanner start | ocscanner |
| 0xc28b73de | ClientStopOcScanner | OC Scanner stop | ocscanner |
| 0xcc727b22 | ClientRevertOc | OC Scanner revert | ocscanner |
| 0x593e8e72 | ClientGetLastOcScannerResults | OC Scanner results | ocscanner |
| 0x1cb41116 | ClientRegisterForOcScannerStatusUpdates | OC Scanner callback | ocscanner |
| 0x5f608315 | GetTachReading | Fan tachometer RPM | fan |
| 0xbd71f0c9 | GetCurrentFanSpeedLevel | Current fan speed level | fan |
| 0xd2488b79 | GetCurrentThermalLevel | Current thermal level | thermal |
| 0x55590cb2 | ForceGC6Exit | GC6 wake | power/thermal |
| 0xd387d414 | GC6Control | GC6 control (wake) | power/thermal |

### OC-relevant but NOT wrapped (candidates)

#### Clock — 6 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0x1ea54a3b | GetPerfClocks | Perf clock read | uncertain; possibly legacy predecessor of GetAllClocks |
| 0x07bcf4ac | SetPerfClocks | Perf clock SET | uncertain; possibly legacy clock-set (pre-Pstates20) |
| 0x6f151055 | **SetClocks** | Set clocks | **legacy direct clock set** (Pascal-era, predates Pstates20 offset path); may still work on older GPUs |
| 0x1b46d4cc | GetPublicClockInfo | Public clock info | YOFOO; uncertain semantics |
| 0x40bddb36 | ClockClkDomainFreqsEnum | Enumerate clock domain frequencies | may complement ClockClkDomainsGetFreqInfo |
| 0xd2fc1b34 | ClockClkDomainsGetFreqInfo | Clock domain freq info | pair with above; may be richer than GetAllClockFrequencies |

#### P-state / Perf — 14 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0xba94c56e | **GetPstatesInfo** | P-states info (legacy V1) | pre-GetPstates20; the legacy table Green-Curve reads |
| 0x843c0256 | GetPstatesInfoEx | P-states info Ex | extended variant of above |
| 0xcdf27911 | SetPstatesInfo | P-states info SET | legacy counterpart |
| 0x3b0d30df | GetPstatesEx | P-states Ex | uncertain; may duplicate GetPstatesInfoEx |
| 0x4af0011d | GetPstateLimitsInfo | P-state limits info | uncertain; may overlap PerfClientLimits |
| 0x825ddf13 | SetPstates | Set P-states | legacy (pre-Pstates20) |
| 0xa69f8e29 | GetPstates | Get P-states | legacy |
| 0x4c0b519a | SetPstates20Private | P-states20 private SET | uncertain; may be a privileged variant of SetPstates20 |
| 0xc5ddf56e | GetPstates20Private | P-states20 private GET | pair with above |
| 0xe7b1198d | SetForcePstateEx | Force P-state Ex | extended force-pstate; nvoc uses 0x025bfb10 |
| 0x03caeb65 | PerfPstatesGetStatus | Perf P-states status | YOFOO nvPerf family — **may be the documented P-state status** nvoc lacks |
| 0x0f03dc87 | PerfPstatesSetControl | Perf P-states control | YOFOO; **documented P-state setter** nvoc may want |
| 0x2bc18dbd | PerfPstatesGetControl | Perf P-states control GET | pair with above |
| 0x75dd3e6a | SetPerfLevel | Set perf level | uncertain; may overlap EnableDynamicPstates |

#### Perf limits / Vf-tables / Vfe — 12 unwrapped (nvPerf family, V/F curve edit surface)

| ID | Name | Function | Note |
|---|---|---|---|
| 0x0b62b9e2 | PerfPerfLimitsGetStatus | Perf limits status (nvPerf) | **may duplicate PerfClientLimitsGetStatus 0xe440b867** — needs disambiguation |
| 0x8159b63f | PerfPerfLimitsSetControl | Perf limits SET (nvPerf) | **may duplicate PerfClientLimitsSetStatus 0x39442cfb** |
| 0x4d2c0a9c | PerfPerfLimitsGetInfo | Perf limits info (nvPerf) | **may duplicate PerfLimitsGetInfo 0xe63ae22b** |
| 0xa59be705 | PerfPerfLimitsGetControl | Perf limits control GET | uncertain |
| 0x3f475f9b | **PerfVfTablesGetInfo** | V/F tables info | **V/F curve table surface** — may be a 3rd V/F family (distinct from ClockClient/ClockClk) |
| 0x4150ff5c | PerfVpstatesGetControl | Vpstates GET | V/F pstate control; uncertain |
| 0x6592ae66 | PerfVpstatesSetControl | Vpstates SET | pair with above |
| 0x4c75c9fe | **PerfVfeEquGetControl** | VFE equation GET | **V/F equation edit** — advanced curve manipulation |
| 0x68b798c4 | PerfVfeEquSetControl | VFE equation SET | pair with above |
| 0x5d387298 | PerfVfeVarGetControl | VFE variable GET | V/F variable control |
| 0x79fa23a2 | PerfVfeVarSetControl | VFE variable SET | pair with above |
| 0x8d49471c | PerfVfeEquGetInfo | VFE equation info | descriptor for above |
| 0xb9da41d6 | PerfVfeVarGetInfo | VFE variable info | descriptor for above |

#### Rated TDP — 3 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0x87bd35ef | PerfRatedTdpGetInfo | Rated TDP info | **rated TDP** (nominal power baseline) — nvoc has ClientRatedTdpControl 0xc9e9bb33 SET but no GET |
| 0xfcbdf642 | PerfRatedTdpGetStatus | Rated TDP status | pair with above |
| 0xed2bea09 | PerfRatedTdpGetControl | Rated TDP control GET | the GET nvoc is missing for 0xc9e9bb33 |

#### Thermal — 12 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0x14277c24 | ThermalHwFsGetInfo | HW FS slowdown info | **HW slowdown cap** — complement to ThermChannel |
| 0xcbc9361b | ThermalHwFsSetInfo | HW FS slowdown SET | **adjust HW slowdown threshold** |
| 0x661aa3af | ThermHwFsSlowdownAmountGet | Slowdown amount read | (gpumon uses this; not wrapped) |
| 0x6683ee65 | GetThermalSlowdownState | Thermal slowdown state | pair with SetThermalSlowdownState |
| 0x1b71d425 | SetThermalSlowdownState | Thermal slowdown SET | (gpumon uses; enable/disable 0xFFFF) |
| 0x1b4f669b | ThermalPolicyGetStatus | Thermal policy status (nvThermal) | **may duplicate ClientThermalPoliciesGetStatus 0xe9c425a1** |
| 0x4b4bd039 | ThermMonitorsGetStatus | Thermal monitors status | **may duplicate GetThermalSettings 0xe3640a56** |
| 0xb2c9d666 | ThermMonitorsGetInfo | Thermal monitors info | pair with above |
| 0x6ff0350c | ThermDeviceGetInfo | Thermal device info | descriptor |
| 0x8cd42541 | SetThermalSimulationMode | Thermal simulation SET (VBIOS override) | **temp simulation** — Thermspy gap |
| 0xaf97fe75 | GetThermalSimulationMode | Thermal simulation GET | pair with above |
| 0x95e71ab6 | SetExtendedThermalSimulationMode | Extended thermal sim SET | extended variant |
| 0x8df19fa2 | ThermChannelSetControl | Thermal channel SET | **adjust thermal thresholds per-channel** |
| 0xa933ce98 | ThermChannelGetControl | Thermal channel control GET | pair with above |

#### Voltage — 13 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0x00d57b3b | VoltVoltPoliciesGetInfo | Volt policies info | **voltage policy table** |
| 0x33d32759 | VoltVoltPoliciesGetControl | Volt policies GET | pair with above |
| 0x17117663 | VoltVoltPoliciesSetControl | Volt policies SET | **adjust voltage policy** |
| 0x8d877b8f | VoltVoltPoliciesGetStatus | Volt policies status | pair with above |
| 0x02533065 | VoltVoltDevicesGetControl | Volt devices GET | per-device voltage |
| 0x2691615f | VoltVoltDevicesSetControl | Volt devices SET | pair with above |
| 0xa38acf9d | VoltVoltDevicesGetInfo | Volt devices info | descriptor |
| 0x58337fa3 | GetCoreVoltage | Core voltage read | **direct core voltage** |
| 0xa91f88eb | GetCoreVoltageControl | Core voltage control GET | uncertain |
| 0xdc2bd4a6 | SetCoreVoltageControl | Core voltage control SET | **set core voltage** |
| 0x1785b492 | GetVoltagesInternal | Internal voltages | uncertain |
| 0x54f67bbf | VoltPmumonVoltRailsGetSamples | Volt rail samples (pmumon) | **per-rail voltage history** — sampling variant |
| 0x717648fd | GetPMGRVoltageRequestArbiterValues | PMGR voltage arbiter GET | uncertain; power-management-voltage |
| 0x9c4bb8d0 | SetPMGRVoltageRequestArbiterValues | PMGR voltage arbiter SET | pair with above |

#### Fan / Cooler — 9 unwrapped

| ID | Name | Function | Note |
|---|---|---|---|
| 0x15b85505 | FanPolicyGetStatus | Fan policy status (nvCooler) | **may duplicate ClientFanPolicies family** |
| 0x76a38d54 | FanPolicyGetInfo | Fan policy info (nvCooler) | pair with above |
| 0x10741a55 | FanArbiterGetInfo | Fan arbiter info (gpumon) | (gpumon uses; not wrapped) |
| 0x0956ab25 | FanArbiterGetStatus | Fan arbiter status (gpumon) | (gpumon uses; not wrapped) |
| 0x41716ac2 | FanPmumonFanCoolersGetSamples | Fan cooler samples (pmumon) | **fan RPM history** — sampling variant |
| 0x98a4411a | FanTestGetInfo | Fan test info | diagnostic |
| 0xb699f73a | SetPmuFanControlBlock | PMU fan control SET | uncertain |
| 0xc3adab77 | GetPmuFanControlBlock | PMU fan control GET | pair with above |
| 0xcf6cef26 | ClientFanPoliciesGetStatus | Fan policies status | **may duplicate ClientFanPoliciesGetControl 0xe543c540** |
| 0xfd871348 | QueryFanSpinSenseSupport | Fan spin sense support | diagnostic |
| 0x65ce5bfc | FanCoolerGetInfo | Fan cooler info (gpumon) | (gpumon uses; not wrapped) |
| 0x3cc2d181 | FanCoolerGetStatus | Fan cooler status (gpumon) | (gpumon uses; not wrapped) |
| 0xcf86b990 | FanCoolerGetControl | Fan cooler control GET (gpumon) | (gpumon uses; not wrapped) |
| 0xeb44e8aa | FanCoolerSetControl | Fan cooler SET (gpumon) | (gpumon uses; not wrapped) |

#### Power management / residency (low-power, not OC-tuning but OC-adjacent)

| ID | Name | Function | Note |
|---|---|---|---|
| 0x50016c78 | **SetPowerMizerInfo** | PowerMizer SET | **PowerMizer mode set** — nvoc has GET (0x76bfa16b) but not SET |
| 0x60ded2ed | **GetDynamicPStatesInfoEx** | Dynamic P-states info | **per-domain utilization+clock** (the documented GetUsages sibling) — nvoc uses GetUsages 0x189a1fdf instead; this may carry richer P-state-specific data |
| 0xc9e9bb33 | ClientRatedTdpControl | Rated TDP SET | wrapped (SET only); GET is PerfRatedTdpGetControl above |

#### Miscellaneous (OC lifecycle / Qboost — kept for record, NOT candidates)

| ID | Name | Note |
|---|---|---|
| 0xad298d3f | PrivateLifecycleInit | init gate; wrapped in core init path |
| 0xb4c5d8ba | ClientQboostGetInfo | Qboost — ruled out (wrong PPAB path) |
| 0xb78734ab | ClientQboostSetStatus | Qboost — ruled out |
| 0xb6a3da5b | PCF_SysPwrLimitGetInfo | SBIOS power-limit-table GET (read-only) |
| 0xd7c61344 | Unknown_D7C61344_InternalUnload | teardown-only, do not wrap |

---

## 2. Duplicate / Version-Family Analysis

**True duplicate enum entries: 0.** Every hex ID is registered to exactly one enum variant. The 28 "collisions" from a naive `uniq -d` over all hex literals are **false positives** — they are hex values appearing in *doc comments* (version magics like `0x00010008`, RM escapes like `0x07000191`, IDA handler addresses like `0x18025766`), not duplicate `Name = 0x...` enum lines.

### YOFOO alias pairs (same ID, two names — documented, intentional)

These are single enum entries with a doc comment noting a YOFOO-table alias. Not duplicates, but the comment cluster is worth consolidating:

| ID | RE'd name | YOFOO alias | Line |
|---|---|---|---|
| 0x189a1fdf | GetUsages | GetDynamicPStatesInfo | 807 |
| 0x42aea16a | GetRamMaker | GetRamVendorID | 810 |
| 0x28766157 | GetVoltageStep | GetVoltageDomainsInfo | 747 |
| 0xa4218928 | GetManufacturingInfo | ManufacturingInfo | 702 |
| 0x7b30ae0d | PerfPstatesGetInfoPrivate | PerfPstatesGetInfo | 1387 |
| 0x1504fc3d | ClientDynamicBoostSetStatus | PCF_DynamicBoostSetStatus | 1292 |
| 0x48e0847d | ClientExternPowerStateSet | SetExternPowerState | 1338 |
| 0x67f31384 | ClientPowerPoliciesGetInfoPrivate | PowerPolicyGetInfo | 1364 |
| 0x8b3e7343 | ClientTgpWattGetStatus | PowerPolicyGetControl | 1396 |
| 0xaffc2279 | ClientTgpWattSetStatus | PowerPolicySetControl | 1454 |
| 0x9962c97c | ClientPStateLimitStatus | GetPstateActiveLimits | 1413 |
| 0xc9e9bb33 | ClientRatedTdpControl | PerfRatedTdpSetControl | 1559 |
| 0xb4c5d8ba | ClientQboostGetInfo | PCF_ControllerGetInfo | 1432 |
| 0xb78734ab | ClientQboostSetStatus | PCF_ControllerSetControl | 1443 |
| 0xc4554575 | ClientThermalTargetGetStatus | ThermalPolicyGetControl | 1467 |
| 0xe097144f | ClientThermalTargetSetStatus | ThermalPolicySetControl | 1569 |
| 0x2f69f8e5 | ClientThermalPoliciesPrivateGetInfo | ThermalPolicyGetInfo | 1306 |
| 0xb7bcf50d | GetComputeCapabilities | QueryComputeCaps | 1087 |
| 0xcb6a5d5a | Initialize_CB6A5D5A | Initialize | 1619 |

### TGP-watt SET sibling pair (different IDs, same role)

`0xaffc2279` (CLI variant, live) vs `0xbff09e59` (GUI variant, NULL on 4060L) — already documented at lines 1445-1461. This is the only true "two IDs, one function" case, and it's already explained.

---

## 3. Comment Merge Opportunities

### Cluster A — "may duplicate" cross-family pairs (highest priority to disambiguate)

Several nvPerf/nvThermal/nvCooler-spec IDs are **likely the documented-spec siblings** of already-wrapped Client-family IDs, but nvid.rs has no comment cross-referencing them. These should get a one-line "see also" note once disambiguated via IDA:

- `PerfPerfLimitsGetStatus 0x0b62b9e2` ↔ `PerfClientLimitsGetStatus 0xe440b867` (line 2498 vs 771)
- `PerfPerfLimitsSetControl 0x8159b63f` ↔ `PerfClientLimitsSetStatus 0x39442cfb` (line 2544 vs 772)
- `PerfPerfLimitsGetInfo 0x4d2c0a9c` ↔ `PerfLimitsGetInfo 0xe63ae22b` (line 2523 vs 1571)
- `ThermalPolicyGetStatus 0x1b4f669b` ↔ `ClientThermalPoliciesGetStatus 0xe9c425a1` (line 1838 vs 743)
- `ThermMonitorsGetStatus 0x4b4bd039` ↔ `GetThermalSettings 0xe3640a56` (line 1839 vs 301)
- `FanPolicyGetStatus 0x15b85505` ↔ `ClientFanPoliciesGetControl 0xe543c540` (line 1879 vs 788)
- `ClientFanPoliciesGetStatus 0xcf6cef26` ↔ `ClientFanPoliciesGetControl 0xe543c540` (line 1885 vs 788)

### Cluster B — PerfRatedTdp family (split across two spec blocks)

The Rated-TDP family is split: the SET (`ClientRatedTdpControl 0xc9e9bb33`, gpumon block line 1560) is in the gpumon section, while the GET/Info/Status triplet (`PerfRatedTdpGetInfo/GetStatus/GetControl`, nvPerf spec lines 2545/2574/2565) is 1000 lines away. A single "Rated TDP family" comment block grouping all 4 would aid discovery.

### Cluster C — VFE/VF-table family (undocumented V/F edit surface)

`PerfVfTablesGetInfo`, `PerfVpstatesGetControl/SetControl`, `PerfVfeEquGetControl/SetControl/GetInfo`, `PerfVfeVarGetControl/SetControl/GetInfo` (nvPerf spec lines 2517-2573) form a **9-ID V/F equation/table edit family** with zero documentation. This is the most significant unwrapped OC surface. A consolidated comment block explaining "this is a 3rd V/F family (nvPerf-spec), distinct from ClockClient (public) and ClockClk (private)" would prevent future confusion.

### Cluster D — Thermal simulation trio

`SetThermalSimulationMode 0x8cd42541`, `GetThermalSimulationMode 0xaf97fe75`, `SetExtendedThermalSimulationMode 0x95e71ab6` (lines 1842/1422/1403) — the Thermspy gap. Comments are scattered; a "thermal simulation family" note grouping these three (plus noting 0x95e71ab6 is the extended variant of 0x8cd42541) would help.

### Cluster E — ClockClkDomains vs ClockClkDomainFreqsEnum/GetFreqInfo

`ClockClkDomainsGetInfo/GetControl/SetControl` (wrapped, XBar family) vs `ClockClkDomainFreqsEnum 0x40bddb36` + `ClockClkDomainsGetFreqInfo 0xd2fc1b34` (unwrapped, nvClocks spec lines 1774/1799) — likely a related freq-info pair. No comment links them.

### Cluster F — VoltVoltRails "private vs public" already noted but split

The VoltVoltRails family (lines 1859-1873) is well-commented individually, but the relationship to the already-wrapped `ClientVoltRailsGetStatus 0x465f9bcf` (public, line 745) is only implicit. A "the ClientVoltRails (public, % boost) vs VoltVoltRails (private, µV offset) distinction" header would consolidate the 6 VoltVolt IDs.

---

## 4. Priority Recommendations

**High value, likely actionable:**
1. `PerfRatedTdpGetControl 0xed2bea09` — the missing GET for the already-wrapped Rated-TDP SET (`ClientRatedTdpControl 0xc9e9bb33`). Completes a write/readback pair.
2. `GetDynamicPStatesInfoEx 0x60ded2ed` — documented per-domain P-state+utilization; may be richer than the GetUsages nvoc currently uses.
3. `SetPowerMizerInfo 0x50016c78` — PowerMizer SET (nvoc has GET only); the PowerMizer mode toggle.
4. `PerfVfTablesGetInfo 0x3f475f9b` + VFE equation family — a **3rd V/F curve edit surface** not yet explored; could enable direct V/F equation manipulation beyond offset tables.

**Medium value, needs IDA disambiguation first:**
5. The 7 "may duplicate" nvPerf/nvThermal/nvCooler pairs (Cluster A) — one IDA session per family to determine if they're truly distinct from the wrapped Client variants.
6. `SetCoreVoltageControl 0xdc2bd4a6` + `GetCoreVoltage 0x58337fa3` — direct core voltage set/get (may be the MSI Afterburner voltage-control path).

**Low value / likely skip:**
7. Legacy Pstates family (GetPstatesInfo/SetPstates/SetPstates20Private) — superseded by Pstates20 which nvoc already wraps.
8. PMU/Pmumon sample variants — sampling history, not real-time OC control.
9. Qboost family — already ruled out as wrong PPAB path.
