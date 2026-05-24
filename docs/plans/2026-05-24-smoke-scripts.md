# Smoke scripts — 2026-05-24

Three shell smoke scripts land under `scripts/smoke/` to make the post-v1
release path verifiable from a clean machine. Each script is `set -euo
pipefail`, self-contained, and tears down the compose stack on exit (success
or failure) so they're safe to chain in CI without leaking volumes.

1. **`compose-up.sh`** — the minimum viable check: bring up
   `deploy/docker-compose.yml`, poll `http://localhost:7600/healthz` until
   it returns `ok` (60 s budget), tear down. Intended to run on every PR
   that touches `deploy/` or `crates/xiaoguai-core/`, and nightly on `main`
   as a deploy canary.
2. **`end-to-end.sh`** — extends the compose probe with an actual REST +
   SSE round-trip: it `POST`s to `/v1/sessions` (relying on the documented
   MockBackend auto-fallback when `llm_providers` is empty, hence
   `model: "mock"`), then `POST`s a message and asserts the SSE response
   contains at least one `data:` prefixed line. Intended to run nightly on
   `main` as the highest-fidelity-without-external-dependency signal that
   the wire format hasn't regressed.
3. **`real-llm.sh`** — gated on `ANTHROPIC_API_KEY`; skips with exit 0 if
   unset (so CI can invoke it unconditionally). When the gate is open it
   also requires `OPENAI_API_KEY` + `OPENAI_BASE_URL`, registers an
   `openai_compat` provider via `docker compose exec xiaoguai-core
   xiaoguai provider register …`, restarts core to pick the row up
   (provider auto-reload is v1.1), then runs one prompt and asserts the
   assistant reply is non-empty. We tunnel through the OpenAI-compat
   backend because the dedicated Anthropic backend hasn't shipped yet (per
   HANDOFF §1 C1 deferral); the `ANTHROPIC_API_KEY` env var is purely the
   "I opted in to a real-LLM smoke" gate. Intended to run on demand before
   releases — not in CI, since secret injection + LLM cost belong to the
   operator.

The compose file's published port is now `7600:7600` (was `8080:8080`) to
match `Settings::default().server.port` in `xiaoguai-config`, which is what
the smoke scripts assume. The Helm chart's `8080` is independent and
untouched — that's container-internal and the chart sets its own service
port.

**These scripts were not executed in the agent environment (no Docker
available).** Syntax is verified with `bash -n scripts/smoke/*.sh`; first
real execution will be by the user (locally) or by CI on the next push that
opts these in.

## Deferrals

- No `--build` cache warming. First compose-up build of `xiaoguai-core`
  takes ~2 min; CI should plumb Docker layer caching separately.
- No timeout on `docker compose up -d --build` itself. The build step
  blocks until it completes; on a slow box this can dominate the
  perceived runtime.
- `real-llm.sh` does not yet measure latency or token cost; it only
  asserts the reply is non-empty. Add `time` + log scraping when a real
  release rehearsal pipeline lands.
- No JSON schema validation of the SSE payload — `end-to-end.sh` only
  greps for `^data:` because that's the contract the scripts care about
  (well-formed SSE). Deeper validation belongs in the existing Rust
  integration tests, not in shell.
