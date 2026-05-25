//! Integration tests for `PersonaRepository` using the in-memory backend.
//!
//! These cover every trait method and the tool-allowlist enforcement helpers.
//! They run without a database so CI doesn't need Postgres.

use std::sync::Arc;
use uuid::Uuid;
use xiaoguai_personas::{
    enforcement::{filter_tools, tool_allowed},
    model::{CreatePersonaRequest, UpdatePersonaRequest},
    InMemoryPersonaRepository, PersonaError, PersonaRepository,
};

fn make_create(tenant: Uuid, name: &str) -> CreatePersonaRequest {
    CreatePersonaRequest {
        tenant_id: tenant,
        name: name.to_string(),
        system_prompt: format!("You are {name}."),
        default_model: None,
        tool_allowlist: None,
        escalation_tier: None,
    }
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_get_persona() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let req = make_create(tenant, "Support Bot");
    let created = repo.create(&req).await.unwrap();
    assert_eq!(created.name, "Support Bot");
    assert_eq!(created.tenant_id, tenant);
    assert!(!created.archived);

    let fetched = repo.get(created.id).await.unwrap();
    assert_eq!(fetched.id, created.id);
}

#[tokio::test]
async fn list_returns_only_active_for_tenant() {
    let repo = InMemoryPersonaRepository::new();
    let t1 = Uuid::new_v4();
    let t2 = Uuid::new_v4();
    repo.create(&make_create(t1, "Bot A")).await.unwrap();
    repo.create(&make_create(t1, "Bot B")).await.unwrap();
    repo.create(&make_create(t2, "Bot C")).await.unwrap();

    let t1_list = repo.list(t1).await.unwrap();
    assert_eq!(t1_list.len(), 2);
    assert!(t1_list.iter().all(|p| p.tenant_id == t1));
    // Ordered by name.
    assert_eq!(t1_list[0].name, "Bot A");
    assert_eq!(t1_list[1].name, "Bot B");

    let t2_list = repo.list(t2).await.unwrap();
    assert_eq!(t2_list.len(), 1);
}

#[tokio::test]
async fn create_duplicate_name_returns_error() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    repo.create(&make_create(tenant, "Unique")).await.unwrap();
    let err = repo.create(&make_create(tenant, "Unique")).await.unwrap_err();
    assert!(matches!(err, PersonaError::DuplicateName(_)));
}

#[tokio::test]
async fn duplicate_name_allowed_across_tenants() {
    let repo = InMemoryPersonaRepository::new();
    let t1 = Uuid::new_v4();
    let t2 = Uuid::new_v4();
    repo.create(&make_create(t1, "Shared Name")).await.unwrap();
    let second = repo.create(&make_create(t2, "Shared Name")).await;
    assert!(second.is_ok(), "same name under different tenant must succeed");
}

#[tokio::test]
async fn get_missing_returns_not_found() {
    let repo = InMemoryPersonaRepository::new();
    let err = repo.get(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, PersonaError::NotFound));
}

#[tokio::test]
async fn update_persona_fields() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let created = repo.create(&make_create(tenant, "Draft")).await.unwrap();
    let req = UpdatePersonaRequest {
        name: Some("Published".to_string()),
        system_prompt: Some("Updated prompt.".to_string()),
        tool_allowlist: Some(Some(vec!["tool_a".to_string()])),
        default_model: Some("gpt-4o".to_string()),
        escalation_tier: Some("L2".to_string()),
    };
    let updated = repo.update(created.id, &req).await.unwrap();
    assert_eq!(updated.name, "Published");
    assert_eq!(updated.system_prompt, "Updated prompt.");
    assert_eq!(
        updated.tool_allowlist,
        Some(vec!["tool_a".to_string()])
    );
    assert_eq!(updated.default_model.as_deref(), Some("gpt-4o"));
    assert_eq!(updated.escalation_tier.as_deref(), Some("L2"));
}

#[tokio::test]
async fn update_missing_returns_not_found() {
    let repo = InMemoryPersonaRepository::new();
    let err = repo
        .update(Uuid::new_v4(), &UpdatePersonaRequest::default())
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::NotFound));
}

#[tokio::test]
async fn archive_hides_from_list() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Retiring")).await.unwrap();
    repo.archive_persona(p.id).await.unwrap();
    let list = repo.list(tenant).await.unwrap();
    assert!(list.is_empty(), "archived persona must not appear in list");
    // get() still returns it (for admin inspection).
    let fetched = repo.get(p.id).await.unwrap();
    assert!(fetched.archived);
}

#[tokio::test]
async fn archive_is_idempotent() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "X")).await.unwrap();
    repo.archive_persona(p.id).await.unwrap();
    let second = repo.archive_persona(p.id).await;
    assert!(second.is_ok(), "archiving twice must not error");
}

// ── Session attachment ────────────────────────────────────────────────────────

#[tokio::test]
async fn attach_and_get_session_persona() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Finance")).await.unwrap();
    let session = "sess_abc123";
    let sp = repo
        .attach_persona_to_session(session, p.id)
        .await
        .unwrap();
    assert_eq!(sp.session_id, session);
    assert_eq!(sp.persona_id, p.id);

    let attached = repo.get_session_persona(session).await.unwrap().unwrap();
    assert_eq!(attached.id, p.id);
}

#[tokio::test]
async fn attach_replaces_existing_session_persona() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p1 = repo.create(&make_create(tenant, "P1")).await.unwrap();
    let p2 = repo.create(&make_create(tenant, "P2")).await.unwrap();
    let session = "sess_replace";
    repo.attach_persona_to_session(session, p1.id).await.unwrap();
    repo.attach_persona_to_session(session, p2.id).await.unwrap();
    let active = repo.get_session_persona(session).await.unwrap().unwrap();
    assert_eq!(active.id, p2.id, "second attach must replace the first");
}

#[tokio::test]
async fn attach_archived_persona_returns_error() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Old")).await.unwrap();
    repo.archive_persona(p.id).await.unwrap();
    let err = repo
        .attach_persona_to_session("sess_x", p.id)
        .await
        .unwrap_err();
    assert!(matches!(err, PersonaError::Archived));
}

#[tokio::test]
async fn detach_removes_session_persona() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Temp")).await.unwrap();
    repo.attach_persona_to_session("sess_detach", p.id)
        .await
        .unwrap();
    repo.detach_persona_from_session("sess_detach").await.unwrap();
    let result = repo.get_session_persona("sess_detach").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn detach_is_idempotent() {
    let repo = InMemoryPersonaRepository::new();
    let second = repo.detach_persona_from_session("no_session").await;
    assert!(second.is_ok());
}

#[tokio::test]
async fn get_session_persona_none_when_no_attachment() {
    let repo = InMemoryPersonaRepository::new();
    let result = repo.get_session_persona("ghost_session").await.unwrap();
    assert!(result.is_none());
}

// ── Multi-tenant isolation ────────────────────────────────────────────────────

#[tokio::test]
async fn multi_persona_per_tenant() {
    let repo = Arc::new(InMemoryPersonaRepository::new());
    let tenant = Uuid::new_v4();
    let names = ["Alpha", "Beta", "Gamma", "Delta"];
    for name in &names {
        repo.create(&CreatePersonaRequest {
            tenant_id: tenant,
            name: (*name).to_string(),
            system_prompt: format!("I am {name}."),
            default_model: None,
            tool_allowlist: None,
            escalation_tier: None,
        })
        .await
        .unwrap();
    }
    let list = repo.list(tenant).await.unwrap();
    assert_eq!(list.len(), 4, "all four personas must appear");
    // Alphabetical ordering preserved.
    let list_names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(list_names, ["Alpha", "Beta", "Delta", "Gamma"]);
}

// ── Tool-allowlist enforcement ────────────────────────────────────────────────

#[tokio::test]
async fn tool_allowlist_enforcement_unrestricted() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Unrestricted")).await.unwrap();
    assert!(tool_allowed(&p, "bash"));
    assert!(tool_allowed(&p, "web_search"));
    let tools = vec!["a".to_string(), "b".to_string()];
    let filtered = filter_tools(&p, &tools);
    assert_eq!(filtered, tools);
}

#[tokio::test]
async fn tool_allowlist_enforcement_restricted() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let req = CreatePersonaRequest {
        tenant_id: tenant,
        name: "Restricted".to_string(),
        system_prompt: String::new(),
        default_model: None,
        tool_allowlist: Some(vec!["read_file".to_string(), "web_search".to_string()]),
        escalation_tier: None,
    };
    let p = repo.create(&req).await.unwrap();
    assert!(tool_allowed(&p, "read_file"));
    assert!(tool_allowed(&p, "web_search"));
    assert!(!tool_allowed(&p, "bash"));
    assert!(!tool_allowed(&p, "delete_file"));

    let available = vec![
        "read_file".to_string(),
        "bash".to_string(),
        "web_search".to_string(),
        "delete_file".to_string(),
    ];
    let filtered = filter_tools(&p, &available);
    assert_eq!(filtered, vec!["read_file", "web_search"]);
}

#[tokio::test]
async fn tool_allowlist_enforcement_empty_denies_all() {
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let req = CreatePersonaRequest {
        tenant_id: tenant,
        name: "NoTools".to_string(),
        system_prompt: String::new(),
        default_model: None,
        tool_allowlist: Some(vec![]),
        escalation_tier: None,
    };
    let p = repo.create(&req).await.unwrap();
    assert!(!tool_allowed(&p, "any_tool"));
    let filtered = filter_tools(&p, &["any_tool".to_string()]);
    assert!(filtered.is_empty());
}

// ── Cascade on session deletion (simulated) ───────────────────────────────────

#[tokio::test]
async fn detach_on_session_cascade_simulated() {
    // In production the FK CASCADE handles this at the DB level. Here we
    // simulate the application-layer equivalent: detach and verify gone.
    let repo = InMemoryPersonaRepository::new();
    let tenant = Uuid::new_v4();
    let p = repo.create(&make_create(tenant, "Cascader")).await.unwrap();
    let session = "sess_cascade";
    repo.attach_persona_to_session(session, p.id).await.unwrap();
    // Simulate session deletion → detach persona.
    repo.detach_persona_from_session(session).await.unwrap();
    let after = repo.get_session_persona(session).await.unwrap();
    assert!(after.is_none(), "persona attachment must be gone after session delete");
}
