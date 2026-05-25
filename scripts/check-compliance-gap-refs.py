#!/usr/bin/env python3
"""check-compliance-gap-refs.py

Verify that every G-NNN identifier cited in the six framework mapping documents
has a corresponding entry in docs/compliance/compliance-gaps.md (the master index).

Usage:
    python scripts/check-compliance-gap-refs.py

Exit codes:
    0  — all cited gap IDs are present in the master index
    1  — one or more cited gap IDs are missing from the master index

Intended to be added to CI as a gate after the per-framework docs are updated
to embed G-NNN references (follow-up to the initial unified index).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).parent.parent

MASTER_INDEX = REPO_ROOT / "docs" / "compliance" / "compliance-gaps.md"

FRAMEWORK_DOCS: list[Path] = [
    REPO_ROOT / "docs" / "compliance" / "soc2-mapping.md",
    REPO_ROOT / "docs" / "compliance" / "gdpr-mapping.md",
    REPO_ROOT / "docs" / "compliance" / "hipaa-mapping.md",
    REPO_ROOT / "docs" / "compliance" / "pci-dss-mapping.md",
    REPO_ROOT / "docs" / "compliance" / "iso27001-mapping.md",
    REPO_ROOT / "docs" / "compliance" / "eu-ai-act.md",
]

# Pattern that matches a gap identifier anywhere in text, e.g. "G-001", "G-022"
GAP_ID_PATTERN = re.compile(r"\bG-(\d{3})\b")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def extract_gap_ids(text: str) -> set[str]:
    """Return all unique G-NNN identifiers found in *text*."""
    return {f"G-{m}" for m in GAP_ID_PATTERN.findall(text)}


def load_master_ids() -> set[str]:
    """Parse the master index and return the set of declared gap IDs."""
    if not MASTER_INDEX.exists():
        print(f"ERROR: master index not found at {MASTER_INDEX}", file=sys.stderr)
        sys.exit(1)
    text = MASTER_INDEX.read_text(encoding="utf-8")
    return extract_gap_ids(text)


def check_framework_doc(doc_path: Path, master_ids: set[str]) -> list[str]:
    """Return a list of error strings for gap IDs cited in *doc_path* but absent
    from *master_ids*. Returns an empty list when the file does not exist (the
    framework doc may not yet be on this branch)."""
    if not doc_path.exists():
        print(
            f"  SKIP  {doc_path.relative_to(REPO_ROOT)} — file not present on this branch",
        )
        return []

    text = doc_path.read_text(encoding="utf-8")
    cited = extract_gap_ids(text)
    missing = sorted(cited - master_ids)

    if not cited:
        print(
            f"  INFO  {doc_path.relative_to(REPO_ROOT)} — no G-NNN references found"
            " (framework doc not yet wired to master index)",
        )
        return []

    errors: list[str] = []
    for gap_id in missing:
        errors.append(
            f"  FAIL  {doc_path.relative_to(REPO_ROOT)}: cites {gap_id}"
            " which has no entry in compliance-gaps.md",
        )
    if not missing:
        print(
            f"  OK    {doc_path.relative_to(REPO_ROOT)}"
            f" — {len(cited)} gap reference(s) all present in master index",
        )
    return errors


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    print(f"Master index: {MASTER_INDEX.relative_to(REPO_ROOT)}")
    master_ids = load_master_ids()
    print(f"  Found {len(master_ids)} gap IDs in master index: {', '.join(sorted(master_ids))}\n")

    print("Checking framework mapping documents:")
    all_errors: list[str] = []
    for doc_path in FRAMEWORK_DOCS:
        errors = check_framework_doc(doc_path, master_ids)
        all_errors.extend(errors)

    print()
    if all_errors:
        print(f"FAILED — {len(all_errors)} broken reference(s):")
        for err in all_errors:
            print(err)
        return 1

    print("PASSED — all cited gap IDs are present in the master index.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
