//! Feishu push sink — reuses the v0.7.1 `FeishuClient` + `TokenCache`.
//!
//! The scheduler hands every push through one call into the
//! `xiaoguai-im-feishu` adapter's `send_text_message` so the
//! `tenant_access_token` cache lives in one place. v0.10.3
//! deliberately does NOT duplicate the token-fetch path — that code
//! is already tested in `xiaoguai-im-feishu::api::tests` and any
//! divergence would create a second source of truth for "what is a
//! Feishu token".
//!
//! Production wiring (deferred to v0.12.0 alongside the operator
//! binary):
//!
//! ```ignore
//! let cfg: FeishuSinkConfig = settings.scheduler.sinks.feishu.unwrap();
//! let client: Arc<dyn FeishuClient> = Arc::new(HttpFeishuClient::new()?);
//! let sink = FeishuPushSink::new("feishu:ops", client, cfg);
//! let runner = JobRunner::new(...).with_sink(Arc::new(sink));
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use xiaoguai_im_feishu::{FeishuClient, TokenCache};
use xiaoguai_im_gateway::ProviderError;

use crate::sink::{PushPayload, PushSink, SinkError};

/// Per-instance config. Each job's `sinks` field references this sink
/// by [`PushSink::id`] (e.g. `"feishu:ops-chat"`); the chat id below
/// is the *destination* the sink sends to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuSinkConfig {
    pub app_id: String,
    pub app_secret: String,
    /// Feishu `chat_id` (a string like `"oc_xxxxxxxxx"`). All pushes
    /// through this sink instance go to this chat — to fan out to
    /// multiple chats wire one sink per destination.
    pub chat_id: String,
}

pub struct FeishuPushSink {
    id: String,
    client: Arc<dyn FeishuClient>,
    chat_id: String,
    tokens: TokenCache,
}

impl FeishuPushSink {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        client: Arc<dyn FeishuClient>,
        cfg: FeishuSinkConfig,
    ) -> Self {
        let tokens = TokenCache::new(client.clone(), cfg.app_id, cfg.app_secret);
        Self {
            id: id.into(),
            client,
            chat_id: cfg.chat_id,
            tokens,
        }
    }

    /// Render the payload as the body Feishu actually receives. Public
    /// so tests can assert on the exact wire shape without touching
    /// the network.
    #[must_use]
    pub fn render_text(payload: &PushPayload) -> String {
        use std::fmt::Write as _;
        let mut buf = String::new();
        if payload.is_proactive && !payload.reason.is_empty() {
            buf.push_str("【主动推送】");
            buf.push_str(&payload.reason);
            buf.push_str("\n\n");
        }
        let _ = write!(
            buf,
            "Job {} #{} [{}]",
            payload.job_id, payload.run_id, payload.status
        );
        if let Some(out) = &payload.output_preview {
            buf.push_str("\n\n");
            buf.push_str(out);
        }
        if let Some(err) = &payload.error_message {
            buf.push_str("\n\nError: ");
            buf.push_str(err);
        }
        buf
    }
}

impl std::fmt::Debug for FeishuPushSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuPushSink")
            .field("id", &self.id)
            .field("chat_id", &self.chat_id)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PushSink for FeishuPushSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError> {
        payload.require_reason_when_proactive()?;
        let token = self
            .tokens
            .get_token()
            .await
            .map_err(|e: ProviderError| SinkError::Delivery(e.to_string()))?;
        let text = Self::render_text(payload);
        self.client
            .send_text_message(&token, &self.chat_id, &text)
            .await
            .map_err(|e: ProviderError| SinkError::Delivery(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use parking_lot::Mutex as SyncMutex;
    use serde_json::json;
    use xiaoguai_im_feishu::TokenResponse;

    #[derive(Default)]
    struct RecordingClient {
        sends: SyncMutex<Vec<(String, String, String)>>,
        token_calls: SyncMutex<u32>,
    }

    #[async_trait]
    impl FeishuClient for RecordingClient {
        async fn fetch_tenant_access_token(
            &self,
            _app_id: &str,
            _app_secret: &str,
        ) -> Result<TokenResponse, ProviderError> {
            *self.token_calls.lock() += 1;
            Ok(TokenResponse {
                token: "tok_feishu".into(),
                expire_in_secs: 7200,
            })
        }

        async fn send_text_message(
            &self,
            token: &str,
            chat_id: &str,
            text: &str,
        ) -> Result<serde_json::Value, ProviderError> {
            self.sends
                .lock()
                .push((token.to_string(), chat_id.to_string(), text.to_string()));
            Ok(json!({"code": 0}))
        }
    }

    fn cfg() -> FeishuSinkConfig {
        FeishuSinkConfig {
            app_id: "cli_app".into(),
            app_secret: "secret".into(),
            chat_id: "oc_room".into(),
        }
    }

    fn payload(is_proactive: bool, reason: &str) -> PushPayload {
        PushPayload {
            job_id: "j1".into(),
            run_id: 7,
            status: "succeeded".into(),
            fired_at: Utc::now(),
            output_preview: Some("the briefing body".into()),
            error_message: None,
            reason: reason.into(),
            is_proactive,
        }
    }

    #[tokio::test]
    async fn proactive_with_empty_reason_is_refused_before_any_call() {
        let client: Arc<RecordingClient> = Arc::new(RecordingClient::default());
        let sink =
            FeishuPushSink::new("feishu:ops", client.clone() as Arc<dyn FeishuClient>, cfg());
        let err = sink.deliver(&payload(true, "")).await.unwrap_err();
        assert!(matches!(err, SinkError::Invalid(_)));
        assert!(client.sends.lock().is_empty(), "no outbound call attempted");
        assert_eq!(*client.token_calls.lock(), 0, "no token fetched");
    }

    #[tokio::test]
    async fn scheduled_payload_delivers_without_reason() {
        let client: Arc<RecordingClient> = Arc::new(RecordingClient::default());
        let sink =
            FeishuPushSink::new("feishu:ops", client.clone() as Arc<dyn FeishuClient>, cfg());
        sink.deliver(&payload(false, "")).await.unwrap();
        let sends = client.sends.lock();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0, "tok_feishu");
        assert_eq!(sends[0].1, "oc_room");
        assert!(sends[0].2.contains("Job j1 #7"));
        assert!(
            !sends[0].2.contains("主动推送"),
            "scheduled jobs never get the proactive prefix"
        );
    }

    #[tokio::test]
    async fn proactive_with_reason_renders_prefix_and_calls_through() {
        let client: Arc<RecordingClient> = Arc::new(RecordingClient::default());
        let sink =
            FeishuPushSink::new("feishu:ops", client.clone() as Arc<dyn FeishuClient>, cfg());
        sink.deliver(&payload(true, "new GitHub mention"))
            .await
            .unwrap();
        let sends = client.sends.lock();
        assert_eq!(sends.len(), 1);
        let body = &sends[0].2;
        assert!(body.contains("【主动推送】new GitHub mention"));
        assert!(body.contains("the briefing body"));
    }

    #[tokio::test]
    async fn token_is_cached_across_pushes() {
        let client: Arc<RecordingClient> = Arc::new(RecordingClient::default());
        let sink =
            FeishuPushSink::new("feishu:ops", client.clone() as Arc<dyn FeishuClient>, cfg());
        sink.deliver(&payload(false, "")).await.unwrap();
        sink.deliver(&payload(false, "")).await.unwrap();
        assert_eq!(client.sends.lock().len(), 2);
        assert_eq!(
            *client.token_calls.lock(),
            1,
            "TokenCache reused across deliveries"
        );
    }
}
