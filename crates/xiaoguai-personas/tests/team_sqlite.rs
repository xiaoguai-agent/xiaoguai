//! `SqliteTeamRepository` round-trip tests against a real temp `SQLite` DB.
//!
//! The in-memory tests (`team_crud.rs`) pin trait semantics; these pin the
//! SQL itself — column lists, RETURNING clauses, JSON TEXT round-trips, the
//! `UNIQUE (name)` constraint, the lead-persona FK, and the
//! one-team-per-session upsert. No Docker — temp file + crate migrations.

use sqlx::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;
use xiaoguai_personas::{
    model::CreatePersonaRequest,
    teams::model::{CreateTeamRequest, UpdateTeamRequest},
    PersonaError, PersonaRepository, SqlitePersonaRepository, SqliteTeamRepository, TeamRepository,
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

/// Create a real persona row so team FKs hold.
async fn make_persona(pool: &SqlitePool, name: &str) -> Uuid {
    let repo = SqlitePersonaRepository::new(pool.clone());
    repo.create(&CreatePersonaRequest {
        name: name.to_string(),
        system_prompt: format!("You are {name}."),
        default_model: None,
        tool_allowlist: None,
        escalation_tier: None,
    })
    .await
    .expect("create persona")
    .id
}

#[tokio::test]
async fn sqlite_team_full_roundtrip() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTeamRepository::new(pool.clone());
    let lead = make_persona(&pool, "Analyst").await;
    let worker = make_persona(&pool, "Worker").await;

    // Create + JSON round-trip.
    let created = repo
        .create(&CreateTeamRequest {
            name: "Finance Squad".to_string(),
            description: "Quarterly reports.".to_string(),
            lead_persona_id: lead,
            member_persona_ids: vec![lead, worker],
            recommended_pack_slugs: vec!["office-tools".to_string()],
        })
        .await
        .unwrap();
    assert_eq!(created.member_persona_ids, vec![lead, worker]);
    assert_eq!(created.recommended_pack_slugs, vec!["office-tools"]);

    let fetched = repo.get(created.id).await.unwrap();
    assert_eq!(fetched.member_persona_ids, vec![lead, worker]);
    assert_eq!(fetched.recommended_pack_slugs, vec!["office-tools"]);

    // UNIQUE (name) surfaces as DuplicateName.
    let dup = repo
        .create(&CreateTeamRequest {
            name: "Finance Squad".to_string(),
            description: String::new(),
            lead_persona_id: lead,
            member_persona_ids: vec![lead],
            recommended_pack_slugs: vec![],
        })
        .await
        .unwrap_err();
    assert!(matches!(
        dup,
        PersonaError::DuplicateName(_) | PersonaError::Database(_)
    ));

    // Partial update keeps unspecified fields.
    let updated = repo
        .update(
            created.id,
            &UpdateTeamRequest {
                description: Some("Annual reports.".to_string()),
                ..UpdateTeamRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Finance Squad");
    assert_eq!(updated.description, "Annual reports.");
    assert_eq!(updated.member_persona_ids, vec![lead, worker]);

    // Attach / replace / get / detach.
    let att = repo
        .attach_team_to_session("sess_1", created.id)
        .await
        .unwrap();
    assert_eq!(att.team_id, created.id);
    let active = repo.get_session_team("sess_1").await.unwrap().unwrap();
    assert_eq!(active.id, created.id);

    // Replacing via the upsert path (same session, attach again).
    repo.attach_team_to_session("sess_1", created.id)
        .await
        .unwrap();
    repo.detach_team_from_session("sess_1").await.unwrap();
    assert!(repo.get_session_team("sess_1").await.unwrap().is_none());

    // Archive blocks new attachments, list hides the row.
    repo.archive_team(created.id).await.unwrap();
    let err = repo
        .attach_team_to_session("sess_2", created.id)
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::Archived));
    assert!(repo.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_lead_fk_is_enforced() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTeamRepository::new(pool.clone());
    let ghost = Uuid::new_v4(); // no personas row

    let err = repo
        .create(&CreateTeamRequest {
            name: "Ghost Team".to_string(),
            description: String::new(),
            lead_persona_id: ghost,
            member_persona_ids: vec![ghost],
            recommended_pack_slugs: vec![],
        })
        .await
        .unwrap_err();
    // SQLite reports FK violations as a generic database error (no 23503
    // code like Postgres) — accept either classification.
    assert!(matches!(
        err,
        PersonaError::ForeignKey(_) | PersonaError::Database(_)
    ));
}
