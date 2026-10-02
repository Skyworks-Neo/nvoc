# VoltVoltRails family — full marshal layout (nvapi64_impl.dll, nvami R610.74)

RE of the four QI handlers behind the private VoltRails family, from
`nvapi64_impl.dll` (DriverStore `nvami.inf_amd64`). Companion to
`oc-gap-layouts-r610-74.md`; corrects the RM cmd guesses recorded there
(0x2080A601/0x2080A613 belong to the *other* — percent — ClientVoltRails
surface, handler `sub_180235F10`).

## Dispatch

Two tables pair the same four IIDs with stub (0x1800DAxxx) and impl handlers;
impl side:

| NvAPI ID | impl handler | role |
|---|---|---|
| 0x2C73AFDC GetInfo | `sub_1801D1420` | fills NV_GPU_VOLT_RAILS_INFO (V1 0x10ACC 2764B / V2 0x2184C 6220B) |
| 0x5D0634EE GetStatus | `sub_1801D1CD0` | V1 0x10AC8 / **V2 0x21620 5664B** |
| 0xA3070DB0 GetControl | `sub_1801D0DB0` | V1 0x10AC8 / V2 0x20AC8 (stamp mask `0xFFFEFFFF`) |
| 0x87C55C8A SetControl | `sub_1801D2450` | same stamps; pre-gate `sub_18038FE40` → -8 |

All four talk to RM through escape **0x07000191** (`sub_180389320`) with a
500,008-byte (0x7A118) request buffer; buffer[12]=gpu, [13]=cmd,
[15]=rail-mask filter (GetInfo sends 0 = all rails).

| op | RM ctrl cmd | RM response record |
|---|---|---|
| GetInfo | **0x2080B201** | stride **76 B**, type byte @+0x44, class byte @+0x77 |
| GetStatus | **0x2080B202** | stride **100 B**, type byte @+88 |
| GetControl | **0x2080B213** | stride **32 B**, type byte @record+0, 6-dword payload @record+4 |
| SetControl | **0x2080F214** | 32 B stride, encoder `sub_18015B6E0` (type 0/2/3 all OK) |

## Rail-type encoder `sub_18015B690`

RM raw byte → exported entry type dword:
`2 → 0, 4 → 2, 5 → 3, 0xFF → -1, other → -2` (never errors).
The exported 0/2/3 space is a per-rail format tag (legacy / intermediate /
Blackwell), matching the existing control-type notes.

## Rail-class encoder `sub_18015B540`

RM raw byte → GetInfo descriptor +80: identity 1..8, anything else → 0 with
status -5. This is the **rail category** that the V1 *status* type field
mirrors (below).

## GetInfo V2 (6220 B) — descriptor slot map (192 B/rail)

Pure marshal from the 76 B RM records; `sub_1801D1420` fills:

| dst off | width | live 4060L | source |
|---|---|---|---|
| +76 | u32 | 0 | `sub_18015B690(src byte 0x44)` — type tag |
| +80 | u32 | 1 | `sub_18015B540(src byte 0x77)` — rail class 1..8 |
| +84 | u8 | 1 | src byte 0x53 |
| +88 | u32 | 750000 | src dword 0x48 — µV (0.75 V) |
| +92 | u16 | 2 | src word 0x4C |
| +94 | u16 | 0xFFFF | src word 0x4E (invalid marker?) |
| +96 | u16 | 8 | src word 0x50 |
| +100 | u32 | 1 | src dword 0x64 |
| +104 | u16 | 7 | src word 0x56 |
| +106 | u16 | 10 | src word 0x58 |
| +110 | u16 | 2 | src byte 0x5E (zero-extended) |
| +112 | u8 | 1 | src byte 0x54 |
| +113 | u8 | 0xFF | **constant** |
| +116 | u32 | 29 | src dword 0x60 |
| +120 | u8 | 16 | src byte 0x5A |
| +124 | u32 | 820000 | src dword 0x6C — µV (0.82 V) |
| +128 | u32 | 959 | src dword 0x68 |
| +132..+139 | per-type tail | 2 / 17 / 16 | see below |

Struct head outside the loop: `mask(+4) |= 1<<bit` (RM dword +0x3C);
`struct byte +8 ← RM byte +0x40`.

Per-type tail at +132..+139:

| exported type | +132 | +136 |
|---|---|---|
| 0 (legacy) | u8 ← src byte 0x7A; u16@+134 ← src word 0x7C | u16 ← src word 0x7E |
| 2 | u32 ← src dword 0x80 | u8 ← min(word src 0x84, 0xFF) saturate |
| 3 | u32 ← src dword 0x7C | u8 ← src byte 0x80 |

Live 4060L type-0 tail: +132=2, +134=17 (0x11), +136=16.

## GetStatus — V2 (5664 B) layout and the V1 back-copy

V2 entry (per **rail bit**, 43-dword = 172 B stride, entry base = struct
+160+172·bit), from 100 B RM records:

| V2 off | source | live 4060L |
|---|---|---|
| +0 type | `sub_18015B690(src byte +88)` | 0 |
| +4 values[0] (current) | src dword +92 | 625000 |
| +8 values[1] (target wall) | src dword +100 | 1005000 |
| +12 values[2] (vbios wall) | src dword +104 | 0 |
| +16 values[3] (vrm max) | src dword +108 | 1200000 |
| +20 values[4] (effective) | src dword +112 | 1005000 |
| +24 values[5] (min hold) | src dword +116 | 625000 |
| +28 values[6] | src dword +120 | 0 |
| +32 values[7] | src dword +124 | 625000 |
| +36 values[8] | src dword +128 | 0 |
| +40 enum | src byte +132 → 0/1/2, else -1 + status -5 | 0 |
| (type==0 only) +48/+52/+56 | src dwords +168/+164/+172 | 810000 / 820000 / 29 |
| (type==0 only) byte +44 | src byte +176 | 1 |

**V1 (2760 B) is a lossy projection** done by `sub_1801C83E0`, which
internally re-runs GetInfo (second RM call) and copies:

| V1 off | source |
|---|---|
| dense +72 "type" | **GetInfo descriptor +80 class (1..8)** — NOT the RM type |
| values[0..5] (+76..99) | V2 values[0..5] |
| +100/+104/+108 | V2 values[6..8] |
| +112 u8 | V2 enum (+40) |
| +116/+120/+124 | V2 +48/+52/+56 (810000 / 820000 / 29), only when type==0 |
| +113 u8 | V2 byte +44 |

This resolves the live paradox that status type=1 while descriptor type=0 on
the same rail: they are different fields (class vs RM type tag).

## GetControl

Accepts V1/V2 (2760 B); dense entries 84 B, type@+72 ← RM byte record+0 via
the type encoder, values[0..5]@+76 ← 24 bytes at record+4. Stock 4060L
returns all-zero payloads (offset 0) with RM type byte = 2 → exported 0.

## Open semantics

Structurally mapped but semantically unconfirmed: GetInfo +84/+92/+96/+100/
+104/+106/+110/+112/+116/+120/+128/+134/+136, GetStatus values[6..8] and
enum/+44, and the µV readings +88 (0.75 V) / +124 (0.82 V) — candidates are
V/F-floor and nominal rail voltage but need an A/B (clock or offset change)
to confirm.
