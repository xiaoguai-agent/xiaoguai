#!/usr/bin/env python3
"""dedupe-package-json.py — fix duplicate keys in frontend/admin-ui/package.json.

Background: multiple agents independently fixed the react-router-dom version,
leaving two entries under 'dependencies'. The file may also have syntax issues
(missing commas between duplicate entries). This script:
  1. Pre-processes the raw text to fix missing commas before duplicate keys.
  2. Parses using a pairs-hook that tracks duplicates (last value wins).
  3. Pretty-prints the clean result.

Usage:
    python scripts/integration/dedupe-package-json.py [--dry-run]
    python scripts/integration/dedupe-package-json.py path/to/package.json [--dry-run]
"""
import json
import re
import sys
from pathlib import Path

DEFAULT_PATH = "frontend/admin-ui/package.json"


def fix_missing_commas(raw: str) -> str:
    """Insert missing commas between adjacent string values on separate lines.

    Handles the pattern:
        "key": "val1"
        "key": "val2",
    where a comma was omitted after val1.
    """
    # Match a line ending with a string value (no trailing comma) followed by
    # a line starting with whitespace + a quoted key.
    pat = re.compile(
        r'("(?:[^"\\]|\\.)*")\s*\n(\s+"(?:[^"\\]|\\.)*"\s*:)'
    )
    return pat.sub(r'\1,\n\2', raw)


def pairs_hook(pairs: list[tuple[str, object]]) -> dict:
    """Record duplicate keys; last value wins."""
    result: dict = {}
    duplicates: list[tuple[str, object, object]] = []
    for key, value in pairs:
        if key in result:
            duplicates.append((key, result[key], value))
        result[key] = value
    # Attach duplicates list to result dict for later inspection
    result["__duplicates__"] = duplicates  # type: ignore[assignment]
    return result


def dedupe_json_file(path: Path, dry_run: bool) -> int:
    if not path.exists():
        print(f"ERROR: {path} not found")
        return 1

    raw = path.read_text(encoding="utf-8")

    # Step 1: fix missing commas (pre-parse)
    fixed = fix_missing_commas(raw)
    comma_fixes = fixed != raw
    if comma_fixes:
        print("  FIXED missing comma(s) between adjacent entries.")

    # Step 2: parse with duplicate tracking
    try:
        data = json.loads(fixed, object_pairs_hook=pairs_hook)
    except json.JSONDecodeError as exc:
        print(f"ERROR: JSON parse failed even after comma fix: {exc}")
        print("The file may have deeper syntax issues — inspect manually.")
        return 1

    # Collect all duplicates recursively
    all_dupes: list[tuple[str, object, object]] = []

    def collect_dupes(obj: object) -> object:
        if isinstance(obj, dict):
            dupes = obj.pop("__duplicates__", [])
            all_dupes.extend(dupes)
            return {k: collect_dupes(v) for k, v in obj.items() if k != "__duplicates__"}
        if isinstance(obj, list):
            return [collect_dupes(i) for i in obj]
        return obj

    clean_data = collect_dupes(data)

    if not all_dupes and not comma_fixes:
        print(f"No issues found in {path} — nothing to do.")
        return 0

    for key, old_val, new_val in all_dupes:
        print(f"  DEDUP key='{key}':")
        print(f"    dropped: {json.dumps(old_val)}")
        print(f"    kept:    {json.dumps(new_val)}")

    print(f"\nFound {len(all_dupes)} duplicate key(s).")

    if dry_run:
        print("Dry-run — not writing to disk.")
        print("Would write:")
        print(json.dumps(clean_data, indent=2)[:800])
        return 0

    path.write_text(json.dumps(clean_data, indent=2) + "\n", encoding="utf-8")
    print(f"Written: {path}")
    return 0


def main() -> int:
    args = sys.argv[1:]
    dry_run = "--dry-run" in args
    path_args = [a for a in args if not a.startswith("--")]

    target = Path(path_args[0]) if path_args else Path(DEFAULT_PATH)
    return dedupe_json_file(target, dry_run)


if __name__ == "__main__":
    sys.exit(main())
