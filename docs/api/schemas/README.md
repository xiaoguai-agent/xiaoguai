# Xiaoguai JSON Schemas

JSON Schema (draft 2020-12) files for the YAML/JSON spec types that operators author by hand.
Used for editor validation and CI linting.

## Schema Files

| File | Validates | Rust ground truth |
|------|-----------|-------------------|
| `pack.yaml.schema.json` | `packs/*/pack.yaml` manifests | — (derived from all pack examples) |
| `watch.yaml.schema.json` | `WatchSpec` YAML/JSON files | `crates/xiaoguai-watch/src/spec.rs` |
| `hotl-policy.json.schema.json` | `HotlPolicy` / `CreateHotlPolicyRequest` API bodies | `crates/xiaoguai-api/src/hotl/policy.rs` |
| `outcome.json.schema.json` | `OutcomeRecord`, `Aggregate`, `OutcomeSummary`, `OutcomeDay` | `crates/xiaoguai-audit/src/outcomes.rs` |

## Editor Integration

### VS Code / yaml-language-server

Add a modeline comment to the top of any YAML file to get inline validation:

```yaml
# yaml-language-server: $schema=https://xiaoguai-agent.github.io/xiaoguai/schemas/pack.yaml.schema.json
name: my-pack
version: "1.0.0"
...
```

Or wire all `packs/*/pack.yaml` files via VS Code `settings.json`:

```json
{
  "yaml.schemas": {
    "https://xiaoguai-agent.github.io/xiaoguai/schemas/pack.yaml.schema.json": "packs/*/pack.yaml",
    "https://xiaoguai-agent.github.io/xiaoguai/schemas/watch.yaml.schema.json": "packs/*/watches/*.yaml"
  }
}
```

### IntelliJ / JetBrains

Use **Languages & Frameworks > Schemas and DTDs > JSON Schema Mappings** to map each schema
to the corresponding file glob.

## CI Linting

### Prerequisites

```bash
pip install check-jsonschema jsonschema
```

### Validate the schemas themselves (meta-validation)

```bash
python3 -c "
import json, jsonschema, pathlib
for p in pathlib.Path('docs/api/schemas').glob('*.json'):
    jsonschema.Draft202012Validator.check_schema(json.loads(p.read_text()))
    print(f'OK: {p.name}')
"
```

### Validate pack manifests against the schema

```bash
check-jsonschema \
  --schemafile docs/api/schemas/pack.yaml.schema.json \
  packs/*/pack.yaml
```

### Validate watch specs

```bash
check-jsonschema \
  --schemafile docs/api/schemas/watch.yaml.schema.json \
  packs/*/watches/*.yaml
```

### GitHub Actions snippet

```yaml
- name: Lint JSON schemas (meta-validate)
  run: |
    python3 -c "
    import json, jsonschema, pathlib
    for p in pathlib.Path('docs/api/schemas').glob('*.json'):
        jsonschema.Draft202012Validator.check_schema(json.loads(p.read_text()))
        print(f'OK: {p.name}')
    "

- name: Lint pack manifests
  run: |
    pip install check-jsonschema -q
    check-jsonschema \
      --schemafile docs/api/schemas/pack.yaml.schema.json \
      packs/*/pack.yaml
```

## Schema design notes

- `pack.yaml.schema.json` — supports both `path` and `ref` keys for agent/inbound/output entries
  (different packs use different conventions). RAG pack fields (`embedding_model`, `top_k`, etc.)
  are optional top-level properties so the same schema covers both orchestration and RAG packs.
- `watch.yaml.schema.json` — `source` uses `oneOf` to enforce exactly one of `sql` or `http`.
  The `schedule` field is optional (defaults to 60-second interval in the runner).
- `hotl-policy.json.schema.json` — exposes both `HotlPolicy` (with server-assigned `id`) and
  `CreateHotlPolicyRequest` (without `id`) as `$defs`; use `$ref` to the specific def in tooling.
- `outcome.json.schema.json` — `kind` accepts any non-empty string (not restricted to enum values)
  because `OutcomeKind::Custom` allows operator-defined kinds at runtime.
