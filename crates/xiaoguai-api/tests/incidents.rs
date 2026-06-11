//! Integration tests for `/v1/incidents` (T6.2 — self-healing GLUE-1).
//!
//! Boots the production router with the in-memory incident store and
//! exercises: token-gated sentry/datadog/manual ingest, dedup, the
//! `incident.open` audit entry, list/get-with-details, the 503 fallbacks
//! (store absent, validator absent — the latter mirrors the scheduler
//! public webhook posture), and 400/404 on garbage.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::hotl::audit::InMemoryHotlAuditSink;
use xiaoguai_api::incident_store::{IncidentStore, RcaRecord, RepairRecord};
use xiaoguai_api::{
    router, AppState, CancelRegistry, InMemoryIncidentStore, StaticWebhookTokenValidator,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

const TOKEN: &str = "tok-incidents-1";
const TOKEN_HEADER: &str = "X-Xiaoguai-Token";

struct Fixture {
    incidents: Arc<InMemoryIncidentStore>,
    audit: Arc<InMemoryHotlAuditSink>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            incidents: Arc::new(InMemoryIncidentStore::new()),
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
            // Same gate mechanism as the scheduler public webhook: token
            // bound to the fixed "incidents" route id.
            webhook_token_validator: Some(Arc::new(StaticWebhookTokenValidator {
                token: TOKEN.to_string(),
                route_id: "incidents".to_string(),
            })),
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
            teams: None,
            incidents: Some(self.incidents.clone()),
            team_audit: Some(self.audit.clone()),
            watchers: None,
            loops: None,
            decision_registry: Arc::new(
                xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
            ),
        }
    }
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(TOKEN_HEADER, t);
    }
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn sentry_payload() -> serde_json::Value {
    json!({
        "action": "created",
        "data": {
            "issue": {
                "id": "123",
                "title": "ZeroDivisionError: division by zero",
                "level": "error",
                "firstSeen": "2026-06-10T01:02:03.000Z",
                "permalink": "https://sentry.io/organizations/acme/issues/123/",
                "project": {"slug": "backend"},
                "tags": [{"key": "environment", "value": "production"}]
            }
        }
    })
}

fn datadog_payload() -> serde_json::Value {
    json!({
        "alert_id": "456",
        "alert_type": "error",
        "alert_priority": "P1",
        "title": "CPU saturated on web-01",
        "last_updated_at": "2026-06-10T02:03:04Z",
        "event_url": "https://app.datadoghq.com/event/456",
        "tags": "env:production,host:web-01"
    })
}

fn manual_payload() -> serde_json::Value {
    json!({
        "id": "manual:disk-full-1",
        "title": "Disk full on backup host",
        "severity": "high",
        "source": "manual",
        "occurred_at": "2026-06-10T03:04:05Z",
        "url": "",
        "project": "infra",
        "environment": "production",
        "raw": {}
    })
}

// ── Ingest: happy paths ───────────────────────────────────────────────────────

#[tokio::test]
async fn sentry_ingest_creates_incident_and_audits() {
    let fx = Fixture::new();
    let app = router(fx.state());

    let (status, body) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(body["was_duplicate"], false);
    let incident = &body["incident"];
    assert_eq!(incident["source"], "sentry");
    assert_eq!(incident["external_id"], "sentry:123");
    assert_eq!(incident["title"], "ZeroDivisionError: division by zero");
    assert_eq!(incident["severity"], "high");
    assert_eq!(incident["status"], "open");
    assert_eq!(incident["environment"], "production");

    // The row is persisted…
    let id: Uuid = incident["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        fx.incidents.get(id).await.unwrap().external_id,
        "sentry:123"
    );

    // …and the open was audited.
    let entries = fx.audit.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "incident.open");
    assert_eq!(
        entries[0].resource.as_deref(),
        Some(&*format!("incident:{id}"))
    );
}

#[tokio::test]
async fn duplicate_reingest_returns_200_was_duplicate() {
    let fx = Fixture::new();
    let app = router(fx.state());

    let (first_status, first) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED);

    let (second_status, second) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(second_status, StatusCode::OK, "body: {second}");
    assert_eq!(second["was_duplicate"], true);
    assert_eq!(second["incident"]["id"], first["incident"]["id"]);

    // Only the first open is audited; one row total.
    assert_eq!(fx.audit.snapshot().len(), 1);
    assert_eq!(fx.incidents.list(None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn datadog_ingest_round_trip() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, body) = send(
        app,
        "POST",
        "/v1/incidents/ingest/datadog",
        Some(TOKEN),
        Some(datadog_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(body["incident"]["external_id"], "datadog:456");
    assert_eq!(body["incident"]["severity"], "critical");
    assert_eq!(body["incident"]["project"], "web-01");
}

#[tokio::test]
async fn manual_ingest_round_trip() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, body) = send(
        app,
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(manual_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(body["incident"]["source"], "manual");
    assert_eq!(body["incident"]["external_id"], "manual:disk-full-1");
}

#[tokio::test]
async fn manual_ingest_cannot_spoof_another_source() {
    // #284: the path `{source}` is authoritative. A manual body claiming
    // `"source": "sentry"` must NOT land in the sentry dedup slot — that
    // would let a manual poster suppress a later real sentry alert.
    let fx = Fixture::new();
    let app = router(fx.state());

    let mut spoofed = manual_payload();
    spoofed["source"] = json!("sentry");
    spoofed["id"] = json!("sentry:123");
    let (status, body) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(spoofed),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    // Stored under the path source, not the body's claim.
    assert_eq!(body["incident"]["source"], "manual");

    // A real sentry alert with the same external id still opens its own
    // incident — the dedup slot was not poisoned.
    let (status, body) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(body["was_duplicate"], false);
    assert_eq!(body["incident"]["source"], "sentry");
    assert_eq!(fx.incidents.list(None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn duplicate_reingest_refreshes_severity_and_payload() {
    // #284: a re-fired alert that escalated must update the live row.
    let fx = Fixture::new();
    let app = router(fx.state());

    let (status, first) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(manual_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(first["incident"]["severity"], "high");

    let mut escalated = manual_payload();
    escalated["severity"] = json!("critical");
    let (status, second) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(escalated.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {second}");
    assert_eq!(second["was_duplicate"], true);
    assert_eq!(second["incident"]["id"], first["incident"]["id"]);
    assert_eq!(second["incident"]["severity"], "critical");
    // The stored raw payload is the re-fired body, not the original.
    assert_eq!(second["incident"]["raw_payload"], escalated);
}

#[tokio::test]
async fn sentry_resolved_action_is_ignored_with_200() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let mut payload = sentry_payload();
    payload["action"] = json!("resolved");
    let (status, body) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ignored"], true);
    assert!(fx.incidents.list(None).await.unwrap().is_empty());
    assert!(fx.audit.snapshot().is_empty());
}

// ── Ingest: bad input ─────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_source_returns_404() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/pagerduty",
        Some(TOKEN),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_sentry_payload_returns_400() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, body) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(json!({"action": "created", "data": {}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn garbage_manual_payload_returns_400() {
    let fx = Fixture::new();
    let app = router(fx.state());
    // Missing required Incident fields.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(json!({"title": "no id"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Structurally valid but semantically empty id.
    let mut empty_id = manual_payload();
    empty_id["id"] = json!("   ");
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/manual",
        Some(TOKEN),
        Some(empty_id),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── Token gate (mirrors scheduler public webhook) ─────────────────────────────

#[tokio::test]
async fn ingest_without_token_returns_401() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        None,
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingest_with_wrong_token_returns_401() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        Some("wrong-token"),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingest_returns_503_when_validator_absent() {
    // Mirror of the scheduler public webhook posture: no validator wired
    // means the out-of-band surface is unavailable, not open.
    let fx = Fixture::new();
    let mut state = fx.state();
    state.webhook_token_validator = None;
    let app = router(state);
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ── 503 when store absent ─────────────────────────────────────────────────────

#[tokio::test]
async fn incidents_routes_return_503_when_store_absent() {
    let fx = Fixture::new();
    let mut state = fx.state();
    state.incidents = None;
    let app = router(state);

    let (status, _) = send(app.clone(), "GET", "/v1/incidents", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = send(
        app.clone(),
        "GET",
        &format!("/v1/incidents/{}", Uuid::new_v4()),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = send(
        app,
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ── Read side ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_filters_by_status_and_rejects_unknown() {
    let fx = Fixture::new();
    let app = router(fx.state());

    send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/datadog",
        Some(TOKEN),
        Some(datadog_payload()),
    )
    .await;

    let (status, list) = send(app.clone(), "GET", "/v1/incidents", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    // Newest first: datadog landed second.
    assert_eq!(list[0]["source"], "datadog");

    let (status, open) = send(app.clone(), "GET", "/v1/incidents?status=open", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(open.as_array().unwrap().len(), 2);

    let (status, resolved) = send(
        app.clone(),
        "GET",
        "/v1/incidents?status=resolved",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(resolved.as_array().unwrap().is_empty());

    let (status, _) = send(app, "GET", "/v1/incidents?status=bogus", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_incident_returns_details_with_rcas_and_repairs() {
    let fx = Fixture::new();
    let app = router(fx.state());

    let (_, body) = send(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    let id: Uuid = body["incident"]["id"].as_str().unwrap().parse().unwrap();

    // Seed RCA + repair through the store (the analyze/approve routes are
    // T6.3/T6.4 — this pins the read-side join only).
    let rca = RcaRecord {
        id: Uuid::new_v4(),
        incident_id: id,
        session_id: format!("incident:{id}"),
        summary: "Division guard missing".to_string(),
        root_cause: "Empty cart".to_string(),
        confidence: 0.8,
        action_items: json!(["add guard"]),
        raw_markdown: "## RCA".to_string(),
        created_at: Utc::now(),
    };
    fx.incidents.insert_rca(&rca).await.unwrap();
    fx.incidents
        .insert_repair(&RepairRecord {
            id: Uuid::new_v4(),
            incident_id: id,
            rca_id: rca.id,
            session_id: format!("incident:{id}"),
            ok: true,
            summary: "guarded".to_string(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let (status, details) = send(
        app.clone(),
        "GET",
        &format!("/v1/incidents/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {details}");
    assert_eq!(details["incident"]["id"].as_str().unwrap(), id.to_string());
    assert_eq!(details["rcas"].as_array().unwrap().len(), 1);
    assert_eq!(details["rcas"][0]["root_cause"], "Empty cart");
    assert_eq!(details["repairs"].as_array().unwrap().len(), 1);
    assert_eq!(details["repairs"][0]["ok"], true);

    // Unknown id → 404.
    let (status, _) = send(
        app,
        "GET",
        &format!("/v1/incidents/{}", Uuid::new_v4()),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
