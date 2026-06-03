//! # `xiaoguai-observability`
//!
//! Prometheus metrics + OTLP trace export for the Xiaoguai stack.
//!
//! ## What this crate provides
//!
//! | Module | What it does |
//! |---|---|
//! | [`prometheus`] | Registry init, metric handles, `/metrics` route |
//! | [`otlp`] | OTLP/gRPC trace pipeline via `tracing-opentelemetry` |
//! | [`instrument`] | Macros: `instrument_llm_call!`, `instrument_scheduler_tick!` |
//!
//! ## Quick start (xiaoguai-core)
//!
//! Enable the feature flag in `xiaoguai-core`'s `Cargo.toml`:
//!
//! ```toml
//! xiaoguai-observability = { path = "../xiaoguai-observability", features = ["full"] }
//! ```
//!
//! Then call `mount` in `main.rs` after building the axum router:
//!
//! ```rust,ignore
//! #[cfg(feature = "observability")]
//! let router = xiaoguai_observability::mount(router)?;
//! ```
//!
//! `mount` calls `init_prometheus` + `init_otlp` internally and returns the
//! router with `/metrics` attached. The OTLP tracer provider is stored
//! globally; call `xiaoguai_observability::shutdown()` on graceful exit.

pub mod instrument;
pub mod otlp;
pub mod prometheus;
pub mod redact;
pub mod signal;

pub use otlp::{init_otlp, shutdown_tracer};
pub use prometheus::{
    anomaly_detections_total, compaction_fallback_total, compaction_token_savings,
    compaction_triggered_total, global_handles, hotl_check_duration, hotl_registry_replayed_total,
    hotl_suspended_loops_gauge, hotl_suspension_duration_seconds, hotl_suspensions_total,
    hotl_usage_total, im_messages_total, init_prometheus, mount_metrics, outcomes_chain_depth,
    outcomes_recorded_total, watch_wakeups_total, MetricHandles,
};
pub use redact::RedactingSpanExporter;
pub use signal::Signal;

use anyhow::Result;
use axum::Router;
use once_cell::sync::OnceCell;
use opentelemetry_sdk::trace::SdkTracerProvider;

static TRACER_PROVIDER: OnceCell<SdkTracerProvider> = OnceCell::new();

/// One-call integration point for `xiaoguai-core`.
///
/// 1. Calls [`init_prometheus`] — registers metric handles globally.
/// 2. Calls [`init_otlp`] — installs the tracing subscriber with the
///    OTLP layer (honours `RUST_LOG` and `OTEL_EXPORTER_OTLP_ENDPOINT`).
/// 3. Mounts `GET /metrics` on the provided router.
///
/// Returns the augmented router.
///
/// # Errors
///
/// Returns an error if Prometheus registration fails, the OTLP pipeline
/// cannot connect, or the global subscriber is already set.
pub fn mount(router: Router) -> Result<Router> {
    let (registry, _handles) = init_prometheus()?;
    let provider = init_otlp()?;
    // Store so shutdown() can reach it.
    let _ = TRACER_PROVIDER.set(provider);
    let router = mount_metrics(router, registry);
    Ok(router)
}

/// Flush and shut down the OTLP tracer provider.
///
/// Safe to call even when `mount` was never called (no-op in that case).
pub fn shutdown() {
    // opentelemetry 0.30+ removed global::shutdown_tracer_provider(); flush
    // via the stored provider instead (shutdown takes &self, no move needed).
    if let Some(provider) = TRACER_PROVIDER.get() {
        otlp::shutdown_tracer(provider);
    }
}
