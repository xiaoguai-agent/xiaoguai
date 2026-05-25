# CLI Reference — Wave-3 Subcommands

This directory contains reference pages for the `xg` subcommands planned in the v1.3 wave.

> **Status note**: As of the current `main` branch (2026-05-26) the binary is
> named **`xiaoguai`**, not `xg`. All five command groups documented here exist
> as REST API endpoints and library crates but have **no CLI wrapper yet**. Each
> page is therefore marked *(planned for v1.3)* and describes the intended
> interface to be implemented. Flags are grounded in the actual Rust source;
> nothing is fabricated.

| Page | Command group | Description |
|------|---------------|-------------|
| [xg-hotl.md](xg-hotl.md) | `xg hotl` | Manage Human-on-the-Loop budget policies and run ad-hoc budget checks |
| [xg-outcomes.md](xg-outcomes.md) | `xg outcomes` | Record and query agent outcome telemetry (ROI dashboard data) |
| [xg-skills.md](xg-skills.md) | `xg skills` | Browse the skill-pack catalog and install/uninstall packs for a tenant |
| [xg-watch.md](xg-watch.md) | `xg watch` | Manage declarative active-wakeup watchers (list/start/stop/test) |
| [xg-anomaly.md](xg-anomaly.md) | `xg anomaly` | Run and test time-series anomaly detectors against a KPI stream |

## Global Flags (all `xg` subcommands)

| Flag | Env override | Default | Description |
|------|-------------|---------|-------------|
| `--config <PATH>` | `XIAOGUAI_CONFIG` | `~/.xiaoguai/config.yaml` | Path to YAML config file |
| `--token <TOKEN>` | `XIAOGUAI_API_TOKEN` | — | Bearer token for authenticated API calls |
| `--api-base <URL>` | `XIAOGUAI_API_BASE` | `http://localhost:8080` | Base URL of the `xiaoguai-api` server |
| `--output <FORMAT>` | — | `table` | Output format: `json`, `yaml`, or `table` |

## See Also

- [Quickstart](../quickstart.md)
- [REST API reference](../../book/src/api/rest.md)
- [Skills catalog overview](../../book/src/skills/overview.md)
