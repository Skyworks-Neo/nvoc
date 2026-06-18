#!/usr/bin/env python3
"""Validate release-management docs and sprint issue triage metadata."""

from __future__ import annotations

import re
import sys
from pathlib import Path

import sprint_issue_triage as triage

ROOT = Path(__file__).resolve().parents[1]
TRIAGE_DOC = ROOT / ".github" / "SPRINT_ISSUE_TRIAGE.md"
RELEASE_GUARD_DOC = ROOT / ".github" / "RELEASE_GUARD.md"
ROADMAP_DOC = ROOT / "docs" / "wiki" / "Release-Roadmap.md"
HOME_DOCS = [ROOT / "docs" / "wiki" / "Home.md", ROOT / "docs" / "wiki" / "Home-zh.md"]


class ValidationError(RuntimeError):
    """Raised when release-management metadata is inconsistent."""


def validate_unique_labels() -> None:
    names = [label.name for label in triage.LABELS]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        raise ValidationError(f"duplicate labels: {', '.join(duplicates)}")


def validate_issue_labels() -> None:
    known = {label.name for label in triage.LABELS}
    for issue, labels in triage.ISSUE_LABELS.items():
        missing = sorted(set(labels) - known)
        if missing:
            raise ValidationError(
                f"#{issue} uses undefined labels: {', '.join(missing)}"
            )
        if not any(label.startswith("sprint:") for label in labels):
            raise ValidationError(f"#{issue} has no sprint label")
        if not any(label.startswith("priority:") for label in labels):
            raise ValidationError(f"#{issue} has no priority label")


def validate_triage_doc() -> None:
    text = TRIAGE_DOC.read_text(encoding="utf-8")
    for label in triage.LABELS:
        if f"`{label.name}`" not in text:
            raise ValidationError(f"{TRIAGE_DOC} does not document label {label.name}")
    for issue, labels in triage.ISSUE_LABELS.items():
        if f"| #{issue} |" not in text:
            raise ValidationError(f"{TRIAGE_DOC} does not document issue #{issue}")
        for label in labels:
            issue_row = next(
                line for line in text.splitlines() if line.startswith(f"| #{issue} |")
            )
            if f"`{label}`" not in issue_row:
                raise ValidationError(f"{TRIAGE_DOC} row for #{issue} omits {label}")


def validate_local_markdown_links() -> None:
    for path in [TRIAGE_DOC, RELEASE_GUARD_DOC, ROADMAP_DOC, *HOME_DOCS]:
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(r"\[[^\]]+\]\(([^)]+)\)", text):
            link = match.group(1)
            if link.startswith(("http://", "https://", "#", "mailto:")):
                continue
            target_path = link.split("#", 1)[0]
            if not target_path:
                continue
            target = (path.parent / target_path).resolve()
            if not target.exists():
                raise ValidationError(f"{path}: missing local link target {link}")


def main() -> int:
    try:
        validate_unique_labels()
        validate_issue_labels()
        validate_triage_doc()
        validate_local_markdown_links()
    except ValidationError as exc:
        print(f"release-management validation failed: {exc}", file=sys.stderr)
        return 1
    print("release-management validation ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
