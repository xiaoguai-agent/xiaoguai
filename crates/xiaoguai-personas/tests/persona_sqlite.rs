//! `SqlitePersonaRepository` archive-semantics test against a real temp `SQLite`
//! DB. Mirrors `team_sqlite.rs`'s #283 coverage: pins migration 0039, which
//! replaced the 0016 table-level `UNIQUE (name)` (spanning archived rows) with a
//! PARTIAL UNIQUE index over active rows, so an archived persona's name is freed
//! for reuse — matching `InMemoryPersonaRepository` (active-only check) and the
//! `expert_teams` repository (migration 0035).
//!
//! No Docker — temp file + crate migrations.

use sqlx::SqlitePool;
use tempfile::TempDir;
use xiaoguai_personas::{
    model::CreatePersonaRequest, PersonaError, PersonaRepository, SqlitePersonaRepository,
};
use xiaoguai_storage::db;

async fn test_setup() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let pool = db::connect(path.to_str().expect("utf8 path"), 5)
        .await
        .expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, dir)
}

fn persona_req(name: &str) -> CreatePersonaRequest {
    CreatePersonaRequest {
        name: name.to_string(),
        system_prompt: format!("You are {name}."),
        default_model: None,
        tool_allowlist: None,
        escalation_tier: None,
    }
}

/// Migration 0039: an archived persona's name is reusable by a new active
/// persona, while two ACTIVE personas still cannot share a name. Mirrors
/// `team_sqlite::sqlite_archived_team_name_is_reusable` (#283, migration 0035).
#[tokio::test]
async fn sqlite_archived_persona_name_is_reusable() {
    let (pool, _guard) = test_setup().await;
    let repo = SqlitePersonaRepository::new(pool.clone());

    let req = persona_req("Reborn Analyst");
    let first = repo.create(&req).await.unwrap();

    // While active, the name is taken (partial unique index over active rows).
    let dup = repo.create(&req).await.unwrap_err();
    assert!(matches!(dup, PersonaError::DuplicateName(_)));

    // Archiving frees the name for a new active persona — the whole point of 0039.
    repo.archive_persona(first.id).await.unwrap();
    let second = repo
        .create(&req)
        .await
        .expect("archived persona name must be reusable (migration 0039)");
    assert_ne!(second.id, first.id);

    // …but only ONE active persona may hold it at a time.
    let dup_again = repo.create(&req).await.unwrap_err();
    assert!(matches!(dup_again, PersonaError::DuplicateName(_)));
}
