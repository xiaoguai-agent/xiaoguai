# xg skills *(planned for v1.3)*

> **Implementation status**: The skill-pack marketplace is fully implemented in
> `crates/xiaoguai-api/src/skills.rs` (v1.2.28) and exposes four REST endpoints
> under `/v1/skills/`. The catalog (`catalog/skill_packs.json`) is baked into
> the binary at compile time. The `xg skills` CLI wrapper does **not yet exist**;
> this page describes the intended interface grounded in `SkillPackEntry`,
> `InstalledPackRow`, and `InstallRequest` from the actual source.
>
> Note: `install-from-file` (local YAML/JSON pack definition) is **not in the
> current API** — it is planned for v1.3 alongside the pack hot-reload loader.

## SYNOPSIS

```
xg skills [GLOBAL-FLAGS] <SUBCMD> [SUBCMD-FLAGS] [ARGS]
```

## DESCRIPTION

`xg skills` manages the skill-pack marketplace for a Xiaoguai deployment. Skill
packs are pre-built, curated agent configurations that bundle prompts, MCP tool
requirements, and operator-tuneable knobs for a specific business domain
(e.g. `ar-collections`, `incident-triage`, `deal-desk`). Installing a pack
records a row in `installed_skill_packs`; pack runtime hot-reload lands in v1.3.

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
| `list` | List packs (catalog or installed) |
| `install` | Install a catalog pack for a tenant |
| `install-from-file` | Install a local pack definition *(planned for v1.3)* |
| `uninstall` | Uninstall a pack by installed-row id |

---

### xg skills list

```
xg skills list [--tenant-id <ID>] [--category <CAT>] [--installed]
```

Without `--installed`, lists the static catalog baked into the binary. With
`--installed`, lists packs currently installed for the given tenant.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | no* | Tenant to filter; required when `--installed` is set |
| `--category <CAT>` | no | Filter catalog by category: `finance`, `ops`, `dev`, `hr`, `rag` |
| `--installed` | no | Show installed packs instead of full catalog |

**Example — browse catalog by category:**

```
$ xg skills list --category finance

SLUG              NAME                      VERSION  CATEGORY  DESCRIPTION
ar-collections    AR Collections Assistant  1.0.0    finance   Automates AR workflows…
deal-desk         Deal Desk Assistant       1.0.0    finance   Drafts proposals, tracks…
```

**Example — list installed packs for tenant `acme`:**

```
$ xg skills list --tenant-id acme --installed

ID                PACK_SLUG        VERSION  INSTALLED_AT
inst_01hzabcd01   ar-collections   1.0.0    2026-05-20T10:00:00Z
inst_01hzabcd02   incident-triage  1.0.0    2026-05-22T08:30:00Z
```

---

### xg skills install

```
xg skills install --tenant-id <ID> --pack <SLUG> [--config <JSON>]
```

Installs a pack from the catalog for a tenant. Returns `AlreadyInstalled` if
the pack is already installed for the same tenant (the API enforces a
`UNIQUE (tenant_id, pack_slug)` constraint).

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Target tenant |
| `--pack <SLUG>` | yes | Catalog slug (see `xg skills list`) |
| `--config <JSON>` | no | Operator knob overrides as inline JSON (e.g. `'{"overdue_days_threshold":45}'`) |

**Example — install AR collections with custom overdue threshold:**

```
$ xg skills install \
    --tenant-id acme \
    --pack ar-collections \
    --config '{"overdue_days_threshold": 45, "reminder_tone": "firm"}'

id: inst_01hzabcd03
tenant_id: acme
pack_slug: ar-collections
version: 1.0.0
installed_at: 2026-05-25T12:00:00Z
```

**Example — install incident-triage with defaults:**

```
$ xg skills install --tenant-id acme --pack incident-triage

id: inst_01hzabcd04
tenant_id: acme
pack_slug: incident-triage
version: 1.0.0
installed_at: 2026-05-25T12:01:00Z
```

---

### xg skills install-from-file *(planned for v1.3)*

```
xg skills install-from-file --tenant-id <ID> --file <PATH>
```

Installs a locally authored pack definition (YAML or JSON) that is not in
the catalog. The file must conform to the `SkillPackEntry` schema. Pack
hot-reload (making the pack active without a server restart) is also planned
for v1.3.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | yes | Target tenant |
| `--file <PATH>` | yes | Path to local `.yaml` or `.json` pack definition |

This subcommand is **not implemented** in the current API. Attempting to call
it will exit with code 2 and a message explaining that it is planned for v1.3.

---

### xg skills uninstall

```
xg skills uninstall --id <INSTALLED-ROW-ID>
```

Soft-deletes an installed-pack row. Does not hot-unload the pack from a
running server (restart required until v1.3 hot-reload lands).

| Flag | Required | Description |
|------|:--------:|-------------|
| `--id <INSTALLED-ROW-ID>` | yes | Row id from `xg skills list --installed` |

**Example:**

```
$ xg skills uninstall --id inst_01hzabcd03

{"ok": true}
```

## EXIT CODES

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic error (network, auth, catalog parse failure) |
| 2 | Invalid arguments or unimplemented subcommand |
| 64 | Pack slug not found in catalog |
| 66 | Pack already installed for this tenant |

## SEE ALSO

- REST API: `GET /v1/skills/catalog`, `GET /v1/skills/installed`, `POST /v1/skills/install`, `DELETE /v1/skills/install/:id`
- Source: `crates/xiaoguai-api/src/skills.rs`, `catalog/skill_packs.json`
- [Skills catalog overview](../../book/src/skills/overview.md)
