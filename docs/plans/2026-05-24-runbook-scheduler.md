# v1.0.1 — operator runbook: scheduler + IM chapters

**Status:** done (2026-05-24).

## Why this slice

The v0.5–v0.9 runbook content predates the entire v0.10.x → v0.12.x
scheduler band. An operator dropping in today has no canonical answer
for:

- How to turn the scheduler on, what gets spawned, what migration
  underlies it.
- How to wire per-sink config (Feishu / Telegram / Email / Inbox) and
  where credentials go.
- How the webhook + file-watch sources work, what's gated by admin
  auth today, what's deferred.
- How proactive triggers actually fire (checker + budget + reason)
  and why "no checker installed ⇒ no fires" is the intentional
  default.
- How to verify the `actor = "scheduler:<job_id>"` audit chain and
  what "broken chain" investigation looks like.
- The five most common operational failure modes and their fixes.
- The v0.7.4 IM PG-history default and the
  `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY` escape hatch, plus the
  v0.12.1 synthetic-session-per-scheduled-run shape.

This tag is docs-only. No code change, no test count change.

## Pieces

Extend `docs/runbooks/operator.md` with two new chapters appended
after the existing v0.5–v0.9 sections:

1. **Scheduler operations** — 8 sub-sections:
   1. Scheduler overview (`JobRunner`, timer arm, event-channel arm,
      6 trigger variants, 4 sink types).
   2. Enabling the scheduler (`[scheduler].enabled`,
      `tick_interval_secs`, migration `0007_scheduled_jobs.sql`).
   3. Configuring push sinks (per-sink config block + env-var
      credential separation).
   4. Webhook source (route, admin-bearer gate today, per-tenant
      tokens deferred).
   5. File-watch source (`[scheduler.file_watch]`, route merge
      semantics, debouncer caveat).
   6. Proactive triggers (budget knob, reason-required contract,
      fail-safe rationale).
   7. Audit chain (`actor = "scheduler:<job_id>"` rows, verify
      endpoint, "broken chain" interpretation).
   8. Troubleshooting (5 scenarios: stuck job, runaway budget,
      webhook 404, audit chain break, macOS fsevent
      `/var → /private/var`).

2. **Sessions + IM history operations** — 1 short chapter covering
   `[im].use_in_process_history`, the
   `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY` escape hatch, the
   `max_messages_per_conversation` replay cap, and the v0.12.1
   synthetic-session-per-scheduled-run shape (user_id =
   `scheduler:<job_id>`).

## Acceptance

- Runbook word count grows from 355 to ~2400 words.
- No code change, no test impact.
- Tagged locally as `v1.0.1-runbook`.

## Deferred

- **Admin-ui Scheduler pane walkthrough.** The pane itself is
  deferred to v0.12.1.1; once it lands the runbook gains a "click
  here to create a job" screenshot pass.
- **Per-tenant webhook token rotation procedure.** Lands together
  with the token mechanism itself (v0.12.x.1).
- **`CompositeExecutor` payload-routing recipe.** Today every job
  runs through `RuntimeJobExecutor`; the `rag_reindex` payload
  dispatch is v0.12.2.1 work. Runbook will document the recipe
  when the executor actually picks `RagReindexExecutor`.
- **Release README + screenshots.** Separate slice; see
  `docs/HANDOFF-2026-05-24.md` §5.

## Design decisions worth flagging

- **Append, don't restructure.** The v0.5–v0.9 content is correct
  and load-bearing. New chapters land at the bottom; existing
  sections untouched. Operators searching for "rotating HMAC key"
  still find it where they expected.
- **Five troubleshooting scenarios, all drawn from real plan-doc
  history.** No invented failure modes. `git checkout` burst → file
  watcher saturation comes from v0.10.1 deferred-list; macOS
  `/private/var` redirect comes from the v0.10.1 integration test;
  webhook 404 comes from the v0.12.0 route shape; audit chain break
  comes from the v0.6.5 verify endpoint semantics.
- **Tag locally only.** Caller explicitly asked for no push. Tag is
  there so a future operator can `git log v1.0.1-runbook..main` to
  see exactly which runbook revision their training was based on.
