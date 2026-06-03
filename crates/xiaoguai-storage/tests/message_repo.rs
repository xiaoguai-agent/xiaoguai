//! Integration tests for [`PgMessageRepository`] (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.

mod common;

use chrono::{Duration, Utc};
use common::test_setup;
use serde_json::json;
use sqlx::SqlitePool;
use xiaoguai_storage::repositories::{
    MessageRepository, PgMessageRepository, PgSessionRepository, SessionRepository,
};
use xiaoguai_storage::OWNER_TENANT_ID;
use xiaoguai_types::{
    ContentBlock, Message, MessageId, MessageRole, Session, SessionId, SessionStatus, TenantId,
    UserId,
};

/// Seed a user via raw SQL so the session FK is satisfied. The `users` table no
/// longer carries `tenant_id`; we return a synthetic owner id for fixtures.
async fn seed_user(pool: &SqlitePool) -> (TenantId, UserId) {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES (?, ?, ?)")
        .bind(user_id.as_str())
        .bind(format!("u-{}@example.com", user_id.as_str()))
        .bind("Test User")
        .execute(pool)
        .await
        .expect("insert user");
    (TenantId::from(OWNER_TENANT_ID.to_string()), user_id)
}

async fn seed_session(pool: &SqlitePool, tenant: &TenantId, user: &UserId) -> SessionId {
    let now = Utc::now();
    let s = Session {
        id: SessionId::new(),
        tenant_id: tenant.clone(),
        user_id: user.clone(),
        title: None,
        created_at: now,
        updated_at: now,
        model: "gpt-4o-mini".to_string(),
        status: SessionStatus::Active,
        parent_session_id: None,
        forked_from_message_id: None,
    };
    let id = s.id.clone();
    PgSessionRepository::new(pool.clone())
        .create(None, &s)
        .await
        .expect("create session");
    id
}

fn fixture_message(
    session_id: &SessionId,
    role: MessageRole,
    content: Vec<ContentBlock>,
) -> Message {
    Message {
        id: MessageId::new(),
        session_id: session_id.clone(),
        role,
        content,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn append_and_list_roundtrip_text_content() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let session_id = seed_session(&pool, &tenant, &user).await;
    let repo = PgMessageRepository::new(pool.clone());

    let msg = fixture_message(
        &session_id,
        MessageRole::User,
        vec![ContentBlock::Text {
            text: "Hello, 世界 🚀".to_string(),
        }],
    );
    repo.append(None, &msg).await.expect("append");

    let list = repo
        .list_by_session(None, session_id.as_str(), 10, 0)
        .await
        .expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_str(), msg.id.as_str());
    assert_eq!(list[0].role, MessageRole::User);
    match &list[0].content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello, 世界 🚀"),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn append_jsonb_tool_call_and_tool_result_roundtrip() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let session_id = seed_session(&pool, &tenant, &user).await;
    let repo = PgMessageRepository::new(pool.clone());

    let assistant_msg = fixture_message(
        &session_id,
        MessageRole::Assistant,
        vec![
            ContentBlock::Text {
                text: "Calling tool...".to_string(),
            },
            ContentBlock::ToolCall {
                tool_call_id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: json!({"query": "rust async", "limit": 5}),
            },
        ],
    );
    repo.append(None, &assistant_msg)
        .await
        .expect("append assistant");

    let tool_msg = fixture_message(
        &session_id,
        MessageRole::Tool,
        vec![ContentBlock::ToolResult {
            tool_call_id: "tc_1".to_string(),
            output: json!({"hits": [{"title": "tokio"}]}),
            is_error: false,
        }],
    );
    repo.append(None, &tool_msg).await.expect("append tool");

    let list = repo
        .list_by_session(None, session_id.as_str(), 10, 0)
        .await
        .expect("list");
    assert_eq!(list.len(), 2);
    // Round-trip preserved the JSONB content discriminator + payload.
    match &list[0].content[1] {
        ContentBlock::ToolCall {
            tool_call_id,
            name,
            arguments,
        } => {
            assert_eq!(tool_call_id, "tc_1");
            assert_eq!(name, "search");
            assert_eq!(arguments["query"], "rust async");
            assert_eq!(arguments["limit"], 5);
        }
        other => panic!("expected tool_call, got {other:?}"),
    }
    match &list[1].content[0] {
        ContentBlock::ToolResult {
            tool_call_id,
            output,
            is_error,
        } => {
            assert_eq!(tool_call_id, "tc_1");
            assert!(!is_error);
            assert_eq!(output["hits"][0]["title"], "tokio");
        }
        other => panic!("expected tool_result, got {other:?}"),
    }
}

#[tokio::test]
async fn list_orders_by_created_at_ascending_with_pagination() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let session_id = seed_session(&pool, &tenant, &user).await;
    let repo = PgMessageRepository::new(pool.clone());

    let base = Utc::now() - Duration::minutes(30);
    let mut ids = Vec::with_capacity(4);
    for i in 0..4_i64 {
        let mut m = fixture_message(
            &session_id,
            MessageRole::User,
            vec![ContentBlock::Text {
                text: format!("msg {i}"),
            }],
        );
        m.created_at = base + Duration::minutes(i);
        repo.append(None, &m).await.expect("append");
        ids.push(m.id);
    }

    let page1 = repo
        .list_by_session(None, session_id.as_str(), 2, 0)
        .await
        .expect("page1");
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].id.as_str(), ids[0].as_str());
    assert_eq!(page1[1].id.as_str(), ids[1].as_str());

    let page2 = repo
        .list_by_session(None, session_id.as_str(), 2, 2)
        .await
        .expect("page2");
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[0].id.as_str(), ids[2].as_str());
    assert_eq!(page2[1].id.as_str(), ids[3].as_str());

    let count = repo
        .count_by_session(None, session_id.as_str())
        .await
        .expect("count");
    assert_eq!(count, 4);
}

#[tokio::test]
async fn cascading_delete_when_session_dropped() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let session_id = seed_session(&pool, &tenant, &user).await;
    let msg_repo = PgMessageRepository::new(pool.clone());
    let sess_repo = PgSessionRepository::new(pool.clone());

    for _ in 0..3 {
        let m = fixture_message(
            &session_id,
            MessageRole::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        );
        msg_repo.append(None, &m).await.expect("append");
    }
    assert_eq!(
        msg_repo
            .count_by_session(None, session_id.as_str())
            .await
            .expect("count"),
        3
    );

    // Drop the parent session — FK ON DELETE CASCADE should wipe messages.
    sess_repo
        .delete(None, session_id.as_str())
        .await
        .expect("delete session");
    assert_eq!(
        msg_repo
            .count_by_session(None, session_id.as_str())
            .await
            .expect("count after cascade"),
        0
    );
}

#[tokio::test]
async fn delete_by_session_returns_rowcount_and_is_idempotent() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let session_id = seed_session(&pool, &tenant, &user).await;
    let repo = PgMessageRepository::new(pool.clone());

    for _ in 0..2 {
        let m = fixture_message(
            &session_id,
            MessageRole::User,
            vec![ContentBlock::Text {
                text: "x".to_string(),
            }],
        );
        repo.append(None, &m).await.expect("append");
    }

    let deleted = repo
        .delete_by_session(None, session_id.as_str())
        .await
        .expect("delete");
    assert_eq!(deleted, 2);

    // Idempotent — second call returns 0, no error.
    let again = repo
        .delete_by_session(None, session_id.as_str())
        .await
        .expect("delete again");
    assert_eq!(again, 0);

    assert_eq!(
        repo.count_by_session(None, session_id.as_str())
            .await
            .expect("count"),
        0
    );
}
