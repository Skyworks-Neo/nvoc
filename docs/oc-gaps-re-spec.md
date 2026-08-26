# OC-Gap Wrap RE Spec (R610.74, nvapi64_impl.dll)

RE'd 2026-08-26 via IDA (QI dispatch table @ 0x1804DE000). Feeds the 9-gap wrap effort.

## Group A (signatures)

### PowerMizer pair — escape 0x700003A, 0x48B private struct
- **GetPowerMizerInfo 0x76BFA16B** @0x1802392A0: `fn(hGpu, powerSource: u32∈{1,2}, queryType: u32=3, *outMode: u32)`. Output mode ∈ {6,7} only (internal 0→6, 1→7). a3 must be 3.
- **SetPowerMizerInfo 0x50016C78** @0x180261BC0: `fn(hGpu, powerSource: u32, queryType: u32=3, mode: u32∈{6,7})` — mode by value, maps 6→0/7→1 internally. op field 2 (vs 3 for get).
- ⚠️ Existing sys decl `fn(hGpu, *mut u32)` is WRONG (2-arg vs actual 4-arg) — fix power.rs:736 + medium power_mizer_info (gpu.rs:3784).

### PCF_DynamicBoostGetStatus 0xC80068A1 @0x180069CC0
`fn(active: *mut bool/u8)` — single arg, writes 1 byte. `*active = (statusByte != 2 && statusByte2 != 2)`. No GPU handle (like the SET 0x1504FC3D `fn(active: BoolU32)`).

### PerfRatedTdp GET trio — RM cmd 0x7000048, 0x81868 workbuf, hGpu@buf+0x30, sub-cmd@buf+0x34
- **GetControl 0xED2BEA09** @0x1802A90F0: 12B struct ver **0x1000C** `{+0 ver, +4 mode IN, +8 mode OUT}` — same struct as SET 0xC9E9BB33. Sub-cmd 0x207E004E.
- **GetInfo 0x87BD35EF** @0x1802A93D0: 8B struct ver **0x10008** `{+0 ver, +4 u8 out}`. Sub-cmd 0x207F000C.
- **GetStatus 0xFCBDF642** @0x1802A96A0: 36B struct ver **0x10024**. Fills +4 u32(buf+0x38), +8 u8(buf+0x3C), +12 u32(buf+0x40 decoded), five mode dwords from buf+0x48 array → +16,+32,+20,+24,+28 (values 0-4). Sub-cmd 0x207F000D(545300589).

### SetThermalSimulationMode 0x8CD42541 @0x1801DFFE0
CONFIRMED 4-arg `(hGpu: u64, flags: u32, enable: u32, temperature: i32)` — identical shape to Extended 0x95E71AB6. Validates temp ≤ 0xFF. Existing FFI decl correct.

### OC Scanner completeness
- **ClientEnableBackgroundOcScanner 0x06DC7CE8** @0x1800717C0: `fn(hGpu, struct*)` ver **0x10048** (72B). Reads enable byte @+4, GUID/feature-tag 9 bytes @+10..21 = `0B 0A 0E 08 E8 72 9D D9 F3`. RPC cmd 7.
- **GetLastIncompleteOcScannerResults 0xBE371D0A** @0x180073550: `fn(hGpu, struct*)` ver **0x10044** (68B, same magic as NV_GPU_OC_SCANNER_CONTROL). GUID @+9..20 (same bytes). RPC cmd 13. RPC 2→-104, 4→-191.

## Group B (verdicts: all WRAP — distinct surfaces)

### B1 Core voltage — escape 0x07000043(GET)/44(SET)/45(GETControl), 56B buffer
- **GetCoreVoltage 0x58337FA3** @0x1801C9CE0: `fn(selector: u32, *value: u32)` — selector@esc+0x28, value from esc+0x34. flag arg5=1.
- **GetCoreVoltageControl 0xA91F88EB** @0x1801C9E30: `fn(u32, *u32)` same shape, arg5=0.
- **SetCoreVoltageControl 0xDC2BD4A6** @0x1801CB300: `fn(a1: u32, a2: u32)` — a1@esc+0x28, a2 packed. Gated by sub_18038FE40() → -104.
- Plain scalars, no version magic. Distinct from VoltVoltRails (0x07000191/0x2080A613).

### B2 PMGR voltage arbiter — escape 0x0700019F, 112B heap buffer
- **GetPMGRVoltageRequestArbiterValues 0x717648FD** @0x1801C9F80 (get flag=0), **SetPMGRVoltageRequestArbiterValues 0x9C4BB8D0** @0x1801CB480 (flag=1).
- `fn(gpuSelector: u32, pVals: *struct)`. Version magics **0x10024**(v1, 8 payload dwords) / **0x20030**(v2, +3 dwords).
- Escape map: gpuSel@+0x30, get/set flag@+0x34; dwords[1..4]@esc+0x38..0x44, [5..8]@+0x50..0x5C; v2 [9..10]@+0x48..0x4C, [11]@+0x60. Get copies back esc 56..96.

### B3 VFE family — escape 0x070001C6, 0x100440-byte buffer, RM ctrl cmd in dword[13], hDomain in dword[12]
All 6: `fn(hDomain: u32, versioned-struct*)`.
| ID | Handler | RM cmd |
|---|---|---|
| PerfVfeEquGetControl 0x4C75C9FE | 0x1802AA9C0 | 0x2080A0B6 |
| PerfVfeEquSetControl 0x68B798C4 | 0x1802ABD90 | 0x2080E0B7 |
| PerfVfeEquGetInfo 0x8D49471C | 0x1802AB410 | 0x2080A0B5 |
| PerfVfeVarGetControl 0x5D387298 | 0x1802AC850 | 0x2080A0B3 |
| PerfVfeVarSetControl 0x79FA23A2 | 0x1802AE0C0 | 0x2080E0B0 |
| PerfVfeVarGetInfo 0xB9DA41D6 | 0x1802AD1E0 | 0x2080A0B1 |
- **Equ**: 256 entries × 512B @escape+0x3C, selected by two 0x2000-bit masks @dword[15]/dword[15+64]. Deep-copy versions: 0x14C28/0x33064/0x36174/0x56164/0x1584C4. Entry type tags 1/2/3/6/7. GetInfo struct ver 0xD8444 (885828), size 0x98444, 256×76B info entries (type enum 0-14).
- **Var**: versions 0x10ACC(68300)/0x29FF8(171976). GetControl: 32B header (esc+0x3C..0x5B) + 255 entries × 44B (@esc+0x80) → user entries 160B. Type tags 2/3/5/7/8/9/10/11/13/15/18.
- Sets gated by sub_18038FE40() → -104 (needs elevation).
- **Distinct from public VfPoints (0x07000049) and private VfPoints (0x2080906x) — 3rd V/F edit surface.**

## Error codes
-4 not-init, -5 invalid-arg, -9 bad-version, -14 NULL ptr, -101 NULL handle, -130 alloc, -152/-184 unsupported.
