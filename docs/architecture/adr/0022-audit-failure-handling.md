# ADR-0022 — Audit-failure handling: best-effort audit sinks, fail-closed generic runtime hook

Date: 2026-06-06
Status: Accepted

## Context

The audit round-3 follow-up flagged an apparent contradiction. Two patterns
coexist for "what happens when a sink write fails":

1. **Dedicated audit-append paths are best-effort (non-blocking).** Every place
   that appends to the HMAC audit chain discards the error after a best-effort
   log and lets the main operation succeed:
   - HotL decision route — `let _ = sink.append(entry).await;`
     (`routes/hotl_decisions.rs`)
   - REST chat `agent.run` audit — `if let Err(err) = sink.append(...) { warn!() }`
     (`routes/sessions.rs`, added in the same backlog)
   - skill-author audit, scheduler audit — same shape.

   This matches the project philosophy: **an audit-write failure must not block
   the main operation** (degrade to a log, keep going).

2. **The generic `run_to_sink` runtime hook is fail-closed.** In
   `crates/xiaoguai-runtime/src/lib.rs`, `sink.on_finish(&outcome).await?`
   propagates a sink error to the caller, failing the whole run.

The flag asked: doesn't (2) contradict the "audit must not block" rule?

Key fact established while triaging: **`run_to_sink` is currently an additive,
not-yet-wired hook** — its only call sites are its own unit tests; the scheduler
runner calls its executor directly today (see the function's own doc comment).
So (2) is a forward-looking design choice on an unused entry point, not a live
production behaviour.

## Decision

**Keep `run_to_sink`'s `on_finish` fail-closed.** (Owner decision, 2026-06-06.)

`run_to_sink` is a *generic* runtime sink hook, not an audit-specific path. Its
sink does more than audit — a real adopter (the scheduler) will also persist the
job outcome and push notifications in `on_finish`. A generic hook should surface
a genuine sink failure (e.g. "could not persist the job outcome") to its caller,
which decides whether to retry or mark the job failed. Swallowing all sink errors
there would silently lose job-outcome persistence, which is worse than failing
loudly.

The "audit must not block" guarantee is provided **where it actually matters** —
the dedicated audit-append paths in pattern (1), which are all best-effort. Audit
*integrity* (a separate concern from availability) is served by the HMAC chain
itself, not by forcing the main op to fail on an append error.

**Guidance for the eventual `run_to_sink` adopter:** keep the audit-append
portion *inside* your `on_finish` best-effort (log-and-continue), so an audit
failure does not propagate; reserve the propagated error for genuine
sink-operation failures (outcome persistence, etc.).

## Consequences

- **Positive:** no contradiction in practice — live audit writes never block;
  the generic hook stays honest about non-audit sink failures.
- **Positive:** the decision is now documented at the code site
  (`run_to_sink`) and here, so a future adopter doesn't re-litigate it.
- **Negative / watch-item:** if a future adopter naively does the audit write
  with `?` inside `on_finish`, it *would* make audit failure block the run.
  The guidance above and the code comment mitigate this.

## Related: non-blocking CI gates stay manual (mutation + perf)

A second governance item from the same backlog: `mutation-testing.yml` and
`perf-regression.yml` are `workflow_dispatch`-only (manual).

**Decision (2026-06-06): keep them manual-only for now.** Both were demoted from
scheduled/push triggers because they were failing and generating PR noise
(cargo-mutants baseline drift; k6 p95-budget flakiness). Promoting a flaky gate
to required would erode trust in CI more than the coverage gains. They stay
available on demand; promotion is deferred until the underlying baselines are
stabilized. Documented in-file at the top of each workflow.

## References

- `crates/xiaoguai-runtime/src/lib.rs` — `run_to_sink` / `on_finish`
- `crates/xiaoguai-api/src/routes/hotl_decisions.rs`,
  `crates/xiaoguai-api/src/routes/sessions.rs` — best-effort audit appends
- DEC-004 (HMAC audit chain, design repo) — audit integrity is chain-based, not block-based
- `docs/HANDOFF-2026-06-06-audit.md` §"SESSION FINAL STATE" — the deferred item
