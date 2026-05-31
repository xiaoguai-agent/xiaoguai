//! Alertmanager webhook → `WatchEvent` bridge (sprint-10 S10-5 / DEC-022).
//!
//! When a Prometheus burn-rate alert fires (see
//! `deploy/helm/xiaoguai-observability/templates/prometheus-rules.yaml`),
//! Alertmanager dispatches an HTTP POST to `/internal/alerts` carrying the
//! standard Alertmanager v2 webhook payload. The axum handler — mounted in
//! `xiaoguai-api` — parses each alert into an [`AlertmanagerEvent`] and pushes
//! it to a per-process [`AlertmanagerInbox`].
//!
//! [`AlertmanagerWebhookSource`] is a [`crate::source::WatchSource`] that drains
//! the inbox on poll. SLO breaches therefore flow through the existing
//! `WatchRunner` → `WatchEvent` → dispatcher pipeline (LLD-OBS-001 §4.8).
//! No new Alertmanager receiver, no new dispatcher — DEC-022 "no new alerting
//! plumbing" honoured.
//!
//! ## Design choice — inbox + `WatchSource`, not direct emit
//!
//! Two alternatives were considered:
//!
//! 1. **Direct emit.** Webhook handler builds a `WatchEvent` directly and
//!    sends it on the runner's channel.
//! 2. **Inbox + `WatchSource` (this module).** Handler pushes to an mpsc-style
//!    buffer; a `WatchSource` impl drains it.
//!
//! (2) wins because it reuses the existing dedup cache (`crate::dedup`) — two
//! Alertmanager webhook deliveries for the same alert (alert routing retry)
//! collapse to one `WatchEvent`. (1) would need its own dedup.

use crate::source::{Match, SourceError, WatchSource};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map};
use std::sync::Arc;
use tokio::sync::Mutex;
use xiaoguai_observability::Signal;

/// One Alertmanager alert decoded into the SLO-specific shape we publish as a
/// `WatchEvent`. Field names are stable — downstream watchers depend on them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlertmanagerEvent {
    /// The `signal` label on the alert rule (`latency` / `traffic` / `errors` / `saturation`).
    pub signal: Signal,
    /// The `burn_rate_pair` or `window` label — short label distinguishing fast vs slow burn.
    /// We emit `"fast"` or `"slow"` regardless of how the alert labels it, mapped from the
    /// `burn_rate_pair` value (1 → fast, others → slow).
    pub window: BurnWindow,
    /// The HTTP route / queue name / budget identifier.
    pub surface: String,
    /// Tenant ID if the alert was per-tenant; `None` for global alerts.
    pub tenant: Option<String>,
    /// Severity from the alert label (`critical` / `warning`).
    pub severity: String,
    /// Original Alertmanager `alertname` (kept for traceability).
    pub alert_name: String,
    /// Alertmanager `status` — `firing` or `resolved`.
    pub status: AlertStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BurnWindow {
    Fast,
    Slow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlertStatus {
    Firing,
    Resolved,
}

impl AlertmanagerEvent {
    /// Render as the JSON `Match` body that the watch runner publishes.
    /// Field ordering matches what the `WatchSpec` deduplicator hashes —
    /// changing field names is a wire-compat event.
    #[must_use]
    pub fn to_match(&self) -> Match {
        let mut m = Map::new();
        m.insert("kind".into(), json!("slo_breach"));
        m.insert("signal".into(), json!(self.signal.as_label()));
        m.insert(
            "window".into(),
            json!(match self.window {
                BurnWindow::Fast => "fast",
                BurnWindow::Slow => "slow",
            }),
        );
        m.insert("surface".into(), json!(self.surface));
        if let Some(t) = &self.tenant {
            m.insert("tenant".into(), json!(t));
        }
        m.insert("severity".into(), json!(self.severity));
        m.insert("alert_name".into(), json!(self.alert_name));
        m.insert(
            "status".into(),
            json!(match self.status {
                AlertStatus::Firing => "firing",
                AlertStatus::Resolved => "resolved",
            }),
        );
        Match(m)
    }
}

// ── Inbox (shared by webhook handler + WatchSource impl) ──────────────────────

/// Process-local FIFO buffer of Alertmanager events.
///
/// The webhook handler in `xiaoguai-api` holds an `Arc<AlertmanagerInbox>` in
/// axum state; on POST `/internal/alerts` it calls [`AlertmanagerInbox::push`].
/// The [`AlertmanagerWebhookSource`] drains it on every `WatchSource::poll`.
#[derive(Debug, Default)]
pub struct AlertmanagerInbox {
    inner: Mutex<Vec<AlertmanagerEvent>>,
}

impl AlertmanagerInbox {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn push(&self, ev: AlertmanagerEvent) {
        self.inner.lock().await.push(ev);
    }

    pub async fn drain(&self) -> Vec<AlertmanagerEvent> {
        std::mem::take(&mut *self.inner.lock().await)
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

// ── WatchSource impl ──────────────────────────────────────────────────────────

/// A `WatchSource` that drains an [`AlertmanagerInbox`] on each poll.
pub struct AlertmanagerWebhookSource {
    inbox: Arc<AlertmanagerInbox>,
}

impl AlertmanagerWebhookSource {
    pub fn new(inbox: Arc<AlertmanagerInbox>) -> Self {
        Self { inbox }
    }

    #[must_use]
    pub fn inbox(&self) -> Arc<AlertmanagerInbox> {
        self.inbox.clone()
    }
}

#[async_trait]
impl WatchSource for AlertmanagerWebhookSource {
    async fn poll(&self) -> Result<Vec<Match>, SourceError> {
        let events = self.inbox.drain().await;
        Ok(events.into_iter().map(|e| e.to_match()).collect())
    }
}

// ── Alertmanager webhook payload parser ───────────────────────────────────────
//
// Wire format: Alertmanager v2 webhook. We only need a subset of the fields.

/// Minimal subset of the Alertmanager webhook payload — the fields we
/// actually need. Unknown fields are ignored by serde default.
#[derive(Debug, Deserialize)]
pub struct AlertmanagerWebhookPayload {
    pub alerts: Vec<RawAlert>,
}

#[derive(Debug, Deserialize)]
pub struct RawAlert {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

/// Convert a webhook payload into the SLO-specific events. Alerts that do not
/// carry the `signal` label (e.g. non-SLO alerts that happen to share the
/// receiver) are silently dropped.
pub fn parse_payload(payload: &AlertmanagerWebhookPayload) -> Vec<AlertmanagerEvent> {
    payload.alerts.iter().filter_map(parse_alert).collect()
}

fn parse_alert(a: &RawAlert) -> Option<AlertmanagerEvent> {
    let signal_raw = a.labels.get("signal")?;
    let signal = match signal_raw.as_str() {
        "latency" => Signal::Latency,
        "traffic" => Signal::Traffic,
        "errors" => Signal::Errors,
        "saturation" => Signal::Saturation,
        _ => return None,
    };

    // burn_rate_pair "1" = fast (1h+5m, 14.4×); anything else = slow (6h, 6×).
    // Also accept an explicit `window` label.
    let window = if let Some(w) = a.labels.get("window") {
        match w.as_str() {
            "fast" => BurnWindow::Fast,
            _ => BurnWindow::Slow,
        }
    } else if a.labels.get("burn_rate_pair").map(String::as_str) == Some("1") {
        BurnWindow::Fast
    } else {
        BurnWindow::Slow
    };

    let surface = a.labels.get("surface").cloned().unwrap_or_default();
    let tenant = a.labels.get("tenant").cloned().filter(|s| !s.is_empty());
    let severity = a
        .labels
        .get("severity")
        .cloned()
        .unwrap_or_else(|| "warning".into());
    let alert_name = a.labels.get("alertname").cloned().unwrap_or_default();
    let status = match a.status.as_str() {
        "resolved" => AlertStatus::Resolved,
        _ => AlertStatus::Firing,
    };

    Some(AlertmanagerEvent {
        signal,
        window,
        surface,
        tenant,
        severity,
        alert_name,
        status,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex as TokioMutex;

    fn raw_alert(labels: &[(&str, &str)], status: &str) -> RawAlert {
        RawAlert {
            status: status.into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn parse_alert_extracts_slo_fields() {
        let a = raw_alert(
            &[
                ("alertname", "ApiLatencySLOBurnRateFast"),
                ("signal", "latency"),
                ("burn_rate_pair", "1"),
                ("surface", "/v1/chat/*"),
                ("severity", "critical"),
                ("team", "platform"),
            ],
            "firing",
        );
        let ev = parse_alert(&a).expect("parsable");
        assert_eq!(ev.signal, Signal::Latency);
        assert_eq!(ev.window, BurnWindow::Fast);
        assert_eq!(ev.surface, "/v1/chat/*");
        assert_eq!(ev.severity, "critical");
        assert_eq!(ev.alert_name, "ApiLatencySLOBurnRateFast");
        assert_eq!(ev.status, AlertStatus::Firing);
        assert!(ev.tenant.is_none());
    }

    #[test]
    fn parse_alert_per_tenant() {
        let a = raw_alert(
            &[
                ("alertname", "SaturationSLOBurnRateFast"),
                ("signal", "saturation"),
                ("burn_rate_pair", "1"),
                ("surface", "tenant_settings.daily_llm_token_budget"),
                ("tenant", "tenant_x"),
                ("severity", "warning"),
            ],
            "firing",
        );
        let ev = parse_alert(&a).expect("parsable");
        assert_eq!(ev.tenant.as_deref(), Some("tenant_x"));
        assert_eq!(ev.signal, Signal::Saturation);
    }

    #[test]
    fn parse_alert_explicit_window_label_wins() {
        let a = raw_alert(
            &[
                ("alertname", "X"),
                ("signal", "errors"),
                ("window", "fast"),
                ("burn_rate_pair", "3"), // would normally map to slow
                ("severity", "warning"),
            ],
            "firing",
        );
        let ev = parse_alert(&a).expect("parsable");
        assert_eq!(ev.window, BurnWindow::Fast);
    }

    #[test]
    fn parse_alert_resolved_status() {
        let a = raw_alert(&[("signal", "latency")], "resolved");
        let ev = parse_alert(&a).expect("parsable");
        assert_eq!(ev.status, AlertStatus::Resolved);
    }

    #[test]
    fn parse_alert_drops_non_slo() {
        // No `signal` label = not an SLO alert.
        let a = raw_alert(&[("alertname", "PodCrashLooping")], "firing");
        assert!(parse_alert(&a).is_none());
    }

    #[test]
    fn parse_alert_drops_unknown_signal() {
        let a = raw_alert(&[("signal", "magic")], "firing");
        assert!(parse_alert(&a).is_none());
    }

    #[test]
    fn parse_payload_filters_mixed_alerts() {
        let payload = AlertmanagerWebhookPayload {
            alerts: vec![
                raw_alert(&[("signal", "latency"), ("burn_rate_pair", "1")], "firing"),
                raw_alert(&[("alertname", "PodCrashLooping")], "firing"),
                raw_alert(&[("signal", "errors"), ("window", "slow")], "firing"),
            ],
        };
        let events = parse_payload(&payload);
        assert_eq!(events.len(), 2, "only SLO alerts kept");
        assert_eq!(events[0].signal, Signal::Latency);
        assert_eq!(events[1].signal, Signal::Errors);
    }

    #[test]
    fn event_to_match_emits_expected_keys() {
        let ev = AlertmanagerEvent {
            signal: Signal::Latency,
            window: BurnWindow::Fast,
            surface: "/v1/chat/*".into(),
            tenant: Some("acme".into()),
            severity: "critical".into(),
            alert_name: "ApiLatencySLOBurnRateFast".into(),
            status: AlertStatus::Firing,
        };
        let m = ev.to_match();
        assert_eq!(m.0.get("kind").unwrap(), &json!("slo_breach"));
        assert_eq!(m.0.get("signal").unwrap(), &json!("latency"));
        assert_eq!(m.0.get("window").unwrap(), &json!("fast"));
        assert_eq!(m.0.get("surface").unwrap(), &json!("/v1/chat/*"));
        assert_eq!(m.0.get("tenant").unwrap(), &json!("acme"));
        assert_eq!(m.0.get("severity").unwrap(), &json!("critical"));
        assert_eq!(m.0.get("status").unwrap(), &json!("firing"));
    }

    #[test]
    fn event_to_match_omits_tenant_when_global() {
        let ev = AlertmanagerEvent {
            signal: Signal::Errors,
            window: BurnWindow::Slow,
            surface: "/v1/chat/*".into(),
            tenant: None,
            severity: "warning".into(),
            alert_name: "X".into(),
            status: AlertStatus::Firing,
        };
        let m = ev.to_match();
        assert!(!m.0.contains_key("tenant"), "tenant key absent for global");
    }

    #[tokio::test]
    async fn inbox_push_and_drain() {
        let inbox = AlertmanagerInbox::new();
        inbox
            .push(AlertmanagerEvent {
                signal: Signal::Latency,
                window: BurnWindow::Fast,
                surface: "/v1/chat/*".into(),
                tenant: None,
                severity: "critical".into(),
                alert_name: "X".into(),
                status: AlertStatus::Firing,
            })
            .await;
        assert_eq!(inbox.len().await, 1);
        let drained = inbox.drain().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(inbox.len().await, 0);
    }

    #[tokio::test]
    async fn webhook_source_returns_matches_then_empties() {
        let inbox = AlertmanagerInbox::new();
        for s in [Signal::Latency, Signal::Errors] {
            inbox
                .push(AlertmanagerEvent {
                    signal: s,
                    window: BurnWindow::Fast,
                    surface: "/v1/chat/*".into(),
                    tenant: None,
                    severity: "critical".into(),
                    alert_name: "X".into(),
                    status: AlertStatus::Firing,
                })
                .await;
        }
        let source = AlertmanagerWebhookSource::new(inbox.clone());
        let matches = source.poll().await.expect("poll succeeds");
        assert_eq!(matches.len(), 2);
        let again = source.poll().await.expect("poll succeeds");
        assert_eq!(again.len(), 0, "drained inbox is empty on next poll");
    }

    // Suppress unused-import warning on platforms where the test framework
    // doesn't need the explicit mutex import.
    #[allow(dead_code)]
    fn _typecheck_mutex() {
        let _: TokioMutex<()> = TokioMutex::new(());
    }
}
