# Incident Triage Skill Pack

LLM-assisted root-cause analysis for on-call engineers: receives Sentry or
Datadog alerts via authenticated webhook, drafts an RCA from recent git
history and audit logs, and opens a draft GitHub PR with an IM notification.

---

## What it does

```
Sentry webhook  ─┐
                  ├─► normalize → common Incident schema
Datadog webhook ─┘
                         |
                   triage-agent
                     · recent_commits (git log, -4 h)
                     · audit_log (xiaoguai_audit_search)
                     · recent_deploys (git tags)
                     · LLM → RcaDraft JSON
                         |
                   draft-pr output
                     · render rca.md.j2 → .github/incidents/<id>.md
                     · open draft GitHub PR  [RCA Draft]
                     · IM message to TRIAGE_IM_CHANNEL
```

**Zero-merge policy.** The PR is always created as a draft; an engineer must
promote and merge it.

---

## Install

```bash
xiaoguai pack install packs/incident-triage/
```

Requires xiaoguai-api >= 0.12.1 (per-tenant webhook token middleware) plus
`llm` and at least one IM adapter (`feishu`, `dingtalk`, or `wecom`).

Activate the route family: set `XIAOGUAI_INCIDENTS_ENABLED=true` in your env.

---

## Inbound adapters

| Adapter | Auth | Optional HMAC |
|---|---|---|
| Sentry | Bearer token (per-tenant) | `X-Sentry-Hook-Signature` via `SENTRY_WEBHOOK_SECRET` |
| Datadog | Bearer token (per-tenant) | `DD-Signature` via `DATADOG_WEBHOOK_SECRET` |

Sentry actions `created`, `assigned`, `triggered` are processed; `resolved`
and others are acknowledged and dropped. Datadog `alert_type` values `error`,
`warning`, `info` are processed.

---

## Agents

| Agent | What it does |
|---|---|
| `triage-agent` | Gathers 4-hour context window (commits, audit log, deploys), calls LLM at temperature 0.2, emits a structured `RcaDraft` with summary, impact, root cause, timeline, action items, and confidence level |

---

## Outputs

- **Draft GitHub PR** — RCA markdown committed to `.github/incidents/`, PR
  labelled `incident`, `rca-draft`, `severity-<level>`.
- **IM notification** — short summary (≤ 500 chars) + PR URL sent to
  `TRIAGE_IM_CHANNEL`.

---

## Required env vars

| Var | Purpose |
|---|---|
| `XIAOGUAI_INCIDENTS_ENABLED` | Set `true` to activate the `/v1/incidents` route family |
| `TRIAGE_GITHUB_REPO` | Repository where RCA documents are committed |
| `TRIAGE_IM_CHANNEL` | IM channel/chat_id for notifications |

## Optional env vars

| Var | Default | Purpose |
|---|---|---|
| `SENTRY_WEBHOOK_SECRET` | — | Enable HMAC verification on Sentry payloads |
| `DATADOG_WEBHOOK_SECRET` | — | Enable HMAC verification on Datadog payloads |
| `TRIAGE_LLM_MODEL` | workspace default | Override the LLM model for the triage agent |
| `TRIAGE_GITHUB_BASE_BRANCH` | `main` | Target branch for draft PRs |

---

## Example trigger payload (Sentry)

```json
{
  "action": "created",
  "installation": {"uuid": "install-uuid"},
  "data": {
    "issue": {
      "id": "123",
      "title": "ZeroDivisionError: division by zero",
      "level": "error",
      "firstSeen": "2024-05-24T10:00:00.000Z",
      "permalink": "https://sentry.io/organizations/acme/issues/123/",
      "project": {"slug": "backend"},
      "tags": [{"key": "environment", "value": "production"}]
    }
  }
}
```

---

> **Operator notes**
>
> - **Feature flag** — the pack is inert until `XIAOGUAI_INCIDENTS_ENABLED=true`;
>   missing this env var is the most common "why is nothing happening" cause.
> - **Context window** — commits and audit-log entries are each truncated to
>   2000 chars; recent deploys to 500 chars. Very active repos may lose older
>   entries; adjust `max_lines: 50` in `triage-agent.yaml` if needed.
> - **Deferred** — PagerDuty on-call lookup, Slack/Teams output, and Jira
>   ticket creation are explicitly out of scope for v0.1.0.
