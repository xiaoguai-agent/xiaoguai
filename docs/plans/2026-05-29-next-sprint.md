# Next sprint — task table + detailed sub-plans (session-7)

> Companion to session-5 handoff and the four session-6 plans
> (`docs/plans/2026-05-28-*.md`). Meta-plan style; written *before*
> execution so the user can review and re-rank.

## 1. Context

Session-6 closed the following:

- **Plan D.2 (compaction)** — shipped in PR #66 (token estimator, `compact()`,
  ReAct loop wiring, 3 Prometheus metrics, 13 new tests, runbook).
- **Plan B safe portion** — PR #67 (systemd `ExecStart=/usr/local/bin/xiaoguai
  serve`, README install matrix).
- **Plan A docs/script** — PR #68 (`docs/scripts/demo-mcp-exec.sh`,
  `mcp-exec-sandbox.md` E2E section).
- **Plan C Phase 1** — PR #69 (`docs/architecture/design-link.md`); Phase 2
  briefing staged in design repo.
- **Plan C Phase 2** — design-doc updates landed (this session, 2026-05-29):
  HLD DEC-013, lld-agent §4.4 rewritten, lld-llm §4.3 added,
  test-spec §3.1 CASE-CHAT-006…013, test-strategy §8 RISK-OPS-003.
- **Harness Engineering philosophy** doc enriched to 18 sections from the
  TRAE long-form (PPAF, 4-D 造缰/驭马/相马/育马, strategic matrix,
  3 core constraints, formal components, FC lifecycle).

This sprint picks up what's left from session-5 plus what session-6
identified as follow-ons. The user requested a **table** plus
**detailed MDs** plus **review** plus **execution**. This file is the
table + sub-plans; per-task plans live as separate `docs/plans/*.md` files
written incrementally as each is selected.

---

## 2. Backlog table

Read top-to-bottom in priority order. Each row maps to a separate
sub-plan file (to be written when the user picks it).

| Pri | ID | Task | Tier | Est. | Blocked-by | R.E.S.T axis | Owner |
|---|---|---|---|---:|---|---|---|
| P0 | **T1** | Operator-driven live recording of the agent → mcp-exec demo (Plan A §4.7 + 4.9) | Tier-2 finishing | 1 h | running stack | T (Traceability) | operator |
| P0 | **T2** | Plan B remainder — cargo-dist workflow + Homebrew tap repo | Tier-1 finishing | 2–3 h | tap repo + PAT | E + R | this session, gated |
| P1 | **T3** | Tier-2 D.1 — agent-authored skills (HotL-gated) | Tier-2 | ~6 h | none (HotL gate already in #61) | S + 育马 (data-driven evolution) | this session |
| P1 | **T4** | Tier-3 OAuth 2.1 PKCE for outbound MCP servers | Tier-3 | ~6 h | none | S | this session |
| P1 | **T5** | Tier-3 compliance export from audit chain (SOC2 / GDPR / HIPAA report templates) | Tier-3 | ~4 h | none | T | this session |
| P2 | **T6** | `execute_javascript` MCP server (Hermes parity) | Tier-2 sibling of #64 | ~6 h | new crate scaffold | E + S | this session |
| P2 | **T7** | wasmtime + pyodide sandbox upgrade (L3 sandbox tier) | sandbox L3 | 1–2 wks | feasibility study | S | not this sprint |
| P2 | **T8** | Plan C Phase 2 reviewer pass — run `/testany-eng:hld-reviewer` etc. against the doc updates we just made | docs polish | 30 min | fresh session | T | fresh session needed |
| P3 | **T9** | Notarised macOS `.pkg` + Windows code-signing | release polish | 1 wk | Apple Dev ID, EV cert | E | deferred |
| P3 | **T10** | Cleanup zombie CI workflows (`k6` / `mdbook` / `perf-regression` non-gating) | infra cleanup | 1 h | none | E | this session |

P0 = blocks ship readiness; P1 = roadmap-critical; P2 = nice-to-have;
P3 = deferred. T7 and T9 are outside the sprint by intent (each is
multi-week or needs paid accounts).

---

## 3. Sub-plan sketches

Each sketch is the body that will go into a per-task `docs/plans/*.md`
when selected. Same 8-section template as session-6 plans.

### T1 — Operator-driven live demo recording

**Why this is small but P0.** Code is in tree (PR #68). Needs an operator
shell with Postgres + Ollama running. Without the recording, the demo
script is unverified.

**Steps the sub-plan will spell out** — boot the stack, follow plan A
§4.1–4.6 verbatim, then `asciinema rec -c "bash
docs/scripts/demo-mcp-exec.sh" docs/asciinema/agent-mcp-exec-e2e.cast`,
commit the cast.

**Out of scope.** Hosting the cast on asciinema.org (cast file is
sufficient for the runbook). Adding a second tenant (single-tenant is
the demo).

### T2 — cargo-dist + Homebrew tap (Plan B remainder)

**Steps**: per Plan B §4.1–4.5 and §4.7–4.11. Gated on the user
creating `xiaoguai-agent/homebrew-tap` repo and provisioning
`HOMEBREW_TAP_TOKEN` PAT (the only manual prereqs). Workflow generation
and tarball matrix are automated.

**Open question for the sub-plan**: keep `release-tarball.yml` as a
parallel SLSA-L3 path or retire it. Plan B recommended retire; this
sub-plan will lock in a final decision.

**Risk**: cargo-dist's glibc floor differs from `release-packages.yml`'s
ubuntu-22.04 baseline. Pin runners explicitly.

### T3 — Agent-authored skills, HotL-gated (Plan D.1)

**Goal**: agent can author a new skill-pack at runtime; HotL gate
forwards the manifest to a human operator for approval; approved skills
land in `skill_proposals` → `skills` and become loadable.

**Six checkpoints** (per Plan D §D.1.4):

1. Schema migration `20260529_skill_proposals.sql`.
2. New `propose_skill` tool in `crates/xiaoguai-agent/src/toolbox.rs`.
3. HTTP endpoint `POST /v1/skills/proposals/:id/approve` (Casbin
   `skill.approve`).
4. Integration test in `crates/xiaoguai-tasks/tests/skill_author_e2e.rs`.
5. CLI: `xiaoguai skills proposals {list,approve}`.
6. `docs/runbooks/agent-authored-skills.md`.

**Safety model**: off by default; per-tenant config flag
`allow_skill_authoring=true`; HotL bucket `skill_author` capped at
5/day default; manifest validated against existing
tools-allow-list-only schema (no new MCP servers, no native code).

**R.E.S.T**: Security + Reliability dominant. Maps to the 育马
(Cultivating) dimension of §5 in `harness-engineering.md`.

### T4 — Outbound MCP OAuth 2.1 PKCE (Tier-3)

**Goal**: register a remote MCP server that requires OAuth 2.1 (with
PKCE) and have `xiaoguai-mcp::McpClient` carry the dance through:
authorisation request → user consent → token exchange → bearer-token
calls to the MCP HTTP transport.

**Six checkpoints**:

1. Extend `mcp_servers` schema: `auth: jsonb` carrying
   `{ kind: "oauth2_pkce", auth_url, token_url, client_id, scopes[] }`.
2. New module `crates/xiaoguai-mcp/src/auth/oauth2_pkce.rs`. Stores
   `(server_id, tenant_id, refresh_token, access_token, expires_at)` in
   a new `mcp_oauth_tokens` table.
3. CLI flow: `xiaoguai mcp register --auth oauth2-pkce …` prints a
   one-time consent URL; operator opens it; redirect lands at a small
   local listener (`xiaoguai mcp auth callback`) that completes the
   exchange.
4. `McpClient` interceptor adds `Authorization: Bearer <token>` on every
   call, refreshes when `expires_at < now + 60s`.
5. Integration test against an OAuth-mock fixture
   (`wiremock-rs` with PKCE state machine).
6. `docs/runbooks/outbound-mcp-oauth.md` (operator + threat model).

**Risks**: refresh-token rotation breakage (record server-provided new
refresh token); user typing the redirect URL into the wrong machine
(use localhost callback with random port); proxy / corporate-CA
breakage (already a known landmine — see ci-gotchas).

**R.E.S.T**: Security primary. Threat model — what if the OAuth
authorisation-server cert chain doesn't validate? Default fail-closed.

### T5 — Compliance export from audit chain (Tier-3)

**Goal**: a `xiaoguai audit export` CLI subcommand and matching
`POST /v1/audit/exports` HTTP endpoint that produces SOC2 / GDPR /
HIPAA report bundles from `audit_log` over a time window.

**Five checkpoints**:

1. New module `crates/xiaoguai-audit/src/export.rs`. Templates per
   framework (SOC2 CC7.2, GDPR Art. 30, HIPAA §164.312):
   each template is a list of `(audit.action, audit.actor, audit.resource,
   audit.ts)` projections + filters.
2. Output formats: JSON (programmatic), CSV (auditor-friendly), PDF
   stub (placeholder; PDF generation deferred to a separate step).
3. Chain-verify before export: re-run `verify_chain` over the window;
   refuse export if break detected. Embed verification proof in the
   export header.
4. CLI: `xiaoguai audit export --framework soc2 --from 2026-01-01
   --to 2026-05-31 --output /tmp/soc2.json`.
5. `docs/runbooks/compliance-export.md` — what each framework expects
   us to report, what gaps remain, sample auditor-question mapping.

**R.E.S.T**: Traceability primary. **Out of scope**: a polished PDF
template, regulator-specific signing, evidence collection from outside
the audit chain (e.g., HR provisioning records).

### T6 — `execute_javascript` MCP server (Hermes parity)

**Goal**: sibling crate to `xiaoguai-mcp-exec`, exposing
`execute_javascript` with the same env-scrub / ulimit / timeout
contract but using Node.js 22 (or deno, decided in the sub-plan) as
the runtime.

**Why**: parity with Nous Hermes Agent. Some tools (JSON
transformations, DOM parsing) are dramatically smaller in JS than in
Python; agent-authored snippets in TypeScript are a common idiom.

**Five checkpoints**:

1. New crate `crates/xiaoguai-mcp-exec-js/`.
2. Mirror `mcp-exec` structure: `exec.rs` / `tools.rs` / `server.rs` /
   `main.rs` / `lib.rs`.
3. **Decide**: Node vs Deno. Deno has `--allow-none` flag for sandboxing
   built-in; Node requires more wrapping. Sub-plan will recommend Deno.
4. Threat model: same six rows as the Python sandbox plus "npm
   transitive dep at install time" (Deno avoids by importing from URL).
5. 17 unit tests mirroring `mcp-exec`'s coverage.

**R.E.S.T**: Efficiency (snippet size) + Security (separate trust
boundary so Python and JS run-time exploits don't compose).

### T7 — wasmtime + pyodide L3 sandbox upgrade

Scope-capped: **feasibility study + ADR only this sprint**. Actual
implementation 1–2 weeks. Decide whether L3 is wasmtime+pyodide, or
Firecracker micro-VMs, or gVisor.

### T8 — Plan C Phase 2 reviewer pass

`cd xiaoguai-agent-design && claude`, run `/testany-eng:hld-reviewer`,
`/testany-eng:lld-reviewer`, `/testany-eng:test-reviewer` against the
doc updates we just made. Address any CRITICAL/HIGH findings;
log MEDIUM as TODOs. 30-minute polish, not a full restructure.

### T9 — macOS notarisation + Windows code-signing

Deferred. Needs paid Apple Developer ID ($99/yr) and EV cert
(~$300/yr). Track as a separate issue.

### T10 — CI workflow cleanup

The session-5 handoff flagged `k6` / `mdbook` / `perf-regression` as
"non-gating zombies". Either gate them properly or delete them. Two
hours, single-PR.

---

## 4. Execution order recommendation

Given Friday-afternoon energy + 5-min cache budget per turn, the
optimal serial order is:

1. **T8** (30 min) — fresh session has the reviewer skills loaded; cheap
   warm-up.
2. **T10** (1 h) — fast, mechanical, frees up CI noise before any new
   work lands.
3. **T2** *if* user has tap repo + PAT ready; otherwise skip to T3.
4. **T3** — biggest payoff (agent-authored skills is the Hermes parity
   killer feature).
5. **T5** — compliance export. Smaller surface than T4, no auth flow
   complexity.
6. **T4** — OAuth PKCE. Larger surface; do it after T5 lands so we have
   the audit-export view as a debugging aid.
7. **T6** — JS sandbox. Independent of T3-T5; can be parallelised with
   T4 if dispatching to a sub-agent in a fresh worktree.
8. **T1** — operator action, can happen any time once a stack is up.
9. **T7 / T9** — out of this sprint.

If we have to drop scope, drop in this order: T6 → T4 → T2 → T5 → T10
→ T8 → T3 → T1.

---

## 5. Per-sub-plan template

When a task is selected, its sub-plan file uses the same 8-section
template as session-6:

1. Context (why now, what changed since the parent plan)
2. Success criteria (measurable / verifiable yes-no)
3. Prerequisites
4. Step-by-step with `VC:` (verification checkpoint) per step
5. Risks & open questions
6. Rollback / abort criteria
7. Out of scope
8. References

Plus a `Self-review` appendix using the 6-point protocol from the
session-6 meta-plan.

---

## 6. Self-review (against the 6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | Cited file paths exist | **PASS** — session-5 handoff, all session-6 plans, design repo refs verified before drafting |
| 2 | Every step proposes a runnable verification | **PARTIAL** — table-level only; per-task `VC:` lines appear when each sub-plan is written |
| 3 | Each task has a success criterion in its sketch | **PASS** — every sketch has "Goal" + checkpoints |
| 4 | Out-of-scope is honored at the table level | **PASS** — T7 and T9 explicitly out-of-sprint |
| 5 | Risks have mitigations | **PARTIAL** — surface-level risks listed in sketches; per-task sub-plans will deepen |
| 6 | Time estimates are sane | **PASS** — sum ≤ 36 h; realistic 1-week sprint at 2 focused sessions |

**Soft spot**: this file is a *table-of-contents*, not a deep plan. Each
selected task still gets its own per-task plan written before code
work begins (matching the session-6 protocol that the user explicitly
endorsed).

---

## 7. References

- Session-5 handoff:
  `docs/HANDOFF-2026-05-28-session5.md` — "what didn't get done" table
  is the primary input.
- Session-6 plans:
  `docs/plans/2026-05-28-{agent-mcp-exec-e2e,release-packaging,retro-design-docs,tier2-next}.md`.
- Roadmap memory:
  `~/.claude/projects/-Users-zw-testany-myskills-xiaoguai/memory/agent-roadmap.md`.
- Philosophy doc (R.E.S.T mapping):
  `xiaoguai-agent-design/docs/harness-engineering.md`.
- Design-doc release log:
  `xiaoguai-agent-design/docs/RELEASE-LOG.md`.
