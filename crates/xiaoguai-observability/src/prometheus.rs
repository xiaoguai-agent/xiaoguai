//! Prometheus metrics registry + HTTP exposition for Xiaoguai.
//!
//! # Exported metrics
//!
//! | Name | Type | Labels | Description |
//! |---|---|---|---|
//! | `xiaoguai_http_request_duration_seconds` | Histogram | `method`, `path`, `status` | HTTP request latency |
//! | `xiaoguai_llm_call_duration_seconds` | Histogram | `provider`, `model` | LLM call latency |
//! | `xiaoguai_scheduler_tick_duration_seconds` | Histogram | — | Scheduler tick latency |
//! | `xiaoguai_hotl_usage_total` | Counter | `tenant`, `scope`, `verdict` | HOTL enforcer decisions |
//! | `xiaoguai_hotl_check_duration_seconds` | Histogram | — | HOTL enforcer check latency |
//! | `xiaoguai_outcomes_recorded_total` | Counter | `tenant`, `kind` | Outcome attributions recorded |
//! | `xiaoguai_outcomes_chain_depth` | Histogram | — | Chain depth per recorded outcome |
//! | `xiaoguai_rate_limit_hits_total` | Counter | `tenant`, `route`, `decision` | Rate-limit decisions |
//! | `xiaoguai_anomaly_detections_total` | Counter | `detector`, `severity` | Anomaly detector fires |
//! | `xiaoguai_watch_wakeups_total` | Counter | `watcher_id`, `outcome` | Watch task wakeup results |
//! | `xiaoguai_im_messages_total` | Counter | `adapter`, `direction` | IM gateway messages |
//!
//! On Linux, default process collectors (CPU, memory, file descriptors) are
//! also registered automatically.
//!
//! # Usage
//!
//! ```rust,ignore
//! let (registry, handles) = xiaoguai_observability::init_prometheus()?;
//! // mount on existing axum router:
//! let router = xiaoguai_observability::mount_metrics(router, registry);
//! ```

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use once_cell::sync::OnceCell;
use prometheus::{
    exponential_buckets, register_histogram_vec_with_registry, register_histogram_with_registry,
    register_int_counter_vec_with_registry, Histogram, HistogramVec, IntCounterVec, Registry,
};

/// Bucket boundaries shared by HTTP and LLM histograms (seconds).
/// Covers 1 ms → ~65 s in exponential steps.
const LATENCY_BUCKETS_START: f64 = 0.001;
const LATENCY_BUCKETS_FACTOR: f64 = 2.0;
const LATENCY_BUCKETS_COUNT: usize = 16;

/// Global handle to the lazily-initialised metric handles.
static HANDLES: OnceCell<MetricHandles> = OnceCell::new();

/// All metric handles in one struct so they can be cloned cheaply.
#[derive(Clone)]
pub struct MetricHandles {
    /// HTTP request latency histogram, labelled by `(method, path, status)`.
    pub http_request_duration: HistogramVec,
    /// LLM provider call latency histogram, labelled by `(provider, model)`.
    pub llm_call_duration: HistogramVec,
    /// Scheduler tick latency histogram (unlabelled).
    pub scheduler_tick_duration: Histogram,

    // ── Wave-3 metrics ──────────────────────────────────────────────────────
    /// HOTL enforcer decisions: `(tenant, scope, verdict)`.
    pub hotl_usage_total: IntCounterVec,
    /// HOTL enforcer check latency (unlabelled).
    pub hotl_check_duration: Histogram,
    /// Outcome attributions recorded: `(tenant, kind)`.
    pub outcomes_recorded_total: IntCounterVec,
    /// Chain depth per recorded outcome.
    pub outcomes_chain_depth: Histogram,
    /// Rate-limit decisions: `(tenant, route, decision)`.
    pub rate_limit_hits_total: IntCounterVec,
    /// Anomaly detector fires: `(detector, severity)`.
    pub anomaly_detections_total: IntCounterVec,
    /// Watch task wakeup results: `(watcher_id, outcome)`.
    pub watch_wakeups_total: IntCounterVec,
    /// IM gateway messages: `(adapter, direction)`.
    pub im_messages_total: IntCounterVec,

    // ── v0.5.4.1 history-compaction metrics ─────────────────────────────────
    /// Agent history compaction triggers: `(reason)`.
    pub compaction_triggered_total: IntCounterVec,
    /// Agent history compaction fallbacks (slide instead of summary): `(reason)`.
    pub compaction_fallback_total: IntCounterVec,
    /// Tokens saved per compaction event (before - after).
    pub compaction_token_savings: Histogram,
}

/// Initialise the Prometheus registry.
///
/// Registers default process collectors (Linux only) and the
/// Xiaoguai-specific histograms into a fresh [`Registry`].
///
/// The metric handles are also stored globally via [`global_handles`]
/// so macros can find them without explicit threading. Calling this
/// function a second time is harmless — the first set of handles wins.
///
/// # Errors
///
/// Returns an error if registration fails (duplicate metric name, etc.)
/// or if exponential bucket generation overflows.
pub fn init_prometheus() -> Result<(Registry, MetricHandles)> {
    let registry = Registry::new_custom(Some("xiaoguai".into()), None)
        .context("create prometheus registry")?;

    // Process collector: only available on Linux (prometheus crate gates
    // the module on `target_os = "linux"` regardless of Cargo features).
    #[cfg(target_os = "linux")]
    {
        use prometheus::process_collector::ProcessCollector;
        let pc = ProcessCollector::for_self();
        registry
            .register(Box::new(pc))
            .context("register process collector")?;
    }

    let latency_buckets = exponential_buckets(
        LATENCY_BUCKETS_START,
        LATENCY_BUCKETS_FACTOR,
        LATENCY_BUCKETS_COUNT,
    )
    .context("build latency buckets")?;

    let http_request_duration = register_histogram_vec_with_registry!(
        "http_request_duration_seconds",
        "HTTP request latency in seconds",
        &["method", "path", "status"],
        latency_buckets.clone(),
        registry
    )
    .context("register http_request_duration_seconds")?;

    let llm_call_duration = register_histogram_vec_with_registry!(
        "llm_call_duration_seconds",
        "LLM provider call latency in seconds",
        &["provider", "model"],
        latency_buckets.clone(),
        registry
    )
    .context("register llm_call_duration_seconds")?;

    let scheduler_tick_duration = register_histogram_with_registry!(
        "scheduler_tick_duration_seconds",
        "Scheduler tick processing latency in seconds",
        latency_buckets.clone(),
        registry
    )
    .context("register scheduler_tick_duration_seconds")?;

    // ── Wave-3 counters + histograms ────────────────────────────────────────

    let hotl_usage_total = register_int_counter_vec_with_registry!(
        "hotl_usage_total",
        "HOTL enforcer decisions, labelled by tenant, scope, and verdict",
        &["tenant", "scope", "verdict"],
        registry
    )
    .context("register hotl_usage_total")?;

    let hotl_check_duration = register_histogram_with_registry!(
        "hotl_check_duration_seconds",
        "HOTL enforcer check latency in seconds",
        latency_buckets.clone(),
        registry
    )
    .context("register hotl_check_duration_seconds")?;

    let outcomes_recorded_total = register_int_counter_vec_with_registry!(
        "outcomes_recorded_total",
        "Outcome attributions recorded, labelled by tenant and kind",
        &["tenant", "kind"],
        registry
    )
    .context("register outcomes_recorded_total")?;

    let outcomes_chain_depth = register_histogram_with_registry!(
        "outcomes_chain_depth",
        "Chain depth per recorded outcome (number of agent turns)",
        // Fine-grained small-integer buckets: 1..=32 plus overflow.
        vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0],
        registry
    )
    .context("register outcomes_chain_depth")?;

    let rate_limit_hits_total = register_int_counter_vec_with_registry!(
        "rate_limit_hits_total",
        "Rate-limit decisions, labelled by tenant, route class, and decision",
        &["tenant", "route", "decision"],
        registry
    )
    .context("register rate_limit_hits_total")?;

    let anomaly_detections_total = register_int_counter_vec_with_registry!(
        "anomaly_detections_total",
        "Anomaly detector fires, labelled by detector type and severity",
        &["detector", "severity"],
        registry
    )
    .context("register anomaly_detections_total")?;

    let watch_wakeups_total = register_int_counter_vec_with_registry!(
        "watch_wakeups_total",
        "Watch task wakeup results, labelled by watcher_id and outcome",
        &["watcher_id", "outcome"],
        registry
    )
    .context("register watch_wakeups_total")?;

    let im_messages_total = register_int_counter_vec_with_registry!(
        "im_messages_total",
        "IM gateway messages processed, labelled by adapter and direction",
        &["adapter", "direction"],
        registry
    )
    .context("register im_messages_total")?;

    // v0.5.4.1 compaction metrics.
    let compaction_triggered_total = register_int_counter_vec_with_registry!(
        "compaction_triggered_total",
        "Agent history compaction triggers, labelled by reason (threshold|manual)",
        &["reason"],
        registry
    )
    .context("register compaction_triggered_total")?;

    let compaction_fallback_total = register_int_counter_vec_with_registry!(
        "compaction_fallback_total",
        "Agent compaction fell back to slide (summary unavailable), labelled by reason",
        &["reason"],
        registry
    )
    .context("register compaction_fallback_total")?;

    let compaction_token_savings = register_histogram_with_registry!(
        "compaction_token_savings",
        "Tokens saved per compaction event (before_estimate - after_estimate)",
        // Coarse buckets — compaction events are infrequent.
        vec![100.0, 500.0, 1_000.0, 5_000.0, 10_000.0, 30_000.0, 60_000.0],
        registry
    )
    .context("register compaction_token_savings")?;

    let handles = MetricHandles {
        http_request_duration,
        llm_call_duration,
        scheduler_tick_duration,
        hotl_usage_total,
        hotl_check_duration,
        outcomes_recorded_total,
        outcomes_chain_depth,
        rate_limit_hits_total,
        anomaly_detections_total,
        watch_wakeups_total,
        im_messages_total,
        compaction_triggered_total,
        compaction_fallback_total,
        compaction_token_savings,
    };

    // Store globally so macros can look them up without threading the
    // handles through every call site. Silently ignore duplicate-init
    // in test binaries where multiple test cases call init.
    let _ = HANDLES.set(handles.clone());

    Ok((registry, handles))
}

/// Return a reference to the global [`MetricHandles`] if
/// `init_prometheus` was called.
pub fn global_handles() -> Option<&'static MetricHandles> {
    HANDLES.get()
}

/// Axum handler that renders the registry in Prometheus text format.
async fn metrics_handler(State(registry): State<Registry>) -> Response {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    let mut buf = Vec::with_capacity(4096);
    match encoder.encode(&registry.gather(), &mut buf) {
        Ok(()) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            buf,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("prometheus encode error: {e}"),
        )
            .into_response(),
    }
}

/// Mount `GET /metrics` on the provided router, backed by `registry`.
///
/// This is the only integration call `xiaoguai-core` needs — no axum
/// internals required.
///
/// ```rust,ignore
/// let (registry, _handles) = xiaoguai_observability::init_prometheus()?;
/// let router = xiaoguai_observability::mount_metrics(router, registry);
/// ```
pub fn mount_metrics(router: Router, registry: Registry) -> Router {
    router.route("/metrics", get(metrics_handler).with_state(registry))
}

// ── Public accessor helpers (wave-3 emission sites) ──────────────────────────
//
// Each function returns `None` when `init_prometheus` has not yet been called
// (e.g. in unit tests that bypass the full initialisation).  Callers silently
// skip the increment rather than panicking.

/// Counter: `xiaoguai_hotl_usage_total{tenant, scope, verdict}`.
pub fn hotl_usage_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.hotl_usage_total)
}

/// Histogram: `xiaoguai_hotl_check_duration_seconds`.
pub fn hotl_check_duration() -> Option<&'static Histogram> {
    HANDLES.get().map(|h| &h.hotl_check_duration)
}

/// Counter: `xiaoguai_outcomes_recorded_total{tenant, kind}`.
pub fn outcomes_recorded_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.outcomes_recorded_total)
}

/// Histogram: `xiaoguai_outcomes_chain_depth`.
pub fn outcomes_chain_depth() -> Option<&'static Histogram> {
    HANDLES.get().map(|h| &h.outcomes_chain_depth)
}

/// Counter: `xiaoguai_rate_limit_hits_total{tenant, route, decision}`.
pub fn rate_limit_hits_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.rate_limit_hits_total)
}

/// Counter: `xiaoguai_anomaly_detections_total{detector, severity}`.
pub fn anomaly_detections_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.anomaly_detections_total)
}

/// Counter: `xiaoguai_watch_wakeups_total{watcher_id, outcome}`.
pub fn watch_wakeups_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.watch_wakeups_total)
}

/// Counter: `xiaoguai_im_messages_total{adapter, direction}`.
pub fn im_messages_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.im_messages_total)
}

/// Counter: `xiaoguai_compaction_triggered_total{reason}`.
pub fn compaction_triggered_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.compaction_triggered_total)
}

/// Counter: `xiaoguai_compaction_fallback_total{reason}`.
pub fn compaction_fallback_total() -> Option<&'static IntCounterVec> {
    HANDLES.get().map(|h| &h.compaction_fallback_total)
}

/// Histogram: `xiaoguai_compaction_token_savings`.
pub fn compaction_token_savings() -> Option<&'static Histogram> {
    HANDLES.get().map(|h| &h.compaction_token_savings)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a fresh registry + handles for each test to avoid the
    /// global `HANDLES` `OnceCell` interfering with assertion of exact counts.
    fn fresh() -> (Registry, MetricHandles) {
        init_prometheus().expect("init_prometheus failed in test")
    }

    #[test]
    fn prometheus_hotl_usage_total_increments() {
        let (_reg, h) = fresh();
        h.hotl_usage_total
            .with_label_values(&["t1", "llm_call", "allow"])
            .inc();
        let val = h
            .hotl_usage_total
            .with_label_values(&["t1", "llm_call", "allow"])
            .get();
        assert!(val > 0, "hotl_usage_total must be > 0 after inc()");
    }

    #[test]
    fn prometheus_hotl_check_duration_observes() {
        let (_reg, h) = fresh();
        h.hotl_check_duration.observe(0.001);
        // Histogram sample count should be 1.
        assert_eq!(
            h.hotl_check_duration.get_sample_count(),
            1,
            "hotl_check_duration must record one observation"
        );
    }

    #[test]
    fn prometheus_outcomes_recorded_total_increments() {
        let (_reg, h) = fresh();
        h.outcomes_recorded_total
            .with_label_values(&["tenant_a", "revenue_usd"])
            .inc();
        let val = h
            .outcomes_recorded_total
            .with_label_values(&["tenant_a", "revenue_usd"])
            .get();
        assert!(val > 0, "outcomes_recorded_total must be > 0 after inc()");
    }

    #[test]
    fn prometheus_outcomes_chain_depth_observes() {
        let (_reg, h) = fresh();
        h.outcomes_chain_depth.observe(3.0);
        assert_eq!(
            h.outcomes_chain_depth.get_sample_count(),
            1,
            "outcomes_chain_depth must record one observation"
        );
    }

    #[test]
    fn prometheus_rate_limit_hits_total_increments() {
        let (_reg, h) = fresh();
        h.rate_limit_hits_total
            .with_label_values(&["t2", "default", "deny"])
            .inc();
        let val = h
            .rate_limit_hits_total
            .with_label_values(&["t2", "default", "deny"])
            .get();
        assert!(val > 0, "rate_limit_hits_total must be > 0 after inc()");
    }

    #[test]
    fn prometheus_anomaly_detections_total_increments() {
        let (_reg, h) = fresh();
        h.anomaly_detections_total
            .with_label_values(&["zscore", "high"])
            .inc();
        let val = h
            .anomaly_detections_total
            .with_label_values(&["zscore", "high"])
            .get();
        assert!(val > 0, "anomaly_detections_total must be > 0 after inc()");
    }

    #[test]
    fn prometheus_watch_wakeups_total_increments() {
        let (_reg, h) = fresh();
        h.watch_wakeups_total
            .with_label_values(&["watcher-1", "match"])
            .inc();
        let val = h
            .watch_wakeups_total
            .with_label_values(&["watcher-1", "match"])
            .get();
        assert!(val > 0, "watch_wakeups_total must be > 0 after inc()");
    }

    #[test]
    fn prometheus_im_messages_total_increments() {
        let (_reg, h) = fresh();
        h.im_messages_total
            .with_label_values(&["feishu", "inbound"])
            .inc();
        let val = h
            .im_messages_total
            .with_label_values(&["feishu", "inbound"])
            .get();
        assert!(val > 0, "im_messages_total must be > 0 after inc()");
    }
}
