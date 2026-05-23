//! Integration coverage for v0.6.3: GET /v1/sessions listing,
//! GET /v1/admin/tenants, and the per-tenant rate-limit middleware.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use parking_lot::Mutex;
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::{router, AppState, CancelRegistry, RateLimiter};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_storage::repositories::{RepoResult, TenantRepository};
use xiaoguai_types::{Session, SessionId, SessionStatus, Tenant, TenantId, TenantStatus, UserId};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

#[derive(Default)]
struct InMemoryTenantRepo {
    rows: Mutex<Vec<Tenant>>,
}

impl InMemoryTenantRepo {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn push(&self, t: Tenant) {
        self.rows.lock().push(t);
    }
}

#[async_trait]
impl TenantRepository for InMemoryTenantRepo {
    async fn create(&self, t: &Tenant) -> RepoResult<()> {
        self.rows.lock().push(t.clone());
        Ok(())
    }
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Tenant>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .find(|t| t.id.as_str() == id)
            .cloned())
    }
    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Tenant>> {
        Ok(self.rows.lock().iter().find(|t| t.name == name).cloned())
    }
    async fn list(&self, limit: i64, offset: i64) -> RepoResult<Vec<Tenant>> {
        let rows = self.rows.lock().clone();
        let offset = usize::try_from(offset.max(0)).unwrap_or(0);
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }
    async fn delete(&self, id: &str) -> RepoResult<()> {
        self.rows.lock().retain(|t| t.id.as_str() != id);
        Ok(())
    }
}

fn build_state(
    sessions: Arc<InMemorySessionRepo>,
    tenants: Option<Arc<dyn TenantRepository>>,
    limiter: Option<Arc<RateLimiter>>,
    auth: Option<Arc<dyn TokenValidator>>,
) -> AppState {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    AppState {
        sessions,
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth,
        authz: None,
        tenants,
        rate_limiter: limiter,
        audit: None,
        audit_verifier: None,
        mcp_publish_enabled: false,
    }
}

fn fixture_session(user_id: &str, tenant: &str, model: &str) -> Session {
    let now = chrono::Utc::now();
    Session {
        id: SessionId::new(),
        tenant_id: TenantId::from(tenant.to_string()),
        user_id: UserId::from(user_id.to_string()),
        title: Some("t".into()),
        created_at: now,
        updated_at: now,
        model: model.into(),
        status: SessionStatus::Active,
    }
}

async fn body_arr(body: Body) -> Vec<Value> {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn list_sessions_returns_only_that_user() {
    use xiaoguai_storage::repositories::SessionRepository;
    let sessions = InMemorySessionRepo::arc();
    sessions
        .create(None, &fixture_session("alice", "ten_a", "m"))
        .await
        .unwrap();
    sessions
        .create(None, &fixture_session("alice", "ten_a", "m"))
        .await
        .unwrap();
    sessions
        .create(None, &fixture_session("bob", "ten_a", "m"))
        .await
        .unwrap();
    let app = router(build_state(sessions, None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions?user_id=alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 2);
    for s in &v {
        assert_eq!(s["user_id"], "alice");
    }
}

#[tokio::test]
async fn list_sessions_400s_when_user_id_missing_and_no_claims() {
    let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_tenants_lists_all() {
    let tenants = InMemoryTenantRepo::arc();
    tenants.push(Tenant {
        id: TenantId::from("ten_a".to_string()),
        name: "alpha".into(),
        display_name: "Alpha".into(),
        created_at: chrono::Utc::now(),
        status: TenantStatus::Active,
    });
    tenants.push(Tenant {
        id: TenantId::from("ten_b".to_string()),
        name: "beta".into(),
        display_name: "Beta".into(),
        created_at: chrono::Utc::now(),
        status: TenantStatus::Suspended,
    });
    let app = router(build_state(
        InMemorySessionRepo::arc(),
        Some(tenants),
        None,
        None,
    ));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/tenants")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 2);
    let names: Vec<&str> = v.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

#[tokio::test]
async fn admin_tenants_500s_when_repo_not_wired() {
    let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/tenants")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn rate_limit_returns_429_when_bucket_drained() {
    // Tight limiter: 0 refill, burst 2 → after 2 successful reqs, the
    // 3rd gets 429.
    let limiter = Arc::new(RateLimiter::new(0.0, 2.0));
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "alice".into(),
            tenant_id: "ten_a".into(),
            roles: vec![],
        },
    });
    let app = router(build_state(
        InMemorySessionRepo::arc(),
        None,
        Some(limiter),
        Some(validator),
    ));
    let mk = || {
        Request::builder()
            .uri("/v1/sessions/sess_x")
            .header(header::AUTHORIZATION, "Bearer t")
            .body(Body::empty())
            .unwrap()
    };
    // First two: 404 (session not found), confirms middleware allowed
    // and the handler ran.
    let r1 = app.clone().oneshot(mk()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::NOT_FOUND);
    let r2 = app.clone().oneshot(mk()).await.unwrap();
    assert_eq!(r2.status(), StatusCode::NOT_FOUND);
    // Third: bucket empty → 429.
    let r3 = app.clone().oneshot(mk()).await.unwrap();
    assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn admin_audit_returns_rows_for_tenant() {
    use chrono::Utc;
    use xiaoguai_api::{AuditEntryView, AuditReader, StaticAuditReader};

    let now = Utc::now();
    let mk = |id: i64, tenant: &str| AuditEntryView {
        id,
        ts: now,
        tenant_id: tenant.into(),
        actor: "system".into(),
        action: "session.create".into(),
        resource: Some(format!("sess_{id}")),
        details: serde_json::json!({"model": "mock"}),
        prev_hmac: "00".repeat(32),
        hmac: "ab".repeat(32),
    };
    let reader: Arc<dyn AuditReader> = Arc::new(StaticAuditReader::with_rows(vec![
        mk(1, "ten_a"),
        mk(2, "ten_b"),
        mk(3, "ten_a"),
    ]));

    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.audit = Some(reader);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit?tenant_id=ten_a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 2);
    for row in &v {
        assert_eq!(row["tenant_id"], "ten_a");
        assert!(row["hmac"].as_str().unwrap().len() == 64);
    }
}

#[tokio::test]
async fn admin_audit_503s_when_reader_not_wired() {
    let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit?tenant_id=ten_a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn admin_audit_verify_returns_ok_for_unbroken_chain() {
    use xiaoguai_api::{AuditVerifier, StaticAuditVerifier, VerifyReport};
    let v: Arc<dyn AuditVerifier> = Arc::new(StaticAuditVerifier::with_verdict(
        "ten_a",
        VerifyReport::Ok { verified_count: 7 },
    ));
    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.audit_verifier = Some(v);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit/verify?tenant_id=ten_a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["verified_count"], 7);
    assert!(body.get("broken_at").is_none() || body["broken_at"].is_null());
}

#[tokio::test]
async fn admin_audit_verify_reports_broken_chain() {
    use xiaoguai_api::{AuditVerifier, StaticAuditVerifier, VerifyReport};
    let v: Arc<dyn AuditVerifier> = Arc::new(StaticAuditVerifier::with_verdict(
        "ten_a",
        VerifyReport::Broken { broken_at: 42 },
    ));
    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.audit_verifier = Some(v);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit/verify?tenant_id=ten_a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["broken_at"], 42);
}

#[tokio::test]
async fn admin_audit_verify_503s_when_verifier_not_wired() {
    let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit/verify?tenant_id=ten_a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn admin_audit_verify_400s_when_tenant_missing() {
    use xiaoguai_api::{AuditVerifier, StaticAuditVerifier};
    let v: Arc<dyn AuditVerifier> = Arc::new(StaticAuditVerifier::default());
    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.audit_verifier = Some(v);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_audit_400s_when_tenant_id_missing() {
    use xiaoguai_api::{AuditReader, StaticAuditReader};
    let reader: Arc<dyn AuditReader> = Arc::new(StaticAuditReader::default());
    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.audit = Some(reader);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rate_limit_is_bypassed_without_claims() {
    // Same limiter, but no auth → no Claims → no tenant key → middleware
    // lets every request through.
    let limiter = Arc::new(RateLimiter::new(0.0, 1.0));
    let app = router(build_state(
        InMemorySessionRepo::arc(),
        None,
        Some(limiter),
        None,
    ));
    for _ in 0..5 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/sess_x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 because session doesn't exist; the point is the middleware
        // didn't 429.
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
