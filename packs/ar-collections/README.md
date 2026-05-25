# AR Collections Skill Pack

Continuously watch accounts-receivable aging and flag deteriorating
receivables **before** anyone opens the PDF.

This is the reference "Sivulka scenario" implementation: a declarative,
platform-native pack that wires a SQL watch, a rolling-window anomaly
detector, and a HOTL-gated dunning-drafter agent from a single install
command.

---

## What it does

```
ar_aging table
     |
     +-- [every 15 min] dso-over-60 watch ──────► WatchEvent ar.dso_over_60
     |                                                   |
     +-- [every 08:00]  dso-drift anomaly ─────► AnomalyEvent ar.dso_drift
                                                         |
                                               dunning-drafter agent
                                                         |
                                           1. lookup_customer (SQL tool)
                                           2. get_prior_dunning_history
                                           3. select tier (1st/2nd/final)
                                           4. draft_email (Jinja template)
                                           5. save_draft → ar_dunning_log
                                                         |
                                              status = pending_approval
                                                         |
                                              Human approves in admin UI
                                                         |
                                                    email sent
```

**Zero autonomous sends.** The agent writes every draft to `ar_dunning_log`
with `status = 'pending_approval'` and halts. A human must approve before
anything reaches a customer.

---

## Install

```bash
# From the xiaoguai workspace root
xiaoguai pack install packs/ar-collections/

# This applies migration 0001_ar_aging.sql, registers the watch + anomaly
# specs into their respective registries, and boots the dunning-drafter agent.
```

Requires xiaoguai >= v1.3.1 with features: `watch`, `anomaly`, `llm`,
`outcome-telemetry`.

---

## Configure thresholds

Override defaults in `pack-config.yaml` (optional):

```yaml
pack: ar-collections

watches:
  dso-over-60:
    # Change the overdue threshold from 60 to 45 days
    query_override:
      days_threshold: 45
    schedule:
      cron: "*/10 * * * *"   # check every 10 minutes instead of 15

anomalies:
  dso-drift:
    alert:
      n_sigma: 2.0            # alert earlier (less sensitive)
      absolute_threshold:
        value: 60             # days (tighter absolute cap)

agents:
  dunning-drafter:
    hotl:
      notify_channel: "dingtalk"  # or "wecom", "feishu", "admin_ui"
    rate_limits:
      per_customer_per_day: 1
```

Apply:

```bash
xiaoguai pack configure packs/ar-collections/ --config pack-config.yaml
```

---

## Demo flow

1. Seed fixture data:

```sql
INSERT INTO ar_aging (tenant_id, customer_id, invoice_id, amount, due_date)
VALUES
  ('acme', 'CUST-001', 'INV-2026-0042', 2500.00, NOW() - INTERVAL '75 days'),
  ('acme', 'CUST-001', 'INV-2026-0055', 1700.00, NOW() - INTERVAL '68 days'),
  ('acme', 'CUST-002', 'INV-2026-0071', 900.00,  NOW() - INTERVAL '61 days');
```

2. Force a watch tick:

```bash
xiaoguai watch tick dso-over-60 --tenant acme
```

3. Observe dunning drafts (admin UI or SQL):

```sql
SELECT customer_id, tier, status, LEFT(draft_body, 80) AS preview
FROM ar_dunning_log
WHERE tenant_id = 'acme'
ORDER BY created_at DESC;
```

4. Approve a draft:

```bash
xiaoguai draft approve <draft_id>
# → status flips to 'approved', email dispatched via configured channel
```

---

## File layout

```
packs/ar-collections/
├── pack.yaml                      Pack manifest (version, features, schema)
├── migrations/
│   └── 0001_ar_aging.sql          ar_aging + ar_dunning_log tables + indexes
├── watches/
│   └── dso-over-60.yaml           WatchSpec: SQL on ar_aging, 15-min cron
├── anomalies/
│   └── dso-drift.yaml             AnomalySpec: rolling 30-day DSO z-score
├── agents/
│   └── dunning-drafter.yaml       Agent: tools, prompts, HOTL guardrails
├── templates/
│   ├── email-dunning-1st.md.j2    Friendly first reminder
│   ├── email-dunning-2nd.md.j2    Firmer second notice
│   └── email-final.md.j2          Final notice (escalation warning)
└── tests/
    └── integration.rs             Spec parse + template render tests
                                   (DB + F1/F2 tests marked #[ignore])
```

---

## What works today vs. what is gated

| Capability | Status |
|---|---|
| Pack manifest parse | Working |
| Watch spec parse + round-trip | Working |
| Anomaly spec parse + round-trip | Working |
| Jinja template rendering (all 3 tiers) | Working (Tera) |
| `packs.rs` stub loader | Working (feature-gated `cfg(feature = "packs")`) |
| DB migration apply | Working SQL (applied by `xiaoguai pack install`) |
| Watch tick + WatchEvent emission | Gated on F1 merge |
| Anomaly rolling baseline + alert | Gated on F2 merge |
| Agent dunning loop (end-to-end) | Gated on F1 + F2 merge |
| Outcome telemetry loop | Gated on F3 merge |
| Admin-UI dashboard widgets | Gated on pack-UI admin surface |

---

## Deferred (out of scope for v1.3.1-prep)

- **Multi-currency**: all amounts stored with a `currency` column; template
  rendering passes the currency string through. Conversion to a single
  reporting currency (e.g. USD equivalent) requires an FX rate feed — deferred.
- **Real-world template tuning**: the 3 templates are production-quality
  scaffolds. Tone, legal language per jurisdiction, and branding require
  operator review before live use.
- **Dispute workflow**: the agent detects disputed invoices and flags them,
  but the dispute-resolution sub-flow (credit notes, partial payment plans)
  is a separate pack (`ar-disputes`, planned for v1.4).
- **ERP connector**: ingesting ar_aging rows from SAP / NetSuite / QuickBooks
  via a CDC connector is a separate integration concern.
