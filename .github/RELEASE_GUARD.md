# Release and tag guard

The `Release and tag guard` workflow runs for `v*` tag pushes, release events, and pushes to `main`. It enforces three rules:

1. Release/tag names must be SemVer-like and start with `v`, for example `v0.1.0-alpha.1` or `v1.0.0`.
2. The resolved release/tag target commit must be reachable from `origin/main`.
3. The tag must equal the workspace version at the tagged commit (`.github/scripts/check-versions.py`).

## How the ruleset's required check actually gates tag creation

A tag ruleset evaluates required status checks on the target commit **before the tag exists**, so a check produced only by tag pushes can never satisfy it — every tag push would be rejected ("check missing"), regardless of correctness. That is why the guard also runs on every push to `main` and succeeds trivially there: each main commit carries the `Require release targets from main` check, so

- tags pointing at a main commit are **allowed** at creation time;
- tags pointing at any other commit (feature branches, test tags) are **rejected** at creation time for everyone off the bypass list.

The tag-push run of the guard then performs the full validation (SemVer shape, reachability, version match) as a visible post-hoc alarm, and the release workflow independently re-checks before building artifacts.

To test the release pipeline without a tag, use the Release workflow's `workflow_dispatch` dry run (it builds and smoke-tests but can never attest or publish), e.g. `gh workflow run Release --ref <branch>`.

## Required repository ruleset

Configure a GitHub repository ruleset for protected tags before relying on release tags:

- Target: tags matching `v*`.
- Require status checks to pass.
- Required check: `Require release targets from main` from the `Release and tag guard` workflow.
- Restrict bypass permissions to maintainers who are responsible for release recovery.

Without the repository ruleset, the workflow can report a failing status but cannot by itself prevent a tag or release from being created.
