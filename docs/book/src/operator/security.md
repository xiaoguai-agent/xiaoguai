# Security

## Authentication and authorization

Xiaoguai uses OIDC RS256/ES256 JWT validation with a JWKS cache. Set
`XIAOGUAI_AUTH_REQUIRED=true` to enforce authentication on all `/v1/**` endpoints.

RBAC is handled by Casbin with a model that supports tenant-scoped role assignments.
Roles are stored in Postgres and loaded at startup.

## Multi-tenant data isolation

Every tenant-scoped table in Postgres has row-level security (RLS) enabled. The application
layer also filters by `tenant_id` on every query. A leaked JWT cannot access another tenant's
data without also having a matching `tenant_id` claim.

## Audit chain

The audit log is append-only and HMAC-chained. Each row carries:

- `id` — UUID
- `tenant_id`, `user_id`, `session_id`
- `event_kind` — one of `chat_turn`, `tool_call`, `scheduler_run`, `im_message`, …
- `payload` — JSON
- `prev_hash` — HMAC of the previous row
- `hash` — HMAC of this row using the chain key

To verify the chain:

```bash
xiaoguai admin audit verify --tenant <tenant-id>
```

A chain inconsistency means tampered data.

## Rotating the audit HMAC key

See [Day-2 Operations](day2.md) for the full procedure. The summary:

1. Export the chain head pointer
2. Create a new Kubernetes secret with a fresh key (`openssl rand -hex 32`)
3. Rolling upgrade with the new secret name
4. Keep both secrets for the 30-day verification window

## Supply chain security

Cargo dependency vetting is documented in [Supply Chain Security](../developer/supply-chain.md).

The CI `deny.yml` workflow enforces:
- No licenses incompatible with BUSL-1.1 (no AGPL/SSPL/GPL-2.0)
- No unmaintained crates flagged in RustSec
- No duplicate dependency versions (advisory)

## Secrets management

| Secret | Where stored |
|--------|-------------|
| Postgres DSN | K8s Secret / `.env` |
| Valkey URL | K8s Secret / `.env` |
| Audit HMAC key | K8s Secret (rotatable) |
| OIDC JWKS endpoint | Config / env |
| IM webhook secrets (Feishu/DingTalk/WeCom) | K8s Secret / `.env` |
| Scheduler webhook tokens | Postgres `scheduler_webhook_tokens` table (hashed) |

Never commit secrets to git. The `cargo-deny` check will flag `rustsec://` advisories for
crates with known secret-handling bugs.
