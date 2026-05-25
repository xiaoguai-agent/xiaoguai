//! Telegram Bot API types used across webhook, long-poll and outbound modules.
//!
//! Only the fields consumed by this crate are declared; the rest arrive as
//! ignored JSON keys (`serde` default: skip unknown fields).

use serde::{Deserialize, Serialize};

/// A Telegram `Update` object. One update = one event delivered to the bot.
///
/// Reference: <https://core.telegram.org/bots/api#update>
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Update {
    /// Global monotonically-increasing update identifier.
    pub update_id: i64,

    /// Set when a new message arrived (text, commands, …).
    #[serde(default)]
    pub message: Option<Message>,

    /// Set when the user edited a previously-sent message.
    #[serde(default)]
    pub edited_message: Option<Message>,

    /// Set when an inline keyboard button was pressed.
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

/// A Telegram `Message`. We extract the fields relevant for routing.
///
/// Reference: <https://core.telegram.org/bots/api#message>
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Message {
    /// Unique message identifier within the chat.
    pub message_id: i64,
    /// Sender (absent for channel posts).
    #[serde(default)]
    pub from: Option<User>,
    /// The chat where the message was sent.
    pub chat: Chat,
    /// Text content (present for text messages; absent for media).
    #[serde(default)]
    pub text: Option<String>,
}

/// A Telegram `User`.
///
/// Reference: <https://core.telegram.org/bots/api#user>
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    pub first_name: String,
}

/// A Telegram `Chat`.
///
/// Reference: <https://core.telegram.org/bots/api#chat>
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Chat {
    pub id: i64,
    /// "private" | "group" | "supergroup" | "channel"
    #[serde(rename = "type")]
    pub chat_type: String,
}

/// A Telegram `CallbackQuery` — fired when an inline button is pressed.
///
/// Reference: <https://core.telegram.org/bots/api#callbackquery>
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CallbackQuery {
    /// Unique identifier for the query.
    pub id: String,
    /// Sender of the callback.
    pub from: User,
    /// Data associated with the pressed button (if set).
    #[serde(default)]
    pub data: Option<String>,
    /// Message from which the query originated (if available).
    #[serde(default)]
    pub message: Option<Message>,
}

/// Bot API response envelope: `{"ok": true, "result": <T>}`.
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub ok: bool,
    #[serde(default)]
    pub result: Option<T>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `getUpdates` result is an array of updates.
pub type GetUpdatesResult = ApiResponse<Vec<Update>>;
