# IM adapter onboarding — wave-3

Adding a new Discord, Telegram, Mattermost, or Slack adapter instance.
For Feishu, DingTalk, and WeCom see `docs/plans/2026-05-24-v1.1.3.md`.

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`)
> with an embedded SQLite database — no Kubernetes, no Helm, no external
> datastore. Secrets are delivered as `XIAOGUAI_*` environment variables
> through a systemd drop-in (or a `chmod 600` env file), not a Kubernetes
> Secret. Operate the process with `systemctl` / `journalctl` and inspect
> state with `sqlite3 ~/.xiaoguai/data.db` (under systemd:
> `/var/lib/xiaoguai/data.db`). There is a single implicit **owner** — no
> tenants.

---

## Supported adapters (wave-3)

| Adapter | Inbound mechanism | Signature scheme |
|---|---|---|
| **Discord** | Interactions webhook | Ed25519 (`X-Signature-Ed25519` + `X-Signature-Timestamp`) |
| **Telegram** | Webhook or long-poll | `X-Telegram-Bot-Api-Secret-Token` (HMAC-SHA256) |
| **Mattermost** | Outgoing webhook or slash command | Shared token (constant-time compare) |
| **Slack** | Events API or Socket Mode | HMAC-SHA256 of `v0:timestamp:body` |

---

## Pre-checks

Before touching env vars, confirm:

- [ ] Bot token obtained from the platform developer portal
- [ ] Webhook secret / signing secret generated (min 16 random bytes;
      `openssl rand -hex 32` is convenient)
- [ ] Webhook URL reachable from the platform's IP ranges — verify your
      firewall allows inbound HTTPS from the respective platform's CIDR
      block
- [ ] Channel ID (not channel name) confirmed — most adapters take the
      raw numeric or snowflake ID
- [ ] Bot has the minimum required permissions:
  - Discord: `applications.commands`, `Send Messages`
  - Telegram: none beyond the bot token
  - Mattermost: `post:channel`, `read_channel`
  - Slack: `chat:write`, `channels:read`, `app_mentions:read`

---

## Configure: environment variables

Secrets are passed as environment variables, never committed to
`config.yaml`. Put them in a systemd drop-in (`chmod 600`):

```ini
# /etc/systemd/system/xiaoguai-core.service.d/im.conf
[Service]
EnvironmentFile=/etc/xiaoguai/im.env
```

…with the secrets in `/etc/xiaoguai/im.env` (`chmod 600`, owned by the
`xiaoguai` service user). After editing, `systemctl daemon-reload &&
systemctl restart xiaoguai-core`. For a non-systemd / dev run, `export`
the same variables in the shell that launches `xiaoguai serve`.

### Discord

```sh
# /etc/xiaoguai/im.env
XIAOGUAI_IM_DISCORD_BOT_TOKEN="Bot YOUR_TOKEN"
XIAOGUAI_IM_DISCORD_PUBLIC_KEY="YOUR_APP_PUBLIC_KEY_HEX"
XIAOGUAI_IM_DISCORD_REPLY_CHANNEL_ID="CHANNEL_SNOWFLAKE_ID"
```

The adapter mounts at `POST /v1/im/discord/webhook`. Register this URL
in the Discord Developer Portal → General Information → Interactions
Endpoint URL.

### Telegram

```sh
# /etc/xiaoguai/im.env
XIAOGUAI_IM_TELEGRAM_BOT_TOKEN="123456:ABC-your-token"
XIAOGUAI_IM_TELEGRAM_WEBHOOK_SECRET="your-webhook-secret"
```

Register the webhook with Telegram:

```bash
curl -s "https://api.telegram.org/bot$BOT_TOKEN/setWebhook" \
  -d "url=https://your-domain.example.com/v1/im/telegram/webhook" \
  -d "secret_token=$WEBHOOK_SECRET"
# → {"ok":true,"result":true,"description":"Webhook was set"}
```

For long-poll mode (behind NAT, dev only) set
`XIAOGUAI_IM_TELEGRAM_LONG_POLL=true` instead of registering a webhook.

### Mattermost

```sh
# /etc/xiaoguai/im.env
XIAOGUAI_IM_MATTERMOST_BOT_TOKEN="your-bot-access-token"
XIAOGUAI_IM_MATTERMOST_WEBHOOK_TOKEN="your-outgoing-webhook-token"
XIAOGUAI_IM_MATTERMOST_BASE_URL="https://mm.example.com"
```

In the Mattermost admin panel: Integrations → Outgoing Webhooks →
Add → set Callback URL to
`https://your-domain.example.com/v1/im/mattermost/webhook`.

For slash commands: Integrations → Slash Commands →
`POST /v1/im/mattermost/slash`.

### Slack

```sh
# /etc/xiaoguai/im.env
XIAOGUAI_IM_SLACK_BOT_TOKEN="xoxb-your-bot-token"
XIAOGUAI_IM_SLACK_SIGNING_SECRET="your-signing-secret"
```

In the Slack App manifest, set Request URL to
`https://your-domain.example.com/v1/im/slack/webhook`.

For Socket Mode (dev, behind NAT) add the app-level token and flag:

```sh
XIAOGUAI_IM_SLACK_APP_TOKEN="xapp-your-app-level-token"
XIAOGUAI_IM_SLACK_SOCKET_MODE=true
```

---

## Deploy

```bash
# Reload the unit so the new EnvironmentFile is picked up, then restart:
systemctl daemon-reload
systemctl restart xiaoguai-core

# Verify the service is healthy:
systemctl status xiaoguai-core --no-pager
xiaoguai smoke
```

---

## Verify

```bash
# Discord PING (type=1) round-trip:
curl -s -X POST \
  "http://localhost:8080/v1/im/discord/webhook" \
  -H "Content-Type: application/json" \
  -H "X-Signature-Ed25519: <computed-sig>" \
  -H "X-Signature-Timestamp: $(date +%s)" \
  -d '{"type":1}'
# → {"type":1}  (PONG)

# Confirm the audit log shows an inbound event:
sqlite3 ~/.xiaoguai/data.db "
  SELECT action, json_extract(details,'\$.adapter') AS adapter, ts
  FROM audit_log
  WHERE action LIKE 'im.%'
  ORDER BY ts DESC LIMIT 5;"

# Or tail the live logs while you send a real message:
journalctl -u xiaoguai-core -f | grep -i im
```

---

## Common pitfalls

**Webhook signature mismatch (Discord / Slack)**

Discord and Slack reject any request where the computed signature does
not match the `X-Signature-*` headers. Common causes:

- Bot token or signing secret copied with a trailing newline or space.
  Strip with: `echo -n "$SECRET" | xxd | head`.
- Webhook URL registered with HTTP instead of HTTPS.
- Reverse proxy stripping or rewriting the body before xiaoguai reads it
  (e.g. nginx body buffer or gzip re-encoding). Set
  `proxy_request_buffering off` in nginx.

**Channel ID typos (Discord)**

Discord uses integer snowflakes for channel IDs; channel names are not
accepted. Copy the ID from Discord Developer Mode (right-click → Copy
Channel ID).

**Telegram "Webhook was set" but no events arriving**

- The domain TLS certificate must be trusted by Telegram's CA set
  (Let's Encrypt is accepted; self-signed is not).
- Confirm the `secret_token` in the `setWebhook` call exactly matches
  `XIAOGUAI_IM_TELEGRAM_WEBHOOK_SECRET` — case-sensitive, no trailing
  slash.

**Mattermost outgoing webhook token not verified**

If you leave `XIAOGUAI_IM_MATTERMOST_WEBHOOK_TOKEN` empty, the adapter
skips verification and accepts all POSTs. This is acceptable in a
trusted-network deployment but should be treated as a misconfiguration
in internet-facing deployments.

**Slack rate limits (tier 1 — 1 req/min per method)**

The `chat.write` method is Tier 3 (50 req/min), but `conversations.list`
is Tier 2 (20 req/min). If the bot fetches channel info frequently,
cache the results or use `channels:read` with a long TTL.

---

## Postmortem checklist

- [ ] Signature verified end-to-end (PING/PONG round-trip passes)
- [ ] Channel ID confirmed correct (not the channel name)
- [ ] Secrets stored in a `chmod 600` env file / systemd drop-in, not in
      `config.yaml`
- [ ] Audit log shows inbound events after the first real message
- [ ] Rate limit tier confirmed for all API methods used
