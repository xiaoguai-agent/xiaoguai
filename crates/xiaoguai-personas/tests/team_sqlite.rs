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
            glossary_md: None,
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
            glossary_md: None,
        })
        .await
        .unwrap_err();
    // #283: must be exactly DuplicateName — the old two-way
    // `DuplicateName(_) | Database(_)` assertion masked the SQLSTATE-vs-SQLite
    // code mismatch that turned 409s into 500s at the API layer.
    assert!(matches!(dup, PersonaError::DuplicateName(_)));

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
            glossary_md: None,
        })
        .await
        .unwrap_err();
    // #283: must be exactly ForeignKey — `from_sqlx` now classifies via the
    // driver-normalised `ErrorKind::ForeignKeyViolation`, so SQLite's native
    // FK code (787) is no longer lumped into the generic Database arm.
    assert!(matches!(err, PersonaError::ForeignKey(_)));
}

// ── #283 archive semantics: KNOWN LIMITATION — archived names not yet reusable ─

/// #283 known limitation (deferred): the `0032` table-level `UNIQUE (name)`
/// spans archived rows, so an archived team's name cannot yet be reused —
/// diverging from the in-memory repository, which only checks active teams.
/// The fix (drop the table constraint, add a partial unique index over active
/// rows) needs a no-transaction table-rebuild migration, which the current
/// `sqlx::migrate!` setup does not yet support (it always wraps in a
/// transaction → `PRAGMA foreign_keys` is a no-op and the rebuild's DROP is
/// blocked by `session_teams`' FK). Tracked in #283; this test pins the
/// CURRENT behaviour so the divergence is intentional and visible.
#[tokio::test]
async fn sqlite_archived_team_name_not_yet_reusable() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTeamRepository::new(pool.clone());
    let lead = make_persona(&pool, "Phoenix").await;

    let req = CreateTeamRequest {
        name: "Reborn Squad".to_string(),
        description: String::new(),
        lead_persona_id: lead,
        member_persona_ids: vec![lead],
        recommended_pack_slugs: vec![],
        glossary_md: None,
    };
    let first = repo.create(&req).await.unwrap();

    // While active, the name is taken.
    let dup = repo.create(&req).await.unwrap_err();
    assert!(matches!(dup, PersonaError::DuplicateName(_)));

    // Archiving does NOT yet free the name (table-level UNIQUE spans archived
    // rows — see the doc comment above; #283 deferred).
    repo.archive_team(first.id).await.unwrap();
    let still_dup = repo.create(&req).await.unwrap_err();
    assert!(matches!(still_dup, PersonaError::DuplicateName(_)));
}

// ── T7.1 glossary column round-trip (set / clear / cap) ───────────────────────

#[tokio::test]
async fn sqlite_glossary_set_clear_and_cap() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTeamRepository::new(pool.clone());
    let lead = make_persona(&pool, "Glossarist").await;

    // Set on create + SELECT round-trip (including the session-team JOIN).
    let created = repo
        .create(&CreateTeamRequest {
            name: "Glossary Squad".to_string(),
            description: String::new(),
            lead_persona_id: lead,
            member_persona_ids: vec![lead],
            recommended_pack_slugs: vec![],
            glossary_md: Some("MRR = monthly recurring revenue".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(
        created.glossary_md.as_deref(),
        Some("MRR = monthly recurring revenue")
    );
    assert_eq!(
        repo.get(created.id).await.unwrap().glossary_md,
        created.glossary_md
    );
    repo.attach_team_to_session("sess_g", created.id)
        .await
        .unwrap();
    let attached = repo.get_session_team("sess_g").await.unwrap().unwrap();
    assert_eq!(attached.glossary_md, created.glossary_md);

    // Partial update of another field keeps the glossary; blank clears it.
    let updated = repo
        .update(
            created.id,
            &UpdateTeamRequest {
                description: Some("desc".to_string()),
                ..UpdateTeamRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.glossary_md, created.glossary_md);
    let cleared = repo
        .update(
            created.id,
            &UpdateTeamRequest {
                glossary_md: Some(String::new()),
                ..UpdateTeamRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.glossary_md, None);

    // Over-cap rejected with InvalidArgument before any write.
    let oversized = "x".repeat(xiaoguai_personas::teams::model::MAX_GLOSSARY_BYTES + 1);
    let err = repo
        .update(
            created.id,
            &UpdateTeamRequest {
                glossary_md: Some(oversized),
                ..UpdateTeamRequest::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::InvalidArgument(_)));
    assert_eq!(repo.get(created.id).await.unwrap().glossary_md, None);
}
