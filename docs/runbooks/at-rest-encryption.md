# Encryption-at-rest for provider API keys

Closes the field-encryption half of **SEC-08**: the LLM provider `api_key`
column (`llm_providers.api_key`, web-UI / CLI-registered providers) can now be
authenticated-encrypted at rest with a key the database has never seen. A
backup leak or host compromise no longer hands an attacker the plaintext
provider keys.

It is **opt-in** and **backwards-compatible**: with no key configured, nothing
changes — keys are stored and read as cleartext exactly as before.

## Scope

| Secret | Encrypted at rest? | Notes |
|---|---|---|
| `llm_providers.api_key` | ✅ when `XIAOGUAI_AT_REST_KEY` set | this runbook |
| `mcp_oauth_tokens.refresh_token` | n/a | only `InMemoryTokenStore` today — never written to disk. Uses `XIAOGUAI_MCP_OAUTH_TOKEN_KEY` once a `SqliteTokenStore` lands (`outbound-mcp-oauth.md`) |
| `messages.content` | ❌ deferred | full-text search / RAG recall / size trade-offs — a separate design decision |

The crypto primitive is shared (`xiaoguai-types::at_rest`, AES-256-GCM, 12-byte
random nonce, versioned envelope); each domain binds its own env-var names.

## Threat model

The key is supplied out-of-band via an environment variable and is never stored
in the database. An attacker with the DB file (and `-wal`/`-shm`) but not the
key cannot recover the provider keys. An attacker with **both** the DB and the
process environment can — at-rest encryption does not defend a fully
compromised host; it defends backups, snapshots, and shared-host file
permissions.

## Generate a key

A key is 32 random bytes, base64url-encoded:

```bash
# base64url, no padding
openssl rand 32 | basenc --base64url | tr -d '='
# or
python3 -c "import os,base64;print(base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip('='))"
```

## Enable encryption

1. Set the key in the server's environment (systemd drop-in, `.env`, secret
   manager — never commit it):

   ```bash
   XIAOGUAI_AT_REST_KEY=<32-byte-base64url-key>
   ```

2. Restart `xiaoguai serve`. On boot it runs an **idempotent backfill**
   (`backfill_encrypt_api_keys`) that seals any provider key still stored
   cleartext. The log line on success:

   ```
   INFO  encrypted pre-existing cleartext llm provider api keys at rest count=N
   ```

3. From then on, every key written via the web UI, the `provider` CLI, or the
   API is sealed before it touches disk. Reads transparently decrypt — callers
   (the LLM router, `doctor`) see plaintext and need no changes.

At rest a sealed value looks like `xgenc1:<base64url-envelope>`; a value
without the `xgenc1:` prefix is cleartext. That discriminator is how the opt-in
window and the pre-backfill state stay unambiguous.

## Rotate the key

1. Generate a fresh key. Move the **current** key into the `_PREV` slot and put
   the new key in the current slot:

   ```bash
   XIAOGUAI_AT_REST_KEY=<new-key>
   XIAOGUAI_AT_REST_KEY_PREV=<old-key>
   ```

2. Restart. New writes use the new key; existing ciphertext still decrypts
   against `_PREV`. The boot backfill only re-seals **cleartext** rows, so to
   force every row onto the new key, re-save the providers (UI/CLI) — or just
   leave `_PREV` set until they naturally update.

3. Once all rows are on the new key, drop `XIAOGUAI_AT_REST_KEY_PREV` and
   restart.

## Fail-safe behaviour (important)

Encryption is asymmetric: a wrong, missing, or corrupted key means a sealed
value cannot be opened. The repository **fails safe** rather than bricking:

- A row it cannot decrypt is treated as **no api key** (`None`) and logged at
  `error!` — that one provider becomes unauthenticated (and its upstream will
  reject calls), but boot succeeds and other providers are unaffected.
- It never returns the ciphertext as if it were a key, and it never silently
  swallows a misconfiguration.
- `xiaoguai doctor` surfaces a **malformed** `XIAOGUAI_AT_REST_KEY` as a failing
  check (`providers: XIAOGUAI_AT_REST_KEY is set but invalid: …`).

Practical consequence: **do not lose the key.** If you rotate or change it
without `_PREV`, the encrypted provider keys are unrecoverable and must be
re-entered. Back the key up alongside (but not inside) your DB backups.

## Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| Providers suddenly unauthenticated after setting a key | key differs from the one rows were sealed with | put the original key in `XIAOGUAI_AT_REST_KEY_PREV`, restart; or re-enter the keys |
| Boot log: `... backfill failed (non-fatal)` | DB write error during backfill | non-fatal; rows stay cleartext, retried next boot. Check disk/permissions |
| `doctor` reports `XIAOGUAI_AT_REST_KEY is set but invalid` | not 32 bytes base64url | regenerate per "Generate a key" |
| Want to confirm a key is sealed on disk | — | `sqlite3 <db> "SELECT substr(api_key,1,8) FROM llm_providers"` → `xgenc1:` prefix means sealed |

## Disabling

Unset `XIAOGUAI_AT_REST_KEY` and restart. New writes are cleartext again, but
**already-sealed rows stay sealed and will now read back as absent** (no key to
open them). To fully revert, re-enter each provider key while the key is unset
(overwriting the sealed value with cleartext), or keep the key configured.
