//! `LlmProviderRepository` — `SQLite`-backed registry of LLM providers.
//!
//! Single-owner deployment (DEC-033): all providers belong to the one owner;
//! every method runs in a plain transaction.

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use xiaoguai_types::{ids::ProviderId, Keyring, LlmProvider, ProviderKind};

use crate::repositories::error::{RepoError, RepoResult};

/// Env var holding the 32-byte base64url AES-256-GCM key used for field
/// encryption-at-rest (today: the provider `api_key` column). Unset = encryption
/// disabled — keys are stored and read as cleartext, fully backwards-compatible.
pub const ENV_AT_REST_KEY: &str = "XIAOGUAI_AT_REST_KEY";

/// Optional previous key for the rotation window. Same encoding as
/// [`ENV_AT_REST_KEY`]; accepted on decrypt only.
pub const ENV_AT_REST_KEY_PREV: &str = "XIAOGUAI_AT_REST_KEY_PREV";

/// Discriminator prefix marking a stored `api_key` as a sealed envelope
/// (`xgenc1:` + base64url-no-pad of the [`Keyring`] envelope). A value without
/// this prefix is treated as cleartext — that is how opt-in encryption and the
/// pre-backfill window stay unambiguous. No real API key begins with this
/// literal, so the discriminator never collides with a cleartext secret.
const ENC_PREFIX: &str = "xgenc1:";

#[async_trait]
pub trait LlmProviderRepository: Send + Sync {
    async fn create(&self, prov: &LlmProvider) -> RepoResult<()>;
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>>;
    /// All registered providers, ordered by `fallback_order` ascending.
    async fn list(&self) -> RepoResult<Vec<LlmProvider>>;
    async fn delete(&self, id: &str) -> RepoResult<()>;
    /// Overwrite the mutable columns of an existing provider, matched by
    /// `prov.id`. `id`, `name`, and `created_at` are left unchanged. Returns
    /// [`RepoError::NotFound`] when no row matches the id.
    async fn update(&self, prov: &LlmProvider) -> RepoResult<()>;
    /// Persist ONLY the connectivity-probe result set (`verified_models`),
    /// touching no other column. The probe endpoint uses this instead of
    /// [`Self::update`] so it never round-trips the secret `api_key` through
    /// reveal→conceal — a full-row rewrite would overwrite the stored key with
    /// `NULL` if it currently can't be decrypted (rotated at-rest key). Returns
    /// [`RepoError::NotFound`] when no row matches the id.
    async fn update_verified_models(&self, id: &str, verified: &[String]) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct SqliteLlmProviderRepository {
    pool: SqlitePool,
    /// When present, `api_key` is sealed before write and opened on read.
    /// `None` = encryption disabled (cleartext, the pre-existing behaviour).
    keyring: Option<Keyring>,
}

impl SqliteLlmProviderRepository {
    /// Construct a repository with encryption-at-rest **disabled** — api keys
    /// are stored and read as cleartext. Used by tests and call sites that do
    /// not load a keyring. Production serve/CLI paths use [`Self::from_env`].
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            keyring: None,
        }
    }

    /// Construct a repository with an explicit (optional) keyring. `None`
    /// behaves exactly like [`Self::new`]. Used by tests that exercise the
    /// encrypted path without touching process-global env vars.
    #[must_use]
    pub fn new_with_keyring(pool: SqlitePool, keyring: Option<Keyring>) -> Self {
        Self { pool, keyring }
    }

    /// Construct a repository, loading the at-rest keyring from
    /// [`ENV_AT_REST_KEY`] / [`ENV_AT_REST_KEY_PREV`]. An unset key disables
    /// encryption (cleartext, backwards-compatible); a *malformed* key is a
    /// hard error so the misconfiguration surfaces loudly instead of silently
    /// writing cleartext.
    ///
    /// # Errors
    /// [`RepoError::Encryption`] when a key env var is present but does not
    /// decode to a 32-byte base64url value.
    pub fn from_env(pool: SqlitePool) -> RepoResult<Self> {
        let keyring = Keyring::from_env_vars(ENV_AT_REST_KEY, ENV_AT_REST_KEY_PREV)
            .map_err(|e| RepoError::Encryption(e.to_string()))?;
        Ok(Self { pool, keyring })
    }

    /// Seal a plaintext api key for storage. Returns the value to write to the
    /// `api_key` column. `None` and empty stay as-is; an already-sealed value
    /// passes through unchanged (idempotent); with no keyring the plaintext is
    /// returned verbatim (opt-in cleartext).
    fn conceal(&self, plaintext: Option<&str>) -> RepoResult<Option<String>> {
        match plaintext {
            Some(pt) if !pt.is_empty() && !pt.starts_with(ENC_PREFIX) => match &self.keyring {
                Some(kr) => {
                    let envelope = kr
                        .encrypt(pt)
                        .map_err(|e| RepoError::Encryption(e.to_string()))?;
                    Ok(Some(format!(
                        "{ENC_PREFIX}{}",
                        URL_SAFE_NO_PAD.encode(envelope)
                    )))
                }
                None => Ok(Some(pt.to_string())),
            },
            other => Ok(other.map(str::to_string)),
        }
    }

    /// Reveal a stored api key for use. A value without the [`ENC_PREFIX`] is
    /// cleartext and returned as-is. A sealed value is decrypted; on any
    /// failure (no keyring configured, wrong key, corrupt envelope) the key is
    /// treated as **absent** (`None`) with a loud `error!` — fail-safe, so a
    /// single unreadable row makes one provider unauthenticated rather than
    /// bricking boot or leaking ciphertext as a bogus key.
    fn reveal(&self, stored: Option<String>) -> Option<String> {
        let raw = stored?;
        let Some(body) = raw.strip_prefix(ENC_PREFIX) else {
            return Some(raw);
        };
        let Some(kr) = &self.keyring else {
            tracing::error!(
                "llm provider api_key is encrypted at rest but {ENV_AT_REST_KEY} is not configured; \
                 treating the key as absent (provider will be unauthenticated)"
            );
            return None;
        };
        match URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|e| e.to_string())
            .and_then(|env| kr.decrypt(&env).map_err(|e| e.to_string()))
        {
            Ok(plaintext) => Some(plaintext),
            Err(reason) => {
                tracing::error!(
                    reason,
                    "failed to decrypt llm provider api_key at rest; treating the key as absent \
                     (provider will be unauthenticated). Check {ENV_AT_REST_KEY}/_PREV."
                );
                None
            }
        }
    }

    /// Return a copy of `prov` with its `api_key` decrypted for use (see
    /// [`Self::reveal`]). Immutable: produces a new value rather than mutating.
    fn with_revealed_key(&self, prov: LlmProvider) -> LlmProvider {
        let api_key = self.reveal(prov.api_key);
        LlmProvider { api_key, ..prov }
    }

    /// Opt-in encryption-at-rest backfill. When a keyring is configured, seal
    /// every provider `api_key` still stored in cleartext. No-op (returns `0`)
    /// when encryption is disabled. Idempotent: already-sealed rows (carrying
    /// the [`ENC_PREFIX`]) are skipped. Mirrors SEC-19's webhook-token backfill;
    /// run once at serve startup, non-fatal.
    ///
    /// # Errors
    /// Any error reading or updating the table.
    pub async fn backfill_encrypt_api_keys(&self) -> RepoResult<usize> {
        if self.keyring.is_none() {
            return Ok(0);
        }
        let cleartext: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, api_key FROM llm_providers \
             WHERE api_key IS NOT NULL AND api_key <> '' AND api_key NOT LIKE 'xgenc1:%'",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;

        let mut migrated = 0usize;
        for (id, plaintext) in cleartext {
            let sealed = self.conceal(Some(&plaintext))?;
            sqlx::query("UPDATE llm_providers SET api_key = ? WHERE id = ?")
                .bind(sealed.as_deref())
                .bind(&id)
                .execute(&self.pool)
                .await
                .map_err(RepoError::from_sqlx)?;
            migrated += 1;
        }
        if migrated > 0 {
            tracing::info!(
                count = migrated,
                "encrypted pre-existing cleartext llm provider api keys at rest"
            );
        }
        Ok(migrated)
    }
}

#[derive(Debug, FromRow)]
struct LlmProviderRow {
    id: String,
    name: String,
    kind: String,
    endpoint: String,
    models: serde_json::Value,
    default_for_models: serde_json::Value,
    /// JSON array of probe-confirmed models, or NULL when never probed. Added
    /// in migration 0038; `#[sqlx(default)]` keeps pre-migration reads safe.
    #[sqlx(default)]
    verified_models: Option<serde_json::Value>,
    fallback_order: i32,
    api_key_env: Option<String>,
    /// Directly-stored API key (web-UI providers); NULL for env-var /
    /// unauthenticated providers. Added in migration 0029.
    #[sqlx(default)]
    api_key: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// v1.1.1.1 — may be NULL if the column was added after the row was
    /// inserted (pre-migration rows) or for mock/test providers.
    cost_per_1k_input_usd: Option<f64>,
    cost_per_1k_output_usd: Option<f64>,
}

impl LlmProviderRow {
    fn into_domain(self) -> RepoResult<LlmProvider> {
        let kind = ProviderKind::parse(&self.kind).ok_or_else(|| {
            RepoError::InvalidArgument(format!("unknown provider kind in DB: {}", self.kind))
        })?;
        let models: Vec<String> = serde_json::from_value(self.models)?;
        let default_for_models: Vec<String> = serde_json::from_value(self.default_for_models)?;
        let verified_models: Option<Vec<String>> = self
            .verified_models
            .map(serde_json::from_value)
            .transpose()?;
        Ok(LlmProvider {
            id: ProviderId::from(self.id),
            name: self.name,
            kind,
            endpoint: self.endpoint,
            models,
            default_for_models,
            verified_models,
            fallback_order: self.fallback_order,
            api_key_env: self.api_key_env,
            api_key: self.api_key,
            created_at: self.created_at,
            updated_at: self.updated_at,
            cost_per_1k_input_usd: self.cost_per_1k_input_usd,
            cost_per_1k_output_usd: self.cost_per_1k_output_usd,
        })
    }
}

const SELECT_COLUMNS: &str = "id, name, kind, endpoint, models, default_for_models, \
     verified_models, fallback_order, api_key_env, api_key, created_at, updated_at, \
     cost_per_1k_input_usd, cost_per_1k_output_usd";

#[async_trait]
impl LlmProviderRepository for SqliteLlmProviderRepository {
    async fn create(&self, prov: &LlmProvider) -> RepoResult<()> {
        let models = serde_json::to_value(&prov.models)?;
        let defaults = serde_json::to_value(&prov.default_for_models)?;
        let verified = prov
            .verified_models
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let api_key = self.conceal(prov.api_key.as_deref())?;
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query(
            "INSERT INTO llm_providers \
             (id, name, kind, endpoint, models, default_for_models, \
              verified_models, fallback_order, api_key_env, api_key, created_at, updated_at, \
              cost_per_1k_input_usd, cost_per_1k_output_usd) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(prov.id.as_str())
        .bind(&prov.name)
        .bind(prov.kind.as_str())
        .bind(&prov.endpoint)
        .bind(models)
        .bind(defaults)
        .bind(verified)
        .bind(prov.fallback_order)
        .bind(prov.api_key_env.as_deref())
        .bind(api_key.as_deref())
        .bind(prov.created_at)
        .bind(prov.updated_at)
        .bind(prov.cost_per_1k_input_usd)
        .bind(prov.cost_per_1k_output_usd)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let row = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(row
            .map(LlmProviderRow::into_domain)
            .transpose()?
            .map(|p| self.with_revealed_key(p)))
    }

    async fn list(&self) -> RepoResult<Vec<LlmProvider>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let rows = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers \
             ORDER BY fallback_order ASC, created_at ASC"
        ))
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter()
            .map(LlmProviderRow::into_domain)
            .map(|r| r.map(|p| self.with_revealed_key(p)))
            .collect()
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query("DELETE FROM llm_providers WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn update(&self, prov: &LlmProvider) -> RepoResult<()> {
        let models = serde_json::to_value(&prov.models)?;
        let defaults = serde_json::to_value(&prov.default_for_models)?;
        let verified = prov
            .verified_models
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let api_key = self.conceal(prov.api_key.as_deref())?;
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let res = sqlx::query(
            "UPDATE llm_providers SET \
             kind = ?, endpoint = ?, models = ?, default_for_models = ?, \
             verified_models = ?, fallback_order = ?, api_key_env = ?, api_key = ?, \
             updated_at = ?, cost_per_1k_input_usd = ?, cost_per_1k_output_usd = ? \
             WHERE id = ?",
        )
        .bind(prov.kind.as_str())
        .bind(&prov.endpoint)
        .bind(models)
        .bind(defaults)
        .bind(verified)
        .bind(prov.fallback_order)
        .bind(prov.api_key_env.as_deref())
        .bind(api_key.as_deref())
        .bind(prov.updated_at)
        .bind(prov.cost_per_1k_input_usd)
        .bind(prov.cost_per_1k_output_usd)
        .bind(prov.id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn update_verified_models(&self, id: &str, verified: &[String]) -> RepoResult<()> {
        // Narrow write: only `verified_models` (+ `updated_at`). Deliberately
        // does NOT touch `api_key`, so a probe can't clobber a stored secret it
        // couldn't decrypt. `verified` is always a concrete array (possibly
        // empty = "probed, nothing reachable"), never NULL.
        let json = serde_json::to_value(verified)?;
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let res = sqlx::query(
            "UPDATE llm_providers SET verified_models = ?, updated_at = ? WHERE id = ?",
        )
        .bind(json)
        .bind(Utc::now())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
