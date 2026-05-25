# xg outcomes *(planned for v1.3)*

> **Implementation status**: The outcome telemetry layer is fully implemented
> in `crates/xiaoguai-audit/src/outcomes.rs` and exposed over REST at
> `POST /v1/outcomes`, `GET /v1/outcomes/summary`, and
> `GET /v1/outcomes/timeseries` (v1.2.4). The `xg outcomes` CLI wrapper does
> **not yet exist**; this page describes the intended interface grounded in
> the actual `OutcomeRecord`, `OutcomeSummary`, and `OutcomeDay` types.

## SYNOPSIS

```
xg outcomes [GLOBAL-FLAGS] <SUBCMD> [SUBCMD-FLAGS] [ARGS]
```

## DESCRIPTION

`xg outcomes` manages agent outcome telemetry — the "revenue, not time" ROI
tracking layer. Agents call `record` after completing a task that produced
measurable business value. The `summary` and `timeseries` subcommands drive
the admin-ui Outcomes pane and can be used directly from CI or ops scripts.

Six first-class outcome kinds are supported: `revenue_usd`, `cost_saved_usd`,
`hours_saved`, `deals_closed`, `tickets_resolved`, and `custom`.

## GLOBAL FLAGS

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--config <PATH>` | `XIAOGUAI_CONFIG` | `~/.xiaoguai/config.yaml` | YAML config file |
| `--token <TOKEN>` | `XIAOGUAI_API_TOKEN` | — | Bearer token |
| `--api-base <URL>` | `XIAOGUAI_API_BASE` | `http://localhost:8080` | API server base URL |
| `--output <FORMAT>` | — | `table` | `json` \| `yaml` \| `table` |

## SUBCOMMANDS

| Subcommand | Description |
|-----------|-------------|
| `record` | Record one outcome attribution for a tenant/session |
| `list` | List raw outcome records for a tenant within a time range |
| `summary` | Aggregated ROI summary (one row per outcome kind) |
| `timeseries` | Day-by-day breakdown of outcome values |

---

### xg outcomes record

```
xg outcomes record --tenant-id <ID> --agent-name <NAME>
    --kind <KIND> --value <F>
    [--session-id <ID>] [--unit <STR>] [--description <STR>]
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Tenant the outcome is attributed to |
| `--agent-name <NAME>` | yes | Name of the agent (e.g. `ar-collections`) |
| `--kind <KIND>` | yes | One of: `revenue_usd`, `cost_saved_usd`, `hours_saved`, `deals_closed`, `tickets_resolved`, `custom` |
| `--value <F>` | yes | Non-negative numeric value |
| `--session-id <ID>` | no | Session id for attribution (optional traceability) |
| `--unit <STR>` | no | Unit label for `custom` kind (e.g. `"leads"`) |
| `--description <STR>` | no | Human-readable note attached to the record |

**Example — record $1,200 revenue after a deal-close session:**

```
$ xg outcomes record \
    --tenant-id acme \
    --agent-name sales-assist \
    --kind revenue_usd \
    --value 1200.00 \
    --session-id ses_01hzabcd \
    --description "Q2 deal: Acme → Contoso"

ok: true
```

**Example — record 3 hours of analyst time saved:**

```
$ xg outcomes record \
    --tenant-id acme \
    --agent-name report-writer \
    --kind hours_saved \
    --value 3.0

ok: true
```

---

### xg outcomes list

```
xg outcomes list --tenant-id <ID> [--range <RANGE>] [--kind <KIND>]
    [--limit <N>]
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Filter by tenant |
| `--range <RANGE>` | no | Time range shorthand: `24h`, `7d`, `30d` (default: `30d`) |
| `--kind <KIND>` | no | Filter by outcome kind |
| `--limit <N>` | no | Maximum rows to return (default: 100) |

**Example:**

```
$ xg outcomes list --tenant-id acme --range 7d --output table

RECORDED_AT           AGENT          KIND          VALUE    SESSION
2026-05-25T09:12:00Z  sales-assist   revenue_usd   1200.00  ses_01hzabcd
2026-05-24T14:30:00Z  report-writer  hours_saved      3.00  -
```

---

### xg outcomes summary

```
xg outcomes summary --tenant-id <ID> [--range <RANGE>]
```

Returns one aggregated row per outcome kind: total, count, and average.
Backs the four ROI cards in the admin-ui Outcomes pane.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Tenant to summarise |
| `--range <RANGE>` | no | `24h` \| `7d` \| `30d` (default: `30d`) |

**Example:**

```
$ xg outcomes summary --tenant-id acme --range 30d

KIND            TOTAL       COUNT   AVG
revenue_usd     $18,400.00  12      $1,533.33
cost_saved_usd  $2,100.00   7       $300.00
hours_saved     47.5 h      18      2.64 h
deals_closed    9           9       1
tickets_resolved 34         34      1
```

---

### xg outcomes timeseries

```
xg outcomes timeseries --tenant-id <ID> [--range <RANGE>] [--kind <KIND>]
```

Returns a day-by-day breakdown of outcome totals. Each row is one
`OutcomeDay` bucket (date + per-kind value map).

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Tenant to query |
| `--range <RANGE>` | no | `24h` \| `7d` \| `30d` (default: `30d`) |
| `--kind <KIND>` | no | Restrict to one outcome kind for a single-series chart |

**Example — 7-day revenue timeseries:**

```
$ xg outcomes timeseries --tenant-id acme --range 7d --kind revenue_usd

DATE        revenue_usd
2026-05-19  2400.00
2026-05-20     0.00
2026-05-21  1200.00
2026-05-22  3600.00
2026-05-23     0.00
2026-05-24  1200.00
2026-05-25  9000.00
```

**Example — full timeseries as JSON for a dashboard script:**

```
$ xg outcomes timeseries --tenant-id acme --range 7d --output json | jq '.days[] | .date'
"2026-05-19"
"2026-05-20"
...
```

## EXIT CODES

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic error (network, auth failure) |
| 2 | Invalid arguments (negative value, unknown kind, unrecognised range shorthand) |
| 64 | Tenant not found |

## SEE ALSO

- REST API: `POST /v1/outcomes`, `GET /v1/outcomes/summary`, `GET /v1/outcomes/timeseries`
- Source: `crates/xiaoguai-audit/src/outcomes.rs`, `crates/xiaoguai-api/src/outcomes.rs`
- Admin-ui Outcomes pane (ROI dashboard)
