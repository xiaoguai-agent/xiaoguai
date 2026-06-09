# Security

## Authentication and authorization

Under the single-user pivot (DEC-033) each person runs their own instance, so
there is no OIDC, no JWT, no Casbin, no RBAC, no scopes, and no multi-tenancy.
Access collapses to a **single static owner** protected by an optional
username + password checked via HTTP Basic:

- Set `auth.username` + `auth.password` (or env `XIAOGUAI_AUTH__USERNAME` /
  `XIAOGUAI_AUTH__PASSWORD`) and every `/v1/**` request must carry a matching
  `Authorization: Basic …` header.
- Leave both empty for an open localhost dev run.

Set the credential before exposing the service on a URL; front it with TLS
(nginx/Caddy/cloud LB) for transport security.

## Data isolation

State is one embedded SQLite file owned by the OS user that runs the binary —
isolation is the filesystem boundary, not database RLS. There are no tenants
and no per-tenant scoping. Protect the data directory (`~/.xiaoguai/` or
`$XDG_DATA_HOME/xiaoguai/`) with normal file permissions and, for sensitive
deployments, full-disk / volume encryption.

## Audit chain

The audit log is append-only and HMAC-chained. Each row carries:

- `id` — UUID
- `user_id`, `session_id`
- `event_kind` — one of `chat_turn`, `tool_call`, `scheduler_run`, `im_message`, …
- `payload` — JSON
- `prev_hash` — HMAC of the previous row
- `hash` — HMAC of this row using the chain key

To verify the chain:

```bash
xiaoguai admin audit verify
```

A chain inconsistency means tampered data.

## Rotating the audit HMAC key

See [Day-2 Operations](day2.md) for the full procedure. The summary:

1. Export the chain head pointer.
2. Generate a fresh key (`openssl rand -hex 32`) and set it in the configured
   signing-key env var (`XIAOGUAI_AUDIT_SIGNING_KEY` by default).
3. Restart the service with the new key.
4. Keep the old key for the 30-day verification window.

## Supply chain security

Cargo dependency vetting is documented in [Supply Chain Security](../developer/supply-chain.md).

The CI `deny.yml` workflow enforces:
- No licenses incompatible with Apache-2.0 (no AGPL/SSPL/GPL-2.0)
- No unmaintained crates flagged in RustSec
- No duplicate dependency versions (advisory)

## Secrets management

| Secret | Where stored |
|--------|-------------|
| Owner password (`auth.password`) | env `XIAOGUAI_AUTH__PASSWORD` / `.env` |
| Audit HMAC key | env (rotatable) — `XIAOGUAI_AUDIT_SIGNING_KEY` |
| IM webhook secrets (Feishu/DingTalk/WeCom) | `.env` |
| Scheduler webhook tokens | `scheduler_webhook_tokens` table (hashed, in `data.db`) |

Never commit secrets to git. The `cargo-deny` check will flag `rustsec://` advisories for
crates with known secret-handling bugs.
