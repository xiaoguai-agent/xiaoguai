//! Prometheus metrics registry + HTTP exposition for Xiaoguai.
//!
//! # Exported metrics
//!
//! | Name | Type | Labels | Description |
//! |---|---|---|---|
//! | `xiaoguai_http_request_duration_seconds` | Histogram | `method`, `path`, `status` | HTTP request latency |
//! | `xiaoguai_llm_call_duration_seconds` | Histogram | `provider`, `model` | LLM call latency |
//! | `xiaoguai_scheduler_tick_duration_seconds` | Histogram | — | Scheduler tick latency |
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
    Histogram, HistogramVec, Registry,
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
        latency_buckets,
        registry
    )
    .context("register scheduler_tick_duration_seconds")?;

    let handles = MetricHandles {
        http_request_duration,
        llm_call_duration,
        scheduler_tick_duration,
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
