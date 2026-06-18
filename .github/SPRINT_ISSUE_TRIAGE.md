# Sprint issue triage plan

This plan is based on the open GitHub issues visible on 2026-06-18. It assigns sprint labels that maintainers can apply directly with `tools/sprint_issue_triage.py`.

## Label set

| Label | Purpose |
|---|---|
| `sprint:0-release-gate` | Release governance, release blocking safety, and tag policy work. |
| `sprint:1-alpha` | Alpha hardening, critical bug fixes, and compatibility decisions. |
| `sprint:2-beta` | Beta stabilization, UX polish, packaging, and refactor follow-up. |
| `sprint:3-rc` | Stable release-candidate cleanup and release-note finalization. |
| `sprint:4-post-stable` | Post-stable expansion and non-blocking roadmap items. |
| `priority:P0` | Blocks any public pre-release or safe release process. |
| `priority:P1` | Blocks alpha quality or core release confidence. |
| `priority:P2` | Blocks beta/stable polish but not an alpha preview. |
| `priority:P3` | Non-blocking product exploration or future expansion. |

## Current issue assignments

| Issue | Current title | Sprint labels to add | Why |
|---|---|---|---|
| #219 | 我们是不是应该锁release和tag？ | `sprint:0-release-gate`, `priority:P0`, `area:release`, `kind:safety` | Release/tag locking is the immediate release governance blocker. |
| #190 | 内存泄漏 | `sprint:1-alpha`, `priority:P1`, `area:gui`, `kind:bug` | A memory leak should be triaged before broad alpha use, but does not by itself block release governance setup. |
| #187 | 竞品分析 | `sprint:4-post-stable`, `priority:P3`, `kind:feature` | Competitive analysis is useful product planning, not a pre-release blocker. |
| #185 | gui: finish TUI-aligned refactor follow-up | `sprint:2-beta`, `priority:P2`, `area:gui`, `kind:tech-debt` | GUI/TUI alignment matters for beta polish and maintainability. |
| #180 | NVAPI 和 NVML 支持的功能能做到 1:1 吗 | `sprint:1-alpha`, `priority:P1`, `area:cli`, `kind:compatibility` | Backend parity or explicitly documented non-parity is needed before a credible alpha. |
| #161 | [Bug]: 极端混合压测下产生虚假 "code #1"不过测，底层 FECS 挂起导致级联降频测试失败 | `sprint:1-alpha`, `priority:P1`, `area:auto-optimizer`, `kind:bug` | False stress failures can invalidate autoscan results and must be bounded before alpha. |
| #156 | 统一单位、编号 | `sprint:2-beta`, `priority:P2`, `area:cli`, `kind:ux` | Unit and numbering consistency improves beta usability and docs. |
| #153 | INFOMATION ISSUE: 如何使用 Ajax Codex | `sprint:4-post-stable`, `priority:P3`, `kind:docs` | Informational/process issue; not release blocking. |
| #146 | 画饼: 同时支持 nova GPU 驱动 | `sprint:4-post-stable`, `priority:P3`, `kind:feature` | New driver support is future expansion, not first-release scope. |
| #142 | 自动超频压力测试严格化 | `sprint:1-alpha`, `priority:P1`, `area:auto-optimizer`, `kind:safety` | Stricter stress validation directly affects release confidence for autoscan. |
| #5 | 自动超频扫描架构优化 | `sprint:2-beta`, `priority:P2`, `area:auto-optimizer`, `kind:tech-debt` | Architecture optimization is important but should follow alpha safety triage unless a specific blocker is found. |

## Feature ship decision

| Feature area | Ship in next pre-release? | Rationale |
|---|---|---|
| Read-only GPU discovery/status and V-F curve export | Yes, as alpha | Read-only paths are the safest useful entry point and can be validated by normal/GPU CI plus smoke tests. |
| `nvoc-cli` manual setting writes | Yes, as alpha with warnings | Ship only with explicit backend/platform support notes and recovery instructions. |
| Auto optimizer autoscan | Yes, as experimental alpha | Useful core workflow, but release notes must call out stress false-positive risk and hardware validation limits. |
| CUDA Rust stressor | Yes, as alpha | Ship with CUDA toolkit/artifact compatibility notes and short stress smoke coverage. |
| OpenCL stressor | Yes, as alpha fallback | Ship as a fallback path while marking Linux OpenCL GPU CI as not yet complete. |
| GUI and TUI | Yes, as alpha frontends | Frontends can ship as alpha if they clearly depend on external CLI binaries and packaged smoke checks are documented. |
| Windows service / localhost control | No stable ship; optional experimental artifact only | Service lifecycle and security review should be completed before beta/stable claims. |
| NVML autoscan parity and nova driver support | No | These are roadmap/compatibility items and should not be advertised as shipped capability yet. |

## Maintainer action

Run the triage tool in dry-run mode first:

```bash
python3 tools/sprint_issue_triage.py --repo Skyworks-Neo/nvoc
```

After confirming the output, apply labels with a token that can edit issues:

```bash
GITHUB_TOKEN=<token> python3 tools/sprint_issue_triage.py --repo Skyworks-Neo/nvoc --apply
```
