# Changelog

All notable changes to this repository are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions are
repo-wide (one tag releases all components) and follow SemVer. The version's
single source of truth is `[workspace.package].version` in the root
`Cargo.toml` — see `.github/RELEASE.md` for the release procedure.

## [Unreleased]

## [0.3.1-alpha.1] — 2026-10-25

### Added

- profile: full capture / apply / restore / export / import surface across
  core, CLI, and pynvoc, covering clock, fan-policy, volt-rail, VF-point,
  TopRels-ratio, and private clock-plane domains, with per-domain verification,
  rollback, and multi-GPU fan-out.
- re+cli: Afterburner 19412 parity for the private NVAPI lanes (locks,
  TopRels, I2C, FRL, tempsim) and the kernel-mode wide table.
- srv: closed-loop control service replacing the windows-service stub.
- fan-curve: editable fan curve in GUI, TUI, and pynvoc, plus standalone
  fan-cooler RE probes.
- stressor: ride-on-load verification circuit, Vulkan render stress, OpenCL
  verification, and offscreen Vulkan stress on Linux.
- kmd: embedded `pmxdrv.sys` for `nvoc-cli` on Windows x64 with SHA-256 pin
  and arm64 refusal.
- vbios: structural layer, 3x/GB20x table decoding, and FWSEC inventory.
- guard: neutralize persisted G-Helper GPU OC on TUI/GUI/CLI startup
  (Windows).
- volt: `set-volt-rail-limit --slot` (0..5), pinning P100 / 582.41 slot
  semantics.
- xtask: tkinter environment diagnosis and self-repair; pynvoc pytest step
  fails when every test skips.
- Repo-wide version unification (workspace inheritance for all crates, PEP 440
  mirror for Python packages) with a CI consistency gate.
- Supply-chain CI: cargo-audit, pip-audit, actionlint, Dependabot version
  updates.
- Release pipeline (#224 + #225): tag-triggered draft-release workflow with a
  linux/windows × amd64/arm64 matrix, artifact smoke tests, SHA256SUMS, and
  build provenance attestations.

### Security

- GPU CI: fork pull requests now run on self-hosted GPU runners only at the
  moment the `ci:gpu` label is applied; later pushes require re-labeling.

### Changed

- CI now runs the `nvoc-auto-optimizer` non-GPU unit test suite.
- README Quick Start now documents prebuilt release downloads, checksum
  validation, and GitHub build-provenance verification (#232).
- stressor: removed the legacy image-based Vulkan load (`--legacy-vulkan`,
  `--legacy-vulkan-image-*`, `--legacy-vulkan-minor-mixture-rate`, and the
  matching TOML keys). The FurMark-style render pipeline (`--vulkan`) is the
  only graphics load; `--vulkan-only` now always runs it. Nothing in the
  optimizer, profiles (beyond a comment), tests, or workflows referenced the
  removed interface.
- stressor: `--vulkan-only` is now a pure-Vulkan path handled before any CUDA
  init — the GPU is selected directly from `--gpu-uuid`/`--pci-bus`/
  `--gpu-index` (Vulkan-side PCI-sorted index, full mapping printed before
  selection), so a Vulkan-only run no longer requires a working CUDA driver.
  Mixed-mode `--vulkan` still aligns the sidecar to the CUDA device by UUID.

### Fixed

- TUI no longer crashes when the config file is unwritable.
- release: stage the renamed `nvoc-srv` / `nvoc-srv-ctl` binaries instead of
  the retired `nvoc_service`-style names, which broke the Windows build jobs.
- GUI/TUI VF-curve apply and reset now route **per point**: a point the public
  V/F table can move is written through the open interface, and only a point
  whose public `point_type` reads Fixed goes to the private table. A GPC curve
  built as *hybrid* (private segment + healthy public read) used to keep
  `write_mode="private"` from the segment build, so every GPC apply on a card
  with a populated private segment hit the private table — whose offsets
  **stack** on the public ones on Ada and never show up in the public read, so
  the curve moved twice as far as the user asked with no feedback. Shift+Reset
  stays reachable on such a curve (it is the only way to clear private offsets
  written by an older build) and now zeroes the curve's public runs as well.

## [0.1.0] — historical

Development before versioned releases; see the git history and merged pull
requests up to this point.
