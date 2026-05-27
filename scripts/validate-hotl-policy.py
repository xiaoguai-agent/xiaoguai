#!/usr/bin/env python3
"""
HotL policy JSON Schema 2020-12 validator.

Usage:
    python scripts/validate-hotl-policy.py [FILE ...]

Prints:
    PASS <path>
    PASS (negative-test) <path>  — file is named invalid-* and correctly fails validation
    FAIL <path> | <json-path>: <message>
    UNEXPECTED_PASS <path>       — file is named invalid-* but unexpectedly passed validation

Exit codes:
    0  all files valid (positive fixtures pass; negative fixtures correctly fail)
    1  one or more validation failures
    2  usage / dependency error

Negative-test fixtures:
    Files whose basename starts with "invalid-" are expected to fail validation.
    If they fail (as expected), the run is reported as PASS (negative-test).
    If they unexpectedly pass, the run is counted as a failure (UNEXPECTED_PASS).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCHEMA_PATH = (
    Path(__file__).parent.parent
    / "docs"
    / "api"
    / "schemas"
    / "hotl-policy.json.schema.json"
)

BUSINESS_RULE_MSG = (
    "at least one of max_count or max_usd must be non-null (both are null)"
)


def _check_deps() -> None:
    """Fail fast with a clear message if jsonschema is missing."""
    try:
        import jsonschema  # noqa: F401
    except ImportError:
        print(
            "ERROR: 'jsonschema' is not installed.\n"
            "  pip install 'jsonschema[format-nongpl]'",
            file=sys.stderr,
        )
        sys.exit(2)


def _load_schema() -> dict:
    if not SCHEMA_PATH.exists():
        print(f"ERROR: schema not found at {SCHEMA_PATH}", file=sys.stderr)
        sys.exit(2)
    with SCHEMA_PATH.open() as fh:
        return json.load(fh)


def _json_path(err) -> str:  # type: ignore[no-untyped-def]
    """Convert a jsonschema ValidationError into a JSON-path string."""
    parts = list(err.absolute_path)
    if not parts:
        return "$"
    return "$." + ".".join(str(p) for p in parts)


def _check_business_rules(data: dict, path: str) -> list[str]:
    """
    Check business rules that cannot be expressed purely in JSON Schema 2020-12.

    The schema allows max_count and max_usd to each be null independently, but
    the Rust store enforcer (InMemoryHotlPolicyStore::create) rejects the case
    where both are null simultaneously.
    """
    errors: list[str] = []
    max_count = data.get("max_count")
    max_usd = data.get("max_usd")
    if max_count is None and max_usd is None:
        errors.append(f"FAIL {path} | $: {BUSINESS_RULE_MSG}")
    return errors


def is_negative_fixture(file_path: str) -> bool:
    """Return True if this file is an intentionally-invalid negative test fixture.

    Convention: files whose basename starts with 'invalid-' are expected to
    fail validation. Their failure is asserted (not counted as a CI error).
    """
    return Path(file_path).name.startswith("invalid-")


def validate_file(file_path: str, schema: dict) -> list[str]:
    """
    Validate a single JSON file against the HotL policy schema.

    Returns a list of error strings. Empty list means the file is valid.
    """
    from jsonschema import Draft202012Validator
    from jsonschema.exceptions import SchemaError

    path = str(file_path)

    try:
        with open(file_path) as fh:
            data = json.load(fh)
    except json.JSONDecodeError as exc:
        return [f"FAIL {path} | $: JSON parse error — {exc}"]
    except OSError as exc:
        return [f"FAIL {path} | $: cannot read file — {exc}"]

    # Strip internal _comment key before validating (used in invalid examples)
    if isinstance(data, dict) and "_comment" in data:
        data = {k: v for k, v in data.items() if k != "_comment"}

    try:
        validator = Draft202012Validator(schema)
        schema_errors = sorted(
            validator.iter_errors(data), key=lambda e: list(e.absolute_path)
        )
    except SchemaError as exc:
        return [f"FAIL {path} | $: schema is invalid — {exc.message}"]

    errors: list[str] = []
    for err in schema_errors:
        errors.append(f"FAIL {path} | {_json_path(err)}: {err.message}")

    # Only run business-rule checks when structure is otherwise valid,
    # to avoid redundant noise on already-broken files.
    if not errors and isinstance(data, dict):
        errors.extend(_check_business_rules(data, path))

    return errors


def main(argv: list[str]) -> int:
    _check_deps()
    schema = _load_schema()

    files = argv if argv else []
    if not files:
        print(
            "ERROR: no files specified. "
            "Pass JSON file path(s) as arguments.",
            file=sys.stderr,
        )
        return 2

    total_pass = 0
    total_fail = 0

    for file_path in files:
        errors = validate_file(file_path, schema)
        negative = is_negative_fixture(file_path)

        if negative:
            if errors:
                # Expected: negative fixture correctly fails validation. Good.
                print(f"PASS (negative-test) {file_path}")
                total_pass += 1
            else:
                # Unexpected: negative fixture passed validation — the schema
                # may be too permissive. Treat as a failure.
                print(f"UNEXPECTED_PASS {file_path} | negative fixture unexpectedly passed schema validation")
                total_fail += 1
        else:
            if errors:
                for line in errors:
                    print(line)
                total_fail += 1
            else:
                print(f"PASS {file_path}")
                total_pass += 1

    print(
        f"\nSummary: {total_pass} passed, {total_fail} failed",
        file=sys.stderr,
    )
    return 0 if total_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
