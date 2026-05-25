# validate-watcher — WatchSpec YAML Validator

Validates every `packs/*/watches/*.yaml` file against the
[WatchSpec JSON Schema](../docs/api/schemas/watch.yaml.schema.json)
(JSON Schema 2020-12).

## Dependencies

| Tool | Install |
|---|---|
| Python 3.10+ | system / pyenv |
| `jsonschema` (2020-12 support) | `pip install 'jsonschema[format]>=4.18'` |
| `PyYAML` | `pip install PyYAML` |

One-liner:

```bash
pip install 'jsonschema[format]>=4.18' PyYAML
```

## Usage

```bash
# Validate all packs/*/watches/*.yaml
./scripts/validate-watcher.sh

# Verbose output (prints each file being checked)
./scripts/validate-watcher.sh -v

# Validate a specific file
./scripts/validate-watcher.sh packs/ar-collections/watches/dso-over-60.yaml

# Use a custom Python interpreter
PYTHON=python3.12 ./scripts/validate-watcher.sh
```

The Python script can also be called directly:

```bash
python3 scripts/validate-watcher.py --verbose
python3 scripts/validate-watcher.py path/to/watch.yaml
```

## Exit codes

| Code | Meaning |
|---|---|
| `0` | All files pass |
| `1` | One or more files fail validation |
| `2` | Setup error (missing dep, schema not found) |

## Output format

Each violation is printed as:

```
FAIL <path> | <json-path>: <message>
```

Example:

```
FAIL packs/ar-collections/watches/dso-over-60.yaml | $: 'id' is a required property
FAIL packs/ar-collections/watches/dso-over-60.yaml | $: 'on_match' is a required property

validate-watcher: 0/1 passed, 1 failed
```

## Schema location

The schema lives at `docs/api/schemas/watch.yaml.schema.json`.
It is sourced from `origin/docs/json-schemas-wave3` and is not yet on `main`.
To refresh it:

```bash
git checkout origin/docs/json-schemas-wave3 -- docs/api/schemas/watch.yaml.schema.json
```

## GitHub Actions snippet

```yaml
name: Validate watcher manifests

on:
  pull_request:
    paths:
      - 'packs/**/watches/*.yaml'
      - 'docs/api/schemas/watch.yaml.schema.json'
      - 'scripts/validate-watcher.*'

jobs:
  validate-watchers:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v5
        with:
          python-version: '3.12'

      - name: Install validator deps
        run: pip install 'jsonschema[format]>=4.18' PyYAML

      - name: Run watcher validator
        run: ./scripts/validate-watcher.sh --verbose
```
