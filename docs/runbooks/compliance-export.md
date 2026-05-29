# Compliance export from the audit chain (T5)

Operator guide for the `xiaoguai audit export` CLI and
`POST /v1/audit/exports` HTTP endpoint. Both produce SOC2 / GDPR / HIPAA
report bundles over a time window. Every bundle carries a chain-verification
proof in the header — auditors don't have to take our word that the chain
is intact.

## Quick reference

```bash
# CLI — produces a JSON bundle.
xiaoguai audit export \
  --api-base http://localhost:8080 \
  --tenant-id tenant-acme \
  --framework soc2 \
  --from 2026-01-01T00:00:00Z \
  --to   2026-04-01T00:00:00Z \
  --output ./acme-q1-2026-soc2.json \
  --format json

# CSV is the auditor-friendly projection of the same data.
xiaoguai audit export \
  --tenant-id tenant-acme --framework gdpr \
  --from 2026-01-01T00:00:00Z --to 2026-04-01T00:00:00Z \
  --output ./acme-q1-2026-gdpr.csv --format csv
```

```bash
# HTTP — same shape, returns the rendered bundle inline.
curl -X POST http://localhost:8080/v1/audit/exports \
  -H 'Content-Type: application/json' \
  -d '{
    "tenant_id":"tenant-acme",
    "framework":"hipaa",
    "format":"json",
    "from":"2026-01-01T00:00:00Z",
    "to":"2026-04-01T00:00:00Z"
  }' > acme-q1-2026-hipaa.json
```

## What the bundle contains

```json
{
  "header": {
    "framework": "soc2-cc72",
    "framework_label": "SOC2 CC7.2 (System Monitoring)",
    "tenant_id": "tenant-acme",
    "window": { "from": "...", "to": "..." },
    "generated_at": "2026-05-29T...",
    "chain_proof": {
      "first_id": 12345,
      "last_id": 67890,
      "count": 4218,
      "start_prev_hmac_hex": "...",
      "end_hmac_hex": "..."
    }
  },
  "rows": [
    { "id": 12345, "ts": "...", "actor": "...", "action": "...",
      "resource": "...", "details_summary": "..." }
  ]
}
```

The `chain_proof` is the load-bearing field. An auditor with access to a
read-only Postgres replica can re-walk the rows offline using the canonical
encoding from `crates/xiaoguai-audit/src/chain.rs` and confirm the
`end_hmac_hex` matches the recomputed terminal HMAC. Any tampering inside
the window would have made the export refuse to render (see "chain
broken" below).

## Framework templates

Each framework is a static `match` arm over `action` strings in
`crates/xiaoguai-audit/src/export.rs`. To add an action, edit the
`*_keeps()` helper and this runbook section together — no runtime
template DSL exists by design.

### SOC2 CC7.2 — System Monitoring

What the standard expects: evidence that the system detects and responds
to security events. Reviewer wants to see authentication attempts, access
to sensitive operations, policy denials, cost incidents, and confirmations
that audit logs themselves are reviewed.

Actions kept:
- `session.create`, `session.cancel` — agent session lifecycle.
- `tool.invoke`, `tool.deny` — every tool call (including denied ones).
- `auth.login`, `auth.failure` — authentication outcomes.
- `policy.deny`, `hotl.escalate` — HotL boundary enforcement.
- `audit.verify` — periodic chain-integrity checks (evidence of monitoring
  the monitoring).
- `cost.charge` — financial events that the SOC2 reviewer treats as
  security-relevant.

Gaps (call out in your evidence package):
- We record `audit.verify` events but not who reviewed the audit chain.
  Reviewer evidence is currently the operator's responsibility.
- We record `policy.deny` but not the operator response time to the
  escalation.

### GDPR Art. 30 — Records of Processing Activities

What the standard expects: a record of personal-data processing
operations, including the purpose, categories of data subjects and data,
and the retention period.

Actions kept:
- `memory.create`, `memory.update`, `memory.delete`, `memory.recall` —
  every personal-data flow through the long-term memory store.
- `session.create`, `session.delete` — conversation lifecycle (subject
  to the lawful basis declared in your Art. 6 record).
- `data.export`, `data.purge` — DSR (data subject request) fulfillment.
- `consent.grant`, `consent.revoke` — consent management events.

Gaps:
- The retention period is configured per tenant but not embedded in each
  audit row. Surface it from the tenant config when you assemble the
  evidence pack.
- Cross-border transfers (Art. 44) are not modelled in the current action
  set. If your deployment crosses borders, add a `data.transfer` event
  type in your handler code and extend `gdpr_art30_keeps()`.

### HIPAA §164.312 — Technical Safeguards

What the standard expects: evidence of access control (§164.312(a)), audit
controls (§164.312(b)), and integrity (§164.312(c)).

Actions kept:
- `auth.login`, `auth.failure` — access control events.
- `session.create` — established sessions.
- `tool.invoke` **filtered to `resource` starting with `phi:`** — only
  PHI-tagged tool calls are reported. Non-PHI traffic is dropped at the
  template layer.
- `audit.verify` — audit-control evidence.
- `policy.deny` — access-control enforcement.

Gaps:
- Resource tagging (`phi:patient/42`) is the responsibility of the tool
  itself. Forgotten tags = invisible to HIPAA export. We recommend a
  pre-commit hook that flags new MCP tools touching PHI-shaped data.
- Encryption status (§164.312(a)(2)(iv)) is not in the audit chain — it's
  enforced upstream at the storage layer (`xiaoguai-storage` uses TLS to
  Postgres; PG itself uses transparent-data-encryption per operator
  config).

## Sample auditor question mappings

| Question | Command |
|---|---|
| "Show me all access to PHI in Q1 2026." | `xiaoguai audit export --framework hipaa --tenant-id <id> --from 2026-01-01T00:00:00Z --to 2026-04-01T00:00:00Z --output q1-hipaa.csv --format csv` |
| "Prove the audit log is reviewed monthly." | Search the SOC2 bundle for `action=audit.verify` rows — one per cron tick. |
| "What personal data did agent X touch last week?" | `xiaoguai audit export --framework gdpr --tenant-id <id> --from 2026-W21-MON --to 2026-W21-SUN` then grep `actor=agent:X`. |
| "Who got denied by HotL last 24h, and why?" | SOC2 bundle filtered to `action=policy.deny` — `details_summary` carries the reason. |
| "Did the chain break at any point in Q1?" | The export itself refuses if the chain broke inside the window. Plus: `GET /v1/admin/audit/verify?tenant_id=...` walks the global chain. |

## Chain broken — what now?

If the export refuses, the HTTP response is **409 Conflict** + a JSON body:

```json
{
  "error": "chain_broken",
  "first_broken_id": 89421,
  "first_broken_ts": "2026-03-14T07:42:18Z"
}
```

The CLI exits non-zero with the same JSON on stderr. There is **no
`--skip-verify` flag** by design — a refusing export is the point.

Diagnose:

1. `GET /v1/admin/audit/verify?tenant_id=<id>` — confirms the global break
   (the export only verifies the window slice).
2. Pull the offending row: `GET /v1/admin/audit?tenant_id=<id>&since=<ts-1h>&until=<ts+1h>`
   and identify the broken `id`.
3. Compare `prev_hmac` of the broken row to `hmac` of the previous row.
   If they don't match → row was inserted out-of-band or a previous row
   was deleted. If they match but the HMAC itself is wrong → row payload
   was mutated post-write.
4. Restore from backup (`xiaoguai backup` + `xiaoguai restore`); do NOT
   try to repair the chain in place — the cryptographic property is the
   audit value.

## Operational caps and limits

- **Window row cap: 100 000.** The exporter pulls at most 100k rows from
  Postgres per call. Larger windows should be requested in chunks; the
  underlying chain is per-tenant so chunking on `ts` is safe.
- **No background jobs.** The export is synchronous. For very large
  bundles, expect the HTTP call to take seconds-to-minutes — increase
  client timeouts accordingly.
- **No PDF yet.** `--format pdf` returns HTTP 501 / non-zero CLI exit
  with `pdf_unimplemented`. Tracked as a post-T5 follow-up; the surface
  area (`render_pdf` stub function) is in place.
- **No cross-tenant export.** The audit chain is per-tenant, so is the
  export. There is no admin override.

## Operator setup

The exporter is wired automatically whenever the signing key env var
(`XIAOGUAI_AUDIT_SIGNING_KEY` by default; configurable via
`settings.audit.signing_key_env`) is set in the `xiaoguai serve`
environment. With the key absent, `POST /v1/audit/exports` returns 503,
matching the existing behaviour of `/v1/admin/audit` and
`/v1/admin/audit/verify`.

```bash
# Production boot — signing key from a secret manager.
XIAOGUAI_AUDIT_SIGNING_KEY=$(aws ssm get-parameter --name /xiaoguai/audit/key --with-decryption --query Parameter.Value --output text) \
  xiaoguai --config /etc/xiaoguai/production.yaml serve
```

## Follow-up work (post-T5)

1. **PDF rendering.** `crates/xiaoguai-audit/src/export.rs::render_pdf`
   is a stub. Likely use `printpdf` or `genpdf`; needs a layout pass on
   what the bundle looks like printed.
2. **Streaming / paginated export** for tenants whose windows exceed the
   100k row cap.
3. **Runtime template DSL.** Customers occasionally ask for a tenant-
   specific framework profile. The current template engine is static
   `match` arms; a future YAML-driven projection would let tenants
   declare their own filters without forking the crate.
4. **Bundle re-signing.** The chain proof in the header is the integrity
   guarantee; we don't currently sign the *bundle* itself. A future
   detached signature (e.g. minisign over the rendered bytes) would let
   the bundle travel through untrusted channels.
