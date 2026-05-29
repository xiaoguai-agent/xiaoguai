# Sprint-10b — Frontend gap-fill (DEC-025)

> Companion to `xiaoguai-agent-design#6` (DEC-025 + `lld-admin-ui.md` LLD-ADMIN-UI-001 + `lld-chat-ui.md` LLD-CHAT-UI-001 + PRD §4.14 REQ-UI-001..008 + test-spec §3.14).
> Per workflow rule (`memory/sprint-workflow.md`): **this is Step 2 (任务安排); Step 3 (审核) gates Step 4 (执行)**.
>
> Parallel to sprint-10 (`docs/plans/2026-05-29-sprint-10-slo.md`). Both ship together as v1.8.0.

---

## 1. Context

The `frontend/` monorepo (pnpm workspace) has been growing organically since v1.3: chat-ui Gemini-style welcome, admin-ui Kanban / anomaly / HotL editor / memory browser, watch indicator, AI disclosure banner. **Architecture documentation caught up in Step 1 (PR #6)** — DEC-025 codifies the dual-SPA split, the two LLDs name every existing pane.

Sprint-10b is the **gap-fill sprint**: close the holes the LLDs identified between "what's already in main" and "what DEC-025 + LLD-ADMIN-UI-001 + LLD-CHAT-UI-001 require". The gaps fall into three buckets:

1. **Missing UI panes** that the LLDs require but the code doesn't have yet: Personas, Skill Proposals.
2. **Missing backend wiring** the UI assumes: `/v1/personas` not mounted on main router; `/v1/watchers` returns 404; Memory subsystem (task #155) end-to-end; `/v1/admin/me/scopes` for the `<RequireScope>` gate.
3. **Quality bars** that ship today but aren't enforced: i18n missing-key lint, Axe-core in e2e, multi-browser matrix coverage assertion in CI, RBAC UX hint integration.

R.E.S.T axis: primarily **Reliability** (Personas + Skill Proposals turn opaque CLI workflows into observable UI flows) and **Security** (RBAC scope hints + delegated auth boundary made explicit per DEC-025 §3).

Total: **~8.5 dev-days** (under the 10-dev-day sprint cap).

---

## 2. Sprint-10b backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T axis |
|:-:|---|---|---|---:|---|
| P0 | **S10b-1** | Mount `/v1/personas` routes on main router. `xiaoguai-personas/src/routes.rs:52-64` defines CRUD but is unreferenced in `xiaoguai-api/src/routes/mod.rs`. Add the `Router::merge`; add Casbin scope `personas.{read,write}`; smoke test against `crates/xiaoguai-api/tests/`. | merge of design PR #6 | 0.5 day | T |
| P0 | **S10b-2** | `frontend/admin-ui/src/panes/Personas.tsx` (new) + `XiaoguaiClient` Personas methods in `frontend/shared/src/index.ts`. Table view (list + filter by tag) + edit drawer (name, system_prompt, model_preference, memory_view_scope, role tags). Implements REQ-UI-007. | S10b-1 | 1 day | T |
| P0 | **S10b-3** | `frontend/admin-ui/src/panes/SkillProposals.tsx` (new) + `XiaoguaiClient` proposal-list/approve/reject methods. Card list with `<SkillManifestPreview>` (lives in `@xiaoguai/shared`, reused by chat-ui's `/skills` page). Wrapped in `<RequireScope name="skill.approve">`. Implements REQ-UI-008. | S10b-6 (RequireScope) | 1 day | S + T |
| P0 | **S10b-4** | Memory subsystem backend completion (task #155 follow-up): mount `/v1/memories/*` routes on main router; `XiaoguaiClient::listMemories/createMemory/recallMemory` already exist as 404-safe — remove the 404 fallback in `frontend/admin-ui/src/panes/Memory.tsx` once routes mount; CRUD integration tests. | none | 1 day | R |
| P1 | **S10b-5** | `/v1/watchers` backend endpoints — list/pause/resume per session. `frontend/shared` already calls `listSessionWatchers/pause/resume` with 404 fallback. Mount in `xiaoguai-api`; implementation reads from `xiaoguai-watch::WatchRunner` state. Removes the 404 degrade in `chat-ui::WatchIndicator`. | none | 1 day | R |
| P1 | **S10b-6** | `<RequireScope name="…">` component + `GET /v1/admin/me/scopes` endpoint (returns Casbin-resolved scope list for the bearer subject). Fail-open behaviour per DEC-LLD-ADMIN-UI-002 when endpoint absent. Used by S10b-3 + future panes. Implements REQ-UI-006-adjacent UX hint. | none | 1 day | S |
| P1 | **S10b-7** | i18n + a11y enforcement: ESLint rule forbidding untranslated JSX literals (`@xiaoguai/admin-ui` + `chat-ui` configs); fill `zh-Hans` + `en` bundles for newly-added panes (Personas, SkillProposals); Axe-core integrated into Playwright golden-path specs; CI fails on serious/critical violations. Implements REQ-UI-005 + REQ-NFR-014. | S10b-2, S10b-3 | 1 day | T |
| P1 | **S10b-8** | E2E coverage extension under `frontend/e2e/tests/`: `admin-personas.spec.ts` + `admin-skill-proposals.spec.ts` + `admin-audit-export.spec.ts` (Audit HMAC chain badge → export → ChainProof download per LLD-ADMIN-UI-001 §4.2) + `chat-hotl-suspend-resume.spec.ts` + `chat-sse-reconnect.spec.ts` + `chat-ai-disclosure-mandatory.spec.ts` per LLD-CHAT-UI-001 §7. Multi-browser matrix (Chromium / Firefox / WebKit) per REQ-NFR-012. | S10b-2, S10b-3 | 1.5 days | T |
| P2 | **S10b-9** | Auth UI placeholder: 401 from any `XiaoguaiClient` call → redirect to `VITE_LOGIN_URL` env value (no in-SPA login form, per DEC-025 §3); 403 with scope hint → `<ForbiddenPane scope="…">` copy linking to operator runbook. Implements REQ-UI-006. | S10b-6 | 0.5 day | S |

**Sprint-10b total: ~8.5 dev-days.**

---

## 3. Sub-agent dispatch plan

| Phase | Sub-agents | Range |
|---|---|---|
| Phase A | (none — I drive) | S10b-1 (mount Personas routes) + S10b-4 (mount Memory routes) + S10b-5 (Watchers backend) — three small backend mounts, ~2.5 days serial. These are blockers for frontend work and share the same `xiaoguai-api/src/routes/mod.rs` file. |
| Phase B | **2 parallel sub-agents** | (A) S10b-6 RequireScope component + `/me/scopes` endpoint. (B) S10b-2 Personas pane. Disjoint files; (A) unblocks S10b-3. |
| Phase C | **1 sub-agent** | S10b-3 Skill Proposals pane — needs `<RequireScope>` from B; isolated to admin-ui. |
| Phase D | (I drive in parallel with B+C) | S10b-7 i18n + a11y plumbing — touches lint config + i18n bundles; coordinates with B/C output (newly-added panes need translations). |
| Phase E | **1 sub-agent** | S10b-8 E2E extension — 6 new spec files; happens after all panes are in place. |
| Phase F | (I drive) | S10b-9 Auth UI placeholder — small, single component. |

Disk budget: peak 3 worktrees × ~30 GB = ~90 GB. Within budget.

---

## 4. Cross-sprint risks (sprint-10b-specific)

| Risk | Mitigation |
|---|---|
| Personas routes wired on main router conflict with the `xiaoguai-personas` crate's `routes.rs` route paths (drift since written) | Run `cargo check -p xiaoguai-api` after the merge; integration test in S10b-1 hits every route once. |
| `<RequireScope>` fails open in production when `/me/scopes` is misconfigured — operators see actions they can't use; clicks land 403 | Document the fail-open behaviour in the operator runbook (S10b-7 deliverable); add a one-time WARN log in browser console on absence so SRE notices in audit. **Backend Casbin still enforces** — this is UX only, per LLD-ADMIN-UI-001 §4.8. |
| Memory backend (task #155) has pending PR; merging into main mid-sprint disrupts S10b-4 | If the upstream PR isn't merged by S10b-4 start, my work in S10b-4 becomes "rebase / verify already-merged"; if conflict, raise to user — do not silently fix #155's PR. |
| E2E flake on Firefox/WebKit for SSE reconnect scenarios | Playwright `trace: 'retain-on-failure'` + screenshots/video on failure; if a browser is consistently flaky on one spec, mark `test.fixme()` with a follow-up tracked separately rather than weakening assertions. |
| Watchers backend (S10b-5) needs `xiaoguai-watch::WatchRunner` to expose state — may force a trait change | Pre-flight before sprint start: `cargo expand` the `WatchRunner` impl to confirm. If a trait change is needed, S10b-5 becomes 1.5 days; flag at Phase A handoff. |
| Sub-agent introduces React 19 / Vite 6 upgrades while implementing a pane | Lock package.json versions before sub-agent dispatch; ESLint rule forbidding new top-level deps without explicit approval; review pnpm-lock.yaml diff carefully on PR. |

---

## 5. Out of scope (sprint-10b)

- **Full login page or in-SPA OIDC integration.** DEC-025 §3 explicitly delegates auth to the surrounding reverse proxy. S10b-9 only handles the 401/403 UX.
- **Anomaly UI live data wiring.** The pane exists with mock fallback (`frontend/admin-ui/src/panes/Anomaly.tsx`); backing data path is `xiaoguai-anomaly` crate which has its own roadmap slot. Reserved for sprint-11+.
- **Mobile responsive admin-ui.** PRD non-goal (admin-ui is desktop-first; chat-ui is responsive).
- **Tenant switcher refactor.** Today the URL `?tenant=X` works; rebuilding it as a context-aware switcher is future polish.
- **Real-time co-editing of HotL Policies.** Single-operator-at-a-time is enough; concurrent-edit 409 toast handled in LLD §4.10.
- **Skill manifest authoring UI.** Operators write `skill.yaml` by hand for now (CLI `xiaoguai skills install path/to/manifest`); a manifest editor is sprint-11+.
- **Service-worker offline cache.** DEC-025 §3 explicit non-goal — air-gapped deploys don't need it.

---

## 6. Workflow checkpoint

```
1. 更新架构文档    ✅ xiaoguai-agent-design#6 (merged)
2. 安排任务        ✅ THIS PR (sprint-10b) + companion sprint-10 plan
3. 审核           ← awaiting your sign-off
4. 执行           ← only after step 3
5-7. Merge/push/release ← after BOTH sprint-10 + sprint-10b ship → v1.8.0
```

---

## 7. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | Cited file paths exist | **PASS** — `frontend/admin-ui/src/App.tsx:24-104` (router), `frontend/chat-ui/src/App.tsx:16-41`, `frontend/shared/src/index.ts:908-1613` (XiaoguaiClient), `xiaoguai-personas/src/routes.rs:52-64` (unmounted routes), `xiaoguai-api/src/routes/mod.rs` (mount point), `xiaoguai-watch/src/runner.rs:39,92,155` (executor dispatch) — all verified during sprint-10b Step 1 audit. |
| 2 | Every task proposes runnable verification | **PASS** — S10b-1: `curl /v1/personas` 200 after mount. S10b-2/3: Vitest + Playwright. S10b-4: `curl /v1/memories` 200. S10b-5: `curl /v1/watchers` 200. S10b-6: scope endpoint returns Casbin-resolved list. S10b-7: `pnpm lint` zero violations + Axe-core green. S10b-8: `pnpm test:e2e` green on 3 browsers. S10b-9: 401 mock → redirect URL changes. |
| 3 | Each task has a measurable outcome | **PASS** — est. + dep + R.E.S.T axis per row. |
| 4 | Out-of-scope is honored | **PASS** — §5 lists 7 explicit non-goals. |
| 5 | Risks have mitigations | **PASS** — §4 has 6 concrete risk/mitigation pairs. |
| 6 | Time estimate sane | **PASS** — 0.5+1+1+1+1+1+1+1.5+0.5 = 8.5 dev-days; fits two weeks with parallel sub-agent dispatch (Phase B + D concurrent, Phase E after panes ship). |

**Soft spots flagged for reviewer**:

1. **Phase B parallelism (RequireScope + Personas pane)** — sub-agent A's `<RequireScope>` ships in `@xiaoguai/admin-ui/src/components/` while sub-agent B's Personas pane wants `<RequireScope name="personas.write">` on the Edit button. If A lands late, B has to stub the component. Mitigation: A is ~0.5 day backend + 0.5 day component, simpler than B; A ships first. If timing slips, B stubs with a no-op wrapper and S10b-3 picks up the real one.
2. **S10b-5 Watchers backend may need a trait extension** — `WatchRunner` currently doesn't expose introspection. If extending the trait forces ripple changes across `WatchSource` impls, sprint-10b stretches to ~10 dev-days. Flagged at Phase A; user can decide to drop S10b-5 or stretch.
3. **S10b-8 e2e on 3 browsers × 6 new specs = 18 test invocations per CI run** — runtime budget question. Playwright `--shard` if CI runtime exceeds 15 min; otherwise accept.

---

## 8. Asks for the reviewer (you)

Before I drive Phase A:

1. **Memory backend (task #155) status** — is the upstream PR ready to merge before S10b-4 starts? If not, do I (a) wait for #155, (b) merge it as part of S10b-4, or (c) defer Memory mount to sprint-11?
2. **`<RequireScope>` failure mode** — fail open (action visible; backend rejects on click) per DEC-LLD-ADMIN-UI-002, or fail closed (action hidden when scope endpoint unavailable)? The LLD chose fail open; confirm — fail closed is stricter but breaks older backend compatibility.
3. **Watchers backend trait extension** — if `WatchRunner` needs trait change, scope grows by 0.5 day. Approve in advance, or do you want me to pause and report?
4. **Auth UI: `VITE_LOGIN_URL` env requirement** — if it's empty, do nothing on 401 (current behaviour) or show a "session expired; reload" toast? Recommendation: toast + reload button; never silently hang.
5. **Sprint-10 + sprint-10b release window** — Both target v1.8.0. If sprint-10b stretches because of trait change or #155 timing, do we (a) hold the release for both, (b) ship sprint-10 alone as v1.8.0 and sprint-10b as v1.8.1, or (c) feature-flag sprint-10b panes and ship together? Recommendation: (a) if slip ≤ 2 days; (b) if slip > 2 days.
