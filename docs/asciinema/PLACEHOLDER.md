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
| 03 | `03-provider-register.cast` | `xiaoguai provider register` | (1) `xiaoguai provider list` — should be empty or only `mock`. (2) `xiaoguai provider register --name ollama --kind ollama --endpoint http://localhost:11434 --models qwen2.5-coder`. (3) `xiaoguai provider register --name deepseek --kind openai_compat --endpoint https://api.deepseek.com/v1 --api-key-env DEEPSEEK_API_KEY --models deepseek-chat,deepseek-reasoner`. (4) `xiaoguai provider list` — show both registered; note the LlmRouter will auto-select on next request. (5) `xiaoguai chat --model deepseek-chat --prompt 'one-liner: what is BUSL-1.1?'` to prove the registration is live. |

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
