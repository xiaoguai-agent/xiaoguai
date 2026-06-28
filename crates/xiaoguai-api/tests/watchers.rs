//! Integration test for `/v1/watchers/*` mount (sprint-10b S10b-5).
//!
//! Boots the production router with a `StaticWatcherIntrospector` (the
//! zero-watcher steady state) and a custom in-test impl that exposes a
//! single watcher so we can exercise pause / resume. Covers:
//!
//!   * `GET /v1/watchers?session_id=…` returns 200 + `[]` with the
//!     static introspector — drops the frontend client's 404 fallback.
//!   * `POST /v1/watchers/:id/pause` and `/resume` return 204 No Content
//!     when the watcher exists, 404 when it doesn't.
//!   * Every endpoint returns 503 when `watchers` is `None`.
//!
//! Why not extend `xiaoguai-watch::WatchRunner` directly: see the
//! module docs of `crates/xiaoguai-api/src/watchers.rs`. The runner is
//! not session-aware today and adding that capability is architectural
//! surgery the sprint plan flagged as out of scope.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use parking_lot::Mutex;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{
    router, AppState, CancelRegistry, StaticWatcherIntrospector, WatcherError, WatcherInfo,
    WatcherIntrospector, WatcherSourceType, WatcherStatus,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(watchers: Option<Arc<dyn WatcherIntrospector>>) -> AppState {
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
        memory_store: None,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers,
        loops: None,
        teams: None,
        incidents: None,
        team_audit: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
        pack_rescanner: None,
        coding_toolbox_factory: None,
    }
}

#[tokio::test]
async fn list_watchers_returns_200_and_empty_array_with_static_impl() {
    let app = router(build_state(Some(StaticWatcherIntrospector::arc())));
    let req = Request::builder()
        .uri("/v1/watchers?session_id=sess_abc")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /v1/watchers must return 200 once mounted; the static \
         introspector reports zero watchers as an empty list, not a \
         404 (which is what the client falls back to today)"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(arr.is_array(), "shape: bare JSON array");
    assert_eq!(arr.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn pause_unknown_watcher_returns_404_with_static_impl() {
    let app = router(build_state(Some(StaticWatcherIntrospector::arc())));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/watchers/w_nope/pause")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_watchers_returns_503_when_introspector_is_none() {
    let app = router(build_state(None));
    let req = Request::builder()
        .uri("/v1/watchers?session_id=sess_abc")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn pause_returns_503_when_introspector_is_none() {
    let app = router(build_state(None));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/watchers/w1/pause")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── Fake introspector with a known watcher for happy-path coverage ─────────

#[derive(Default)]
struct SingleWatcherIntrospector {
    paused: Mutex<bool>,
}

impl SingleWatcherIntrospector {
    const WATCHER_ID: &'static str = "w_known";
    const SESSION_ID: &'static str = "sess_with_watcher";
}

#[async_trait]
impl WatcherIntrospector for SingleWatcherIntrospector {
    async fn list_for_session(&self, session_id: &str) -> Result<Vec<WatcherInfo>, WatcherError> {
        if session_id != Self::SESSION_ID {
            return Ok(Vec::new());
        }
        let status = if *self.paused.lock() {
            WatcherStatus::Paused
        } else {
            WatcherStatus::Running
        };
        Ok(vec![WatcherInfo {
            id: Self::WATCHER_ID.into(),
            name: "Nightly report watcher".into(),
            source_type: WatcherSourceType::Schedule,
            last_fired_at: None,
            status,
            schedule: Some("0 0 * * *".into()),
        }])
    }

    async fn pause(&self, watcher_id: &str) -> Result<(), WatcherError> {
        if watcher_id == Self::WATCHER_ID {
            *self.paused.lock() = true;
            Ok(())
        } else {
            Err(WatcherError::NotFound(watcher_id.to_string()))
        }
    }

    async fn resume(&self, watcher_id: &str) -> Result<(), WatcherError> {
        if watcher_id == Self::WATCHER_ID {
            *self.paused.lock() = false;
            Ok(())
        } else {
            Err(WatcherError::NotFound(watcher_id.to_string()))
        }
    }
}

#[tokio::test]
async fn list_pause_resume_round_trip_with_fake() {
    let intro: Arc<dyn WatcherIntrospector> = Arc::new(SingleWatcherIntrospector::default());
    let app = router(build_state(Some(intro)));

    // List shows one running watcher.
    let req = Request::builder()
        .uri("/v1/watchers?session_id=sess_with_watcher")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(arr.as_array().unwrap().len(), 1);
    assert_eq!(arr[0]["status"], "running");

    // Pause it.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/watchers/w_known/pause")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Now it's paused.
    let req = Request::builder()
        .uri("/v1/watchers?session_id=sess_with_watcher")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(arr[0]["status"], "paused");

    // Resume it.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/watchers/w_known/resume")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}
