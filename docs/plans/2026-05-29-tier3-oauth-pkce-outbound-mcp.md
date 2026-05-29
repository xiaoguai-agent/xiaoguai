# Tier-3 T4 — OAuth 2.1 PKCE for outbound MCP servers

**Status:** drafted 2026-05-29, awaiting self-review.
**Branch:** `feat/tier3-oauth-pkce-outbound-mcp`
**R.E.S.T. axis:** Security primary.

## 1. Context

Today `xiaoguai-mcp::McpClient` connects to MCP servers via three transports
(stdio child process, SSE legacy, Streamable HTTP). Authentication is
limited to a static bearer string passed verbatim through
`HttpClientConfig::auth_header`. There is no acquisition, no refresh, no
per-tenant token storage, and `xiaoguai mcp register` has no way to
acquire a token via user consent.

This blocks every authed remote MCP server in the wild: Linear, Notion,
GitHub's official MCP service, every Anthropic-hosted connector — all
require an OAuth 2.1 + PKCE consent flow per RFC 7636 with refresh-token
rotation.

T4 adds OAuth 2.1 PKCE acquisition + persistence + auto-refresh inside
`xiaoguai-mcp`, surfaces a consent flow through `xiaoguai mcp register
--auth oauth2-pkce`, persists `(server_id, tenant_id) -> TokenBundle`
in a new RLS-protected table, and refreshes tokens 60s before expiry
on every outbound HTTP request.

### Adjustments to the brief

- **Encrypted-at-rest refresh tokens: deferred.** The brief asks for a
  decision; the answer is "no, not in this PR". Reusing
  `xiaoguai-audit::signing_key` for encryption-at-rest is tempting but
  conflates two concerns: signing keys are HMAC, not symmetric
  encryption keys, and audit-key rotation semantics (append-only chain
  invariant) don't fit a mutable token table. A clean implementation
  needs its own envelope-encryption module (AES-GCM + key handle in
  env-var, with key-rotation re-encrypt). That's a 200-LOC + RLS-policy
  refactor on its own. RLS already enforces tenant isolation; DB
  filesystem encryption is the operator's responsibility. This is
  documented in the runbook as a hardening item.
- **mockito vs wiremock-rs.** The brief mentions `wiremock-rs`; the
  workspace already pulls `mockito 1.6` and uses it in nine crates.
  Using mockito means no new transitive deps. Both libraries support
  the same assertions (PKCE verifier round-trip, refresh-token
  rotation). The plan uses mockito.
- **OAuth crate vs hand-rolled.** The `oauth2` crate (5.0) pulls a new
  `getrandom 0.3` chain and an alternative async-http abstraction. We
  already have `rand 0.10` (`fill_bytes`), `sha2 0.11`, `base64 0.22`,
  and `reqwest`. Hand-rolling PKCE is ~120 LOC and avoids a
  potentially-MSRV-affecting dep. Plan uses hand-rolled.
- **In-process consent flow for CLI.** A tokio TCP listener on
  `127.0.0.1:0` with a single-route handler accepts the redirect, then
  shuts down. The CLI prints the consent URL and waits up to 5
  minutes. No browser auto-open in this PR (test on headless servers
  too); operator clicks the URL.

## 2. Success criteria

- `cargo test -p xiaoguai-storage -p xiaoguai-mcp -p xiaoguai-cli` exits 0.
- New integration test `crates/xiaoguai-mcp/tests/oauth_pkce_e2e.rs` covers:
  - PKCE happy path: verifier+challenge round-trip via mockito; code
    exchanged for `TokenBundle` matches expected access/refresh/expiry.
  - Refresh path: when `expires_at < now + 60s`, `McpClient` calls
    `refresh_pkce` and attaches the new bearer.
  - Refresh-token rotation: token endpoint returns a new
    `refresh_token`; the stored bundle is updated atomically.
  - Old refresh preserved: token endpoint returns no new
    `refresh_token`; old one is retained.
- `cargo fmt --check` exit 0.
- Unit tests for: `code_verifier_shape` (43-128 chars, URL-safe
  alphabet), `code_challenge = base64url(sha256(verifier))`,
  in-memory `TokenStore` get/put round-trip, `should_refresh` window.
- Migration `0022_mcp_oauth_tokens.sql` parses and runs offline (no
  live PG required for tests; mirrored from PR #72's pattern).
- Runbook documents `SSL_CERT_FILE` corporate-proxy workaround,
  `XIAOGUAI_MCP_OAUTH_INSECURE` escape hatch logged at `warn`,
  revoke flow, and explicitly the out-of-scope list (RFC 7591, RFC
  8628, mTLS, RFC 7662, encrypted-at-rest, UI).

## 3. Prerequisites

- Workspace deps already present: `rand 0.10`, `sha2 0.11`, `base64
  0.22`, `reqwest 0.12`, `mockito 1.6`, `tokio 1.40`, `serde_json`,
  `chrono`, `sqlx 0.8`.
- `xiaoguai-mcp` already depends on `reqwest` (used by rmcp HTTP
  transport and `servers::github_pr`); adding `rand` + `base64` is a
  workspace passthrough — no new transitive deps.
- `xiaoguai-cli` already depends on `xiaoguai-storage` and `tokio`.
- No live PG required for tests (per brief).

## 4. Step-by-step

### Step 1 — Migration `0022_mcp_oauth_tokens.sql`

Add column `auth JSONB NULL` to `mcp_servers` with a CHECK constraint
that only allows `null` or `{"type":"oauth2_pkce", ...}` shape.

Create `mcp_oauth_tokens`:

```
id              TEXT PRIMARY KEY,
server_id       TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
tenant_id       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
access_token    TEXT NOT NULL,
refresh_token   TEXT,
expires_at      TIMESTAMPTZ NOT NULL,
created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
UNIQUE(server_id, tenant_id)
```

RLS policy `tenant_isolation_mcp_oauth_tokens`: `tenant_id =
current_setting('app.current_tenant_id', true)`. Index on
`(server_id, tenant_id)`.

`VC:` File parses; `sqlx migrate run` against the
test-container PG in migrations-smoke job is a follow-up; here the
parse check is "file ends in semicolon, valid SQL keyword tokens".

### Step 2 — Hand-rolled PKCE primitives in `crates/xiaoguai-mcp/src/auth/oauth2_pkce.rs`

Module surface:

```rust
pub struct AuthConfig { /* enum-tag for future variants */ }
pub struct OAuth2PkceConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub redirect_uri: String,  // bound by CLI to 127.0.0.1:<port>/callback
}
pub struct TokenBundle {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}
pub struct PkcePair { pub verifier: String, pub challenge: String }

pub fn new_pkce_pair() -> PkcePair;  // rand + sha256 + base64url-no-pad
pub fn build_authorize_url(cfg: &OAuth2PkceConfig, challenge: &str, state: &str) -> String;
pub async fn exchange_code(http: &reqwest::Client, cfg: &OAuth2PkceConfig, code: &str, verifier: &str) -> McpResult<TokenBundle>;
pub async fn refresh_pkce(http: &reqwest::Client, cfg: &OAuth2PkceConfig, bundle: &TokenBundle) -> McpResult<TokenBundle>;

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn get(&self, server_id: &str, tenant_id: &str) -> McpResult<Option<TokenBundle>>;
    async fn put(&self, server_id: &str, tenant_id: &str, bundle: &TokenBundle) -> McpResult<()>;
}

pub struct InMemoryTokenStore { /* DashMap */ }
```

Implementation notes:
- `new_pkce_pair`: 64 random bytes → base64url-no-pad = 86-char
  verifier (within RFC 43..128 bound). SHA-256 → base64url-no-pad =
  43-char challenge.
- `exchange_code` POSTs form-urlencoded:
  `grant_type=authorization_code`, `code`, `redirect_uri`,
  `client_id`, `code_verifier`.
- Response parses `access_token`, optional `refresh_token`,
  `expires_in` (seconds). `expires_at = now + expires_in`.
- `refresh_pkce` POSTs `grant_type=refresh_token`, `refresh_token`,
  `client_id`. If the response contains a new `refresh_token`, the
  returned bundle carries it; otherwise the old `refresh_token` is
  preserved (per RFC 6749 §6).
- TLS: `XIAOGUAI_MCP_OAUTH_INSECURE=1` toggles
  `reqwest::ClientBuilder::danger_accept_invalid_certs(true)` and
  emits one `tracing::warn!` at client build.

`VC:` 5 unit tests:
1. `pkce_verifier_shape` — len ∈ [43,128], chars ∈ URL-safe alphabet.
2. `pkce_challenge_matches_sha256` — `base64url_nopad(sha256(verifier)) == challenge`.
3. `inmemory_store_round_trip`.
4. `authorize_url_includes_required_params` — `code_challenge_method=S256`, `state`, `scope`, `client_id`, `response_type=code`, `redirect_uri`.
5. `should_refresh_within_60s_window` — boolean helper covers
   "expired", "expires in 30s", "expires in 5min".

### Step 3 — Wire `TokenStore` into outbound HTTP transport

The current `HttpMcpClient::connect` is one-shot: it bakes the
`auth_header` into the transport at construct time. There is no
per-request hook in `StreamableHttpClientTransport`. Two options:

(a) Spawn a refresh-tick task at connect time: check expiry, refresh,
construct a new transport. Rebuild on rotation.

(b) Resolve at connect time: pull current bundle from `TokenStore`
once, set `Authorization` header, set up a `reconnect_on_401` hook
later (not in this PR).

Option (b) ships in this PR — covers the test matrix (refresh fires
when `expires_at < now + 60s` *before* the next `connect`). Option (a)
is documented in the runbook as a follow-up. Real refresh-during-an-
open-session is a property of the supervisor reconnect path, not the
transport — and the supervisor already supports stop/start.

Adds `HttpMcpClient::connect_with_store(cfg, store, server_id,
tenant_id) -> McpResult<Self>`. On call:
1. `TokenStore::get(server_id, tenant_id)`.
2. If `Some(bundle)` and `bundle.expires_at < now + 60s`: build a
   reqwest client (with `XIAOGUAI_MCP_OAUTH_INSECURE` honored), call
   `refresh_pkce`, `TokenStore::put`, use the new bundle.
3. If `None` and `cfg.auth_header.is_none()`: return
   `McpError::AuthRequired`.
4. Attach `Authorization: Bearer <access_token>` and connect.

New error variant `McpError::AuthRequired`.

`VC:` Integration test `oauth_pkce_e2e.rs` exercises this code path
end-to-end with mockito serving the token endpoint.

### Step 4 — CLI `xiaoguai mcp register --auth oauth2-pkce`

Add flags to `McpCmd::Register`:
- `--auth` (Option<String>): `oauth2-pkce` or omitted.
- `--auth-url`, `--token-url`, `--client-id`: required when `--auth=oauth2-pkce`.
- `--scopes` (Vec<String>, comma-separated).

When `--auth=oauth2-pkce`:
1. Generate `PkcePair` + `state` (`new_pkce_pair` + 32 random bytes →
   base64url-no-pad).
2. Bind `tokio::net::TcpListener::bind("127.0.0.1:0")`, capture port.
3. `redirect_uri = http://127.0.0.1:<port>/callback`.
4. Build authorize URL via `build_authorize_url`.
5. Print: `Open this URL in a browser:\n<url>\nWaiting for callback (timeout: 5min)...`
6. Accept one connection, parse `?code=...&state=...` from the request
   line, write a 200 with a "you can close this window" body, drop
   the connection.
7. Validate `state` matches.
8. `exchange_code` → `TokenBundle`.
9. Insert into `mcp_servers` (with `auth = jsonb` describing
   `oauth2_pkce` config, sans secrets), then insert into
   `mcp_oauth_tokens`.
10. Print `registered <id> (<name>@<version>)` as today, plus a line
    `oauth: access_token expires <expires_at>`.

The browser flow is testable by POSTing the callback URL directly to
the listener — the test harness doesn't need a real browser.

`VC:` Unit test for `parse_callback_query` (URL parse + state
validation), and an integration smoke that runs the full register-
with-oauth flow against a mockito token endpoint with the callback
fired synthetically.

### Step 5 — Integration test `crates/xiaoguai-mcp/tests/oauth_pkce_e2e.rs`

Three test cases, all using `InMemoryTokenStore` + mockito:

1. `pkce_verifier_matches_challenge`: spin up a mockito token
   endpoint that asserts the `code_verifier` form param's SHA-256 =
   the challenge it captured during authorize URL build. Call
   `exchange_code` and assert OK + non-empty access_token.
2. `expired_access_token_triggers_refresh`: pre-seed store with a
   bundle whose `expires_at = now - 1s`. Mockito asserts a
   `grant_type=refresh_token` request. After the refresh path runs,
   store contains the new access_token, the new `expires_at` is
   future, and the bearer flowing into the (mock) MCP server is the
   new one. (Asserts via a mockito-served MCP endpoint that the
   bearer header matches.)
3. `refresh_token_rotation_persists`: token endpoint returns a
   *different* `refresh_token`; assert stored bundle's
   `refresh_token` is the new one. Then a second test fork: token
   endpoint returns *no* `refresh_token`; assert stored bundle keeps
   the old one.

`VC:` `cargo test -p xiaoguai-mcp --test oauth_pkce_e2e` exits 0
with 3 passing tests.

### Step 6 — Runbook `docs/runbooks/outbound-mcp-oauth.md`

Sections:
1. Overview — what xiaoguai supports, what it doesn't.
2. Registering an OAuth-authed MCP server — full
   `xiaoguai mcp register --auth oauth2-pkce ...` walkthrough.
3. Threat model — code_verifier as transient secret, refresh-token
   as long-lived secret persisted in PG (acknowledge no app-level
   encryption-at-rest in this release).
4. Corporate proxy / self-signed CAs — `SSL_CERT_FILE=/path/to/ca-
   bundle.pem` for reqwest (rustls reads it via webpki-roots
   fallback). `XIAOGUAI_MCP_OAUTH_INSECURE=1` as the last-resort
   bypass; logged at `warn`.
5. Revoke flow — `xiaoguai mcp remove <id>` cascades to
   `mcp_oauth_tokens` (RLS + FK ON DELETE CASCADE).
6. Out of scope — RFC 7591 dynamic client registration, RFC 8628
   device code, mTLS client auth, RFC 7662 introspection,
   encrypted-at-rest refresh tokens, UI for token management.

`VC:` File is ≥150 lines; markdown lints clean; runbook is linked
from the PR description.

## 5. Risks

- **Hand-rolled PKCE drift from spec.** Verifier alphabet + lengths
  per RFC 7636 §4.1, challenge per §4.2. Unit tests pin both. If a
  server disagrees, the failure is loud and obvious (server returns
  `invalid_grant`).
- **`oauth2` crate may be a better long-term home.** If it lands an
  MSRV-compatible version, replacing the hand-rolled module is a
  drop-in. The trait surface (`TokenStore`, `TokenBundle`) is
  independent.
- **Listener-port-collision in CI.** `TcpListener::bind("127.0.0.1:0")`
  picks a free port — no collision unless the OS is starved.
- **Token endpoint over plaintext HTTP in tests.** Tests use
  `http://127.0.0.1:<port>` via mockito; production code defaults to
  TLS-on. `XIAOGUAI_MCP_OAUTH_INSECURE=1` escape hatch documented.
- **Migration `0022` ordering.** Last applied is `0020_ollama_default`
  in the worktree; need to confirm `0021_*` doesn't already exist.
  Verified `ls migrations/` — `0020` is last.
- **`auth_url` injection.** CLI builds the authorize URL from operator
  flags; treated as trusted input. Documented in runbook.
- **In-memory `TokenStore` not for production.** The CLI test path
  uses it; the `xiaoguai serve` path uses a future `PgTokenStore`
  (deferred — supervisor reconnect path is the other blocker).

## 6. Rollback

- Migration `0022` is additive (new table + nullable column on
  existing table). `DROP TABLE mcp_oauth_tokens; ALTER TABLE
  mcp_servers DROP COLUMN auth;` cleanly reverts.
- `HttpMcpClient::connect_with_store` is a new method; the existing
  `connect` keeps current bearer-string behavior. No removed APIs.
- `xiaoguai mcp register` without `--auth=oauth2-pkce` is unchanged.
- Removing the `auth` module is a single `mod` line in `lib.rs`.

## 7. Out of scope (per brief)

- RFC 7591 dynamic client registration.
- RFC 8628 device-code flow.
- mTLS client auth.
- RFC 7662 token introspection.
- Encrypted-at-rest refresh tokens (deferred to hardening PR).
- UI for OAuth token management.
- `xiaoguai serve` end-to-end consumption of `mcp_oauth_tokens` (this
  PR ships the storage + the CLI path; supervisor consumption is a
  follow-up because the supervisor's connect-on-start loop needs a
  matching change in `xiaoguai-core::run_serve` to wire a
  `PgTokenStore`).

## 8. References

- RFC 7636 (PKCE), RFC 6749 (OAuth 2.1 spec), RFC 6749 §6 (refresh
  rotation).
- PR #72 (`feat/tier2-d1-agent-authored-skills`) — in-memory store +
  audit emission pattern.
- PR #66 (`feat/tier2-mcp-exec`) — clear module surface convention.
- `docs/HANDOFF-2026-05-28-session5.md` — roadmap context, Tier-3
  row.
- `crates/xiaoguai-storage/migrations/0005_mcp_servers.sql` — RLS
  policy pattern.
- `crates/xiaoguai-mcp/src/http.rs` — existing HTTP transport surface.
- `crates/xiaoguai-im-discord/src/signature.rs` — rand 0.10 byte-
  filling pattern.
- ci-gotchas memory — corporate-CA / `SSL_CERT_FILE` landmine.

---

## Self-review (6-point protocol)

1. **Has the brief been addressed end-to-end?**
   - All six implementation checkpoints (migration, oauth2_pkce
     module, McpClient wiring, CLI flow, integration test, runbook)
     are individual steps. The open question on encrypted-at-rest is
     resolved with a defended "defer" in §1 Adjustments.

2. **Are the hard constraints honored?**
   - TLS default-on, `XIAOGUAI_MCP_OAUTH_INSECURE=1` escape hatch
     logged at `warn` (Step 2).
   - Refresh-token rotation handled atomically + old retained when
     absent (Step 2 spec + Step 5 test cases).
   - No live PG (in-memory store + mockito; mirrors PR #72) — Step 5.
   - Browser flow tests POST the callback synthetically (Step 4 VC).
   - Out-of-scope items mirror the brief verbatim (§7).

3. **Are the success criteria observable and testable?**
   - All five are mechanical: cargo exit codes, file presence,
     specific test counts, specific runbook sections. Self-review can
     verify each post-implementation.

4. **Are risks named with mitigations or accepted?**
   - 6 risks named; each either has a mitigation (unit tests pin spec
     conformance, `bind(":0")` avoids port collision) or is
     acknowledged-and-deferred (in-memory store is CLI-only;
     supervisor wiring is follow-up).

5. **Is the diff bounded?**
   - 1 migration, 1 new module dir (`auth/` w/ `mod.rs` + `oauth2_pkce.rs`),
     additions to `lib.rs` / `error.rs` / `http.rs`, additions to
     `commands/mcp.rs` + `main.rs`, 1 new integration test, 1 new
     runbook. ~600 LOC additions, ~30 LOC mods. No deletions.

6. **What's the smallest verifiable shippable slice?**
   - Steps 1+2+5 alone would be shippable (storage + lib + test).
     Adding Step 3 closes the test loop end-to-end.
     Step 4 adds the operator-facing entry point.
     Step 6 closes the docs gap. All five are needed for "Tier-3 row
     ticks green" on the roadmap; none can be split off without
     leaving a half-feature.

**Self-review verdict: PASS.** Proceed to implementation.
