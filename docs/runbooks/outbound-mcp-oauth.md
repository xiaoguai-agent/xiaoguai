# Outbound MCP OAuth 2.1 PKCE (Tier-3 T4)

Operator runbook for connecting xiaoguai to remote MCP servers that
require OAuth 2.1 + PKCE per RFC 7636. Covers registration, threat
model, the corporate-CA landmine, the revoke flow, and what's
explicitly **not** supported.

## Overview

`xiaoguai-mcp` now speaks OAuth 2.1 with PKCE for outbound HTTP
transports. The acquisition + storage paths are:

- **Acquisition**: `xiaoguai mcp register --auth oauth2-pkce …`
  generates a PKCE verifier+S256 challenge, binds a local listener
  on `127.0.0.1:<random-port>`, prints the consent URL, waits for
  the browser redirect (5 min default), and exchanges the code for
  a `TokenBundle`.
- **Storage**: tokens are persisted to `mcp_oauth_tokens`
  (RLS-isolated by `tenant_id`). The table is created by migration
  `0022_mcp_oauth_tokens.sql`.
- **Refresh**: on every outbound HTTP connect, if the stored
  `expires_at` is within 60s, `xiaoguai-mcp` calls
  `grant_type=refresh_token` and atomically updates the bundle.
  Refresh-token rotation is respected (RFC 6749 §6).

## Registering an OAuth-authed MCP server

```bash
xiaoguai mcp register \
  --name notion \
  --transport http \
  --endpoint https://mcp.notion.so/v1 \
  --tenant tenant_acme \
  --auth oauth2-pkce \
  --auth-url https://auth.notion.so/oauth/authorize \
  --token-url https://auth.notion.so/oauth/token \
  --client-id xg-client-abc123 \
  --scopes mcp.read,mcp.write
```

Output:

```
Open this URL in a browser to consent:

  https://auth.notion.so/oauth/authorize?response_type=code&client_id=xg-client-abc123&redirect_uri=http%3A%2F%2F127.0.0.1%3A53217%2Fcallback&scope=mcp.read+mcp.write&state=...&code_challenge=...&code_challenge_method=S256

Waiting for callback (timeout: 300s)...
registered mcp_01HZX… (notion@1.0.0)
oauth: access_token expires 2026-05-29T16:42:11Z
```

The browser shows the auth server's consent page. After the user
clicks "Allow", the auth server redirects to `http://127.0.0.1:<port>/callback?code=…&state=…`,
the CLI's in-process listener accepts the redirect, exchanges the
code for a token bundle, persists it, and exits.

### Flag reference

| Flag | Required when | Notes |
|---|---|---|
| `--auth oauth2-pkce` | OAuth flow | Omit for static / no-auth servers (`--auth none` is the explicit form). |
| `--auth-url` | `--auth=oauth2-pkce` | `/authorize` endpoint. |
| `--token-url` | `--auth=oauth2-pkce` | `/token` endpoint. |
| `--client-id` | `--auth=oauth2-pkce` | Public client identifier registered with the IdP. |
| `--scopes` | optional | Comma-separated list. Joined with spaces in the authorize URL. |
| `--tenant` | OAuth flow | Required — tokens are per-tenant, no system-wide OAuth in this release. |

## Threat model

| Asset | Lifetime | Storage | Mitigations |
|---|---|---|---|
| `code_verifier` (PKCE) | one consent flow (~5 min) | RAM (CLI process) | Never written to disk; dropped after `exchange_code`. |
| Authorization `code` | seconds | RAM | Single-use per RFC 6749; bound to the `redirect_uri` by the IdP. |
| `access_token` | minutes–hours | `mcp_oauth_tokens.access_token` | RLS by `tenant_id`. Refreshed within 60 s of `expires_at`. |
| `refresh_token` | days–months | `mcp_oauth_tokens.refresh_token` | RLS by `tenant_id`. **Not encrypted at the application layer** (see "Deferred"). |
| OAuth client config (`auth_url`, `token_url`, `client_id`, `scopes`) | until revoke | `mcp_servers.auth` JSONB | Public — no secrets. |

### What's defended

- **CSRF** (Cross-Site Request Forgery on the redirect leg): each
  consent flow generates a fresh `state` value (32 random bytes →
  base64url-no-pad). The callback handler rejects the response if
  `state` doesn't match what was sent.
- **Auth-code injection**: PKCE binds the authorization code to the
  caller (verifier ⇄ challenge), preventing a malicious app from
  redeeming an intercepted code.
- **Tenant isolation**: `mcp_oauth_tokens` is gated by RLS policy
  `tenant_isolation_mcp_oauth_tokens` — tenant A cannot read tenant
  B's tokens even if both share the same `server_id`.
- **TLS verification**: ON by default. `XIAOGUAI_MCP_OAUTH_INSECURE=1`
  is the only escape hatch and emits `tracing::warn` at every
  client build.

### What's NOT defended (operator's responsibility)

- **DB-at-rest encryption** for refresh tokens. Use PostgreSQL TDE
  / cloud volume encryption / `pgcrypto` column-level if your
  threat model needs it. App-level envelope encryption is a
  deferred hardening PR (see end of this doc).
- **Local-machine compromise** during the consent flow. The
  `code_verifier` lives in process memory; an attacker with
  arbitrary local code execution can read it.
- **Callback listener race** when multiple `xiaoguai mcp register`
  processes are running concurrently. Each binds its own random
  port — collisions are theoretically possible but practically
  zero. If a port is exhausted, the second `register` fails fast.

## Corporate proxy / self-signed CA landmine

xiaoguai uses `reqwest` with `rustls-tls`. `rustls` honours the
process environment for CA bundles via `webpki-roots` plus the
standard `SSL_CERT_FILE` / `SSL_CERT_DIR` variables.

If your corporate proxy MITMs HTTPS with its own root CA:

```bash
export SSL_CERT_FILE=/etc/pki/tls/cert.pem        # RHEL/CentOS
export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt   # Debian/Ubuntu
export SSL_CERT_FILE=/opt/corp/ca/internal-root.pem        # bring-your-own bundle

xiaoguai mcp register --auth oauth2-pkce …
```

If `SSL_CERT_FILE` doesn't help (chain order weirdness, custom
intermediates not in the bundle), the last-resort escape hatch is:

```bash
XIAOGUAI_MCP_OAUTH_INSECURE=1 xiaoguai mcp register --auth oauth2-pkce …
```

This disables TLS verification for the token endpoint. It is logged
at `warn` level on every client build:

```
WARN  TLS verification DISABLED for OAuth token endpoint;
      do NOT use in production env=XIAOGUAI_MCP_OAUTH_INSECURE
```

**Never** set this on a production tenant. The OAuth tokens you
acquire while it's set are still durably stored — they're just
acquired over an unverified TLS channel. Rotate them after fixing
the certificate chain.

Background: an analogous landmine bit us in the Python skills via
`uv`'s bundled `webpki` ignoring the system CA store (see
`ci-gotchas.md`). The Rust path is friendlier because `rustls`
respects `SSL_CERT_FILE`, but the failure mode rhymes.

## Revoke flow

```bash
xiaoguai mcp remove --id mcp_01HZX…
```

The `ON DELETE CASCADE` from `mcp_servers` to `mcp_oauth_tokens`
guarantees the token rows are cleaned up in the same transaction.
After the row deletion, **revoke the refresh_token on the IdP side**
by calling the IdP's revocation endpoint manually — xiaoguai does
not currently issue RFC 7009 revocations on its own.

If the IdP supports administrative session listing, double-check
that all sessions tied to your `client_id` are torn down on the IdP
side as well.

## What's NOT supported in this release

Documented for clarity — the brief explicitly defers these:

- **RFC 7591 dynamic client registration**. Operators must register
  the OAuth client with the IdP out-of-band and supply the
  `client_id` via `--client-id`.
- **RFC 8628 device-code flow**. Browser redirect is the only
  consent flow. Headless servers that can't reach a browser must
  proxy the consent URL through a workstation that can.
- **mTLS client authentication**. Only public clients (PKCE) are
  supported.
- **RFC 7662 token introspection**. xiaoguai only refreshes on
  expiry; it does not pre-validate access tokens against the IdP.
- **Application-layer encryption-at-rest** for refresh tokens. RLS
  handles tenant isolation; volume-level / TDE encryption is the
  operator's responsibility. A future hardening PR may add envelope
  encryption with a key handle in `XIAOGUAI_OAUTH_KEK_ENV` modelled
  on `xiaoguai-audit::signing_key`.
- **UI for token management**. CLI only. List with
  `xiaoguai mcp list --tenant <id>`; rotate by re-registering;
  delete with `xiaoguai mcp remove --id <id>`.
- **Supervisor-side auto-reconnect after token refresh**. When the
  long-running `xiaoguai serve` supervisor opens an HTTP MCP
  client, it currently passes the stored bearer in at connect time.
  If the access token expires mid-session, the next reconnect will
  refresh. Implementing in-session bearer rotation requires a
  follow-up that touches the `RunningService` transport.

## Migration runbook

```bash
# Apply schema:
psql "$DATABASE_URL" -f crates/xiaoguai-storage/migrations/0022_mcp_oauth_tokens.sql

# Inspect:
psql "$DATABASE_URL" -c "\d mcp_oauth_tokens"
psql "$DATABASE_URL" -c "\d mcp_servers" | grep -i auth
```

Rollback:

```bash
psql "$DATABASE_URL" -c "DROP TABLE mcp_oauth_tokens;"
psql "$DATABASE_URL" -c "ALTER TABLE mcp_servers DROP COLUMN auth;"
```

The migration is additive: dropping the table + column leaves the
rest of the registry untouched, and `xiaoguai mcp register` without
`--auth=oauth2-pkce` continues to work unchanged.

## Verifying the deployment

```bash
# 1. Sanity-check the schema:
psql "$DATABASE_URL" -c "SELECT column_name FROM information_schema.columns \
                          WHERE table_name = 'mcp_oauth_tokens' ORDER BY ordinal_position;"

# 2. Register against a sandbox IdP (e.g. Auth0 test tenant):
xiaoguai mcp register --auth oauth2-pkce \
  --name auth0-sandbox --transport http \
  --endpoint https://example-mcp.local \
  --auth-url https://example.auth0.com/authorize \
  --token-url https://example.auth0.com/oauth/token \
  --client-id <PUBLIC_CLIENT_ID> \
  --scopes mcp.read --tenant tenant_sandbox

# 3. Confirm the bundle landed in storage:
psql "$DATABASE_URL" -c "SELECT server_id, tenant_id, expires_at FROM mcp_oauth_tokens;"

# 4. Confirm RLS:
psql "$DATABASE_URL" -c "SET app.current_tenant_id = 'tenant_other'; SELECT * FROM mcp_oauth_tokens;"
# (should return 0 rows)
```

## See also

- `docs/plans/2026-05-29-tier3-oauth-pkce-outbound-mcp.md` — design plan.
- `crates/xiaoguai-mcp/src/auth/oauth2_pkce.rs` — implementation.
- `crates/xiaoguai-mcp/tests/oauth_pkce_e2e.rs` — integration test.
- `crates/xiaoguai-storage/migrations/0022_mcp_oauth_tokens.sql` — schema.
- RFC 7636 (PKCE), RFC 6749 (OAuth 2.0), RFC 6749 §6 (refresh rotation).
