//! PII redaction for exported OpenTelemetry spans.
//!
//! [`RedactingSpanExporter`] wraps any [`SpanExporter`] and scrubs string
//! attribute values (via [`xiaoguai_types::redact_str`]) from each span just
//! before export.
//!
//! It is implemented as an **exporter decorator**, not a sibling
//! `SpanProcessor`: the SDK hands every processor its own `SpanData` copy, so
//! mutating a span in one processor would not change what the batch exporter
//! actually sends. Wrapping the exporter is the only place a rewrite reliably
//! reaches the wire.
//!
//! Redaction is on by default; set `XIAOGUAI_OBSERVABILITY_REDACT_PII` to a
//! falsey value (`false`/`0`/`no`/`off`) to disable.

use std::time::Duration;

use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use opentelemetry_sdk::Resource;
use xiaoguai_types::redact_str;

/// Span-attribute keys left untouched — structural identifiers, not user data.
const SKIP_KEYS: &[&str] = &[
    "service.name",
    "service.version",
    "telemetry.sdk.name",
    "telemetry.sdk.version",
    "telemetry.sdk.language",
    "otel.scope.name",
    "otel.scope.version",
];

/// Whether redaction is enabled, per `XIAOGUAI_OBSERVABILITY_REDACT_PII`
/// (on by default).
fn redaction_enabled() -> bool {
    match std::env::var("XIAOGUAI_OBSERVABILITY_REDACT_PII") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Redact string attribute values in place. Keys in [`SKIP_KEYS`] and
/// non-string values are left untouched.
fn redact_attributes(attrs: &mut [KeyValue]) {
    for kv in attrs.iter_mut() {
        if SKIP_KEYS.contains(&kv.key.as_str()) {
            continue;
        }
        let replacement = match &kv.value {
            Value::String(s) => {
                let redacted = redact_str(s.as_str());
                (redacted != s.as_str()).then(|| Value::String(redacted.into()))
            }
            _ => None,
        };
        if let Some(v) = replacement {
            kv.value = v;
        }
    }
}

/// A [`SpanExporter`] decorator that redacts PII from string span attributes
/// before delegating to the wrapped exporter.
#[derive(Debug)]
pub struct RedactingSpanExporter<E: SpanExporter> {
    inner: E,
    enabled: bool,
}

impl<E: SpanExporter> RedactingSpanExporter<E> {
    /// Wrap `inner`, reading the enable flag from the environment once.
    #[must_use]
    pub fn new(inner: E) -> Self {
        Self {
            inner,
            enabled: redaction_enabled(),
        }
    }
}

impl<E: SpanExporter> SpanExporter for RedactingSpanExporter<E> {
    fn export(
        &self,
        mut batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        if self.enabled {
            for span in &mut batch {
                redact_attributes(&mut span.attributes);
            }
        }
        self.inner.export(batch)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::redact_attributes;
    use opentelemetry::{KeyValue, Value};

    #[test]
    fn redacts_string_attrs_skips_structural_and_nonstring() {
        let mut attrs = vec![
            KeyValue::new("user.email", "alice@example.com"),
            KeyValue::new("client.ip", "10.0.0.1"),
            KeyValue::new("service.name", "xiaoguai"), // structural — skipped
            KeyValue::new("http.status_code", 200_i64), // non-string — untouched
        ];
        redact_attributes(&mut attrs);

        assert!(matches!(&attrs[0].value, Value::String(s) if s.as_str() == "[redacted-email]"));
        assert!(matches!(&attrs[1].value, Value::String(s) if s.as_str() == "[redacted-ip]"));
        assert!(matches!(&attrs[2].value, Value::String(s) if s.as_str() == "xiaoguai"));
        assert!(matches!(attrs[3].value, Value::I64(200)));
    }
}
