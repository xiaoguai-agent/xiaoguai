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

## Bare-metal install (v1.1.6)

For deployments that don't run Docker, every `v*` tag publishes
release tarballs alongside the container image:

```
xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
xiaoguai-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz
SHA256SUMS
```

Install on a systemd-based host (root required):

```bash
curl -LO https://github.com/xiaoguai-agent/xiaoguai/releases/download/vX.Y.Z/xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
tar -xzf xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
cd xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu
sudo bash scripts/install.sh

sudo cp /etc/xiaoguai/config.example.yaml /etc/xiaoguai/config.yaml
sudo $EDITOR /etc/xiaoguai/config.yaml      # set database / cache / hmac
sudo systemctl start xiaoguai-core
```

Build the tarballs locally:

```bash
# requires Rust + cross (https://github.com/cross-rs/cross) + Docker.
VERSION=1.1.6 bash scripts/release/build-tarball.sh
ls dist/
```

The systemd unit lives in `deploy/systemd/xiaoguai-core.service` —
`Type=simple` with full hardening (`ProtectSystem=strict`,
`NoNewPrivileges`, empty `CapabilityBoundingSet`, syscall filter,
read-only `/etc/xiaoguai`, etc.). Operators wanting `:80` / `:443`
add `CAP_NET_BIND_SERVICE` via a drop-in.

## What's not here (deferred post-v1.1.6)

- Helm chart with values + ingress + secrets refs.
- Debian / RPM packages (would need `cargo-deb` or `fpm`).
- musl statically-linked tarball (Alpine / older RHEL support).
- Windows MSI, macOS pkg.
