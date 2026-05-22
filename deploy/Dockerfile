# syntax=docker/dockerfile:1.7
#
# Multi-stage build for `xiaoguai-core` + the `xiaoguai` CLI.
#
#   docker build -f deploy/Dockerfile -t xiaoguai:dev .
#
# Build stage uses the official rust image, runtime stage uses a slim
# distroless variant so we ship roughly 30 MB compressed instead of 1 GB.

ARG RUST_VERSION=1.88

FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

# Install only the OS deps Cargo needs for sqlx/rustls; everything else is
# Rust-side.
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Copy the entire workspace. The Cargo cache is keyed off Cargo.lock so
# bumping a leaf crate doesn't bust deps unnecessarily.
COPY Cargo.toml Cargo.lock rust-toolchain.toml clippy.toml rustfmt.toml deny.toml ./
COPY crates ./crates

RUN cargo build --release \
    --bin xiaoguai-core \
    --bin xiaoguai

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

# Distroless ships a non-root `nonroot` user (uid 65532). All state lives
# under /data which the operator should mount as a volume.
WORKDIR /app
COPY --from=builder /src/target/release/xiaoguai-core /usr/local/bin/xiaoguai-core
COPY --from=builder /src/target/release/xiaoguai /usr/local/bin/xiaoguai

# Defaults can be overridden via env (XIAOGUAI_*) or a mounted config.yaml.
ENV RUST_LOG=info,sqlx=warn

USER nonroot:nonroot
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/xiaoguai-core"]
CMD ["serve"]
