# Reading VRAM (memory) hotspot temperature via direct MMIO

Companion to `gpuz-per-rail-investigation.md`. That doc covers the **Windows**
path (GPU-Z → WinRing0 kernel driver → PCI/MMIO). This doc covers the **Linux**
path (a third-party `gddr6.c` → `/dev/mem` → BAR0 MMIO) and, more usefully, the
**reverse-engineering methodology** behind both — how the magic register offsets
and decode formulas are actually obtained.

## 1. The technique is identical to GPU-Z

Both approaches bypass the driver APIs (NVAPI / NVML) entirely and read the
GPU's MMIO registers directly. The only difference is *how they earn the
privilege* to touch physical memory — a platform difference, not a technical one.

| Dimension | `gddr6.c` (Linux) | GPU-Z (Windows) |
|---|---|---|
| What it reads | GPU BAR0 MMIO registers (raw hardware) | Same — GPU MMIO / PCI config |
| Privilege source | `mmap(/dev/mem)` — kernel's physical-memory window | WinRing0 kernel driver (IOCTL `0x800064A0`) does the PCI/MMIO R/W on its behalf |
| Kernel-mode requirement | root + `iomem=relaxed` (else STAP blocks `/dev/mem`) | must install the private kernel driver |
| Hardcoded SKU table | `dev_id` → BAR0 offset | same (GPU-Z carries an equivalent internal table) |
| Uses NVAPI/NVML | **No** | **No** (confirmed for per-rail; same here) |

The program's own error message — *"Did you enable iomem=relaxed? Are you
r00t?"* — is the tell. It depends solely on "can I touch physical addresses."
Linux gives that via `/dev/mem` (so root suffices, no driver needed); Windows
has no equivalent, so GPU-Z must lean on a signed kernel driver.

This corroborates the earlier conclusion: this style of read is **dirty**. NV
keeps NVAPI/NVML consistent across driver updates, but MMIO register offsets
(especially Blackwell's raw `0x9A24C0` FBPA sensor and the `0xE2A8` scratch
mirror) can move or become PLM-locked at any time, and every SKU must be
tagged individually.

## 2. How these offsets are reverse-engineered

The code's own comments are a fossil record of the RE path. The methodology
boils down to three routes, usually combined.

### Route 1 — Symbol/name lookup in the driver/firmware (primary route)

Note the names in the Blackwell comment: **`NV_PFB_FBPA_DQR_STATUS_DQ_IC0_SUBP0`**,
**`FBFALCON`**. These are internal NVIDIA driver/firmware symbol names.

1. **Mine driver symbols.** NVIDIA's closed-source `nvidia.ko` (Linux) or
   `nvlddmkm.sys` (Windows) sometimes retain debug symbols or yield structure
   names to IDA/Ghidra. The open-source **nouveau** driver and the
   **envytools/rnndb** project maintain a living database of NVIDIA register
   names (`NV_PFB_FBPA_*` decodes as PFB = memory controller, FBPA = framebuffer
   partition A, etc.).
2. **Find "who reads this register."** Search the driver/firmware for the
   `FBFALCON` (memory-controller firmware coprocessor) temperature-read path
   and see which offset it `readl`s — that is the authoritative source. The
   comment "the register the FBFALCON firmware reads" is exactly this.
3. **Verify readability + locate PLM.** NVIDIA uses **PLM (Privilege Level
   Management)**: some registers are EL3/firmware-only and return garbage or
   fault from userspace. The comment spells out the outcome: `0x9A44B0` is
   PLM-locked (unreadable), its `0xE2A8` scratch mirror is unpopulated on the
   5090, so it **falls back to the raw sensor `0x9A24C0`** (unlocked). Which
   register is unlocked-vs-locked is found by **trial + firmware RE**.

### Route 2 — Differential scan against a known value (the Ada/Ampere batch)

How `0x0000E2A8` gets attached to a pile of SKUs:

1. **Obtain a known temperature source** — `nvidia-smi -q` memory temperature,
   or GPU-Z's displayed VRAM temp.
2. **Scan BAR0.** `mmap` all of BAR0 (typically 16 MB), read word-by-word, find
   the word whose value (low-12-bits / 32) matches the known temperature *and*
   changes as the memory heats up. That locks the offset.
3. **Generalize across SKUs.** Same-architecture GPUs usually share the layout,
   so an offset found on one card (e.g. `0xE2A8`) very likely holds for the
   whole architecture — hence the long run of `0xE2A8` rows. Exceptions
   (GA104 / RTX 3070 uses `0xEE50`) require per-card verification.

### Route 3 — Standing on open-source predecessors (the usual real source)

In practice most such code is not reverse-engineered from scratch. It is ported
from:

- **nouveau** (`drivers/gpu/drm/nouveau/` in the kernel tree) register defs.
- **envytools / rnndb** — the NVIDIA register name↔offset database.
- Leaked offsets from HWiNFO / GPU-Z RE, posted to forums / GitHub gists.

The `dev_table` here — dev_ids plus symbol names in comments — is clearly
ported from a nouveau/envytools-style project and then validated against
`nvidia-smi` on real hardware.

## 3. The three things that must be reverse-engineered

| What | How |
|---|---|
| **Register offset** (`0xE2A8`, `0x9A24C0`) | driver symbol lookup (nouveau/rnndb) + BAR0 differential scan vs a known temperature |
| **Decode formula** (`/32`, MR-code × 2) | read the driver/firmware temperature path and see how it computes it + calibrate against a thermometer |
| **Readability / PLM status** | live read test + firmware RE to confirm which register is unlocked |

## 4. Implication for nvoc

nvoc's earlier conclusion stands: GPU-Z's per-rail (and by the same mechanism,
VRAM-hotspot) readings require a **kernel-mode component** (signed driver) to
do the MMIO access. On Windows there is no `/dev/mem` equivalent, so without
shipping a kernel driver, userspace NVAPI/NVML cannot deliver per-rail or
VRAM-hotspot granularity. This Linux sample is the same idea expressed on a
platform where the kernel *hands you* a physical-memory window, removing the
need for a third-party driver.

Sources: the `gddr6.c` sample (author's comments), `gpuz-per-rail-investigation.md`,
nouveau/envytools public register databases.
