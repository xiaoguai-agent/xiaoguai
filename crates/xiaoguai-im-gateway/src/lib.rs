//! IM gateway library — webhook entry points + provider abstraction.
//!
//! v0.7 ships the common `ImProvider` trait and the Feishu mount. Each
//! provider implementation lives in its own crate; this crate exposes a
//! thin `mount(state, provider)` helper that adds the right routes onto
//! an existing axum router so operators can compose multiple IM channels
//! into one binary.
//!
//! v0.7.2 added an in-process `ConversationHistory` so the gateway can
//! reply across multiple webhook deliveries on the same chat. v0.7.3
//! generalises that into the [`ImHistoryStore`] trait, with two impls:
//!
//! * [`ConversationHistory`] (in-process, single-replica) — default.
//! * [`SqliteImHistoryStore`] — durable, multi-replica. Maps external IM IDs
//!   to internal tenant/user/session rows via the `im_identities` /
//!   `im_conversations` tables.

#![forbid(unsafe_code)]

pub mod history;
pub mod pg_history;
pub mod provider;
pub mod router;

pub use history::{ConversationHistory, ConversationIdent, HistoryError, ImHistoryStore};
pub use pg_history::SqliteImHistoryStore;
pub use provider::{ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook};
pub use router::{
    mount_dingtalk, mount_dingtalk_with_history, mount_feishu, mount_feishu_with_history,
    mount_wecom, mount_wecom_with_history, run_agent_and_reply, GatewayState,
    DEFAULT_HISTORY_TURNS,
};
