# HotL Policy Validator

Validates HotL (Human-on-the-Loop) policy JSON files against the JSON Schema 2020-12 definition at `docs/api/schemas/hotl-policy.json.schema.json`, plus a business-rule check grounded in `crates/xiaoguai-api/src/hotl/policy.rs`.

## Files

| File | Purpose |
|------|---------|
| `scripts/validate-hotl-policy.sh` | Bash wrapper; orchestrates file discovery and dependency checks |
| `scripts/validate-hotl-policy.py` | Python validator; prints `PASS`/`FAIL` per file, exits 0/1 |
| `docs/api/schemas/hotl-policy.json.schema.json` | JSON Schema 2020-12 source of truth |

## Dependencies

- Python 3.11+
- `jsonschema` with format extras:

```bash
pip install 'jsonschema[format-nongpl]'
```

## Usage

### Validate specific files

```bash
python scripts/validate-hotl-policy.py path/to/policy.json
# or via bash wrapper
scripts/validate-hotl-policy.sh path/to/policy.json another.json
```

### Scan the default examples directory

```bash
scripts/validate-hotl-policy.sh
# Scans examples/hotl-policies/*.json automatically
```

### Verbose / quiet

```bash
scripts/validate-hotl-policy.sh --verbose   # default: already verbose
scripts/validate-hotl-policy.sh --quiet     # suppress summary line
```

## Output format

```
PASS examples/hotl-policies/count-budget-llm-calls.json
PASS examples/hotl-policies/amount-budget-usd-spend.json
PASS examples/hotl-policies/mixed-high-risk-write.json
FAIL examples/hotl-policies/invalid-missing-both.json | $: at least one of max_count or max_usd must be non-null (both are null)

Summary: 3 passed, 1 failed
```

Exit code `0` = all valid. Exit code `1` = at least one failure. Exit code `2` = dependency/usage error.

## Validation rules

The schema enforces:

- `tenant_id`, `scope`, `window_seconds` are required.
- `window_seconds` must be ≥ 1 (> 0).
- `max_count`, when present and non-null, must be ≥ 1.
- `max_usd`, when present and non-null, must be ≥ 0.
- No additional properties allowed.

The Python validator adds one business rule that JSON Schema 2020-12 cannot express as a cross-property constraint without `if`/`then`:

> **At least one of `max_count` or `max_usd` must be non-null.**

This mirrors the rejection logic in `InMemoryHotlPolicyStore::create`.

## Example policies

| File | Scope | Limits | Escalation |
|------|-------|--------|------------|
| `count-budget-llm-calls.json` | `llm_call` | 1000 calls / 24 h | tier-2 |
| `amount-budget-usd-spend.json` | `usd_spend` | $500 / 7 d | tier-3 |
| `mixed-high-risk-write.json` | `high_risk_write` | 10 calls + $50 / 1 h | tier-1 |
| `invalid-missing-both.json` | `llm_call` | **none** (intentionally invalid) | — |

## GitHub Actions snippet

```yaml
name: Validate HotL policies

on:
  pull_request:
    paths:
      - 'examples/hotl-policies/**'
      - 'docs/api/schemas/hotl-policy.json.schema.json'

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v5
        with:
          python-version: '3.11'

      - name: Install dependencies
        run: pip install 'jsonschema[format-nongpl]'

      - name: Validate HotL policy examples
        run: bash scripts/validate-hotl-policy.sh

      - name: Validate any other hotl policy JSON files in repo
        run: |
          FILES=$(find . -name '*hotl*policy*.json' \
            -not -path './examples/hotl-policies/*' \
            -not -path './.git/*' 2>/dev/null || true)
          if [ -n "$FILES" ]; then
            python scripts/validate-hotl-policy.py $FILES
          else
            echo "No additional hotl-policy JSON files found."
          fi
```
