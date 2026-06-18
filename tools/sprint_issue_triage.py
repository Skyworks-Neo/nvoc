#!/usr/bin/env python3
"""Create sprint labels and assign them to known NVOC GitHub issues.

Dry-run is the default. Set GITHUB_TOKEN and pass --apply to modify GitHub.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass


@dataclass(frozen=True)
class Label:
    name: str
    color: str
    description: str


LABELS = [
    Label(
        "sprint:0-release-gate",
        "b60205",
        "Release governance and release-blocking safety work",
    ),
    Label("sprint:1-alpha", "d93f0b", "Alpha hardening and compatibility decisions"),
    Label(
        "sprint:2-beta",
        "fbca04",
        "Beta stabilization, packaging, UX, and refactor polish",
    ),
    Label(
        "sprint:3-rc",
        "0e8a16",
        "Stable release-candidate cleanup and final release notes",
    ),
    Label(
        "sprint:4-post-stable",
        "1d76db",
        "Post-stable expansion and non-blocking roadmap work",
    ),
    Label(
        "priority:P0", "b60205", "Blocks any public pre-release or safe release process"
    ),
    Label("priority:P1", "d93f0b", "Blocks alpha quality or core release confidence"),
    Label("priority:P2", "fbca04", "Blocks beta/stable polish but not alpha preview"),
    Label(
        "priority:P3", "c5def5", "Non-blocking product exploration or future expansion"
    ),
    Label("area:release", "5319e7", "Release process, tags, artifacts, and notes"),
    Label("area:cli", "5319e7", "nvoc-cli and command-line backend behavior"),
    Label("area:gui", "5319e7", "Python GUI frontend"),
    Label("area:auto-optimizer", "5319e7", "Auto optimizer workflows and autoscan"),
    Label("kind:safety", "b60205", "Safety, recovery, or dangerous-operation controls"),
    Label("kind:bug", "d73a4a", "Incorrect behavior or regression"),
    Label("kind:feature", "a2eeef", "New capability or product expansion"),
    Label(
        "kind:compatibility",
        "bfdadc",
        "Backend, OS, GPU, driver, or toolkit compatibility",
    ),
    Label("kind:tech-debt", "cccccc", "Refactor, cleanup, or maintainability work"),
    Label("kind:ux", "c2e0c6", "User experience, consistency, or usability"),
    Label("kind:docs", "0075ca", "Documentation or process information"),
]

ISSUE_LABELS: dict[int, list[str]] = {
    219: ["sprint:0-release-gate", "priority:P0", "area:release", "kind:safety"],
    190: ["sprint:1-alpha", "priority:P1", "area:gui", "kind:bug"],
    187: ["sprint:4-post-stable", "priority:P3", "kind:feature"],
    185: ["sprint:2-beta", "priority:P2", "area:gui", "kind:tech-debt"],
    180: ["sprint:1-alpha", "priority:P1", "area:cli", "kind:compatibility"],
    161: ["sprint:1-alpha", "priority:P1", "area:auto-optimizer", "kind:bug"],
    156: ["sprint:2-beta", "priority:P2", "area:cli", "kind:ux"],
    153: ["sprint:4-post-stable", "priority:P3", "kind:docs"],
    146: ["sprint:4-post-stable", "priority:P3", "kind:feature"],
    142: ["sprint:1-alpha", "priority:P1", "area:auto-optimizer", "kind:safety"],
    5: ["sprint:2-beta", "priority:P2", "area:auto-optimizer", "kind:tech-debt"],
}


class GitHubClient:
    def __init__(self, repo: str, token: str | None) -> None:
        self.repo = repo
        self.token = token

    def request(
        self, method: str, path: str, payload: dict[str, object] | None = None
    ) -> object:
        data = None if payload is None else json.dumps(payload).encode()
        request = urllib.request.Request(
            f"https://api.github.com/repos/{self.repo}{path}",
            data=data,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Content-Type": "application/json",
                "User-Agent": "nvoc-sprint-issue-triage",
                **({"Authorization": f"Bearer {self.token}"} if self.token else {}),
            },
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read()
        return json.loads(body) if body else {}

    def ensure_label(self, label: Label) -> None:
        try:
            self.request(
                "POST",
                "/labels",
                {
                    "name": label.name,
                    "color": label.color,
                    "description": label.description,
                },
            )
            print(f"created label {label.name}")
        except urllib.error.HTTPError as exc:
            if exc.code != 422:
                raise
            self.request(
                "PATCH",
                f"/labels/{urllib.parse.quote(label.name, safe='')}",
                {
                    "new_name": label.name,
                    "color": label.color,
                    "description": label.description,
                },
            )
            print(f"updated label {label.name}")

    def add_issue_labels(self, issue: int, labels: list[str]) -> None:
        self.request("POST", f"/issues/{issue}/labels", {"labels": labels})
        print(f"labeled #{issue}: {', '.join(labels)}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo", default="Skyworks-Neo/nvoc", help="GitHub repo in owner/name form"
    )
    parser.add_argument(
        "--apply", action="store_true", help="Modify GitHub labels and issues"
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    token = os.environ.get("GITHUB_TOKEN")
    if args.apply and not token:
        print("error: --apply requires GITHUB_TOKEN", file=sys.stderr)
        return 2

    if not args.apply:
        print("dry run: would create/update labels:")
        for label in LABELS:
            print(f"  - {label.name}: {label.description}")
        print("dry run: would label issues:")
        for issue, labels in ISSUE_LABELS.items():
            print(f"  - #{issue}: {', '.join(labels)}")
        return 0

    client = GitHubClient(args.repo, token)
    for label in LABELS:
        client.ensure_label(label)
    for issue, labels in ISSUE_LABELS.items():
        client.add_issue_labels(issue, labels)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
