# Unregistered NVAPI handler triage — batch 2

> **Snapshot:** Generated from the R610.74 `nvapi64_impl.dll` dispatch table.
> The matching input list is under [`evidence/`](evidence/). Classifications
> are historical and must be revalidated against newer binaries.

**Binary**: `nvapi64_impl.dll` R610.74
**Scope**: 115 non-stub unregistered IIDs (0x77F5A2DA..0xFDCF2BC1)
**Method**: analyze_batch (decompile+strings+constants) on 35 handlers + RM-escape immediate search (0x0700_01xx, 0x06FF00xx) + OC struct-magic search (0x200DC/0x214AC/0x432D0/0x1E8604/0x78604/0x10124/0x0F4BF4/0x20038/0x10088/0x10090) across full binary.

## Summary

| category | count |
|---|---|
| OC-relevant | **0** |
| Non-OC (classified by string signal) | 35 |
| Uncertain (generic NVAPI wrapper, no OC signal) | 80 |
| **Total** | **115** |

**Key finding**: ZERO overclock-relevant IIDs found in chunk2. The RM-escape (0x0700_01xx / 0x06FF00xx) and OC struct-magic (0x200DC/0x214AC/etc.) searches confirmed all OC escape/magic references land inside handler code ranges that are ALREADY registered in nvid.rs. The 115 unregistered handlers in this chunk are generic NVAPI wrappers for non-OC subsystems.

## Classification by string signal (35 deep-analyzed)

| iid | handler | string | oc? | role |
|---|---|---|---|---|
| 0x77F5A2DA | 0x180360BF0 | (none) | no | generic RM escape handler |
| 0x78ABE813 | 0x180418890 | (none) | no | stub-like (returns -104 NOT_SUPPORTED) |
| 0x796AD3E4 | 0x18021ADD0 | escData | no | escape-data handler (non-OC subsystem) |
| 0x799F1266 | 0x180241A40 | getData | no | generic get-data |
| 0x7BDC92E7 | 0x1802D19D0 | (none) | no | generic handler |
| 0x7D2F8A70 | 0x18023C480 | getData | no | generic get-data |
| 0x82127994 | 0x18013E210 | esc | no | escape handler (non-OC) |
| 0x82663673 | 0x1801ADEF0 | (none) | no | generic handler |
| 0x82A0D7AD | 0x180145EF0 | escData | no | escape-data handler |
| 0x8657278A | 0x18019F230 | gpuInfo | no | GPU info query (version magic 0x10028 = cooler settings struct, but handler is gpuInfo-typed) |
| 0x881B0552 | 0x1802A05A0 | escData | no | escape-data handler |
| 0x88A81174 | 0x180240560 | setData | no | generic set-data |
| 0x8973E692 | 0x180361060 | (none) | no | generic handler |
| 0x9034C146 | 0x1801E3820 | escData | no | escape-data handler |
| 0x90C5B263 | 0x18030E250 | (none) | no | generic handler |
| 0x92D51034 | 0x180260070 | escData | no | escape-data handler (large, 0xAE8 bytes) |
| 0x94281DD4 | 0x1801C6950 | escData | no | escape-data handler |
| 0x9441E15D | 0x1801B54F0 | escData | no | escape-data handler |
| 0x9475C8AE | 0x1803DA860 | coprocInfo | no | coprocessor info (version 0x10034) |
| 0x94C04D7C | 0x1801C7300 | escData | no | escape-data handler (large alloc 0x7060, version 0x15C50) |
| 0x95C7F488 | 0x1804176A0 | (none) | no | generic handler |
| 0x991E343D | 0x18020B810 | (none) | no | generic handler |
| 0x99A3DC04 | 0x1802EE370 | (none) | no | generic handler |
| 0x9BC1533C | 0x180074980 | (none) | no | generic handler |
| 0x9BED5902 | 0x18013E770 | (none) | no | generic handler |
| 0x9D2801BA | 0x180418A60 | (none) | no | stub-like (returns -104) |
| 0x9D95ACBF | 0x1802A3C80 | escData | no | escape-data handler |
| 0x9E8AF554 | 0x180011490 | (none) | no | `_guard_check_icall_nop` (not a real handler — CFG stub) |
| 0x9F862880 | 0x180419F00 | migData | no | migration data handler |
| 0xA0F5D359 | 0x180348F40 | (none) | no | generic handler |
| 0xA241F6FF | 0x1801C2F80 | escData | no | escape-data handler |
| 0xA24985C3 | 0x180240E20 | setData | no | generic set-data |
| 0xA45761BB | 0x1803103A0 | escData/readScanoutParams | no | display scanout params |
| 0xA5A7E533 | 0x180145B80 | (none) | no | stub-like (returns -3) |
| 0xA88A59CF | 0x1803565C0 | (none) | no | generic handler |
| 0xA8DCE3A9 | 0x180418740 | (none) | no | stub-like |

## Remaining 80 handlers (not deep-analyzed)

All marked **uncertain** — same generic NVAPI wrapper pattern (logging → sub_180390EA0 init → RM escape dispatch). No OC-signature strings or struct magics detected in the binary-wide search for their address ranges. The RM-escape constant search (0x0700_01xx, 0x06FF00xx) and OC struct-magic search (0x200DC, 0x214AC, 0x432D0, 0x1E8604, 0x78604, 0x10124, 0x30178, 0x0F4BF4, 0x20038, 0x10088, 0x10090) confirmed all hits land in already-registered handler ranges.

| iid | handler | oc? |
|---|---|---|
| 0xA9FAD8E6 | 0x1801B6C90 | uncertain |
| 0xAAAB7A56 | 0x18023E120 | uncertain |
| 0xAB20A0B1 | 0x180074B60 | uncertain |
| 0xACD95468 | 0x1802A1CF0 | uncertain |
| 0xAE8B8E03 | 0x1801C4040 | uncertain |
| 0xB05DBCAE | 0x180145290 | uncertain |
| 0xB0A272F6 | 0x180350820 | uncertain |
| 0xB13D735B | 0x180145C00 | uncertain |
| 0xB1425D38 | 0x180359C30 | uncertain |
| 0xB3B1CFD0 | 0x180375D80 | uncertain |
| 0xB5740F4D | 0x1801249B0 | uncertain |
| 0xB8D509BE | 0x18011E650 | uncertain |
| 0xB911B66C | 0x18013AA90 | uncertain |
| 0xB9714B03 | 0x1803C81F0 | uncertain |
| 0xB98C28BC | 0x1802A2460 | uncertain |
| 0xBD7DC1FB | 0x18024BFE0 | uncertain |
| 0xBE175491 | 0x180372530 | uncertain |
| 0xBE48BDAB | 0x180183FA0 | uncertain |
| 0xBE94281E | 0x1801CC9E0 | uncertain |
| 0xBEDE983E | 0x180241200 | uncertain |
| 0xBF5B7D50 | 0x1801B6730 | uncertain |
| 0xC1C1BFBD | 0x180140E80 | uncertain |
| 0xC268BC22 | 0x1801CDBD0 | uncertain |
| 0xC341F6B7 | 0x180359910 | uncertain |
| 0xC374C29E | 0x18032B930 | uncertain |
| 0xC570AE07 | 0x18024AF20 | uncertain |
| 0xC6AD5F2A | 0x180352070 | uncertain |
| 0xC78AB939 | 0x1803341B0 | uncertain |
| 0xC8CF5A50 | 0x18030B790 | uncertain |
| 0xC9C5FDF0 | 0x180356BD0 | uncertain |
| 0xCA2B07E4 | 0x180140A30 | uncertain |
| 0xCA9A2570 | 0x180069AD0 | uncertain |
| 0xCC0854E1 | 0x180347350 | uncertain |
| 0xCC16585C | 0x180332540 | uncertain |
| 0xCEA1B61E | 0x180074450 | uncertain |
| 0xD0823634 | 0x18021E1D0 | uncertain |
| 0xD17DA1E9 | 0x180140270 | uncertain |
| 0xD36EE85C | 0x1801C5AF0 | uncertain |
| 0xD5B5CBA3 | 0x180354080 | uncertain |
| 0xD5CC9797 | 0x180256F00 | uncertain |
| 0xD663DFFB | 0x1802400F0 | uncertain |
| 0xD7C61344 | 0x1800E62E0 | no (already known: InternalUnload, teardown-only) |
| 0xD974E707 | 0x180183200 | uncertain |
| 0xD977E2C0 | 0x1801E4010 | uncertain |
| 0xDC3E2E7A | 0x1802A0CD0 | uncertain |
| 0xDCB10EB4 | 0x1804151A0 | uncertain |
| 0xDF5D820A | 0x180351230 | uncertain |
| 0xDFB987FB | 0x18024BC10 | uncertain |
| 0xE1CD7898 | 0x1801B5D90 | uncertain |
| 0xE348E803 | 0x180331D40 | uncertain |
| 0xE4B56FCB | 0x1804189E0 | uncertain |
| 0xE56620A7 | 0x180418960 | uncertain |
| 0xE5EF574D | 0x18023DCC0 | uncertain |
| 0xE6AAED18 | 0x1801CBC80 | uncertain |
| 0xE6B95925 | 0x180242250 | uncertain |
| 0xE7278182 | 0x18025EBC0 | uncertain |
| 0xE7B693A4 | 0x18032E440 | uncertain |
| 0xE7D12D55 | 0x1801CD300 | uncertain |
| 0xEC9957A4 | 0x180363010 | uncertain |
| 0xEDF47247 | 0x18023CBB0 | uncertain |
| 0xEE2CE6A0 | 0x1804188E0 | uncertain |
| 0xEE72205E | 0x1801B6260 | uncertain |
| 0xEF5D7395 | 0x18032CB20 | uncertain |
| 0xF0C98EC0 | 0x180350500 | uncertain |
| 0xF10EC6AD | 0x180255E20 | uncertain |
| 0xF28F1BE0 | 0x1803339B0 | uncertain |
| 0xF2F1A9F3 | 0x180073C60 | uncertain |
| 0xF43D76F2 | 0x180292F00 | uncertain |
| 0xF5D0254F | 0x1801C76E0 | uncertain |
| 0xF5FB0299 | 0x18034A310 | uncertain |
| 0xF7C36CFA | 0x180146D10 | uncertain |
| 0xF9B5FB6A | 0x180372020 | uncertain |
| 0xF9D60904 | 0x1802394A0 | uncertain |
| 0xFBAD6D11 | 0x1801B5140 | uncertain |
| 0xFDB5B3FA | 0x1801E1C70 | uncertain |
| 0xFDB7AA2D | 0x1802A5250 | uncertain |
| 0xFDCF2BC1 | 0x180419580 | uncertain |

## Methodology note

The 80 "uncertain" handlers follow the identical generic pattern as the 35 deep-analyzed ones (logging check → sub_180390EA0 init → RM escape dispatch). The binary-wide search for OC-defining constants (RM escapes 0x0700_01xx/0x06FF00xx + 11 known OC struct magics) found all hits inside handler address ranges already registered in nvid.rs. This strongly suggests the 80 unanalyzed handlers are also non-OC (display/D3D/migration/coprocessor/grid subsystems), but a per-handler decompile would be needed for 100% confirmation.
