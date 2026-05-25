# validate-recipe — Recipe Manifest Validator

Validates every `recipes/*.yaml` file against the
[Recipe JSON Schema](../docs/api/schemas/recipe.yaml.schema.json)
(JSON Schema 2020-12).

## Files

| File | Purpose |
|------|---------|
| `scripts/validate-recipe.sh` | Bash wrapper; orchestrates file discovery and dependency checks |
| `scripts/validate-recipe.py` | Python validator; prints `PASS`/`FAIL` per file, exits 0/1/2 |
| `docs/api/schemas/recipe.yaml.schema.json` | JSON Schema 2020-12 source of truth |

## Dependencies

| Tool | Install |
|------|---------|
| Python 3.11+ | system / pyenv |
| `jsonschema` (2020-12 support) | `pip install 'jsonschema[format-nongpl]>=4.18'` |
| `PyYAML` | `pip install pyyaml` |

One-liner:

```bash
pip install 'jsonschema[format-nongpl]>=4.18' pyyaml
```

## Usage

```bash
# Validate all recipes/*.yaml (failures only)
./scripts/validate-recipe.sh

# Verbose: print every result including PASS lines
./scripts/validate-recipe.sh -v

# Validate a specific recipe
./scripts/validate-recipe.sh recipes/ticket-to-csm-action.yaml

# Use a custom Python interpreter
PYTHON=python3.12 ./scripts/validate-recipe.sh

# Suppress summary line (useful in CI log aggregators)
./scripts/validate-recipe.sh -q
```

The Python script can also be called directly:

```bash
python3 scripts/validate-recipe.py recipes/incident-detected-to-resolved.yaml
python3 scripts/validate-recipe.py recipes/*.yaml
```

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | All files pass |
| `1` | One or more files fail validation |
| `2` | Setup error (missing dep, schema not found, bad usage) |

## Output format

Each line is either:

```
PASS <path>
FAIL <path> | <json-path>: <message>
```

Example run (all passing):

```
PASS recipes/anomaly-spike-to-investigation.yaml
PASS recipes/cve-feed-to-audit-archive.yaml
PASS recipes/incident-detected-to-resolved.yaml
PASS recipes/ticket-to-csm-action.yaml

Summary: 4 passed, 0 failed
---
Validated 4 recipe(s): 4 passed, 0 failed.
```

Example failure:

```
FAIL recipes/bad-recipe.yaml | $: 'steps' is a required property
FAIL recipes/bad-recipe.yaml | $.name: 'BadName' does not match '^[a-z0-9][a-z0-9-]*[a-z0-9]$'

Summary: 0 passed, 1 failed
```

## Schema location

The schema lives at `docs/api/schemas/recipe.yaml.schema.json`.
It is sourced from `origin/docs/recipe-schema`.
To refresh it:

```bash
git checkout origin/docs/recipe-schema -- docs/api/schemas/recipe.yaml.schema.json
```

## What the schema validates

The schema enforces:

- `name`, `version`, `description`, and `steps` are required.
- `name` matches kebab-case pattern `^[a-z0-9][a-z0-9-]*[a-z0-9]$`.
- `version` matches SemVer pattern `^\d+\.\d+\.\d+$`.
- `steps` is a non-empty array; each step requires at least an `id`.
- Step `id` matches `^[a-z][a-z0-9_]*$` (snake_case).
- `requires.features` items are constrained to known feature flags (`hotl`, `outcome-telemetry`, `anomaly`, `watch`).
- No unrecognised properties on `trigger`, `hotl` blocks, `outcome` blocks, and other structural nodes.
- `hotl` blocks require `policy` and `gated_on`; `outcome` blocks require `event` and `payload`.

## GitHub Actions snippet

```yaml
name: Validate recipe manifests

on:
  pull_request:
    paths:
      - 'recipes/*.yaml'
      - 'docs/api/schemas/recipe.yaml.schema.json'
      - 'scripts/validate-recipe.*'

jobs:
  validate-recipes:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v5
        with:
          python-version: '3.11'

      - name: Install validator deps
        run: pip install 'jsonschema[format-nongpl]>=4.18' pyyaml

      - name: Validate recipe manifests
        run: bash scripts/validate-recipe.sh -v
```
