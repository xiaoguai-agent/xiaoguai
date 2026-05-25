#!/usr/bin/env python3
"""
validate-recipe.py — JSON Schema 2020-12 validator for Xiaoguai recipe YAML files.

Usage:
    python scripts/validate-recipe.py <path-to-recipe.yaml> [...]

Prints:
    PASS <path>
    FAIL <path> | <json-path>: <message>

Exit codes:
    0  all files valid
    1  one or more validation errors
    2  usage / dependency error

Dependencies:
    pip install 'jsonschema[format-nongpl]' pyyaml
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCHEMA_PATH = (
    Path(__file__).resolve().parent.parent
    / "docs"
    / "api"
    / "schemas"
    / "recipe.yaml.schema.json"
)


def _check_deps() -> None:
    """Fail fast with a clear message if required libraries are missing."""
    missing = []
    try:
        import jsonschema  # noqa: F401
    except ImportError:
        missing.append("jsonschema[format-nongpl]")
    try:
        import yaml  # noqa: F401
    except ImportError:
        missing.append("pyyaml")

    if missing:
        print(
            "ERROR: missing dependencies: " + ", ".join(missing) + "\n"
            "  pip install " + " ".join(f"'{p}'" for p in missing),
            file=sys.stderr,
        )
        sys.exit(2)


def _load_schema() -> dict:
    if not SCHEMA_PATH.exists():
        print(f"ERROR: schema not found at {SCHEMA_PATH}", file=sys.stderr)
        print(
            "Run: git checkout origin/docs/recipe-schema -- "
            "docs/api/schemas/recipe.yaml.schema.json",
            file=sys.stderr,
        )
        sys.exit(2)
    with SCHEMA_PATH.open(encoding="utf-8") as fh:
        return json.load(fh)


def _json_path(err) -> str:  # type: ignore[no-untyped-def]
    """Convert a jsonschema ValidationError into a JSON-path string."""
    parts = list(err.absolute_path)
    if not parts:
        return "$"
    segments: list[str] = []
    for part in parts:
        if isinstance(part, int):
            segments.append(f"[{part}]")
        else:
            segments.append(f".{part}")
    return "$" + "".join(segments)


def _load_yaml(file_path: str) -> object:
    import yaml  # imported here so _check_deps controls the error path

    with open(file_path, encoding="utf-8") as fh:
        return yaml.safe_load(fh)


def validate_file(file_path: str, schema: dict) -> list[str]:
    """
    Validate a single YAML recipe file against the recipe schema.

    Returns a list of error strings (empty list = file is valid).
    """
    from jsonschema import Draft202012Validator
    from jsonschema.exceptions import SchemaError

    path = str(file_path)

    try:
        data = _load_yaml(file_path)
    except Exception as exc:  # noqa: BLE001
        return [f"FAIL {path} | $: YAML parse error — {exc}"]

    if data is None:
        return [f"FAIL {path} | $: file is empty or contains only comments"]

    if not isinstance(data, dict):
        return [
            f"FAIL {path} | $: expected a YAML mapping, "
            f"got {type(data).__name__}"
        ]

    try:
        validator = Draft202012Validator(schema)
        schema_errors = sorted(
            validator.iter_errors(data), key=lambda e: list(e.absolute_path)
        )
    except SchemaError as exc:
        return [f"FAIL {path} | $: schema is invalid — {exc.message}"]

    errors: list[str] = []
    for err in schema_errors:
        # Trim verbose jsonschema context after the first newline
        message = err.message.split("\n")[0]
        errors.append(f"FAIL {path} | {_json_path(err)}: {message}")

    return errors


def main(argv: list[str]) -> int:
    _check_deps()

    if not argv:
        print(
            "ERROR: no files specified. Pass recipe YAML path(s) as arguments.\n"
            f"Usage: {sys.argv[0]} <recipe.yaml> [...]",
            file=sys.stderr,
        )
        return 2

    schema = _load_schema()

    total_pass = 0
    total_fail = 0

    for file_path in argv:
        if not Path(file_path).exists():
            print(f"FAIL {file_path} | $: file not found", flush=True)
            total_fail += 1
            continue

        errors = validate_file(file_path, schema)
        if errors:
            for line in errors:
                print(line, flush=True)
            total_fail += 1
        else:
            print(f"PASS {file_path}", flush=True)
            total_pass += 1

    print(
        f"\nSummary: {total_pass} passed, {total_fail} failed",
        file=sys.stderr,
    )
    return 0 if total_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
