# Pack Manifest Validator

Validates every `packs/*/pack.yaml` file against the JSON Schema 2020-12 schema
at `docs/api/schemas/pack.yaml.schema.json`.

## Dependencies

```
pip install pyyaml jsonschema
```

Both scripts require Python 3.8+ and are tested with `jsonschema` >= 4.17 (Draft 2020-12 support).

## Usage

### Bash wrapper (recommended for CI and local use)

```bash
# Validate all packs — failures printed, exit 1 if any fail
./scripts/validate-pack.sh

# Verbose: show PASS and FAIL for every pack
./scripts/validate-pack.sh -v

# Validate a single pack
./scripts/validate-pack.sh -v packs/pr-review/pack.yaml

# Use a specific Python interpreter
PYTHON=python3.11 ./scripts/validate-pack.sh
```

### Python validator directly

```bash
python scripts/validate-pack.py packs/pr-review/pack.yaml
```

Exit 0 = valid, 1 = schema violations found, 2 = usage or load error.

## Example Output

```
PASS packs/ar-collections/pack.yaml
PASS packs/hr-onboarding/pack.yaml
FAIL packs/incident-triage/pack.yaml | $.agents[0]: 'agents/triage-agent.yaml' is not of type 'object'
FAIL packs/rag-finance/pack.yaml | $: 'name' is a required property
FAIL packs/rag-finance/pack.yaml | $: 'version' is a required property
FAIL packs/rag-finance/pack.yaml | $: 'description' is a required property
---
Validated 7 pack(s): 3 passed, 4 failed.
Failed packs:
  - packs/incident-triage/pack.yaml
  - packs/rag-finance/pack.yaml
  - packs/rag-hr/pack.yaml
  - packs/rag-legal/pack.yaml
```

Each violation line format:
```
FAIL <pack.yaml path> | <json-path>: <violation description>
```

## CI Integration (GitHub Actions)

```yaml
- name: Validate pack manifests
  run: |
    pip install pyyaml jsonschema
    ./scripts/validate-pack.sh
```

Full step with caching:

```yaml
- name: Set up Python
  uses: actions/setup-python@v5
  with:
    python-version: "3.11"

- name: Cache pip
  uses: actions/cache@v4
  with:
    path: ~/.cache/pip
    key: ${{ runner.os }}-pip-pyyaml-jsonschema

- name: Install validator dependencies
  run: pip install pyyaml jsonschema

- name: Validate pack manifests
  run: ./scripts/validate-pack.sh
```

## Known Pack Issues (as of initial validator run)

The following packs on `main` fail schema validation. These are real findings —
the packs have fields that do not conform to the schema:

| Pack | Violations |
|------|-----------|
| `incident-triage` | `agents` items use bare strings (`agents/triage-agent.yaml`) instead of objects with a `path` or `ref` key. The schema requires `{"path": "..."}` or `{"ref": "..."}`. |
| `rag-finance` | Missing required top-level fields: `name`, `version`, `description`. |
| `rag-hr` | Missing required top-level fields: `name`, `version`, `description`. |
| `rag-legal` | Missing required top-level fields: `name`, `version`, `description`. |

Resolution options (out of scope for this validator script — do not silently fix):

- For `incident-triage`: change `agents` items from bare strings to `- path: agents/triage-agent.yaml` form,
  or extend the schema `agents.items` to also accept strings.
- For RAG packs: add the missing `name`, `version`, and `description` fields,
  or relax the schema to make them optional for `kind: scaffold-only` packs.

## Schema Location

`docs/api/schemas/pack.yaml.schema.json` — JSON Schema 2020-12.

Sourced from branch `docs/json-schemas-wave3` via:
```bash
git checkout origin/docs/json-schemas-wave3 -- docs/api/schemas/pack.yaml.schema.json
```
