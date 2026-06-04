//! IM identity + conversation mapping repository.
//!
//! v0.7.3 bridges IM-provider external IDs (Feishu `tenant_key` + `open_id`,
//! DingTalk `corpid` + `userid`, …) to internal tenant/user/session IDs.
//!
//! Two tables (see `0006_im_identity.sql`):
//!
//! * `im_identities`    — `(provider, tenant_ext, user_ext)` → `user_id`
//! * `im_conversations` — `(provider, tenant_ext, conv_id)`  → `session_id`
//!
//! The repo exposes two `resolve_or_create_*` helpers. Each is idempotent:
//! the first webhook for a chat creates the user + session rows inside a
//! single transaction; every subsequent webhook hits the PK index and
//! returns the stored IDs. Under the single-user pivot (DEC-033) there is no
//! `tenants` table; every resolved user/session belongs to the one owner.
//!
//! Note: the `tenant_external_id` / `tenant_key` carried by
//! [`ExternalIdentity`] / [`ExternalConversation`] is the IM **platform**
//! tenant (Feishu/Lark `tenant_key`), part of the external addressing key —
//! it is unrelated to the (removed) internal domain tenant.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{FromRow, Sqlite, SqlitePool};
use xiaoguai_types::{
    ids::{SessionId, UserId},
    Session, SessionStatus, TenantRole, User,
};

use crate::repositories::error::{RepoError, RepoResult};

/// External identity payload coming off an IM webhook.
#[derive(Debug, Clone)]
pub struct ExternalIdentity<'a> {
    pub provider: &'a str,
    pub tenant_external_id: &'a str,
    pub user_external_id: &'a str,
}

/// External conversation payload coming off an IM webhook.
#[derive(Debug, Clone)]
pub struct ExternalConversation<'a> {
    pub provider: &'a str,
    pub tenant_external_id: &'a str,
    pub conversation_id: &'a str,
}

/// Resolved internal IDs after auto-creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImIdentity {
    pub user_id: String,
}

/// Resolved internal session ID after auto-creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImConversation {
    pub session_id: String,
}

/// What the model column on auto-created sessions defaults to. The IM
/// gateway uses the per-request agent config to pick the real model at
/// runtime; this is just a label so the column has a value.
const DEFAULT_IM_MODEL: &str = "im-default";

#[async_trait]
pub trait ImIdentityRepository: Send + Sync {
    /// Resolve `(provider, tenant_ext, user_ext)` → internal `user_id`,
    /// auto-creating the user row on first sight.
    /// `display_hint` is used as the synthetic display name when creating.
    async fn resolve_or_create_identity(
        &self,
        ext: ExternalIdentity<'_>,
        display_hint: Option<&str>,
    ) -> RepoResult<ImIdentity>;

    /// Resolve `(provider, tenant_ext, conv_id)` → internal `session_id`,
    /// auto-creating the session on first sight. The session is bound to
    /// the resolved identity's tenant + user.
    async fn resolve_or_create_conversation(
        &self,
        conv: ExternalConversation<'_>,
        identity: &ImIdentity,
        model: Option<&str>,
    ) -> RepoResult<ImConversation>;
}

#[derive(Debug, Clone)]
pub struct SqliteImIdentityRepository {
    pool: SqlitePool,
}

impl SqliteImIdentityRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct IdentityRow {
    user_id: String,
}

#[derive(Debug, FromRow)]
struct ConversationRow {
    session_id: String,
}

#[async_trait]
impl ImIdentityRepository for SqliteImIdentityRepository {
    async fn resolve_or_create_identity(
        &self,
        ext: ExternalIdentity<'_>,
        display_hint: Option<&str>,
    ) -> RepoResult<ImIdentity> {
        // Fast path: hit the PK index.
        if let Some(row) = sqlx::query_as::<_, IdentityRow>(
            "SELECT user_id FROM im_identities \
             WHERE provider = ? AND tenant_external_id = ? AND user_external_id = ?",
        )
        .bind(ext.provider)
        .bind(ext.tenant_external_id)
        .bind(ext.user_external_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?
        {
            return Ok(ImIdentity {
                user_id: row.user_id,
            });
        }

        // Slow path: create user + mapping in one tx. Wrap with
        // `ON CONFLICT DO NOTHING` semantics + a re-select so two concurrent
        // webhooks for the same identity converge safely. No tenants table
        // under the single-user pivot — every user belongs to the owner.
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let synthetic_user = User {
            id: UserId::new(),
            email: synthetic_user_email(ext.provider, ext.tenant_external_id, ext.user_external_id),
            display_name: display_hint
                .map_or_else(|| ext.user_external_id.to_string(), str::to_string),
            roles: vec![TenantRole::Member],
            created_at: Utc::now(),
            last_login_at: None,
        };

        let user_id = upsert_user(&mut tx, &synthetic_user).await?;

        // Mapping insert. ON CONFLICT DO NOTHING + re-select handles races.
        sqlx::query(
            "INSERT INTO im_identities \
             (provider, tenant_external_id, user_external_id, user_id) \
             VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(ext.provider)
        .bind(ext.tenant_external_id)
        .bind(ext.user_external_id)
        .bind(&user_id)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let row: IdentityRow = sqlx::query_as(
            "SELECT user_id FROM im_identities \
             WHERE provider = ? AND tenant_external_id = ? AND user_external_id = ?",
        )
        .bind(ext.provider)
        .bind(ext.tenant_external_id)
        .bind(ext.user_external_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;

        Ok(ImIdentity {
            user_id: row.user_id,
        })
    }

    async fn resolve_or_create_conversation(
        &self,
        conv: ExternalConversation<'_>,
        identity: &ImIdentity,
        model: Option<&str>,
    ) -> RepoResult<ImConversation> {
        if let Some(row) = sqlx::query_as::<_, ConversationRow>(
            "SELECT session_id FROM im_conversations \
             WHERE provider = ? AND tenant_external_id = ? AND conversation_id = ?",
        )
        .bind(conv.provider)
        .bind(conv.tenant_external_id)
        .bind(conv.conversation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?
        {
            return Ok(ImConversation {
                session_id: row.session_id,
            });
        }

        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let session = Session {
            id: SessionId::new(),
            user_id: UserId::from(identity.user_id.clone()),
            title: Some(synthetic_session_title(conv.provider, conv.conversation_id)),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            model: model.unwrap_or(DEFAULT_IM_MODEL).to_string(),
            status: SessionStatus::Active,
            parent_session_id: None,
            forked_from_message_id: None,
        };

        sqlx::query(
            "INSERT INTO sessions (id, user_id, title, model, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(session.id.as_str())
        .bind(session.user_id.as_str())
        .bind(session.title.as_deref())
        .bind(&session.model)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO im_conversations \
             (provider, tenant_external_id, conversation_id, session_id) \
             VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(conv.provider)
        .bind(conv.tenant_external_id)
        .bind(conv.conversation_id)
        .bind(session.id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let row: ConversationRow = sqlx::query_as(
            "SELECT session_id FROM im_conversations \
             WHERE provider = ? AND tenant_external_id = ? AND conversation_id = ?",
        )
        .bind(conv.provider)
        .bind(conv.tenant_external_id)
        .bind(conv.conversation_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        // If a concurrent insert beat us to it the session row we just
        // created is now an orphan; clean it up so we don't leak rows.
        if row.session_id != session.id.as_str() {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(session.id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(RepoError::from_sqlx)?;
        }

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(ImConversation {
            session_id: row.session_id,
        })
    }
}

/// Synthetic tenant name encoding, stable per `(provider, tenant_ext)`.
/// Unused in the single-user pivot (no `tenants` table); retained only for
/// the `synthetic_names_are_stable` regression test.
#[cfg(test)]
fn synthetic_tenant_name(provider: &str, tenant_ext: &str) -> String {
    format!("im:{provider}:{tenant_ext}")
}

/// Synthetic user email used to satisfy `users (tenant_id, email)` uniqueness.
/// Lives in a reserved `.im.invalid` zone so it cannot collide with a real
/// directory email.
fn synthetic_user_email(provider: &str, tenant_ext: &str, user_ext: &str) -> String {
    format!("{user_ext}@{tenant_ext}.{provider}.im.invalid")
}

fn synthetic_session_title(provider: &str, conv: &str) -> String {
    format!("{provider}:{conv}")
}

/// Insert the user if absent, returning the id. `email` is globally unique
/// under the single-user pivot, so a race converges on the existing row.
async fn upsert_user(tx: &mut sqlx::Transaction<'_, Sqlite>, user: &User) -> RepoResult<String> {
    sqlx::query(
        "INSERT INTO users (id, email, display_name, created_at) \
         VALUES (?, ?, ?, ?) ON CONFLICT (email) DO NOTHING",
    )
    .bind(user.id.as_str())
    .bind(&user.email)
    .bind(&user.display_name)
    .bind(user.created_at)
    .execute(&mut **tx)
    .await
    .map_err(RepoError::from_sqlx)?;

    let (id,): (String,) = sqlx::query_as("SELECT id FROM users WHERE email = ?")
        .bind(&user.email)
        .fetch_one(&mut **tx)
        .await
        .map_err(RepoError::from_sqlx)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_names_are_stable() {
        assert_eq!(
            synthetic_tenant_name("feishu", "ten_x"),
            "im:feishu:ten_x".to_string()
        );
        assert_eq!(
            synthetic_user_email("feishu", "ten_x", "ou_alice"),
            "ou_alice@ten_x.feishu.im.invalid".to_string()
        );
        assert_eq!(
            synthetic_session_title("feishu", "oc_a"),
            "feishu:oc_a".to_string()
        );
    }
}
