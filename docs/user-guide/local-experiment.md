# Local experiment guide — drive xiaoguai with a real LLM key

This page walks you from "fresh checkout" to "11 README screenshots
captured" in about 30 minutes. Assumes Docker Desktop + pnpm + a real
OpenAI-compatible API key (OpenAI, Codex relay, DeepSeek, etc.).

## TL;DR

```bash
cp .env.local.example .env.local
$EDITOR .env.local                # set OPENAI_API_KEY at minimum
bash scripts/local-experiment.sh  # bring up + register provider + install MCPs
# in two new terminals:
cd frontend && pnpm -F chat-ui dev    # → http://localhost:5173
cd frontend && pnpm -F admin-ui dev   # → http://localhost:5174
```

That's it. Below is the longer story plus the exact prompts to use
when capturing each README screenshot.

## 1. Configure

Copy the example env and edit it:

```bash
cp .env.local.example .env.local
$EDITOR .env.local
```

You **must** set `OPENAI_API_KEY`. Everything else has sane defaults:

| Var | Default | When to override |
|---|---|---|
| `OPENAI_API_KEY` | — | required |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | Chinese reseller / Codex relay / DeepSeek |
| `OPENAI_MODEL` | `gpt-4o-mini` | bigger model needed |
| `PROVIDER_NAME` | `codex` | rename if multiple providers |
| `MARKETPLACE_INSTALL` | `filesystem,fetch,sqlite` | which MCP servers to pre-install |
| `SKIP_FRONTEND_INSTALL` | (unset) | set to `1` if `node_modules` is warm |

## 2. Boot

```bash
bash scripts/local-experiment.sh
```

This is idempotent — re-running is safe. The script:

1. Validates env + Docker.
2. `docker-compose up -d` the PG + Valkey + xiaoguai-core stack.
3. Polls `/healthz` until ready.
4. Registers your provider in PG.
5. Restarts xiaoguai-core so the LlmRouter picks it up.
6. Installs the configured MCP servers via the marketplace endpoint.
7. `pnpm install` the frontend workspaces.
8. Prints the next-step punch list.

Tear-down: `bash scripts/local-experiment.sh down`.

If `/healthz` never returns OK, inspect:

```bash
docker compose -f deploy/docker-compose.yml logs xiaoguai-core | tail -50
```

Most common cause: port 7600 already in use locally, or Docker can't
pull `postgres:16` / `valkey:8`.

## 3. Start the dev servers

Two new terminals:

```bash
# Terminal A
cd frontend
pnpm -F chat-ui dev   # http://localhost:5173

# Terminal B
cd frontend
pnpm -F admin-ui dev  # http://localhost:5174
```

The admin-ui defaults to the **Today** pane (v0.11.1's audit-first
landing). That's intentional — it's the visual signature of this
platform vs every chat-first competitor.

## 4. Walk the screenshot punch list

Eleven shots. Each maps to a section in the top-level README. The
manifest with exact filenames lives in
[`../screenshots/PLACEHOLDER.md`](../screenshots/PLACEHOLDER.md);
the prompts below are what to type/click to *reach* the right state
before pressing Shift-Cmd-4.

> **Tip:** turn off your system menu bar auto-hide and chrome
> bookmarks bar before capturing, so frames stay clean.

### 01. `01-chat-light.png` — chat-ui light, settled markdown

Light theme. Send:

> Explain xiaoguai's audit chain in three short paragraphs.
> Use a bullet list for the integrity properties.

Wait until the streaming dots animation has stopped (~3-4s on
gpt-4o-mini). Capture full window.

### 02. `02-chat-dark.png` — chat-ui dark, syntax highlight + copy

Toggle dark mode (header → sun/moon icon). Send:

> Write a small Rust function that consumes a tokio mpsc channel
> and forwards each item to a sqlx::PgPool insert. Add comments.

Wait for the reply. Hover the resulting code block — the **Copy
button** appears top-right (v0.8.3). Capture frame includes the
button + the highlighted Rust tokens.

### 03. `03-today-pane.png` — admin-ui Today

Default landing on `localhost:5174`. Before capturing, seed three
items so the timeline isn't empty:

```bash
# A. one chat run — already done if you did shot 01.

# B. one IM webhook (simulated):
curl -sX POST http://localhost:7600/v1/admin/scheduler/webhooks/local-demo \
  -H 'content-type: application/json' \
  -d '{"source":"demo","detail":"manual fire"}'

# C. one scheduled job + immediate fire:
curl -sX POST http://localhost:7600/v1/admin/scheduler/jobs \
  -H 'content-type: application/json' \
  -d '{"id":"demo-cron","tenant_id":null,"name":"demo","description":null,
       "trigger":{"type":"interval","secs":60},
       "payload":{"prompt":"summarize this minute"},
       "retry_policy":{"max_attempts":1,"initial_backoff_secs":0,"max_backoff_secs":0,"multiplier":1.0},
       "sinks":[],"enabled":true,"next_fire_at":null,"last_fire_at":null,
       "created_at":"2026-05-24T00:00:00Z","updated_at":"2026-05-24T00:00:00Z"}'
curl -sX POST http://localhost:7600/v1/admin/scheduler/jobs/demo-cron/fire-now
```

Refresh the Today pane (or wait for the 30s auto-refresh). Now there
are 3 rows of different kinds. Capture full window.

### 04. `04-eval-pane-run.png` — eval suite running

Navigate to `/eval` in admin-ui. Pick the bundled `regression` suite
(from `crates/xiaoguai-eval/examples/eval/regression/`). Click "Run
suite". Capture once the first case row turns green (pass) but at
least one row is still pending.

If the suite isn't visible, you may need to point
`[eval].suites_dir` at the right path in `config.yaml` — by default
it expects `./eval-suites/`. Quick fix:

```bash
docker compose -f deploy/docker-compose.yml exec xiaoguai-core \
  ls /app/crates/xiaoguai-eval/examples/eval/regression/
```

If the directory isn't accessible from the container, mount it (or
just capture by curling `POST /v1/admin/eval/run` and screenshotting
the JSON in your terminal).

### 05. `05-marketplace-install.png` — install confirm modal

Navigate to `/mcp/marketplace`. Hover the `filesystem` entry's
**Install** button. Modal pops with the config diff. Capture mid-hover.

### 06. `06-mcp-servers-list.png` — installed servers list

After running `local-experiment.sh` you should have 3 installed
(filesystem + fetch + sqlite). Navigate to `/mcp/servers`. Each row
shows transport (stdio/SSE/HTTP) + status dot + tool count. Full
window.

### 07. `07-audit-chain.png` — HMAC verify

Hit (or click in admin-ui):

```bash
curl -s "http://localhost:7600/v1/admin/audit/verify?tenant_id=system" \
  | jq .
```

Capture either the admin-ui rendering or the JSON terminal output
showing `signature_ok: true` (or `verified_count: N`).

### 08. `08-scheduler-jobs.png` — scheduler pane

Navigate to `/scheduler` (v0.12.x.1). Jobs tab shows your `demo-cron`
row from shot 03. Click "Create from description" tab, type:

> Every 10 minutes, fetch GitHub trending repos and summarize the top three.

Click "Compile" → JSON preview appears → don't save unless you want
to. Capture either tab. Full window.

## 5. Quick smoke without the UI

If you just want to verify the LLM round-trip works at all:

```bash
docker compose -f deploy/docker-compose.yml exec xiaoguai-core \
  xiaoguai chat \
    --model "${OPENAI_MODEL:-gpt-4o-mini}" \
    --user "Reply with: smoke-ok-from-xiaoguai"
```

You should see the model reply with a string containing
`smoke-ok-from-xiaoguai`. If you don't, check:

1. `xiaoguai provider list` (inside the container) — is the provider
   row there?
2. The container env actually has `OPENAI_API_KEY` set? (The
   `local-experiment.sh` script passes it via `-e` during the
   `provider register` step, which writes it to the container's
   long-lived env. If you skipped step 5 and restarted manually,
   re-run `local-experiment.sh up`.)

## 6. After capturing

Drop the PNGs into `docs/screenshots/` with the filenames in the
manifest. Then:

```bash
git add docs/screenshots/*.png
git commit -m "docs(v1.0.2): wire screenshot captures into README"
git push origin main
```

The README already references the screenshot filenames (in §
"5-minute quickstart" and § "What makes it different"), so they'll
render automatically once the files land in the right directory.

## 7. Switch providers mid-session

To verify the v0.6.4 per-tenant LlmRouter actually routes, register a
second provider (e.g. MiniMax) and switch in the admin-ui Providers
pane:

```bash
docker compose -f deploy/docker-compose.yml exec -T \
  -e MINIMAX_API_KEY=sk-... \
  xiaoguai-core \
  xiaoguai provider register \
    --name minimax \
    --kind openai_compat \
    --base-url https://api.minimaxi.com/v1 \
    --api-key-env MINIMAX_API_KEY \
    --models abab6.5s-chat

docker compose -f deploy/docker-compose.yml restart xiaoguai-core
```

Now admin-ui Providers shows both. Send messages from chat-ui and
watch them route by the tenant defaults (or by per-request `model:`
override).

## 8. Tear down

```bash
bash scripts/local-experiment.sh down
```

Deletes the PG volume + the Valkey volume. The MCP server installs
disappear with the PG volume; re-running `local-experiment.sh up`
restores them via `MARKETPLACE_INSTALL`.
