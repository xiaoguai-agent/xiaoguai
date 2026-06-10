//! Integration tests for `TeamRepository` using the in-memory backend.
//!
//! Covers every trait method plus composition validation (lead must be a
//! member, at least one member, no duplicate members). Runs without a
//! database, mirroring `persona_crud.rs`.

use uuid::Uuid;
use xiaoguai_personas::{
    teams::model::{CreateTeamRequest, UpdateTeamRequest},
    InMemoryTeamRepository, PersonaError, TeamRepository,
};

fn make_create(name: &str, lead: Uuid, members: Vec<Uuid>) -> CreateTeamRequest {
    CreateTeamRequest {
        name: name.to_string(),
        description: format!("Team {name}."),
        lead_persona_id: lead,
        member_persona_ids: members,
        recommended_pack_slugs: vec![],
    }
}

fn make_valid(name: &str) -> CreateTeamRequest {
    let lead = Uuid::new_v4();
    make_create(name, lead, vec![lead, Uuid::new_v4()])
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_get_team() {
    let repo = InMemoryTeamRepository::new();
    let lead = Uuid::new_v4();
    let other = Uuid::new_v4();
    let created = repo
        .create(&make_create("Finance Squad", lead, vec![lead, other]))
        .await
        .unwrap();
    assert_eq!(created.name, "Finance Squad");
    assert_eq!(created.lead_persona_id, lead);
    assert_eq!(created.member_persona_ids, vec![lead, other]);
    assert!(!created.archived);

    let fetched = repo.get(created.id).await.unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.member_persona_ids, created.member_persona_ids);
}

#[tokio::test]
async fn get_missing_returns_not_found() {
    let repo = InMemoryTeamRepository::new();
    let err = repo.get(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, PersonaError::NotFound));
}

#[tokio::test]
async fn create_duplicate_name_returns_error() {
    let repo = InMemoryTeamRepository::new();
    repo.create(&make_valid("Unique")).await.unwrap();
    let err = repo.create(&make_valid("Unique")).await.unwrap_err();
    assert!(matches!(err, PersonaError::DuplicateName(_)));
}

#[tokio::test]
async fn list_returns_only_active_ordered_by_name() {
    let repo = InMemoryTeamRepository::new();
    repo.create(&make_valid("Team B")).await.unwrap();
    let a = repo.create(&make_valid("Team A")).await.unwrap();
    repo.create(&make_valid("Team C")).await.unwrap();
    repo.archive_team(a.id).await.unwrap();

    let list = repo.list().await.unwrap();
    let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Team B", "Team C"]);
}

#[tokio::test]
async fn update_team_fields() {
    let repo = InMemoryTeamRepository::new();
    let created = repo.create(&make_valid("Draft")).await.unwrap();

    let new_lead = Uuid::new_v4();
    let req = UpdateTeamRequest {
        name: Some("Final".to_string()),
        description: Some("Polished.".to_string()),
        lead_persona_id: Some(new_lead),
        member_persona_ids: Some(vec![new_lead]),
        recommended_pack_slugs: Some(vec!["office-tools".to_string()]),
    };
    let updated = repo.update(created.id, &req).await.unwrap();
    assert_eq!(updated.name, "Final");
    assert_eq!(updated.description, "Polished.");
    assert_eq!(updated.lead_persona_id, new_lead);
    assert_eq!(updated.member_persona_ids, vec![new_lead]);
    assert_eq!(updated.recommended_pack_slugs, vec!["office-tools"]);
}

#[tokio::test]
async fn partial_update_keeps_other_fields() {
    let repo = InMemoryTeamRepository::new();
    let created = repo.create(&make_valid("Keep")).await.unwrap();

    let req = UpdateTeamRequest {
        description: Some("New description only.".to_string()),
        ..UpdateTeamRequest::default()
    };
    let updated = repo.update(created.id, &req).await.unwrap();
    assert_eq!(updated.name, "Keep");
    assert_eq!(updated.description, "New description only.");
    assert_eq!(updated.lead_persona_id, created.lead_persona_id);
    assert_eq!(updated.member_persona_ids, created.member_persona_ids);
}

// ── Composition validation ────────────────────────────────────────────────────

#[tokio::test]
async fn create_rejects_empty_members() {
    let repo = InMemoryTeamRepository::new();
    let err = repo
        .create(&make_create("Empty", Uuid::new_v4(), vec![]))
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::InvalidArgument(_)));
}

#[tokio::test]
async fn create_rejects_lead_not_in_members() {
    let repo = InMemoryTeamRepository::new();
    let err = repo
        .create(&make_create(
            "Leadless",
            Uuid::new_v4(),
            vec![Uuid::new_v4()],
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::InvalidArgument(_)));
}

#[tokio::test]
async fn create_rejects_duplicate_members() {
    let repo = InMemoryTeamRepository::new();
    let lead = Uuid::new_v4();
    let err = repo
        .create(&make_create("Dupes", lead, vec![lead, lead]))
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::InvalidArgument(_)));
}

#[tokio::test]
async fn update_rejects_duplicate_name() {
    let repo = InMemoryTeamRepository::new();
    repo.create(&make_valid("Taken")).await.unwrap();
    let other = repo.create(&make_valid("Renaming")).await.unwrap();

    let err = repo
        .update(
            other.id,
            &UpdateTeamRequest {
                name: Some("Taken".to_string()),
                ..UpdateTeamRequest::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::DuplicateName(_)));
}

#[tokio::test]
async fn update_validates_merged_composition() {
    let repo = InMemoryTeamRepository::new();
    let created = repo.create(&make_valid("Merge")).await.unwrap();

    // New lead who is NOT among the (unchanged) existing members → invalid.
    let req = UpdateTeamRequest {
        lead_persona_id: Some(Uuid::new_v4()),
        ..UpdateTeamRequest::default()
    };
    let err = repo.update(created.id, &req).await.unwrap_err();
    assert!(matches!(err, PersonaError::InvalidArgument(_)));

    // Original state untouched after the failed update.
    let fetched = repo.get(created.id).await.unwrap();
    assert_eq!(fetched.lead_persona_id, created.lead_persona_id);
}

// ── Archive ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn archive_is_idempotent_and_blocks_attach() {
    let repo = InMemoryTeamRepository::new();
    let created = repo.create(&make_valid("Old Guard")).await.unwrap();
    repo.archive_team(created.id).await.unwrap();
    repo.archive_team(created.id).await.unwrap(); // idempotent

    let err = repo
        .attach_team_to_session("sess_x", created.id)
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::Archived));
}

// ── Session attachment ────────────────────────────────────────────────────────

#[tokio::test]
async fn attach_get_replace_detach_session_team() {
    let repo = InMemoryTeamRepository::new();
    let first = repo.create(&make_valid("First")).await.unwrap();
    let second = repo.create(&make_valid("Second")).await.unwrap();

    let att = repo
        .attach_team_to_session("sess_1", first.id)
        .await
        .unwrap();
    assert_eq!(att.session_id, "sess_1");
    assert_eq!(att.team_id, first.id);

    let active = repo.get_session_team("sess_1").await.unwrap().unwrap();
    assert_eq!(active.id, first.id);

    // Replace.
    repo.attach_team_to_session("sess_1", second.id)
        .await
        .unwrap();
    let active = repo.get_session_team("sess_1").await.unwrap().unwrap();
    assert_eq!(active.id, second.id);

    // Detach is idempotent.
    repo.detach_team_from_session("sess_1").await.unwrap();
    repo.detach_team_from_session("sess_1").await.unwrap();
    assert!(repo.get_session_team("sess_1").await.unwrap().is_none());
}

#[tokio::test]
async fn attach_missing_team_returns_not_found() {
    let repo = InMemoryTeamRepository::new();
    let err = repo
        .attach_team_to_session("sess_1", Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::NotFound));
}
