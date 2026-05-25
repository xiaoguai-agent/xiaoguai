#!/usr/bin/env python3
"""keep-both.py — resolves git conflicts by keeping both HEAD and incoming sides.

Implements the merge-convoy pattern: for each conflicted file (from
`git diff --name-only --diff-filter=U`), applies a regex to capture HEAD and
incoming blocks, concatenates them, and writes the result back.

Lock files (Cargo.lock, pnpm-lock.yaml) are skipped — handle them separately.
"""
import re
import subprocess
import sys
from pathlib import Path

LOCK_FILES = {"Cargo.lock", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"}

CONFLICT_PAT = re.compile(
    r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> [^\n]+\n",
    re.DOTALL,
)


def get_conflicted_files() -> list[Path]:
    result = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=U"],
        capture_output=True,
        text=True,
        check=True,
    )
    paths = [Path(p.strip()) for p in result.stdout.splitlines() if p.strip()]
    return paths


def resolve_file(path: Path) -> str:
    """Returns 'resolved', 'skipped', or 'manual'."""
    if path.name in LOCK_FILES:
        print(f"  SKIP  {path}  (lock file — handle separately)")
        return "skipped"

    if not path.exists():
        print(f"  SKIP  {path}  (file not found)")
        return "skipped"

    original = path.read_text(encoding="utf-8", errors="replace")

    def keep_both(m: re.Match) -> str:
        head_side = m.group(1)
        incoming_side = m.group(2)
        # Avoid blank-line doubling when one side is empty
        if head_side.rstrip() == incoming_side.rstrip():
            return head_side
        return head_side + incoming_side

    resolved, count = CONFLICT_PAT.subn(keep_both, original)

    if count == 0:
        # Conflict markers present but regex didn't match — needs human
        if "<<<<<<< HEAD" in original:
            print(f"  MANUAL {path}  (conflict markers found but regex unmatched)")
            return "manual"
        # No conflicts at all
        print(f"  CLEAN  {path}  (no conflict markers)")
        return "resolved"

    path.write_text(resolved, encoding="utf-8")
    print(f"  OK     {path}  ({count} conflict block(s) resolved — both sides kept)")
    return "resolved"


def main(dry_run: bool = False) -> int:
    conflicted = get_conflicted_files()

    if not conflicted:
        print("No conflicted files found (git diff --diff-filter=U returned empty).")
        return 0

    print(f"Conflicted files: {len(conflicted)}")
    resolved_count = 0
    manual_needed: list[Path] = []

    for path in conflicted:
        if dry_run:
            print(f"  DRY-RUN  {path}")
            continue
        status = resolve_file(path)
        if status == "resolved":
            resolved_count += 1
        elif status == "manual":
            manual_needed.append(path)

    print()
    if dry_run:
        print(f"Dry-run complete — would process {len(conflicted)} file(s).")
        return 0

    print(f"Resolved: {resolved_count}  |  Need manual review: {len(manual_needed)}")
    if manual_needed:
        print("\nFiles requiring manual review:")
        for p in manual_needed:
            print(f"  {p}")
        return 1
    return 0


if __name__ == "__main__":
    dry_run = "--dry-run" in sys.argv
    sys.exit(main(dry_run=dry_run))
