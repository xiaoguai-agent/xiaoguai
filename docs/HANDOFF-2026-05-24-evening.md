# Session handoff — 2026-05-24 evening

**v1.1 + B+C bands shipped.** Earlier today's session closed v1 feature-complete
at v0.12.2 (13 tags). This evening's session ran the B+C "shipping
readiness + v1.1 backlog" wave that the user explicitly asked for —
12 more tags merged + pushed.

## 0. Session close-out — start here on resume

**Resume prompt:**

```
继续。先 cd /Users/zw/testany/myskills/xiaoguai 看 git log -30，
然后读 docs/HANDOFF-2026-05-24-evening.md。
打开方向（不动手就停在这）：
1. C1 公有云 LLM provider 5 选 (Anthropic / Gemini / 通义 / DeepSeek / 智谱)
2. 浏览器实跑 chat-ui + admin-ui 截图（用户操作）
3. v0.12.x.1 后续：DingTalk/WeCom encrypted-payload + WeCom AES + 交互卡片
4. 把 .github/workflows/release-tarball.yml + pip-wheel.yml + snyk.yml 在真 CI 上跑一遍并修问题
5. 等保 2.0 三级 — 联系 MPS 认可评估机构（用户操作）
```

**State summary as of close:**

- Latest tag: `v0.12.x.1` (`ab4c4ee`) — webhook tokens + CompositeExecutor + Scheduler admin pane.
- Latest commit on main: `e23b307` — integration fix (4 cross-AppState fields in tests/fork.rs + tests/usage.rs).
- Tests: **530 passed / 0 failed / 67 ignored** across the workspace.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --check` clean.
- Frontend `pnpm -r typecheck` clean.
- No uncommitted changes; in sync with `origin/main` + 12 new tags pushed.

**What THIS session shipped (12 tags + 1 integration fix, in commit order):**

| Tag             | Hash       | Headline |
|---              |---         |---|
| v1.0.1-readme   | `7b7eabe`  | top-level README + 8 screenshot + 3 asciinema placeholders |
| v1.0.1-runbook  | `93b2c4c`  | operator runbook scheduler + IM chapters (+2619 words) |
| v1.0.1-smoke    | `db4763a`  | docker-compose smoke + e2e + real-LLM scripts (+ port :8080 → :7600 fix) |
| v1.1.4          | `0da897c`  | HA scaffolding — PG logical repl + Valkey cluster + runbook |
| v1.1.6          | `44f58c4`  | bare-metal tarball + hardened systemd unit + install scripts |
| v1.1.7          | `7caea09`  | pip wheel for xiaoguai CLI (cibuildwheel matrix) |
| v1.1.8          | `3125e52`  | CI security — cargo-deny gating + cargo-audit cron + Snyk |
| v1.1.5a         | `fdb67ef`  | multi-agent peer MVP — example + arch doc + integration test |
| v1.1.5b-doc     | `b4ad6d0`  | supervisor plan doc (orchestrator future) |
| v1.1.3          | `6d86d6c`  | DingTalk + WeCom IM adapters (+37 tests, full impl) |
| v1.1.2          | `58f6230`  | conversation fork — `POST /v1/sessions/:id/fork` + chat-ui Branch button |
| v1.1.1          | `52f5c1b`  | `/v1/usage` + Usage admin pane + Today summary card |
| v0.12.x.1       | `ab4c4ee`  | per-tenant webhook tokens + CompositeExecutor + Scheduler admin pane |
| (no tag)        | `e23b307`  | integration fix: 4 cross-AppState fields in 2 new test files |

**Cumulative session totals (today's two sessions combined): 26 tags.**

## 1. B+C band coverage

The user asked for B+C. Final allocation:

### B band — shipping-readiness (5 items)

- ✅ **B2** smoke scripts — bonus fix on port mismatch (`:8080` → `:7600`)
- ✅ **B3** README + 11 placeholders user owns to capture
- ✅ **B4** operator runbook scheduler/IM chapter
- ✅ **B5** v0.12.x.1 webhook tokens + CompositeExecutor + Scheduler pane
- ⏳ **B1** chat-ui + admin-ui browser pass — **USER ACTION** (can't be agent-done)

### C band — v1.1 backlog (10 items)

- ⏳ **C1** public-cloud LLM providers — **deferred by user**: "本地模型优先，云LLM暂时空着"
- ✅ **C2** `/v1/usage` endpoint + Usage admin pane + Today 24h summary card
- ✅ **C3** conversation fork (migration 0008_session_parent + Branch button)
- ✅ **C4** DingTalk + WeCom IM adapters (full impl, +37 tests)
- ✅ **C5** HA scaffold (PG logical repl + Valkey 6-node cluster + nginx + runbook)
- ✅ **C6** multi-agent peer MVP (v1.1.5a) + supervisor plan doc (v1.1.5b-doc)
- ✅ **C7** bare-metal tarball + heavily-hardened systemd unit
- ✅ **C8** pip wheel scaffolding (cibuildwheel matrix)
- ✅ **C9** CI security — gating + 4 ignored advisories (3 dev-only, 1 unused backend)
- ⏳ **C10** 等保 2.0 三级 measurement — **USER ACTION** (external MPS-accredited assessor)

## 2. Sharp edges still live

Nothing blocks v1.1 shipping. Honest deferrals, all in plan docs:

### From v0.12.x.1
- `JobRepository::list_all` — admin-ui Jobs table reuses `list_due` with `now + 10 years` so disabled rows are invisible. Cheap follow-up when a user files a disabled-job report.
- Webhook routes added `ApiError::Unauthorized` (new variant) — verify the `WWW-Authenticate` header semantics match expectations.

### From v1.1.1 (usage)
- Cost rates: `llm_providers` has no `cost_per_1k_*` columns. UI shows "cost: —". Add columns + reseed catalogs in v1.1.1.1.
- No Recharts bar chart yet on Usage pane (placeholder text).
- PG testcontainer test marked `#[ignore]`.

### From v1.1.2 (fork)
- "Branch from here" button doesn't carry over tool turns mid-branch (only message-id slice). The slice semantics match what most users want; advanced "branch and rewrite this turn" is a v1.2 surface.

### From v1.1.3 (DingTalk + WeCom)
- WeCom `EncodingAESKey` (encrypted payload) — rejected with directive today. Plain-text-only deployments work.
- DingTalk Stream API long-poll client.
- Group `@mention` parsing for WeCom (different inbound API surface).
- Interactive cards (DingTalk `actionCard` + WeCom rich types).

### From v1.1.4 (HA)
- Replica-aware read pool routing (v1.1.4.1) — today reads still go to primary even with the HA scaffold up.
- Valkey cluster client migration (v1.1.4.x) — `redis` crate uses single-node connection today; need `redis::cluster::ClusterClient` swap.
- Automatic failover handler in xiaoguai-core — today nginx reroutes; in-process retry on PG primary loss is v1.1.4.2.

### From v1.1.5 (multi-agent)
- Supervisor pattern (v1.1.5b) — plan doc only. Peer MVP ships and is tested in-process.
- v1.1.5a Peer example is documented but uses real `xiaoguai-core` deployment (PG/Valkey/LLM); user runs locally to actually try.

### From v1.1.6 (tarball)
- deb/rpm packages (would need fpm/cargo-deb).
- Windows MSI / macOS pkg.
- `Type=notify` systemd integration (needs `sd-notify` crate in core).
- Tarball cosign signing / SLSA attestations.
- `CAP_NET_BIND_SERVICE` drop-in template for port :80/:443.

### From v1.1.7 (pip)
- PyPI publishing automation (trusted-publisher OIDC).
- conda-forge package.
- macOS code-signing of the bundled binary (Gatekeeper risk).
- PyO3 native bindings → v1.2.

### From v1.1.8 (CI security)
- cargo-vet bootstrap → v1.1.8.1.
- Dependabot → v1.1.8.2.
- CodeQL — only when going public.

### From earlier (still standing)
- chat-ui + admin-ui browser verification (every UI tag since v0.8.1).
- `notify-debouncer-full` for file-watch.

## 3. Where things are

| What                 | Path                                                              |
|----------------------|-------------------------------------------------------------------|
| **Repo (work here)** | `/Users/zw/testany/myskills/xiaoguai`                             |
| Design workspace     | `/Users/zw/testany/myskills/xiaoguai-agent-design` (docs only)    |
| Remote               | `https://github.com/xiaoguai-agent/xiaoguai`                      |
| Latest tag           | `v0.12.x.1` (`ab4c4ee`)                                           |
| Latest commit        | `e23b307` — integration fix                                       |
| Active roadmap       | `docs/plans/2026-05-23-roadmap-v0.9-v0.12.md` — **v1 + v1.1 done** |

## 4. v1.1 additions to crate layout

- `xiaoguai-im-dingtalk` — was scaffold; v1.1.3 full impl (signature verify, OpenAPI reply, token cache).
- `xiaoguai-im-wecom` — was scaffold; v1.1.3 full impl (signature verify, XML parser, OpenAPI reply, token cache).
- `xiaoguai-scheduler::composite_executor` — v0.12.x.1 module; payload-dispatching executor.
- `xiaoguai-scheduler::sources::webhook` — unchanged shape but now fronted by public `/v1/scheduler/webhooks/:route_id` with token middleware.
- `xiaoguai-api::scheduler::{WebhookTokenValidator, WebhookTokenAdmin, ScheduledJobsReader}` — three new traits.
- `xiaoguai-api::usage` — `UsageReader` trait + `GET /v1/usage` route.
- `xiaoguai-api::sessions_ext::SessionForker` — fork trait + `POST /v1/sessions/:id/fork` route.
- `xiaoguai-core::usage_bridge`, `sessions_bridge` — production PG impls of the v1.1 traits.
- `xiaoguai-core::scheduler_bridge::{LlmNlJobCompiler, PgScheduledJobUpserter, PgScheduledSessionWriter, RagReindexExecutor, spawn_file_watch_source, PgSchedulerAuditAppender, WebhookSourceAdapter, PgWebhookTokenValidator, PgWebhookTokenAdmin, PgScheduledJobsReader}` — central bridge hub (~10 adapters).

PG migrations through `0009_scheduler_webhook_tokens.sql`.

## 5. Conventions that worked this session

- **3-way parallel sub-agents in worktrees** — for trivial wave 1 (9 agents touching different subdirs), zero conflict. For wave 2 (3 agents touching shared state.rs / routes/mod.rs / test fixtures), conflicts resolve in <5 min with a "keep both sides" python script because each agent only ADDS lines.
- **Pre-assign each parallel agent**: unique `AppState` field name + unique admin-ui sidebar slot + agreed migration number. Avoids the conflicts that bit v0.12.1 + v0.12.2.
- **Migration number conflict resolution**: rename the second-merged file. Wave 2 hit this (B5 + C3 both picked 0008). C3 first in keeps 0008; B5 renamed to 0009.
- **New test files miss cross-agent fields**: every wave-2 agent added new `AppState { ... }` constructions in new test files. After integration, run `cargo test --workspace` once; the missing-field errors point exactly to which test file needs the other agents' default-None lines.

## 6. Likely next directions (none queued)

- **C1 public-cloud LLM providers** — user explicitly deferred. Pick this up when first user asks for a non-Ollama/non-DeepSeek-compat backend.
- **CI dry-run** — every new workflow (release-tarball.yml, pip-wheel.yml, snyk.yml, audit.yml) needs a real CI run to surface integration issues.
- **chat-ui / admin-ui browser verification** — punch list from v0.8.1+ tags awaits user.
- **WeCom `EncodingAESKey`** — common Chinese enterprise deployment ask.
- **Cost columns on llm_providers** — would unblock the Usage pane's "—" cells.
- **v1.2 territory** — supervisor (xiaoguai-orchestrator crate), PyO3 native bindings, real cargo-vet supply chain attestation, multi-region deploy.

The roadmap doc was explicitly silent past v1.0. v1.1 grew from B+C. v1.2 is "wait for first user, then prioritise."
