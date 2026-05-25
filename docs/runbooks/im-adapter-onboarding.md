# IM adapter onboarding — wave-3

Adding a new Discord, Telegram, Mattermost, or Slack adapter instance.
For Feishu, DingTalk, and WeCom see `docs/plans/2026-05-24-v1.1.3.md`.

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

Before touching env vars or Helm, confirm:

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

All secrets go in Kubernetes Secrets, never in `config.yaml` or Helm
`values.yaml`.

### Discord

```bash
kubectl create secret generic xiaoguai-im-discord \
  --from-literal=XIAOGUAI_IM_DISCORD_BOT_TOKEN="Bot YOUR_TOKEN" \
  --from-literal=XIAOGUAI_IM_DISCORD_PUBLIC_KEY="YOUR_APP_PUBLIC_KEY_HEX" \
  --from-literal=XIAOGUAI_IM_DISCORD_REPLY_CHANNEL_ID="CHANNEL_SNOWFLAKE_ID"
```

Helm `values.yaml` addition:

```yaml
envFrom:
  - secretRef:
      name: xiaoguai-im-discord
```

The adapter mounts at `POST /v1/im/discord/webhook`. Register this URL
in the Discord Developer Portal → General Information → Interactions
Endpoint URL.

### Telegram

```bash
kubectl create secret generic xiaoguai-im-telegram \
  --from-literal=XIAOGUAI_IM_TELEGRAM_BOT_TOKEN="123456:ABC-your-token" \
  --from-literal=XIAOGUAI_IM_TELEGRAM_WEBHOOK_SECRET="your-webhook-secret"
```

Register the webhook with Telegram:

```bash
curl -s "https://api.telegram.org/bot$BOT_TOKEN/setWebhook" \
  -d "url=https://your-domain.example.com/v1/im/telegram/webhook" \
  -d "secret_token=$WEBHOOK_SECRET"
# → {"ok":true,"result":true,"description":"Webhook was set"}
```

For long-poll mode (behind NAT, dev only):

```bash
# Do NOT register a webhook — set this env var instead:
XIAOGUAI_IM_TELEGRAM_LONG_POLL=true
```

### Mattermost

```bash
kubectl create secret generic xiaoguai-im-mattermost \
  --from-literal=XIAOGUAI_IM_MATTERMOST_BOT_TOKEN="your-bot-access-token" \
  --from-literal=XIAOGUAI_IM_MATTERMOST_WEBHOOK_TOKEN="your-outgoing-webhook-token" \
  --from-literal=XIAOGUAI_IM_MATTERMOST_BASE_URL="https://mm.example.com"
```

In the Mattermost admin panel: Integrations → Outgoing Webhooks →
Add → set Callback URL to
`https://your-domain.example.com/v1/im/mattermost/webhook`.

For slash commands: Integrations → Slash Commands →
`POST /v1/im/mattermost/slash`.

### Slack

```bash
kubectl create secret generic xiaoguai-im-slack \
  --from-literal=XIAOGUAI_IM_SLACK_BOT_TOKEN="xoxb-your-bot-token" \
  --from-literal=XIAOGUAI_IM_SLACK_SIGNING_SECRET="your-signing-secret"
```

In the Slack App manifest, set Request URL to
`https://your-domain.example.com/v1/im/slack/webhook`.

For Socket Mode (dev, behind NAT):

```bash
kubectl create secret generic xiaoguai-im-slack \
  --from-literal=XIAOGUAI_IM_SLACK_BOT_TOKEN="xoxb-your-bot-token" \
  --from-literal=XIAOGUAI_IM_SLACK_SIGNING_SECRET="your-signing-secret" \
  --from-literal=XIAOGUAI_IM_SLACK_APP_TOKEN="xapp-your-app-level-token"
# Set XIAOGUAI_IM_SLACK_SOCKET_MODE=true
```

---

## Deploy

```bash
# Apply the secret + roll the deployment:
kubectl apply -f xiaoguai-im-<adapter>-secret.yaml
helm upgrade xiaoguai deploy/helm/xiaoguai --reuse-values

# Verify pods are healthy:
kubectl rollout status deploy/xiaoguai --timeout=120s
kubectl exec deploy/xiaoguai -- /usr/local/bin/xiaoguai-core smoke
```

---

## Verify

```bash
# Send a test message via the adapter's webhook endpoint:
xg im test discord   # Discord ping round-trip
xg im test telegram  # Telegram sendMessage echo
xg im test mattermost
xg im test slack

# If the xg CLI is not available, hit the endpoint directly:
# Discord PING (type=1):
curl -s -X POST \
  "http://xiaoguai-core.svc:8080/v1/im/discord/webhook" \
  -H "Content-Type: application/json" \
  -H "X-Signature-Ed25519: <computed-sig>" \
  -H "X-Signature-Timestamp: $(date +%s)" \
  -d '{"type":1}'
# → {"type":1}  (PONG)

# Confirm the audit log shows an inbound event:
psql "$DATABASE_URL" -c "
  SELECT action, details->>'adapter' AS adapter, created_at
  FROM audit_log
  WHERE action LIKE 'im.%'
  ORDER BY created_at DESC LIMIT 5;"
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

- [ ] Signature verified end-to-end (`xg im test` passes)
- [ ] Channel ID confirmed correct (not the channel name)
- [ ] Secrets stored in Kubernetes Secret, not in config files or Helm values
- [ ] Audit log shows inbound events after the first real message
- [ ] Rate limit tier confirmed for all API methods used
