//! Request authentication layer.
//!
//! Under the single-user `SQLite` pivot (DEC-033) authentication collapses to
//! a single static **owner** identity. There is no OIDC, no Casbin, no
//! roles, no scopes, no tenants — every authenticated request is the owner.
//!
//! The optional access gate is a single configured **username + password**
//! checked via HTTP Basic auth. When `AppState::auth` is `None` (no
//! credential configured) the middleware is not mounted and handlers fall
//! back to a body-supplied / owner identity — convenient for a localhost
//! dev run. When it is `Some(...)`, every `/v1/**` request must carry a
//! matching `Authorization: Basic base64(user:pass)` header; `/healthz` and
//! `/v1/openapi.json` stay public.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Realm advertised on the `WWW-Authenticate` challenge so browsers render
/// their native Basic-auth prompt.
const BASIC_REALM: &str = "xiaoguai";

/// Identity surfaced to handlers. A single static owner under DEC-033.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Owner subject — the configured username (or a synthetic owner id in
    /// dev mode where no credential is set).
    pub sub: String,
}

impl Claims {
    /// Build the owner identity for `sub`.
    #[must_use]
    pub fn owner(sub: impl Into<String>) -> Self {
        Self { sub: sub.into() }
    }
}

#[derive(Debug, Clone, Error)]
pub enum AuthError {
    #[error("missing credentials")]
    Missing,
    #[error("invalid credentials: {0}")]
    Invalid(String),
}

#[async_trait]
pub trait TokenValidator: Send + Sync {
    /// Validate the credential carried by the `Authorization` header — the
    /// portion after the scheme word. Returns the owner [`Claims`] on
    /// success.
    async fn validate(&self, credential: &str) -> Result<Claims, AuthError>;
}

/// Production validator: a single static username/password checked via HTTP
/// Basic. The `credential` is the base64 blob after `Basic `.
pub struct StaticCredentialValidator {
    username: String,
    password: String,
}

impl StaticCredentialValidator {
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}

#[async_trait]
impl TokenValidator for StaticCredentialValidator {
    async fn validate(&self, credential: &str) -> Result<Claims, AuthError> {
        if credential.is_empty() {
            return Err(AuthError::Missing);
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(credential.trim())
            .map_err(|_| AuthError::Invalid("malformed Basic credential".into()))?;
        let text = String::from_utf8(decoded)
            .map_err(|_| AuthError::Invalid("non-utf8 credential".into()))?;
        let (user, pass) = text
            .split_once(':')
            .ok_or_else(|| AuthError::Invalid("credential missing ':'".into()))?;
        // Compare both fields without short-circuiting so request timing
        // does not leak which field mismatched.
        let ok = ct_eq(user.as_bytes(), self.username.as_bytes())
            & ct_eq(pass.as_bytes(), self.password.as_bytes());
        if ok {
            Ok(Claims::owner(&self.username))
        } else {
            Err(AuthError::Invalid("username or password mismatch".into()))
        }
    }
}

/// Length-independent byte comparison to avoid a trivial timing oracle on
/// the configured password.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Deterministic stub for tests + dev. Returns the configured claims for any
/// non-empty credential; rejects an empty credential as `Missing`.
pub struct StubValidator {
    pub claims: Claims,
}

#[async_trait]
impl TokenValidator for StubValidator {
    async fn validate(&self, credential: &str) -> Result<Claims, AuthError> {
        if credential.is_empty() {
            return Err(AuthError::Missing);
        }
        Ok(self.claims.clone())
    }
}

/// Axum middleware that authenticates `/v1/**` routes when an
/// `Arc<dyn TokenValidator>` is present in app state. Public routes
/// (healthz, openapi) are mounted outside this layer.
///
/// On failure it returns `401 Unauthorized` with a
/// `WWW-Authenticate: Basic realm="xiaoguai"` challenge so browsers prompt
/// for the owner's username + password.
pub async fn require_auth(
    validator: Arc<dyn TokenValidator>,
    mut req: Request,
    next: Next,
) -> Response {
    let header_val = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    // Strip the scheme word ("Basic"/"Bearer") and hand the remainder to the
    // validator. The static validator treats it as base64(user:pass); the
    // test stub treats any non-empty string as valid.
    let credential = header_val.split_once(' ').map_or(header_val, |(_, c)| c);
    match validator.validate(credential).await {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(_) => unauthorized_response(),
    }
}

fn unauthorized_response() -> Response {
    let mut resp = StatusCode::UNAUTHORIZED.into_response();
    let challenge = format!("Basic realm=\"{BASIC_REALM}\"");
    if let Ok(v) = HeaderValue::from_str(&challenge) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_rejects_empty_credential() {
        let v = StubValidator {
            claims: Claims::owner("u"),
        };
        assert!(matches!(v.validate("").await, Err(AuthError::Missing)));
    }

    #[tokio::test]
    async fn stub_accepts_non_empty() {
        let v = StubValidator {
            claims: Claims::owner("alice"),
        };
        let c = v.validate("anything").await.expect("ok");
        assert_eq!(c.sub, "alice");
    }

    #[tokio::test]
    async fn static_validator_accepts_matching_basic_credential() {
        let v = StaticCredentialValidator::new("owner", "s3cret");
        let cred = base64::engine::general_purpose::STANDARD.encode("owner:s3cret");
        let c = v.validate(&cred).await.expect("ok");
        assert_eq!(c.sub, "owner");
    }

    #[tokio::test]
    async fn static_validator_rejects_wrong_password() {
        let v = StaticCredentialValidator::new("owner", "s3cret");
        let cred = base64::engine::general_purpose::STANDARD.encode("owner:nope");
        assert!(matches!(
            v.validate(&cred).await,
            Err(AuthError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn static_validator_rejects_empty() {
        let v = StaticCredentialValidator::new("owner", "s3cret");
        assert!(matches!(v.validate("").await, Err(AuthError::Missing)));
    }
}
