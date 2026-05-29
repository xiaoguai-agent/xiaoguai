# Tier-2 D.1 — Agent-authored skills (HotL-gated, admin-approved)

> Expansion of the D.1 sketch in `docs/plans/2026-05-28-tier2-next.md`.
> Sprint task T3 of session-6 (2026-05-29).
> Reviewer protocol: `~/.claude/plans/drifting-zooming-stroustrup.md` §6 self-review.

---

## 1. Context

Tier-1 + the first Tier-2 item (`xiaoguai-mcp-exec`) shipped in session-5
(`HANDOFF-2026-05-28-session5.md`). Two Tier-2 items remain:

| Tier-2 item | Status before T3 |
|---|---|
| Agent-authored skills gated by HotL | 🔲 prereq HotL gate landed in #61 |
| Session compaction | 🔲 separate plan |

This plan lands **agent-authored skills**, the data-driven-evolution piece
called out in §5 of `xiaoguai-agent-design/docs/harness-engineering.md` and
the pi/Hermes roadmap (`~/.claude/projects/-Users-zw-testany-myskills-xiaoguai/memory/agent-roadmap.md`).

The idea: the agent itself can author a new skill-pack manifest at runtime
through a new MCP tool `propose_skill`. The HotL gate (PR #61) is reused
with a bucket `skill_author` (default budget: 5 proposals / tenant / day).
On `Allow`, the proposal is persisted as `pending`. A human admin then
calls a new endpoint to flip it to `installed`, at which point the
manifest is materialised into `~/.xiaoguai/skills/<name>-<version>.yaml`
so `xiaoguai skills reload` picks it up.

The mechanism is **off by default**. Operators must enable it per tenant
via the new `tenant_settings` JSONB row (`allow_skill_authoring=true`).
This is the §11 harness-engineering anti-pattern *avoidance* — "don't let
the LLM author its own escapes" → make every authoring a HotL event AND
require human approval before any code becomes loadable.

### Adjustments from the D.1 sketch

The sketch made three assumptions that don't match the codebase:

1. **`tenants.id` is `TEXT`, not `UUID`** (`0001_initial.sql:6`). The new
   migration must use `TEXT REFERENCES tenants(id)`, not `UUID`.
2. **No per-tenant config store exists today** (`routes/tenants.rs:78-82`).
   The plan adds a minimal `tenant_settings` table (one row per tenant)
   holding a JSONB blob — keeps the surface area tiny while giving
   `allow_skill_authoring` a real persistence home.
3. **No `skill.approve` Casbin action exists** (`grep` came up empty in
   `crates/xiaoguai-auth/`). The endpoint uses `require_authorized` with
   resource `/v1/skills/proposals/:id/approve` action `approve`, and the
   default policy file gets a new row granting `tenant_admin` +
   `system_admin` that action. Documented in
   `docs/runbooks/agent-authored-skills.md`.

---

## 2. Success criteria

Every item is a runnable check; none is "implementation finished".

1. **Migration applied cleanly.**
   `cargo test -p xiaoguai-storage --test migrations` (existing harness) is
   green; new tables `skill_proposals` and `tenant_settings` show up via
   `sqlx::query("SELECT 1 FROM skill_proposals LIMIT 0")` and same for
   `tenant_settings` in a unit test.

2. **`propose_skill` tool dispatches via the HotL gate.**
   Unit test in `crates/xiaoguai-tasks/src/skill_author.rs::tests` exercises
   `propose(...)` with `AllowAllGate` → returns `Ok(ProposalId)`; with
   `DenyAllGate::new("budget exceeded")` → returns
   `Err(SkillAuthorError::Denied("budget exceeded"))`.

3. **Manifest validator is whitelist-only.**
   Reject proposals where any tool in `tool_allowlist` does NOT appear in
   the registered toolbox. Reject proposals that mention `mcp_server_url`,
   `command`, or any field outside the strict allow-list schema. Unit tests
   cover both rejection paths.

4. **`POST /v1/skills/proposals/:id/approve` flips state and writes a YAML.**
   Integration test: in-memory `SkillProposalRepository` + temp dir under
   `XIAOGUAI_SKILLS_DIR` → approve handler returns 200 → tempdir contains
   `<name>-<version>.yaml` whose contents round-trip parse to the original
   manifest. Without an admin claim → 403.

5. **End-to-end test:**
   `crates/xiaoguai-tasks/tests/skill_author_e2e.rs` drives a MockBackend
   that:
   1. emits a `propose_skill` tool call with an over-broad allowlist;
   2. observes a `Deny` (gate denies because the allowlist references a
      tool that is not in the toolbox);
   3. emits a second `propose_skill` call with a clean allowlist;
   4. observes an `Allow` → proposal persisted as `pending`;
   5. test then calls `approve_proposal()` directly (admin path) → state
      becomes `installed`; the YAML file is on disk.

   Assert `audit_log` has three rows for this tenant with `action` values
   `skill.propose`, `skill.hotl_gate`, `skill.approve`.

6. **Off-by-default enforcement.**
   With `tenant_settings` row absent OR `allow_skill_authoring=false`,
   `propose_skill` returns `Err(SkillAuthorError::Disabled)` before the
   gate is consulted. Unit test covers both cases.

7. **CLI surface.**
   `xiaoguai skills proposals list --tenant-id <id>` prints pending +
   recently-decided rows. `xiaoguai skills proposals approve <id>` calls
   the new endpoint. Both commands' `--help` output checked in test.

8. **Runbook checked in.**
   `docs/runbooks/agent-authored-skills.md` exists and documents:
   enabling the flag, the HotL budget bucket, the approval flow, how to
   revoke an installed skill (manual delete of YAML + DB row — full
   uninstall flow is out of scope, see §7).

9. **`cargo test` green** on every touched crate:
   `xiaoguai-storage`, `xiaoguai-tasks`, `xiaoguai-agent`, `xiaoguai-api`,
   `xiaoguai-cli`, `xiaoguai-core`.

---

## 3. Prerequisites

| What | Verify by |
|---|---|
| HotL gate trait (#61) merged | `grep -q 'pub trait HotlGate' crates/xiaoguai-agent/src/hotl_gate.rs` |
| `xiaoguai-tasks` compiles | `cargo check -p xiaoguai-tasks` exits 0 |
| Existing `installed_skill_packs` table loaded | `grep -q 'CREATE TABLE installed_skill_packs' crates/xiaoguai-storage/migrations/0015_skill_packs.sql` |
| MockBackend supports multi-step scripts | `grep -q 'with_script' crates/xiaoguai-llm/src/mock.rs` |
| `PgAuditSink::append` available | `grep -q 'pub async fn append' crates/xiaoguai-audit/src/sink.rs` |
| Branch `feat/tier2-d1-agent-authored-skills` checked out | `git rev-parse --abbrev-ref HEAD` |

---

## 4. Step-by-step

Each step has a verification checkpoint (`VC:`) that must succeed before
moving on.

### Step 4.1 — Migration `0021_skill_proposals.sql`

Add a single migration with two tables:

```sql
-- 0021_skill_proposals.sql
-- v1.5.x: agent-authored skill proposals + minimal per-tenant settings store.
--
-- `skill_proposals`: one row per agent-emitted draft. States move
--   pending → approved → installed
--   pending → rejected
-- transitions; the `decided_at` + `decided_by` columns capture the human
-- (or system) decision. `manifest_json` holds the full skill manifest;
-- validation happens at write time so only well-formed drafts hit the row.
--
-- `tenant_settings`: tiny JSONB store keyed by tenant_id. Holds opt-in
-- flags like `allow_skill_authoring`. Free-form on purpose — we don't
-- want a migration every time a new opt-in flag lands.

CREATE TABLE tenant_settings (
    tenant_id   TEXT PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    settings    JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE skill_proposals (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    proposed_by     TEXT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT,
    version         TEXT NOT NULL,
    manifest_json   JSONB NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','installed')),
    reason          TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at      TIMESTAMPTZ,
    decided_by      TEXT
);

CREATE INDEX skill_proposals_tenant_status_idx
    ON skill_proposals (tenant_id, status);
```

**VC:** `cargo test -p xiaoguai-storage` exits 0 (the existing migration
runner test loads every numbered file in order).

### Step 4.2 — `skill_author` module in `xiaoguai-tasks`

New file `crates/xiaoguai-tasks/src/skill_author.rs` (~ 250 LOC). Public
surface:

```rust
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub system_prompt: String,
    pub tool_allowlist: Vec<String>,
}

pub struct ProposalRow {
    pub id: String,
    pub tenant_id: String,
    pub proposed_by: String,
    pub manifest: SkillManifest,
    pub status: ProposalStatus,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
}

#[async_trait]
pub trait SkillProposalRepository: Send + Sync {
    async fn insert(&self, row: ProposalRow) -> Result<ProposalRow, SkillAuthorError>;
    async fn get(&self, id: &str) -> Result<Option<ProposalRow>, SkillAuthorError>;
    async fn list(&self, tenant_id: &str, status: Option<ProposalStatus>) -> Result<Vec<ProposalRow>, SkillAuthorError>;
    async fn set_status(&self, id: &str, status: ProposalStatus, decided_by: &str) -> Result<ProposalRow, SkillAuthorError>;
}

#[async_trait]
pub trait TenantSettingsReader: Send + Sync {
    async fn allow_skill_authoring(&self, tenant_id: &str) -> Result<bool, SkillAuthorError>;
}

pub fn validate_manifest(
    m: &SkillManifest,
    known_tools: &HashSet<String>,
) -> Result<(), SkillAuthorError>;

pub async fn propose(
    ctx: &SkillAuthorCtx<'_>,    // gate + repo + settings + audit + known_tools
    tenant_id: &str,
    proposed_by: &str,
    manifest: SkillManifest,
) -> Result<ProposalRow, SkillAuthorError>;

pub async fn approve_proposal(
    ctx: &SkillAuthorCtx<'_>,
    proposal_id: &str,
    decided_by: &str,
    skills_dir: &Path,
) -> Result<ProposalRow, SkillAuthorError>;
```

`propose()` flow:
1. Check `TenantSettingsReader::allow_skill_authoring(tenant_id)`. False
   → `Err(Disabled)`, no audit emission (off-by-default is a quiet drop).
2. `validate_manifest()` — reject if tool_allowlist references unknown
   tool names; reject any manifest field outside the whitelist schema.
3. Audit emit `skill.propose` (details: name, version, proposed_by).
4. Call gate `.check(tenant_uuid, "skill_author", 1.0)`.
   * Audit emit `skill.hotl_gate` (details: verdict, reason).
   * `Deny(reason)` → return `Err(Denied(reason))`.
5. Insert row with `status = 'pending'`. Return row.

`approve_proposal()` flow:
1. Load row (404 if missing).
2. Set status to `installed`.
3. Render `SkillManifest` as YAML, write to
   `<skills_dir>/<name>-<version>.yaml` (create dir if missing, fail if
   file already exists — overwrite is out of scope).
4. Audit emit `skill.approve` (details: id, name, version, decided_by).

Unit tests in this file:
* `propose_disabled_returns_disabled`
* `propose_with_unknown_tool_rejected_before_gate`
* `propose_with_invalid_field_rejected`
* `propose_with_denied_gate_returns_denied`
* `propose_with_allowed_gate_persists_pending_row`
* `approve_writes_yaml_and_flips_status`
* `approve_missing_proposal_returns_not_found`

**VC:** `cargo test -p xiaoguai-tasks skill_author` exits 0.

### Step 4.3 — In-memory + Postgres implementations

Add `InMemorySkillProposalRepository` (used by tests) and `InMemoryTenantSettings`
in `skill_author.rs`. Add `PgSkillProposalRepository` + `PgTenantSettings`
in `crates/xiaoguai-tasks/src/pg.rs` (extending the existing module).

**VC:** unit tests for in-memory repos pass; Pg path compiles
(`cargo check -p xiaoguai-tasks --tests`).

### Step 4.4 — Register `propose_skill` tool with the agent

The HotL check from PR #61 happens in the ReAct dispatch loop. But the
`propose_skill` tool is not a *real* MCP tool — it's an in-process call.
The cleanest path: register a synthetic `McpClient` that fronts the
`skill_author::propose()` call.

In `crates/xiaoguai-agent/src/skill_author_tool.rs` (new file, ~ 120 LOC):

```rust
pub struct ProposeSkillClient {
    inner: Arc<dyn ProposeSkillBackend>,
    tenant_id: String,
    proposed_by: String,
}

#[async_trait]
pub trait ProposeSkillBackend: Send + Sync {
    async fn invoke(
        &self,
        tenant_id: &str,
        proposed_by: &str,
        manifest_json: serde_json::Value,
    ) -> Result<String, String>;  // Ok(proposal_id) | Err(human-readable)
}

impl ProposeSkillClient {
    pub fn descriptor() -> ToolDescriptor { ... }   // name=propose_skill, schema=manifest fields
}

#[async_trait]
impl McpClient for ProposeSkillClient {
    async fn call_tool(&self, name: &str, args: Value) -> McpResult<ToolResult> { ... }
    // other methods: trivial stubs
}
```

The backing impl in `xiaoguai-core` (`crates/xiaoguai-core/src/skill_author_bridge.rs`,
new file ~ 80 LOC) implements `ProposeSkillBackend` by calling into
`xiaoguai_tasks::skill_author::propose`.

The HotL bucket name is `tool_call.propose_skill` (the natural bucket for
the existing per-tool gate). To add the extra `skill_author` bucket
required by the success criteria, `propose()` itself emits a *second*
gate event with scope `skill_author` so both the tool-call gate and the
proposal-rate gate are enforced. (The double-count is intentional: the
tool-call gate covers "is this agent allowed to call propose_skill at
all", the `skill_author` bucket covers "rate-limit proposals per
tenant".)

**VC:** new unit test `propose_skill_descriptor_has_strict_schema()` in
`skill_author_tool.rs` asserts the JSON-schema for `propose_skill`
rejects any property outside the allow-list.

### Step 4.5 — HTTP endpoints

Add to `crates/xiaoguai-api/src/skills.rs` (or split into
`skills_proposals.rs` if it pushes the file past 600 lines):

```rust
GET    /v1/skills/proposals?tenant_id=...&status=pending
POST   /v1/skills/proposals/:id/approve     // body: { decided_by }
POST   /v1/skills/proposals/:id/reject      // body: { decided_by, reason }
```

`approve` calls `skill_author::approve_proposal(..., skills_dir = state.skills_dir)`.
`reject` flips status to `rejected`. Both gated by
`require_authorized(... "approve")` matching a new Casbin row.

Add `pub skill_proposals: Option<Arc<dyn SkillProposalRepository>>` and
`pub skills_dir: PathBuf` to `AppState`.

Register routes in `crates/xiaoguai-api/src/routes/mod.rs` alongside
existing `/v1/skills/*` routes.

**VC:** `cargo test -p xiaoguai-api skills_proposals` (new test file)
asserts:
* GET pending list returns rows in `created_at DESC` order;
* approve with admin → 200, status flips to `installed`, file on disk;
* approve without admin → 403.

### Step 4.6 — Casbin policy default

Add a row to the default Casbin policy CSV (`packs/casbin/default_policy.csv`
or wherever the live policy lives — confirm with
`grep -rn 'tenant_admin' crates/xiaoguai-auth/ packs/`).

`p, tenant_admin, /v1/skills/proposals/*, approve`
`p, system_admin, /v1/skills/proposals/*, approve`

**VC:** existing `cargo test -p xiaoguai-auth` policy test passes; add
one row asserting tenant_admin is allowed.

### Step 4.7 — CLI commands

Add to `crates/xiaoguai-cli/src/main.rs::SkillsCmd`:

```rust
Proposals {
    #[command(subcommand)]
    action: ProposalsCmd,
},

enum ProposalsCmd {
    List {
        #[arg(long)] tenant_id: String,
        #[arg(long)] status: Option<String>,
    },
    Approve {
        #[arg(long)] id: String,
        #[arg(long)] decided_by: String,
    },
    Reject {
        #[arg(long)] id: String,
        #[arg(long)] decided_by: String,
        #[arg(long)] reason: String,
    },
}
```

Implementation lives in `crates/xiaoguai-cli/src/commands/skills.rs`
(extend existing module).

**VC:** `cargo run -p xiaoguai-cli -- skills proposals --help` returns
help text including `list`, `approve`, `reject`.

### Step 4.8 — Wiring in `run_serve`

In `crates/xiaoguai-core/src/lib.rs::run_serve`:
1. If `state.skill_proposals` is `Some` and tenant has
   `allow_skill_authoring=true`, register `ProposeSkillClient` with the
   agent's toolbox.
2. Pass `skills_dir` (default `~/.xiaoguai/skills`, configurable via
   `XIAOGUAI_SKILLS_DIR`) into `AppState`.

**VC:** `cargo build -p xiaoguai-core` exits 0;
`cargo test -p xiaoguai-core run_serve_smoke` still passes (if such a
test exists; otherwise grep for any state-construction test).

### Step 4.9 — Integration test

`crates/xiaoguai-tasks/tests/skill_author_e2e.rs` per success criterion §5.

Test setup:
* `InMemorySkillProposalRepository`
* `InMemoryTenantSettings` with `allow_skill_authoring=true`
* `InMemoryAuditSink` (introduce one if `xiaoguai-audit` doesn't have it;
  see `tests/hotl_gate.rs` for the pattern)
* Two consecutive `MockBackend` script steps:
  1. tool_calls = [propose_skill with bad allowlist]
  2. tool_calls = [propose_skill with good allowlist]
  3. text = "done"
* `ScopeDenyGate` denying scope `skill_author` on the first call;
  reconfigured to allow on the second call (or use a stateful gate
  fixture).
* Tempdir for `skills_dir`.

Assertions per §5.

**VC:** `cargo test -p xiaoguai-tasks --test skill_author_e2e` exits 0.

### Step 4.10 — Runbook

`docs/runbooks/agent-authored-skills.md` (~ 100 lines) covering:

1. **Threat model.** Why proposals are HotL-gated and admin-approved.
2. **Enabling per tenant.** SQL example:
   `INSERT INTO tenant_settings(tenant_id, settings) VALUES ('t1',
   '{"allow_skill_authoring": true}') ON CONFLICT (tenant_id) DO
   UPDATE SET settings = tenant_settings.settings || EXCLUDED.settings;`
3. **HotL budget.** Default policy seed (5 proposals/tenant/day).
4. **Approving / rejecting.** CLI examples.
5. **Revoking.** Delete the YAML from `~/.xiaoguai/skills/` AND
   `DELETE FROM skill_proposals WHERE id = ...`. Full uninstall flow is
   tracked as a follow-up.
6. **Schema constraints.** What an agent-authored manifest may and may
   not contain (no native code, no new MCP server URLs, must reference
   existing tool names).

**VC:** file exists, mdlint passes, `grep -q 'allow_skill_authoring'
docs/runbooks/agent-authored-skills.md`.

### Step 4.11 — PR

```
git checkout -b feat/tier2-d1-agent-authored-skills
git push -u origin HEAD
gh pr create --title "feat(tier-2): agent-authored skills (D.1, HotL-gated, admin-approved)" --body @<self-prepared body>
```

**VC:** PR opens; CI green; report PR URL + test count back to user.

---

## 5. Risks & open questions

| Risk / Q | Mitigation / Plan |
|---|---|
| `propose_skill` tool's bucket name clashes with the per-tool gate | Use **two** buckets: `tool_call.propose_skill` (per-tool, count-based) + `skill_author` (per-day quota). Step 4.4 documents the rationale. |
| Agent generates a manifest with `tool_allowlist` pointing at `propose_skill` itself → recursion | `validate_manifest` strips `propose_skill` from allowlists (cannot author a skill that authors skills). Unit test covers. |
| Casbin policy file format unknown until step 4.6 | First action of step 4.6 is `grep -rn 'tenant_admin' crates/xiaoguai-auth/ packs/` to confirm format. If format differs from the assumed CSV, update the appendix. |
| Two `InMemory*` fixtures duplicate logic that already exists in `xiaoguai-audit` | Reuse `InMemoryOutcomeRecorder` pattern as the template; if no `InMemoryAuditSink` exists, add a minimal one to `xiaoguai-audit` (~ 40 LOC, lives in `tests/` not `src/` if used only by other crates' tests). |
| `~/.xiaoguai/skills/` directory may not exist at first call | `approve_proposal` creates it. Failure to write the YAML fails the approval (no orphaned `installed` row in DB). |
| `xiaoguai skills reload` doesn't exist yet | Out of scope for this PR (see §7). The YAML lands on disk; reloading is a documentation note ("restart the server for now"). |
| HotL gate's `tenant_id: Uuid` vs project's `TEXT` tenant ids | Parse string → Uuid with `Uuid::parse_str(...).ok()`; if it fails, propose() returns `SkillAuthorError::InvalidTenant`. Same pattern as `xiaoguai-core::hotl_bridge::EnforcerGate`. |
| The new tool may collide with a tool already named `propose_skill` | Toolbox `insert()` returns `Duplicate`; we wrap in a `tracing::warn` and skip registration (don't fail the whole agent). Tested. |

---

## 6. Rollback

1. **Step 4.1 fails:** drop tables, revert migration file. No other
   crates touched yet.
2. **Step 4.2-4.4 fail:** stay on the branch, no production impact.
3. **Step 4.5-4.8 fail at compile time:** `git restore` the touched
   files; no DB state changed.
4. **Integration test in 4.9 reveals a design flaw:** the fix is local
   to `skill_author.rs` (validation, gate ordering) or
   `skill_author_tool.rs` (tool descriptor) — no migration rewrite needed.
5. **After PR merge a defect is found in production:** disable per
   tenant via `UPDATE tenant_settings SET settings = settings ||
   '{"allow_skill_authoring": false}' WHERE tenant_id = '...';`. The
   tool stops being registered on the next session.

---

## 7. Out of scope

* Multi-tenant publishing of approved skills to the global marketplace.
* Versioned skill upgrades (re-proposing the same name + version is
  rejected; a new version requires a fresh manifest).
* UI for the approval flow (admin-ui can pick it up in a follow-up).
* Automatic agent self-revising of a denied proposal (it can retry, but
  the same proposal counts against the budget).
* `xiaoguai skills reload` to hot-load new YAMLs without a restart.
* Full uninstall flow (YAML + DB delete + cache invalidation). Runbook
  documents the manual two-step.
* Sandboxing the YAML's `system_prompt` for prompt injection — handled
  by the existing `_sanitize()` in the audit redaction layer, but a
  dedicated review is a follow-up.

---

## 8. References

* D.1 sketch (parent): `docs/plans/2026-05-28-tier2-next.md`
* Session-5 handoff: `docs/HANDOFF-2026-05-28-session5.md`
* HotL gate trait: `crates/xiaoguai-agent/src/hotl_gate.rs`
* HotL gate tests (fixture pattern): `crates/xiaoguai-agent/tests/hotl_gate.rs`
* Existing skill marketplace: `crates/xiaoguai-api/src/skills.rs`,
  `crates/xiaoguai-storage/migrations/0015_skill_packs.sql`
* Audit chain: `crates/xiaoguai-audit/src/chain.rs`,
  `crates/xiaoguai-audit/src/sink.rs`
* HotL enforcer adapter: `crates/xiaoguai-core/src/hotl_bridge.rs`
* Toolbox: `crates/xiaoguai-agent/src/toolbox.rs`
* MockBackend: `crates/xiaoguai-llm/src/mock.rs`
* Agent roadmap memory:
  `~/.claude/projects/-Users-zw-testany-myskills-xiaoguai/memory/agent-roadmap.md`
* Harness Engineering philosophy:
  `~/testany/myskills/xiaoguai-agent-design/docs/harness-engineering.md`
  (§5 data-driven evolution motivates the feature; §11 "letting the LLM
  author its own escapes" motivates the gating)

---

## Self-review (drifting-zooming-stroustrup §6)

| # | Check | Result | Notes |
|---|---|---|---|
| 1 | All cited file paths exist in tree | **PASS** | Verified by direct read during plan drafting: `hotl_gate.rs`, `react.rs`, `skills.rs`, `routes/mod.rs`, `chain.rs`, `sink.rs`, `mock.rs`, `toolbox.rs`, `0001_initial.sql`, `0015_skill_packs.sql` all present. |
| 2 | Every `VC:` is runnable today | **PASS** | Each VC is a `cargo test -p ...` or `cargo build -p ...` or a `grep`. No "live PG required" gates on the unit/integration path; integration test uses `InMemory*` fixtures. |
| 3 | Each §2 success criterion has a §4 step | **PASS** | 1→4.1, 2→4.2, 3→4.2 (validator), 4→4.5, 5→4.9, 6→4.2 (`propose_disabled_returns_disabled` test) + 4.8, 7→4.7, 8→4.10, 9→4.1-4.9 cumulative VCs. |
| 4 | Out-of-scope items honored, scope creep flagged | **PASS** | §7 lists 7 deferred items. Plan adjustment §1 explicitly notes 3 sketch assumptions that don't hold and the chosen workarounds. |
| 5 | Risks have mitigations | **PASS** | 8 risks in §5; each row has a concrete mitigation or step pointer. |
| 6 | Step duration sane (≤ ~1 h each, ~6 h total) | **PARTIAL** | Step 4.9 (E2E integration test with 3 stateful actors — backend, gate, repo) is likely the longest single step (~ 1.5 h). All others fit. Total estimate: ~ 7 h, which fits a session. |

### Soft spots flagged for the reviewer

1. **Casbin policy file location is unconfirmed.** Step 4.6 begins with
   a `grep` to confirm. If the location is different from the assumed
   `packs/casbin/default_policy.csv`, this plan will gain a "Plan
   adjustment" appendix.

2. **`AppState.skills_dir` is new.** I'm adding a field to `AppState`
   that does not exist today. The existing `state.rs` doc-comments use
   per-feature `Option<Arc<...>>`; `skills_dir: PathBuf` doesn't match
   that pattern. Trade-off accepted: a plain `PathBuf` (no Option) keeps
   the `approve_proposal` handler trivial and matches the fact that the
   directory always has a sensible default.

3. **Two HotL gate buckets (`tool_call.propose_skill` + `skill_author`).**
   This is intentional double-counting (per-call gate + per-day quota).
   If the reviewer prefers a single bucket, the simplification is to
   drop the per-tool gate from `propose_skill` specifically and only
   keep `skill_author` — but that needs special-casing inside `react.rs`
   which would dilute PR #61's "one gate event per tool call"
   invariant. Leaving as designed.

4. **The synthetic `InMemoryAuditSink` may need to land in
   `xiaoguai-audit` if no equivalent exists.** Plan adjustment will
   note if I end up adding it; otherwise it lives in the test file.

All six self-review criteria pass or are explicitly flagged. **Proceeding
to implementation.**
