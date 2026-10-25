# Reverse-engineering archive

This directory preserves investigation notes and generated evidence that explain
how NVOC's NVIDIA API coverage was evaluated. It is an engineering archive, not
a supported API contract. Start with the maintained wiki summary in
[`../wiki/Reverse-Engineering.md`](../wiki/Reverse-Engineering.md).

## Safety and scope

- Production NVOC code should prefer documented NVAPI or NVML interfaces.
- A private handler, structure layout, or MMIO offset is not considered stable
  merely because it worked on one driver and GPU.
- Raw probes must remain read-only, ignored by default, and explicitly gated on
  compatible hardware. Private setters require a separate design, recovery
  plan, and cross-hardware validation.
- Handler addresses and structure-version observations in this archive are
  snapshots. Re-derive them for a different driver build before relying on them.
- Proprietary DLLs, executables, disassembler databases, and local probe output
  are intentionally not stored in the repository.

## Archive map

### Cross-tool ecosystem surveys

| Document | Purpose | Status |
|---|---|---|
| [`gpu-unlock-ecosystem-survey.md`](gpu-unlock-ecosystem-survey.md) | Full survey of the five GPU-unlock ecosystem repos under `reverse/` (envytools, cmp170hx, gpu-unlock, gpu-unlock-ada, gpu-unlock-4090-48g-64g-bar1) + TOTORONG: lineage, why it stays independent of NVOC, the five capability gaps, what to borrow, the IDA/idalib Falcon verdict (three-wall experiment), and the six-item learning list | Conclusions as of 2026-10-25; learning list ①–⑥ with acceptance criteria |
| [`stressor-stability-technique-audit.md`](stressor-stability-technique-audit.md) | External stress-tool stability techniques vs NVOC's approach | Historical audit |
### GPU-Z and direct hardware access

| Document | Purpose | Status |
|---|---|---|
| [`gpu-z/per-rail-power.md`](gpu-z/per-rail-power.md) | Evidence about GPU-Z per-rail power and the boundary between NVAPI and direct PCI/MMIO access | Historical conclusion with later corrections called out |
| [`gpu-z/query-interface-tracing.md`](gpu-z/query-interface-tracing.md) | WinDbg workflow for tracing `nvapi_QueryInterface` calls | Reusable method; addresses and candidate lists are snapshot-specific |
| [`gpu-z/trace-query-interface.js`](gpu-z/trace-query-interface.js) | Frida helper for recording lazily resolved QueryInterface IDs | Superseded for the original watts hypothesis; reusable for other IDs |
| [`gpu-z/vram-hotspot-mmio.md`](gpu-z/vram-hotspot-mmio.md) | Linux and Windows MMIO approaches for memory-hotspot research | Background only; direct MMIO is outside NVOC's supported path |
| [`gpu-z/gpuz-sensor-audit.md`](gpu-z/gpuz-sensor-audit.md) | GPU-Z 2.71 sensor-source audit (fill-in for compute/topology capability counters) | Snapshot audit |

### NVAPI audits

| Document | Purpose | Snapshot |
|---|---|---|
| [`nvapi/oc-surface-audit.md`](nvapi/oc-surface-audit.md) | Broad inventory of OC-relevant IDs | `nvapi-rs` state on 2026-08-25 |
| [`nvapi/wrapper-gap-audit.md`](nvapi/wrapper-gap-audit.md) | Focused wrapper-gap and alias audit | `nvapi-rs` state on 2026-08-25 |
| [`nvapi/oc-gap-layouts-r610-74.md`](nvapi/oc-gap-layouts-r610-74.md) | Recovered signatures and layouts for selected gaps | Windows driver R610.74 |
| [`nvapi/unregistered-handler-triage-1.md`](nvapi/unregistered-handler-triage-1.md) | First batch of unregistered handler classifications | Windows driver R610.74 |
| [`nvapi/unregistered-handler-triage-2.md`](nvapi/unregistered-handler-triage-2.md) | Second batch of unregistered handler classifications | Windows driver R610.74 |
| [`nvapi/registerop-0x2eb3c140-handler-r61047.md`](nvapi/registerop-0x2eb3c140-handler-r61047.md) | RegisterOp read lane (per-chip GDDR6x/7 temps, Blackwell hotspot fallback) | Windows driver R610.47 |
| [`nvapi/evidence/`](nvapi/evidence/) | Generated QueryInterface and handler input tables | Evidence for the two triage reports |

### VBIOS decoding

| Document | Purpose | Snapshot |
|---|---|---|
| [`vbios/tu106-fwsec-ucode-decode.md`](vbios/tu106-fwsec-ucode-decode.md) | BIT → token 0x70 → ucode-table → FWSEC descriptor chain; V1/V2/V3 descriptor layouts (kernel_gsp_fwsec.c as authority); TU106 measured geometry; parser enhancement `8353da3` and the nvapi-rs gitlink incident | `1.rom` TU106 90.06.18.00.EE, 2026-10-25 |
| [`vbios/vix-editor-field-map.md`](vbios/vix-editor-field-map.md) | vix-editor GUI field-name mapping to nvoc decoder fields | vbios-parser-enhanced branch |
| [`vbios/parser-gap-fill-envytools.md`](vbios/parser-gap-fill-envytools.md) | Parser completeness pass against the five-repo ecosystem survey: DCB pointer subtables (I2C PMGR ports / GPIO named lines / EXTDEV devices / CONN / MUX), BIT 'P' v2 power subtables (PowerBudget cap-entry = nvidia-smi wall / sense / base-clock / voltage), BIT token-level tables ('M'/'I'/'C'/'d'), FWSEC DMEM interface + FRTS mapper, PCIR/EFI header details; three divergence verdicts vs envytools | `1.rom` TU106 + GP102, 2026-10-25 |

### UEFI pre-driver research

| Document | Purpose | Snapshot |
|---|---|---|
| [`uefi/windows-uefi-pre-driver-unlocker-dev-plan.md`](uefi/windows-uefi-pre-driver-unlocker-dev-plan.md) | Development plan for a TOTORONG-style Windows EFI pre-driver unlocker: toolchain decision (Rust + uefi-rs), machine reality (TU106 experiment rig / GA106 deferred / P100 N/A), environment assessment, milestones M0–M3 with acceptance/risk/rollback, written safety discipline | Approved 2026-10-25; no code yet |
| [`uefi/uefi-dev-agent-prompt.md`](uefi/uefi-dev-agent-prompt.md) | Self-contained task brief (提示词) for a fresh development agent in the separate UEFI project: safety constitution, TU106/GA10x/AD10x target profiles with register tables, reference-repo reading order, condensed fact library (boot chain / Falcon interface / FWSEC descriptor V1–V3 / V67 payload structure / EFI carrier craft), milestones M0–M5 with gates, gotchas | v1 2026-10-25; copy wholesale into the new project |

### MODS cross-reference

| Document | Purpose | Snapshot |
|---|---|---|
| [`mods/mods-cross-reference-blackwell-pro6000.md`](mods/mods-cross-reference-blackwell-pro6000.md) | MODS cross-reference for Blackwell Pro6000 findings | Blackwell Pro6000 |

### MSI Afterburner

| Document | Purpose | Snapshot |
|---|---|---|
| [`afterburner/oow-mode-verification.md`](afterburner/oow-mode-verification.md) | Differential verification of the "out of warranty" mode claims (4.6.7.16935 vs 4.6.7.19412): oem3 database, hidden-control infrastructure, VRM/policy/MMIO surface | Both builds on disk under `reverse/afterburner*` (binaries not stored; RTCore drivers committed by explicit request) |
| [`afterburner/ab-19412-nvapi-id-surface-tier-table.md`](afterburner/ab-19412-nvapi-id-surface-tier-table.md) | Full private NVAPI interface surface (74 QueryInterface IDs mapped to named wrapper methods), naming-deviation verdicts vs nvoc, and the P0–Pn fill-in tier table | RTHAL.dll 4.6.7.19412 |
| [`afterburner/rtcore64-kernel-surface.md`](afterburner/rtcore64-kernel-surface.md) | RTCore64.sys full IOCTL/kernel-primitive decode (BAR-only physical gate, MSR blacklist, port/PCI guards), RTHAL consumption map, and the comparison with NVOC's pmxdrv wide-table lane | RTCore64.sys 2024-05-13 build |

### NVML audit

| Document | Purpose | Snapshot |
|---|---|---|
| [`nvml/export-coverage-audit.md`](nvml/export-coverage-audit.md) | Comparison of DLL exports, public headers, and Rust bindings | Audited DLL/header versions recorded in the document |

## Reading snapshot claims

The documents intentionally retain negative results and abandoned hypotheses so
future work does not repeat them. A statement can become stale when any of these
change:

1. the NVIDIA driver branch or binary build;
2. the GPU architecture, board design, or firmware;
3. the `nvapi-rs` or `nvml-wrapper-sys` revision;
4. the structure version passed to an undocumented handler.

When a historical note conflicts with current code or a newer measurement,
prefer reproducible current evidence. Update the maintained wiki summary and add
a dated correction to the relevant archive document; do not silently rewrite
the original measurement.

## Promoting archive work into NVOC

Move a result into production only through a focused PR that documents:

- the public or reverse-engineered source of the interface;
- exact driver and GPU coverage;
- read/write and privilege behavior;
- safe failure and recovery behavior;
- hardware-gated tests, with mutating tests ignored by default.
