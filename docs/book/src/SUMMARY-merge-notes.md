# SUMMARY.md Merge Notes

Pre-merge traceability for `chore/integration-summary-merge`.
Generated 2026-05-25. Base: `origin/main` @ `9970aa0`.

## Branches Consolidated

| Branch | SHA (short) | Entries Added | Section |
|--------|-------------|---------------|---------|
| `docs/book-watch-anomaly` | `ed44e50` | `[Active Wakeup: Watchers & Anomaly Detection](operator/active-wakeup.md)` | Operator Guide |
| `docs/book-hotl-outcomes` | `355ee84` | `[Human-on-the-Loop Policy](operator/human-on-the-loop.md)`, `[Outcome Telemetry](operator/outcome-telemetry.md)` | Operator Guide |
| `docs/kanban-design` | `09f8cf7` | `[Task Board](operator/task-board.md)` | Operator Guide |
| `docs/runbooks-wave3` | `4053ee2` | 5 runbook entries + `# Runbooks — wave-3` section | New section (between Operator Guide and Developer Guide) |
| `docs/book-skill-packs` | `40abd97` | `[Skill Packs](skills/skill-packs.md)` | Skills Catalog |
| `docs/glossary-wave3` | `595702e` | `[Glossary](glossary.md)` + `# Glossary` section | New section (end of file) |

## Branches with No SUMMARY.md Changes vs Main

These branches added content files but did not modify `docs/book/src/SUMMARY.md`:

| Branch | SHA (short) | Note |
|--------|-------------|------|
| `docs/adrs-wave3` | `3022864` | ADR files added directly; no SUMMARY entry added on branch |
| `docs/contributing-wave3` | `377f561` | Contributing doc extensions; no SUMMARY entry added |
| `docs/cli-wave3` | `a1310e3` | CLI reference docs; no SUMMARY entry added |
| `docs/per-env-setup` | `966c98a` | Per-env setup playbook; no SUMMARY entry added |
| `docs/dr-playbook-wave3` | `6b5ef13` | DR playbook; no SUMMARY entry added |
| `docs/multi-region-failover` | `a938ee1` | Multi-region docs; no SUMMARY entry added |
| `docs/backup-wave3` | `a9ad702` | Backup docs; no SUMMARY entry added |
| `docs/pyroscope-setup` | `e3b6a51` | Pyroscope setup; no SUMMARY entry added |

**Action required during convoy merge**: Each of these branches should add a SUMMARY.md entry for
their content, or a consolidator commit should add them after content files are merged into main.

## Merge Decisions

- Operator Guide insertions: ordered as active-wakeup → hotl-policy → outcome-telemetry → task-board
  (preserves each branch's relative position after `Release Signing`).
- Runbooks section inserted between Operator Guide and Developer Guide (matching `docs/runbooks-wave3`
  placement intent).
- Glossary appended at end of file after Roadmap section (matching `docs/glossary-wave3` intent).
- Runbook link paths use `../../runbooks/` (relative from `docs/book/src/`); these targets live on
  `docs/runbooks-wave3` — flagged as unverified pending merge (see below).

## Unverified Link Targets

The following linked files exist on their source branches but are not yet on `main`:

| Link path | Source branch |
|-----------|---------------|
| `operator/active-wakeup.md` | `docs/book-watch-anomaly` |
| `operator/human-on-the-loop.md` | `docs/book-hotl-outcomes` |
| `operator/outcome-telemetry.md` | `docs/book-hotl-outcomes` |
| `operator/task-board.md` | `docs/kanban-design` |
| `skills/skill-packs.md` | `docs/book-skill-packs` |
| `glossary.md` | `docs/glossary-wave3` |
| `../../runbooks/hotl-escalation-stuck.md` | `docs/runbooks-wave3` |
| `../../runbooks/outcome-chain-debug.md` | `docs/runbooks-wave3` |
| `../../runbooks/pack-install-troubleshoot.md` | `docs/runbooks-wave3` |
| `../../runbooks/im-adapter-onboarding.md` | `docs/runbooks-wave3` |
| `../../runbooks/anomaly-false-positive-triage.md` | `docs/runbooks-wave3` |

All links will resolve correctly once the source branches are merged into main.
`mdbook build` will fail on this branch until then; this is expected for the pre-merge integration commit.
