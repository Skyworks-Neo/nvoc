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

### GPU-Z and direct hardware access

| Document | Purpose | Status |
|---|---|---|
| [`gpu-z/per-rail-power.md`](gpu-z/per-rail-power.md) | Evidence about GPU-Z per-rail power and the boundary between NVAPI and direct PCI/MMIO access | Historical conclusion with later corrections called out |
| [`gpu-z/query-interface-tracing.md`](gpu-z/query-interface-tracing.md) | WinDbg workflow for tracing `nvapi_QueryInterface` calls | Reusable method; addresses and candidate lists are snapshot-specific |
| [`gpu-z/trace-query-interface.js`](gpu-z/trace-query-interface.js) | Frida helper for recording lazily resolved QueryInterface IDs | Superseded for the original watts hypothesis; reusable for other IDs |
| [`gpu-z/vram-hotspot-mmio.md`](gpu-z/vram-hotspot-mmio.md) | Linux and Windows MMIO approaches for memory-hotspot research | Background only; direct MMIO is outside NVOC's supported path |

### NVAPI audits

| Document | Purpose | Snapshot |
|---|---|---|
| [`nvapi/oc-surface-audit.md`](nvapi/oc-surface-audit.md) | Broad inventory of OC-relevant IDs | `nvapi-rs` state on 2026-08-25 |
| [`nvapi/wrapper-gap-audit.md`](nvapi/wrapper-gap-audit.md) | Focused wrapper-gap and alias audit | `nvapi-rs` state on 2026-08-25 |
| [`nvapi/oc-gap-layouts-r610-74.md`](nvapi/oc-gap-layouts-r610-74.md) | Recovered signatures and layouts for selected gaps | Windows driver R610.74 |
| [`nvapi/unregistered-handler-triage-1.md`](nvapi/unregistered-handler-triage-1.md) | First batch of unregistered handler classifications | Windows driver R610.74 |
| [`nvapi/unregistered-handler-triage-2.md`](nvapi/unregistered-handler-triage-2.md) | Second batch of unregistered handler classifications | Windows driver R610.74 |
| [`nvapi/evidence/`](nvapi/evidence/) | Generated QueryInterface and handler input tables | Evidence for the two triage reports |

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
