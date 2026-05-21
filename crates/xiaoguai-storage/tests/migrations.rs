//! Migration smoke test against an ephemeral Postgres container.
//!
//! Tests are marked `#[ignore]` by default since they require Docker. Run via
//! `cargo test -p xiaoguai-storage -- --ignored`.

#[cfg(test)]
mod containerized {
    use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};
    use xiaoguai_storage::db;

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn migrations_apply_clean() {
        let pg = Postgres::default().start().await.expect("start pg");
        let port = pg.get_host_port_ipv4(5432).await.expect("port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = db::connect(&url, 5).await.expect("connect");
        db::migrate(&pool).await.expect("migrate");

        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM tenants")
            .fetch_one(&pool)
            .await
            .expect("query");
        assert_eq!(count.0, 0);

        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'audit_log')",
        )
        .fetch_one(&pool)
        .await
        .expect("query");
        assert!(exists.0, "audit_log table should exist");
    }
}
