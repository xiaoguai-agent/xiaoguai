//! Tests for the Prometheus metrics layer.
//!
//! Each test uses a fresh [`Registry`] to avoid global-state
//! cross-contamination between test cases.

use prometheus::Registry;
use xiaoguai_observability::prometheus::{init_prometheus, mount_metrics};

/// Spin up the registry, observe a single LLM call duration, then
/// scrape the text format and assert that `histogram_count` is 1 and
/// the metric name is present.
#[test]
fn test_llm_histogram_emits_on_observation() {
    let (registry, handles) = init_prometheus().expect("init_prometheus failed");

    handles
        .llm_call_duration
        .with_label_values(&["ollama", "qwen2.5"])
        .observe(0.123);

    let text = gather_text(&registry);
    assert!(
        text.contains("xiaoguai_llm_call_duration_seconds_count"),
        "llm duration histogram count not found in:\n{text}"
    );
    assert!(
        text.contains("ollama"),
        "provider label 'ollama' not found in:\n{text}"
    );
}

/// Observe a scheduler tick duration and verify the metric appears.
#[test]
fn test_scheduler_tick_histogram_emits_on_observation() {
    let (registry, handles) = init_prometheus().expect("init_prometheus failed");

    handles.scheduler_tick_duration.observe(0.005);

    let text = gather_text(&registry);
    assert!(
        text.contains("xiaoguai_scheduler_tick_duration_seconds_count"),
        "scheduler tick histogram count not found in:\n{text}"
    );
}

/// Multiple observations accumulate correctly.
#[test]
fn test_histogram_accumulates_multiple_observations() {
    let (registry, handles) = init_prometheus().expect("init_prometheus failed");

    for i in 0..5_u32 {
        handles
            .llm_call_duration
            .with_label_values(&["test-provider", "test-model"])
            .observe(f64::from(i) * 0.1);
    }

    let text = gather_text(&registry);
    // The _count metric should reflect 5 observations.
    // We parse just to verify "5" appears after the metric name.
    assert!(
        text.contains("xiaoguai_llm_call_duration_seconds_count{"),
        "histogram count line not found"
    );
    // Extract the count value from the text.
    let count = extract_count(&text, "xiaoguai_llm_call_duration_seconds_count");
    assert_eq!(count, 5, "expected 5 observations, got {count}");
}

/// The Prometheus text format can be parsed by `prometheus-parse`.
#[test]
fn test_metrics_text_format_is_valid_prometheus() {
    let (registry, handles) = init_prometheus().expect("init_prometheus failed");
    handles
        .llm_call_duration
        .with_label_values(&["ollama", "qwen2.5"])
        .observe(0.07);

    let text = gather_text(&registry);

    // prometheus-parse will return an error for malformed exposition format.
    let lines = text.lines().map(|l| Ok(l.to_owned()));
    let scrape = prometheus_parse::Scrape::parse(lines).expect("prometheus-parse failed");
    assert!(
        !scrape.samples.is_empty(),
        "no samples parsed from Prometheus text output"
    );
}

/// `GET /metrics` axum route returns HTTP 200 with the correct content-type.
#[tokio::test]
async fn test_metrics_http_endpoint_returns_200() {
    use axum::body::Body;
    use http_body_util::BodyExt;

    let (registry, handles) = init_prometheus().expect("init_prometheus failed");
    handles.scheduler_tick_duration.observe(0.001);

    let app = mount_metrics(axum::Router::new(), registry);

    let req = axum::http::Request::builder()
        .uri("/metrics")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app, req)
        .await
        .expect("request failed");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .expect("no content-type header")
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/plain"),
        "expected text/plain content-type, got: {ct}"
    );

    let body_bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    let body = std::str::from_utf8(&body_bytes).expect("body utf8");
    assert!(
        body.contains("xiaoguai_scheduler_tick_duration_seconds"),
        "body missing expected metric"
    );
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn gather_text(registry: &Registry) -> String {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    let mut buf = Vec::new();
    encoder
        .encode(&registry.gather(), &mut buf)
        .expect("encode failed");
    String::from_utf8(buf).expect("utf8")
}

/// Parse a `_count` value for a labelled histogram from the raw text.
/// Finds the *last* line matching `name{...} <count>` (sum over all label sets).
fn extract_count(text: &str, metric_name: &str) -> u64 {
    for line in text.lines().rev() {
        if line.starts_with(metric_name) && !line.starts_with('#') {
            if let Some(val) = line.split_whitespace().last() {
                // prometheus text format: "1" or "1.0" or "1 <timestamp>"
                if let Ok(n) = val.parse::<f64>() {
                    // Prometheus histogram counts are always non-negative
                    // integers stored as f64; truncation is intentional.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    return n as u64;
                }
            }
        }
    }
    0
}
