//! REST handlers for `/v1/incidents` (T6.2 — self-healing GLUE-1).
//!
//! ## Routes (mounted in [`crate::routes::router`])
//!
//! | Method | Path                                | Auth                | Description                         |
//! |--------|-------------------------------------|---------------------|-------------------------------------|
//! | POST   | `/v1/incidents/ingest/{source}`     | `X-Xiaoguai-Token`  | Ingest a sentry/datadog/manual alert |
//! | GET    | `/v1/incidents?status=`             | owner               | List incidents, newest first        |
//! | GET    | `/v1/incidents/{id}`                | owner               | Incident + RCA + repair history     |
//! | POST   | `/v1/incidents/{id}/analyze`        | owner               | Analyst consult turn → RCA (T6.3)   |
//! | POST   | `/v1/incidents/{id}/approve-repair` | owner               | Executor execute turn (T6.4); body `{"rca_id"}` names the approved RCA (#284) |
//! | GET    | `/v1/incidents/{id}/report`         | owner               | Markdown report (T6.4 GLUE-4)       |
//!
//! The analyze/approve handlers `await` the pipeline turn in the request
//! (single in-process agent turns — the orchestrate precedent keeps the
//! request open for the whole run too; no detached task, no 202). The
//! [`IncidentPipeline`] is constructed per request from existing `AppState`
//! fields (all `Arc` clones) — deliberately not another `AppState` field,
//! which would touch every fixture for zero gain.
//!
//! Ingest sits OUTSIDE the owner-auth layer (observability platforms can't
//! do HTTP Basic) and mirrors the scheduler public webhook's token gate
//! exactly — same `X-Xiaoguai-Token` header, same `WebhookTokenValidator`,
//! same 503-when-validator-absent posture — with the fixed route id
//! [`INCIDENTS_WEBHOOK_ROUTE_ID`] (mint a token bound to `incidents` via
//! `/v1/admin/scheduler/tokens`).
//!
//! Ingest status codes (in order of precedence):
//! * 503 — incident store OR token validator unwired
//! * 401 — token missing OR validation returned `false`
//! * 404 — unknown `{source}`
//! * 400 — malformed payload
//! * 200 — ignored event (e.g. Sentry "resolved") or dedup hit
//!   (`was_duplicate: true`)
//! * 201 — fresh incident opened (audited as `incident.open`)

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::incident_pipeline::{render_incident_report, IncidentPipeline, PipelineError};
use crate::incident_store::{IncidentStatus, IncidentStoreError};
use crate::incidents::{
    DatadogSource, Incident, IncidentSource, NormalizeError, SentrySource, Severity,
};
use crate::state::AppState;

/// Same header as the scheduler public webhook route.
const TOKEN_HEADER: &str = "X-Xiaoguai-Token";

/// Fixed `route_id` the ingest token is validated against — one token
/// covers all incident sources.
pub const INCIDENTS_WEBHOOK_ROUTE_ID: &str = "incidents";

// ─── Shared error helpers (teams.rs conventions) ─────────────────────────────

fn incidents_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "incident store not configured"})),
    )
        .into_response()
}

fn err_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

fn map_err(e: IncidentStoreError) -> Response {
    match e {
        IncidentStoreError::NotFound => err_response(StatusCode::NOT_FOUND, "not found"),
        IncidentStoreError::InvalidTransition { from, to } => err_response(
            StatusCode::CONFLICT,
            format!("illegal status transition: {from} → {to}"),
        ),
        IncidentStoreError::InvalidArgument(msg) => err_response(StatusCode::BAD_REQUEST, msg),
        IncidentStoreError::Backend(msg) => {
            tracing::error!(error = %msg, "incidents: store error");
            err_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── Best-effort audit ────────────────────────────────────────────────────────

/// Append an `incident.*` audit entry. Failures are logged and discarded —
/// the incident is already persisted and must not be rolled back over
/// telemetry. Reuses `AppState.team_audit`: despite the field name, the
/// sink is the feature-generic HMAC-chained append adapter (entries differ
/// only by action namespace), so no new audit plumbing is added here.
async fn audit(state: &AppState, action: &str, resource: String, details: serde_json::Value) {
    if let Some(sink) = &state.team_audit {
        let entry = xiaoguai_audit::AuditEntry {
            ts: Utc::now(),
            tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
            actor: "owner".to_string(),
            action: action.to_string(),
            resource: Some(resource),
            details,
        };
        if let Err(e) = sink.append(entry).await {
            tracing::warn!(error = %e, action, "incidents: audit append failed (non-blocking)");
        }
    }
}

// ─── Ingest ───────────────────────────────────────────────────────────────────

/// Normalize the raw body for `{source}`. `None` = unknown source (404).
fn normalize_for_source(
    source: &str,
    raw: serde_json::Value,
) -> Option<Result<Incident, NormalizeError>> {
    match source {
        "sentry" => Some(SentrySource.normalize(raw)),
        "datadog" => Some(DatadogSource.normalize(raw)),
        "manual" => Some(normalize_manual(raw)),
        _ => None,
    }
}

/// Manual ingest (handoff §3.2): humans hand-type these bodies, so only
/// `title` is required — every other [`Incident`] field falls back to a
/// sensible default instead of a 400:
///
/// * `id` → `manual:<uuid>` — fresh per request, so omitting it never
///   collides with the `(source, external_id)` dedup slot of an earlier
///   manual incident;
/// * `severity` → `medium` (the same fallback the datadog adapter uses
///   for a missing priority);
/// * `occurred_at` → now;
/// * `url` → `""`, `project` → `"unknown"` (the sentry/datadog fallbacks);
/// * `environment` → `None`;
/// * `raw` → the entire request body, preserving the [`IncidentSource`]
///   contract that the agent always sees full context.
///
/// Absence (or JSON `null`) defaults; fields that ARE present must still
/// be well-typed — and `id`/`title` non-blank — so garbage keeps failing
/// fast with a clear 400. The body's `source` is ignored entirely: this
/// stamps the `"manual"` constant exactly like the sentry/datadog
/// normalizers stamp theirs, and the handler re-stamps the path
/// `{source}` on top (#284).
fn normalize_manual(raw: serde_json::Value) -> Result<Incident, NormalizeError> {
    let Some(body) = raw.as_object() else {
        return Err(NormalizeError::Malformed(
            "manual incident body must be a JSON object".into(),
        ));
    };

    let title = match manual_opt_string(body, "title")? {
        Some(t) if !t.trim().is_empty() => t,
        _ => {
            return Err(NormalizeError::Malformed(
                "manual incident `title` must be a non-empty string".into(),
            ));
        }
    };

    let id = match manual_opt_string(body, "id")? {
        None => format!("manual:{}", Uuid::new_v4()),
        Some(s) if s.trim().is_empty() => {
            return Err(NormalizeError::Malformed(
                "manual incident `id` must be non-empty".into(),
            ));
        }
        Some(s) => s,
    };

    let severity = match manual_field(body, "severity") {
        None => Severity::Medium,
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| NormalizeError::Malformed(format!("manual incident `severity`: {e}")))?,
    };

    let occurred_at = match manual_opt_string(body, "occurred_at")? {
        None => Utc::now(),
        Some(s) => s.parse::<DateTime<Utc>>().map_err(|e| {
            NormalizeError::Malformed(format!("manual incident `occurred_at`: {e}"))
        })?,
    };

    let url = manual_opt_string(body, "url")?.unwrap_or_default();
    let project = manual_opt_string(body, "project")?.unwrap_or_else(|| "unknown".to_owned());
    let environment = manual_opt_string(body, "environment")?;
    let incident_raw = manual_field(body, "raw")
        .cloned()
        .unwrap_or_else(|| raw.clone());

    Ok(Incident {
        id,
        title,
        severity,
        source: "manual".to_owned(),
        occurred_at,
        url,
        project,
        environment,
        raw: incident_raw,
    })
}

/// `body[field]` with absence and JSON `null` collapsed to `None` — both
/// mean "use the manual-ingest default" (handoff §3.2).
fn manual_field<'a>(
    body: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<&'a serde_json::Value> {
    body.get(field).filter(|v| !v.is_null())
}

/// Optional string field of a manual body: absent/`null` → `Ok(None)`;
/// present but not a JSON string → `Malformed` (defaults cover absence
/// only — they never paper over a wrongly-typed field the caller sent).
fn manual_opt_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<String>, NormalizeError> {
    match manual_field(body, field) {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(NormalizeError::Malformed(format!(
            "manual incident `{field}` must be a string"
        ))),
    }
}

/// `POST /v1/incidents/ingest/{source}` — token-gated alert intake.
pub async fn ingest_incident(
    State(state): State<AppState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(store) = state.incidents.clone() else {
        return incidents_unavailable();
    };
    // Token gate — exact mirror of routes/scheduler_public.rs, including
    // the 503 posture when no validator is wired.
    let Some(validator) = state.webhook_token_validator.clone() else {
        return ApiError::ServiceUnavailable("incident webhook token validator not wired".into())
            .into_response();
    };
    let token = headers
        .get(TOKEN_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .trim();
    if token.is_empty() {
        return ApiError::missing_webhook_token(format!("{TOKEN_HEADER} header missing or empty"))
            .into_response();
    }
    match validator.validate(token, INCIDENTS_WEBHOOK_ROUTE_ID).await {
        Ok(true) => {}
        Ok(false) => {
            return ApiError::invalid_webhook_token("invalid webhook token for incident ingest")
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(anyhow::anyhow!("token validate: {e}")).into_response();
        }
    }

    // Normalize via the existing IncidentSource adapters.
    let mut incident = match normalize_for_source(&source, body.clone()) {
        None => return err_response(StatusCode::NOT_FOUND, format!("unknown source: {source}")),
        Some(Err(NormalizeError::Ignored(action))) => {
            // Known-but-unactionable event (e.g. Sentry "resolved") —
            // 200 no-op per the IncidentSource contract.
            return (
                StatusCode::OK,
                Json(json!({"ignored": true, "action": action})),
            )
                .into_response();
        }
        Some(Err(NormalizeError::Malformed(msg))) => {
            return err_response(StatusCode::BAD_REQUEST, msg);
        }
        Some(Ok(incident)) => incident,
    };
    // #284: the path `{source}` is authoritative — never trust a source
    // claimed inside the body. Without this, a manual ingest carrying
    // `"source": "sentry"` would poison the sentry dedup slot (suppressing
    // a later real sentry alert). All three normalizers now stamp their
    // own constant (manual since handoff §3.2), so this is defense in
    // depth — keep it anyway.
    incident.source = source;

    match store.ingest(&incident, body).await {
        Ok(outcome) => {
            let status = if outcome.was_duplicate {
                StatusCode::OK
            } else {
                audit(
                    &state,
                    "incident.open",
                    format!("incident:{}", outcome.record.id),
                    json!({
                        "source": outcome.record.source,
                        "external_id": outcome.record.external_id,
                        "title": outcome.record.title,
                        "severity": outcome.record.severity,
                    }),
                )
                .await;
                StatusCode::CREATED
            };
            (
                status,
                Json(json!({
                    "incident": outcome.record,
                    "was_duplicate": outcome.was_duplicate,
                })),
            )
                .into_response()
        }
        Err(e) => map_err(e),
    }
}

// ─── Read side ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListIncidentsQuery {
    pub status: Option<String>,
}

/// `GET /v1/incidents?status=` — newest first, optional status filter.
pub async fn list_incidents(
    State(state): State<AppState>,
    Query(query): Query<ListIncidentsQuery>,
) -> Response {
    let Some(store) = state.incidents.clone() else {
        return incidents_unavailable();
    };
    let status = match query.status.as_deref() {
        None => None,
        Some(s) => match IncidentStatus::parse(s) {
            Some(parsed) => Some(parsed),
            None => {
                return err_response(StatusCode::BAD_REQUEST, format!("unknown status: {s}"));
            }
        },
    };
    match store.list(status).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => map_err(e),
    }
}

/// `GET /v1/incidents/{id}` — incident + RCA + repair history.
pub async fn get_incident(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(store) = state.incidents.clone() else {
        return incidents_unavailable();
    };
    match store.get_with_details(id).await {
        Ok(details) => (StatusCode::OK, Json(details)).into_response(),
        Err(e) => map_err(e),
    }
}

// ─── Pipeline (T6.3/T6.4) ─────────────────────────────────────────────────────

/// Build the pipeline from existing `AppState` fields. `None` only when the
/// incident store itself is unwired (→ the same 503 as the read routes).
/// Everything else is mandatory state, so no second 503 axis exists.
fn pipeline_from_state(state: &AppState) -> Option<IncidentPipeline> {
    let store = state.incidents.clone()?;
    Some(IncidentPipeline::new(
        store,
        state.backend.clone(),
        state.toolbox.clone(),
        state.agent_defaults.clone(),
        state.team_audit.clone(),
    ))
}

fn map_pipeline_err(e: PipelineError) -> Response {
    match e {
        // Same table as the T6.2 handlers: NotFound → 404,
        // InvalidTransition → 409 (e.g. analyze while analyzing/resolved,
        // approve while open), Backend → 500.
        PipelineError::Store(e) => map_err(e),
        // `awaiting_approval` without an RCA — state conflict. The stale
        // `rca_id` rejection (#284) is the same class: the approval
        // conflicts with the incident's current analysis state.
        e @ (PipelineError::NoRca | PipelineError::StaleRca { .. }) => {
            err_response(StatusCode::CONFLICT, e.to_string())
        }
        // The agent (upstream of this API) errored or broke the RCA
        // contract; the incident was reverted to `open` and is retryable.
        e @ (PipelineError::AnalysisRun(_) | PipelineError::RcaParse(_)) => {
            err_response(StatusCode::BAD_GATEWAY, e.to_string())
        }
    }
}

/// `POST /v1/incidents/{id}/analyze` — run the Analyst consult turn
/// (T6.3). 409 unless the incident is `open`; 502 when the agent fails or
/// breaks the RCA contract (the incident reverts to `open`, retryable).
pub async fn analyze_incident(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(pipeline) = pipeline_from_state(&state) else {
        return incidents_unavailable();
    };
    match pipeline.analyze(id).await {
        Ok(rca) => (
            StatusCode::OK,
            Json(json!({
                "rca": rca,
                "status": IncidentStatus::AwaitingApproval,
            })),
        )
            .into_response(),
        Err(e) => map_pipeline_err(e),
    }
}

/// `POST /v1/incidents/{id}/approve-repair` request body (#284): the
/// approval must name the RCA it was made against, so a stale approval
/// (e.g. fired from an outdated UI view) can never execute a different
/// analysis than the one the owner reviewed.
#[derive(Debug, Deserialize)]
pub struct ApproveRepairRequest {
    pub rca_id: Uuid,
}

/// `POST /v1/incidents/{id}/approve-repair` — the explicit human approval
/// point (T6.4). The body must carry the `rca_id` being approved (#284);
/// 400 when missing, 409 when it is not the incident's latest RCA. 409
/// unless `awaiting_approval`. A repair that ran but did not succeed is
/// still 200: the attempt is recorded (`ok: false`) and the incident lands
/// on `failed`.
pub async fn approve_repair(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<ApproveRepairRequest>>,
) -> Response {
    let Some(pipeline) = pipeline_from_state(&state) else {
        return incidents_unavailable();
    };
    // #284: `rca_id` is mandatory. `Option<Json<…>>` keeps the 503-store
    // check above ahead of body validation (matching the ingest handler's
    // status-code precedence) and turns a missing body into a clear 400.
    let Some(Json(ApproveRepairRequest { rca_id })) = body else {
        return err_response(
            StatusCode::BAD_REQUEST,
            "request body must be JSON with `rca_id` — the RCA this approval was made against",
        );
    };
    match pipeline.approve_repair(id, rca_id).await {
        Ok(repair) => {
            let status = if repair.ok {
                IncidentStatus::Resolved
            } else {
                IncidentStatus::Failed
            };
            (
                StatusCode::OK,
                Json(json!({"repair": repair, "status": status})),
            )
                .into_response()
        }
        Err(e) => map_pipeline_err(e),
    }
}

/// `GET /v1/incidents/{id}/report` — the composed markdown report (T6.4
/// GLUE-4): status header + the existing 5-section RCA renderer + repairs.
pub async fn incident_report(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(store) = state.incidents.clone() else {
        return incidents_unavailable();
    };
    match store.get_with_details(id).await {
        Ok(details) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8",
            )],
            render_incident_report(&details),
        )
            .into_response(),
        Err(e) => map_err(e),
    }
}

// ─── Unit tests: normalize_manual (handoff §3.2) ──────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-§3.2 full shape must keep working unchanged (scripts that
    /// already send every field).
    #[test]
    fn manual_full_body_normalizes_every_field() {
        let body = json!({
            "id": "manual:disk-full-1",
            "title": "Disk full on backup host",
            "severity": "high",
            "source": "manual",
            "occurred_at": "2026-06-10T03:04:05Z",
            "url": "https://wiki.internal/runbooks/disk-full",
            "project": "infra",
            "environment": "production",
            "raw": {"note": "from runbook"}
        });
        let incident = normalize_manual(body).expect("full body normalizes");
        assert_eq!(incident.id, "manual:disk-full-1");
        assert_eq!(incident.title, "Disk full on backup host");
        assert_eq!(incident.severity, Severity::High);
        assert_eq!(incident.source, "manual");
        assert_eq!(
            incident.occurred_at,
            "2026-06-10T03:04:05Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(incident.url, "https://wiki.internal/runbooks/disk-full");
        assert_eq!(incident.project, "infra");
        assert_eq!(incident.environment.as_deref(), Some("production"));
        // An explicit `raw` is honored verbatim (pre-§3.2 behavior).
        assert_eq!(incident.raw, json!({"note": "from runbook"}));
    }

    #[test]
    fn manual_minimal_title_only_body_gets_defaults() {
        let body = json!({"title": "Disk full on backup host"});
        let incident = normalize_manual(body.clone()).expect("minimal body normalizes");
        assert_eq!(incident.title, "Disk full on backup host");
        assert!(incident.id.starts_with("manual:"), "id: {}", incident.id);
        assert_eq!(incident.severity, Severity::Medium);
        assert_eq!(incident.source, "manual");
        assert_eq!(incident.url, "");
        assert_eq!(incident.project, "unknown");
        assert!(incident.environment.is_none());
        // Missing `raw` defaults to the entire request body.
        assert_eq!(incident.raw, body);
        // `occurred_at` defaults to (roughly) now.
        let drift = (Utc::now() - incident.occurred_at).num_seconds().abs();
        assert!(drift < 60, "occurred_at drifted {drift}s from now");
    }

    #[test]
    fn manual_omitted_id_is_fresh_per_request() {
        // Two identical minimal bodies must NOT collide in the
        // (source, external_id) dedup slot — each opens its own incident.
        let a = normalize_manual(json!({"title": "t"})).unwrap();
        let b = normalize_manual(json!({"title": "t"})).unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn manual_null_fields_default_like_missing_ones() {
        let body = json!({
            "title": "t",
            "id": null,
            "severity": null,
            "occurred_at": null,
            "url": null,
            "project": null,
            "environment": null,
            "raw": null
        });
        let incident = normalize_manual(body.clone()).expect("nulls default");
        assert!(incident.id.starts_with("manual:"), "id: {}", incident.id);
        assert_eq!(incident.severity, Severity::Medium);
        assert_eq!(incident.url, "");
        assert_eq!(incident.project, "unknown");
        assert!(incident.environment.is_none());
        assert_eq!(incident.raw, body);
    }

    #[test]
    fn manual_body_source_is_ignored_and_stamped_manual() {
        // #284 belt: even before the handler re-stamps the path source,
        // the normalizer never trusts a body-claimed source.
        let incident = normalize_manual(json!({"title": "t", "source": "sentry"})).unwrap();
        assert_eq!(incident.source, "manual");
    }

    #[test]
    fn manual_missing_or_blank_title_is_malformed() {
        for body in [
            json!({}),
            json!({"severity": "high"}),
            json!({"title": "   "}),
            json!({"title": null}),
            json!({"title": 42}),
        ] {
            assert!(
                matches!(
                    normalize_manual(body.clone()),
                    Err(NormalizeError::Malformed(_))
                ),
                "body should be malformed: {body}"
            );
        }
    }

    #[test]
    fn manual_blank_or_non_string_id_is_malformed() {
        for body in [
            json!({"title": "t", "id": "   "}),
            json!({"title": "t", "id": 7}),
        ] {
            assert!(
                matches!(
                    normalize_manual(body.clone()),
                    Err(NormalizeError::Malformed(_))
                ),
                "body should be malformed: {body}"
            );
        }
    }

    #[test]
    fn manual_present_but_invalid_fields_are_malformed() {
        // Defaults cover absence only — a field the caller did send must
        // still be well-typed.
        for body in [
            json!({"title": "t", "severity": "urgent"}),
            json!({"title": "t", "severity": 3}),
            json!({"title": "t", "occurred_at": "yesterday"}),
            json!({"title": "t", "occurred_at": 1_718_000_000}),
            json!({"title": "t", "url": 1}),
            json!({"title": "t", "project": ["infra"]}),
            json!({"title": "t", "environment": {}}),
        ] {
            assert!(
                matches!(
                    normalize_manual(body.clone()),
                    Err(NormalizeError::Malformed(_))
                ),
                "body should be malformed: {body}"
            );
        }
    }

    #[test]
    fn manual_non_object_body_is_malformed() {
        for body in [
            json!("just a string"),
            json!([1, 2, 3]),
            json!(null),
            json!(42),
        ] {
            assert!(
                matches!(
                    normalize_manual(body.clone()),
                    Err(NormalizeError::Malformed(_))
                ),
                "body should be malformed: {body}"
            );
        }
    }
}
