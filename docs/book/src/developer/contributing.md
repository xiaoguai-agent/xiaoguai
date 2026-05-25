# Contributing

## Development setup

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai

# Rust workspace (requires stable toolchain per rust-toolchain.toml)
cargo build --workspace

# Frontend (requires pnpm)
cd frontend && pnpm install && pnpm -r typecheck

# Run all tests
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## Test conventions

- Unit tests live next to the module they test (`#[cfg(test)]` blocks)
- Integration tests that need a real DB are `#[ignore]`-marked and run with `XIAOGUAI_TEST_DATABASE_URL`
- Eval tests live **per-crate** under `crates/<crate>/tests/<feature>_eval.rs`. The `xiaoguai-eval` crate provides shared eval scaffolding + the canonical regression eval pattern from the v0.x era; new capability evals belong with the feature they exercise. Examples: `crates/xiaoguai-watch/tests/dsl_eval.rs`, `crates/xiaoguai-anomaly/tests/accuracy_eval.rs`, `crates/xiaoguai-api/tests/hotl_eval.rs`, `crates/xiaoguai-audit/tests/outcomes_eval.rs`.

## Commit style

```
type(scope): short description

Longer body if needed.
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`

## Pull request checklist

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `pnpm -r typecheck` clean (if frontend changed)
- [ ] New PG migration file added if schema changed
- [ ] `AppState` fields initialized with `..Default::default()` in test fixtures

---

## Wave-3 author guides

The sections below cover the four extension points added in the wave-3 milestone
(v1.2.x / v1.3.x-prep): skill packs, watchers, HotL policies, eval suites, outcome
attribution, and Cargo workspace hygiene.

---

### Authoring a skill pack

A skill pack is a self-contained directory under `packs/` that bundles agents,
inbound event adapters, output adapters, Jinja2 templates, and optional SQL
migrations into a single deployable unit.

**Canonical example:** `packs/pr-review`

#### Directory layout

```
packs/<pack-name>/
├── pack.yaml                   ← manifest (required)
├── agents/
│   └── <agent-name>.yaml       ← one file per agent
├── inbound/
│   └── <source-name>.yaml      ← webhook / cron / event-bus source
├── outputs/
│   └── <output-name>.yaml      ← MCP-tool or HTTP output adapter
├── templates/
│   └── <name>.md.j2            ← Jinja2 templates used by agents/outputs
├── migrations/
│   └── 0001_<name>.sql         ← optional pack-scoped Postgres migrations
└── tests/
    └── integration.rs          ← Rust integration tests (see §Eval suite)
```

#### pack.yaml schema

The manifest ties all pieces together and declares the execution plan:

```yaml
name: <pack-name>          # kebab-case; matches directory name
version: "1.0.0"           # semver
description: >
  One-paragraph description shown in the marketplace.

inbound:
  - ref: inbound/<source>.yaml   # one or more sources

agents:
  - ref: agents/<agent>.yaml     # order is informational; plan drives execution

outputs:
  - ref: outputs/<output>.yaml

plan:
  - id: <step-id>
    agent: <agent-name>          # OR output: <output-name>
    description: "What this step does."
    deps: [<step-id>, ...]       # omit for the first step

requires:
  env:
    - ENV_VAR_NAME               # validated at pack load time
  llm: true                      # set to false for pure-tool packs
```

A JSON Schema for editor validation lives at
`docs/api/schemas/pack.yaml.schema.json` (generated from the Rust types in
`crates/xiaoguai-scheduler/src/pack.rs`). Configure your editor to validate
`packs/**/pack.yaml` against this schema for instant feedback.

#### How the pieces plug together

- **inbound** adapters produce a context map (key → value) from the incoming
  event. The `extract:` block in a webhook adapter declares which JSON-path
  fields are promoted into that map.
- **agents** receive the context map as their initial input and append their
  output back into it under their `id` key.
- **outputs** read from the accumulated context map and call an MCP tool or HTTP
  endpoint.
- **templates** (`.md.j2`) are Jinja2 files rendered by the output adapter or
  agent prompt builder; they have access to the full context map.

---

### Defining a watcher (xg-watch DSL)

Watchers poll a data source on a schedule and emit `WatchEvent`s when new rows
appear. The Rust implementation lives in `crates/xiaoguai-watch/`.

#### WatchSpec YAML structure

```yaml
id: <unique-stable-id>          # used as the dedup namespace — must not change
source:
  sql:
    query: "SELECT ... FROM ... WHERE ..."   # must be a SELECT
  # OR
  http:
    url: "https://example.com/api/data"
    jsonpath: "$[*]"            # default; selects top-level array
    method: GET                 # default

schedule:
  interval_secs: 3600           # fixed interval (seconds); OR:
  # cron:
  #   expr: "0 */6 * * * *"    # 6-field ISO 8601 cron (sec min h dom mon dow)

on_match:
  action: notify                # logical action type dispatched by the scheduler
  target: ops-channel           # opaque string interpreted by the action handler
  params:                       # optional arbitrary metadata forwarded verbatim
    priority: high
```

**SQL source constraints:** the query must be a `SELECT` statement — the runner
validates this at load time. Use standard positional bind params (`$1`, `$2`)
where needed (dynamic-binding extension is planned for v1.3.x).

**HTTP source:** the `jsonpath` expression selects an array of objects from the
JSON response body; each object becomes one potential match row.

#### Dedup mechanics

Every match row is fingerprinted as:

```
SHA-256( spec_id + ":" + canonical_json(row) )
```

Canonical JSON sorts map keys for stability. The fingerprint is stored in a
TTL-based cache (backed by `moka`). A row is suppressed while its fingerprint
remains in the cache; once the TTL elapses the next occurrence fires again —
the intended behaviour for recurring alerts such as "fire daily while DSO > 60".

#### Cooldown semantics

The cache TTL acts as the cooldown period. Set it to match your alerting
cadence: a 24-hour TTL means each unique row fires at most once per day even
if the watcher runs every 15 minutes.

For pack-level watchers with richer re-fire logic (e.g. "re-fire if
`total_overdue` has grown by > 10%"), express the condition in the pack's
`watches/*.yaml` `dedup.re_fire_if` field; the watch runtime evaluates the
expression against the prior payload stored in the dedup cache.

#### Living examples

`crates/xiaoguai-watch/tests/dedup_integration.rs` contains integration
scenarios covering: first-occurrence fires, same-row suppression within TTL,
TTL expiry re-fires, and cross-spec independence. Read these before writing a
new watcher to understand the expected behaviour contract.

---

### Writing a HotL policy

HotL (Hard-off-the-top Limit) policies gate side-effecting actions — LLM
calls, email sends, webhook invocations — inside rolling time windows. The
implementation lives in `crates/xiaoguai-api/src/hotl/`.

#### Request body shape

`POST /v1/hotl/policies` accepts:

```json
{
  "tenant_id": "<uuid>",
  "scope": "llm_call",
  "window_seconds": 3600,
  "max_count": 100,
  "max_usd": 5.00,
  "escalate_to": "ops@example.com"
}
```

- `scope` — the action category string checked by the enforcer at each call
  site. Built-in scopes: `llm_call`, `email_send`, `webhook_invoke`. Packs may
  define custom scopes.
- `window_seconds` — rolling window width; must be `> 0`.
- `max_count` — maximum invocation count within the window. `null` = no count
  limit.
- `max_usd` — maximum cumulative USD cost within the window. `null` = no cost
  limit. At least one of `max_count` or `max_usd` must be set.
- `escalate_to` — IM channel ID or email address. When set the enforcer emits
  `HotlVerdict::Escalate` and the action proceeds; a human reviews
  asynchronously. When `null` the enforcer emits `HotlVerdict::Deny` and the
  caller must abort.

#### Fail-closed semantics

If the policy store is unreachable (network error, connection pool exhausted),
the enforcer returns `HotlVerdict::Deny`. **Never silently allow on error.**
This is intentional: a broken budget gate is safer than an open one.

#### Tier-routing strings

`escalate_to` accepts any string the IM gateway understands — a Slack channel
ID (`C0123ABCDEF`), a DingTalk group ID, or an email address. The gateway
routes based on the configured IM adapter.

#### Adding a new gated call site

1. Inject `Arc<dyn HotlEnforcer>` into the handler (already available via
   `AppState.hotl_enforcer`).
2. Call `enforcer.check(tenant_id, scope, amount).await` before executing the
   side effect.
3. Match on `HotlVerdict`: allow `Allow`, log-and-continue on `Escalate`,
   return `429` on `Deny`.
4. Call `enforcer.record_usage(tenant_id, scope, amount).await` after a
   successful action so the window counter advances.

See `crates/xiaoguai-api/src/hotl/enforcer.rs` for the full enforcer trait and
`crates/xiaoguai-api/tests/hotl.rs` for HTTP-level integration tests.

---

### Eval suite expectations

Every new crate should ship a `tests/<feature>_eval.rs` capability eval in
addition to unit tests. Eval files assert behavioural boundaries (precision,
recall, correctness thresholds) rather than exact implementation details.

#### Convention

```rust
// tests/my_feature_eval.rs

/// Scenario: <description of the real-world case being exercised>
#[tokio::test]
async fn my_feature_detects_obvious_case() {
    // Arrange — minimal fixture that represents the scenario.
    // Assert — express as precision/recall/correctness bounds, not exact values.
    assert!(precision >= 0.9, "precision {precision} below 0.9 threshold");
}

/// Known gap: <link to tracking issue>
#[tokio::test]
#[ignore = "TODO: #NNN — <one-line description of the gap>"]
async fn my_feature_handles_edge_case() {
    // Skeleton for a scenario not yet passing.
}
```

Key rules:
- Scenarios are **behavioural** (what the feature should do for a user) not
  implementation-bound (not "function X returns Y").
- Thresholds must be explicit and documented. Avoid `assert!(result.is_ok())` —
  prefer `assert!(precision >= 0.9, "…")`.
- Gaps are `#[ignore]` with a `TODO: #NNN` comment pointing to the tracking
  issue. Never delete a failing scenario; mark it ignored instead.

#### Reference evals in the codebase

| Crate | File | What it covers |
|---|---|---|
| `xiaoguai-watch` | `tests/dedup_integration.rs` | TTL dedup behavioural contract |
| `xiaoguai-anomaly` | `tests/integration.rs` | Anomaly detection integration scenarios |
| `xiaoguai-api` | `tests/hotl.rs` | HotL policy CRUD + budget enforcement |
| `xiaoguai-api` | `tests/outcomes.rs` | Outcome attribution round-trip |
| `xiaoguai-audit` | `tests/chain_basic.rs` | Audit chain integrity |
| `xiaoguai-eval` | `tests/example_suite.rs` | Shipped example eval suite loads and passes |

---

### Outcome attribution discipline

Every agent that produces a side effect with measurable business value should
record an outcome so the ROI dashboard can surface it.

#### Trait

```rust
// crates/xiaoguai-audit/src/outcomes.rs
#[async_trait]
pub trait OutcomeRecorder: Send + Sync {
    async fn record(
        &self,
        tenant_id: &str,
        session_id: Option<&str>,
        agent_name: &str,
        kind: &str,          // OutcomeKind::as_str()
        value: f64,
        unit: Option<&str>,
        description: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<(), OutcomeError>;
}
```

Built-in `kind` strings: `revenue_usd`, `cost_saved_usd`, `hours_saved`,
`deals_closed`, `tickets_resolved`, `custom`.

#### Recommended pattern

```rust
// After a successful side effect:
if let Some(recorder) = &state.outcome_writer {
    let _ = recorder
        .record(
            &tenant_id,
            Some(&session_id),
            "my-agent",
            "hours_saved",
            2.5,
            Some("hours"),
            Some("Automated dunning draft saved manual email time"),
            serde_json::json!({ "customer_id": customer_id }),
        )
        .await;
    // Swallow errors — outcome recording must not block the happy path.
}
```

**Parent chain:** when an orchestrator spawns sub-agents, pass
`parent_outcome_id` in the `metadata` JSON so the dashboard can reconstruct the
attribution chain. Use the UUID of the parent agent's outcome record.

**Do not record outcomes for dry-run or preview executions.** Gate on the
actual commit to the external system.

The `InMemoryOutcomeRecorder` in `xiaoguai-audit` is the test double — wire it
via `AppState.outcome_writer` in test fixtures and call `.snapshot()` to assert
recorded outcomes.

---

### Cross-cutting: Cargo workspace etiquette

#### Adding a new crate

1. Create `crates/<name>/Cargo.toml` with `[package]` inheriting from
   `[workspace.package]` (`version.workspace = true`, etc.).
2. **Add the crate to `[workspace] members`** in the root `Cargo.toml`.
   Forgetting this is the most common mistake — the crate compiles in isolation
   but is invisible to `cargo test --workspace` and CI.
3. Prefer `workspace.dependencies` over pinning a version directly. If the dep
   you need is not yet in `[workspace.dependencies]`, add it there first, then
   reference it with `dep.workspace = true` in the crate.

#### Do not bump `rust-version` casually

The workspace `rust-version` is a hard floor for all contributors. Bumping it
forces everyone to upgrade their toolchain. The v1.2.19 `audit-s3` incident
bumped `rust-version` from `1.88` to `1.91` because `aws-smithy-types`
introduced a transitive dependency that required it — a legitimate reason.
Bumping for convenience (e.g. to use a stabilised API that has a `1.70+`
stable alternative) is not acceptable without a team discussion.

#### AppState convoy

`AppState` in `crates/xiaoguai-api/src/lib.rs` accumulates one optional field
per major feature. Every test fixture that constructs `AppState` directly must
compile when a new field is added.

Use `..Default::default()` (or the `..AppState::test_default()` helper if one
exists) for all fields your test does not exercise. Do not copy-paste a
full-field struct literal — that pattern caused 15+ conflicts during the
`feat/hotl-policy` merge and 22+ during `feat/outcome-telemetry`. See the
wave-3 HANDOFF (`docs/HANDOFF-2026-05-26.md §3`) for the full incident log.

---

### Frontend pattern: `frontend/shared` is the wire-type source of truth

`frontend/shared/src/index.ts` exports every TypeScript type that mirrors a
Rust API response shape. **Do not duplicate these types in `admin-ui` or
`chat-ui`.** Import from `@xiaoguai/shared` instead.

When the Rust API adds a field:

1. Update the Rust type in `crates/xiaoguai-api/src/routes/`.
2. Mirror the field in `frontend/shared/src/index.ts`.
3. Run `pnpm -r typecheck` — both apps must pass before the PR is merged.

New fields should be optional (`field?: Type`) when they are additive
(consumers that have not yet been updated must not break). Mark them required
only when the API guarantees the field is always present.

