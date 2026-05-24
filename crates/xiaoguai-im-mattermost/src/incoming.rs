//! Mattermost outgoing webhook handler.
//!
//! Mattermost delivers outgoing webhooks as `application/x-www-form-urlencoded`
//! POST bodies with at minimum `token`, `channel_id`, `user_name`, and `text`.
//!
//! We verify the `token` field against the configured `webhook_token` using a
//! constant-time comparison before touching any other field so a mis-configured
//! or malicious peer cannot probe the parser behaviour.
//!
//! Full field list: https://developers.mattermost.com/integrate/webhooks/outgoing/
//!
//! Fields we care about (all others are forwarded as-is to the caller):
//!
//! | Field        | Description                              |
//! |--------------|------------------------------------------|
//! | `token`      | Shared secret set in Mattermost config   |
//! | `channel_id` | Channel identifier (e.g. `abc12345def`)  |
//! | `user_name`  | Poster's login name                      |
//! | `text`       | Message body that triggered the webhook  |
//! | `post_id`    | Original post id — used as `event_id`    |
//! | `team_id`    | Team identifier — used as `tenant_id`    |

use serde::Deserialize;

use xiaoguai_im_gateway::{ImEvent, IncomingMessage, ProviderError, Webhook};

use crate::constant_time_eq;

/// Form-encoded body of a Mattermost outgoing webhook.
#[derive(Debug, Deserialize)]
pub struct OutgoingWebhookPayload {
    /// Shared verification token set in the Mattermost integration config.
    pub token: String,
    pub channel_id: String,
    pub user_name: String,
    pub text: String,
    #[serde(default)]
    pub post_id: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
}

/// Parse + verify a Mattermost outgoing webhook request.
///
/// Verification is done against `webhook_token`.  Returns
/// [`ProviderError::BadSignature`] if the token is missing or does not
/// match — this makes the error indistinguishable from a missing field so
/// callers cannot distinguish the two failure modes.
///
/// # Errors
///
/// * [`ProviderError::BadSignature`] — token absent or mismatch.
/// * [`ProviderError::Malformed`] — form body cannot be decoded or a
///   required field is missing.
pub fn parse(webhook: &Webhook, webhook_token: &str) -> Result<ImEvent, ProviderError> {
    let payload: OutgoingWebhookPayload = serde_urlencoded::from_bytes(&webhook.body)
        .map_err(|e| ProviderError::Malformed(format!("outgoing webhook form decode: {e}")))?;

    // Constant-time check so partial-match timing cannot be observed.
    if !constant_time_eq(payload.token.as_bytes(), webhook_token.as_bytes()) {
        return Err(ProviderError::BadSignature);
    }

    let event_id = payload.post_id.unwrap_or_else(|| {
        // Stable synthetic id from channel + user when post_id is absent.
        format!("{}-{}", payload.channel_id, payload.user_name)
    });
    let tenant_external_id = payload.team_id.unwrap_or_default();

    Ok(ImEvent::Message(IncomingMessage {
        provider: "mattermost".into(),
        user_external_id: payload.user_name,
        tenant_external_id,
        conversation_id: payload.channel_id,
        text: payload.text,
        event_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_webhook(body: &str) -> Webhook {
        Webhook {
            headers: vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: body.as_bytes().to_vec(),
        }
    }

    const TOKEN: &str = "mm_secret";

    #[test]
    fn happy_path_parses_all_fields() {
        let body = format!(
            "token={TOKEN}&channel_id=ch1&user_name=alice&text=hello+world\
             &post_id=p123&team_id=team_xyz"
        );
        let webhook = make_webhook(&body);
        let event = parse(&webhook, TOKEN).expect("should succeed");

        match event {
            ImEvent::Message(m) => {
                assert_eq!(m.provider, "mattermost");
                assert_eq!(m.user_external_id, "alice");
                assert_eq!(m.conversation_id, "ch1");
                assert_eq!(m.text, "hello world");
                assert_eq!(m.event_id, "p123");
                assert_eq!(m.tenant_external_id, "team_xyz");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_post_id_synthesises_event_id() {
        let body = format!("token={TOKEN}&channel_id=ch2&user_name=bob&text=hi&team_id=team_abc");
        let webhook = make_webhook(&body);
        let event = parse(&webhook, TOKEN).expect("should succeed");
        match event {
            ImEvent::Message(m) => {
                assert_eq!(m.event_id, "ch2-bob");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn wrong_token_returns_bad_signature() {
        let body = "token=wrong_token&channel_id=c&user_name=u&text=t";
        let webhook = make_webhook(body);
        assert!(matches!(
            parse(&webhook, TOKEN),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn missing_token_field_returns_malformed() {
        // No `token` key at all — serde_urlencoded requires it since the field
        // is not Option<>, so we get Malformed (not BadSignature).
        let body = "channel_id=c&user_name=u&text=t";
        let webhook = make_webhook(body);
        assert!(matches!(
            parse(&webhook, TOKEN),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn empty_body_returns_malformed() {
        let webhook = make_webhook("");
        assert!(matches!(
            parse(&webhook, TOKEN),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn text_with_plus_encoding_is_decoded() {
        let body = format!("token={TOKEN}&channel_id=c&user_name=u&text=hello+there");
        let webhook = make_webhook(&body);
        match parse(&webhook, TOKEN).expect("ok") {
            ImEvent::Message(m) => assert_eq!(m.text, "hello there"),
            _ => panic!(),
        }
    }
}
