# Skill Pack Runtime Loader — Phase 2 Technical Design (watch / anomaly drivers)

**Status:** DESIGN — review checkpoint (2026-06-23). No Phase-2 code is written yet.

**Context.** The owner ratified the four §3 decisions of
[`2026-06-21-skill-pack-loader.md`](2026-06-21-skill-pack-loader.md) on 2026-06-23
("批准 §3 → 写 Phase 2 设计"). That doc captured the verified state and a phasing
plan; **Phase 1 (`xiaoguai pack validate`) has since shipped** (#336/#338, plus the
soft-adapter coverage in #344) — the read-only validator now covers the full
manifest over a 44/44-clean corpus. This document **resolves** the §3 decisions and
specifies **Phase 2 — making registered watches/anomalies actually run** — before any
code, so execution can proceed in reviewable slices.

> Methodology note (per the project's "verify before citing in design docs" rule):
> every code fact below was grep-checked on 2026-06-23. Three claims surfaced by the
> scouting pass were **over-stated** and are corrected in §B. Recommendations are
> labelled **REC**; verified facts are labelled **V**.

---

## A. Ratified §3 decisions (owner-approved 2026-06-23)

| # | Decision | Ratified resolution |
|---|----------|---------------------|
| 1 | **Activation entry point** | A new `xiaoguai pack install <dir>` CLI + an *enabled-packs* record in the embedded SQLite. Marketplace `/v1/skills/install` stays **listing-only**. (Least coupling, clearest lifecycle.) |
| 2 | **Per-pack SQL migrations into the one SQLite** | **Not in Phase 2.** Phase 2 packs are *schema-free* — they bind to existing tables only. Per-pack migrations stay deferred (a later phase, only if needed, and only ledgered + idempotent + reversible). Keeps Phase 2 off the DEC-033 third rail. |
| 3 | **Watch / anomaly live runtime** | **Yes — build the live driver.** This is the value of Phase 2 and the bulk of this doc. |
| 4 | **Agent execution** (pack `agents[]`) | **Out of scope** for loader v1. `xiaoguai-orchestrator`'s `AgentRegistry` is test-only; wiring it into `serve` is a separate effort. `agents[]` keep validating (Phase 1) but do not execute. |

---

## B. Verified runtime surface (grep-checked 2026-06-23)

### B1. `xiaoguai-watch` — **V**
- Public API (`crates/xiaoguai-watch/src/lib.rs:69-77`): `WatchSpec`, `WatchSourceSpec` (`Sql { query }` | `Http { url, jsonpath, method }`), `WatchSchedule`, `ActionRef`, `WatchRunner`, `WatchEvent`, `DedupCache`, trait `WatchSource { async fn poll() -> Vec<Match> }`, and `SqlSource`/`HttpSource`/`InMemorySource`.
- Driving model (`runner.rs`): `WatchRunner::run()` **self-drives** — spawns one tokio task per spec on a `tokio::time::interval`, emits `WatchEvent`s to an `mpsc::Receiver`. It is *not* a tick-on-demand API.
- **Cron is NOT implemented** (`runner.rs:273-276`): `WatchSchedule::Cron { expr }` falls back to a 60-second interval and logs a warning; only `IntervalSecs` is honored (`spec.rs:78-87`). ⚠️ Both shipped pack examples use cron (`packs/ar-collections/watches/dso-over-60.yaml` `0 */15 * * * *`; `packs/lease-management/watches/renewal-window-approaching.yaml` `0 7 * * *`) — so on the watch crate's own timer they would silently run every 60 s. **This is the load-bearing constraint that shapes §C1.**
- No `WatchRegistry` in the crate; the only one is the no-op stub in `packs.rs` (§B4).

### B2. `xiaoguai-anomaly` — **V**
- `AnomalyRegistry::register(spec)` + `observe(id, ts, value) -> Option<(&AnomalySpec, Anomaly)>` (`registry.rs:108,127`). `observe` is stateful (Welford / EWMA baseline) and persists via an `AnomalyStore`.
- `Anomaly` (`detector.rs:14-29`): `ts, value, baseline_mean, baseline_std, score, description`. Detectors: `ZScoreDetector`, `EwmaDetector`.
- `AnomalySpec.on_anomaly: ActionRef` (`spec.rs:89`); **`ActionRef` = `WakeSession` | `Notify` | `Webhook { route_id }`** (`spec.rs:19-35`). The `Webhook` variant's `route_id` is *"as registered in the scheduler"* — anomaly webhooks already route through the scheduler's webhook system.
- The registry **does not fetch data** — the caller must run the spec's KPI query each tick and pass `(ts, value)` in.
- **Reusable driver pattern (#333):** `xiaoguai_anomaly::backtest(spec, &points) -> Vec<Anomaly>` (backtest.rs) builds a detector and feeds points; `/v1/anomaly/test` drives it. `/v1/anomaly/run` is a 503 stub ("no external data source"). No live poll loop exists.

### B3. `xiaoguai-scheduler` + serve wiring — **V**
- The scheduler is a **dedicated crate** with **Cron / Interval / reactive `Trigger`s** (Cargo desc + `cron` dep; `Trigger::interval(…)` in `composite_executor.rs:98`). Jobs are `ScheduledJob`; a **`CompositeExecutor` picks a `JobExecutor` per job** (`composite_executor.rs:52,64`) — the clean extension seam.
- Started in `serve` at `crates/xiaoguai-core/src/lib.rs:610-620`: `JobRunner::new(jobs, runs, executor, audit_appender).run_loop(event_rx, Some(tick))` on a tokio task, gated by `settings.scheduler.enabled` (`:514`), tick = `settings.scheduler.tick_interval_secs` (`:617`). Runs are audit-chained through the same `SqliteAuditSink` as REST/IM.
- `AppState` (`crates/xiaoguai-api/src/state.rs`): has `skill_packs: Option<Arc<dyn SkillPackRepository>>` (`:271`) and `watchers: Option<Arc<dyn WatcherIntrospector>>` (`:325`, backs `/v1/watchers/*`, 503 when `None`). **No `watch_registry` / `anomaly_registry` fields yet.**

### B4. Pack loader + storage + CLI — **V**
- `crates/xiaoguai-core/src/packs.rs` (feature `packs`): `PackLoader::load` parses + validates; `PackManifest.watches`/`.anomalies` are `Vec<PackPath>` (`:78,82`); `WatchRegistry`/`AnomalyRegistry`/`register_*` are **no-op stubs** (`:284-323`).
- Storage: `installed_skill_packs` (`migrations/0015_skill_packs.sql`) = `pack_slug TEXT, version TEXT, config TEXT DEFAULT '{}', UNIQUE(pack_slug)` — **no `enabled`/`activation_status` column** (the API's `activation_status:"pending"` is computed, not stored). DB at `~/.xiaoguai/data.db` (`xiaoguai-storage/src/db.rs:22-32`).
- CLI: `PackCmd::Validate { dir }` exists (`cli_args.rs`); **no `Install` subcommand**. Handler `commands/pack.rs:54 validate()`.

### B5. Corrections to the scouting pass (do not propagate)
1. **"Incident creation is an existing on_anomaly sink"** — ✗. `on_anomaly` is only `WakeSession`/`Notify`/`Webhook`. Auto-creating an incident (via the #323 incident pipeline) would be **new** work — listed as an optional enhancement in §C5, not assumed.
2. **"A session-aware watcher adapter is needed"** (a TODO in `xiaoguai-api/watchers.rs`) — moot under DEC-033. With a single owner there are no per-tenant/per-session watcher scopes; global-by-`spec.id` is correct. The TODO is dissolved by the product model, not a Phase-2 task.
3. **"Add `watch_registry`/`anomaly_registry` as `dyn` traits in `AppState`"** — the crate types are concrete structs, not traits; they go in as `Arc<…>` concrete handles (or an owned driver handle), not `Arc<dyn …>`.

---

## C. Key design resolution

> **Host each pack watch/anomaly spec as a `ScheduledJob` in the existing
> `xiaoguai-scheduler`, executed by two new `JobExecutor`s behind the
> `CompositeExecutor`.** This is the central decision; everything else follows.

Rationale — it **reuses** (per [[feedback-reuse-over-build]] / DEC-033):
- the scheduler's **working Cron + Interval triggers** → the pack YAMLs' cron schedules Just Work, sidestepping xiaoguai-watch's 60 s-only timer (§B1) with **zero new scheduling code**;
- the scheduler's **tick loop, retry policy, per-day budget, and audit-chained runs** (§B3) — no parallel lifecycle to build or secure;
- the `CompositeExecutor` **dispatch-by-job-kind** seam that already exists.

The alternative (drive watches via `WatchRunner::run()`'s own task pool + a separate anomaly poll loop) means **two** lifecycles, re-implements scheduling the scheduler already does, and inherits the watch crate's cron gap. Rejected as more code for less.

### C1. Scheduling — one job per spec
On install, each `watches[]` / `anomalies[]` spec becomes a `ScheduledJob` whose `Trigger` is the spec's `schedule` (Cron or Interval). The job payload carries `{ kind: "pack.watch" | "pack.anomaly", pack_slug, spec_path }`. The scheduler fires it; the executor (C3) does one poll.
- **REC:** keep `WatchSource::poll()` (one-shot) + `DedupCache` from xiaoguai-watch as **libraries**, called per fire. Do **not** use `WatchRunner::run()` (its self-driving 60 s timer is the part we're replacing).

### C2. Spec normalization (the real Phase-1 leftover) — **V + REC**
The pack `watches[]` YAMLs are **not schema-uniform** with the crate's `WatchSpec`: `ar-collections` uses `source.sql.query` + `on_match`, `lease-management` uses `source.kind: sql` + `event:` + `payload_mapping:` + `trigger:`. **REC:** Phase 2 ships a `xiaoguai-pack-normalize` mapping layer (in `xiaoguai-core::packs`) that converts both pack idioms → the canonical `WatchSpec`/`AnomalySpec`, and `pack validate` (Phase 1) is extended to run the same normalization so authoring errors surface offline. Anomaly specs are closer to uniform but get the same treatment.

### C3. Registries & driver handle in `AppState` — **REC**
Add to `AppState`: an `Arc<Mutex<AnomalyRegistry>>` (stateful baselines must persist across fires) and a watch-source/dedup map. Construct them in `run_serve` **before** scheduler start (`lib.rs:~600`), and hand clones to the two new executors so a fire can reach the live registry. (Concrete handles, not `dyn` — see §B5.3.)

### C4. Enabled-packs persistence + install CLI — **REC**
- Migration `00NN_pack_enablement.sql`: add `enabled INTEGER NOT NULL DEFAULT 1` and `pack_dir TEXT` to `installed_skill_packs` (DEC-033: state lives in the one SQLite, not a side YAML).
- `xiaoguai pack install <dir>`: validate (reuse Phase 1) → upsert `installed_skill_packs` (`enabled=1`, record `pack_dir`) → register the spec jobs. Idempotent on `pack_slug`. A matching `xiaoguai pack disable/uninstall <slug>` flips `enabled` and removes the jobs.
- **Boot scan** in `run_serve` (after the skill-packs repo is built, before scheduler start): list `installed_skill_packs WHERE enabled=1`, load each manifest from `pack_dir`, ensure its spec jobs exist. This restores drivers across restarts. Marketplace `/v1/skills/install` may later flip `activation_status` to `active` once a row's jobs are live (Phase 5).

### C5. Action dispatch — **REC**
On a fire that produces a `WatchEvent` / `Anomaly`, dispatch its `ActionRef`:
- `Notify` → existing IM notify sinks; `Webhook { route_id }` → the scheduler's existing `WebhookPusher` (already wired, §B3); `WakeSession` → spawn an agent session via the runtime.
- **Optional enhancement (not assumed):** also open an incident via the #323 incident pipeline (`incident_pipeline.rs`). Gated behind an explicit per-spec opt-in; **new** code, parked unless the owner wants it in Phase 2.

### C6. Anomaly data access — **V + REC**
Each anomaly fire must turn `spec.kpi_query` into one `(ts, value)`. **REC:** Phase 2 supports **SQL KPI queries against the embedded SQLite read pool only** (DEC-033 — no external TSDB). HTTP/Prometheus sources are out of scope for v1 (matches the `/v1/anomaly/run` 503 rationale). This bounds the driver to data already in the one DB.

---

## D. Phase 2 — sliced into reviewable PRs (TDD, each gated on Build-and-test)

- **2a — Spec normalization (no runtime).** `packs::normalize` maps both pack idioms → `WatchSpec`/`AnomalySpec`; extend `pack validate` to normalize + reject unknown keys (closes the §C2 / Phase-1 risk). Pure functions, table-tested. Zero serve-path change. *Lowest risk; unblocks the rest.*
- **2b — `pack install`/`disable` CLI + enablement migration.** §C4 storage + CLI; no driver yet (jobs are recorded but a feature flag keeps execution off). Tests: install→row+jobs, idempotency, disable→jobs gone.
- **2c — Anomaly executor.** New `AnomalyPollExecutor` (kind `pack.anomaly`): run SQL KPI → `registry.observe()` → dispatch `on_anomaly`. Registry in `AppState` (§C3). Reuses the #333 detector path. Tests: seeded series fires at the expected tick; cooldown respected.
- **2d — Watch executor.** New `WatchPollExecutor` (kind `pack.watch`): `WatchSource::poll()` + `DedupCache` + dispatch `on_match`. Tests: dedup across fires; SQL source against a seeded table.
- **2e — Boot scan + marketplace flip (Phase 5 seam).** Wire the boot scan (§C4); flip `activation_status` → `active` for live packs. End-to-end: install a pack → restart → driver runs.

Each slice is independently shippable and reviewable; 2c/2d are the load-bearing ones and can land behind a `[packs].drivers_enabled` flag until validated on a real pack.

---

## E. DEC-033 guardrails + explicit scope cuts

- **Binding:** no Postgres/Redis/external queue; all state (enablement, anomaly baselines via `AnomalyStore`, scheduled jobs, audit) in the one embedded SQLite. Single-owner — no per-tenant pack scoping (this dissolves the "session-aware watcher" TODO, §B5.2). Single binary — packs ship in-repo / operator dirs, not a network registry.
- **Cut from Phase 2:** per-pack SQL migrations (§A.2); pack `agents[]` execution (§A.4); HTTP/Prometheus anomaly sources & non-SQL watch sources (§C6); auto-incident-creation (§C5, optional); cron *in xiaoguai-watch itself* — we get cron from the scheduler instead, and may later backfill the crate's own cron as independent cleanup.

---

## F. Risks / open questions for review

1. **Scheduler job model fit.** This design assumes `ScheduledJob` + `Trigger` cleanly carry a JSON payload and a per-job cron, and that `CompositeExecutor` dispatch-by-kind is the intended extension point. Verified at the type level (§B3); **a 2a/2c spike should confirm** the `ScheduledJob` constructor + payload routing before committing 2c/2d.
2. **Anomaly baseline persistence.** `observe()` is stateful; across restarts the baseline resets unless the `AnomalyStore` rehydrates it. Phase 2c must decide: cold-start re-warm (replay recent history from SQLite) vs. accept a warm-up window after boot. **REC:** accept warm-up for v1, document it.
3. **KPI query trust.** `spec.kpi_query` is operator-authored SQL run against the live DB. Single-owner lowers the risk, but the executor should run it read-only (reader pool) with a timeout. (Boundary-validation per the global input-validation rule.)
4. **Dedup semantics.** Pack YAMLs declare rich re-fire rules (`re_fire_if: "payload.x > prior.x*1.1"`) the crate's SHA-256 `DedupCache` doesn't evaluate. v1 honors key+TTL dedup only; expression re-fire is a documented gap.

---

## G. References (grep-verified 2026-06-23)

- `crates/xiaoguai-watch/src/{lib.rs:69-77, spec.rs:78-107, runner.rs:174-276}` — WatchSpec/Runner/Event; **cron→60 s fallback at runner.rs:273-276**.
- `crates/xiaoguai-anomaly/src/{registry.rs:108-144, detector.rs:14-29, spec.rs:19-89, backtest.rs}` — registry `observe`, `Anomaly`, `ActionRef`, the #333 backtest path.
- `crates/xiaoguai-scheduler/src/{composite_executor.rs:52-98, executor.rs:28, job.rs}` — `Trigger`, `JobExecutor`, `CompositeExecutor`; Cargo `cron` dep.
- `crates/xiaoguai-core/src/lib.rs:514-622` — scheduler spawn in `serve`. `crates/xiaoguai-api/src/state.rs:271,325` — `skill_packs`, `watchers` (no watch/anomaly registry).
- `crates/xiaoguai-core/src/packs.rs:55-323` — `PackManifest`, `PackLoader::load`, register stubs.
- `crates/xiaoguai-storage/migrations/0015_skill_packs.sql` — `installed_skill_packs` (no `enabled`). `crates/xiaoguai-storage/src/db.rs:22-32` — `~/.xiaoguai/data.db`.
- `crates/xiaoguai-cli/src/{cli_args.rs (PackCmd), commands/pack.rs:54}` — `validate` only, no `install`.
- `packs/ar-collections/watches/dso-over-60.yaml`, `packs/lease-management/watches/renewal-window-approaching.yaml`, `packs/data-eng-pipeline/anomalies/row-count-drop.yaml` — non-uniform spec idioms (§C2).
