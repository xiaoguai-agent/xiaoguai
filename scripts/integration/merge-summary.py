#!/usr/bin/env python3
"""merge-summary.py — merges conflicting versions of docs/book/src/SUMMARY.md.

Reads the conflicted SUMMARY.md (or multiple branch versions via git show),
parses the mdBook section structure, deduplicates entries by (title, link),
and writes a merged file with section order preserved.

Usage:
    # Resolve a conflict already in the working tree:
    python scripts/integration/merge-summary.py

    # Merge from specific branches:
    python scripts/integration/merge-summary.py --branches feat/a feat/b

    # Dry run — print to stdout only:
    python scripts/integration/merge-summary.py --dry-run
"""
import re
import subprocess
import sys
from pathlib import Path

SUMMARY_PATH = "docs/book/src/SUMMARY.md"
CONFLICT_PAT = re.compile(
    r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> [^\n]+",
    re.DOTALL,
)
# Matches mdBook list entries: optional indent, "- [Title](link)"
ENTRY_PAT = re.compile(r"^(\s*)-\s+\[([^\]]+)\]\(([^)]+)\)")
# Section divider
SECTION_PAT = re.compile(r"^#\s+.+")


def parse_sections(text: str) -> list[tuple[str, list[str]]]:
    """Returns list of (section_header, [raw_lines])."""
    sections: list[tuple[str, list[str]]] = []
    current_header = ""
    current_lines: list[str] = []
    for line in text.splitlines(keepends=True):
        if SECTION_PAT.match(line.rstrip()):
            if current_lines or current_header:
                sections.append((current_header, current_lines))
            current_header = line
            current_lines = []
        else:
            current_lines.append(line)
    if current_lines or current_header:
        sections.append((current_header, current_lines))
    return sections


def dedupe_lines(lines_sets: list[list[str]]) -> list[str]:
    """Merge line lists, deduplicating mdBook entries by (title, link). Non-entry lines kept as-is."""
    seen: set[tuple[str, str]] = set()
    result: list[str] = []
    for lines in lines_sets:
        for line in lines:
            m = ENTRY_PAT.match(line.rstrip("\n"))
            if m:
                key = (m.group(2), m.group(3))
                if key in seen:
                    continue
                seen.add(key)
            result.append(line)
    return result


def merge_texts(texts: list[str]) -> str:
    """Merge multiple SUMMARY.md texts into one."""
    all_sections: dict[str, list[list[str]]] = {}
    section_order: list[str] = []

    for text in texts:
        for header, lines in parse_sections(text):
            if header not in all_sections:
                section_order.append(header)
                all_sections[header] = []
            all_sections[header].append(lines)

    merged_parts: list[str] = []
    for header in section_order:
        if header:
            merged_parts.append(header)
        merged_parts.extend(dedupe_lines(all_sections[header]))

    return "".join(merged_parts)


def load_from_branch(branch: str) -> str | None:
    result = subprocess.run(
        ["git", "show", f"{branch}:{SUMMARY_PATH}"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"  WARN  branch '{branch}' has no {SUMMARY_PATH} — skipping")
        return None
    return result.stdout


def resolve_conflict_in_file(text: str) -> list[str]:
    """Extract all conflict sides from a file with conflict markers."""
    sides: list[str] = []
    head_parts: list[str] = []
    incoming_parts: list[str] = []

    for m in CONFLICT_PAT.finditer(text):
        head_parts.append(m.group(1))
        incoming_parts.append(m.group(2))

    if not head_parts:
        return [text]

    # Rebuild two full texts from the non-conflicted base + each conflict side
    base = CONFLICT_PAT.sub("", text)
    sides.append(base + "".join(head_parts))
    sides.append(base + "".join(incoming_parts))
    return sides


def main() -> int:
    args = sys.argv[1:]
    dry_run = "--dry-run" in args
    use_branches = "--branches" in args

    summary_file = Path(SUMMARY_PATH)
    if not summary_file.exists():
        print(f"ERROR: {SUMMARY_PATH} not found")
        return 1

    texts: list[str] = []

    if use_branches:
        idx = args.index("--branches")
        branches = [a for a in args[idx + 1:] if not a.startswith("--")]
        if not branches:
            print("ERROR: --branches requires at least one branch name")
            return 1
        # Include working-tree version
        texts.append(summary_file.read_text(encoding="utf-8"))
        for branch in branches:
            t = load_from_branch(branch)
            if t:
                texts.append(t)
    else:
        raw = summary_file.read_text(encoding="utf-8")
        if "<<<<<<< HEAD" in raw:
            texts = resolve_conflict_in_file(raw)
            print(f"Conflict markers found — extracted {len(texts)} version(s)")
        else:
            print("No conflict markers in file — nothing to merge.")
            return 0

    merged = merge_texts(texts)

    if dry_run:
        print("--- MERGED SUMMARY.md (dry-run) ---")
        print(merged[:2000])
        if len(merged) > 2000:
            print(f"... ({len(merged)} total chars)")
        return 0

    summary_file.write_text(merged, encoding="utf-8")
    print(f"Written: {SUMMARY_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
