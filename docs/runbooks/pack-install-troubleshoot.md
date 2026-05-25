# Pack install troubleshoot — v1.2.28

`POST /v1/skills/install` returns `503`, or the DB row is recorded but
the pack's tools are not available to agents.

> **v1.2 caveat — runtime loader not yet active.**
> Installing a pack writes a row to `skill_packs` and returns `200` with
> the installation record, but the runtime tool-loader is not wired
> (`v1.3` item).  After a successful `install` call the pack's tools will
> not appear in the agent's `Toolbox` until the service is restarted _and_
> the v1.3 hot-reload path ships.  Document this honestly to users who ask
> "why isn't my pack doing anything?".

---

## Symptoms

- `POST /v1/skills/install` returns `503 Service Unavailable`.
- Install call returns `200` with an `id`, but agents don't gain the
  pack's tools.
- `GET /v1/skills/installed?tenant=<id>` returns `503`.
- Install returns `409 Conflict` when the tenant already has the pack.
- `POST /v1/skills/install` returns `404` — pack slug not in catalog.

---

## Diagnose

**1. Confirm the catalog is reachable (never needs the repo):**

```bash
curl -s \
  "http://xiaoguai-core.svc:8080/v1/skills/catalog" \
  | jq '.packs[].slug'
# Expected slugs: ar-collections, incident-triage, pr-review,
#                 hr-onboarding, rag-legal, rag-finance, rag-hr
```

If this returns `503`, the core binary itself is unhealthy — start
with `GET /healthz`.

**2. Check whether the skill_pack repository is wired:**

```bash
curl -s \
  "http://xiaoguai-core.svc:8080/v1/skills/installed?tenant=$TENANT_ID"
# 503 → skill_packs not wired in AppState (likely missing DB migration or
#       config knob)
# 200 → wired; proceed to step 3
```

If `503`, confirm migration 0013 ran (the `skill_packs` table):

```bash
kubectl exec deploy/xiaoguai -- psql "$DATABASE_URL" -c \
  "SELECT version FROM _sqlx_migrations WHERE version = 13;"
# No row → migration not applied; restart the pod (migrations run at boot)
```

**3. Check DB state for the installed pack:**

```bash
psql "$DATABASE_URL" -c "
  SELECT id, tenant_id, pack_slug, version, installed_at, config
  FROM skill_packs
  WHERE tenant_id = '$TENANT_ID'
  ORDER BY installed_at DESC;"
```

If the row is present but tools aren't active, that is the expected
v1.2 state — see the caveat above.

**4. Handle `409 Conflict` (pack already installed):**

```bash
# Uninstall first, then reinstall:
curl -s -X DELETE \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/skills/$INSTALL_ID"

# Confirm it's gone:
curl -s \
  "http://xiaoguai-core.svc:8080/v1/skills/installed?tenant=$TENANT_ID" \
  | jq '.[] | select(.pack_slug == "'"$PACK_SLUG"'")'
# Expect empty
```

**5. Handle `404` — slug not in catalog:**

Check for typos. The catalog is compiled into the binary; slugs are
case-sensitive and hyphenated:

```
ar-collections  incident-triage  pr-review  hr-onboarding
rag-legal       rag-finance      rag-hr
```

---

## Remediate

### Option A — Restart the pod (apply pending migration)

```bash
kubectl rollout restart deploy/xiaoguai
kubectl rollout status deploy/xiaoguai --timeout=120s
kubectl exec deploy/xiaoguai -- /usr/local/bin/xiaoguai-core smoke
```

### Option B — Manual install via SQL (when API is 503 but DB is up)

```bash
psql "$DATABASE_URL" -c "
  INSERT INTO skill_packs (tenant_id, pack_slug, version, config)
  VALUES (
    '$TENANT_ID',
    '$PACK_SLUG',
    '1.0.0',
    '{}'
  )
  ON CONFLICT (tenant_id, pack_slug) DO NOTHING
  RETURNING id, pack_slug, version;"
```

### Option C — Make tools available now (v1.2 workaround)

Until v1.3 hot-reload ships, the only way to make a freshly installed
pack's tools active is to restart the pod. The installed row persists
across restarts because it is in PG, not in memory.

```bash
# Confirm the DB row exists:
psql "$DATABASE_URL" -c \
  "SELECT id, pack_slug FROM skill_packs WHERE tenant_id = '$TENANT_ID';"

# Restart to apply:
kubectl rollout restart deploy/xiaoguai

# Tools from the installed pack are now available to new sessions.
# Existing sessions may need to be cancelled and re-started.
```

Communicate to the end user:

> Pack installed successfully. Tools from this pack are available in
> **new sessions** after a brief service restart. Existing sessions
> must be cancelled and re-started to pick up the new tools.
> Hot-reload without restart is planned for v1.3.

---

## Verify

```bash
# Confirm the pack is in the installed list:
curl -s \
  "http://xiaoguai-core.svc:8080/v1/skills/installed?tenant=$TENANT_ID" \
  | jq '.[] | {id, pack_slug, version}'

# Start a test session and call a tool from the pack:
curl -s -X POST \
  -H "Authorization: Bearer $USER_JWT" \
  -H "Content-Type: application/json" \
  -d '{"model":"default","message":"list the tools available to you"}' \
  "http://xiaoguai-core.svc:8080/v1/sessions"
```

---

## Postmortem checklist

- [ ] Root cause: 503 (repo unwired) / 409 (duplicate) / 404 (bad slug)
- [ ] Migration 0013 confirmed on all replicas
- [ ] Users notified about v1.2 restart requirement if tools weren't active
- [ ] v1.3 hot-reload tracked in backlog (no action needed here)
