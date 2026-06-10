//! T7.2 integration tests for `GET /v1/memories/export` +
//! `POST /v1/memories/import`: round-trip (counts match, embeddings
//! regenerated → recall works in the destination store), fail-soft skips,
//! the `source:imported` auto-tag, explicit `source:` tags respected, the
//! 503-when-absent contract, and the best-effort `memory.*` audit entries
//! through the generic `team_audit` sink.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::hotl::audit::InMemoryHotlAuditSink;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_memory::{InMemoryEmbedder, InMemoryMemoryStore, MemoryStore};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

struct Fixture {
    store: Option<Arc<dyn MemoryStore>>,
    audit: Arc<InMemoryHotlAuditSink>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            store: Some(Arc::new(InMemoryMemoryStore::new(Arc::new(
                InMemoryEmbedder::default_dim(),
            )))),
            audit: Arc::new(InMemoryHotlAuditSink::new()),
        }
    }

    fn without_store() -> Self {
        Self {
            store: None,
            audit: Arc::new(InMemoryHotlAuditSink::new()),
        }
    }

    fn state(&self) -> AppState {
        let backend: Arc<dyn LlmBackend> =
            Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
        AppState {
            sessions: InMemorySessionRepo::arc(),
            messages: InMemoryMessageRepo::arc(),
            backend,
            toolbox: Arc::new(Toolbox::new()),
            agent_defaults: AgentConfig::new("mock"),
            cancels: Arc::new(CancelRegistry::new()),
            mcp_servers: None,
            auth: None,
            audit: None,
            audit_verifier: None,
            audit_chain_exporter: None,
            mcp_publish_enabled: false,
            mcp_supervisor: None,
            today: None,
            eval: None,
            webhook_pusher: None,
            nl_job_compiler: None,
            job_upserter: None,
            session_forker: None,
            usage_reader: None,
            webhook_token_validator: None,
            webhook_token_admin: None,
            scheduler_jobs_reader: None,
            hotl_policy_store: None,
            hotl_enforcer: None,
            hotl_decision_store: None,
            hotl_audit: None,
            outcome_writer: None,
            outcomes_reader: None,
            skill_packs: None,
            memory_store: self.store.clone(),
            workspace_repository: None,
            skill_proposals: None,
            tenant_settings: None,
            skill_author_gate: None,
            skill_audit: None,
            skills_dir: std::path::PathBuf::new(),
            personas: None,
            watchers: None,
            loops: None,
            teams: None,
            incidents: None,
            team_audit: Some(self.audit.clone()),
            decision_registry: Arc::new(
                xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
            ),
        }
    }

    fn audit_actions(&self) -> Vec<String> {
        self.audit
            .snapshot()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<(&str, String)>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some((content_type, text)) => {
            builder = builder.header(header::CONTENT_TYPE, content_type);
            Body::from(text)
        }
        None => Body::empty(),
    };
    let resp = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn create_memory(app: &axum::Router, kind: &str, content: &str, tags: &[&str]) {
    let body = serde_json::json!({"kind": kind, "content": content, "tags": tags}).to_string();
    let (status, text) = send(
        app.clone(),
        "POST",
        "/v1/memories",
        Some(("application/json", body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create memory failed: {text}");
}

#[tokio::test]
async fn export_import_round_trip_with_recall_in_destination() {
    let src = Fixture::new();
    let src_app = router(src.state());
    create_memory(
        &src_app,
        "facts",
        "deploy window is Friday 02:00 UTC",
        &["ops"],
    )
    .await;
    create_memory(&src_app, "preferences", "owner prefers terse answers", &[]).await;

    // Export from the source store.
    let (status, jsonl) = send(src_app.clone(), "GET", "/v1/memories/export", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jsonl.lines().count(), 2);
    assert!(
        !jsonl.contains("content_embedding"),
        "embeddings never exported"
    );

    // Import into a fresh store.
    let dst = Fixture::new();
    let dst_app = router(dst.state());
    let (status, body) = send(
        dst_app.clone(),
        "POST",
        "/v1/memories/import",
        Some(("text/plain", jsonl)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "import failed: {body}");
    let report: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(report["imported"], 2);
    assert_eq!(report["skipped"].as_array().unwrap().len(), 0);

    // Embeddings were regenerated by the destination's embedder → recall works.
    let recall_body = serde_json::json!({
        "query": "deploy window is Friday 02:00 UTC",
        "top_k": 1,
    })
    .to_string();
    let (status, recalled) = send(
        dst_app,
        "POST",
        "/v1/memories/recall",
        Some(("application/json", recall_body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recalled: serde_json::Value = serde_json::from_str(&recalled).unwrap();
    let hits = recalled["data"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0]["memory"]["content"],
        "deploy window is Friday 02:00 UTC"
    );

    // Audit: both operations left best-effort entries.
    assert!(src.audit_actions().contains(&"memory.export".to_string()));
    assert!(dst.audit_actions().contains(&"memory.import".to_string()));
}

#[tokio::test]
async fn export_filters_by_kind_and_rejects_unknown_kind() {
    let fx = Fixture::new();
    let app = router(fx.state());
    create_memory(&app, "facts", "a fact", &[]).await;
    create_memory(&app, "episodes", "an episode", &[]).await;

    let (status, jsonl) = send(app.clone(), "GET", "/v1/memories/export?kind=facts", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jsonl.lines().count(), 1);
    assert!(jsonl.contains("a fact"));

    let (status, _) = send(app, "GET", "/v1/memories/export?kind=bogus", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_is_fail_soft_on_mixed_lines() {
    let fx = Fixture::new();
    let app = router(fx.state());

    let text = concat!(
        r#"{"kind":"facts","content":"good one"}"#,
        "\n",
        "not json\n",
        "\n", // blank — skipped silently, not reported
        r#"{"kind":"bogus","content":"x"}"#,
        "\n",
        r#"{"kind":"episodes","content":"good two"}"#,
        "\n",
    );
    let (status, body) = send(
        app.clone(),
        "POST",
        "/v1/memories/import",
        Some(("text/plain", text.to_string())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let report: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(report["imported"], 2);
    let skipped = report["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0]["line"], 2);
    assert_eq!(skipped[1]["line"], 4);
    assert!(skipped[0]["reason"]
        .as_str()
        .unwrap()
        .contains("invalid JSON"));

    // The good lines really landed.
    let (_, listed) = send(app, "GET", "/v1/memories", None).await;
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn import_auto_tags_source_imported_unless_source_tag_present() {
    let fx = Fixture::new();
    let app = router(fx.state());

    let text = concat!(
        r#"{"kind":"facts","content":"untagged"}"#,
        "\n",
        r#"{"kind":"facts","content":"from im","tags":["source:im"]}"#,
        "\n",
    );
    let (status, _) = send(
        app.clone(),
        "POST",
        "/v1/memories/import",
        Some(("text/plain", text.to_string())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, listed) = send(app, "GET", "/v1/memories", None).await;
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let tags_of = |content: &str| -> Vec<String> {
        listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["content"] == content)
            .unwrap()["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(tags_of("untagged"), vec!["source:imported"]);
    assert_eq!(tags_of("from im"), vec!["source:im"]);
}

#[tokio::test]
async fn import_and_export_return_503_when_store_absent() {
    let fx = Fixture::without_store();
    let app = router(fx.state());

    let (status, _) = send(app.clone(), "GET", "/v1/memories/export", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = send(
        app,
        "POST",
        "/v1/memories/import",
        Some(("text/plain", String::new())),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
