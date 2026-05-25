#!/usr/bin/env python3
"""Manifest validator hook dispatcher.

Usage (called by pre-commit):
    validate_manifest.py <kind> [file ...]

<kind> is one of: pack | watcher | hotl-policy | recipe

Each validator calls the appropriate CLI or Python validation logic.
At this stage it provides structural YAML parsing and required-field
checks; replace the _validate_* functions with full jsonschema calls
once the parallel validator branches land on main.
"""

from __future__ import annotations

import sys
import yaml
from pathlib import Path


# ---------------------------------------------------------------------------
# Required-field definitions (minimal — extend when validator branches land)
# ---------------------------------------------------------------------------

REQUIRED_FIELDS: dict[str, list[str]] = {
    "pack": ["apiVersion", "kind", "metadata", "spec"],
    "watcher": ["apiVersion", "kind", "metadata", "spec"],
    "hotl-policy": ["apiVersion", "kind", "metadata", "spec"],
    "recipe": ["apiVersion", "kind", "metadata", "spec"],
}


def _validate_file(kind: str, path: Path) -> list[str]:
    """Return a list of error strings (empty = pass)."""
    errors: list[str] = []

    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"{path}: cannot read file: {exc}"]

    try:
        doc = yaml.safe_load(raw)
    except yaml.YAMLError as exc:
        return [f"{path}: YAML parse error: {exc}"]

    if doc is None:
        return [f"{path}: file is empty"]

    if not isinstance(doc, dict):
        return [f"{path}: expected a YAML mapping at the top level"]

    for field in REQUIRED_FIELDS.get(kind, []):
        if field not in doc:
            errors.append(f"{path}: missing required field '{field}'")

    # kind-specific checks
    if kind == "pack":
        errors.extend(_check_pack(path, doc))
    elif kind == "watcher":
        errors.extend(_check_watcher(path, doc))
    elif kind == "hotl-policy":
        errors.extend(_check_hotl_policy(path, doc))
    elif kind == "recipe":
        errors.extend(_check_recipe(path, doc))

    return errors


def _check_pack(path: Path, doc: dict) -> list[str]:
    errs: list[str] = []
    if doc.get("kind") not in (None, "Pack"):
        errs.append(f"{path}: 'kind' must be 'Pack', got '{doc['kind']}'")
    metadata = doc.get("metadata", {}) or {}
    if not isinstance(metadata, dict) or not metadata.get("name"):
        errs.append(f"{path}: 'metadata.name' is required")
    return errs


def _check_watcher(path: Path, doc: dict) -> list[str]:
    errs: list[str] = []
    if doc.get("kind") not in (None, "Watcher"):
        errs.append(f"{path}: 'kind' must be 'Watcher', got '{doc['kind']}'")
    spec = doc.get("spec", {}) or {}
    if not isinstance(spec, dict) or not spec.get("trigger"):
        errs.append(f"{path}: 'spec.trigger' is required")
    return errs


def _check_hotl_policy(path: Path, doc: dict) -> list[str]:
    errs: list[str] = []
    if doc.get("kind") not in (None, "HoTLPolicy"):
        errs.append(
            f"{path}: 'kind' must be 'HoTLPolicy', got '{doc.get('kind')}'"
        )
    return errs


def _check_recipe(path: Path, doc: dict) -> list[str]:
    errs: list[str] = []
    if doc.get("kind") not in (None, "Recipe"):
        errs.append(f"{path}: 'kind' must be 'Recipe', got '{doc.get('kind')}'")
    spec = doc.get("spec", {}) or {}
    if not isinstance(spec, dict) or not spec.get("steps"):
        errs.append(f"{path}: 'spec.steps' is required")
    return errs


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> int:
    if len(sys.argv) < 2:
        print("Usage: validate_manifest.py <kind> [file ...]", file=sys.stderr)
        return 2

    kind = sys.argv[1]
    files = sys.argv[2:]

    if kind not in REQUIRED_FIELDS:
        print(
            f"Unknown manifest kind '{kind}'. "
            f"Valid kinds: {', '.join(REQUIRED_FIELDS)}",
            file=sys.stderr,
        )
        return 2

    if not files:
        # No staged files matched — success.
        return 0

    all_errors: list[str] = []
    for f in files:
        all_errors.extend(_validate_file(kind, Path(f)))

    if all_errors:
        for err in all_errors:
            print(err, file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
