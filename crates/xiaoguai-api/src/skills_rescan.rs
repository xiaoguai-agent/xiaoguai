//! Phase 5 (skill-pack loader): hot-activate an installed conversational
//! pack's agent team **without rebooting `serve`**.
//!
//! Phase 4b ([`xiaoguai_core::pack_runtime::scan_enabled_pack_agents`]) runs
//! only at boot, so a freshly-installed pack needs a process restart to bring
//! its team live. This module adds `POST /v1/admin/skills/rescan`: an
//! owner-authed endpoint that re-runs that same idempotent scan against the
//! **live** `AppState` repositories, so the team goes live immediately.
//!
//! ## Layering
//!
//! The scan itself lives in `xiaoguai-core` (behind its `packs` feature) and
//! needs a `&SqlitePool` plus the persona/team repos — none of which the
//! `xiaoguai-api` crate may reach directly (core depends on api, not the
//! reverse, and the `packs`/`sqlx` deps live in core). So the work is injected
//! the same way every other backend capability is: a small [`PackRescanner`]
//! trait declared here, an `Option<Arc<dyn PackRescanner>>` field on
//! [`AppState`], and a concrete bridge wired by `xiaoguai_core::run_serve`
//! that closes over the pool + repos and calls the core scan. The handler
//! stays sqlx-free and core-free.
//!
//! ## Scope
//!
//! Only **conversational** agent teams are hot-activated (mirroring the boot
//! scan). Anomaly/watch specs are NOT re-scanned here — their detectors live in
//! the scheduler + anomaly/watch registries owned by `run_serve`, not on
//! `AppState`; boot-scan still covers them. See the Phase 4 design
//! (`docs/plans/2026-06-25-skill-pack-loader-phase4.md`).

use async_trait::async_trait;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use thiserror::Error;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Failure modes of a pack rescan. Mirrors the
/// [`crate::skills::SkillPackError`] shape — a backend (DB / pack-load)
/// failure is the only fallible path; the scan itself swallows per-pack
/// errors so one bad pack never fails the whole request.
#[derive(Debug, Clone, Error)]
pub enum PackRescanError {
    /// The underlying store or pack load failed.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Hot-rescan boundary. Production wires a bridge in `xiaoguai-core` that
/// closes over the embedded `SQLite` pool + the live persona/team repos and
/// calls `xiaoguai_core::pack_runtime::scan_enabled_pack_agents`. `None` on
/// [`AppState`] (the default, and every non-pack build) makes the route return
/// 503 — there is no team substrate to activate.
#[async_trait]
pub trait PackRescanner: Send + Sync {
    /// Re-scan every enabled installed pack and activate its conversational
    /// agent team against the live repositories, returning the slugs whose
    /// team is now (re)confirmed active. Idempotent: re-running once a pack is
    /// active is a no-op, so it is safe to call on every install.
    ///
    /// # Errors
    /// Returns [`PackRescanError::Backend`] when the underlying store or pack
    /// load fails. Per-pack failures are swallowed (logged) by the scan so one
    /// bad pack never fails the whole request.
    async fn rescan(&self) -> Result<Vec<String>, PackRescanError>;
}

/// Wire shape for `POST /v1/admin/skills/rescan`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RescanResponse {
    /// Slugs whose conversational agent team is now active.
    pub activated: Vec<String>,
}

/// `POST /v1/admin/skills/rescan`
///
/// Owner-authed (the whole `/v1/**` surface is behind HTTP Basic when
/// `AppState.auth` is set — DEC-033 single owner). Re-runs the Phase 4b
/// conversational-team activation against the live repos so a just-installed
/// pack's team becomes runnable through `/orchestrate` immediately.
///
/// * 200 `{ "activated": ["slug", …] }` — the (possibly empty) list of packs
///   whose team is now active.
/// * 503 when no rescanner is wired (a non-`packs` build, or a deployment
///   without the persona/team repos) — there is no team substrate to activate.
///   This matches every other unwired-capability route under `/v1/admin/*`.
///
/// # Errors
/// Returns 503 when unwired, or 500 when the underlying store/pack-load fails.
pub async fn rescan_skills(State(state): State<AppState>) -> ApiResult<Json<RescanResponse>> {
    let rescanner = state.pack_rescanner.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "skill-pack agent rescan not wired (build without `packs`, or no persona/team store)"
                .into(),
        )
    })?;
    let activated = rescanner
        .rescan()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e.to_string())))?;
    Ok(Json(RescanResponse { activated }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub rescanner so the trait + response shape can be exercised without
    /// pulling in core/sqlx. The route-level wiring (503 unwired / 200 wired)
    /// is covered by the integration test `tests/skills_rescan.rs`.
    struct StubRescanner(Result<Vec<String>, PackRescanError>);

    #[async_trait]
    impl PackRescanner for StubRescanner {
        async fn rescan(&self) -> Result<Vec<String>, PackRescanError> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn rescan_returns_activated_slugs() {
        let stub = StubRescanner(Ok(vec!["app-store-reviews".into()]));
        let out = stub.rescan().await.unwrap();
        assert_eq!(out, vec!["app-store-reviews".to_string()]);
    }

    #[tokio::test]
    async fn rescan_propagates_backend_error() {
        let stub = StubRescanner(Err(PackRescanError::Backend("db down".into())));
        let err = stub.rescan().await.unwrap_err();
        assert!(matches!(err, PackRescanError::Backend(_)));
    }

    #[test]
    fn response_serializes_activated_array() {
        let r = RescanResponse {
            activated: vec!["a".into(), "b".into()],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v, serde_json::json!({ "activated": ["a", "b"] }));
    }
}
