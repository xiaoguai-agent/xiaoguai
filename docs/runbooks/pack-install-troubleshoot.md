# Pack install troubleshoot — v1.2.28

`POST /v1/skills/install` returns `503`, or the DB row is recorded but
the pack's tools are not available to agents.

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`)
> with an embedded SQLite database — no Postgres, no Kubernetes. Inspect
> state with `sqlite3 ~/.xiaoguai/data.db` (under systemd:
> `/var/lib/xiaoguai/data.db`) and operate the process with `systemctl` /
> `journalctl`. There is a single implicit **owner** — no tenants.

> **v1.2 caveat — runtime loader not yet active.**
> Installing a pack writes a row to `installed_skill_packs` and returns
> `200` with the installation record, but the runtime tool-loader is not
> wired (`v1.3` item).  After a successful `install` call the pack's tools
> will not appear in the agent's `Toolbox` until the service is restarted
> _and_ the v1.3 hot-reload path ships.  Document this honestly to users
> who ask "why isn't my pack doing anything?".

---

## Auth note

When `auth.username` / `auth.password` are set (env
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`), pass
`-u "$USER:$PASS"` on every `curl`. When no credential is configured the
API gate is **open** — drop the `-u` flag.

---

## Symptoms

- `POST /v1/skills/install` returns `503 Service Unavailable`.
- Install call returns `200` with an `id`, but agents don't gain the
  pack's tools.
- `GET /v1/skills/installed` returns `503`.
- Install returns `409 Conflict` when the pack is already installed.
- `POST /v1/skills/install` returns `404` — pack slug not in catalog.

---

## Diagnose

**1. Confirm the catalog is reachable (never needs the repo):**

```bash
curl -s "http://localhost:7600/v1/skills/catalog" \
  | jq '.packs[].slug'
# Expected slugs: ar-collections, incident-triage, pr-review,
#                 hr-onboarding, rag-legal, rag-finance, rag-hr
```

If this returns `503`, the core binary itself is unhealthy — start
with `GET /healthz`.

**2. Check whether the skill-pack repository is wired:**

```bash
curl -s "http://localhost:7600/v1/skills/installed"
# 503 → installed_skill_packs not wired in AppState (likely a missing
#       migration or config knob)
# 200 → wired; proceed to step 3
```

If `503`, confirm the `installed_skill_packs` migration ran (migrations
run automatically at boot):

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT name FROM sqlite_master WHERE type='table' AND name='installed_skill_packs';"
# No row → migration not applied; check `journalctl -u xiaoguai-core`
#          for migration errors, then `systemctl restart xiaoguai-core`
```

**3. Check DB state for the installed pack:**

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, pack_slug, version, installed_at, config
  FROM installed_skill_packs
  ORDER BY installed_at DESC;"
```

If the row is present but tools aren't active, that is the expected
v1.2 state — see the caveat above.

**4. Handle `409 Conflict` (pack already installed):**

```bash
# Uninstall first, then reinstall:
curl -s -X DELETE -u "$USER:$PASS" \
  "http://localhost:7600/v1/skills/$INSTALL_ID"

# Confirm it's gone:
curl -s "http://localhost:7600/v1/skills/installed" \
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

### Option A — Restart the service (apply a pending migration)

```bash
systemctl restart xiaoguai-core
systemctl status xiaoguai-core --no-pager
xiaoguai smoke
```

### Option B — Manual install via SQL (when API is 503 but DB is up)

`installed_skill_packs.id` is a TEXT primary key — supply your own
identifier (the API normally generates one):

```bash
sqlite3 ~/.xiaoguai/data.db "
  INSERT INTO installed_skill_packs (id, pack_slug, version, config)
  VALUES (
    '$INSTALL_ID',
    '$PACK_SLUG',
    '1.0.0',
    '{}'
  )
  ON CONFLICT (pack_slug) DO NOTHING;"

sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, pack_slug, version FROM installed_skill_packs WHERE pack_slug = '$PACK_SLUG';"
```

### Option C — Make tools available now (v1.2 workaround)

Until v1.3 hot-reload ships, the only way to make a freshly installed
pack's tools active is to restart the service. The installed row
persists across restarts because it is in the SQLite store, not in
memory.

```bash
# Confirm the DB row exists:
sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, pack_slug FROM installed_skill_packs;"

# Restart to apply:
systemctl restart xiaoguai-core

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
curl -s "http://localhost:7600/v1/skills/installed" \
  | jq '.[] | {id, pack_slug, version}'

# Start a test session and call a tool from the pack:
curl -s -X POST -u "$USER:$PASS" \
  -H "Content-Type: application/json" \
  -d '{"model":"default","message":"list the tools available to you"}' \
  "http://localhost:7600/v1/sessions"
```

---

## Postmortem checklist

- [ ] Root cause: 503 (repo unwired) / 409 (duplicate) / 404 (bad slug)
- [ ] `installed_skill_packs` table confirmed present after boot
- [ ] Users notified about v1.2 restart requirement if tools weren't active
- [ ] v1.3 hot-reload tracked in backlog (no action needed here)
