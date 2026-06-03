# Service objectives — latency & errors

This runbook is operator guidance for keeping a single-owner Xiaoguai instance
healthy on the two signals that matter to one user reaching their own box:
**latency** and **errors**.

> **What changed (DEC-033, single-user SQLite pivot).** Xiaoguai is now a
> single self-contained binary with an embedded SQLite database. The formal
> SLO-contract feature was removed: there is no `xiaoguai-observability::slo`
> module parsing a YAML contract, no `xiaoguai_slo_burn_rate` metric, and no
> Alertmanager burn-rate page chain. Per-tenant traffic/saturation SLOs are
> gone with multi-tenancy. Observability (`/metrics` + OTLP) is opt-in behind
> the `observability` cargo feature and **off by default** — if you have not
> built with that feature, none of the Prometheus queries below apply and you
> rely on logs + `/healthz` instead.

The objectives below are **targets, not a parsed contract**. Treat them as
"when the instance feels slow or erroring, here's what to check".

---

## Objectives (targets)

| Signal | Surface | Target |
|---|---|---|
| Latency | `/v1/chat/*` p95 | ≤ 5 s rolling 1 h |
| Latency | first token on `/v1/sessions/*/messages` p95 | ≤ 2 s rolling 1 h |
| Errors | `/v1/chat/*` non-2xx rate | < 1% rolling 1 h |
| Errors | `/v1/sessions/*/messages` non-2xx rate | < 1% rolling 1 h |

A single-process SQLite instance has **no connection pool** — storage access
is in-process against the embedded database file, so there is no
pool-starvation or `pg_locks` failure mode to chase. Storage latency, when it
matters, is dominated by disk I/O on the volume holding `data.db`.

---

## When latency degrades

**Symptom.** `/v1/chat/*` p95 climbs past ~5 s, or first token on streaming
endpoints past ~2 s.

**Triage.**

```bash
# Latest slow requests (always available — logs, not metrics):
journalctl -u xiaoguai --since "10 min ago" | grep -E "request_duration|slow"

# If you built with the observability feature, per-provider LLM latency:
curl -s http://localhost:7600/metrics | grep xiaoguai_llm_call_duration_seconds
```

**Likely root causes (in order of likelihood).**

1. **LLM provider degradation** — check the provider status page (Anthropic /
   OpenAI / Ollama). If only one provider is slow, switch
   `agent.preferred_provider` in `config.yaml` and restart.
2. **Cold Ollama model** — a freshly-restarted local model is slow on the
   first request (~30 s cold start). Warm it up by issuing one throwaway
   request to the model (e.g. `ollama run <model> ''` on the Ollama host, or
   a single `xiaoguai chat --prompt hi`).
3. **Disk I/O pressure on `data.db`** — if the volume holding the SQLite file
   is contended (slow disk, other heavy writers), storage reads/writes slow
   down. Check disk utilisation; move `data.db` to faster storage if needed.
4. **Recent upgrade regressed a hot path** — if you upgraded recently, roll
   back to the previous package/container and restart; the data file is
   compatible across the additive migrations.
5. **Streaming proxy buffering** — if a reverse proxy fronts the instance,
   SSE buffering must be off (e.g. nginx `proxy_buffering off`) or first-token
   latency inflates.
6. **Context windows growing** — if compaction is not firing
   (`agent.compaction.threshold_tokens` too high) histories grow and LLM calls
   slow. Tune the threshold; see `history-compaction.md` if present.

**Mitigation.**
- Transient provider regression: wait it out; it usually self-resolves.
- Cold Ollama: warm up as above.
- Disk pressure: relocate `data.db` or reduce competing I/O.
- Upgrade regression: reinstall the previous version and restart.

---

## When errors climb

**Symptom.** Non-2xx rate on `/v1/chat/*` or `/v1/sessions/*/messages` rises
above ~1%.

**Triage.**

```bash
# Recent error logs:
journalctl -u xiaoguai --since "10 min ago" | grep -E "ERROR|panic|5[0-9][0-9]"

# If observability is built in, status-code breakdown:
curl -s http://localhost:7600/metrics | grep 'xiaoguai_http_request_duration_seconds_count'
```

**Likely root causes.**

1. **Bad upgrade** — correlate the error onset with a recent
   package/container upgrade. Roll back and restart.
2. **Storage error on `data.db`** — disk full, file permissions changed, or a
   locked database (SQLite returns "database is locked" under pathological
   contention). Check free disk space and the data file's ownership; ensure
   only one `xiaoguai serve` process holds the file.
3. **MCP server / tool failure** — a tool the agent depends on is erroring.
   Identify the implicated tool from logs; disable it via
   `mcp.disabled_tools` in `config.yaml` if it is crash-looping.
4. **Upstream LLM provider 5xx** — provider returning errors. Switch
   `agent.preferred_provider` to a healthy backend.

**Mitigation.**
- Bad upgrade: reinstall the previous version and restart. Stops the bleeding.
- Disk full: free space on the volume holding `data.db`; SQLite cannot write
  to a full disk.
- MCP failure: disable the offending tool.
- Provider 5xx: switch provider.

---

## Tuning the targets

These are operator targets, not a code-enforced contract — change them by
editing this runbook. There is no schema to keep in sync and no PR-time
`serde_yaml` validation, because the SLO-contract module was removed with the
SQLite pivot. If a target is consistently unrealistic for your hardware
(e.g. a small box on slow disk genuinely can't hit chat p95 ≤ 5 s), adjust the
number here and note why.

---

## References

- DEC-033 — single-user SQLite pivot (`docs/plans/2026-06-02-sqlite-single-user-pivot.md`)
- Google SRE Workbook chapter 5 — Alerting on SLOs (background on latency/error objectives)
