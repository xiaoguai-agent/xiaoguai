# Implementation plan — T8: Install-and-go polish

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED (owner blanket "全部执行" 2026-06-10)** |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-G / §3-T8 |
| Hard constraints | DEC-033 unchanged; **no desktop shell** |

## 0. Goal & grounding

Close the "UX-dark" gap the explore pass found (2026-06-10): the install
chain (pip/deb/rpm/tarball, Ollama-local default, migrate-on-connect, init
wizard) is technically solid, but a fresh user gets no guidance at the three
moments that matter — after `serve` starts, after `init` finishes, and when
something's wrong. Skipped deliberately: the YAML doc-generator idea
(over-engineering; the existing four-docs-stay-in-sync rule covers it).

## 1. Deliverables

### T8.1 — first-run guidance (S)
- `serve` prints a post-bind banner: `✓ xiaoguai running at http://<addr>` +
  one next-step line (chat UI URL / first-message command). Plain stdout, no
  auto-open browser (server installs; an env opt-in can come later).
- `init` final output gains the two next-step lines (start serve → first
  message / open UI).
- Port-in-use bind error becomes actionable: detect EADDRINUSE and print the
  three remedies (kill / `--port` / lsof hint) instead of a bare anyhow.

### T8.2 — `xiaoguai doctor` (M)
New CLI subcommand printing a ✓/✗ checklist, exit 1 if any ✗:
- data dir + DB writable (migrate-on-connect dry check),
- provider configured? default provider key present (reuse the init wizard's
  provider listing),
- Ollama reachability when an ollama provider is default (GET /api/tags on
  the endpoint) + whether the seeded model is present (warn + `ollama pull`
  hint),
- port 7600 (or configured) free / already-serving (healthz probe → "already
  running" is a ✓ with a note).
Each check is a pure-ish testable fn; network checks behind short timeouts.

### T8.3 — `xiaoguai service install|uninstall|status` (M)
- Linux: writes the existing `deploy/systemd/xiaoguai-core.service` content
  (embedded via include_str!) to /etc/systemd/system, creates user/dirs
  (idempotent, mirrors the rpm post-install scriptlet), daemon-reload +
  enable + start; requires root with a clear error otherwise.
- macOS: writes a launchd plist (new template under deploy/launchd/) to
  `~/Library/LaunchAgents` + `launchctl load`; no root needed.
- `status` shells out to systemctl/launchctl. Windows: friendly "not
  supported, use Docker/WSL" message.
- This makes /loop + scheduler genuinely survivable — the daemon-resident
  features finally have a one-command daemon.

### T8.4 — boot posture + docs (S)
- Empty-providers boot: keep the mock fallback (tests/e2e rely on it) but
  promote the warning to a prominent multi-line stderr banner with the two
  paths (Ollama pull / `xiaoguai init`); banner suppressed when
  `XIAOGUAI_LLM__MOCK=true` (explicit opt-in stays quiet).
- `docs/user-guide/install-and-verify.md`: per-method install → expected
  output → smoke test (healthz/doctor) table; quickstart gains the
  "running as a daemon" section pointing at `service install`; README
  cross-links. (Respect the four-docs-sync rule — update all touched ones.)

## 2. Boundaries

- No desktop shell, no auto-open browser, no doc generator, no brew formula
  work (separate decision), no changes to wheels/packaging pipelines.

## 3. Verification

Each CLI piece: unit tests on the check/templating fns + workspace green.
service install/doctor get a manual smoke on this dev machine (macOS launchd
path) recorded in the PR.
