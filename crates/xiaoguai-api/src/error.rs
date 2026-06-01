//! API-layer error type. Maps onto HTTP status codes via [`IntoResponse`].
//!
//! Conventions:
//!   - 4xx errors include a stable `code` slug so clients can switch on it
//!     without parsing free-form messages.
//!   - 5xx errors render a generic `internal_error` slug; the original cause
//!     is logged via `tracing` but not surfaced to the response body.
//!
//! ## WWW-Authenticate (RFC 6750)
//!
//! [`ApiError::Unauthorized`] emits a well-formed `WWW-Authenticate` header
//! on every 401 response, per RFC 6750 §3:
//!
//! ```text
//! WWW-Authenticate: Bearer realm="<realm>", error="<error>",
//!                          error_description="<description>"
//! ```
//!
//! `error` and `error_description` are omitted when not provided.
//! Embedded `"` and `\` in the description are backslash-escaped so the
//! header value is always syntactically valid.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;
use xiaoguai_agent::AgentError;
use xiaoguai_storage::repositories::RepoError;

/// RFC 6750 §3.1 standardised `error` codes for Bearer challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnauthorizedReason {
    /// `invalid_request` — the request is missing a required parameter,
    /// includes an unsupported parameter value (other than grant type),
    /// repeats a parameter, or is otherwise malformed.  Use when the token
    /// header / query param is absent or structurally invalid.
    InvalidRequest,
    /// `invalid_token` — the token is expired, revoked, malformed, or
    /// otherwise invalid.  Use when a token is present but fails validation.
    InvalidToken,
    /// `insufficient_scope` — the token does not carry the scope required
    /// by this resource.
    InsufficientScope,
}

impl UnauthorizedReason {
    /// Returns the RFC 6750 wire string for the `error` parameter.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidToken => "invalid_token",
            Self::InsufficientScope => "insufficient_scope",
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    /// 401 Unauthorized with optional RFC 6750 challenge parameters.
    ///
    /// `realm` — advertised protection space (e.g. `"api"`, `"webhook"`).
    /// `error` — optional RFC 6750 error code.
    /// `description` — optional human-readable detail; embedded `"` / `\`
    ///   are escaped before emission.
    #[error("unauthorized: {}", description.as_deref().unwrap_or("unauthorized"))]
    Unauthorized {
        realm: &'static str,
        error: Option<UnauthorizedReason>,
        description: Option<String>,
    },
    /// 403 Forbidden — the request's bearer token authenticated, but did
    /// not carry the OAuth-style scope required by the route. Rendered
    /// with the api-contract §1.6 nested envelope:
    ///
    /// ```json
    /// {"error":{"code":"scope_required",
    ///           "message":"missing required scope: <slug>",
    ///           "details":{"scope":"<slug>"}}}
    /// ```
    ///
    /// Sprint-14 S14-1 (DEC-HLD-018). Constructed via
    /// [`ApiError::scope_required`].
    #[error("missing required scope: {scope}")]
    ScopeRequired { scope: &'static str },
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("gateway timeout: {0}")]
    GatewayTimeout(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("storage: {0}")]
    Storage(#[from] RepoError),
    #[error("agent: {0}")]
    Agent(#[from] AgentError),
}

impl ApiError {
    /// Shorthand: missing / malformed token on an API endpoint.
    pub fn missing_token(description: impl Into<String>) -> Self {
        Self::Unauthorized {
            realm: "api",
            error: Some(UnauthorizedReason::InvalidRequest),
            description: Some(description.into()),
        }
    }

    /// Shorthand: token present but invalid or expired.
    pub fn invalid_token(description: impl Into<String>) -> Self {
        Self::Unauthorized {
            realm: "api",
            error: Some(UnauthorizedReason::InvalidToken),
            description: Some(description.into()),
        }
    }

    /// Shorthand: missing / malformed webhook token.
    pub fn missing_webhook_token(description: impl Into<String>) -> Self {
        Self::Unauthorized {
            realm: "webhook",
            error: Some(UnauthorizedReason::InvalidRequest),
            description: Some(description.into()),
        }
    }

    /// Shorthand: webhook token present but invalid.
    pub fn invalid_webhook_token(description: impl Into<String>) -> Self {
        Self::Unauthorized {
            realm: "webhook",
            error: Some(UnauthorizedReason::InvalidToken),
            description: Some(description.into()),
        }
    }

    /// Shorthand: bearer token authenticated but did not carry the
    /// scope required by the route. Renders as 403 with the
    /// api-contract §1.6 nested envelope (see [`Self::ScopeRequired`]).
    #[must_use]
    pub fn scope_required(scope: &'static str) -> Self {
        Self::ScopeRequired { scope }
    }
}

/// Escape `"` and `\` in a `WWW-Authenticate` quoted-string value.
///
/// RFC 7235 §2.1 defines the `quoted-string` production: any character
/// except `"` and `\` is allowed; those two must be preceded by `\`.
fn escape_www_auth_description(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

/// Build the `WWW-Authenticate` header value for a Bearer challenge.
fn build_www_authenticate(
    realm: &str,
    error: Option<&UnauthorizedReason>,
    description: Option<&str>,
) -> String {
    let mut parts = vec![format!("Bearer realm=\"{}\"", realm)];
    if let Some(e) = error {
        parts.push(format!("error=\"{}\"", e.as_str()));
    }
    if let Some(d) = description {
        parts.push(format!(
            "error_description=\"{}\"",
            escape_www_auth_description(d)
        ));
    }
    parts.join(", ")
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

/// Nested envelope variant used by [`ApiError::ScopeRequired`] — matches
/// api-contract §1.6 `{"error":{"code","message","details"}}`. Sprint-14
/// S14-1 introduces this shape for new error codes; existing 4xx error
/// codes keep the flat [`ErrorBody`] shape until a broader migration
/// (out of scope for this sprint).
#[derive(Serialize)]
struct NestedErrorBody<'a> {
    error: NestedErrorInner<'a>,
}

#[derive(Serialize)]
struct NestedErrorInner<'a> {
    code: &'a str,
    message: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    details: serde_json::Value,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            // Sprint-14 S14-1: nested envelope per api-contract §1.6.
            // BREAKING vs sprint-13's flat
            // `{"error":"forbidden","required_scope":"..."}` shape.
            Self::ScopeRequired { scope } => {
                let message = format!("missing required scope: {scope}");
                let body = Json(NestedErrorBody {
                    error: NestedErrorInner {
                        code: "scope_required",
                        message,
                        details: serde_json::json!({ "scope": scope }),
                    },
                });
                (StatusCode::FORBIDDEN, body).into_response()
            }
            Self::Unauthorized {
                realm,
                ref error,
                ref description,
            } => {
                let www_auth =
                    build_www_authenticate(realm, error.as_ref(), description.as_deref());
                let body = Json(ErrorBody {
                    code: "unauthorized",
                    message: self.to_string(),
                });
                let mut response = (StatusCode::UNAUTHORIZED, body).into_response();
                if let Ok(val) = HeaderValue::from_str(&www_auth) {
                    response.headers_mut().insert(header::WWW_AUTHENTICATE, val);
                }
                response
            }
            other => {
                let (status, code, message) = match &other {
                    Self::NotFound => (StatusCode::NOT_FOUND, "not_found", other.to_string()),
                    Self::BadRequest(_) => {
                        (StatusCode::BAD_REQUEST, "bad_request", other.to_string())
                    }
                    Self::InvalidRequest(_) => (
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        other.to_string(),
                    ),
                    Self::Conflict(_) => (StatusCode::CONFLICT, "conflict", other.to_string()),
                    Self::ServiceUnavailable(_) => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "service_unavailable",
                        other.to_string(),
                    ),
                    Self::PayloadTooLarge(_) => (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "payload_too_large",
                        other.to_string(),
                    ),
                    Self::GatewayTimeout(_) => (
                        StatusCode::GATEWAY_TIMEOUT,
                        "gateway_timeout",
                        other.to_string(),
                    ),
                    Self::Internal(err) => {
                        tracing::error!(?err, "internal api error");
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "internal_error",
                            "internal error".to_string(),
                        )
                    }
                    Self::Storage(err) => {
                        tracing::error!(?err, "storage error in api");
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "storage_error",
                            "storage failure".to_string(),
                        )
                    }
                    Self::Agent(err) => {
                        tracing::error!(?err, "agent error in api");
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "agent_error",
                            "agent failure".to_string(),
                        )
                    }
                    // Unauthorized + ScopeRequired arms are handled
                    // above; these branches are unreachable but required
                    // for exhaustiveness.
                    Self::Unauthorized { .. } | Self::ScopeRequired { .. } => unreachable!(),
                };
                (status, Json(ErrorBody { code, message })).into_response()
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    // ── escape_www_auth_description ──────────────────────────────────────

    #[test]
    fn escape_plain_string_unchanged() {
        assert_eq!(
            escape_www_auth_description("Token expired"),
            "Token expired"
        );
    }

    #[test]
    fn escape_double_quote() {
        assert_eq!(
            escape_www_auth_description(r#"has "quotes" inside"#),
            r#"has \"quotes\" inside"#
        );
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(
            escape_www_auth_description(r"has\backslash"),
            r"has\\backslash"
        );
    }

    #[test]
    fn escape_both_quote_and_backslash() {
        assert_eq!(
            escape_www_auth_description(r#"say \"hi\""#),
            r#"say \\\"hi\\\""#
        );
    }

    // ── build_www_authenticate ───────────────────────────────────────────

    #[test]
    fn www_auth_realm_only() {
        let s = build_www_authenticate("api", None, None);
        assert_eq!(s, r#"Bearer realm="api""#);
    }

    #[test]
    fn www_auth_invalid_request() {
        let s = build_www_authenticate(
            "api",
            Some(&UnauthorizedReason::InvalidRequest),
            Some("X-Xiaoguai-Token header missing or empty"),
        );
        assert_eq!(
            s,
            r#"Bearer realm="api", error="invalid_request", error_description="X-Xiaoguai-Token header missing or empty""#
        );
    }

    #[test]
    fn www_auth_invalid_token() {
        let s = build_www_authenticate(
            "api",
            Some(&UnauthorizedReason::InvalidToken),
            Some("Token expired at 2024-01-01T00:00:00Z"),
        );
        assert_eq!(
            s,
            r#"Bearer realm="api", error="invalid_token", error_description="Token expired at 2024-01-01T00:00:00Z""#
        );
    }

    #[test]
    fn www_auth_insufficient_scope() {
        let s = build_www_authenticate("api", Some(&UnauthorizedReason::InsufficientScope), None);
        assert_eq!(s, r#"Bearer realm="api", error="insufficient_scope""#);
    }

    #[test]
    fn www_auth_webhook_realm() {
        let s = build_www_authenticate(
            "webhook",
            Some(&UnauthorizedReason::InvalidRequest),
            Some("X-Xiaoguai-Token header missing or empty"),
        );
        assert_eq!(
            s,
            r#"Bearer realm="webhook", error="invalid_request", error_description="X-Xiaoguai-Token header missing or empty""#
        );
    }

    #[test]
    fn www_auth_description_with_embedded_quotes_is_escaped() {
        let s = build_www_authenticate(
            "api",
            Some(&UnauthorizedReason::InvalidToken),
            Some(r#"Token "abc" is invalid"#),
        );
        assert_eq!(
            s,
            r#"Bearer realm="api", error="invalid_token", error_description="Token \"abc\" is invalid""#
        );
    }

    // ── UnauthorizedReason::as_str ───────────────────────────────────────

    #[test]
    fn reason_wire_strings() {
        assert_eq!(
            UnauthorizedReason::InvalidRequest.as_str(),
            "invalid_request"
        );
        assert_eq!(UnauthorizedReason::InvalidToken.as_str(), "invalid_token");
        assert_eq!(
            UnauthorizedReason::InsufficientScope.as_str(),
            "insufficient_scope"
        );
    }

    // ── IntoResponse — status code + header ──────────────────────────────

    #[test]
    fn into_response_status_401() {
        let err = ApiError::Unauthorized {
            realm: "api",
            error: Some(UnauthorizedReason::InvalidToken),
            description: Some("bad token".into()),
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn into_response_has_www_authenticate_header() {
        let err = ApiError::Unauthorized {
            realm: "api",
            error: Some(UnauthorizedReason::InvalidToken),
            description: Some("bad token".into()),
        };
        let resp = err.into_response();
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .expect("WWW-Authenticate must be present on 401");
        assert_eq!(
            hdr.to_str().unwrap(),
            r#"Bearer realm="api", error="invalid_token", error_description="bad token""#
        );
    }

    #[test]
    fn missing_token_shorthand_has_invalid_request_error() {
        let err = ApiError::missing_token("Authorization header missing");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(hdr.contains(r#"error="invalid_request""#), "header: {hdr}");
        assert!(hdr.contains(r#"realm="api""#), "header: {hdr}");
    }

    #[test]
    fn invalid_token_shorthand_has_invalid_token_error() {
        let err = ApiError::invalid_token("Token expired at 2024-01-01T00:00:00Z");
        let resp = err.into_response();
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(hdr.contains(r#"error="invalid_token""#), "header: {hdr}");
        assert!(
            hdr.contains("Token expired at 2024-01-01T00:00:00Z"),
            "header: {hdr}"
        );
    }

    #[test]
    fn missing_webhook_token_uses_webhook_realm() {
        let err = ApiError::missing_webhook_token("X-Xiaoguai-Token header missing");
        let resp = err.into_response();
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(hdr.contains(r#"realm="webhook""#), "header: {hdr}");
        assert!(hdr.contains(r#"error="invalid_request""#), "header: {hdr}");
    }

    #[test]
    fn invalid_webhook_token_uses_webhook_realm() {
        let err = ApiError::invalid_webhook_token("invalid webhook token for this route");
        let resp = err.into_response();
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(hdr.contains(r#"realm="webhook""#), "header: {hdr}");
        assert!(hdr.contains(r#"error="invalid_token""#), "header: {hdr}");
    }

    #[test]
    fn description_with_embedded_quotes_rendered_safely() {
        let err = ApiError::Unauthorized {
            realm: "api",
            error: Some(UnauthorizedReason::InvalidToken),
            description: Some(r#"Token "abc" is invalid"#.into()),
        };
        let resp = err.into_response();
        let hdr = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        // The raw `"` chars in the description must be escaped so the header
        // remains syntactically valid.
        assert!(
            hdr.contains(r#"error_description="Token \"abc\" is invalid""#),
            "header: {hdr}"
        );
    }

    #[test]
    fn non_401_errors_have_no_www_authenticate() {
        let err = ApiError::NotFound;
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    // ── ScopeRequired (sprint-14 S14-1) — nested envelope ────────────────

    #[tokio::test]
    async fn scope_required_renders_nested_envelope() {
        use axum::body::to_bytes;
        let err = ApiError::scope_required("hotl:decide");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "scope_required");
        assert_eq!(v["error"]["details"]["scope"], "hotl:decide");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("hotl:decide"),
            "message should include the scope: {v}"
        );
    }

    #[test]
    fn scope_required_constructor_carries_scope() {
        let err = ApiError::scope_required("hotl:policy:write");
        match err {
            ApiError::ScopeRequired { scope } => assert_eq!(scope, "hotl:policy:write"),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
