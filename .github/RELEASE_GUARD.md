# Release and tag guard

The `Release and tag guard` workflow runs for `v*` tag pushes and release events. It enforces two rules:

1. Release/tag names must be SemVer-like and start with `v`, for example `v0.1.0-alpha.1` or `v1.0.0`.
2. The resolved release/tag target commit must be reachable from `origin/sprint`.

## Required repository ruleset

Configure a GitHub repository ruleset for protected tags before relying on release tags:

- Target: tags matching `v*`.
- Require status checks to pass.
- Required check: `Require release targets from sprint` from the `Release and tag guard` workflow.
- Restrict bypass permissions to maintainers who are responsible for release recovery.

Without the repository ruleset, the workflow can report a failing status but cannot by itself prevent a tag or release from being created.
