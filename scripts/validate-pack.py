#!/usr/bin/env python3
"""
validate-pack.py — JSON Schema 2020-12 validator for xiaoguai pack manifests.

Reads a pack.yaml file, validates it against the pack manifest schema, and
prints structured errors with json-path and description. Exit 0 = valid,
Exit 1 = invalid.

Usage:
    python scripts/validate-pack.py <path-to-pack.yaml>
    python scripts/validate-pack.py packs/pr-review/pack.yaml

Dependencies:
    pip install pyyaml jsonschema
"""

import json
import sys
from pathlib import Path


def load_schema(schema_path: Path) -> dict:
    with open(schema_path, encoding="utf-8") as f:
        return json.load(f)


def load_pack(pack_path: Path) -> dict:
    try:
        import yaml
    except ImportError as e:
        print(f"ERROR: Missing dependency: {e}", file=sys.stderr)
        print("Install with: pip install pyyaml jsonschema", file=sys.stderr)
        sys.exit(2)

    with open(pack_path, encoding="utf-8") as f:
        content = yaml.safe_load(f)
    if content is None:
        raise ValueError("pack.yaml is empty or contains only comments")
    if not isinstance(content, dict):
        raise ValueError(f"pack.yaml must be a YAML mapping, got {type(content).__name__}")
    return content


def format_path(error) -> str:
    """Return a json-path-style string for the error location."""
    parts = list(error.absolute_path)
    if not parts:
        return "$"
    segments = []
    for part in parts:
        if isinstance(part, int):
            segments.append(f"[{part}]")
        else:
            segments.append(f".{part}")
    return "$" + "".join(segments)


def validate(pack_path: Path, schema_path: Path) -> list[str]:
    """
    Validate pack_path against schema_path.

    Returns a list of error strings (empty = valid).
    Raises on load/parse failures.
    """
    try:
        from jsonschema import Draft202012Validator, ValidationError
        from jsonschema.exceptions import SchemaError
    except ImportError as e:
        print(f"ERROR: Missing dependency: {e}", file=sys.stderr)
        print("Install with: pip install pyyaml jsonschema", file=sys.stderr)
        sys.exit(2)

    schema = load_schema(schema_path)
    pack = load_pack(pack_path)

    try:
        validator = Draft202012Validator(schema)
    except SchemaError as e:
        raise ValueError(f"Schema itself is invalid: {e.message}") from e

    errors = sorted(validator.iter_errors(pack), key=lambda e: list(e.absolute_path))

    results = []
    for error in errors:
        path = format_path(error)
        # jsonschema message can be verbose — trim context after newline
        message = error.message.split("\n")[0]
        results.append(f"{path}: {message}")
    return results


def main() -> None:
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <path-to-pack.yaml>", file=sys.stderr)
        sys.exit(2)

    pack_path = Path(sys.argv[1])
    if not pack_path.exists():
        print(f"ERROR: file not found: {pack_path}", file=sys.stderr)
        sys.exit(2)

    # Schema is always relative to repo root (two levels up from this script)
    script_dir = Path(__file__).resolve().parent
    repo_root = script_dir.parent
    schema_path = repo_root / "docs" / "api" / "schemas" / "pack.yaml.schema.json"

    if not schema_path.exists():
        print(f"ERROR: schema not found at {schema_path}", file=sys.stderr)
        print("Run: git checkout origin/docs/json-schemas-wave3 -- docs/api/schemas/pack.yaml.schema.json", file=sys.stderr)
        sys.exit(2)

    try:
        errors = validate(pack_path, schema_path)
    except (ValueError, OSError) as exc:
        print(f"ERROR loading {pack_path}: {exc}", file=sys.stderr)
        sys.exit(1)

    if errors:
        for err in errors:
            print(f"FAIL {pack_path} | {err}")
        sys.exit(1)
    else:
        print(f"PASS {pack_path}")
        sys.exit(0)


if __name__ == "__main__":
    main()
