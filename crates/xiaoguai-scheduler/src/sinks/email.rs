//! Email push sink — JSON webhook POST.
//!
//! v0.10.3 deliberately does NOT speak SMTP. The audience for this
//! sink is operators who already run an email relay (Postmark /
//! Mailgun / a self-hosted SMTP shim) — adding `lettre` would mean
//! shipping a 200KB dependency chain to solve a problem that
//! "JSON-POST to a relay" already solves. The relay does the
//! envelope-shape work, transient retry, DKIM/SPF, bounce handling —
//! none of which we want re-implemented in the scheduler crate.
//!
//! Wire shape:
//!
//! ```text
//! POST <webhook_url>
//! { "to": "<to>",
//!   "from": "<from>",
//!   "subject": "...",
//!   "body": "...",
//!   "payload": { ...the original PushPayload... } }
//! ```
//!
//! The full `payload` is included so a smart relay can build richer
//! HTML if it wants to; the explicit `subject` and `body` fields keep
//! the dumb-relay path trivial.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::sink::{PushPayload, PushSink, SinkError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailSinkConfig {
    /// Endpoint the JSON payload is `POST`ed to.
    pub webhook_url: String,
    /// Recipient address. v0.10.3 ships single-recipient sinks; fan
    /// out via multiple sink instances.
    pub to: String,
    /// `From` address presented to the relay. Required by most
    /// relays for DMARC alignment.
    pub from: String,
}

pub struct EmailPushSink {
    id: String,
    client: reqwest::Client,
    cfg: EmailSinkConfig,
}

impl EmailPushSink {
    /// # Errors
    /// Bubbles up the reqwest builder error.
    pub fn new(id: impl Into<String>, cfg: EmailSinkConfig) -> Result<Self, SinkError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| SinkError::Delivery(format!("build reqwest: {e}")))?;
        Ok(Self {
            id: id.into(),
            client,
            cfg,
        })
    }

    /// Public so tests can assert on the rendered body. Mirrors the
    /// `render_text` helper on the other sinks.
    #[must_use]
    pub fn render_subject(payload: &PushPayload) -> String {
        if payload.is_proactive && !payload.reason.is_empty() {
            format!("[xiaoguai] {} — {}", payload.job_id, payload.reason)
        } else {
            format!(
                "[xiaoguai] {} #{} {}",
                payload.job_id, payload.run_id, payload.status
            )
        }
    }

    #[must_use]
    pub fn render_body(payload: &PushPayload) -> String {
        use std::fmt::Write as _;
        let mut buf = String::new();
        if payload.is_proactive && !payload.reason.is_empty() {
            buf.push_str("Reason: ");
            buf.push_str(&payload.reason);
            buf.push_str("\n\n");
        }
        let _ = writeln!(buf, "Job: {}", payload.job_id);
        let _ = writeln!(buf, "Run: #{}", payload.run_id);
        let _ = writeln!(buf, "Status: {}", payload.status);
        if let Some(out) = &payload.output_preview {
            buf.push('\n');
            buf.push_str(out);
            buf.push('\n');
        }
        if let Some(err) = &payload.error_message {
            buf.push_str("\nError: ");
            buf.push_str(err);
            buf.push('\n');
        }
        buf
    }
}

impl std::fmt::Debug for EmailPushSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailPushSink")
            .field("id", &self.id)
            .field("to", &self.cfg.to)
            .field("webhook_url", &self.cfg.webhook_url)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PushSink for EmailPushSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError> {
        payload.require_reason_when_proactive()?;
        let body = serde_json::json!({
            "to": self.cfg.to,
            "from": self.cfg.from,
            "subject": Self::render_subject(payload),
            "body": Self::render_body(payload),
            "payload": payload,
        });
        let resp = self
            .client
            .post(&self.cfg.webhook_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SinkError::Delivery(format!("send: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SinkError::Delivery(format!(
                "email webhook http {status}: {body}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn payload(is_proactive: bool, reason: &str) -> PushPayload {
        PushPayload {
            job_id: "weekly-digest".into(),
            run_id: 42,
            tenant_id: Some("t1".into()),
            status: "succeeded".into(),
            fired_at: Utc::now(),
            output_preview: Some("Top 3 stories...".into()),
            error_message: None,
            reason: reason.into(),
            is_proactive,
        }
    }

    fn cfg(url: String) -> EmailSinkConfig {
        EmailSinkConfig {
            webhook_url: url,
            to: "ops@example.com".into(),
            from: "xiaoguai@example.com".into(),
        }
    }

    #[tokio::test]
    async fn proactive_without_reason_is_refused_no_http() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let sink = EmailPushSink::new("email", cfg(format!("{}/relay", server.url()))).unwrap();
        let err = sink.deliver(&payload(true, "")).await.unwrap_err();
        assert!(matches!(err, SinkError::Invalid(_)));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn scheduled_payload_posts_to_webhook_url() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/relay")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "to": "ops@example.com",
                "from": "xiaoguai@example.com",
                "subject": "[xiaoguai] weekly-digest #42 succeeded"
            })))
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;
        let sink = EmailPushSink::new("email", cfg(format!("{}/relay", server.url()))).unwrap();
        sink.deliver(&payload(false, "")).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn proactive_with_reason_subject_includes_reason() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/relay")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "subject": "[xiaoguai] weekly-digest — fresh stories"
            })))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let sink = EmailPushSink::new("email", cfg(format!("{}/relay", server.url()))).unwrap();
        sink.deliver(&payload(true, "fresh stories")).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn webhook_5xx_propagates_as_delivery() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/relay")
            .with_status(500)
            .with_body("boom")
            .create_async()
            .await;
        let sink = EmailPushSink::new("email", cfg(format!("{}/relay", server.url()))).unwrap();
        let err = sink.deliver(&payload(false, "")).await.unwrap_err();
        assert!(matches!(err, SinkError::Delivery(msg) if msg.contains("500")));
    }
}
