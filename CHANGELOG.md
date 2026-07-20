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

## [0.1.0] — historical

Development before versioned releases; see the git history and merged pull
requests up to this point.
