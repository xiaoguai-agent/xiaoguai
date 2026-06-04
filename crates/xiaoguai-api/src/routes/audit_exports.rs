//! `POST /v1/audit/exports` — T5 compliance export.
//!
//! Builds a SOC2 / GDPR / HIPAA bundle over `[from, to]`. The underlying
//! adapter re-verifies chain continuity inside the window and refuses if
//! broken — there is no `skip_verify` flag.
//!
//! Status code mapping:
//! - 200 OK + body — bundle rendered, `Content-Type` matches `format`.
//! - 400 Bad Request — missing/invalid `framework`, `format`, or `from > to`.
//! - 409 Conflict — chain broken inside the window. Body is the structured
//!   error JSON with `first_broken_id` + `first_broken_ts`.
//! - 501 Not Implemented — PDF format (stub).
//! - 503 Service Unavailable — exporter not wired (`audit_chain_exporter`
//!   is `None`, typically because the signing key env var is unset).

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Response, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::audit::{ExportError, ExportRequest};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Wire shape for `POST /v1/audit/exports`.
#[derive(Debug, Deserialize)]
pub struct AuditExportRequest {
    /// Short framework name — `"soc2"`, `"gdpr"`, `"hipaa"`. The adapter
    /// also accepts the long forms (`"soc2-cc7.2"`, etc.) — see
    /// `xiaoguai_audit::Framework::parse`.
    pub framework: String,
    /// Output format — `"json"`, `"csv"`, or `"pdf"`. PDF returns 501.
    #[serde(default = "default_format")]
    pub format: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

fn default_format() -> String {
    "json".into()
}

/// Structured 409 body returned when the chain breaks inside the window.
#[derive(Debug, Serialize)]
struct ChainBrokenBody {
    error: &'static str,
    first_broken_id: i64,
    first_broken_ts: DateTime<Utc>,
}

/// # Errors
///
/// See the module docs for the status-code mapping.
pub async fn export_audit(
    State(state): State<AppState>,
    Json(req): Json<AuditExportRequest>,
) -> ApiResult<Response<Body>> {
    let exporter = state
        .audit_chain_exporter
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("audit chain exporter not wired".into()))?;

    if req.framework.is_empty() {
        return Err(ApiError::InvalidRequest(
            "framework must not be empty".into(),
        ));
    }
    if req.from > req.to {
        return Err(ApiError::InvalidRequest(
            "from must be <= to (RFC3339 timestamps)".into(),
        ));
    }

    let content_type = match req.format.to_ascii_lowercase().as_str() {
        "json" => "application/json",
        "csv" => "text/csv",
        "pdf" => "application/pdf",
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "unknown format: {other} (expected json|csv|pdf)"
            )));
        }
    };

    let result = exporter
        .export(ExportRequest {
            // Single implicit owner: the audit chain HMAC is signed/verified
            // with the audit-crate OWNER value, so the export tenant must match.
            tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
            framework: req.framework,
            format: req.format,
            from: req.from,
            to: req.to,
        })
        .await;

    match result {
        Ok(bytes) => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(bytes))
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("build response: {e}")))?),
        Err(ExportError::ChainBroken {
            first_broken_id,
            first_broken_ts,
        }) => {
            let body = serde_json::to_vec(&ChainBrokenBody {
                error: "chain_broken",
                first_broken_id,
                first_broken_ts,
            })
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("encode chain_broken: {e}")))?;
            Response::builder()
                .status(StatusCode::CONFLICT)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .map_err(|e| ApiError::Internal(anyhow::anyhow!("build 409: {e}")))
        }
        Err(ExportError::PdfUnimplemented) => Response::builder()
            .status(StatusCode::NOT_IMPLEMENTED)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                br#"{"error":"pdf_unimplemented","message":"pdf export is a follow-up; track in post-T5"}"#.to_vec(),
            ))
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("build 501: {e}"))),
        Err(ExportError::InvalidArgument { message }) => Err(ApiError::InvalidRequest(message)),
        Err(ExportError::Backend { message }) => {
            Err(ApiError::Internal(anyhow::anyhow!("export backend: {message}")))
        }
    }
}
