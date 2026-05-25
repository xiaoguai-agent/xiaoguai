#!/usr/bin/env python3
"""merge-catalog.py — merges catalog/skill_packs.json entries from multiple branches.

Usage:
    python scripts/integration/merge-catalog.py <branch1> [branch2 ...]

Reads catalog/skill_packs.json from each branch via `git show <branch>:catalog/skill_packs.json`.
Deduplicates entries by the `slug` field (first-seen wins; later branches only contribute
entries with new slugs). Writes merged result to the working-tree file.
"""
import json
import subprocess
import sys
from pathlib import Path

CATALOG_PATH = "catalog/skill_packs.json"


def load_from_branch(branch: str) -> dict:
    result = subprocess.run(
        ["git", "show", f"{branch}:{CATALOG_PATH}"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"  WARN  branch '{branch}' has no {CATALOG_PATH} — skipping")
        return {}
    return json.loads(result.stdout)


def merge_packs(sources: list[dict]) -> dict:
    """Merge pack lists; deduplication by 'slug' field (first-seen wins)."""
    seen_slugs: set[str] = set()
    merged_packs: list[dict] = []
    version = 1

    for source in sources:
        version = max(version, source.get("version", 1))
        for pack in source.get("packs", []):
            slug = pack.get("slug") or pack.get("id", "")
            if not slug:
                print(f"  WARN  entry without slug/id — skipping: {pack}")
                continue
            if slug not in seen_slugs:
                seen_slugs.add(slug)
                merged_packs.append(pack)
            else:
                print(f"  DEDUP slug='{slug}' already present — skipping duplicate")

    return {"version": version, "packs": merged_packs}


def main() -> int:
    branches = [a for a in sys.argv[1:] if not a.startswith("--")]
    dry_run = "--dry-run" in sys.argv

    if not branches:
        print("Usage: merge-catalog.py <branch1> [branch2 ...] [--dry-run]")
        return 1

    catalog_file = Path(CATALOG_PATH)
    if not catalog_file.exists():
        print(f"ERROR: {CATALOG_PATH} not found in working tree")
        return 1

    # Start with the working-tree version (current branch / merge target)
    base = json.loads(catalog_file.read_text(encoding="utf-8"))
    sources = [base]

    print(f"Base packs: {len(base.get('packs', []))}")

    for branch in branches:
        print(f"Loading from branch '{branch}'...")
        data = load_from_branch(branch)
        if data:
            sources.append(data)
            print(f"  found {len(data.get('packs', []))} packs")

    merged = merge_packs(sources)
    print(f"\nMerged total: {len(merged['packs'])} unique packs")

    if dry_run:
        print("Dry-run — not writing to disk.")
        print(json.dumps(merged, indent=2)[:800], "...")
        return 0

    catalog_file.write_text(json.dumps(merged, indent=2) + "\n", encoding="utf-8")
    print(f"Written: {CATALOG_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
