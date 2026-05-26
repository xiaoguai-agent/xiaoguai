//! Shared testcontainers helper for repository integration tests.
//!
//! Each test boots an isolated Postgres container (≈2s on a warm host) and
//! applies the migrations bundled in this crate. The container handle is
//! returned alongside the pool — callers must keep it alive for the duration
//! of the test, otherwise the container is dropped and the pool's connections
//! die.

#![allow(dead_code)]

use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
};
use xiaoguai_storage::db;

/// Boot a Postgres container, connect a pool, and run all migrations.
///
/// # Panics
///
/// Panics if the Docker daemon is unreachable or migrations fail. Tests using
/// this helper are tagged `#[ignore = "requires Docker"]`.
pub async fn test_setup() -> (PgPool, ContainerAsync<Postgres>) {
    // pgvector image (postgres + the `vector` extension) — migration 0019
    // needs the vector type, which plain `postgres` does not provide.
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await
        .expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, pg)
}
