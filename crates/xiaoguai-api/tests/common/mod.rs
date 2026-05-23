//! In-memory repository impls + helpers for axum integration tests.
//!
//! The Pg repos in `xiaoguai-storage` are the production target; we
//! deliberately avoid testcontainers in the API suite so the loop-level
//! assertions stay fast and deterministic. The PG-backed e2e tests are
//! their own ignored suite in `xiaoguai-storage`.
//!
//! As of v0.6.1 the repo traits take a `tenant: Option<&str>` argument.
//! The in-memory impls below record the last-observed tenant for the
//! benefit of assertions but otherwise ignore it — RLS is a Postgres
//! concern.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use xiaoguai_storage::repositories::{MessageRepository, RepoError, RepoResult, SessionRepository};
use xiaoguai_types::{Message, Session};

#[derive(Default)]
pub struct InMemorySessionRepo {
    inner: Mutex<HashMap<String, Session>>,
    last_tenant: Mutex<Option<String>>,
}

impl InMemorySessionRepo {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[allow(dead_code)]
    pub fn last_tenant(&self) -> Option<String> {
        self.last_tenant.lock().clone()
    }

    fn record_tenant(&self, tenant: Option<&str>) {
        *self.last_tenant.lock() = tenant.map(str::to_string);
    }
}

#[async_trait]
impl SessionRepository for InMemorySessionRepo {
    async fn create(&self, tenant: Option<&str>, session: &Session) -> RepoResult<()> {
        self.record_tenant(tenant);
        let mut g = self.inner.lock();
        if g.contains_key(session.id.as_str()) {
            return Err(RepoError::DuplicateKey("duplicate session id".into()));
        }
        g.insert(session.id.to_string(), session.clone());
        Ok(())
    }

    async fn find_by_id(&self, tenant: Option<&str>, id: &str) -> RepoResult<Option<Session>> {
        self.record_tenant(tenant);
        Ok(self.inner.lock().get(id).cloned())
    }

    async fn list_by_user(
        &self,
        tenant: Option<&str>,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>> {
        self.record_tenant(tenant);
        let mut rows: Vec<Session> = self
            .inner
            .lock()
            .values()
            .filter(|s| s.user_id.as_str() == user_id)
            .cloned()
            .collect();
        rows.sort_by_key(|s| s.created_at);
        let offset = usize::try_from(offset.max(0)).unwrap_or(0);
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    async fn touch(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        self.record_tenant(tenant);
        let mut g = self.inner.lock();
        if let Some(s) = g.get_mut(id) {
            s.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn archive(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        self.record_tenant(tenant);
        let mut g = self.inner.lock();
        if let Some(s) = g.get_mut(id) {
            s.status = xiaoguai_types::SessionStatus::Archived;
        }
        Ok(())
    }

    async fn delete(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        self.record_tenant(tenant);
        self.inner.lock().remove(id);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryMessageRepo {
    inner: Mutex<HashMap<String, Vec<Message>>>,
    last_tenant: Mutex<Option<String>>,
}

impl InMemoryMessageRepo {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[allow(dead_code)]
    pub fn snapshot(&self, session_id: &str) -> Vec<Message> {
        self.inner
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn last_tenant(&self) -> Option<String> {
        self.last_tenant.lock().clone()
    }

    fn record_tenant(&self, tenant: Option<&str>) {
        *self.last_tenant.lock() = tenant.map(str::to_string);
    }
}

#[async_trait]
impl MessageRepository for InMemoryMessageRepo {
    async fn append(&self, tenant: Option<&str>, message: &Message) -> RepoResult<()> {
        self.record_tenant(tenant);
        self.inner
            .lock()
            .entry(message.session_id.to_string())
            .or_default()
            .push(message.clone());
        Ok(())
    }

    async fn list_by_session(
        &self,
        tenant: Option<&str>,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Message>> {
        self.record_tenant(tenant);
        let rows = self
            .inner
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let offset = usize::try_from(offset.max(0)).unwrap_or(0);
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    async fn count_by_session(&self, tenant: Option<&str>, session_id: &str) -> RepoResult<i64> {
        self.record_tenant(tenant);
        Ok(self
            .inner
            .lock()
            .get(session_id)
            .map_or(0, |v| i64::try_from(v.len()).unwrap_or(i64::MAX)))
    }

    async fn delete_by_session(&self, tenant: Option<&str>, session_id: &str) -> RepoResult<u64> {
        self.record_tenant(tenant);
        let removed = self
            .inner
            .lock()
            .remove(session_id)
            .map_or(0, |v| u64::try_from(v.len()).unwrap_or(u64::MAX));
        Ok(removed)
    }
}
