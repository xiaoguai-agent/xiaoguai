//! `SQLite` persistence sink for the HMAC-chained audit log.
//!
//! `append()` is atomic via a transaction over the latest row — this serializes
//! appends for the single-user owner chain. `SQLite`'s write transaction already
//! provides the exclusive lock that Postgres' `SELECT ... FOR UPDATE` gave us.
//!
//! Schema is provided by `xiaoguai-storage/migrations/0002_audit.sql`
//! (single-user: `tenant_id` dropped, `id` INTEGER PK AUTOINCREMENT,
//! `prev_hmac`/`hmac` BLOB, `ts`/`details` TEXT).

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

/// Single-user owner chain identity. The audit `tenant_id` column was dropped in
/// the `SQLite` pivot; the HMAC chain still signs over a `tenant_id` field, so we
/// synthesize this fixed owner id on read to keep `verify_chain` valid.
const OWNER_TENANT_ID: &str = "ten_local_owner";

// We are loaded with `#[path = "sink.rs"] pub mod sink;` from `chain.rs`,
// so the parent module is `chain` and re-exports live at `crate::chain::...`.
use super::{AuditEntry, ChainError, ChainedAudit, StoredEntry, HMAC_LEN};
// `Redactor` lives at the crate root (mod `redact`), not in the `chain` parent.
use crate::Redactor;

/// SQLite-backed append-only audit sink.
#[derive(Clone)]
pub struct PgAuditSink {
    pool: SqlitePool,
    chain: ChainedAudit,
    /// Optional PII/secret redactor applied before signing. `None` = pass-through.
    redactor: Option<Redactor>,
}

impl std::fmt::Debug for PgAuditSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgAuditSink")
            .field("chain", &self.chain)
            .field("pool", &"SqlitePool { .. }")
            .field("redactor", &self.redactor)
            .finish()
    }
}

impl PgAuditSink {
    /// Build a sink from a connection pool and HMAC signing key.
    ///
    /// No redaction is applied — use [`with_redactor`](Self::with_redactor) to
    /// scrub PII/secrets before entries are signed.
    pub fn new(pool: SqlitePool, key: impl Into<Vec<u8>>) -> Self {
        Self {
            pool,
            chain: ChainedAudit::new(key),
            redactor: None,
        }
    }

    /// Enable PII/secret redaction. The redactor runs on every entry before its
    /// HMAC is computed, so the persisted row and its signature match.
    #[must_use]
    pub fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = Some(redactor);
        self
    }

    /// Borrow the underlying chain engine — useful for offline verification
    /// against a slice of [`StoredEntry`] values produced elsewhere.
    #[must_use]
    pub fn chain(&self) -> &ChainedAudit {
        &self.chain
    }

    /// Atomically append an entry.
    ///
    /// Reads the latest row's `hmac`, computes the new `hmac`, and inserts.
    /// The whole sequence runs inside a single transaction so concurrent
    /// appends serialize correctly (`SQLite`'s write lock provides the exclusion
    /// that Postgres' `SELECT ... FOR UPDATE` used to).
    pub async fn append(&self, entry: AuditEntry) -> Result<StoredEntry, ChainError> {
        // Redact PII/secrets before signing so the stored row and its HMAC are
        // both over the redacted form (keeps `verify_chain` valid).
        let entry = match &self.redactor {
            Some(r) => r.redact(entry),
            None => entry,
        };

        let mut tx = self.pool.begin().await?;

        let prev: Option<Vec<u8>> = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT hmac FROM audit_log \
             ORDER BY id DESC \
             LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;

        let prev_bytes: Vec<u8> = prev.unwrap_or_else(|| vec![0u8; HMAC_LEN]);
        if prev_bytes.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }

        // HMAC signs over the in-memory `entry` (incl. its `tenant_id`); the
        // column itself is dropped, so chain semantics are unchanged.
        let new_hmac = self.chain.compute_hmac(&prev_bytes, &entry)?;

        // `details` is a TEXT column in SQLite — serialize to a JSON string.
        let details_text = serde_json::to_string(&entry.details)
            .map_err(|e| ChainError::Canonical(format!("details encode: {e}")))?;

        let id: i64 = sqlx::query_scalar::<_, i64>(
            "INSERT INTO audit_log \
                 (ts, actor, action, resource, details, prev_hmac, hmac) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
        )
        .bind(entry.ts)
        .bind(&entry.actor)
        .bind(&entry.action)
        .bind(entry.resource.as_deref())
        .bind(&details_text)
        .bind(&prev_bytes)
        .bind(&new_hmac)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(StoredEntry {
            id,
            entry,
            prev_hmac: prev_bytes,
            hmac: new_hmac,
        })
    }

    /// List entries in chronological (id ASC) order.
    ///
    /// `tenant_id` is accepted for API compatibility but no longer filters:
    /// the single-user pivot keeps exactly one owner chain in this table.
    pub async fn list(
        &self,
        _tenant_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<StoredEntry>, ChainError> {
        // Optional bounds expressed via NULL sentinels keep the SQL statically
        // prepared. `since`/`until` are reused, so bind by position with `?N`.
        let rows = sqlx::query_as::<_, AuditRow>(
            "SELECT id, ts, actor, action, resource, details, prev_hmac, hmac \
             FROM audit_log \
             WHERE (?1 IS NULL OR ts >= ?1) \
               AND (?2 IS NULL OR ts <= ?2) \
             ORDER BY id ASC \
             LIMIT ?3",
        )
        .bind(since)
        .bind(until)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(AuditRow::into_stored).collect()
    }

    /// Verify the full chain for `tenant_id` from the genesis row.
    ///
    /// Loads up to `i64::MAX` rows — intended for periodic background sweeps.
    /// For large tenants, prefer streaming verification in batches (future work).
    pub async fn verify_tenant(&self, tenant_id: &str) -> Result<(), ChainError> {
        let entries = self.list(tenant_id, None, None, i64::MAX).await?;
        let zero = [0u8; HMAC_LEN];
        self.chain.verify_chain(&zero, &entries)
    }
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    ts: DateTime<Utc>,
    actor: String,
    action: String,
    resource: Option<String>,
    // `details` is a TEXT column in SQLite — read the raw JSON string and parse
    // in `into_stored` so a corrupt payload surfaces as a clear error.
    details: String,
    prev_hmac: Vec<u8>,
    hmac: Vec<u8>,
}

impl AuditRow {
    fn into_stored(self) -> Result<StoredEntry, ChainError> {
        if self.prev_hmac.len() != HMAC_LEN || self.hmac.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }
        let details: serde_json::Value = serde_json::from_str(&self.details)
            .map_err(|e| ChainError::Canonical(format!("details decode: {e}")))?;
        Ok(StoredEntry {
            id: self.id,
            entry: AuditEntry {
                ts: self.ts,
                // `tenant_id` column dropped in the SQLite pivot; synthesize the
                // single owner id so the rebuilt entry hashes identically and
                // `verify_chain` stays valid.
                tenant_id: OWNER_TENANT_ID.to_string(),
                actor: self.actor,
                action: self.action,
                resource: self.resource,
                details,
            },
            prev_hmac: self.prev_hmac,
            hmac: self.hmac,
        })
    }
}
