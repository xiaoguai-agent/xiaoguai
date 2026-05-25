//! Per-route Casbin enforcement.
//!
//! When `AppState.authz` is `Some(...)`, the router wires a middleware
//! that runs *after* `require_bearer` (so `Claims` are already in the
//! request extensions) and *before* the handler. It:
//!
//!   1. Pulls `Claims` from extensions; if missing it returns 401
//!      (defence-in-depth — should never happen behind `require_bearer`).
//!   2. Derives `(resource, action)` from `(request.uri.path(),
//!      request.method())`. The resource is the path with the `/v1`
//!      prefix stripped; the action is `read` / `write` / `delete` based
//!      on the HTTP method.
//!   3. For each role in `claims.roles`, calls
//!      `Authz::check(role, claims.tenant_id, resource, action)`. The
//!      request is allowed if *any* role passes; otherwise the response
//!      is 403.
//!
//! When `AppState.authz` is `None`, the router does not wire this layer
//! at all — the middleware is opt-in via boot-time config.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use xiaoguai_auth::Authz;

use crate::auth::Claims;

/// Map an HTTP method onto the action vocabulary used in the Casbin
/// policy file (`p, role, *, /sessions/*, read|write|delete`).
#[must_use]
pub fn method_to_action(method: &Method) -> &'static str {
    match *method {
        Method::GET | Method::HEAD | Method::OPTIONS => "read",
        Method::DELETE => "delete",
        _ => "write",
    }
}

/// Convert an axum request path into the resource string the policy
/// matches against. Strips the API version prefix so the policy file
/// can use stable `/sessions/*` patterns regardless of versioning.
///
/// Collection endpoints (`/v1/sessions` with no trailing segment) gain a
/// trailing slash so that the `keyMatch` against `/sessions/*` succeeds.
#[must_use]
pub fn path_to_resource(path: &str) -> String {
    let stripped = path.strip_prefix("/v1").unwrap_or(path);
    // Normalize empty (root) → "/", but keep deeper paths as-is.
    if stripped.is_empty() {
        return "/".to_string();
    }
    // For collection endpoints (single segment, no slash after), append a
    // trailing slash so `keyMatch("/sessions/", "/sessions/*")` returns
    // true. Casbin's keyMatch does not treat `*` as "zero or more
    // characters including a slash boundary", so without this the bare
    // `/sessions` POST would be denied.
    let depth = stripped.matches('/').count();
    if depth == 1 && !stripped.ends_with('/') {
        return format!("{stripped}/");
    }
    stripped.to_string()
}

/// Axum middleware that enforces the Casbin policy for a single
/// request. Mount via `from_fn_with_state` so the `Arc<Authz>` is
/// captured by clone.
///
/// # Errors
/// Returns `401 Unauthorized` if claims are missing or `403 Forbidden` if the policy denies access.
pub async fn require_authorized(
    authz: Arc<Authz>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(claims) = req.extensions().get::<Claims>().cloned() else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let action = method_to_action(req.method());
    let resource = path_to_resource(req.uri().path());

    if claims.roles.is_empty() {
        return Err(StatusCode::FORBIDDEN);
    }

    for role in &claims.roles {
        match authz
            .check(role, &claims.tenant_id, &resource, action)
            .await
        {
            Ok(true) => return Ok(next.run(req).await),
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(
                    ?err,
                    role = %role,
                    tenant = %claims.tenant_id,
                    resource = %resource,
                    action = %action,
                    "rbac enforcement errored"
                );
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    Err(StatusCode::FORBIDDEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_mapping_covers_common_verbs() {
        assert_eq!(method_to_action(&Method::GET), "read");
        assert_eq!(method_to_action(&Method::HEAD), "read");
        assert_eq!(method_to_action(&Method::OPTIONS), "read");
        assert_eq!(method_to_action(&Method::POST), "write");
        assert_eq!(method_to_action(&Method::PUT), "write");
        assert_eq!(method_to_action(&Method::PATCH), "write");
        assert_eq!(method_to_action(&Method::DELETE), "delete");
    }

    #[test]
    fn path_strips_v1_prefix() {
        assert_eq!(path_to_resource("/v1/sessions/abc"), "/sessions/abc");
        assert_eq!(path_to_resource("/v1/mcp/servers"), "/mcp/servers");
    }

    #[test]
    fn path_appends_slash_to_collection_endpoints() {
        // `/sessions` is a single segment → keyMatch against `/sessions/*`
        // would otherwise fail; the normaliser adds the slash.
        assert_eq!(path_to_resource("/v1/sessions"), "/sessions/");
    }

    #[test]
    fn nested_paths_unchanged() {
        assert_eq!(
            path_to_resource("/v1/sessions/abc/cancel"),
            "/sessions/abc/cancel"
        );
    }

    #[test]
    fn unprefixed_single_segment_also_gets_trailing_slash() {
        // /healthz is mounted outside the v1 layer so this middleware
        // never sees it in practice, but the normaliser is uniform: any
        // single-segment path gets a trailing slash for collection
        // matching. Documenting the behaviour here so we notice if we
        // ever change it.
        assert_eq!(path_to_resource("/healthz"), "/healthz/");
    }

    #[tokio::test]
    async fn denies_when_no_role_matches() {
        let authz = Authz::new_default().await.expect("authz");
        // Build a request with a Claims that only has a role with no
        // matching policy rule.
        let mut request = Request::builder()
            .uri("/v1/sessions/abc")
            .body(axum::body::Body::empty())
            .unwrap();
        request.extensions_mut().insert(Claims {
            sub: "u".into(),
            tenant_id: "t".into(),
            roles: vec!["nobody".into()],
        });
        let action = method_to_action(request.method());
        let resource = path_to_resource(request.uri().path());
        let allowed = authz.check("nobody", "t", &resource, action).await.unwrap();
        assert!(!allowed);
    }

    #[tokio::test]
    async fn allows_when_role_matches_policy() {
        let authz = Authz::new_default().await.expect("authz");
        let resource = path_to_resource("/v1/sessions/abc");
        let res = authz
            .check("tenant_admin", "ten_a", &resource, "read")
            .await
            .unwrap();
        assert!(res, "tenant_admin must be allowed to read /sessions/*");
    }

    #[tokio::test]
    async fn system_admin_allowed_on_anything() {
        let authz = Authz::new_default().await.expect("authz");
        let resource = path_to_resource("/v1/audit/2026");
        let res = authz
            .check("system_admin", "ten_z", &resource, "write")
            .await
            .unwrap();
        assert!(res);
    }
}
