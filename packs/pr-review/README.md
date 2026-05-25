# PR Review Skill Pack

Automated GitHub pull request review for engineering teams: a two-agent
Reviewer + Challenger pipeline triggered by webhook, posting inline comments
back to GitHub.

---

## What it does

```
GitHub PR opened/synchronize
        |
  github-pr-webhook  (HMAC-SHA256 verified)
        |
   reviewer agent  ── get_pr_diff ──► inline comment JSON
        |
  challenger agent ──────────────────► verdict + supplements
        |
   post-review output
        |
   GitHub PR review  (CHANGES_REQUESTED | COMMENT | APPROVE)
```

**Challenger gating.** If the challenger returns `Reject`, the review is
suppressed and an audit entry is written instead.

---

## Install

```bash
xiaoguai pack install packs/pr-review/
```

Requires xiaoguai >= v1.2.0 with features: `webhook`, `llm`, `github-mcp`.

---

## Inbound adapters

| Adapter | Auth | Endpoint |
|---|---|---|
| GitHub pull_request webhook | HMAC-SHA256 (`X-Hub-Signature-256`) via `GITHUB_WEBHOOK_SECRET` | `POST /v1/scheduler/webhooks/github-pr-<PACK_INSTANCE_ID>` |

Fires on actions `opened` and `synchronize` only; other actions are
acknowledged with HTTP 200 and dropped.

---

## Agents

| Agent | What it does |
|---|---|
| `reviewer` | Fetches PR diff via `github_pr` MCP server; emits up to 20 inline comment objects (functional, style, test-coverage findings) |
| `challenger` | Critiques the reviewer output for gaps and unstated assumptions; returns `Accept / Revise / Reject` verdict |

---

## Outputs

- **GitHub PR review** — inline comments merged from reviewer + challenger
  supplements; event type mapped from max severity (`blocker/major` →
  `CHANGES_REQUESTED`, `minor/nit` → `COMMENT`, empty → `APPROVE`).
- **Audit row** written to `pack_pr_review_runs` for every run, including
  suppressed ones.

---

## Required env vars

| Var | Purpose |
|---|---|
| `GITHUB_TOKEN` | PAT or GitHub App installation token with `repo:write` |
| `GITHUB_WEBHOOK_SECRET` | HMAC-SHA256 shared secret configured in the GitHub repo |

---

## Example trigger payload

```json
{
  "action": "opened",
  "number": 42,
  "pull_request": {
    "title": "feat: add rate limiter",
    "head": {"sha": "abc123"},
    "base": {"sha": "def456"},
    "html_url": "https://github.com/acme/api/pull/42",
    "diff_url": "https://github.com/acme/api/pull/42.diff"
  },
  "repository": {"name": "api", "owner": {"login": "acme"}}
}
```

---

> **Operator notes**
>
> - **Challenger Reject suppression** — when the challenger rejects the
>   reviewer output no review is posted; check `pack_pr_review_runs.suppressed`
>   to diagnose silent runs.
> - **F5 Challenger middleware (v1.2)** — when the orchestrator Challenger
>   middleware lands, remove the explicit `challenge` plan step and set
>   `challenger.middleware: orchestrator/challenger` in `pack.yaml`.
> - **Budget** — reviewer capped at 3 LLM calls / 4096 tokens; challenger at
>   2 calls / 2048 tokens. Large diffs may produce truncated output.
