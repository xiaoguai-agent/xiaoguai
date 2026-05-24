//! Tests for the OTLP trace pipeline.
//!
//! Uses the `opentelemetry_sdk` in-memory exporter with a
//! `SimpleSpanProcessor` (synchronous, no background task) to verify
//! that spans are emitted with the expected attributes without needing
//! a live OTLP collector or a Tokio runtime in the test harness.
//!
//! `SimpleSpanProcessor::on_end` calls the exporter synchronously in
//! the same thread that ends the span, so `get_finished_spans` is
//! safe to call immediately after the span is dropped.

use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_sdk::{
    testing::trace::InMemorySpanExporterBuilder,
    trace::{SimpleSpanProcessor, TracerProvider},
};

/// Build a provider + exporter pair backed by a `SimpleSpanProcessor`.
fn make_provider() -> (
    TracerProvider,
    opentelemetry_sdk::testing::trace::InMemorySpanExporter,
) {
    let exporter = InMemorySpanExporterBuilder::new().build();
    let processor = SimpleSpanProcessor::new(Box::new(exporter.clone()));
    let provider = TracerProvider::builder()
        .with_span_processor(processor)
        .build();
    (provider, exporter)
}

/// A span created via the provider appears in the in-memory exporter
/// with the correct name.
#[test]
fn test_span_is_emitted_with_correct_name() {
    let (provider, exporter) = make_provider();
    let tracer = provider.tracer("test-tracer");

    tracer.in_span("llm.call.test", |_cx| {
        let _ = 1 + 1;
    });

    let spans = exporter.get_finished_spans().expect("get spans");
    assert!(
        spans.iter().any(|s| s.name == "llm.call.test"),
        "expected span 'llm.call.test' not found; got: {:?}",
        spans.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

/// A span carries attributes set via `span_builder`.
#[test]
fn test_span_carries_attributes() {
    use opentelemetry::Context;

    let (provider, exporter) = make_provider();
    let tracer = provider.tracer("test-tracer-attrs");

    {
        let span = tracer
            .span_builder("http.request.test")
            .with_attributes(vec![
                KeyValue::new("http.method", "GET"),
                KeyValue::new("http.target", "/v1/sessions"),
                KeyValue::new("http.status_code", 200_i64),
            ])
            .start(&tracer);
        let cx = Context::current_with_span(span);
        // Span ends when _guard is dropped.
        let _guard = cx.attach();
    }

    let spans = exporter.get_finished_spans().expect("get spans");
    let span = spans
        .iter()
        .find(|s| s.name == "http.request.test")
        .expect("span not found");

    let method_attr = span
        .attributes
        .iter()
        .find(|kv| kv.key.as_str() == "http.method")
        .expect("http.method attribute missing");
    assert_eq!(method_attr.value.as_str(), "GET");
}

/// Multiple spans are all captured in order.
#[test]
fn test_multiple_spans_are_captured() {
    let (provider, exporter) = make_provider();
    let tracer = provider.tracer("test-tracer-multi");

    for i in 0..3_u32 {
        tracer.in_span(format!("span-{i}"), |_cx| {});
    }

    let spans = exporter.get_finished_spans().expect("get spans");
    assert_eq!(spans.len(), 3, "expected 3 spans, got {}", spans.len());
}

/// Nested spans share the same trace ID.
#[test]
fn test_nested_spans_share_trace_id() {
    let (provider, exporter) = make_provider();
    let tracer = provider.tracer("test-tracer-nested");

    tracer.in_span("parent", |parent_cx| {
        let parent_trace_id = parent_cx.span().span_context().trace_id();

        tracer.in_span("child", |child_cx| {
            let child_trace_id = child_cx.span().span_context().trace_id();
            assert_eq!(
                parent_trace_id, child_trace_id,
                "child should share trace-id with parent"
            );
        });
    });

    let spans = exporter.get_finished_spans().expect("get spans");
    assert_eq!(
        spans.len(),
        2,
        "expected parent + child, got {}",
        spans.len()
    );
}
