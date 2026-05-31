//! Request authentication layer.
//!
//! v0.6 introduces a `TokenValidator` trait so the API layer can swap
//! between the real `xiaoguai-auth` `JwtValidator` (production, fetches
//! JWKS) and a deterministic stub (tests + dev). The trait keeps
//! `xiaoguai-api` decoupled from `jsonwebtoken` so test builds stay light.
//!
//! When `AppState::auth` is `None`, the middleware behaves as a no-op —
//! handlers get `Option<Claims>` and may fall back to body-supplied
//! identity. When it's `Some(...)`, every `/v1/**` request must carry a
//! valid Bearer token; healthz and `/v1/openapi.json` remain public.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Claims surfaced to handlers. Mirrors the subset of `xiaoguai-auth`
/// claims that the API layer actually uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub tenant_id: String,
    pub roles: Vec<String>,
    /// OAuth 2.0-style scope strings (sprint-13 S13-10). Empty for
    /// legacy tokens issued before sprint-13. Scope-gated handlers
    /// (e.g. `POST /v1/hotl/decisions`) check membership and return
    /// 403 on miss; non-scope-gated routes ignore this field.
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    Missing,
    #[error("invalid bearer token: {0}")]
    Invalid(String),
}

#[async_trait]
pub trait TokenValidator: Send + Sync {
    async fn validate(&self, token: &str) -> Result<Claims, AuthError>;
}

/// Bridge `xiaoguai-auth::JwtValidator` into our trait without leaking the
/// dependency further. Production callers wrap their `JwtValidator` once
/// at boot time.
pub struct JwtTokenValidator<V>(pub Arc<V>);

#[async_trait]
impl TokenValidator for JwtTokenValidator<xiaoguai_auth::JwtValidator> {
    async fn validate(&self, token: &str) -> Result<Claims, AuthError> {
        match self.0.validate(token).await {
            Ok(c) => Ok(Claims {
                sub: c.sub,
                tenant_id: c.tenant_id,
                roles: c.roles,
                scopes: c.scopes,
            }),
            Err(e) => Err(AuthError::Invalid(e.to_string())),
        }
    }
}

/// Deterministic stub for tests + dev. Returns the configured claims for
/// any non-empty token; rejects empty tokens as `Missing`.
pub struct StubValidator {
    pub claims: Claims,
}

#[async_trait]
impl TokenValidator for StubValidator {
    async fn validate(&self, token: &str) -> Result<Claims, AuthError> {
        if token.is_empty() {
            return Err(AuthError::Missing);
        }
        Ok(self.claims.clone())
    }
}

/// Axum middleware that authenticates `/v1/**` routes when an
/// `Arc<dyn TokenValidator>` is present in app state. Public routes
/// (healthz, openapi) should be mounted outside this layer.
///
/// # Errors
/// Returns `401 Unauthorized` if the bearer token is missing or invalid.
pub async fn require_bearer(
    validator: Arc<dyn TokenValidator>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header_val = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let token = header_val.strip_prefix("Bearer ").unwrap_or("");
    let claims = validator
        .validate(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_rejects_empty_token() {
        let v = StubValidator {
            claims: Claims {
                sub: "u".into(),
                tenant_id: "t".into(),
                roles: vec![],
                scopes: vec![],
            },
        };
        assert!(matches!(v.validate("").await, Err(AuthError::Missing)));
    }

    #[tokio::test]
    async fn stub_accepts_non_empty() {
        let v = StubValidator {
            claims: Claims {
                sub: "alice".into(),
                tenant_id: "ten_a".into(),
                roles: vec!["admin".into()],
                scopes: vec![],
            },
        };
        let c = v.validate("anything").await.expect("ok");
        assert_eq!(c.sub, "alice");
        assert_eq!(c.tenant_id, "ten_a");
    }
}
