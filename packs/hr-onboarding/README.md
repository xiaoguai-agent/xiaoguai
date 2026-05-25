# HR Onboarding Skill Pack

Multi-agent automation for new-hire onboarding: a Supervisor coordinator
decomposes "onboard \<employee\>" into four parallel subtasks — account
provisioning, meeting scheduling, welcome messaging, and buddy assignment —
and delivers a completion report to the hiring manager.

---

## What it does

```
Daily cron  06:00  (employees WHERE start_date = today AND status = 'pending')
               |
    hr-onboarding-coordinator  (Supervisor)
      · single LLM planning turn (or static plan in tests)
      · marks employee status = 'onboarding' before dispatch
               |
    ┌──────────┴──────────┐─────────────────────┐
    A                     B                     C                   D
account-provisioner  meeting-scheduler  welcome-messenger  buddy-assigner
 Okta / Google /      Google Calendar    Feishu msg +        Round-robin
 GitHub (mocked)      (mocked)           group chat          buddy + DM
               |
    onboarding-report.md.j2  →  IM message to hiring manager
```

Worker failures are recorded in the audit log; the coordinator continues
remaining steps rather than aborting.

---

## Install

```bash
xiaoguai pack install packs/hr-onboarding/
```

Requires xiaoguai-orchestrator v1.1.5b+ (Supervisor/Worker traits),
xiaoguai-scheduler (cron trigger), xiaoguai-llm, and at least one IM adapter.

---

## Inbound adapters

| Trigger | Kind | Schedule |
|---|---|---|
| `start-date-trigger` | cron | `0 6 * * *` (daily at 06:00, `TZ` env var) |

No webhook inbound — the trigger polls the `employees` table directly.

---

## Agents

| Agent | What it does |
|---|---|
| `coordinator` | Supervisor: one LLM planning turn, then dispatches to four workers; 10-step budget, 5-minute wall-clock guard |
| `account-provisioner` | Worker A: creates Okta, Google Workspace, and GitHub accounts (mocked; writes to `onboarding_audit_log`) |
| `meeting-scheduler` | Worker B: schedules Day-1 orientation, manager 1:1, team intro, IT setup call (mocked; writes to `scheduled_meetings`) |
| `welcome-messenger` | Worker C: sends welcome email and Feishu group invite (real after C2-merge; mocked in tests) |
| `buddy-assigner` | Worker D: assigns onboarding buddy via configured strategy; notifies buddy by Feishu DM |

---

## Outputs

- **Onboarding report** — rendered from `outputs/onboarding-report.md.j2`,
  sent to the hiring manager via the configured IM adapter.
- **Audit rows** — every worker action written to `onboarding_audit_log`
  (and `scheduled_meetings` for the scheduler worker).

---

## Required env vars

| Var | Purpose |
|---|---|
| `FEISHU_APP_ID` | Feishu app ID (required when `im.adapter = feishu`) |
| `FEISHU_APP_SECRET` | Feishu app secret |
| `TZ` | Timezone for the cron trigger (default `Asia/Shanghai`) |

Real integrations (currently mocked) also need:
`OKTA_ORG_URL`, `OKTA_API_TOKEN`, `GOOGLE_SA_KEY_PATH`,
`GOOGLE_WORKSPACE_DOMAIN`, `GITHUB_ADMIN_TOKEN`.

---

## Optional config knobs (`pack.yaml` `config` section)

| Key | Default | Purpose |
|---|---|---|
| `im.adapter` | `feishu` | IM backend: `feishu` \| `dingtalk` \| `wecom` |
| `coordinator.max_steps` | `10` | Supervisor step budget |
| `coordinator.max_wall_seconds` | `300` | Wall-clock timeout per run |
| `buddy_pool.strategy` | `round_robin` | Buddy selection: `round_robin` \| `least_recently_assigned` |
| `audit.table` | `onboarding_audit_log` | Audit sink table name |

---

## Example trigger event

```sql
-- An employee row that fires the trigger on their start date:
INSERT INTO employees (id, name, email, manager_id, start_date, status)
VALUES ('EMP-042', 'Alice Chen', 'alice@acme.com', 'MGR-007',
        CURRENT_DATE, 'pending');
-- The cron fires at 06:00, sets status='onboarding', dispatches the Supervisor.
```

---

> **Operator notes**
>
> - **Idempotency guard** — `pre_dispatch_sql` flips `status = 'onboarding'`
>   atomically before dispatch; a cron restart will not double-onboard the
>   same employee.
> - **Partial onboarding** — if a worker fails (e.g. Feishu API down), the
>   coordinator logs the failure and continues; the report flags which steps
>   succeeded and which did not.
> - **Mocked vs real** — all external integrations (Okta, Google, GitHub) are
>   mocked by default (`real_integrations: false`). Feishu send is real after
>   C2-merge; tests swap in `MockImAdapter`.
> - **Cron timezone** — the trigger uses server local time by default
>   (`TZ=Asia/Shanghai`). Set `TZ` in your `.env` to match your office locale.
