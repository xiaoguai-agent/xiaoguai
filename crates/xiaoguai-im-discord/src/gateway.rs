//! Discord Gateway WebSocket client (stub — TODO for v1.2+).
//!
//! ## What this will do (planned)
//!
//! The Discord Gateway is an optional WebSocket connection (`wss://gateway.discord.gg`)
//! that the bot uses when it needs to receive events **beyond** Interactions
//! (e.g. presence updates, guild join/leave, reaction add, message edits).
//!
//! For xiaoguai's slash-command use-case the Interactions webhook endpoint
//! (`DiscordProvider`) is sufficient — no Gateway needed.  This stub is
//! here to:
//!
//! 1. Document the planned structure so a future implementer knows where
//!    to start.
//! 2. Reserve the module name so `lib.rs` can re-export the stub type
//!    without a breaking rename later.
//!
//! ## Planned structure
//!
//! ```text
//! GatewayClient::connect(token, intents)
//!   └─ GET /api/v10/gateway/bot  →  wss_url, shards
//!      └─ WebSocket connect (tokio-tungstenite)
//!         ├─ recv OP 10 Hello { heartbeat_interval }
//!         ├─ send OP 2 Identify { token, intents, properties }
//!         ├─ recv OP 0 READY
//!         └─ event loop:
//!              recv OP 0 (dispatch) → emit GatewayEvent
//!              send OP 1 Heartbeat every heartbeat_interval ms
//!              recv OP 11 Heartbeat ACK
//!              handle OP 7 Reconnect, OP 9 Invalid Session
//! ```
//!
//! ## Intents (bit flags)
//!
//! Common intents for a chat bot:
//!
//! | Intent                | Value    | Purpose |
//! |-----------------------|----------|---------|
//! | `GUILDS`              | 1 << 0   | Guild create/delete/update |
//! | `GUILD_MESSAGES`      | 1 << 9   | Message events (privileged) |
//! | `DIRECT_MESSAGES`     | 1 << 12  | DM events |
//! | `MESSAGE_CONTENT`     | 1 << 15  | Access to message content (privileged) |
//!
//! ## Deferred items
//!
//! - Actual WebSocket connection (`tokio-tungstenite` — already in workspace)
//! - Heartbeat task
//! - Reconnect / resume (`OP 6 Resume`) on disconnect
//! - Sharding for large bots (> 2 500 guilds)
//! - Voice gateway (separate WebSocket — out of scope for v1.x)

/// Placeholder for the Discord Gateway client.
///
/// Instantiate with `GatewayClient::new(token, intents)` — currently this
/// panics with a clear TODO message to prevent accidental use.
#[allow(dead_code)]
pub struct GatewayClient {
    token: String,
    intents: u32,
}

impl GatewayClient {
    /// Create a new `GatewayClient`.
    ///
    /// # Panics
    /// Always panics — Gateway WebSocket is not yet implemented.
    /// Tracked for v1.2+.
    #[allow(clippy::new_without_default)]
    pub fn new(token: impl Into<String>, intents: u32) -> Self {
        let _ = (token, intents);
        unimplemented!(
            "Discord Gateway WebSocket is not yet implemented. \
             Use DiscordProvider (Interactions webhook) for slash-command bots. \
             Gateway support is tracked for v1.2+."
        )
    }
}

/// Common Discord Gateway intent bit flags.
///
/// Combine with `|` to request multiple intents.
pub mod intents {
    pub const GUILDS: u32 = 1 << 0;
    /// Guild message events (privileged).
    pub const GUILD_MESSAGES: u32 = 1 << 9;
    /// Direct message events.
    pub const DIRECT_MESSAGES: u32 = 1 << 12;
    /// Access to message content (privileged — must be enabled in the Discord Developer Portal).
    pub const MESSAGE_CONTENT: u32 = 1 << 15;
}
