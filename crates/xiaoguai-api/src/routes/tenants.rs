//! `/v1/tenants/*` — per-tenant client-facing configuration.
//!
//! v1.3.x added `GET /v1/tenants/:id/config`, fetched on mount by chat-ui's
//! `AiDisclosureBanner` component (EU AI Act Art. 50(1) transparency notice).
//! The endpoint returns the tenant's client config, currently scoped to the
//! `ai_disclosure_banner` block.
//!
//! There is no persisted per-tenant override store yet, so the banner config
//! is the platform default (banner on, dismissible, default copy). The handler
//! still verifies the tenant exists — a request for an unknown tenant is a
//! `404`, not a silent default — which keeps the contract honest and mirrors
//! the frontend's documented fallback (`getAiDisclosureConfig` defaults to
//! `enabled=true, dismissible=true` whenever the field is absent or the call
//! fails). When a real override store lands, only `load_banner_config` below
//! needs to change.

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Default banner copy returned when a tenant has no custom override.
///
/// Kept in sync with the chat-ui Pact contract example
/// (`tests/pact/wave3/consumers/chat-ui/xiaoguai-wave3.pact.test.ts`). The
/// frontend prefers its own i18n string when `text_override` is null, so this
/// value is the locale-agnostic fallback / contract anchor.
const DEFAULT_DISCLOSURE_TEXT: &str =
    "This assistant is powered by AI. Responses may not be accurate.";

/// AI disclosure banner config for a single tenant.
///
/// Field set is a superset of what each consumer needs:
///   - chat-ui (`AiDisclosureConfig`): `enabled`, `dismissible`,
///     `text_override`, `link_to_disclosure`.
///   - Pact contract anchor: `enabled` (bool) + `text` (string).
///
/// `text` is always a concrete string (the resolved copy) so the contract's
/// type matcher is satisfied; `text_override` is the operator's optional
/// replacement (null = use the platform/i18n default).
#[derive(Debug, Clone, Serialize)]
pub struct AiDisclosureBannerConfig {
    /// When false the banner is hidden entirely.
    pub enabled: bool,
    /// When false the dismiss button is hidden (regulated tenants).
    pub dismissible: bool,
    /// Resolved banner copy (override when set, otherwise the default).
    pub text: String,
    /// Operator's custom copy; null means use the platform/i18n default.
    pub text_override: Option<String>,
    /// Optional "Learn more" transparency-page URL.
    pub link_to_disclosure: Option<String>,
}

impl AiDisclosureBannerConfig {
    /// Platform default: banner on, dismissible, default copy, no overrides.
    fn platform_default() -> Self {
        Self {
            enabled: true,
            dismissible: true,
            text: DEFAULT_DISCLOSURE_TEXT.to_string(),
            text_override: None,
            link_to_disclosure: None,
        }
    }
}

/// Response body for `GET /v1/tenants/:id/config`.
#[derive(Debug, Clone, Serialize)]
pub struct TenantConfigResponse {
    pub tenant_id: String,
    pub ai_disclosure_banner: AiDisclosureBannerConfig,
}

/// Resolve the banner config for a tenant.
///
/// No persisted override store exists yet, so every tenant gets the platform
/// default. This is the single seam to extend when per-tenant overrides land.
fn load_banner_config(_tenant_id: &str) -> AiDisclosureBannerConfig {
    AiDisclosureBannerConfig::platform_default()
}

/// `GET /v1/tenants/:id/config` — per-tenant client configuration.
///
/// Returns `200` with `{ tenant_id, ai_disclosure_banner }` when the tenant
/// exists, `404` when it does not, and `500` when the tenant repository is not
/// wired into [`AppState`].
///
/// # Errors
/// Returns an error if the tenant repository is not wired, the tenant is
/// unknown, or the lookup query fails.
pub async fn get_tenant_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TenantConfigResponse>> {
    let repo = state.tenants.as_ref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!("tenant repository not wired into AppState"))
    })?;
    let tenant = repo.find_by_id(&id).await?.ok_or(ApiError::NotFound)?;
    let tenant_id = tenant.id.to_string();
    Ok(Json(TenantConfigResponse {
        ai_disclosure_banner: load_banner_config(&tenant_id),
        tenant_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_default_is_enabled_and_dismissible() {
        let c = AiDisclosureBannerConfig::platform_default();
        assert!(c.enabled);
        assert!(c.dismissible);
        assert_eq!(c.text, DEFAULT_DISCLOSURE_TEXT);
        assert!(c.text_override.is_none());
        assert!(c.link_to_disclosure.is_none());
    }

    #[test]
    fn response_serializes_with_contract_shape() {
        let resp = TenantConfigResponse {
            tenant_id: "tenant_acme".to_string(),
            ai_disclosure_banner: load_banner_config("tenant_acme"),
        };
        let v = serde_json::to_value(&resp).unwrap();
        // chat-ui + Pact contract minimum shape.
        assert_eq!(v["tenant_id"], "tenant_acme");
        assert_eq!(v["ai_disclosure_banner"]["enabled"], true);
        assert!(v["ai_disclosure_banner"]["text"].is_string());
        assert!(v["ai_disclosure_banner"]["dismissible"].is_boolean());
    }
}
