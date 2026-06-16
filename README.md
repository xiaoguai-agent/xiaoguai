# Xiaoguai 小怪

> **Rust-implemented, audit-first, scheduler-native local agent platform.**
>
> *Your Little Agent for Big Work · 小怪不小，能办大事*

**English** · [简体中文](README.zh-CN.md)

**Documentation:** the handbook source lives in [`docs/book/`](docs/book/) — build it locally with `mdbook build docs/book` (see [Documentation](#documentation) below).

Xiaoguai is a self-hostable AI agent platform for technical individuals,
small teams, and anyone with a compliance or traceability constraint.
Every tool call writes an HMAC-chained audit row. Every scheduled job
carries a retry policy, a replayable transcript, and a reason field.
Every model interaction has a regression-eval safety net. The whole
thing ships as a single self-contained Rust binary with an embedded
SQLite store — no external database, no Python runtime, no JVM, no JS
server on the hot path.

We are not trying to out-prompt the prompt-magic vendors, out-polish
the UI vendors, or out-host the marketplace vendors. We compete on
engineering seriousness — *models are unreliable, but systems can be
reliable.*

## What makes it different

| Capability | Xiaoguai | n8n | Dify | OpenWebUI / LobeChat |
|---|---|---|---|---|
| **Audit-first console.** `Today` is the default landing page — every chat / IM / scheduled run with HMAC-chained audit metadata. Chat is a secondary entry. | First-class | Workflow runs only | Workflow runs only | Chat-first; audit not surfaced |
| **Scheduler-native (passive → reactive → proactive).** Cron + file watcher + webhook + LLM-initiated runs with per-user budget and a required `reason` field. | First-class | Strong on triggers / weak on agents | Cron only | None |
| **MCP two-way.** Consumes stdio / SSE / streamable-HTTP MCP servers *and* publishes its own toolbox at `/v1/mcp/serve`. External agents see what internal agents see. | First-class | Consumer only | Consumer only (v1.6+) | Limited / via plugins |
| **RAG with first-class citations.** `ContentBlock::Citation` is a typed variant — source URI, line span, preview, score. Adapters that can't cite must not silently emit unsourced text. | First-class | None native | Cited in UI, schema opaque | Cited; long-standing bugs (#12655, #20829) |

## Quickstart

Xiaoguai is a **single binary over an embedded SQLite file** — no Postgres,
no Redis, no Docker required. Every path below ends the same way: a `xiaoguai`
process serving `:7600`, self-contained on `MockBackend` out of the box. Pick
the one that matches what you have.

Verify any of them with `curl http://localhost:7600/healthz` → `ok`, or run
the built-in self-check `xiaoguai doctor`; keep it running across reboots with
`xiaoguai service install`. Per-method expected outputs and smoke tests:
[docs/user-guide/install-and-verify.md](docs/user-guide/install-and-verify.md).

**What each method gives you:**

| Method | Needs toolchain / root | Bundled web UI | Best for |
|---|---|---|---|
| **A. pip / pipx** | no | ✗ — API + CLI only | quickest start; scripting; servers you drive by CLI/API |
| **B. .deb / .rpm / tarball** | root (systemd) | ✓ chat at `/`, admin at `/admin/` | a managed host that should serve the browser UI |
| **C. from source** | Rust toolchain | ✗ — API + CLI only | development / custom builds |
| **D. Docker** | Docker | ✓ | the full stack in one command |

> **No web page at `http://localhost:7600/`?** That's *expected* on **pip** and
> **from-source** installs — they ship the API + CLI only, so `/` returns 404
> while `/healthz` and `/v1/**` work fine. To get the browser console, install a
> package or use Docker (B / D), or point a pip install at a bundled UI — see
> [Web UI](#web-ui) below. Either way you can chat right now from the terminal:
> `xiaoguai chat --prompt 'hello'`.

### Option A — pip (no toolchain, no sudo, all platforms) — recommended

```bash
pip install xiaoguai
xiaoguai serve   # :7600, auto-creates ~/.xiaoguai/data.db, no config needed
```

On Debian 12 / Ubuntu 24 and other PEP 668 "externally-managed" systems, `pip
install` into the system Python is blocked — use **pipx** (it isolates the app
and still puts `xiaoguai` on PATH):

```bash
sudo apt install -y pipx && pipx ensurepath
pipx install xiaoguai      # then reopen the shell, or: exec $SHELL
```

Installs a platform wheel that bundles the native `xiaoguai` binary on PATH
(macOS arm64/x86_64, Linux x86_64/aarch64) — works inside a venv without root.
Gives you the API + CLI; for the bundled web UI use the native packages
(Option B) or Docker (Option D). Offline sanity check, no server needed:
`xiaoguai chat --mock --prompt 'hello'`.

### Option B — pre-built package (no toolchain, bundles the web UI)

After install the systemd unit starts automatically; open `http://<host>:7600/`
(chat) and `/admin/` (console).

| Platform | Command |
|---|---|
| Debian / Ubuntu (amd64) | Download `xiaoguai-cli_*_amd64.deb` from the [latest release](https://github.com/xiaoguai-agent/xiaoguai/releases/latest) and `sudo apt install ./xiaoguai-cli_*_amd64.deb` |
| RHEL / Fedora / Rocky (amd64) | Download `xiaoguai-cli-*.x86_64.rpm` from the same release and `sudo rpm -i xiaoguai-cli-*.x86_64.rpm` |
| Bare-metal tarball (amd64 / arm64, glibc 2.35+) | Download `xiaoguai-vX.Y.Z-<arch>-unknown-linux-gnu.tar.gz`, extract, and `sudo bash scripts/install.sh` (systemd) |

**One-step (any Linux, no sudo, no systemd):**

```bash
curl -fsSL https://raw.githubusercontent.com/xiaoguai-agent/xiaoguai/main/scripts/quickstart-linux.sh | bash
```

Detects your CPU arch, downloads + checksum-verifies the latest tarball into
`~/.xiaoguai` (web UI bundled), runs the setup wizard (provider + API key,
including the MiniMax China / international region picker), and starts `:7600`
with the browser console. Pin a release with `XIAOGUAI_VERSION=vX.Y.Z`; answer
the LAN prompt to bind `0.0.0.0` (it collects owner credentials per SEC-01).

### Option C — from source (needs a Rust toolchain)

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai
cargo install --path crates/xiaoguai-cli --locked
xiaoguai serve   # boots on embedded SQLite (~/.xiaoguai/data.db), :7600, no config needed
```

This path gives you the API + CLI; the bundled chat/admin web UI ships only
with the packages (Option B) and the Docker image (Option D). For a no-network
sanity check without even starting the server:

```bash
xiaoguai chat --mock --prompt 'hello'
```

The sandboxed code-execution MCP server (`xiaoguai-mcp-exec`) builds from the
same workspace: `cargo install --path crates/xiaoguai-mcp-exec --locked`.

### Option D — Docker (one command, full stack + web UI bundled)

```bash
docker compose -f deploy/docker-compose.yml up --build
# first build ~2 min, then open http://localhost:7600/
```

Requires the Docker Compose **v2 plugin** — check with `docker compose version`.
If that errors (`unknown shorthand flag: 'f'`), the plugin is missing: install
`docker-compose-plugin`, or just use Option A / B / C instead.

`xiaoguai serve` is the canonical entrypoint everywhere. The legacy
`xiaoguai-core` shim still works (the .deb wires it in for systemd
backward-compat). For real LLM providers, MCP registration, the admin console,
and config details, see [`docs/user-guide/quickstart.md`](docs/user-guide/quickstart.md).

### First chat — talk to your running server

Out of the box `xiaoguai serve` boots on a built-in `MockBackend`, so the
round-trip works the moment the server is up:

```bash
xiaoguai chat --prompt 'hello'    # talks to the :7600 server you just started
```

`chat` auto-creates a session, streams the reply, and routes through your
registered providers + HotL + audit — no session id to juggle. Register a real
provider to get real answers (interactive, writes to the local DB):

```bash
xiaoguai init                     # pick a provider, paste its API key, set default
# restart `xiaoguai serve`, then:
xiaoguai chat --prompt 'introduce yourself in three sentences'
```

> **MiniMax users — pick your region.** `xiaoguai init` asks International
> (`api.minimax.io`) vs China (`api.minimaxi.com`). The two regions use
> different hosts and their API keys are **not interchangeable** — a
> China-console key against the international host returns `400 / Missing
> Authorization`. If you hit that, re-run `init` and choose China, or:
> `xiaoguai provider update --id <id> --endpoint https://api.minimaxi.com`.
> With the web console you can instead do this in **Admin → Providers → Edit**
> (the endpoint field offers the `api.minimaxi.com` preset; paste the key there)
> — no CLI required.

Want a multi-turn conversation that keeps history? Use `xiaoguai cli` (also
`xiaoguai start`; `xiaoguai repl` still works). Working offline or without a
server? Stay direct with `xiaoguai chat --mock --prompt 'hello'` (or
`--ollama-url http://localhost:11434`).

### Web UI

A browser console — **chat at `/`, operator admin at `/admin/`** — is bundled
with the packages, tarball, and Docker image (Options B–D) and served
automatically. **pip and from-source installs ship the API + CLI only**, so
`http://localhost:7600/` returns 404 by design (this is the single most common
"is it broken?" question — it isn't).

The chat surface (`/`) has a **model picker** that lists only models from
providers with an API key configured — and, once you've run a connectivity
probe (below), only the models that actually responded — so you never pick one
that 401s. It also has persisted **session history** in the sidebar (rename /
delete; it survives navigating to the admin console and back), and a **Consult**
(read-only) vs **Execute** turn toggle. The operator console (`/admin/`) manages
providers, scheduler, HotL policies, skill packs, audit, memory, and more — each
pane carries an inline "what is this / how to use it" intro. You can set or
change a provider's **API key and endpoint directly in the Providers pane**
(Edit → the endpoint field suggests presets including MiniMax China
`api.minimaxi.com`), so no CLI is needed to point the web UI at a working model.
Each provider row also has a **Test models** button: it fires a minimal request
at every model the provider advertises and records the set that connected, which
is exactly what the chat picker then offers.

To add the web UI to a pip / source install, grab the built UI from a release
tarball and point `server.static_dir` at it:

```bash
# x86_64 shown; use the aarch64 tarball on ARM hosts
curl -sL https://github.com/xiaoguai-agent/xiaoguai/releases/download/v1.22.0/xiaoguai-v1.22.0-x86_64-unknown-linux-gnu.tar.gz | tar xz
# the bundled UI lives under share/xiaoguai/static (contains chat-ui/ + admin-ui/)
export XIAOGUAI_SERVER__STATIC_DIR="$PWD/xiaoguai-v1.22.0-x86_64-unknown-linux-gnu/share/xiaoguai/static"
pkill -f 'xiaoguai serve'; xiaoguai serve   # now http://localhost:7600/ (chat) + /admin/ (console)
```

Or persist it in `~/.xiaoguai/config.yaml` so you don't re-export each time:

```yaml
server:
  static_dir: /absolute/path/to/share/xiaoguai/static
```

When `static_dir` is unset, `serve` auto-probes `<binary>/static`,
`<binary>/../share/xiaoguai/static`, and `/usr/(local/)share/xiaoguai/static` —
which is why the packages and Docker image "just work" with zero config.

### Upgrading

Match the upgrade to how you installed (mixing methods desyncs your package
manager's bookkeeping):

| Installed via | Upgrade command |
|---|---|
| pip | `pip install -U xiaoguai` (run inside the same venv) |
| pipx | `pipx upgrade xiaoguai` |
| .deb | download the new `.deb` from the [latest release](https://github.com/xiaoguai-agent/xiaoguai/releases/latest), then `sudo apt install ./xiaoguai-cli_*_amd64.deb` (unit restarts) |
| .rpm | `sudo rpm -U xiaoguai-cli-*.x86_64.rpm` |
| tarball / bare binary | `xiaoguai self-update` — downloads + cosign-verifies the latest release and replaces the binary in place (`--check` previews without applying) |
| from source | `git pull && cargo install --path crates/xiaoguai-cli --locked --force` |
| Docker | `docker compose -f deploy/docker-compose.yml up --build -d` |

`--force` is required for the source path: `Cargo.toml` stays at `0.1.0` on
`main` (the release version comes from the git tag), so without it cargo thinks
the package is already installed and skips the rebuild.

Three things people trip on:

1. **Restart `serve` after upgrading.** A running process keeps the old binary
   in memory — `pkill -f 'xiaoguai serve'` and start it again, or
   `systemctl restart xiaoguai-core` for the packaged service.
2. **`xiaoguai --version` shows `0.1.0`?** Either it's a from-source build (the
   tag → version substitution only happens in release artifacts, so source
   builds always report `0.1.0` — confirm by git commit instead), or another
   `xiaoguai` is shadowing the upgraded one on your `PATH`. Check with
   `which -a xiaoguai`; a stray `~/.cargo/bin/xiaoguai` left over from
   `cargo install` is the usual culprit.
3. **Your data is preserved.** `~/.xiaoguai/data.db` is reused as-is; schema
   migrations run automatically on `serve` boot, so sessions, providers, and
   audit history carry over.

> **Behavior change (v1.17.0):** `xiaoguai chat --prompt '...'` now talks to the
> running `xiaoguai serve` by default — it auto-creates a session and uses your
> registered providers + HotL + audit. The old direct-to-Ollama/Mock one-shot
> moved behind `--mock` / `--ollama-url`. If you scripted `xiaoguai chat` against
> Ollama, add `--ollama-url http://localhost:11434` (or `--mock` for the canned
> backend).

## Agent teams — for complex tasks worth several perspectives

When one pass isn't enough — a security audit, a design with trade-offs, a
multi-angle research question — run the task through an **expert team** instead
of a single chat turn. The team's **members are sub-agents** (each with its own
persona and a scoped toolbox) that work the goal **in parallel**; a **lead**
then synthesizes their findings into one answer, surfacing disagreements rather
than averaging them away. Independent perspectives plus an explicit synthesis
step catch what a single agent misses — and every member and synthesis turn
still flows through the same HotL approval gate and HMAC audit chain, so the
extra horsepower stays governed.

How a complex task is scheduled: you give a **goal**; it routes to a team
(pass `--team <id>`, or omit it to auto-route to the best-matching team); the
orchestrator fans out to the members in parallel, applies a per-run HotL budget,
and the lead composes the final answer. Member failures don't abort the run —
the lead synthesizes from the survivors.

Three ways to run one:

```bash
# CLI — auto-route a goal to the best-matching team (or pass --team <id>)
xiaoguai remote orchestrate --user-id you --goal "Audit auth/ for security bugs"
```

- **Web UI:** attach a team in the chat-ui Expert picker, then click the
  **team-run** button beside the composer (execute mode).
- **API:** `POST /v1/sessions/{id}/orchestrate` streams `OrchestrateEvent`s
  (`run_started → member_completed* → synthesis_started → final`).

Create and manage teams (a lead + members, plus an optional shared glossary) in
the admin console or via `/v1/teams`. Full guide:
[`docs/user-guide/expert-center.md`](docs/user-guide/expert-center.md).

> **Sub-agents for your own work, too:** the same "fan out independent workers,
> then synthesize / adversarially verify" pattern is how you get higher-quality
> results out of any agent — decompose a big task, run workers in parallel,
> have a reviewer pass judge the outputs. Agent teams make that pattern a
> first-class, audited workflow inside xiaoguai.

## Observability (optional)

Telemetry is opt-in. Build with the `observability` cargo feature to expose
`/metrics` (Prometheus) + OTLP trace export — off by default. For a local
Prometheus/Grafana/OTel-collector stack, layer the optional compose file:

```bash
docker compose -f deploy/docker-compose.yml \
  -f deploy/docker-compose.observability.yml up --build
```

## Architecture

Three layers, ~34 Rust crates, one workspace. Substrate at the
bottom is pure data + audit; domain crates in the middle implement
the agent + MCP + RAG + scheduler + eval primitives; edges at the top
are the protocols and binaries users actually touch.

```
edges      ┌──────────────┬──────────────┬──────────────┬──────────────┐
           │ xiaoguai-api │ xiaoguai-im- │ xiaoguai-cli │ xiaoguai-    │
           │ axum REST +  │ gateway      │ chat / eval  │ core         │
           │ SSE, 15+ /v1 │ + im-feishu  │ provider /   │ production   │
           │ endpoints    │ (+dingtalk / │ mcp / remote │ binary;      │
           │              │  wecom       │              │ wires all    │
           │              │  scaffolds)  │              │ crates       │
           └──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┘
                  │              │              │              │
domain     ┌──────┴──────────────┴──────────────┴──────────────┴───────┐
           │                                                            │
           │  xiaoguai-llm     LlmBackend + Ollama / OpenAI-compat /    │
           │                   Mock + LlmRouter + circuit breakers      │
           │  xiaoguai-mcp     stdio / SSE / streamable-HTTP clients +  │
           │                   McpSupervisor (live reload from DB)      │
           │  xiaoguai-agent   Toolbox + ReactAgent::run_stream +       │
           │                   AgentEvent + sliding-window history      │
           │  xiaoguai-rag     R2R HTTP + in-mem fallback + RagMcp-     │
           │                   Adapter + reindex_path                   │
           │  xiaoguai-        Trigger × RetryPolicy × JobRun +         │
           │   scheduler       FileWatch + Webhook + ProactiveChecker + │
           │                   BudgetLedger + 4 PushSinks + SQLite repos│
           │  xiaoguai-runtime run_to_completion / run_streamed /       │
           │                   run_to_sink — shared agent loop          │
           │  xiaoguai-eval    regression + capability suites +         │
           │                   5 graders + EvalRunner + CLI             │
           └──────┬─────────────────────────────────────────────────────┘
                  │
substrate  ┌──────┴─────────────────────────────────────────────────────┐
           │  xiaoguai-types   domain types + ID newtypes               │
           │  xiaoguai-config  Settings (server / db / cache / auth /   │
           │                   audit / scheduler / im / eval)           │
           │  xiaoguai-storage sqlx + embedded SQLite repos +          │
           │                   in-process cache fallback                │
           │  xiaoguai-audit   ChainedAudit (HMAC) + SQLite sink        │
           │  xiaoguai-auth    HotL argument redaction (single-owner    │
           │                   pivot; no OIDC/Casbin — DEC-033)         │
           └────────────────────────────────────────────────────────────┘
```

For the long-form crate dependency rules and where to plug in a new
bridge (trait in `xiaoguai-api` or `xiaoguai-scheduler`, impl in
`xiaoguai-core::scheduler_bridge`), see
[`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §3.

## Status

v1 is feature-complete as of 2026-05-24. Thirteen tags landed in the
final sprint on top of v0.10.0; `cargo test --workspace` reports
**443 passed / 0 failed / 66 ignored**; clippy and fmt are clean.

Releases have continued since — latest is **v1.22.0** (2026-06-15; PyPI + GitHub
Releases + deb/rpm/tarball). v1.21.0 and v1.22.0 focused on web-console
usability: a chat model picker, persisted session history with rename/delete,
in-pane "purpose / how to use" docs, and setting provider keys + endpoints
(including the MiniMax China host) straight from the Providers pane.

| Tag | Headline | Plan doc |
|---|---|---|
| v0.10.1 | reactive triggers — FileWatch + Webhook + `JobRunner::run_loop` | [plan](docs/plans/2026-05-23-v0.10.1.md) |
| v0.6.5  | `PgAuditSink` bootstrap + audit chain verify endpoint + IM tenant routing | [plan](docs/plans/2026-05-23-v0.6.5.md) |
| v0.7.4  | IM gateway PG-history default + persisted tool turns + replay cap | [plan](docs/plans/2026-05-23-v0.7.4.md) |
| v0.9.4.1| `McpSupervisor` live-pickup on marketplace install | [plan](docs/plans/2026-05-23-v0.9.4.1.md) |
| v0.10.2 | proactive triggers — `ProactiveChecker` + budget + reason | [plan](docs/plans/2026-05-23-v0.10.2.md) |
| v0.10.3 | push sinks — Feishu / Telegram / Email / Inbox | [plan](docs/plans/2026-05-23-v0.10.3.md) |
| v0.8.3  | chat-ui code-block syntax highlighting + copy button | [plan](docs/plans/2026-05-23-v0.8.3.md) |
| v0.11.0 | `xiaoguai-eval` crate — regression + capability suites + graders + CLI | [plan](docs/plans/2026-05-23-v0.11.0.md) |
| v0.11.1 | audit-first console — Today view + `/v1/admin/today` endpoint | [plan](docs/plans/2026-05-23-v0.11.1.md) |
| v0.11.2 | eval pane — run suites + convert session to case | [plan](docs/plans/2026-05-23-v0.11.2.md) |
| v0.12.0 | `xiaoguai-runtime` + PG scheduler repos + operator wiring + webhook HTTP route | [plan](docs/plans/2026-05-24-v0.12.0.md) |
| v0.12.1 | natural-language job definition + per-run synthetic session | [plan](docs/plans/2026-05-24-v0.12.1.md) |
| v0.12.2 | file watcher RAG re-index wiring + Obsidian catalog entry | [plan](docs/plans/2026-05-24-v0.12.2.md) |

The full v0.9 → v0.12 master plan is at
[`docs/plans/2026-05-23-roadmap-v0.9-v0.12.md`](docs/plans/2026-05-23-roadmap-v0.9-v0.12.md).

## Compliance

Xiaoguai is built for self-hosted deployments that need to defend their
audit trail to a third party.

- **等保 2.0 Level 3 self-check (`三级`)** — control mapping at
  [`docs/compliance/dengbao-2.0-l3/`](docs/compliance/dengbao-2.0-l3/).
  Covers the mandatory items in GB/T 22239-2019; operators still run
  the formal graded assessment with an MPS-accredited assessor.
- **GDPR DPIA template** — pre-filled threat model and lawful-basis
  worksheet at
  [`docs/compliance/gdpr/dpia-template.md`](docs/compliance/gdpr/dpia-template.md).

Hard guarantees the platform enforces in code (not just docs):

- HMAC-chained `audit_log` rows for every tool call, scheduled run,
  and IM-routed message. Chain verification is exposed at
  `/v1/admin/audit/verify`.
- Single-owner access gate: an optional configured username/password
  (HTTP Basic) protects the API when exposed on a URL (DEC-033 — no
  OIDC/RBAC/multi-tenancy; each person runs their own instance).
- Per-user proactive-push budget with a mandatory `reason` field —
  sinks may refuse delivery if the reason is empty.

## Roadmap

**v1.0 — shipped.** Everything in the table above plus the full v0.1
→ v0.10.0 history. The repo is ready for first users.

**v1.1 — not yet queued.** The honest plan is *"wait for first-user
feedback, then prioritise."* The candidate backlog, per
[`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §5:

- Scoped API tokens for `/v1/admin/scheduler/webhooks/...` (today the
  single-owner HTTP Basic credential gates the whole admin surface).
- `CompositeExecutor` so the scheduler operator can dispatch by
  payload kind instead of the current hard-coded
  `RuntimeJobExecutor`.
- Admin-ui Scheduler pane (backend ships, UI doesn't).
- `RagClient` binary-file re-index path (text-only today).
- `notify-debouncer-full` for the file-watch source.
- First-party write-capable Obsidian connector (community server is
  read-only).
- Browser-walked screenshots + per-pane visual QA on chat-ui and
  admin-ui — every UI-affecting tag from v0.8.1 onward was tuned by
  reading, not eyeballing.
- Conversation fork, public-cloud LLM provider configs, the `/usage`
  endpoint, and multi-agent orchestration have since **shipped** (fork
  v1.1.2; Bedrock / Azure / Mistral / Groq / MiniMax providers in Wave 3;
  **agent teams** — see the [Agent teams](#agent-teams--for-complex-tasks-worth-several-perspectives)
  section above). Remaining roadmap items are in the roadmap §3 v1.0+ section.

## License

Licensed under the [Apache License 2.0](LICENSE).

Xiaoguai is open source: free to use, self-host, modify, embed, and
redistribute — including for commercial and production use — under the
permissive terms of the Apache License 2.0, which adds an explicit patent
grant on top of the usual attribution requirements.

The full text is in [`LICENSE`](LICENSE); attribution is in [`NOTICE`](NOTICE).

## Documentation

The full handbook source lives in [`docs/book/`](docs/book/). To build locally:

```bash
# Install mdbook and mdbook-mermaid first
cargo install mdbook mdbook-mermaid
# Then:
bash docs/book/test-build.sh
```

---

## Wave-3 features (v1.2.x / v1.3.x)

Wave 3 merged 33 feature branches into `main` in late May 2026. The
workspace now passes **1,191 tests / 0 failed / 92 ignored**. Three
Postgres bridges are still wired to return `503` in production until
v1.3 lands — see the honest status section below.

### What shipped

| Feature | One-liner |
|---|---|
| **Human-on-the-Loop policy (HotL)** | Risk-tiered approval gates; every agent action with `risk ≥ threshold` pauses for a human `APPROVE` / `REJECT` before proceeding. |
| **Outcome telemetry & attribution** | Every agent action is recorded with `session_id + tool + latency + cost + outcome`; the chain reader exposes `/v1/outcomes/chain/{session_id}` for audit consumers. |
| **Skill packs** | Declarative install: `POST /v1/skills/install {"slug":"incident-triage"}` records the pack row; 7 packs ship in-repo (`ar-collections`, `incident-triage`, `pr-review`, `hr-onboarding`, `rag-legal`, `rag-finance`, `rag-hr`) with `catalog/skill_packs.json` as the authoritative manifest. |
| **Active watchers (`xiaoguai-watch`)** | New crate; SQL-poll and HTTP-poll wakeups that feed the scheduler, enabling reactive "check every N seconds, fire when condition changes" loops without a dedicated worker process. |
| **Anomaly detection (`xiaoguai-anomaly`)** | Z-score and EWMA detectors over any numeric time series; ships as a standalone crate consumable by scheduler jobs and HotL policy rules. |
| **New IM adapters** | Discord (Ed25519 sig verification), Telegram (Bot API long-poll), Mattermost (WebSocket), Slack (HMAC sig verification) — four new `xiaoguai-im-*` crates alongside the existing Feishu / DingTalk / WeCom adapters. |
| **Cloud LLM v2** | `ProviderKind` gains `Bedrock` (SigV4), `AzureOpenAi`, `Mistral`, and `Groq` — all behind the existing `LlmBackend` trait; circuit breakers and cost-quota defence carry over automatically. |
| **Observability** | New `xiaoguai-observability` crate; opt-in Prometheus scrape endpoint (`/metrics`) and OTLP trace export; zero telemetry by default (ADR-0013 preserved). |

### Quickstart — with telemetry

The base `docker-compose.yml` brings up a single `xiaoguai-core` service
(embedded SQLite). Layer `deploy/docker-compose.observability.yml` on top for
the optional Prometheus / Grafana / OTel-collector stack:

```yaml
# deploy/docker-compose.wave3.yml  (create or adapt from the snippet below)
services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.101.0
    command: ["--config=/etc/otel.yaml"]
    volumes: ["./observability/otel.yaml:/etc/otel.yaml:ro"]

  prometheus:
    image: prom/prometheus:v2.52.0
    volumes: ["./observability/prometheus.yml:/etc/prometheus/prometheus.yml:ro"]
    ports: ["9090:9090"]

  grafana:
    image: grafana/grafana:10.4.2
    environment: {GF_SECURITY_ADMIN_PASSWORD: xiaoguai}
    volumes:
      - "./observability/grafana/provisioning:/etc/grafana/provisioning:ro"
      - "./observability/grafana/dashboards:/var/lib/grafana/dashboards:ro"
    ports: ["3000:3000"]
```

```bash
# Bring up everything
docker compose -f deploy/docker-compose.yml \
               -f deploy/docker-compose.wave3.yml up --build

# Apply wave-3 migrations (run once, idempotent after)
docker compose exec xiaoguai-core xiaoguai migrate run
# Migrations that land new in wave 3:
#   0011_hotl_policies.sql
#   0012_outcomes.sql
#   0015_skill_packs.sql

# Grafana → http://localhost:3000  (admin / xiaoguai)
# Prometheus → http://localhost:9090
```

> If you configured the HTTP Basic gate (`auth.username` / `auth.password`),
> add `-u "$USER:$PASS"` to admin curls. There is no bearer token.

The binary is `xiaoguai` — not `xg`. The wave-3 CLI subcommands
(`xiaoguai skills …`, `xiaoguai outcomes …`, `xiaoguai hotl …`) are wired;
the admin-ui and REST API cover the same surface.

### Documentation index

#### Operator guides (mdbook)

| Chapter | Path |
|---|---|
| Active wakeup / watchers | `docs/book/src/operator/` — day2.md §"Reactive watcher" |
| HotL policy | pending — see `docs/plans/2026-05-24-v1.1.3.md` |
| Outcome telemetry | pending — see `docs/plans/2026-05-24-v1.1.4.md` |
| Skill packs | pending — see `docs/book/src/skills/overview.md` |

Build the handbook locally:

```bash
cargo install mdbook mdbook-mermaid
bash docs/book/test-build.sh
```

#### Runbooks

| Runbook | File |
|---|---|
| Observability (Prometheus + OTLP) | [`docs/runbooks/observability.md`](docs/runbooks/observability.md) |
| Operator day-2 | [`docs/runbooks/operator.md`](docs/runbooks/operator.md) |
| systemd hardening | [`docs/runbooks/systemd-hardening.md`](docs/runbooks/systemd-hardening.md) |
| Disaster recovery | [`docs/runbooks/disaster-recovery-wave3.md`](docs/runbooks/disaster-recovery-wave3.md) |
| Release signing | [`docs/runbooks/release-signing.md`](docs/runbooks/release-signing.md) |

#### Architecture

| Document | Path |
|---|---|
| ADR-0013 Zero-default telemetry | [`docs/architecture/adr/0013-zero-default-telemetry.md`](docs/architecture/adr/0013-zero-default-telemetry.md) |
| ADR-0014 Multimodal MCP architecture | [`docs/architecture/adr/0014-multimodal-mcp-architecture.md`](docs/architecture/adr/0014-multimodal-mcp-architecture.md) |
| ADR-0009 Cost quota + token-bomb defence | [`docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md`](docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md) |
| ADR-0008 Tool-result provenance | [`docs/architecture/adr/0008-tool-result-provenance.md`](docs/architecture/adr/0008-tool-result-provenance.md) |
| Multi-agent peer topology | [`docs/architecture/multi-agent-peer.md`](docs/architecture/multi-agent-peer.md) |
| System design (v0.1 origin) | [`docs/architecture/2026-05-21-design.md`](docs/architecture/2026-05-21-design.md) |

#### Compliance

Existing mappings cover 等保 2.0 L3 and GDPR (see the Compliance
section above). SOC 2, HIPAA, PCI-DSS, ISO 27001, and EU AI Act
control mappings are on the roadmap — not yet written.

#### API

The REST API surface (15+ endpoints) is described in
[`docs/book/src/api/rest.md`](docs/book/src/api/rest.md) and the MCP
toolbox in [`docs/book/src/api/mcp.md`](docs/book/src/api/mcp.md).
An OpenAPI spec and Bruno collection are planned for v1.3; the routes
are all typed in `crates/xiaoguai-api/src/routes/`.

#### Skill packs

| Resource | Path |
|---|---|
| Pack catalog (machine-readable) | [`catalog/skill_packs.json`](catalog/skill_packs.json) |
| AR Collections | [`packs/ar-collections/README.md`](packs/ar-collections/README.md) |
| Incident Triage | `packs/incident-triage/` |
| PR Review | `packs/pr-review/` |
| HR Onboarding | `packs/hr-onboarding/` |
| RAG — Legal | `packs/rag-legal/` |
| RAG — Finance | `packs/rag-finance/` |
| RAG — HR | `packs/rag-hr/` |

#### Recipes & examples

| Recipe | Path |
|---|---|
| Multi-agent peer pair | [`examples/multi-agent/peer-pair/README.md`](examples/multi-agent/peer-pair/README.md) |
| Grafana dashboard pack | [`observability/grafana/README.md`](observability/grafana/README.md) |

#### SDKs

| SDK | Status |
|---|---|
| Python (`xiaoguai` PyPI package) | Shipped — wraps the binary via subprocess; see `python/xiaoguai/` |
| TypeScript | Planned (v1.3) |
| Go | Planned (v1.4) |
| Java | Under consideration |

### Honest status — what is NOT production-ready yet

The HotL, outcomes, and skill-pack surfaces are now backed by real
SQLite-backed stores (the single-user pivot wired them; they no longer return
`503`). Remaining gaps:

- The **pack runtime loader** is not yet wired: installing a pack via the API
  records the row in the `skill_packs` table but does not yet activate the
  pack's prompt overlays or tool registrations at runtime.
- Audit endpoints (`/v1/admin/audit*`, `/v1/audit/exports`) return `503` until
  an audit HMAC signing key is configured (`audit.hmac_key`).
- Air-gapped memory/recall is pending an Ollama-backed embedder (today the
  only real embedder is OpenAI-backed).

Everything else — observability, IM adapters, cloud LLM providers, anomaly /
watcher crates — is fully wired and tested.

---

*Built in Shanghai. 2026.*
