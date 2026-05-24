//! v1.1.2 — conversation fork.
//!
//! Same trait-in-api / impl-in-core pattern the v0.12.x shims use
//! (see [`crate::scheduler`] for the canonical example): the fork
//! operation needs both `SessionRepository` and `MessageRepository`
//! to coordinate atomically. Rather than have the api crate know
//! about the production impl's transaction handling, we expose a
//! narrow [`SessionForker`] trait here. `xiaoguai-core` wires
//! `PgSessionForker` (in `sessions_bridge.rs`) which delegates to the
//! Pg repos' single-tx `fork` method.

use async_trait::async_trait;
use thiserror::Error;
use xiaoguai_types::Session;

#[derive(Debug, Error)]
pub enum SessionForkError {
    /// The parent session (`:id` in the URL) was not found *for this
    /// tenant*. Maps to 404 — we deliberately don't distinguish
    /// "doesn't exist" from "wrong tenant" so we don't leak
    /// cross-tenant existence.
    #[error("parent session not found")]
    ParentNotFound,
    /// `from_message_id` doesn't belong to the parent session. Maps to
    /// 404 with `code: "fork_message_not_found"`.
    #[error("fork message not found in parent session")]
    MessageNotFound,
    /// The parent session is not in a state that permits forking
    /// (e.g. archived). Maps to 409.
    #[error("parent session is {0}, cannot fork")]
    ParentNotForkable(String),
    /// `title` was empty/whitespace, or the request was otherwise
    /// malformed. Maps to 400.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Repository / DB failure. Maps to 500.
    #[error("repository: {0}")]
    Repository(String),
}

/// Fork an existing session at a given message boundary, returning the
/// newly-created child session. The implementation must:
///
/// - Validate that the parent belongs to `tenant`.
/// - Validate that `from_message_id` belongs to the parent.
/// - Copy every message with `created_at <= cutoff.created_at` from
///   the parent into the new session.
/// - Persist the new session row with `parent_session_id` and
///   `forked_from_message_id` populated.
///
/// All three steps must be atomic — partial copies leave the user
/// staring at a half-broken history. Production impl uses a single
/// Pg transaction; in-memory impls do it under one mutex.
#[async_trait]
pub trait SessionForker: Send + Sync {
    async fn fork(
        &self,
        tenant: &str,
        parent_id: &str,
        from_message_id: &str,
        title: Option<String>,
    ) -> Result<Session, SessionForkError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use xiaoguai_types::{SessionId, SessionStatus, TenantId, UserId};

    /// Static forker that returns a pre-canned session — handy for
    /// route tests that don't care about the copy semantics, only
    /// about wiring.
    pub struct StaticForker {
        pub result: Result<Session, String>,
    }

    #[async_trait]
    impl SessionForker for StaticForker {
        async fn fork(
            &self,
            _tenant: &str,
            _parent_id: &str,
            _from_message_id: &str,
            _title: Option<String>,
        ) -> Result<Session, SessionForkError> {
            self.result.clone().map_err(SessionForkError::Repository)
        }
    }

    fn fresh_session() -> Session {
        let now = Utc::now();
        Session {
            id: SessionId::new(),
            tenant_id: TenantId::from("t".to_string()),
            user_id: UserId::from("u".to_string()),
            title: Some("forked".into()),
            created_at: now,
            updated_at: now,
            model: "m".into(),
            status: SessionStatus::Active,
            parent_session_id: Some(SessionId::from("parent".to_string())),
            forked_from_message_id: Some(xiaoguai_types::MessageId::from("msg".to_string())),
        }
    }

    #[tokio::test]
    async fn static_forker_returns_canned_session() {
        let f: Arc<dyn SessionForker> = Arc::new(StaticForker {
            result: Ok(fresh_session()),
        });
        let s = f
            .fork("t", "parent", "msg", Some("x".into()))
            .await
            .unwrap();
        assert_eq!(
            s.parent_session_id.as_ref().map(SessionId::as_str),
            Some("parent")
        );
    }

    #[tokio::test]
    async fn static_forker_returns_canned_error() {
        let f: Arc<dyn SessionForker> = Arc::new(StaticForker {
            result: Err("boom".into()),
        });
        let err = f.fork("t", "p", "m", None).await.unwrap_err();
        assert!(matches!(err, SessionForkError::Repository(_)));
    }
}
