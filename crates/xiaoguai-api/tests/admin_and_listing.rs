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
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        session_forker: None,
        usage_reader: None,
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
        parent_session_id: None,
        forked_from_message_id: None,
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

// ----------------------------------------------------------------------
// v0.11.1 — `/v1/admin/today` (audit-first console substrate).
// ----------------------------------------------------------------------

#[tokio::test]
async fn admin_today_503s_when_reader_not_wired() {
    let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/today")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn admin_today_returns_merged_timeline_sorted_desc() {
    use chrono::Utc;
    use xiaoguai_api::{StaticTodayReader, TodayItem, TodayReader};

    let t0 = Utc::now() - chrono::Duration::hours(2);
    let t1 = Utc::now() - chrono::Duration::hours(1);
    let t2 = Utc::now();
    let reader: Arc<dyn TodayReader> = Arc::new(StaticTodayReader::with_items(vec![
        TodayItem::Chat {
            ts: t0,
            session_id: "sess_chat".into(),
            tenant_id: "ten_a".into(),
            user_id: "u".into(),
            started_at: t0,
            last_message_preview: Some("hi".into()),
            message_count: 2,
            tool_count: 0,
        },
        TodayItem::Scheduled {
            ts: t2,
            job_id: "job_a".into(),
            tenant_id: Some("ten_a".into()),
            run_id: 7,
            attempt: 1,
            status: "succeeded".into(),
            fired_at: t2,
            output_preview: Some("done".into()),
            error_message: None,
            reason: Some("hourly summary".into()),
        },
        TodayItem::Im {
            ts: t1,
            session_id: "sess_im".into(),
            tenant_id: "ten_a".into(),
            provider: "feishu".into(),
            chat_id: "oc_x".into(),
            started_at: t1,
            last_message_preview: None,
            message_count: 5,
        },
    ]));

    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.today = Some(reader);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/today")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 3);
    assert_eq!(v[0]["kind"], "scheduled");
    assert_eq!(v[1]["kind"], "im");
    assert_eq!(v[2]["kind"], "chat");
    // Proactive reason rides through.
    assert_eq!(v[0]["reason"], "hourly summary");
    // Scheduled tenant_id is nullable but populated here.
    assert_eq!(v[0]["tenant_id"], "ten_a");
}

#[tokio::test]
async fn admin_today_filters_by_kind_and_caps_limit() {
    use chrono::Utc;
    use xiaoguai_api::{StaticTodayReader, TodayItem, TodayReader};

    let t = Utc::now();
    let mut items = Vec::new();
    for i in 0i64..3 {
        items.push(TodayItem::Chat {
            ts: t - chrono::Duration::minutes(i),
            session_id: format!("sess_c_{i}"),
            tenant_id: "ten".into(),
            user_id: "u".into(),
            started_at: t,
            last_message_preview: None,
            message_count: 0,
            tool_count: 0,
        });
        items.push(TodayItem::Scheduled {
            ts: t - chrono::Duration::seconds(i),
            job_id: "j".into(),
            tenant_id: None,
            run_id: i,
            attempt: 1,
            status: "succeeded".into(),
            fired_at: t,
            output_preview: None,
            error_message: None,
            reason: None,
        });
    }
    let reader: Arc<dyn TodayReader> = Arc::new(StaticTodayReader::with_items(items));

    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.today = Some(reader);
    let app = router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/admin/today?kind=scheduled&limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 2);
    for row in &v {
        assert_eq!(row["kind"], "scheduled");
    }

    // No filter, default limit applies (50) — fewer items here, so all 6.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/today")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 6);
}

#[tokio::test]
async fn admin_today_passes_since_filter_through() {
    use chrono::Utc;
    use xiaoguai_api::{StaticTodayReader, TodayItem, TodayReader};

    let now = Utc::now();
    let old = now - chrono::Duration::days(2);
    let recent = now - chrono::Duration::minutes(5);
    let reader: Arc<dyn TodayReader> = Arc::new(StaticTodayReader::with_items(vec![
        TodayItem::Chat {
            ts: old,
            session_id: "old".into(),
            tenant_id: "ten".into(),
            user_id: "u".into(),
            started_at: old,
            last_message_preview: None,
            message_count: 0,
            tool_count: 0,
        },
        TodayItem::Chat {
            ts: recent,
            session_id: "new".into(),
            tenant_id: "ten".into(),
            user_id: "u".into(),
            started_at: recent,
            last_message_preview: None,
            message_count: 0,
            tool_count: 0,
        },
    ]));

    let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
    state.today = Some(reader);
    let app = router(state);

    let since = (now - chrono::Duration::hours(1)).to_rfc3339();
    let uri = format!("/v1/admin/today?since={}", urlencoding_simple(&since));
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["session_id"], "new");
}

/// Tiny RFC 3986 encoder for `:` and `+` since `chrono::to_rfc3339`
/// already produces those and the query string would otherwise misread.
fn urlencoding_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            ':' => out.push_str("%3A"),
            '+' => out.push_str("%2B"),
            other => out.push(other),
        }
    }
    out
}

// ----------------------------------------------------------------------
// v0.11.2 — `/v1/admin/eval/*` (eval pane substrate).
// ----------------------------------------------------------------------

mod eval_routes {
    use super::*;
    use std::path::Path;
    use xiaoguai_api::eval::{
        CaseFromSessionSource, EvalService, SessionForCase, StaticCaseFromSessionSource,
        ToolInvocationRecord,
    };
    use xiaoguai_eval::{DefaultEvalAgentBuilder, EvalRunner};
    use xiaoguai_llm::Message as LlmMessage;

    fn write_case(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn build_eval_service(suites_dir: &Path) -> Arc<EvalService> {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(2)));
        let source: Arc<dyn CaseFromSessionSource> =
            Arc::new(StaticCaseFromSessionSource::with_session(SessionForCase {
                session_id: "sess_abc".into(),
                tenant_id: Some("ten".into()),
                input_messages: vec![LlmMessage::user("hello")],
                tool_invocations: vec![ToolInvocationRecord {
                    tool_name: "search".into(),
                    arguments_json: "{\"q\":\"x\"}".into(),
                }],
                final_assistant_text: Some("greetings".into()),
            }));
        Arc::new(EvalService::new(runner, suites_dir.to_path_buf(), source))
    }

    async fn body_json(body: Body) -> Value {
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn eval_suites_503_when_not_wired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/eval/suites")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn eval_run_503_when_not_wired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"suite_name":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn eval_case_from_session_503_when_not_wired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/case-from-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"session_id":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn eval_suites_returns_disk_listing_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let regression = tmp.path().join("regression");
        std::fs::create_dir(&regression).unwrap();
        write_case(
            &regression,
            "a.eval.yaml",
            "id: a\ninput_messages: []\nassertions: []\n",
        );
        write_case(
            tmp.path(),
            "smoke.eval.yaml",
            "id: smoke\ninput_messages: []\nassertions: []\n",
        );
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.eval = Some(build_eval_service(tmp.path()));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/eval/suites")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_arr(resp.into_body()).await;
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["name"], "regression");
        assert_eq!(v[0]["case_count"], 1);
        assert_eq!(v[1]["name"], "smoke");
        assert!(v[1]["case_count"].is_null());
    }

    #[tokio::test]
    async fn eval_run_executes_a_disk_suite() {
        let tmp = tempfile::tempdir().unwrap();
        let suite_dir = tmp.path().join("smoke");
        std::fs::create_dir(&suite_dir).unwrap();
        write_case(
            &suite_dir,
            "greet.eval.yaml",
            "id: greet\n\
             input_messages:\n  - role: user\n    content: hi\n\
             mock_script:\n  turns:\n    - text: hello back\n\
             assertions:\n  - kind: final_message_contains\n    text: hello\n",
        );
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.eval = Some(build_eval_service(tmp.path()));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"suite_name":"smoke"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp.into_body()).await;
        assert_eq!(v["suite"], "smoke");
        assert_eq!(v["results"].as_array().unwrap().len(), 1);
        assert!(v["pass_rate"].as_f64().unwrap() > 0.99);
    }

    #[tokio::test]
    async fn eval_run_400_for_missing_suite_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.eval = Some(build_eval_service(tmp.path()));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"suite_name":"nope"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn eval_case_from_session_returns_yaml_for_known_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.eval = Some(build_eval_service(tmp.path()));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/case-from-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"session_id":"sess_abc"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp.into_body()).await;
        assert_eq!(v["case_id"], "from-session-sess_abc");
        assert_eq!(v["tool_invocation_count"], 1);
        let yaml = v["case_yaml"].as_str().unwrap();
        assert!(yaml.contains("search"));
        assert!(yaml.contains("greetings"));
        assert!(v["suggested_filename"]
            .as_str()
            .unwrap()
            .ends_with(".eval.yaml"));
    }

    #[tokio::test]
    async fn eval_case_from_session_400_for_unknown_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.eval = Some(build_eval_service(tmp.path()));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/eval/case-from-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"session_id":"missing"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

mod scheduler_webhook {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use xiaoguai_api::scheduler::{WebhookPushError, WebhookPusher};

    struct RecordingPusher {
        calls: Mutex<Vec<(String, serde_json::Value)>>,
        deliver: usize,
        err: Option<String>,
    }

    #[async_trait]
    impl WebhookPusher for RecordingPusher {
        async fn push(
            &self,
            route_id: &str,
            detail: serde_json::Value,
        ) -> Result<usize, WebhookPushError> {
            self.calls.lock().push((route_id.into(), detail));
            if let Some(msg) = &self.err {
                return Err(WebhookPushError::Backend(msg.clone()));
            }
            Ok(self.deliver)
        }
    }

    #[tokio::test]
    async fn webhook_503_when_unwired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/webhooks/foo")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn webhook_202_with_delivered_count() {
        let pusher = Arc::new(RecordingPusher {
            calls: Mutex::new(Vec::new()),
            deliver: 3,
            err: None,
        });
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.webhook_pusher = Some(pusher.clone());
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/webhooks/deploy")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"sha":"abc"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(v["delivered"], 3);

        let calls = pusher.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "deploy");
        assert_eq!(calls[0].1, serde_json::json!({"sha": "abc"}));
    }

    #[tokio::test]
    async fn webhook_404_when_no_jobs_bound() {
        let pusher = Arc::new(RecordingPusher {
            calls: Mutex::new(Vec::new()),
            deliver: 0,
            err: None,
        });
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.webhook_pusher = Some(pusher);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/webhooks/nobody")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

mod scheduler_nl_jobs {
    //! v0.12.1 — NL job compile + upsert endpoints.
    use super::*;
    use xiaoguai_api::scheduler::{RecordingJobUpserter, StaticNlJobCompiler};

    fn sample_job() -> serde_json::Value {
        serde_json::json!({
            "id": "j-from-llm",
            "tenant_id": null,
            "name": "scan-hn-daily",
            "description": null,
            "trigger": {"type": "cron", "expr": "0 0 8 * * *"},
            "payload": {"prompt": "scan HN"},
            "retry_policy": {"max_attempts": 3, "initial_backoff_secs": 5, "max_backoff_secs": 60, "multiplier": 2.0},
            "sinks": [],
            "enabled": true,
            "next_fire_at": null,
            "last_fire_at": null,
            "created_at": "2026-05-24T00:00:00Z",
            "updated_at": "2026-05-24T00:00:00Z",
        })
    }

    #[tokio::test]
    async fn compile_503_when_unwired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs/compile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"description":"daily scan"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn compile_rejects_empty_description() {
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.nl_job_compiler = Some(Arc::new(StaticNlJobCompiler {
            job: sample_job(),
            rationale: "x".into(),
        }));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs/compile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"description":"  "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn compile_returns_suggested_job_and_rationale() {
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.nl_job_compiler = Some(Arc::new(StaticNlJobCompiler {
            job: sample_job(),
            rationale: "interpreted as a cron at 08:00 UTC".into(),
        }));
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs/compile")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"description":"每天 8 点扫 HN","tenant_id":"t1"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(v["suggested_job"]["id"], "j-from-llm");
        assert_eq!(v["rationale"], "interpreted as a cron at 08:00 UTC");
    }

    #[tokio::test]
    async fn upsert_503_when_unwired() {
        let app = router(build_state(InMemorySessionRepo::arc(), None, None, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(sample_job().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn upsert_201_records_job() {
        let upserter = Arc::new(RecordingJobUpserter::default());
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.job_upserter = Some(upserter.clone());
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(sample_job().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(v["id"], "j-from-llm");
        let jobs = upserter.jobs.lock();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["id"], "j-from-llm");
    }

    #[tokio::test]
    async fn upsert_400_for_invalid_body() {
        let upserter = Arc::new(RecordingJobUpserter::default());
        let mut state = build_state(InMemorySessionRepo::arc(), None, None, None);
        state.job_upserter = Some(upserter);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/scheduler/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
