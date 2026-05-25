# ADR-0017 — Skill packs: declarative config + deferred hot-reload

Date: 2026-05-26
Status: Accepted

## Context

Xiaoguai needs a marketplace mechanism: operators install domain-specific capability bundles ("skill packs") that add watches, anomaly detectors, agent definitions, SQL migrations, and dashboard layouts without modifying the platform codebase. Examples: an "AR Collections" pack that adds overdue-invoice watch rules and a collections agent, or a "vSphere Ops" pack that registers VMware-specific anomaly detectors.

Three design questions:

**Declarative vs imperative pack registration**: A pack could be a Rust crate that calls platform APIs (imperative, like a plugin), or a YAML manifest that declares what it contains (declarative). Imperative packs are expressive but require API versioning and can call arbitrary platform internals. Declarative packs are constrained to known extension points and are auditable without running code.

**Install-record vs runtime-activation**: Should the installed pack record (row in DB) be the same object that holds the running state of the pack, or should these be separate? Coupling them means the install step immediately activates watches and agents, which is risky for atomic rollback. Separating them means install is a safe DB write; activation is a subsequent step.

**Hot-reload in v1.2 vs v1.3**: Should pack loading support live reload (detecting `pack.yaml` changes and re-registering without restart) in the initial cut? Live reload requires change detection, partial de-registration, and state migration for in-flight agents — significant complexity.

## Decision

### Declarative YAML manifest

Each pack is a directory containing a `pack.yaml` manifest and referenced files. The manifest declares:

```yaml
name: ar-collections        # kebab-case, unique
version: "1.2.0"            # SemVer
description: "…"
requires:
  xiaoguai_version: ">=1.2"
  features: [watch, anomaly, llm, outcome-telemetry]
migrations:
  - path: migrations/0001_ar_schema.sql
watches:
  - path: watches/overdue-invoice.yaml
anomalies:
  - path: anomalies/collection-spike.yaml
agents:
  - path: agents/collections-agent.yaml
dashboards:
  - name: AR Collections Overview
```

The platform validates that all referenced paths exist at load time. It does not execute arbitrary code from the pack. Extension points (watches, anomalies, agents) are registered through typed registries with defined interfaces.

### Install-record row vs runtime-activation (split for safety)

`installed_skill_packs` (migration 0015) records what is installed per tenant:

```sql
CREATE TABLE installed_skill_packs (
    id           UUID PRIMARY KEY,
    tenant_id    UUID NOT NULL,
    pack_slug    TEXT NOT NULL,
    version      TEXT NOT NULL,
    config       JSONB NOT NULL DEFAULT '{}',
    installed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, pack_slug)
);
```

The install API writes this row and returns 200. It does **not** immediately activate watches or agents. The activation step — wiring manifest items into running registries — is separate and gated on the downstream F1 (watch registry) and F2 (anomaly registry) feature branches landing.

This split means: install is a no-op today (safe, always reversible by deleting the row); activation is layered on top once the registries are ready. Operators can install packs today and get activation for free when the platform upgrades.

`UNIQUE (tenant_id, pack_slug)` at the DB level prevents double-installs; the API returns 409 Conflict on violation.

### v1.3 hot-reload model (deferred)

Hot-reload is explicitly deferred to v1.3. The `PackLoader::load` API is synchronous today (parse + validate only); the v1.3 wiring plan documented in `packs.rs` adds:

```
PackLoader::load(path)
  → PackManifest::from_yaml(...)
  → apply_migrations(pool, &manifest.migrations)
  → register_watches(watch_registry, &manifest.watches)   // F1
  → register_anomalies(anomaly_registry, &manifest.anomalies)  // F2
  → register_agents(agent_registry, &manifest.agents)
```

File-system watching for live reload (`inotify`/`kqueue`) will be added in v1.3 once de-registration semantics are defined. The v1.2 model requires a server restart to pick up manifest changes.

### Feature gate

The `packs` module is behind `cfg(feature = "packs")`. The feature is off by default in the library; the server binary enables it. This prevents the `serde_yaml` + `tokio::fs` dependencies from landing in minimal builds.

## Consequences

**Positive:**
- Declarative manifests are auditable: operators and security reviewers can read `pack.yaml` without running code. No arbitrary Rust plugin execution.
- Install-record split means install is always safe and reversible; it cannot cause runtime failures.
- `UNIQUE (tenant_id, pack_slug)` prevents accidental double-installs.
- Feature gate keeps `serde_yaml` out of slim builds.
- v1.3 hot-reload can be layered on without changing the manifest format or the install-record schema.

**Negative:**
- Install is a no-op today — operators who install a pack see no immediate effect until activation lands (F1/F2). This may be confusing. Mitigation: the API response includes `"status": "installed_pending_activation"` and the admin-UI shows pack state clearly.
- Declarative manifests cannot express arbitrary logic (e.g. "if feature X is on, also register Y"). Mitigation: the `requires.features` field gates activation; complex conditional logic can be expressed in agent YAML, not the pack manifest.
- Path validation at load time checks existence but not semantic correctness of referenced YAML files. A malformed watch YAML will fail at registration time (F1), not at install time. Mitigation: pack publishers run `xiaoguai pack validate` CI step.

## Implementation

- `crates/xiaoguai-core/src/packs.rs` — `PackManifest`, `PackLoader`, `WatchRegistry` / `AnomalyRegistry` stubs, `register_pack`
- `crates/xiaoguai-storage/migrations/0015_skill_packs.sql` — `installed_skill_packs` table
- API routes: `POST /v1/skill-packs/install`, `DELETE /v1/skill-packs/:slug`, `GET /v1/skill-packs` (wired against `installed_skill_packs`)

## References

- `crates/xiaoguai-core/src/packs.rs` — full implementation and test suite
- `crates/xiaoguai-storage/migrations/0015_skill_packs.sql` — install-record schema
- ADR-0006 — MCP tasks primitive (agent registry extension point used by packs)
- `docs/plans/` — F1 (watch registry) and F2 (anomaly registry) planning docs
