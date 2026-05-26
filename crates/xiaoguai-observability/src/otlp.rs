//! OTLP trace exporter via `tracing-opentelemetry`.
//!
//! Wires a `tracing_subscriber` layer that forwards spans to an
//! OpenTelemetry SDK pipeline that exports over OTLP/gRPC.
//!
//! # Configuration
//!
//! | Env var | Default | Description |
//! |---|---|---|
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC endpoint |
//! | `OTEL_SERVICE_NAME` | `xiaoguai` | Service name attached to every span |
//!
//! The service version is read from `CARGO_PKG_VERSION` at compile time.

use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{trace::SdkTracerProvider, Resource};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";
const SERVICE_NAME: &str = "xiaoguai";
const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the OTLP tracer provider and install it as the global
/// OpenTelemetry provider.
///
/// Returns the [`SdkTracerProvider`] so the caller can shut it down on exit.
///
/// # Errors
///
/// Returns an error if the OTLP pipeline fails to build (e.g. invalid
/// endpoint URL, gRPC init failure).
pub fn build_tracer_provider() -> Result<SdkTracerProvider> {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| DEFAULT_OTLP_ENDPOINT.to_string());

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| SERVICE_NAME.to_string());

    // opentelemetry_sdk 0.32: Resource is built via the builder; service.name
    // has a dedicated setter, other attributes go through with_attribute.
    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            SERVICE_VERSION,
        ))
        .build();

    // opentelemetry-otlp 0.32 API: SpanExporter::builder().with_tonic().build().
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .context("build OTLP span exporter")?;

    // 0.32: the batch span processor runs on a dedicated background thread,
    // so with_batch_exporter no longer takes a runtime argument.
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

/// Initialise `tracing_subscriber` with:
/// - `EnvFilter` (respects `RUST_LOG`)
/// - a JSON / fmt layer (stdout)
/// - an `OpenTelemetryLayer` exporting to the OTLP endpoint.
///
/// Call **once** from `main` before any tracing spans are created.
/// Returns the [`TracerProvider`] — keep it alive for the process
/// lifetime and call `.shutdown()` during graceful shutdown.
///
/// # Errors
///
/// Returns an error if `build_tracer_provider` fails or the subscriber
/// cannot be set as global default.
pub fn init_otlp() -> Result<SdkTracerProvider> {
    let provider = build_tracer_provider()?;
    let tracer = provider.tracer(SERVICE_NAME);
    let otel_layer = OpenTelemetryLayer::new(tracer);

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));

    Registry::default()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().json())
        .with(otel_layer)
        .try_init()
        .context("set global tracing subscriber")?;

    Ok(provider)
}

/// Flush and shut down the tracer provider.
///
/// Call during graceful shutdown to ensure all buffered spans are
/// exported before the process exits. Errors are logged but not
/// propagated — shutdown must always succeed.
pub fn shutdown_tracer(provider: &SdkTracerProvider) {
    if let Err(e) = provider.shutdown() {
        // Non-fatal: the process is exiting anyway.
        eprintln!("OTLP shutdown error: {e}");
    }
}
