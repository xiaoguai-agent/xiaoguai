# `deploy/`

Operational artifacts. v0.9 ships docker-compose + a multi-stage Dockerfile;
v1.0 adds Helm + bare-metal tarball.

## Quick bring-up

```bash
# from the repo root
docker compose -f deploy/docker-compose.yml up --build

# in another terminal — first chat:
curl http://localhost:8080/healthz       # → ok
curl -X POST http://localhost:8080/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"user_id":"usr_dev","tenant_id":"ten_dev","model":"mock"}'
```

Or with the bundled CLI (assumes a local Rust toolchain):

```bash
cargo run -p xiaoguai-cli -- remote \
  --server http://localhost:8080 \
  chat --user-id usr_dev --tenant-id ten_dev --prompt 'hello'
```

## Layers

| Stage                                    | Lives where                             |
|------------------------------------------|------------------------------------------|
| Rust build                               | `Dockerfile` (rust:slim-bookworm)        |
| Runtime                                  | `Dockerfile` (distroless cc-debian12)    |
| Stack composition                        | `docker-compose.yml`                     |
| State (pg, valkey)                       | named volumes `pg_data`, `valkey_data`   |

## What's not here (deferred to v1.0)

- Helm chart with values + ingress + secrets refs.
- Bare-metal tarball + systemd unit + uninstall.
- Multi-arch (amd64 + arm64) buildx workflow.
- cosign signing + SBOM attachment.
