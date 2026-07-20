#!/usr/bin/env python3
"""Enforce the repo-wide version contract.

Source of truth: [workspace.package].version in the root Cargo.toml (SemVer).
Checks:
  1. Every workspace crate inherits the workspace version (version.workspace = true).
  2. Every Python package version equals the PEP 440 form of the workspace version.
  3. With --tag vX.Y.Z[-pre] (used by release workflows), the tag matches exactly.

Usage: python3 .github/scripts/check-versions.py [--tag <tag>]
"""

import argparse
import re
import sys
import tomllib

CRATES = [
    "core",
    "cli-common",
    "cli",
    "auto-optimizer",
    "srv",
    "cli-stressor-cuda-rs",
    "nvoc-python",
]
PYPROJECTS = [
    "gui/pyproject.toml",
    "tui/pyproject.toml",
    "nvoc-python/pyproject.toml",
    "cli-stressor-opencl/pyproject.toml",
]

# SemVer pre-release -> PEP 440. Extend only when a new pre-release kind is used.
PRERELEASE_MAP = {"alpha": "a", "beta": "b", "rc": "rc"}


def semver_to_pep440(semver: str) -> str:
    m = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-([a-z]+)\.(\d+))?", semver)
    if not m:
        sys.exit(f"workspace version {semver!r} is not SemVer 'X.Y.Z[-kind.N]'")
    base, kind, num = m.groups()
    if kind is None:
        return base
    if kind not in PRERELEASE_MAP:
        sys.exit(f"unknown pre-release kind {kind!r} (known: {sorted(PRERELEASE_MAP)})")
    return f"{base}{PRERELEASE_MAP[kind]}{num}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="release tag to compare, e.g. v0.2.0-alpha.1")
    args = parser.parse_args()

    with open("Cargo.toml", "rb") as f:
        workspace = tomllib.load(f)["workspace"]["package"]["version"]
    expected_py = semver_to_pep440(workspace)
    errors = []

    for crate in CRATES:
        path = f"{crate}/Cargo.toml"
        with open(path, "rb") as f:
            pkg = tomllib.load(f)["package"]
        if pkg.get("version") != {"workspace": True}:
            errors.append(
                f"{path}: version must be inherited (version.workspace = true)"
            )

    for path in PYPROJECTS:
        with open(path, "rb") as f:
            version = tomllib.load(f)["project"]["version"]
        if version != expected_py:
            errors.append(f"{path}: version {version!r} != expected {expected_py!r}")

    if args.tag and args.tag != f"v{workspace}":
        errors.append(f"tag {args.tag!r} != workspace version 'v{workspace}'")

    if errors:
        print("Version consistency check failed:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        sys.exit(1)
    print(f"Version consistency OK: workspace {workspace} / python {expected_py}")


if __name__ == "__main__":
    main()
