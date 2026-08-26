# Reverse Engineering

NVOC keeps reverse-engineering material to document coverage decisions and
avoid repeating failed investigations. The raw notes and generated tables live
in the [reverse-engineering archive](../reverse-engineering/README.md). They are
evidence snapshots, not supported user-facing APIs.

## Current project boundary

NVOC production code prefers documented NVAPI and NVML interfaces. Undocumented
NVAPI handlers may be useful for read-only discovery, but their structure
versions and behavior can vary by driver, GPU, board, and privilege level.
Direct PCI/MMIO access is even less portable and would require privileged
platform support plus a per-device hardware database, so it is not part of the
supported NVOC path.

The archived investigations currently support these decisions:

- GPU-Z's per-rail telemetry investigation found a direct PCI/MMIO path through
  a kernel driver. Separate NVAPI PowerMonitor handlers can expose useful
  descriptors or status on some systems, but the available layouts are not a
  portable product contract.
- Memory-hotspot MMIO offsets and decoding are architecture- and board-specific.
  A value observed on one GPU must not be generalized without independent
  verification.
- NVAPI and NVML coverage audits are point-in-time inventories. Always compare
  them with current bindings and the current driver before treating a listed
  gap as open.

## Safety requirements

Reverse-engineering work must start read-only. Raw probes stay ignored by
default and require explicit compatible-hardware selection. Private setters are
not promoted from an observed signature alone: they need a recovery plan,
cross-hardware validation, and a focused review of failure semantics.

Never commit proprietary NVIDIA binaries, GPU-Z executables, disassembler
databases, firmware dumps, or machine-specific probe output. Repository ignore
rules cover the common local artifacts.

## Archive navigation

- [GPU-Z and direct-hardware investigations](../reverse-engineering/README.md#gpu-z-and-direct-hardware-access)
- [NVAPI audits and generated evidence](../reverse-engineering/README.md#nvapi-audits)
- [NVML export coverage](../reverse-engineering/README.md#nvml-audit)
- [Promotion checklist](../reverse-engineering/README.md#promoting-archive-work-into-nvoc)

---

*Maintained from: dated investigations under `docs/reverse-engineering/` and
current project safety policy.*
