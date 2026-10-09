# Changelog

All notable changes to this repository are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions are
repo-wide (one tag releases all components) and follow SemVer. The version's
single source of truth is `[workspace.package].version` in the root
`Cargo.toml` — see `.github/RELEASE.md` for the release procedure.

## [Unreleased]

### Added

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

### Fixed

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
