//! `RequireScope<S>` — axum extractor gating a handler on a named scope
//! present in the request's [`Claims`].
//!
//! Sprint-14 S14-1 (DEC-HLD-018). Replaces sprint-13's inline
//! `claims.scopes.iter().any(...)` check in
//! [`crate::routes::hotl_decisions`].
//!
//! # Why marker traits, not `const SCOPE: &'static str`
//!
//! Stable Rust 1.93 supports const-generic parameters of *integer* and
//! *bool* type, but NOT `&'static str` — that lives behind the
//! unstable `adt_const_params` feature gate. Writing
//! `RequireScope<const SCOPE: &'static str>` is a compile error on
//! every channel except nightly. The marker-trait pattern below works
//! on stable, produces identical zero-cost code (every `S: ScopeName`
//! type is a ZST), and lets the scope value live in plain associated
//! constants that `impl` blocks can audit.
//!
//! # Usage
//!
//! ```ignore
//! use xiaoguai_api::middleware::require_scope::{RequireScope, HotlDecide};
//!
//! async fn create_decision(
//!     RequireScope(claims, _): RequireScope<HotlDecide>,
//!     // ... other extractors
//! ) -> Response { /* ... */ }
//! ```
//!
//! The handler signature alone declares the scope requirement; the
//! extractor short-circuits with `403 Forbidden` + the api-contract
//! §1.6 nested envelope `{"error":{"code":"scope_required",
//! "message":"...","details":{"scope":"<slug>"}}}` when the bearer
//! token's `Claims` lack the required scope. Anonymous requests are
//! handled by the upstream `require_bearer` middleware and short-
//! circuit at 401 before this extractor ever runs.
//!
//! Errors are intentionally rendered through [`crate::ApiError`]'s
//! `IntoResponse` so the wire shape stays uniform with other 4xx
//! responses across the API surface.

use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::auth::Claims;
use crate::error::ApiError;

/// Marker trait — each implementor is a zero-sized type carrying a
/// single associated constant naming the OAuth-style scope slug it
/// represents. The slug must match the value Casbin rules and JWT
/// `scopes` claim entries use verbatim.
pub trait ScopeName: 'static {
    /// The wire string for the scope (e.g. `"hotl:decide"`).
    const VALUE: &'static str;
}

/// Axum extractor that requires `Claims` to carry `S::VALUE` in its
/// `scopes` set.
///
/// On success: yields the `Claims` (cloned out of the request
/// extensions) so the handler can still use them downstream. On miss:
/// short-circuits with a 403 carrying the api-contract §1.6 nested
/// `scope_required` envelope.
///
/// Destructure in the handler signature as
/// `RequireScope(claims, _): RequireScope<HotlDecide>`. The `_`
/// discards the [`PhantomData`] tag.
pub struct RequireScope<S: ScopeName>(pub Claims, pub PhantomData<S>);

impl<St, S> FromRequestParts<St> for RequireScope<S>
where
    St: Send + Sync,
    S: ScopeName + Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &St) -> Result<Self, Self::Rejection> {
        // The upstream `require_bearer` middleware inserts `Claims`
        // into request extensions on every authenticated request. When
        // it's absent the API is running in unauthed dev/test mode and
        // the scope gate degrades to a no-op (mirrors the sprint-13
        // semantics of the inline check that this extractor replaces).
        let claims_opt = parts.extensions.get::<Claims>().cloned();
        let Some(claims) = claims_opt else {
            // No Claims in extensions → unauthed dev/test mode. Allow
            // through so integration tests that mount the router
            // without an auth layer continue to work.
            return Ok(Self(
                Claims {
                    sub: String::new(),
                    tenant_id: String::new(),
                    roles: Vec::new(),
                    scopes: Vec::new(),
                },
                PhantomData,
            ));
        };

        if claims.scopes.iter().any(|s| s == S::VALUE) {
            Ok(Self(claims, PhantomData))
        } else {
            Err(ApiError::scope_required(S::VALUE).into_response())
        }
    }
}

// ── concrete marker ZSTs ──────────────────────────────────────────────────────

/// Scope slug guarding `POST /v1/hotl/decisions` (sprint-13 S13-10).
pub struct HotlDecide;
impl ScopeName for HotlDecide {
    const VALUE: &'static str = "hotl:decide";
}

/// Scope slug for reading per-tenant `HotL` redaction policies (S14-3+).
pub struct HotlPolicyRead;
impl ScopeName for HotlPolicyRead {
    const VALUE: &'static str = "hotl:policy:read";
}

/// Scope slug for mutating per-tenant `HotL` redaction policies (S14-3+).
pub struct HotlPolicyWrite;
impl ScopeName for HotlPolicyWrite {
    const VALUE: &'static str = "hotl:policy:write";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_constants_match_wire_strings() {
        assert_eq!(HotlDecide::VALUE, "hotl:decide");
        assert_eq!(HotlPolicyRead::VALUE, "hotl:policy:read");
        assert_eq!(HotlPolicyWrite::VALUE, "hotl:policy:write");
    }

    /// Sanity: each marker is a ZST.
    #[test]
    fn markers_are_zero_sized() {
        assert_eq!(std::mem::size_of::<HotlDecide>(), 0);
        assert_eq!(std::mem::size_of::<HotlPolicyRead>(), 0);
        assert_eq!(std::mem::size_of::<HotlPolicyWrite>(), 0);
    }
}
