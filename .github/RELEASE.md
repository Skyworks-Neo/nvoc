# Release procedure

Scope: repo-wide releases — one `vX.Y.Z[-pre.N]` tag builds and ships every
component. The pipeline is `release.yml` (from #224: a linux/windows ×
amd64/arm64 matrix — build → smoke → checksums → provenance → **draft**
release); publishing is always a human action.

## Version contract

- Single source of truth: `[workspace.package].version` in the root
  `Cargo.toml`. All crates inherit it (`version.workspace = true`).
- Python packages (`gui`, `tui`, `nvoc-python`, `cli-stressor-opencl`) carry
  the PEP 440 form (`0.2.0-alpha.1` → `0.2.0a1`).
- `.github/scripts/check-versions.py` enforces this in CI on every PR, and
  additionally that `tag == v<workspace version>` in the tag guard and the
  release workflow.

## One-time repository setup (admin)

1. Tag ruleset per [`RELEASE_GUARD.md`](./RELEASE_GUARD.md): protect `v*`
   tags, require the `Require release targets from main` check.
2. `release` environment (Settings → Environments): add required reviewers.
   The draft-release job waits for this approval.
3. Keep release builds on **GitHub-hosted runners only**. Never add the
   self-hosted GPU runners to `release.yml` — test machines must not be able
   to produce release artifacts.

## Cutting a release

1. **Bump**: edit the workspace version in the root `Cargo.toml`, mirror the
   PEP 440 form in the four `pyproject.toml` files, refresh lockfiles
   (`cargo update --workspace && uv lock`), and move `CHANGELOG.md`
   `Unreleased` items under the new version heading. Open a PR; the
   `version-consistency` CI job must pass.
2. **Dry run** (recommended for pipeline changes): run the *Release build*
   workflow via `workflow_dispatch` on the merged commit. This builds and
   smoke-tests everything but creates no release.
3. **Tag** the merge commit on `main`:
   `git tag vX.Y.Z[-pre.N] <sha> && git push origin --tags`.
   The tag guard verifies SemVer shape, main-reachability, and version match.
4. **Review the draft**: the workflow uploads per-cell artifacts (CLI/TUI/GUI
   single binaries, OpenCL stressor, `nvoc-tools` bundle with auto-optimizer,
   CUDA stressor and, on Windows, the srv service binaries) plus `SHA256SUMS`,
   and creates a draft release. Check the notes against `CHANGELOG.md`,
   spot-verify a checksum, and verify provenance:
   `gh attestation verify <file> --repo Skyworks-Neo/nvoc`.
5. **Publish** the draft. Pre-releases (`-alpha.N` / `-beta.N` / `-rc.N`)
   must be marked "pre-release" on the release page.

## Known limitations (revisit deliberately)

- No Windows Authenticode signing yet — SmartScreen warnings are expected;
  say so in the release notes. Provenance attestations + SHA256SUMS are the
  integrity story for now.
- The OpenCL stressor is not shipped for windows-arm64 (pyopencl publishes
  no win_arm64 wheel).
- CUDA/Vulkan runtime libraries are intentionally not bundled; artifacts
  load them from the user's system (see component READMEs).
