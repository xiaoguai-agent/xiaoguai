//! v1.1.2 — production impl of [`xiaoguai_api::SessionForker`].
//!
//! Wraps `SqliteSessionRepository::fork`, which does the atomic three-step
//! (verify cutoff → insert child → copy prefix) under one Pg transaction.
//! The bridge translates `RepoError` into `SessionForkError` so the api
//! layer can map cleanly onto HTTP status codes.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use xiaoguai_api::{SessionForkError, SessionForker};
use xiaoguai_storage::repositories::{RepoError, SessionRepository};
use xiaoguai_types::{Session, SessionId, SessionStatus, UserId};

pub struct SqliteSessionForker {
    sessions: Arc<dyn SessionRepository>,
}

impl SqliteSessionForker {
    #[must_use]
    pub fn new(sessions: Arc<dyn SessionRepository>) -> Self {
        Self { sessions }
    }

    #[must_use]
    pub fn arc(sessions: Arc<dyn SessionRepository>) -> Arc<dyn SessionForker> {
        Arc::new(Self::new(sessions))
    }
}

#[async_trait]
impl SessionForker for SqliteSessionForker {
    async fn fork(
        &self,
        parent_id: &str,
        from_message_id: &str,
        title: Option<String>,
    ) -> Result<Session, SessionForkError> {
        // Load the parent so we can copy model/user/title-default into
        // the child and confirm active status before paying for the copy.
        let parent = self
            .sessions
            .find_by_id(parent_id)
            .await
            .map_err(repo_err)?
            .ok_or(SessionForkError::ParentNotFound)?;

        if !matches!(parent.status, SessionStatus::Active) {
            return Err(SessionForkError::ParentNotForkable(format!(
                "{:?}",
                parent.status
            )));
        }

        let now = Utc::now();
        let new_title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| derive_fork_title(parent.title.as_deref()));

        let new_session = Session {
            id: SessionId::new(),
            user_id: UserId::from(parent.user_id.as_str().to_string()),
            title: new_title,
            created_at: now,
            updated_at: now,
            model: parent.model.clone(),
            status: SessionStatus::Active,
            parent_session_id: Some(parent.id.clone()),
            forked_from_message_id: Some(xiaoguai_types::MessageId::from(
                from_message_id.to_string(),
            )),
            // Feature ⑤: a forked session inherits the parent's coding
            // workspace so a branched conversation keeps the same directory.
            working_dir: parent.working_dir.clone(),
        };

        self.sessions
            .fork(parent_id, from_message_id, &new_session)
            .await
            .map_err(repo_err)?;
        Ok(new_session)
    }
}

fn derive_fork_title(parent_title: Option<&str>) -> Option<String> {
    parent_title.map(|t| {
        if t.starts_with("Fork: ") {
            t.to_string()
        } else {
            format!("Fork: {t}")
        }
    })
}

fn repo_err(err: RepoError) -> SessionForkError {
    match err {
        RepoError::NotFound => SessionForkError::MessageNotFound,
        RepoError::InvalidArgument(s) => SessionForkError::InvalidArgument(s),
        RepoError::Unsupported(s) => SessionForkError::Repository(s),
        other => SessionForkError::Repository(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use xiaoguai_storage::repositories::{RepoResult, SessionRepository};
    use xiaoguai_types::{Message, MessageId, MessageRole, Session};

    /// In-memory session repo that mimics the Pg fork semantics —
    /// copies messages with `created_at <= cutoff.created_at` from
    /// parent to child under one mutex. Co-owns a message store so
    /// the copy is observable.
    #[derive(Default)]
    struct ForkableRepo {
        sessions: Mutex<Vec<Session>>,
        messages: Mutex<Vec<Message>>,
    }

    impl ForkableRepo {
        fn seed_session(&self, s: Session) {
            self.sessions.lock().push(s);
        }
        fn seed_message(&self, m: Message) {
            self.messages.lock().push(m);
        }
        fn messages_for(&self, id: &str) -> Vec<Message> {
            self.messages
                .lock()
                .iter()
                .filter(|m| m.session_id.as_str() == id)
                .cloned()
                .collect()
        }
    }

    #[async_trait]
    impl SessionRepository for ForkableRepo {
        async fn create(&self, s: &Session) -> RepoResult<()> {
            self.sessions.lock().push(s.clone());
            Ok(())
        }
        async fn find_by_id(&self, id: &str) -> RepoResult<Option<Session>> {
            Ok(self
                .sessions
                .lock()
                .iter()
                .find(|s| s.id.as_str() == id)
                .cloned())
        }
        async fn list_by_user(&self, _u: &str, _l: i64, _o: i64) -> RepoResult<Vec<Session>> {
            Ok(Vec::new())
        }
        async fn touch(&self, _id: &str) -> RepoResult<()> {
            Ok(())
        }
        async fn archive(&self, _id: &str) -> RepoResult<()> {
            Ok(())
        }
        async fn delete(&self, _id: &str) -> RepoResult<()> {
            Ok(())
        }
        async fn fork(
            &self,
            parent_id: &str,
            from_message_id: &str,
            new_session: &Session,
        ) -> RepoResult<()> {
            // Locate cutoff message in parent.
            let cutoff_ts = {
                let g = self.messages.lock();
                g.iter()
                    .find(|m| {
                        m.session_id.as_str() == parent_id && m.id.as_str() == from_message_id
                    })
                    .map(|m| m.created_at)
            };
            let Some(cutoff_ts) = cutoff_ts else {
                return Err(RepoError::NotFound);
            };
            // Insert session row.
            self.sessions.lock().push(new_session.clone());
            // Copy prefix messages.
            let mut to_copy: Vec<Message> = self
                .messages
                .lock()
                .iter()
                .filter(|m| m.session_id.as_str() == parent_id && m.created_at <= cutoff_ts)
                .cloned()
                .collect();
            to_copy.sort_by_key(|m| m.created_at);
            let mut sink = self.messages.lock();
            for (i, m) in to_copy.into_iter().enumerate() {
                sink.push(Message {
                    id: MessageId::from(format!("msg_fk_{}_{}", new_session.id.as_str(), i)),
                    session_id: new_session.id.clone(),
                    role: m.role,
                    content: m.content,
                    created_at: m.created_at,
                });
            }
            Ok(())
        }
    }

    fn mk_session(id: &str) -> Session {
        let now = Utc::now();
        Session {
            id: SessionId::from(id.to_string()),
            user_id: UserId::from("u".to_string()),
            title: Some("Hello".into()),
            created_at: now,
            updated_at: now,
            model: "m".into(),
            status: SessionStatus::Active,
            parent_session_id: None,
            forked_from_message_id: None,
            working_dir: None,
        }
    }

    fn mk_message(id: &str, session_id: &str, offset_secs: i64, text: &str) -> Message {
        let base = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        Message {
            id: MessageId::from(id.to_string()),
            session_id: SessionId::from(session_id.to_string()),
            role: MessageRole::User,
            content: vec![xiaoguai_types::ContentBlock::Text {
                text: text.to_string(),
            }],
            created_at: base + chrono::Duration::seconds(offset_secs),
        }
    }

    #[tokio::test]
    async fn fork_copies_messages_up_to_cutoff() {
        let repo = Arc::new(ForkableRepo::default());
        repo.seed_session(mk_session("s1"));
        for i in 0..5 {
            repo.seed_message(mk_message(
                &format!("m{i}"),
                "s1",
                i64::from(i) * 60,
                &format!("turn-{i}"),
            ));
        }
        let forker = SqliteSessionForker::new(repo.clone());
        let new_session = forker.fork("s1", "m2", None).await.unwrap();

        // Parent untouched (5 messages).
        assert_eq!(repo.messages_for("s1").len(), 5);
        // New session: messages with created_at <= cutoff (m2 inclusive) → 3 messages.
        let child = repo.messages_for(new_session.id.as_str());
        assert_eq!(child.len(), 3);
        assert_eq!(child[0].content.len(), 1);
        // Lineage fields populated.
        assert_eq!(
            new_session
                .parent_session_id
                .as_ref()
                .map(SessionId::as_str),
            Some("s1")
        );
        assert_eq!(
            new_session
                .forked_from_message_id
                .as_ref()
                .map(MessageId::as_str),
            Some("m2")
        );
        // Title auto-derived from parent.
        assert_eq!(new_session.title.as_deref(), Some("Fork: Hello"));
    }

    #[tokio::test]
    async fn fork_unknown_message_returns_message_not_found() {
        let repo = Arc::new(ForkableRepo::default());
        repo.seed_session(mk_session("s1"));
        repo.seed_message(mk_message("m0", "s1", 0, "hello"));
        let forker = SqliteSessionForker::new(repo);
        let err = forker.fork("s1", "missing", None).await.unwrap_err();
        assert!(matches!(err, SessionForkError::MessageNotFound), "{err:?}");
    }

    #[tokio::test]
    async fn fork_unknown_parent_returns_parent_not_found() {
        let repo = Arc::new(ForkableRepo::default());
        let forker = SqliteSessionForker::new(repo);
        let err = forker.fork("missing", "m0", None).await.unwrap_err();
        assert!(matches!(err, SessionForkError::ParentNotFound), "{err:?}");
    }

    #[tokio::test]
    async fn fork_archived_session_returns_conflict() {
        let repo = Arc::new(ForkableRepo::default());
        let mut s = mk_session("s1");
        s.status = SessionStatus::Archived;
        repo.seed_session(s);
        repo.seed_message(mk_message("m0", "s1", 0, "hello"));
        let forker = SqliteSessionForker::new(repo);
        let err = forker.fork("s1", "m0", None).await.unwrap_err();
        assert!(
            matches!(err, SessionForkError::ParentNotForkable(_)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn fork_explicit_title_wins() {
        let repo = Arc::new(ForkableRepo::default());
        repo.seed_session(mk_session("s1"));
        repo.seed_message(mk_message("m0", "s1", 0, "x"));
        let forker = SqliteSessionForker::new(repo);
        let s = forker
            .fork("s1", "m0", Some("Custom title".into()))
            .await
            .unwrap();
        assert_eq!(s.title.as_deref(), Some("Custom title"));
    }
}
