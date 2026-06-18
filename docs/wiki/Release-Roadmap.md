# Release Roadmap

This page turns the current repository state into manageable sprints and a release decision checklist. It is intentionally conservative because NVOC can write overclocking, voltage, clock, power, thermal, and fan settings to real GPUs.

See also: [Sprint issue triage plan](../../.github/SPRINT_ISSUE_TRIAGE.md) for the current issue-to-sprint label mapping and the tool that applies it.

## Release decision as of 2026-06-18

**Decision: do not cut a stable `v1.0.0` tag from the current development state yet.** The project has enough implemented functionality for an internal or community preview, but a stable release should wait until hardware-backed safety, packaging, and known compatibility gaps are closed.

Recommended tag policy:

- **Allowed now:** an explicitly unstable pre-release tag such as `v0.1.0-alpha.1` or `v0.1.0-rc.0`, only from `sprint`, after the release/tag guard is enabled in repository rules.
- **Not recommended now:** a stable SemVer tag such as `v1.0.0`, because GPU-mutating coverage, platform packaging, and support-matrix unknowns still need sprint ownership.
- **Required before any public release:** green normal CI, a labeled/manual GPU CI run on known recoverable hardware, release notes that call out unsupported GPU generations/backends, and a tested rollback/recovery path.

## Current achieved features

| Area | Achieved capability | Release value | Remaining release risk |
|---|---|---|---|
| Core backend | Shared Rust library for NVAPI/NVML domains and CLI consumers. | Gives the repo a common implementation surface instead of duplicating backend calls. | Hardware-specific behavior still varies by GPU generation, driver, and OS. |
| NVOC-CLI | Function-style commands for discovery, status, V-F curve reads, offsets, limits, fan controls, clock locks, voltage locks, and backend selection. | Provides the lowest-friction automation and debugging interface. | Some support-matrix rows remain TODO/unknown, especially NVML autoscan paths and server/workstation edge cases. |
| Auto optimizer | Autoscan, V-F curve export/import, result fixing, retained VFP reset workflows, legacy workflows, crash recovery, breakpoint resumption, and CUDA/OpenCL stress integration. | Main user-facing tuning workflow is present. | Needs final release-gate validation across supported GPU families and backends. |
| CUDA Rust stressor | CUDA stress workloads with build-time CUDA features and short GPU CI coverage. | Recommended native stress path for CUDA-capable systems. | Release packaging needs CUDA 12.x/13.x split or equivalent compatibility guidance. |
| OpenCL stressor | Lightweight Python/OpenCL stress path. | Broad fallback for systems without CUDA setup. | Linux OpenCL GPU CI is not currently active. |
| GUI | Python desktop frontend for dashboard, autoscan, overclocking, V-F curve, fan, and console workflows. | Makes the tool approachable for interactive Windows/Linux users. | Requires packaging validation and dependency/install smoke tests. |
| TUI | Textual frontend for dashboard, autoscan, overclock, V-F curve, and console workflows. | Supports SSH/server use where GUI is unavailable. | Depends on external CLI availability and tolerant parsing; packaging should be verified. |
| Service | Windows service and localhost HTTP control layer. | Enables managed or long-running host control. | Needs service-install, stop, recovery, and security review before stable release. |
| CI | Normal CI covers formatting, linting, Rust/Python tests, component-specific jobs, and a gated GPU CI workflow. | Gives a practical pre-release signal. | GPU CI is intentionally opt-in and read-only for dangerous paths, so manual bench validation remains mandatory. |
| Release governance | Release/tag guard workflow and documented protected-tag ruleset. | Prevents accidental releases away from `sprint`. | Repository rules must be configured in GitHub before relying on this policy. |

## Current bugs, gaps, and risks to sprint-plan

| Priority | Type | Item | Why it matters | Suggested owner area | Target sprint |
|---|---|---|---|---|---|
| P0 | Release governance | Configure GitHub repository rulesets so protected `v*` tags require the release/tag guard status check. | The workflow alone cannot prevent unauthorized tags unless GitHub rules enforce it. | CI / Maintainers | Sprint 0 |
| P0 | Safety | Run full release-gate validation on a recoverable GPU bench, including read-only GPU CI and human-supervised mutating smoke tests. | Normal CI cannot prove write-path safety for overclocking operations. | Core / Auto optimizer | Sprint 0 |
| P0 | Safety | Publish an operator recovery checklist covering driver reset, TDR/reboot behavior, retained VFP reset, and how to undo applied settings. | Users need a visible escape path before trying risky writes. | Docs / Auto optimizer | Sprint 0 |
| P1 | Packaging | Define release artifacts for Windows and Linux, including which binaries are shipped together. | Users need predictable installs for CLI, auto-optimizer, stressors, GUI, TUI, and service. | Build / Release | Sprint 1 |
| P1 | Compatibility | Resolve or explicitly document TODO rows for NVML autoscan and legacy autoscan support. | A release should not imply unsupported backend combinations are ready. | CLI / Auto optimizer | Sprint 1 |
| P1 | Compatibility | Decide CUDA 12.x vs CUDA 13.x artifact strategy for legacy and newer GPU coverage. | A single CUDA build may exclude users depending on architecture and toolkit support. | CUDA stressor / Release | Sprint 1 |
| P1 | Testing | Add or document Linux OpenCL GPU validation, or mark it out of scope for the first release. | OpenCL is the broad fallback path; missing Linux validation should be explicit. | OpenCL stressor / CI | Sprint 1 |
| P2 | UX | Add release-oriented GUI/TUI smoke scripts or checklists for packaged builds. | Frontends can fail from packaging/resource issues even when unit tests pass. | GUI / TUI | Sprint 2 |
| P2 | Service | Complete service security and lifecycle review: install, stop, access binding, failure recovery, and logs. | A local HTTP/service surface should have a clear threat model before stable release. | SRV | Sprint 2 |
| P2 | Documentation | Keep `README.md`, `docs/wiki`, component READMEs, and compatibility tables synchronized before tagging. | Conflicting docs create unsafe assumptions for users. | Docs | Sprint 2 |
| P3 | Product | Define post-release telemetry-free feedback templates for hardware matrix reports. | Hardware projects need structured user reports without collecting private data. | Maintainers | Sprint 3 |

## Sprint roadmap

### Sprint 0 — Release gate and triage freeze

**Goal:** decide whether the current branch may receive an alpha/pre-release tag and prevent accidental stable releases.

**Scope:**

- Enable repository rules for protected `v*` tags and require the release/tag guard check.
- Freeze feature intake except fixes to release blockers.
- Run normal CI and GPU CI on the exact candidate commit from `sprint`.
- Execute a manual bench checklist for mutating paths on known recoverable hardware:
  - read GPU information and status;
  - export V-F curve;
  - apply and reset safe small core/memory offsets;
  - apply and reset safe power/fan settings where supported;
  - run a short stress validation;
  - verify retained settings can be reset.
- Produce release notes that clearly label alpha limitations.

**Exit criteria:**

- `sprint` is the only permitted release source.
- No P0 safety or governance blockers remain.
- Maintainers choose one of:
  - tag `v0.1.0-alpha.1`; or
  - defer tagging and continue to Sprint 1.

### Sprint 1 — Alpha hardening and packaging

**Goal:** turn the current development state into a repeatable alpha release that users can install and test safely.

**Scope:**

- Define artifact matrix:
  - `nvoc-cli`;
  - `nvoc-auto-optimizer`;
  - CUDA Rust stressor artifacts;
  - OpenCL stressor package or documented Python environment;
  - GUI package;
  - TUI package;
  - optional Windows service artifact.
- Decide CUDA artifact compatibility strategy, including whether to publish separate CUDA 12.x and CUDA 13.x builds.
- Replace unknown/TODO compatibility entries with one of: supported, unsupported, experimental, or untested.
- Add release smoke checklists for Windows and Linux installs.
- Document backend assumptions for NVAPI/NVML fallback behavior.

**Exit criteria:**

- A contributor can rebuild release artifacts from documented commands.
- Unsupported GPU/backend combinations are explicit.
- Alpha release notes include known limitations and recovery instructions.

### Sprint 2 — Beta stabilization

**Goal:** make behavior predictable enough for a broader beta.

**Scope:**

- Expand hardware matrix validation across at least one recent consumer GPU, one older consumer GPU, and one workstation/server-class GPU if available.
- Add regression tests around recently fixed bugs, especially autoscan/VFP panic cases, service stop handling, GUI sentinel reset behavior, and CUDA stressor performance paths.
- Validate GUI/TUI packaged resource loading.
- Review service security posture and local API binding.
- Reduce noisy or ambiguous CLI output so GUI/TUI parsers have stable inputs.

**Exit criteria:**

- No known P0/P1 safety issues.
- Beta artifact install and smoke tests pass on at least Windows plus one Linux environment.
- Hardware matrix report is published with tested driver versions.

### Sprint 3 — Stable release candidate

**Goal:** prepare a stable SemVer release candidate.

**Scope:**

- Lock release scope and block new features.
- Run all normal CI, GPU CI, packaging smoke tests, and manual recovery tests on the candidate commit.
- Audit documentation for stale commands, stale feature claims, and missing warnings.
- Finalize changelog, checksums, artifact names, and upgrade/rollback notes.

**Exit criteria:**

- Candidate can be tagged `v1.0.0-rc.1` from `sprint`.
- Stable `v1.0.0` is allowed only after the RC receives no release-blocking regressions during the agreed soak window.

### Sprint 4 — Post-stable follow-up

**Goal:** improve coverage and user confidence after the first stable tag.

**Scope:**

- Backfill lower-priority compatibility gaps.
- Add issue templates for hardware reports, crash/recovery reports, and backend support requests.
- Improve automated packaging and release-note generation.
- Consider expanding GPU CI labels/runners for more hardware classes.

**Exit criteria:**

- Maintainers can plan minor releases from structured feedback rather than ad hoc reports.

## Release checklist

Before any release tag:

- [ ] Candidate commit is on `sprint`.
- [ ] Release/tag guard workflow exists and repository rules require it for `v*` tags.
- [ ] Normal CI is green.
- [ ] GPU CI has run on the candidate commit, or release notes explicitly mark GPU CI as unavailable and explain why.
- [ ] Manual mutating smoke tests were run on recoverable hardware.
- [ ] Release notes list supported operating systems, GPU classes, drivers/toolkits, and unsupported features.
- [ ] Recovery and rollback instructions are linked from the release notes.
- [ ] Artifacts and checksums are attached or documented.
- [ ] Stable tags are avoided until alpha/beta/RC exit criteria are met.

## Issue organization labels

Use these labels when turning this roadmap into GitHub issues:

- `sprint:0-release-gate`, `sprint:1-alpha`, `sprint:2-beta`, `sprint:3-rc`, `sprint:4-post-stable`
- `area:core`, `area:cli`, `area:auto-optimizer`, `area:cuda-stressor`, `area:opencl-stressor`, `area:gui`, `area:tui`, `area:srv`, `area:ci`, `area:docs`, `area:release`
- `kind:bug`, `kind:feature`, `kind:safety`, `kind:compatibility`, `kind:packaging`, `kind:docs`
- `priority:P0`, `priority:P1`, `priority:P2`, `priority:P3`
