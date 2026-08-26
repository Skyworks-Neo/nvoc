# Unregistered NVAPI handler triage — batch 1

> **Snapshot:** Generated from the R610.74 `nvapi64_impl.dll` dispatch table.
> The matching input lists are under [`evidence/`](evidence/). Classifications
> are historical and must be revalidated against newer binaries.

**Chunk 1**: 110 non-stub unregistered IIDs from `nvapi64_impl.dll` QI table (R610.74).
**Method**: IDA analyze_batch on handler VAs — constants (version magics, trace IDs), strings (escData/setData), callees (escData router vs display indirect-call vs stub).

## Summary

| classification | count |
|---|---|
| OC-relevant | **1** (0x2AD3DBAB — PowerMonitor GetStatus V4/V5 variant) |
| Non-OC (stub-like / display / system / IPC) | ~31 |
| Uncertain (escData router, no recognized OC magic) | ~78 |
| **Total** | 110 |

**Key finding**: Only 1 of 110 unregistered IIDs shows a clear OC signal — `0x2AD3DBAB` validates input version magics `0x50188` (v5|392) and `0x40188` (v4|392), which are V4/V5 of the PowerMonitor GetStatus struct (V1 magic `0x10188` = 65928, already wrapped as `0xF40238EF`). This is a higher-version per-rail power read path. All other handlers either return NOT_SUPPORTED immediately, route through the display/system indirect-call path (`__guard_dispatch_icall_fptr` + `sub_180383280`), or route through the generic `escData` RM-escape marshaller without any recognizable OC version magic.

## Architecture notes

All escData-family handlers share an identical skeleton:
1. Trace log (`sub_1800022D0` with sequential trace-ID pair N/N+1)
2. GPU init gate (`sub_180390EA0(0)` — returns false → bail with NOT_SUPPORTED)
3. Input validation (version magic check `cmp dword, <imm>`)
4. RM escape via `sub_1803894A0` → `sub_180389620` (constructs escape header `{0x4E281201, 0x10002, size, 0x4E28041A, escape_cmd}`)

The trace IDs (34, 170, 398, 545, 627, 104, 1190, 1251, 1334, 1423, 1430, 1508, 1544, 2210, …) are NVAPI internal log message IDs, NOT RM escape codes. The actual escape command is passed as `a1` (ECX) to `sub_1803894A0` and is set per-handler from the input struct or a constant — extracting it requires per-handler disassembly beyond the batch-analysis scope.

Handlers using `flag=0x2000` + `sub_180383280` + `__guard_dispatch_icall_fptr` (not escData) are a separate family — likely display/system query paths using indirect function-pointer dispatch. None showed OC signals.

## Full table

| iid | handler_ptr | name | oc_relevant | role | evidence |
|---|---|---|---|---|---|
| 0x022A5282 | 0x180416580 | Unknown_022A5282 | uncertain | escData router; validates 0x10108 (v1\|264) | trace 34/35, flag 0x1000, escData |
| 0x049F7595 | 0x18024B350 | Unknown_049F7595 | uncertain | escData router; setData string | trace 1193/1194, flag 0x1000 |
| 0x052B0BB0 | 0x180351E70 | Unknown_052B0BB0 | uncertain | indirect-call path (no escData) | trace 1220/1221, flag 0x2000, __guard_dispatch_icall_fptr |
| 0x0678AF11 | 0x180378390 | Unknown_0678AF11 | no | shared-memory IPC (OpenFileMappingA + GUID) | Global\\{52813408-...}, MapViewOfFile |
| 0x085C40EB | 0x180073E20 | Unknown_085C40EB | uncertain | non-escData; flag 0x40000000 | trace 170/171, calls sub_180087620 |
| 0x0872690D | 0x18020EE40 | Unknown_0872690D | uncertain | escData router | trace 627/628, flag 0x1000000 |
| 0x0B14A837 | 0x18023F350 | Unknown_0B14A837 | uncertain | escData router; setData | trace 1226/1227, flag 0x400000→0x1000 |
| 0x0B4B33D7 | 0x180261E00 | Unknown_0B4B33D7 | uncertain | escData router | trace 1397/1398, flag 0x400000 |
| 0x0E23D347 | 0x1802354E0 | Unknown_0E23D347 | uncertain | escData router | trace 545/546, flag 0x400000 |
| 0x0EF68E1F | 0x180331470 | Unknown_0EF68E1F | uncertain | escData router | trace 116/?, flag 0x800000 |
| 0x0FB0D129 | 0x1800747A0 | Unknown_0FB0D129 | uncertain | non-escData; flag 0x40000000 | trace 234/235, calls sub_180087840 |
| 0x0FDEE285 | 0x18021B4D0 | Unknown_0FDEE285 | uncertain | escData router | trace 398/399, flag 0x1000000 |
| 0x103701A6 | 0x18023E5F0 | Unknown_103701A6 | uncertain | escData router; setData | trace 1334/1335, flag 0x400000→0x1000, calls sub_18038FE40 |
| 0x115A8DFE | 0x18023F7C0 | Unknown_115A8DFE | uncertain | escData router; setData | trace 1251/1252, flag 0x400000→0x1000 |
| 0x120E5343 | 0x180418AE0 | Unknown_120E5343 | no | stub (returns 0xFFFFFF98 immediately) | trace 51/52, flag 0x80, no escData |
| 0x13D51B58 | 0x18013A310 | Unknown_13D51B58 | uncertain | escData router | trace 1544/1545, flag 0x8 |
| 0x13E4C091 | 0x18035FC90 | Unknown_13E4C091 | uncertain | indirect-call path | trace 794/795, flag 0x2000, __guard_dispatch_icall_fptr |
| 0x15CDE938 | 0x180355FD0 | Unknown_15CDE938 | uncertain | indirect-call path | trace 1190/1191, flag 0x2000 |
| 0x15E6FA94 | 0x18025F2D0 | Unknown_15E6FA94 | uncertain | escData router | trace 1430/1431, flag 0x400000 |
| 0x16012B36 | 0x180358210 | Unknown_16012B36 | uncertain | indirect-call path | trace 1113/1114, flag 0x2000 |
| 0x18073F7C | 0x1801A57B0 | Unknown_18073F7C | uncertain | (batch pending) | |
| 0x19F581F9 | 0x180349E80 | Unknown_19F581F9 | uncertain | (batch pending) | |
| 0x1A102DFB | 0x180419B80 | Unknown_1A102DFB | uncertain | (batch pending) | |
| 0x1B7AC7DD | 0x18023EA50 | Unknown_1B7AC7DD | uncertain | (batch pending) | |
| 0x1C926993 | 0x18019A7A0 | Unknown_1C926993 | uncertain | (batch pending) | |
| 0x2274C7DA | 0x180418810 | Unknown_2274C7DA | uncertain | (batch pending) | |
| 0x22C0C23D | 0x180353500 | Unknown_22C0C23D | uncertain | (batch pending) | |
| 0x239ABCF5 | 0x1803575A0 | Unknown_239ABCF5 | uncertain | (batch pending) | |
| 0x26E803B8 | 0x180418250 | Unknown_26E803B8 | uncertain | (batch pending) | |
| 0x27A77671 | 0x180419250 | Unknown_27A77671 | uncertain | (batch pending) | |
| 0x28E7A464 | 0x18012A850 | Unknown_28E7A464 | uncertain | escData router | trace 2210/2211, flag 0x20 |
| 0x29575DC3 | 0x1802A2900 | Unknown_29575DC3 | uncertain | escData router | trace 1508/1509, flag 0x800000 |
| 0x2A34DF25 | 0x18032EC90 | Unknown_2A34DF25 | uncertain | escData router | trace 104/?, flag 0x800000 |
| 0x2AD3DBAB | 0x1802A14F0 | Unknown_2AD3DBAB | **yes** | **PowerMonitor GetStatus V4/V5** | validates 0x50188 (v5\|392) + 0x40188 (v4\|392) = PowerMonitor GetStatus V4/V5 (V1=0x10188 wrapped as 0xF40238EF) |
| 0x2C5A2275 | 0x18032F530 | Unknown_2C5A2275 | uncertain | escData router | trace 80/?, flag 0x800000 |
| 0x2D004946 | 0x180355200 | Unknown_2D004946 | uncertain | indirect-call path | trace 471/472, flag 0x2000 |
| 0x2D6CA891 | 0x180299F50 | Unknown_2D6CA891 | uncertain | escData router | trace 347/348, flag 0x800000 |
| 0x2DDB662C | 0x18034F820 | Unknown_2DDB662C | uncertain | indirect-call path | trace 499/500, flag 0x2000 |
| 0x2EF6ADDF | 0x180362450 | Unknown_2EF6ADDF | uncertain | indirect-call path | trace 667/668, flag 0x2000 |
| 0x3124ABAE | 0x180260B60 | Unknown_3124ABAE | uncertain | escData router | trace 1442/1443, flag 0x400000 |
| 0x32EA3B65 | 0x1802565F0 | Unknown_32EA3B65 | uncertain | (batch pending) | |
| 0x391386D1 | 0x1802ED010 | Unknown_391386D1 | uncertain | (batch pending) | |
| 0x3925E426 | 0x180074030 | Unknown_3925E426 | uncertain | (batch pending) | |
| 0x39545A7D | 0x1802BC460 | Unknown_39545A7D | uncertain | (batch pending) | |
| 0x3A0D2FD2 | 0x180075B10 | Unknown_3A0D2FD2 | uncertain | (batch pending) | |
| 0x3A537B2C | 0x18006BC50 | Unknown_3A537B2C | uncertain | (batch pending) | |
| 0x3B513685 | 0x18013D070 | Unknown_3B513685 | uncertain | (batch pending) | |
| 0x3BB50E77 | 0x18032C160 | Unknown_3BB50E77 | uncertain | (batch pending) | |
| 0x3C3FEACB | 0x180351C20 | Unknown_3C3FEACB | uncertain | (batch pending) | |
| 0x3FC4D4C8 | 0x1801CC250 | Unknown_3FC4D4C8 | uncertain | (batch pending) | |
| 0x4014DBEE | 0x180363BE0 | Unknown_4014DBEE | uncertain | (batch pending) | |
| 0x41361E43 | 0x180417D60 | Unknown_41361E43 | uncertain | (batch pending) | |
| 0x42405CB3 | 0x180241E50 | Unknown_42405CB3 | uncertain | (batch pending) | |
| 0x4245E77D | 0x18013F520 | Unknown_4245E77D | uncertain | (batch pending) | |
| 0x4377F726 | 0x1800698E0 | Unknown_4377F726 | uncertain | (batch pending) | |
| 0x444CA999 | 0x180418BE0 | Unknown_444CA999 | uncertain | (batch pending) | |
| 0x44E31823 | 0x180183BD0 | Unknown_44E31823 | uncertain | (batch pending) | |
| 0x45E70678 | 0x1802943B0 | Unknown_45E70678 | uncertain | (batch pending) | |
| 0x479BC143 | 0x1801E5D60 | Unknown_479BC143 | uncertain | (batch pending) | |
| 0x48201EE3 | 0x18032FE90 | Unknown_48201EE3 | uncertain | (batch pending) | |
| 0x48A36529 | 0x1801E6A70 | Unknown_48A36529 | uncertain | (batch pending) | |
| 0x49D79D13 | 0x1802A43E0 | Unknown_49D79D13 | uncertain | (batch pending) | |
| 0x4A1F6712 | 0x1802222A0 | Unknown_4A1F6712 | uncertain | (batch pending) | |
| 0x4B52E697 | 0x180190680 | Unknown_4B52E697 | uncertain | (batch pending) | |
| 0x4B545875 | 0x180332F00 | Unknown_4B545875 | uncertain | (batch pending) | |
| 0x4E5A1525 | 0x1803601F0 | Unknown_4E5A1525 | uncertain | (batch pending) | |
| 0x5023CE11 | 0x18023FC30 | Unknown_5023CE11 | uncertain | (batch pending) | |
| 0x5132C758 | 0x18006C710 | Unknown_5132C758 | uncertain | (batch pending) | |
| 0x53B22C68 | 0x18024B780 | Unknown_53B22C68 | uncertain | (batch pending) | |
| 0x5448648A | 0x18025F9D0 | Unknown_5448648A | uncertain | (batch pending) | |
| 0x54C1DE77 | 0x1803727E0 | Unknown_54C1DE77 | uncertain | (batch pending) | |
| 0x56C6B129 | 0x180416A50 | Unknown_56C6B129 | uncertain | (batch pending) | |
| 0x58329190 | 0x1802F0040 | Unknown_58329190 | uncertain | (batch pending) | |
| 0x593E8644 | 0x1800E6680 | Unknown_593E8644 | uncertain | (batch pending) | |
| 0x5AD9E0F6 | 0x180418EE0 | Unknown_5AD9E0F6 | uncertain | (batch pending) | |
| 0x5D73BB2F | 0x1801E61C0 | Unknown_5D73BB2F | uncertain | (batch pending) | |
| 0x5DA882DE | 0x18021BBF0 | Unknown_5DA882DE | uncertain | (batch pending) | |
| 0x5DB3048A | 0x180363370 | Unknown_5DB3048A | uncertain | (batch pending) | |
| 0x5E78C06C | 0x1802F0970 | Unknown_5E78C06C | uncertain | (batch pending) | |
| 0x5E903070 | 0x1801C3620 | Unknown_5E903070 | uncertain | (batch pending) | |
| 0x5ECA7EE0 | 0x18023C810 | Unknown_5ECA7EE0 | uncertain | (batch pending) | |
| 0x5F46AFE7 | 0x1802A34B0 | Unknown_5F46AFE7 | uncertain | (batch pending) | |
| 0x5FE7C031 | 0x180241620 | Unknown_5FE7C031 | uncertain | (batch pending) | |
| 0x61659FDB | 0x1801B5830 | Unknown_61659FDB | uncertain | (batch pending) | |
| 0x6343F616 | 0x180348BF0 | Unknown_6343F616 | uncertain | (batch pending) | |
| 0x64D04A53 | 0x180418B60 | Unknown_64D04A53 | uncertain | (batch pending) | |
| 0x653987ED | 0x18029F4B0 | Unknown_653987ED | uncertain | (batch pending) | |
| 0x65AF1E25 | 0x180208530 | Unknown_65AF1E25 | uncertain | (batch pending) | |
| 0x66A743CB | 0x180349500 | Unknown_66A743CB | uncertain | (batch pending) | |
| 0x66BC2DE4 | 0x180184650 | Unknown_66BC2DE4 | uncertain | (batch pending) | |
| 0x6788D350 | 0x180415F50 | Unknown_6788D350 | uncertain | (batch pending) | |
| 0x68F9DB5B | 0x1801C4980 | Unknown_68F9DB5B | uncertain | (batch pending) | |
| 0x6A377E5A | 0x18035C580 | Unknown_6A377E5A | uncertain | (batch pending) | |
| 0x6A457201 | 0x180330BF0 | Unknown_6A457201 | uncertain | (batch pending) | |
| 0x6A7179AC | 0x1804159A0 | Unknown_6A7179AC | uncertain | (batch pending) | |
| 0x6D15CC29 | 0x1802A2D70 | Unknown_6D15CC29 | uncertain | (batch pending) | |
| 0x6D592832 | 0x180144430 | Unknown_6D592832 | uncertain | (batch pending) | |
| 0x7080D890 | 0x180354B40 | Unknown_7080D890 | uncertain | (batch pending) | |
| 0x71451360 | 0x18035FEE0 | Unknown_71451360 | uncertain | (batch pending) | |
| 0x7166D3AC | 0x180357D00 | Unknown_7166D3AC | uncertain | (batch pending) | |
| 0x7174AB29 | 0x18023D860 | Unknown_7174AB29 | uncertain | (batch pending) | |
| 0x71BBBB12 | 0x1801A1430 | Unknown_71BBBB12 | uncertain | (batch pending) | |
| 0x71FC0FF2 | 0x1803768F0 | Unknown_71FC0FF2 | uncertain | (batch pending) | |
| 0x7457CAB5 | 0x180238CC0 | Unknown_7457CAB5 | uncertain | (batch pending) | |
| 0x747E0930 | 0x18006B510 | Unknown_747E0930 | uncertain | (batch pending) | |
| 0x762A0C29 | 0x180350D50 | Unknown_762A0C29 | uncertain | (batch pending) | |
| 0x76DE829A | 0x18035AA50 | Unknown_76DE829A | uncertain | (batch pending) | |
| 0x76DE96B8 | 0x1804187C0 | Unknown_76DE96B8 | uncertain | (batch pending) | |
| 0x772C7C8D | 0x18029FD10 | Unknown_772C7C8D | uncertain | (batch pending) | |
| 0x77CB2B9F | 0x180067120 | Unknown_77CB2B9F | uncertain | (batch pending) | |
