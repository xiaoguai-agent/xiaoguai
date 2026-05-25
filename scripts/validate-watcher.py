#!/usr/bin/env python3
"""validate-watcher.py — JSON Schema 2020-12 validator for WatchSpec YAML files.

Validates each file against docs/api/schemas/watch.yaml.schema.json.
Exits 0 if all files pass; non-zero on any failure.

Output per violation:
    FAIL <path> | <json-path>: <message>
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Sequence

try:
    import jsonschema
    import yaml
except ImportError as exc:
    print(f"validate-watcher: missing dependency — {exc}", file=sys.stderr)
    print("  pip install jsonschema[format] PyYAML", file=sys.stderr)
    sys.exit(2)

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError, ValidationError


_SCHEMA_REL = Path("docs/api/schemas/watch.yaml.schema.json")


def _repo_root() -> Path:
    """Walk up from this script to the repo root (contains Cargo.toml)."""
    here = Path(__file__).resolve().parent
    for candidate in [here.parent, here.parent.parent]:
        if (candidate / "Cargo.toml").exists():
            return candidate
    # Fallback: two levels above scripts/
    return here.parent


def load_schema(repo_root: Path) -> dict:
    schema_path = repo_root / _SCHEMA_REL
    if not schema_path.exists():
        print(
            f"validate-watcher: schema not found at {schema_path}",
            file=sys.stderr,
        )
        print(
            "  Ensure docs/api/schemas/watch.yaml.schema.json exists "
            "(cherry-pick from docs/json-schemas-wave3).",
            file=sys.stderr,
        )
        sys.exit(2)
    with schema_path.open() as fh:
        return json.load(fh)


def validate_file(
    path: Path,
    validator: Draft202012Validator,
    *,
    verbose: bool = False,
) -> list[str]:
    """Return list of FAIL lines for the given YAML file."""
    if verbose:
        print(f"  checking {path}")

    try:
        with path.open() as fh:
            doc = yaml.safe_load(fh)
    except yaml.YAMLError as exc:
        return [f"FAIL {path} | $: YAML parse error — {exc}"]

    if doc is None:
        return [f"FAIL {path} | $: file is empty"]

    errors: list[str] = []
    for err in sorted(validator.iter_errors(doc), key=lambda e: str(e.absolute_path)):
        json_path = "$." + ".".join(str(p) for p in err.absolute_path) if err.absolute_path else "$"
        errors.append(f"FAIL {path} | {json_path}: {err.message}")

    return errors


def main(argv: Sequence[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(
        description="Validate WatchSpec YAML files against the JSON Schema.",
    )
    parser.add_argument(
        "files",
        nargs="*",
        metavar="FILE",
        help="YAML files to validate. Defaults to all packs/*/watches/*.yaml.",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Print each file being checked.",
    )
    parser.add_argument(
        "--schema",
        metavar="PATH",
        help="Override path to watch.yaml.schema.json.",
    )
    args = parser.parse_args(argv)

    repo_root = _repo_root()

    # Load schema
    if args.schema:
        schema_path = Path(args.schema)
        if not schema_path.exists():
            print(f"validate-watcher: schema override not found: {schema_path}", file=sys.stderr)
            return 2
        with schema_path.open() as fh:
            schema = json.load(fh)
    else:
        schema = load_schema(repo_root)

    try:
        Draft202012Validator.check_schema(schema)
    except SchemaError as exc:
        print(f"validate-watcher: schema itself is invalid — {exc}", file=sys.stderr)
        return 2

    validator = Draft202012Validator(schema)

    # Resolve files
    if args.files:
        files = [Path(f) for f in args.files]
    else:
        files = sorted((repo_root / "packs").glob("*/watches/*.yaml"))

    if not files:
        print("validate-watcher: no watcher YAML files found", file=sys.stderr)
        return 0

    all_failures: list[str] = []
    passed = 0

    for path in files:
        failures = validate_file(path, validator, verbose=args.verbose)
        if failures:
            for line in failures:
                print(line)
            all_failures.extend(failures)
        else:
            if args.verbose:
                print(f"  PASS {path}")
            passed += 1

    total = len(files)
    failed = total - passed
    print(f"\nvalidate-watcher: {passed}/{total} passed, {failed} failed")

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
