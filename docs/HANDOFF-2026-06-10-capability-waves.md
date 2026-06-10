# HANDOFF — capability plan executed: T3 + T1 + T4–T8 (2026-06-10)

> Durable checkpoint following `HANDOFF-2026-06-09-relicense-cleanup-capability.md`.
> One day, two sessions' worth of work: **the entire capability-upgrade plan
> shipped except T2 (browser)**. v1.14.0 released mid-day (T3); T4–T8 are on
> main awaiting the next tag.
>
> **main tip at handoff: post-#279/#276. 0 open PRs from this pipeline.
> License Apache-2.0. Workspace: 2246 Rust tests; frontend 40/102/287.**

## 1. Shipped (chronological, every PR: plan doc → TDD → review → merge)

- **#270/#271 T3 expert center** + **v1.14.0 release** (tag on `5956f86`;
  tarball/deb+rpm/PyPI all green after one quay.io flake rerun). Includes the
  **ChatPage SSE state-wipe race fix** (latent since sprint-11, webkit-manifest)
  and #272 (e2e artifacts untracked).
- **#273 T1 office runbooks**: excel-mcp-server (active, MIT) + GongRzhe
  Word/PPT MCP (**archived 2026-03 — pinned + flagged; no offline alternative
  exists**). Fixed nonexistent `xiaoguai audit list` in the office runbook.
- **#274 T4 executive orchestration**: `patterns/executive.rs` (LLM-free
  MemberRunner/ExecutiveRunner, parallel members + lead synthesis) +
  `POST /v1/sessions/{id}/orchestrate` (SSE, session turn lock, HotL
  `llm_call` members+1, attribution `orch:<run_id>:<persona_id>`, audit
  `orchestration.*`, disconnect-safe) + `orchestrateSession` client.
  Deliberately NOT CapabilityRouter (AND-match ≠ fuzzy goals). Found+fixed:
  persona `filter_tools` had never been applied to any run path.
- **#275 T5 consult/execute + Agent Bridge**: `ToolDescriptor.mutation_hint`
  (serde-default **Write** = fail-closed), MCP import bridge honours rmcp
  `readOnlyHint` (was dropped; deduped stdio/http conversion), `ConsultGate`,
  `SendMessageRequest.mode`, audit mode stamp, chat-ui 执行/咨询 toggle +
  **T4's team-run UI** (deferred there by design).
- **#277 T6 self-healing**: migration 0033 (incidents/rcas/repairs +
  live-dedup index); ingest sentry|datadog|manual (public, token-gated like
  scheduler webhooks); **Analyst = consult-locked turn** (`incident:<id>`
  attribution); explicit human `approve-repair` → Executor execute-mode turn;
  markdown report endpoint. **No auto-repair in v1.** NB: the RcaDraft
  contract is **JSON via serde** (renderer owns markdown) — plan said
  markdown, implementation matched reality.
- **#278 T7 memory**: migration 0034 `expert_teams.glossary_md` (16 KiB;
  injected after USER.md identity in every turn + orchestrate member runs —
  three-tier context identity→glossary→persona); JSONL import/export
  (routes + `xiaoguai memory` CLI; codec in `xiaoguai-memory::jsonl`);
  `source:` tag convention. **CompositeMemoryView unification deferred**
  (orchestrator memory bridge unbuilt; context-bloat risk).
- **#279 T8 install polish**: serve URL banner, init next-steps, AddrInUse
  3-remedy message, `xiaoguai doctor`, `xiaoguai service
  install|uninstall|status` (systemd + new launchd template, `--print-only`),
  empty-providers stderr banner, `install-and-verify.md`. **Broke Docker
  builds via `include_str!(deploy/...)` → fixed with Dockerfile
  `COPY deploy/{systemd,launchd}`** (same mode as the v1.8.1 catalog COPY;
  e2e/k6 caught it — local tests can't).
- **#276** dependabot regex patch.

## 2. Live smoke (2026-06-10, clean-box, mock backend, fresh SQLite)

serve banner ✓ → healthz ✓ → persona/team(+glossary) ✓ → suggest ranks team ✓
→ orchestrate SSE (member→synthesis→final) ✓ → memory import (fail-soft
skipped[] + auto `source:imported`) / export ✓ → incident token mint + manual
ingest ✓ → analyze with mock backend correctly fails RCA parse and **reverts
to open (designed retryable path)** ✓ → doctor (hard-✗ on Ollama down,
"already serving" port note) ✓ → **audit chain verify ok (7 entries)** ✓.

## 3. Known follow-ups (none blocking)

1. **Memory pane stale contract**: admin-ui Memory pane body still targets the
   never-shipped ADR-0019 `/v1/memory/*` shape → mock mode against a real
   backend. Only the T7 import/export toolbar uses real routes. Straight fix.
2. **Manual incident ingest UX**: requires the full `Incident` shape including
   `raw` and non-null strings — fine for scripts that know, hostile to humans.
   Consider defaulting `raw`/nullables for `source=manual`.
3. Playwright specs for expert-center/team-run/incidents (live-stack method,
   #260); `InMemoryPersonaRepository::update` dup-name gap (mirror of the team
   fix); HotlAuditSink rename to a neutral append trait; cross-repo attach
   transactionality (T4 follow-up); incident auto-repair HotL policy (T6 §5.1).
4. **T2 browser** — the only unexecuted capability task; blocked on the owner's
   4 chromium-distribution decisions (`2026-06-08-browser-automation-distribution.md`).

## 4. ⚠️ Parallel-session note (2026-06-10)

The owner ran a **security-audit session in the same repo directory**
concurrently: `docs/SECURITY-AUDIT-2026-06-10.md` (untracked report, 6
read-only audit agents) + branch `fix/security-audit-2026-06-10` + uncommitted
fixes (multiple crates + new `im-gateway/src/dedup.rs`). A branch-switch race
briefly put a T8 commit on their branch (recovered: cherry-pick back; their
branch repointed to clean main). **Lesson: concurrent sessions must use
separate `git worktree`s.** The audit report/fixes are the owner's to land.

## 5. Housekeeping

`cargo clean` freed **191.8 GiB** of stale target/; pact consumer
node_modules (404 MB) removed; smoke ran in a disposable `/tmp` worktree
(removed after use). New memory: `feedback-cleanup-temp-artifacts`;
ci-gotchas gained the sqlx stale-migration-embed note (bitten 2×).

## 6. On resume — read first

1. This file; `docs/plans/2026-06-09-capability-upgrade.md` (T2 = the
   remainder) + the six 2026-06-10 plan docs (expert-center, executive-
   orchestration, consult-execute, self-healing, memory-multisource,
   install-polish).
2. Memories: feature-backlog (the wave summary + follow-ups), ci-gotchas,
   feedback-cleanup-temp-artifacts.
3. Verify: `gh pr list` (only owner's security PR if any), workspace tests
   green, and whether the security-audit session's fixes have landed.
4. Release: v1.14.0 is live; **T4–T8 + fixes are unreleased on main** —
   tag v1.15.0 after the owner lands (or defers) the security fixes, so the
   security patches can ride the same release if ready.
