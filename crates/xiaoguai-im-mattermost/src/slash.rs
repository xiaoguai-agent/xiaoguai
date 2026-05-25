//! Mattermost slash command payload.
//!
//! Slash commands are delivered as `application/x-www-form-urlencoded` POST
//! bodies, just like outgoing webhooks, but the key fields differ:
//!
//! | Field        | Description                                         |
//! |--------------|-----------------------------------------------------|
//! | `token`      | Shared secret configured in the slash command entry |
//! | `command`    | The slash command name, e.g. `/remind`              |
//! | `text`       | Arguments after the command name                    |
//! | `channel_id` | Channel where the command was invoked               |
//! | `user_name`  | Invoking user's login name                          |
//! | `team_id`    | Team identifier                                     |
//! | `response_url` | URL for posting a delayed/ephemeral response      |
//!
//! Full reference: <https://developers.mattermost.com/integrate/slash-commands/>
//!
//! We verify the `token` the same way as outgoing webhooks — constant-time
//! comparison so mismatches are indistinguishable from timing.

use serde::Deserialize;

use xiaoguai_im_gateway::ProviderError;

use crate::constant_time_eq;

/// Parsed Mattermost slash command payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// The slash command (e.g. `/remind`).
    pub command: String,
    /// Arguments that followed the command, possibly empty.
    pub text: String,
    /// Channel where the command was invoked.
    pub channel_id: String,
    /// Login name of the invoking user.
    pub user_name: String,
    /// Team identifier (used as `tenant_external_id`).
    pub team_id: String,
    /// Mattermost-provided URL for posting a delayed reply.
    pub response_url: Option<String>,
}

/// Raw form-encoded fields including the verification token.
#[derive(Deserialize)]
struct RawSlash {
    token: String,
    command: String,
    #[serde(default)]
    text: String,
    channel_id: String,
    user_name: String,
    #[serde(default)]
    team_id: String,
    #[serde(default)]
    response_url: Option<String>,
}

/// Parse + verify a Mattermost slash command HTTP body.
///
/// The `webhook_token` should be the slash command's verification token,
/// configured in the Mattermost integration settings.
///
/// # Errors
///
/// * [`ProviderError::BadSignature`] — token absent or wrong.
/// * [`ProviderError::Malformed`] — body cannot be decoded or a required
///   field is missing.
pub fn parse(body: &[u8], webhook_token: &str) -> Result<SlashCommand, ProviderError> {
    let raw: RawSlash = serde_urlencoded::from_bytes(body)
        .map_err(|e| ProviderError::Malformed(format!("slash command form decode: {e}")))?;

    if !constant_time_eq(raw.token.as_bytes(), webhook_token.as_bytes()) {
        return Err(ProviderError::BadSignature);
    }

    Ok(SlashCommand {
        command: raw.command,
        text: raw.text,
        channel_id: raw.channel_id,
        user_name: raw.user_name,
        team_id: raw.team_id,
        response_url: raw.response_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "slash_token";

    fn body(fields: &str) -> Vec<u8> {
        fields.as_bytes().to_vec()
    }

    #[test]
    fn full_payload_parses_correctly() {
        let raw = format!(
            "token={TOKEN}&command=%2Fremind&text=me+at+noon\
             &channel_id=ch1&user_name=alice&team_id=team_x\
             &response_url=https%3A%2F%2Fmm.example.com%2Fresp"
        );
        let cmd = parse(&body(&raw), TOKEN).expect("should succeed");
        assert_eq!(cmd.command, "/remind");
        assert_eq!(cmd.text, "me at noon");
        assert_eq!(cmd.channel_id, "ch1");
        assert_eq!(cmd.user_name, "alice");
        assert_eq!(cmd.team_id, "team_x");
        assert_eq!(
            cmd.response_url.as_deref(),
            Some("https://mm.example.com/resp")
        );
    }

    #[test]
    fn optional_fields_absent_uses_defaults() {
        let raw = format!("token={TOKEN}&command=%2Fping&channel_id=ch2&user_name=bob");
        let cmd = parse(&body(&raw), TOKEN).expect("should succeed");
        assert_eq!(cmd.command, "/ping");
        assert_eq!(cmd.text, "");
        assert_eq!(cmd.team_id, "");
        assert!(cmd.response_url.is_none());
    }

    #[test]
    fn wrong_token_returns_bad_signature() {
        let raw = "token=bad&command=%2Fping&channel_id=c&user_name=u";
        assert!(matches!(
            parse(&body(raw), TOKEN),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn missing_token_field_returns_malformed() {
        // serde_urlencoded will fail because `token` is not `Option`.
        let raw = "command=%2Fping&channel_id=c&user_name=u";
        assert!(matches!(
            parse(&body(raw), TOKEN),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn missing_command_field_returns_malformed() {
        let raw = format!("token={TOKEN}&channel_id=c&user_name=u");
        assert!(matches!(
            parse(&body(&raw), TOKEN),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn url_encoded_command_is_decoded() {
        // %2F is the slash prefix character.
        let raw = format!("token={TOKEN}&command=%2Fbot+help&channel_id=c&user_name=u");
        let cmd = parse(&body(&raw), TOKEN).expect("ok");
        assert_eq!(cmd.command, "/bot help");
    }
}
