# `frontend/`

Pnpm workspace with two React + TS apps and one shared types/client package.

```
frontend/
├── shared/         # @xiaoguai/shared — wire types + XiaoguaiClient
├── chat-ui/        # @xiaoguai/chat-ui — end-user chat surface (v0.8)
└── admin-ui/       # @xiaoguai/admin-ui — operator panes (v0.9, scaffolded)
```

## Quickstart

```bash
# from this directory
pnpm install
pnpm -F @xiaoguai/chat-ui dev          # http://localhost:5173

# requires the backend running on :8080 (or set VITE_API_URL)
cd ../
cargo run -p xiaoguai-core -- serve
```

Vite's dev server proxies `/v1` + `/healthz` to `http://localhost:8080`. For
a non-default backend point at it with `VITE_API_URL`.

## Production build

```bash
pnpm -F @xiaoguai/chat-ui build
# emits chat-ui/dist/ — serve via nginx or copy into xiaoguai-core's static
# assets directory once the asset-mount lands (v0.9).
```

## Type-check

```bash
pnpm -r typecheck
```

## What's intentionally absent in v0.8

- Auth flow — wire up after v0.6.1 surfaces OIDC.
- Sessions list endpoint — backend ships `GET /v1/sessions?user_id=...` in
  v0.6.1; for now the sidebar tracks sessions created during the current
  browser run.
- Admin UI is just the workspace member; v0.9 fills it in.
- No design system; v1.0 polishes.
