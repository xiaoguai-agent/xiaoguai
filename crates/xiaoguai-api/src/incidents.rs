//! Incident triage — webhook normalization adapter.
//!
//! Gated by the `XIAOGUAI_INCIDENTS_ENABLED` feature flag.
//!
//! Provides:
//! * [`Incident`] — common normalized incident struct.
//! * [`IncidentSource`] — trait for per-provider normalization.
//! * [`SentrySource`] — normalizes Sentry legacy webhook payloads.
//! * [`DatadogSource`] — normalizes Datadog Webhooks integration payloads.
//!
//! Each `IncidentSource` impl is intentionally thin: it extracts the
//! minimum high-signal fields from the raw JSON and maps vendor-specific
//! severity/priority labels onto the common [`Severity`] enum. The full
//! raw payload is preserved in [`Incident::raw`] so the triage agent can
//! reference it without losing context.
//!
//! # Deferred
//! - `PagerDutySource` — PD v2 alert schema (different field layout +
//!   on-call-rotation lookup via PD Schedules API). Blocked on: deciding
//!   whether the on-call lookup belongs inside the adapter or as a
//!   separate agent context step.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Common incident schema
// ---------------------------------------------------------------------------

/// Normalized severity shared across all incident sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    /// Map Sentry alert level strings to [`Severity`].
    fn from_sentry_level(level: &str) -> Self {
        match level {
            "fatal" => Self::Critical,
            "error" => Self::High,
            "warning" => Self::Medium,
            "info" | "debug" => Self::Low,
            _ => Self::Medium,
        }
    }

    /// Map Datadog alert priority strings (P1–P5) to [`Severity`].
    fn from_datadog_priority(priority: &str) -> Self {
        match priority {
            "P1" => Self::Critical,
            "P2" => Self::High,
            "P3" => Self::Medium,
            "P4" | "P5" => Self::Low,
            _ => Self::Medium,
        }
    }
}

/// Common normalized incident produced by all [`IncidentSource`] impls.
///
/// Designed to be the stable input to the triage agent regardless of
/// which observability platform fired the alert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    /// Vendor-scoped incident / issue ID (e.g. Sentry issue id, Datadog
    /// alert id). Prefixed with the source name for uniqueness:
    /// `sentry:123`, `datadog:456`.
    pub id: String,
    /// Human-readable title / name of the issue or monitor alert.
    pub title: String,
    /// Normalized severity.
    pub severity: Severity,
    /// Source platform identifier (`"sentry"`, `"datadog"`, …).
    pub source: String,
    /// When the incident first occurred (ISO-8601 UTC).
    pub occurred_at: DateTime<Utc>,
    /// Deep-link URL to the issue/alert in the originating platform.
    pub url: String,
    /// Project / service slug where the incident originated.
    pub project: String,
    /// Deployment environment (`"production"`, `"staging"`, …).
    /// `None` when not determinable from the payload.
    pub environment: Option<String>,
    /// Full raw webhook payload preserved for agent context injection.
    pub raw: Value,
}

// ---------------------------------------------------------------------------
// IncidentSource trait
// ---------------------------------------------------------------------------

/// Error returned by [`IncidentSource::normalize`].
#[derive(Debug, Error)]
pub enum NormalizeError {
    /// The JSON payload is structurally invalid or missing required fields.
    #[error("malformed payload: {0}")]
    Malformed(String),
    /// The provider's action/alert_type is known but intentionally ignored
    /// (e.g. "resolved" in Sentry). The HTTP handler should return 200
    /// with a no-op body.
    #[error("ignored action: {0}")]
    Ignored(String),
}

/// Normalize a raw webhook payload into the common [`Incident`] schema.
///
/// Implementors:
/// * [`SentrySource`]
/// * [`DatadogSource`]
///
/// # Contract
/// - Returns `Err(NormalizeError::Ignored)` for known-but-unactionable
///   payloads (resolved alerts, etc.) — callers respond 200 and skip
///   triage.
/// - Returns `Err(NormalizeError::Malformed)` for genuinely broken
///   payloads — callers respond 400.
/// - Preserves the full `raw` payload in [`Incident::raw`] so the agent
///   has the complete context without re-parsing.
pub trait IncidentSource: Send + Sync {
    fn normalize(&self, raw: Value) -> Result<Incident, NormalizeError>;
    fn source_name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// SentrySource
// ---------------------------------------------------------------------------

/// Normalizes Sentry legacy webhook payloads.
///
/// Sentry delivers `{"action": "created"|"assigned"|"triggered", "data":
/// {"issue": {...}}, ...}`. Actions other than those three are ignored.
pub struct SentrySource;

impl IncidentSource for SentrySource {
    fn source_name(&self) -> &'static str {
        "sentry"
    }

    fn normalize(&self, raw: Value) -> Result<Incident, NormalizeError> {
        // Gate on action — only process actionable events.
        let action = raw
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `action` field".into()))?;

        match action {
            "created" | "assigned" | "triggered" => {}
            other => return Err(NormalizeError::Ignored(other.to_owned())),
        }

        let issue = raw
            .pointer("/data/issue")
            .ok_or_else(|| NormalizeError::Malformed("missing `data.issue`".into()))?;

        let id_raw = issue
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `data.issue.id`".into()))?;

        let title = issue
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `data.issue.title`".into()))?
            .to_owned();

        let level = issue
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let severity = Severity::from_sentry_level(level);

        let occurred_at_str = issue
            .get("firstSeen")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `data.issue.firstSeen`".into()))?;
        let occurred_at = occurred_at_str.parse::<DateTime<Utc>>().map_err(|e| {
            NormalizeError::Malformed(format!("invalid firstSeen timestamp: {e}"))
        })?;

        let url = issue
            .get("permalink")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        let project = issue
            .pointer("/project/slug")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();

        // Extract environment from tags array: [{key: "environment", value: "..."}]
        let environment = issue
            .get("tags")
            .and_then(Value::as_array)
            .and_then(|tags| {
                tags.iter().find_map(|t| {
                    if t.get("key").and_then(Value::as_str) == Some("environment") {
                        t.get("value").and_then(Value::as_str).map(str::to_owned)
                    } else {
                        None
                    }
                })
            });

        Ok(Incident {
            id: format!("sentry:{id_raw}"),
            title,
            severity,
            source: "sentry".into(),
            occurred_at,
            url,
            project,
            environment,
            raw,
        })
    }
}

// ---------------------------------------------------------------------------
// DatadogSource
// ---------------------------------------------------------------------------

/// Normalizes Datadog Webhooks integration payloads.
///
/// Datadog delivers a flat JSON object containing `alert_id`, `title`,
/// `alert_priority` (P1..P5), `alert_type`, `last_updated_at`,
/// `event_url`, and a comma-separated `tags` string.
///
/// `alert_type` values other than `"error"`, `"warning"`, `"info"` are
/// treated as ignored (e.g. `"success"` = recovery).
pub struct DatadogSource;

impl IncidentSource for DatadogSource {
    fn source_name(&self) -> &'static str {
        "datadog"
    }

    fn normalize(&self, raw: Value) -> Result<Incident, NormalizeError> {
        // Gate on alert_type.
        let alert_type = raw
            .get("alert_type")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `alert_type`".into()))?;

        match alert_type {
            "error" | "warning" | "info" => {}
            other => return Err(NormalizeError::Ignored(other.to_owned())),
        }

        let alert_id = raw
            .get("alert_id")
            .and_then(Value::as_str)
            .or_else(|| raw.get("id").and_then(Value::as_str))
            .ok_or_else(|| NormalizeError::Malformed("missing `alert_id`".into()))?;

        let title = raw
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `title`".into()))?
            .to_owned();

        let priority = raw
            .get("alert_priority")
            .and_then(Value::as_str)
            .unwrap_or("P3");
        let severity = Severity::from_datadog_priority(priority);

        let occurred_at_str = raw
            .get("last_updated_at")
            .and_then(Value::as_str)
            .ok_or_else(|| NormalizeError::Malformed("missing `last_updated_at`".into()))?;
        let occurred_at = occurred_at_str.parse::<DateTime<Utc>>().map_err(|e| {
            NormalizeError::Malformed(format!("invalid last_updated_at timestamp: {e}"))
        })?;

        let url = raw
            .get("event_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        // Parse comma-separated tags string "env:production,host:web-01" into
        // a lookup map for environment and project extraction.
        let tags_str = raw
            .get("tags")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tags = parse_datadog_tags(tags_str);

        let environment = tags.get("env").or_else(|| tags.get("environment")).cloned();
        let project = tags
            .get("host")
            .or_else(|| tags.get("service"))
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned());

        Ok(Incident {
            id: format!("datadog:{alert_id}"),
            title,
            severity,
            source: "datadog".into(),
            occurred_at,
            url,
            project,
            environment,
            raw,
        })
    }
}

/// Parse a Datadog comma-separated tag string into a `key → value` map.
///
/// Format: `"env:production,host:web-01,service:api"`.
/// Tags without a colon (plain labels) are ignored.
fn parse_datadog_tags(tags: &str) -> std::collections::HashMap<String, String> {
    tags.split(',')
        .filter_map(|t| {
            let mut parts = t.trim().splitn(2, ':');
            let key = parts.next()?.trim().to_owned();
            let val = parts.next()?.trim().to_owned();
            if key.is_empty() {
                None
            } else {
                Some((key, val))
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // SentrySource tests
    // -----------------------------------------------------------------------

    fn sentry_sample() -> Value {
        json!({
            "action": "created",
            "installation": {"uuid": "install-uuid"},
            "data": {
                "issue": {
                    "id": "123",
                    "title": "ZeroDivisionError: division by zero",
                    "level": "error",
                    "firstSeen": "2024-05-24T10:00:00.000Z",
                    "permalink": "https://sentry.io/organizations/acme/issues/123/",
                    "project": {"slug": "backend"},
                    "tags": [
                        {"key": "environment", "value": "production"},
                        {"key": "release", "value": "v1.2.3"}
                    ]
                }
            },
            "actor": {}
        })
    }

    #[test]
    fn sentry_sample_normalizes_to_expected_incident() {
        let src = SentrySource;
        let incident = src.normalize(sentry_sample()).expect("normalize");

        assert_eq!(incident.id, "sentry:123");
        assert_eq!(incident.title, "ZeroDivisionError: division by zero");
        assert_eq!(incident.severity, Severity::High);
        assert_eq!(incident.source, "sentry");
        assert_eq!(
            incident.occurred_at,
            "2024-05-24T10:00:00Z"
                .parse::<DateTime<Utc>>()
                .unwrap()
        );
        assert_eq!(
            incident.url,
            "https://sentry.io/organizations/acme/issues/123/"
        );
        assert_eq!(incident.project, "backend");
        assert_eq!(incident.environment.as_deref(), Some("production"));
        // raw payload preserved
        assert_eq!(incident.raw.pointer("/data/issue/id").unwrap(), "123");
    }

    #[test]
    fn sentry_fatal_level_maps_to_critical() {
        let src = SentrySource;
        let mut payload = sentry_sample();
        payload["data"]["issue"]["level"] = json!("fatal");
        let incident = src.normalize(payload).unwrap();
        assert_eq!(incident.severity, Severity::Critical);
    }

    #[test]
    fn sentry_warning_level_maps_to_medium() {
        let src = SentrySource;
        let mut payload = sentry_sample();
        payload["data"]["issue"]["level"] = json!("warning");
        let incident = src.normalize(payload).unwrap();
        assert_eq!(incident.severity, Severity::Medium);
    }

    #[test]
    fn sentry_resolved_action_is_ignored() {
        let src = SentrySource;
        let mut payload = sentry_sample();
        payload["action"] = json!("resolved");
        match src.normalize(payload) {
            Err(NormalizeError::Ignored(a)) => assert_eq!(a, "resolved"),
            other => panic!("expected Ignored, got {other:?}"),
        }
    }

    #[test]
    fn sentry_missing_action_is_malformed() {
        let src = SentrySource;
        let payload = json!({"data": {"issue": {}}});
        assert!(matches!(
            src.normalize(payload),
            Err(NormalizeError::Malformed(_))
        ));
    }

    #[test]
    fn sentry_missing_issue_is_malformed() {
        let src = SentrySource;
        let payload = json!({"action": "created", "data": {}});
        assert!(matches!(
            src.normalize(payload),
            Err(NormalizeError::Malformed(_))
        ));
    }

    #[test]
    fn sentry_no_environment_tag_yields_none() {
        let src = SentrySource;
        let mut payload = sentry_sample();
        payload["data"]["issue"]["tags"] = json!([{"key": "release", "value": "v1"}]);
        let incident = src.normalize(payload).unwrap();
        assert!(incident.environment.is_none());
    }

    // -----------------------------------------------------------------------
    // DatadogSource tests
    // -----------------------------------------------------------------------

    fn datadog_sample() -> Value {
        json!({
            "id": "dd-event-456",
            "title": "[Triggered on {host:web-01}] High CPU usage on web-01",
            "alert_id": "456",
            "alert_priority": "P1",
            "alert_type": "error",
            "last_updated_at": "2024-05-24T11:30:00.000Z",
            "event_url": "https://app.datadoghq.com/monitors/456",
            "tags": "env:production,host:web-01,service:api",
            "body": "CPU usage exceeded 90% for 15 minutes"
        })
    }

    #[test]
    fn datadog_sample_normalizes_to_expected_incident() {
        let src = DatadogSource;
        let incident = src.normalize(datadog_sample()).expect("normalize");

        assert_eq!(incident.id, "datadog:456");
        assert_eq!(
            incident.title,
            "[Triggered on {host:web-01}] High CPU usage on web-01"
        );
        assert_eq!(incident.severity, Severity::Critical);
        assert_eq!(incident.source, "datadog");
        assert_eq!(
            incident.occurred_at,
            "2024-05-24T11:30:00Z"
                .parse::<DateTime<Utc>>()
                .unwrap()
        );
        assert_eq!(
            incident.url,
            "https://app.datadoghq.com/monitors/456"
        );
        assert_eq!(incident.project, "web-01");
        assert_eq!(incident.environment.as_deref(), Some("production"));
        // raw payload preserved
        assert_eq!(
            incident.raw.get("body").unwrap().as_str().unwrap(),
            "CPU usage exceeded 90% for 15 minutes"
        );
    }

    #[test]
    fn datadog_p2_maps_to_high() {
        let src = DatadogSource;
        let mut payload = datadog_sample();
        payload["alert_priority"] = json!("P2");
        let incident = src.normalize(payload).unwrap();
        assert_eq!(incident.severity, Severity::High);
    }

    #[test]
    fn datadog_p3_maps_to_medium() {
        let src = DatadogSource;
        let mut payload = datadog_sample();
        payload["alert_priority"] = json!("P3");
        let incident = src.normalize(payload).unwrap();
        assert_eq!(incident.severity, Severity::Medium);
    }

    #[test]
    fn datadog_success_alert_type_is_ignored() {
        let src = DatadogSource;
        let mut payload = datadog_sample();
        payload["alert_type"] = json!("success");
        match src.normalize(payload) {
            Err(NormalizeError::Ignored(a)) => assert_eq!(a, "success"),
            other => panic!("expected Ignored, got {other:?}"),
        }
    }

    #[test]
    fn datadog_missing_alert_type_is_malformed() {
        let src = DatadogSource;
        let payload = json!({"alert_id": "1", "title": "t", "last_updated_at": "2024-01-01T00:00:00Z"});
        assert!(matches!(
            src.normalize(payload),
            Err(NormalizeError::Malformed(_))
        ));
    }

    #[test]
    fn datadog_missing_alert_id_is_malformed() {
        let src = DatadogSource;
        let payload = json!({"alert_type": "error", "title": "t", "last_updated_at": "2024-01-01T00:00:00Z"});
        assert!(matches!(
            src.normalize(payload),
            Err(NormalizeError::Malformed(_))
        ));
    }

    #[test]
    fn datadog_tags_parsed_correctly() {
        let tags = parse_datadog_tags("env:production,host:web-01,service:api");
        assert_eq!(tags.get("env").map(String::as_str), Some("production"));
        assert_eq!(tags.get("host").map(String::as_str), Some("web-01"));
        assert_eq!(tags.get("service").map(String::as_str), Some("api"));
    }

    #[test]
    fn datadog_empty_tags_string_yields_empty_map() {
        let tags = parse_datadog_tags("");
        assert!(tags.is_empty());
    }

    #[test]
    fn datadog_tags_without_colon_are_skipped() {
        let tags = parse_datadog_tags("plainlabel,env:prod");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags.get("env").map(String::as_str), Some("prod"));
    }

    // -----------------------------------------------------------------------
    // Triage-agent integration test with mock LLM
    // -----------------------------------------------------------------------

    /// Simulates the triage-agent contract: given an Incident and a
    /// pre-canned RCA JSON from a mock LLM, assert the output PR-draft
    /// and IM-message structures are well-formed.
    ///
    /// This test does not call a real LLM or GitHub API — it verifies
    /// the data-flow contract: Incident → RcaDraft (parsed from mock LLM
    /// response) → PrDraft + ImNotification shapes.
    #[test]
    fn triage_agent_integration_mock_llm_output() {
        // 1. Normalize an incoming Sentry webhook.
        let raw = sentry_sample();
        let incident = SentrySource.normalize(raw).unwrap();

        // 2. Mock LLM returns a pre-canned RCA JSON string.
        let mock_llm_response = json!({
            "summary": "A ZeroDivisionError in the payment processor caused 5xx errors.",
            "impact": "~200 users on production checkout affected for 12 minutes.",
            "root_cause": "Commit abc123 introduced a divide-by-zero in discount_calc.py.",
            "timeline": [
                {"time": "2024-05-24T09:55:00Z", "event": "Deploy v1.2.4 to production"},
                {"time": "2024-05-24T10:00:00Z", "event": "Sentry alert fired: ZeroDivisionError"}
            ],
            "action_items": [
                {"assignee": "backend-team", "action": "Revert commit abc123", "priority": "P0"},
                {"assignee": "backend-team", "action": "Add test for zero-discount edge case", "priority": "P1"}
            ],
            "confidence": "high",
            "evidence_refs": ["commit:abc123", "deploy:v1.2.4"]
        });

        // 3. Parse into RcaDraft.
        let rca: RcaDraft = serde_json::from_value(mock_llm_response).unwrap();
        assert_eq!(rca.confidence, "high");
        assert_eq!(rca.action_items.len(), 2);
        assert_eq!(rca.action_items[0].priority, "P0");

        // 4. Assert PrDraft structure.
        let pr = PrDraft::from_incident_and_rca(&incident, &rca, "0.1.0");
        assert!(pr.title.contains("ZeroDivisionError"));
        assert!(pr.title.starts_with("[RCA Draft]"));
        assert!(pr.body.contains("## 1. Summary"));
        assert!(pr.body.contains("## 5. Action Items"));
        assert!(pr.labels.contains(&"incident".to_owned()));
        assert!(pr.labels.contains(&"severity-high".to_owned()));
        assert!(pr.draft);

        // 5. Assert ImNotification structure.
        let im = ImNotification::from_incident_and_rca(
            &incident,
            &rca,
            "https://github.com/acme/repo/pull/42",
        );
        assert!(im.text.contains("[RCA Draft]"));
        assert!(im.text.contains("sentry:123"));
        // IM message fits within 500-char budget.
        assert!(im.text.len() <= 500, "IM message too long: {}", im.text.len());
    }

    // -----------------------------------------------------------------------
    // End-to-end: simulated webhook → IM notification via Feishu mock
    // -----------------------------------------------------------------------

    /// Full pipeline test:
    /// Sentry webhook payload → SentrySource::normalize → RcaDraft (canned)
    /// → ImNotification → recorded in a mock Feishu-style sink.
    ///
    /// Proves the data flows through without panicking and that the sink
    /// receives exactly one notification whose text contains the incident ID.
    #[test]
    fn e2e_webhook_to_im_via_feishu_mock() {
        // Simulate a raw webhook POST body arriving from Sentry.
        let webhook_body = sentry_sample();

        // Normalize.
        let incident = SentrySource
            .normalize(webhook_body)
            .expect("normalize should succeed");

        // Mock triage-agent output (would come from real LLM in production).
        let mock_rca = RcaDraft {
            summary: "ZeroDivisionError in payment processor.".into(),
            impact: "200 users, 12 min.".into(),
            root_cause: "Commit abc123.".into(),
            timeline: vec![],
            action_items: vec![],
            confidence: "medium".into(),
            evidence_refs: vec!["commit:abc123".into()],
        };

        // Build IM notification.
        let pr_url = "https://github.com/acme/repo/pull/99";
        let im = ImNotification::from_incident_and_rca(&incident, &mock_rca, pr_url);

        // Simulate a Feishu-style recording sink.
        let sink: std::sync::Arc<parking_lot::Mutex<Vec<ImNotification>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

        // "Send" — in production this calls FeishuProvider::reply; here we
        // record directly so the test stays sync and dependency-free.
        sink.lock().push(im);

        // Assert sink received exactly one message with the incident ID.
        let sent = sink.lock();
        assert_eq!(sent.len(), 1, "expected exactly one IM notification");
        assert!(
            sent[0].text.contains("sentry:123"),
            "IM message must reference the incident ID"
        );
        assert!(
            sent[0].text.contains(pr_url),
            "IM message must include the draft PR URL"
        );
    }

    // -----------------------------------------------------------------------
    // source_name tests
    // -----------------------------------------------------------------------

    #[test]
    fn source_names_are_stable() {
        assert_eq!(SentrySource.source_name(), "sentry");
        assert_eq!(DatadogSource.source_name(), "datadog");
    }
}

// ---------------------------------------------------------------------------
// Output structures (used by tests and the triage agent)
// ---------------------------------------------------------------------------

/// Parsed RCA draft produced by the triage agent's LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RcaDraft {
    pub summary: String,
    pub impact: String,
    pub root_cause: String,
    pub timeline: Vec<TimelineEntry>,
    pub action_items: Vec<ActionItem>,
    pub confidence: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub time: String,
    pub event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub assignee: String,
    pub action: String,
    pub priority: String,
}

/// Draft GitHub PR produced by the draft-pr output connector.
#[derive(Debug, Clone)]
pub struct PrDraft {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub draft: bool,
}

impl PrDraft {
    /// Build a [`PrDraft`] from a normalized incident + RCA draft.
    /// Renders a minimal Markdown body suitable for a GitHub PR description.
    pub fn from_incident_and_rca(incident: &Incident, rca: &RcaDraft, pack_version: &str) -> Self {
        let severity = format!("{:?}", incident.severity).to_lowercase();
        let title = format!(
            "[RCA Draft] {}",
            truncate_str(&incident.title, 80)
        );
        let labels = vec![
            "incident".to_owned(),
            "rca-draft".to_owned(),
            format!("severity-{severity}"),
        ];
        let body = render_rca_markdown(incident, rca, pack_version);
        Self {
            title,
            body,
            labels,
            draft: true,
        }
    }
}

/// IM notification message produced by the draft-pr output connector.
#[derive(Debug, Clone)]
pub struct ImNotification {
    pub channel: String,
    pub text: String,
}

impl ImNotification {
    /// Build an [`ImNotification`] from a normalized incident + RCA draft.
    /// The text is kept to ≤ 500 characters to fit IM card constraints.
    pub fn from_incident_and_rca(incident: &Incident, rca: &RcaDraft, pr_url: &str) -> Self {
        let severity = format!("{:?}", incident.severity).to_uppercase();
        let root_cause_short = truncate_str(&rca.root_cause, 120);
        let text = format!(
            "[RCA Draft] {} ({})\nIncident: {} | Severity: {}\nRoot cause ({}): {}\nDraft PR: {}",
            truncate_str(&incident.title, 60),
            incident.source,
            incident.id,
            severity,
            rca.confidence,
            root_cause_short,
            pr_url,
        );
        // Trim to hard 500-char limit.
        let text = truncate_str(&text, 500).to_owned();
        Self {
            channel: String::new(), // set by the output connector at send time
            text,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max` characters (character boundary safe).
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Walk backwards from `max` to find a valid char boundary.
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Render the 5-section RCA markdown body for the GitHub PR description.
fn render_rca_markdown(incident: &Incident, rca: &RcaDraft, pack_version: &str) -> String {
    let severity = format!("{:?}", incident.severity).to_uppercase();
    let mut md = format!(
        "# Incident RCA: {title}\n\n\
        > **Incident ID**: `{id}`  \n\
        > **Source**: {source}  \n\
        > **Severity**: {severity}  \n\
        > **Environment**: {env}  \n\
        > **Project**: {project}  \n\
        > **Occurred at**: {occurred_at}  \n\
        > **RCA confidence**: {confidence}  \n\n\
        ---\n\n\
        ## 1. Summary\n\n{summary}\n\n\
        ---\n\n\
        ## 2. Impact\n\n{impact}\n\n\
        ---\n\n\
        ## 3. Root Cause\n\n{root_cause}\n\n",
        title = incident.title,
        id = incident.id,
        source = incident.source,
        severity = severity,
        env = incident.environment.as_deref().unwrap_or("unknown"),
        project = incident.project,
        occurred_at = incident.occurred_at,
        confidence = rca.confidence,
        summary = rca.summary,
        impact = rca.impact,
        root_cause = rca.root_cause,
    );

    // Evidence refs
    if !rca.evidence_refs.is_empty() {
        md.push_str("**Evidence references:**\n");
        for r in &rca.evidence_refs {
            md.push_str(&format!("- `{r}`\n"));
        }
        md.push('\n');
    }

    // Timeline
    md.push_str("---\n\n## 4. Timeline\n\n| Time (UTC) | Event |\n|---|---|\n");
    for entry in &rca.timeline {
        md.push_str(&format!("| `{}` | {} |\n", entry.time, entry.event));
    }

    // Action items
    md.push_str("\n---\n\n## 5. Action Items\n\n| Priority | Assignee | Action |\n|---|---|---|\n");
    for item in &rca.action_items {
        md.push_str(&format!(
            "| {} | {} | {} |\n",
            item.priority, item.assignee, item.action
        ));
    }

    md.push_str(&format!(
        "\n---\n\n*Generated automatically by xiaoguai incident-triage pack v{pack_version}.*\n"
    ));
    md
}
