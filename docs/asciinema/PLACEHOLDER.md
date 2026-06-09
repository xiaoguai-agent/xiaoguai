# Asciinema cast manifest

**Owner:** USER. **Record with:** `asciinema rec -i 2 -t '<title>'
docs/asciinema/<file>.cast` (the `-i 2` clamps idle gaps to 2 s so
playback stays watchable). Target length 60-90 s per cast.

After recording, `asciinema upload docs/asciinema/<file>.cast` if a
public link is wanted; the resulting URL goes into the README via
`[![asciicast](https://asciinema.org/a/<id>.svg)](https://asciinema.org/a/<id>)`.

| # | File | Topic | Beat-by-beat |
|---|---|---|---|
| 01 | `01-chat-end-to-end.cast` | `xiaoguai chat` end-to-end | (1) `docker compose -f deploy/docker-compose.yml up -d` (skip if already up). (2) `xiaoguai provider register --name deepseek --kind openai_compat --endpoint https://api.deepseek.com/v1 --api-key-env DEEPSEEK_API_KEY --models deepseek-chat`. (3) `xiaoguai mcp register --name fs --transport stdio --command npx --args '-y,@modelcontextprotocol/server-filesystem,/tmp'`. (4) `xiaoguai chat --model deepseek-chat --tools fs --prompt 'list the files in /tmp and summarise each'`. Show the streamed reply with tool calls interleaved on stderr. (5) `xiaoguai remote --server http://localhost:8080 chat --user-id usr_demo --tenant-id ten_demo --model mock --prompt 'hello!'` to demonstrate the remote-mode SSE path. |
| 02 | `02-eval-run.cast` | `xiaoguai eval run` | (1) `xiaoguai eval list-suites` — show the bundled `regression` + `capability` suites. (2) `xiaoguai eval run --suite regression --report /tmp/regression.json` — the run prints pass/fail per case live; tail should show the summary line "N passed / M failed" and the JSON report path. (3) `jq '.cases[0]' /tmp/regression.json` to show one case's recorded grader output. (4) Optional: `xiaoguai eval convert-session --session-id <id> --suite regression` to demo turning a prod session into a new regression case (v0.11.2). |
| 03 | `03-provider-register.cast` | `xiaoguai provider register` | (1) `xiaoguai provider list` — should be empty or only `mock`. (2) `xiaoguai provider register --name ollama --kind ollama --endpoint http://localhost:11434 --models qwen2.5-coder`. (3) `xiaoguai provider register --name deepseek --kind openai_compat --endpoint https://api.deepseek.com/v1 --api-key-env DEEPSEEK_API_KEY --models deepseek-chat,deepseek-reasoner`. (4) `xiaoguai provider list` — show both registered; note the LlmRouter will auto-select on next request. (5) `xiaoguai chat --model deepseek-chat --prompt 'one-liner: what is the Model Context Protocol?'` to prove the registration is live. |
| 04 | `04-hotl-approval.cast` | HotL policy inspect + approval | **Script:** `demo-hotl-approval.sh` (~90 s). (1) List HotL policies for the demo tenant. (2) Create a guard-rail: max 100 `llm_call` / hour → escalate to `ops@example.com`. (3) Confirm policy is listed. (4) Simulate enforcer returning `Escalate` decision. (5) Show the admin approval queue. (6) Operator acks the escalation — agent resumes. (7) Verify the ack in the audit log. (8) Delete the demo policy. |
| 05 | `05-outcomes-query.cast` | Outcomes query + attribution | **Script:** `demo-outcomes-query.sh` (~75 s). (1) Seed 7 days of sample outcome rows (revenue, hours_saved, deals_closed). (2) 7-day summary by kind via `GET /v1/outcomes/summary`. (3) Drill into session `sess_001`. (4) Walk the multi-hop attribution chain. (5) Export timeseries to CSV and print a formatted preview. |
| 06 | `06-pack-install.cast` | Skill pack browse + install | **Script:** `demo-pack-install.sh` (~60 s). (1) Browse catalog via `GET /v1/skills/catalog`. (2) Inspect `pr-review` pack details (knobs, requires). (3) List currently installed packs. (4) Install `pr-review` with custom knob config. (5) Verify pack appears in installed list. (6) Run pack diagnostic — notes that activation is pending v1.3 hot-reload; pack metadata is recorded. (7) Show `packs/pr-review/pack.yaml` excerpt. |

## Wave-3 demo scripts (v1.2.x operator workflows)

The three shell scripts above are runnable against a freshly started
`xg serve --config config/dev.toml`.  They create all sample data
inline so no external fixture loading is needed.

### Quick-record all three

```bash
# Terminal 1 — server
xg serve --config config/dev.toml

# Terminal 2 — record each demo
asciinema rec -i 2 -t 'xiaoguai: HotL approval workflow' \
  docs/asciinema/04-hotl-approval.cast \
  --command='bash docs/asciinema/demo-hotl-approval.sh'

asciinema rec -i 2 -t 'xiaoguai: outcomes query & attribution' \
  docs/asciinema/05-outcomes-query.cast \
  --command='bash docs/asciinema/demo-outcomes-query.sh'

asciinema rec -i 2 -t 'xiaoguai: skill pack install' \
  docs/asciinema/06-pack-install.cast \
  --command='bash docs/asciinema/demo-pack-install.sh'
```

### Environment overrides

| Variable | Default | Purpose |
|---|---|---|
| `XG_API` | `http://localhost:8080` | Server base URL |
| `XG_TENANT` | `ten_demo` | Demo tenant ID |
| `CLEANUP` | `0` | Set `1` in pack-install to auto-uninstall at end |

## Once recorded

1. `git add docs/asciinema/*.cast`.
2. (Optional) `asciinema upload docs/asciinema/*.cast`; collect the
   resulting URLs.
3. Open `README.md` and add a `## Demos` section under § "5-minute
   quickstart" with one embed per cast.
4. Commit with `docs(v1.0.3): wire asciinema demos into README`.

## Why we ship the manifest now

The repo is feature-complete but the dev-server pass owed since
v0.8.1 (see `docs/HANDOFF-2026-05-24.md` §1) has to happen before any
real recording — recording over un-eyeballed UI bakes in whatever
visual regressions exist. The manifest crystallises the *what* so
the *when* is a one-sitting job once the user has a clean dev stack
in front of them.
