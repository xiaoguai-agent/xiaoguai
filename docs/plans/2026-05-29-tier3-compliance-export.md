# T5 — Compliance export from audit chain (Tier-3)

Sub-plan for the 2026-05-29 sprint task T5. Parent: `docs/plans/2026-05-29-next-sprint.md` §3 T5
(the sprint plan is not on disk in this worktree; the T5 sketch in the user's task brief is the
seed). Branch: `feat/tier3-compliance-export`.

## 1. Context

The audit chain in `crates/xiaoguai-audit` already persists tamper-evident rows
(`AuditEntry { ts, tenant_id, actor, action, resource, details }` chained via HMAC-SHA256, see
`crates/xiaoguai-audit/src/chain.rs` and `src/sink.rs`). `verify_chain()` walks a slice of
`StoredEntry` and reports the first `HmacMismatch(id)` or `LinkBroken(from, to)`.

What's missing is a compliance-friendly projection: SOC2 CC7.2 / GDPR Art. 30 / HIPAA §164.312
each ask for different slices of "what did the system do, who did it, when, on what". Today an
auditor would have to read raw JSON `details` and reason about HMAC bytes themselves. We need a
CLI + HTTP surface that produces auditor-ready bundles with the chain-verification proof baked
in — so the auditor doesn't take our word that the chain is intact.

Design constraints from the task brief:
- JSON is the canonical format; CSV is a projection (same row count, same column meanings).
- Chain verification is non-bypassable. No `--skip-verify` escape hatch. If the chain is broken
  inside the requested `[from, to]` window, the export refuses and surfaces the first broken
  row's `id` + `ts`.
- Templates are static — hardcoded `(action, actor, resource, ts)` projections + filters per
  framework. No runtime DSL.
- PDF is deferred. A stub function exists so the API surface is in place; calling it returns
  `Error::PdfUnimplemented` and the runbook flags it as follow-up work.
- No live PG required for tests — mirror the in-memory pattern from `xiaoguai-tasks::skill_author`
  (PR #72) and `xiaoguai-audit::chain_basic.rs`.

## 2. Success criteria

1. New module `crates/xiaoguai-audit/src/export.rs` with three hardcoded framework templates
   (SOC2 CC7.2, GDPR Art. 30, HIPAA §164.312) each producing a deterministic row projection
   from `&[StoredEntry]`.
2. `export_bundle(framework, rows, format, window)` returns a `ComplianceBundle { header,
   rows, chain_proof }` where the header carries `{ framework, from, to, generated_at,
   chain_proof: ChainProofOk { first_id, last_id, count, start_hmac_hex, end_hmac_hex } }`.
3. If `verify_chain` fails inside the window, `export_bundle` returns
   `ExportError::ChainBroken { first_broken_id, first_broken_ts }`. JSON output of the error
   is machine-readable.
4. JSON canonical output and CSV projection have identical row counts and identical column
   semantics (CSV columns are a subset of the JSON row keys, no synthesized fields).
5. `pdf_stub(...)` returns `ExportError::PdfUnimplemented` — surface area present, no
   generation.
6. New CLI subcommand `xiaoguai audit export --framework {soc2|gdpr|hipaa} --tenant-id <id>
   --from <RFC3339> --to <RFC3339> --output <path> --format {json|csv}` writes the bundle.
   On chain-broken, exits non-zero AND writes machine-readable error JSON to stderr.
7. New HTTP endpoint `POST /v1/audit/exports` accepting the same parameters returns the
   bundle inline (`Content-Type: application/json` for json, `text/csv` for csv). On
   chain-broken, returns 409 Conflict with the error JSON.
8. Unit tests cover per-framework projection (synthetic rows → expected shape).
9. Integration test in `crates/xiaoguai-audit/tests/export_integration.rs` builds a synthetic
   chain, exports it, asserts chain proof is present and correct; then mutates one row and
   asserts export refuses with the right `first_broken_id`.
10. `cargo test -p xiaoguai-audit -p xiaoguai-cli -p xiaoguai-api` green; `cargo fmt --check`
    exit 0; `cargo clippy -p xiaoguai-audit -p xiaoguai-cli -p xiaoguai-api -- -D warnings`
    green.
11. `docs/runbooks/compliance-export.md` exists, explains what each framework reports, what
    gaps remain, and includes a sample auditor-question → audit-action mapping.
12. PR titled `feat(tier-3): compliance export from audit chain (T5)` opened against `main`
    via `gh pr create`.

## 3. Prerequisites

- Branch `feat/tier3-compliance-export` created off `main` (done).
- Read access to existing audit module structure (`crates/xiaoguai-audit/src/{chain,sink,redact}.rs`).
  Verified above.
- Read access to existing CLI + API route patterns (`crates/xiaoguai-cli/src/commands/outcomes.rs`,
  `crates/xiaoguai-api/src/routes/admin.rs::verify_audit`). Verified above.
- Existing `AuditReader` + `AuditVerifier` traits in `xiaoguai-api::audit` already give us the
  shape we need at the API layer. The new endpoint reuses `AuditReader::list(tenant_id, since,
  until, limit)` to load rows in the window.
- No new dependencies needed. Use `chrono`, `serde`, `serde_json` (already in
  `xiaoguai-audit/Cargo.toml`), and `hex` (already in `xiaoguai-core` via workspace).
  CSV: hand-rolled writer (RFC 4180 escaping is ~20 lines; adding the `csv` crate just for
  this would be over-spec).

## 4. Step-by-step

### Step 4.1 — Define export types in `xiaoguai-audit`

Create `crates/xiaoguai-audit/src/export.rs` with:

```rust
pub enum Framework { Soc2Cc72, GdprArt30, Hipaa164312 }
pub enum Format { Json, Csv, Pdf }
pub struct ExportWindow { pub from: DateTime<Utc>, pub to: DateTime<Utc> }
pub struct ChainProof {
    pub first_id: i64, pub last_id: i64, pub count: u64,
    pub start_prev_hmac_hex: String, pub end_hmac_hex: String,
}
pub struct BundleHeader {
    pub framework: Framework, pub window: ExportWindow,
    pub generated_at: DateTime<Utc>, pub tenant_id: String,
    pub chain_proof: ChainProof,
}
pub struct BundleRow { pub ts, pub actor, pub action, pub resource, pub detail_field: String, ... }
pub struct ComplianceBundle { pub header, pub rows: Vec<BundleRow> }
pub enum ExportError {
    ChainBroken { first_broken_id: i64, first_broken_ts: DateTime<Utc> },
    EmptyWindow,
    PdfUnimplemented,
    Chain(ChainError),
}
```

Add `pub mod export;` to `lib.rs` and re-export the public types.

**VC:** `cargo check -p xiaoguai-audit` exits 0.

### Step 4.2 — Implement per-framework templates

Inside `export.rs`, three pure functions:

- `fn project_soc2_cc72(rows: &[StoredEntry]) -> Vec<BundleRow>` — filter to security-monitoring
  actions: `session.create`, `tool.invoke`, `auth.login`, `auth.failure`, `policy.deny`,
  `audit.verify`, `cost.charge`. Project `(ts, actor, action, resource)` + a flattened
  `details_summary` from `details`.
- `fn project_gdpr_art30(rows: &[StoredEntry]) -> Vec<BundleRow>` — filter to data-processing
  actions: `memory.create`, `memory.update`, `memory.delete`, `memory.recall`, `session.create`,
  `session.delete`, `data.export`, `data.purge`. Same projection.
- `fn project_hipaa_164312(rows: &[StoredEntry]) -> Vec<BundleRow>` — filter to access-control
  + audit-control actions: `auth.login`, `auth.failure`, `session.create`, `tool.invoke`
  (when resource starts with `phi:`), `audit.verify`, `policy.deny`. Same projection.

Filters are hardcoded `match` arms over action strings. Projections preserve `ts` in RFC3339.

**VC:** `cargo test -p xiaoguai-audit export::tests::projection_soc2_filters_correctly` (and
the two siblings) passes.

### Step 4.3 — Implement `export_bundle`

```rust
pub fn export_bundle(
    framework: Framework,
    tenant_id: String,
    rows: Vec<StoredEntry>,
    window: ExportWindow,
    chain: &ChainedAudit,
) -> Result<ComplianceBundle, ExportError>
```

Algorithm:
1. If `rows.is_empty()` → return bundle with `count=0`, `chain_proof` carrying zero markers
   and `first_id/last_id = 0`. EmptyWindow is NOT an error — auditors expect "no events"
   to be a valid finding.
2. Verify chain over the slice. The slice's first entry's `prev_hmac` is what we trust as
   `start_prev` (we are NOT verifying back to genesis — that's the global audit verifier's
   job; we verify continuity within the window). If `verify_chain(&rows[0].prev_hmac, &rows)`
   fails with `HmacMismatch(id)` or `LinkBroken(_, id)`, return `ChainBroken { first_broken_id:
   id, first_broken_ts: rows.iter().find(|r| r.id == id).map(|r| r.entry.ts).unwrap_or_else(Utc::now) }`.
3. Build `ChainProof { first_id, last_id, count, start_prev_hmac_hex, end_hmac_hex }`.
4. Call the right `project_*` and assemble the bundle.

**VC:** `cargo test -p xiaoguai-audit export_bundle_happy_path`, `export_bundle_refuses_tamper`,
`export_bundle_empty_window_is_not_error` pass.

### Step 4.4 — Implement JSON + CSV serialization

```rust
pub fn render_json(bundle: &ComplianceBundle) -> Result<String, ExportError>
pub fn render_csv(bundle: &ComplianceBundle) -> Result<String, ExportError>
pub fn render_pdf(_: &ComplianceBundle) -> Result<String, ExportError> {
    Err(ExportError::PdfUnimplemented)
}
```

JSON: `serde_json::to_string_pretty(bundle)`. The `Framework` enum serializes as kebab-case
(`soc2-cc7.2`, `gdpr-art30`, `hipaa-164.312`).

CSV: header row = `ts,actor,action,resource,details_summary`; one data row per `BundleRow`;
RFC 4180 escaping (`"` → `""`, wrap fields containing `,`, `"`, `\n` in `"..."`). Hand-rolled
to avoid adding the `csv` crate.

Both formats produce the exact same row count; CSV column semantics are a subset of the JSON
row JSON keys.

**VC:** `cargo test -p xiaoguai-audit json_and_csv_have_same_row_count`, `csv_escapes_commas_and_quotes`
pass.

### Step 4.5 — Unit tests inline in `export.rs`

Cover:
- Each framework filter (synthetic 10-row fixture → expected subset).
- Chain proof field is populated correctly on happy path.
- Tampered row triggers `ChainBroken` with the right `first_broken_id`.
- JSON ⇄ CSV row count parity (proptest-lite: random subset of `AuditEntry` actions).
- CSV escaping (commas, quotes, newlines in `details_summary`).
- PDF stub returns `PdfUnimplemented`.

**VC:** `cargo test -p xiaoguai-audit export::` green; total inline test count ≥ 10.

### Step 4.6 — Integration test

`crates/xiaoguai-audit/tests/export_integration.rs`:
1. Build a 5-entry chain using the `build_chain` helper pattern from `tests/chain_basic.rs`.
2. Call `export_bundle(Framework::Soc2Cc72, "t1", entries, window, &chain)`.
3. Assert `chain_proof.count == 5`, `chain_proof.first_id == 1`, `chain_proof.last_id == 5`,
   `start_prev_hmac_hex == "00".repeat(32)`, `end_hmac_hex == hex::encode(&entries[4].hmac)`.
4. Mutate `entries[2].entry.details = json!({"tampered": true})` — leave HMAC stored bytes
   unchanged so `verify_chain` finds `HmacMismatch(3)`.
5. Re-call `export_bundle` → assert `ChainBroken { first_broken_id: 3, .. }`.
6. Re-render the un-mutated bundle as CSV; parse CSV row count manually and assert it equals
   `bundle.rows.len()`.

**VC:** `cargo test -p xiaoguai-audit --test export_integration` green; 3 tests.

### Step 4.7 — CLI subcommand

Create `crates/xiaoguai-cli/src/commands/audit_export.rs`:

```rust
pub struct ExportArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub framework: String,   // "soc2" | "gdpr" | "hipaa"
    pub from: String,        // RFC3339
    pub to: String,
    pub output: PathBuf,
    pub format: String,      // "json" | "csv"
}

pub async fn run(args: ExportArgs) -> Result<()>;
```

Implementation: build the JSON request body, POST it to `{api_base}/v1/audit/exports`. On
2xx, write the body to `output` (binary copy). On 409 Conflict, print the error JSON to
stderr AND return `Err(...)` so the process exits non-zero. On other non-2xx, bubble up.

Wire into `xiaoguai-cli/src/commands/mod.rs` (`pub mod audit_export;`) and into the top-level
`Cli` enum in `main.rs` as a new `Audit { ... }` variant with an `AuditCmd::Export { ... }`
subcommand (so the namespace is `xiaoguai audit export ...`).

**VC:** `cargo build -p xiaoguai-cli` exits 0; `target/debug/xiaoguai audit export --help`
prints the new flags.

### Step 4.8 — API route

Create `crates/xiaoguai-api/src/routes/audit_exports.rs`:

```rust
pub async fn export_audit(
    State(state): State<AppState>,
    Json(req): Json<ExportRequest>,
) -> ApiResult<Response>;
```

Algorithm:
1. Reject if `audit` reader and `audit_verifier` aren't both wired (503).
2. Reject empty `tenant_id` or invalid `framework` (400).
3. Reject `from >= to` (400).
4. Pull rows in `[from, to]` via the reader, but ALSO need them as `StoredEntry` for
   `export_bundle`. Solution: add a parallel `AuditChainReader` trait to `xiaoguai-api::audit`
   that returns `Vec<StoredEntry>` (NOT `AuditEntryView`, which is the hex-encoded wire form).
   The Pg adapter wraps `sink.list(...)` directly. Wire it into `AppState.audit_chain` as
   `Option<Arc<dyn AuditChainReader>>`. Mirrored exactly on the static test variant.
5. Construct a `ChainedAudit` engine. For this we need the signing key. Add `AppState.audit_chain`
   to actually return a closure that produces both the rows AND a verify call — alternative:
   put the verification work on the AdapterSide via a new `AuditChainExporter` trait that just
   exposes `export_bundle_for(tenant, framework, window, format) -> Result<Vec<u8>, ExportError>`.
   Pick this: it keeps the API crate free of any direct dep on `xiaoguai-audit`, mirroring how
   `AuditReader`/`AuditVerifier` work today. The `xiaoguai-core::audit_bridge::PgAuditAdapter`
   implements this third trait.
6. Build response: 200 + body + `Content-Type` per format. On `ChainBroken`, return 409 + the
   error JSON. On `PdfUnimplemented`, return 501.

Wire the route in `crates/xiaoguai-api/src/routes/mod.rs`:
`.route("/v1/audit/exports", post(audit_exports::export_audit))`.

**VC:** `cargo build -p xiaoguai-api` exits 0; route mounted in `Router::new()`.

### Step 4.9 — Wire bridge in `xiaoguai-core`

Extend `crates/xiaoguai-core/src/audit_bridge.rs` to implement the new `AuditChainExporter`
trait on `PgAuditAdapter`. The implementation:
1. `list(tenant, since=from, until=to, limit=large)` to pull rows in the window.
2. Call `xiaoguai_audit::export::export_bundle(framework, tenant, rows, window,
   self.sink.chain())`.
3. Render as `format` and return `Vec<u8>`.

Wire `AppState.audit_chain_exporter: Option<Arc<dyn AuditChainExporter>>` in `run_serve`.

**VC:** `cargo build -p xiaoguai-core` exits 0; integration is testable via the in-memory
`StaticAuditChainExporter` reader.

### Step 4.10 — Runbook

`docs/runbooks/compliance-export.md`:
- One section per framework: what the spec asks for, what xiaoguai records, what gaps remain
  (e.g. SOC2 CC7.2 asks for "logging is reviewed periodically" — we record review events but
  evidence of review is out of scope of the audit chain itself).
- A sample auditor-question table: "Show me all access to PHI in Q1 2026" →
  `xiaoguai audit export --framework hipaa --from 2026-01-01T00:00:00Z --to 2026-04-01T00:00:00Z`.
- "Chain broken — what now?" section: explains the 409 / non-zero exit, points to
  `/v1/admin/audit/verify` for diagnosis.
- PDF follow-up note: "PDF rendering is not yet implemented. Track in T6 (post-tier-3)."

**VC:** runbook file exists, includes all three frameworks, includes at least one
auditor-question example per framework, includes PDF deferred note.

### Step 4.11 — Final verification

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo test -p xiaoguai-audit -p xiaoguai-cli -p xiaoguai-api
cargo clippy -p xiaoguai-audit -p xiaoguai-cli -p xiaoguai-api -- -D warnings
```

**VC:** all four commands exit 0.

### Step 4.12 — Commit + PR

Commit on `feat/tier3-compliance-export`, push, `gh pr create --title "feat(tier-3): compliance
export from audit chain (T5)"` with summary + test plan body.

**VC:** PR URL printed; test count from `cargo test` output captured.

## 5. Risks & open questions

| Risk | Mitigation |
|---|---|
| The API layer doesn't currently depend on `xiaoguai-audit`. Adding `AuditChainExporter` to the api crate as another trait keeps that invariant. | Use the existing trait-bridge pattern from `AuditReader`/`AuditVerifier`. |
| `export_bundle` needs the `ChainedAudit` engine (which holds the signing key). The api crate must not see the key. | The bridge in `xiaoguai-core` constructs the engine + renders inside the adapter; the api crate only sees `Result<Vec<u8>, ExportError>`. |
| If a tenant has millions of rows, `reader.list(..., limit=large)` may OOM. | Cap `limit` at 100 000 in the route handler; document the cap in the runbook. Streaming export is out of scope for this PR. |
| The "first broken row's ts" lookup may fail if `id` doesn't match any row in the window (theoretically possible if the chain breaks at the very first row when `prev_hmac` is wrong). | Fall back to `Utc::now()` with a `tracing::warn` log. The auditor still sees the right `id`. |
| Hardcoded action filters may not match the real action strings used in production. | Document the filter set in the runbook; future work can add a config override (Out of scope). |
| Sub-plan was written without the parent sprint file on disk. | This sub-plan is self-contained; if the parent appears with conflicting scope, flag in PR review before merge. |

## 6. Rollback

Revert the PR with `git revert <merge-sha>`. No schema migration is needed (the export is a
read-only projection of existing `audit_log` rows). No new env vars are required (the signing
key is already present for the chain). Operators relying on the CLI subcommand or the HTTP
endpoint will see "command not found" / 404 after revert — that's the correct rollback
behaviour.

## 7. Out of scope

- PDF rendering (stub only; tracked as follow-up).
- Streaming / paginated export for very large windows (capped at 100 000 rows).
- Runtime template DSL (templates are static `match` arms).
- Cross-tenant export (the audit chain is per-tenant, so is the export).
- Custom action-filter overrides (hardcoded per framework).
- Re-signing of the export bundle (the chain proof in the header is the integrity guarantee).
- E2E test against live Postgres (use in-memory fixture, mirroring PR #72 pattern).

## 8. References

- `crates/xiaoguai-audit/src/chain.rs` — `AuditEntry`, `StoredEntry`, `ChainedAudit`, `verify_chain`,
  `HMAC_LEN`, `ChainError`.
- `crates/xiaoguai-audit/src/sink.rs` — `PgAuditSink::list/verify_tenant`, the in-prod chain reader.
- `crates/xiaoguai-audit/tests/chain_basic.rs` — `build_chain` helper pattern to copy.
- `crates/xiaoguai-storage/migrations/0002_audit.sql` — `audit_log` schema confirms columns.
- `crates/xiaoguai-api/src/audit.rs` — existing `AuditReader`, `AuditVerifier`, `StaticAudit*`
  fixtures.
- `crates/xiaoguai-api/src/routes/admin.rs::verify_audit` — pattern for an audit-related
  HTTP endpoint with 503 / 400 / 200 behaviour.
- `crates/xiaoguai-cli/src/commands/outcomes.rs` — pattern for a CLI subcommand that POSTs JSON
  to the API.
- `crates/xiaoguai-core/src/audit_bridge.rs::PgAuditAdapter` — pattern for a Pg-side adapter
  that implements API traits without exposing the storage layer to the api crate.
- `docs/HANDOFF-2026-05-28-session5.md` — confirms Tier-3 compliance export is the open task.

## Self-review

**6-point protocol:**

1. **Cited paths exist** —
   - `crates/xiaoguai-audit/src/{chain,sink,redact,outcomes,lib}.rs` — verified via `ls`.
   - `crates/xiaoguai-audit/tests/chain_basic.rs` — verified.
   - `crates/xiaoguai-storage/migrations/0002_audit.sql` — verified.
   - `crates/xiaoguai-api/src/{audit,state}.rs` — verified.
   - `crates/xiaoguai-api/src/routes/{admin,mod}.rs` — verified.
   - `crates/xiaoguai-cli/src/commands/{outcomes,mod}.rs`, `crates/xiaoguai-cli/src/main.rs` —
     verified.
   - `crates/xiaoguai-core/src/audit_bridge.rs` — verified.
   - `docs/HANDOFF-2026-05-28-session5.md` — verified.
   PASS.

2. **Every VC runnable** — each `VC:` line is either a `cargo` invocation (check / test /
   build / clippy / fmt) or a "file exists" assertion that `ls` can verify. PASS.

3. **Criteria → step mapping** —
   - SC 1 (module `export.rs`) → 4.1.
   - SC 2 (`export_bundle` + header) → 4.1, 4.3.
   - SC 3 (chain-broken refusal) → 4.3, 4.6.
   - SC 4 (JSON canonical, CSV projection parity) → 4.4, 4.5.
   - SC 5 (PDF stub) → 4.4, 4.5.
   - SC 6 (CLI subcommand) → 4.7.
   - SC 7 (HTTP endpoint) → 4.8, 4.9.
   - SC 8 (per-framework unit tests) → 4.2, 4.5.
   - SC 9 (integration test) → 4.6.
   - SC 10 (cargo green) → 4.11.
   - SC 11 (runbook) → 4.10.
   - SC 12 (PR opened) → 4.12.
   All 12 criteria map to at least one step. PASS.

4. **Out-of-scope honored** — PDF rendering, streaming export, runtime template DSL, cross-
   tenant export, custom filter overrides, live-PG E2E, and re-signing are all in §7 and
   none of the steps in §4 implement them. PASS.

5. **Risks have mitigations** — five enumerated risks each carry a mitigation; the "parent
   sprint file not on disk" risk is acknowledged with a flag for PR review. PASS.

6. **Time budget sane** — twelve steps. Steps 4.1, 4.4, 4.10 are ~30 min each. Steps 4.2,
   4.3, 4.5, 4.6 are ~45 min each. Steps 4.7, 4.8, 4.9 are ~60 min each (most plumbing work).
   Step 4.11 is ~15 min. Step 4.12 is ~15 min. Total ~7-8 hours of focused work. PASS.

**Self-review verdict: PASS — proceeding to implementation.**
