# Release process

NVOC uses one version and one `vX.Y.Z[-pre.N]` tag for the entire repository.
Publishing remains a deliberate maintainer action: automation creates a draft,
but never publishes it.

## Version contract

- `[workspace.package].version` in the root `Cargo.toml` is the source of
  truth. Every Rust crate inherits it with `version.workspace = true`.
- `gui`, `tui`, `nvoc-python`, and `cli-stressor-opencl` use the equivalent
  PEP 440 version (`X.Y.Z-alpha.N` becomes `X.Y.ZaN`).
- `.github/scripts/check-versions.py` checks the Rust and Python versions in
  CI. For a release, it also requires the tag to equal `v<workspace version>`.

## Repository release controls

- The active `v*` tag ruleset requires the `Require release targets from main`
  check and blocks tag updates. See [`RELEASE_GUARD.md`](./RELEASE_GUARD.md).
- The tag guard verifies the tag format, confirms that its commit is reachable
  from `origin/main`, and checks that the tag matches the workspace version.
  The release workflow repeats these checks before building.
- Release builds use GitHub-hosted runners only. Do not give self-hosted GPU
  runners authority to produce release artifacts.
- Only the final draft-creation job receives `contents: write`; it must first
  pass the required-reviewer gate on the `release` environment.

## Release workflow

`.github/workflows/release.yml` provides two entry points:

- A manual `workflow_dispatch` builds and validates Linux and Windows artifacts
  for amd64 and arm64. It is always a dry run; even when pointed at a tag, it
  cannot create attestations or a release.
- A pushed release tag runs the same matrix, creates build-provenance
  attestations, waits for approval through the `release` environment, and
  creates a draft GitHub release. Publishing the draft remains manual.

The matrix produces:

| Deliverable | Linux amd64 | Linux arm64 | Windows amd64 | Windows arm64 |
|---|---:|---:|---:|---:|
| `nvoc-cli`, `nvoc-tui`, `nvoc-gui` | Yes | Yes | Yes | Yes |
| OpenCL stressor | Yes | Yes | Yes | No |
| Tools bundle: auto-optimizer and CUDA/Vulkan stressor | Yes | Yes | Yes | Yes |
| NVOC-SRV binaries inside the tools bundle | No | No | Yes | Yes |

Each matrix cell validates executable architecture, runs CLI smoke tests, and
emits a manifest plus per-cell checksums. Linux amd64 CLI artifacts are also
tested in Debian 12 and Ubuntu 22.04 containers. Tag builds attest the matrix
artifacts, then the publish job creates an aggregate `SHA256SUMS` file and a
draft GitHub release.

## Cutting a release

1. Update `[workspace.package].version` in the root `Cargo.toml`, mirror its
   PEP 440 form in the four Python `pyproject.toml` files, refresh `Cargo.lock`
   and `uv.lock`, and move the relevant `CHANGELOG.md` entries from
   `Unreleased` to the new version heading.
2. Run `python3 .github/scripts/check-versions.py`, open a PR, and merge only
   after the `version-consistency` and other required CI checks pass.
3. From the merged commit on `main`, run the **Release** workflow manually.
   Confirm all four matrix jobs pass and inspect the retained workflow
   artifacts. A dry run does not attest or create a GitHub release.
4. Create a signed tag on that exact `main` commit and push only that tag:

   ```bash
   git tag -s vX.Y.Z[-pre.N] <commit> -m "NVOC vX.Y.Z[-pre.N]"
   git push origin refs/tags/vX.Y.Z[-pre.N]
   ```

5. After the tag guard and release workflow pass, approve the `release`
   environment deployment. This permits the workflow to create the draft; it
   does not publish it.
6. Review the draft assets and replace the generated placeholder notes with
   the matching `CHANGELOG.md` section. Verify the aggregate checksums and at
   least one artifact's provenance:

   ```bash
   sha256sum --check SHA256SUMS
   gh attestation verify <artifact> --repo Skyworks-Neo/nvoc
   ```

7. Mark `-alpha.N`, `-beta.N`, and `-rc.N` versions as pre-releases, then
   publish the draft manually.

## Known limitations

- Windows artifacts are not Authenticode-signed, so SmartScreen warnings are
  expected. Checksums and GitHub build-provenance attestations provide the
  current integrity checks.
- The OpenCL stressor is omitted from Windows arm64 because pyopencl does not
  provide a `win_arm64` wheel.
- The Debian 12 / Ubuntu 22.04 container compatibility check currently covers
  Linux amd64 only; Linux arm64 is built and checked on GitHub's arm64 Ubuntu
  runner.
- NVIDIA driver, CUDA, and Vulkan runtime libraries are not bundled. Release
  artifacts load the applicable libraries from the target system; consult the
  component README files before use.
