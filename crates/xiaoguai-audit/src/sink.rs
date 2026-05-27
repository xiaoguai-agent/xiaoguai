//! Postgres persistence sink for the HMAC-chained audit log.
//!
//! `append()` is atomic per-tenant via `SELECT ... FOR UPDATE` on the latest row
//! within a transaction — this serializes appends for a single tenant without
//! requiring a global `SERIALIZABLE` isolation level. Different tenants can
//! append concurrently (each tenant's chain is independent).
//!
//! Schema is provided by `xiaoguai-storage/migrations/0002_audit.sql`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

// We are loaded with `#[path = "sink.rs"] pub mod sink;` from `chain.rs`,
// so the parent module is `chain` and re-exports live at `crate::chain::...`.
use super::{AuditEntry, ChainError, ChainedAudit, StoredEntry, HMAC_LEN};
// `Redactor` lives at the crate root (mod `redact`), not in the `chain` parent.
use crate::Redactor;

/// Postgres-backed append-only audit sink.
#[derive(Clone)]
pub struct PgAuditSink {
    pool: PgPool,
    chain: ChainedAudit,
    /// Optional PII/secret redactor applied before signing. `None` = pass-through.
    redactor: Option<Redactor>,
}

impl std::fmt::Debug for PgAuditSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgAuditSink")
            .field("chain", &self.chain)
            .field("pool", &"PgPool { .. }")
            .field("redactor", &self.redactor)
            .finish()
    }
}

impl PgAuditSink {
    /// Build a sink from a connection pool and HMAC signing key.
    ///
    /// No redaction is applied — use [`with_redactor`](Self::with_redactor) to
    /// scrub PII/secrets before entries are signed.
    pub fn new(pool: PgPool, key: impl Into<Vec<u8>>) -> Self {
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
    /// Reads the latest row's `hmac` for `entry.tenant_id` under a row-level
    /// lock, computes the new `hmac`, and inserts. The whole sequence runs
    /// inside a single transaction so concurrent appends for the same tenant
    /// serialize correctly.
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
             WHERE tenant_id = $1 \
             ORDER BY id DESC \
             LIMIT 1 \
             FOR UPDATE",
        )
        .bind(&entry.tenant_id)
        .fetch_optional(&mut *tx)
        .await?;

        let prev_bytes: Vec<u8> = prev.unwrap_or_else(|| vec![0u8; HMAC_LEN]);
        if prev_bytes.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }

        let new_hmac = self.chain.compute_hmac(&prev_bytes, &entry)?;

        let id: i64 = sqlx::query_scalar::<_, i64>(
            "INSERT INTO audit_log \
                 (ts, tenant_id, actor, action, resource, details, prev_hmac, hmac) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING id",
        )
        .bind(entry.ts)
        .bind(&entry.tenant_id)
        .bind(&entry.actor)
        .bind(&entry.action)
        .bind(entry.resource.as_deref())
        .bind(&entry.details)
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

    /// List entries for a tenant in chronological (id ASC) order.
    pub async fn list(
        &self,
        tenant_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<StoredEntry>, ChainError> {
        // Use a single query with optional bounds expressed via COALESCE-style
        // sentinels to keep the SQL simple and statically prepared.
        let rows = sqlx::query_as::<_, AuditRow>(
            "SELECT id, ts, tenant_id, actor, action, resource, details, prev_hmac, hmac \
             FROM audit_log \
             WHERE tenant_id = $1 \
               AND ($2::timestamptz IS NULL OR ts >= $2) \
               AND ($3::timestamptz IS NULL OR ts <= $3) \
             ORDER BY id ASC \
             LIMIT $4",
        )
        .bind(tenant_id)
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
    tenant_id: String,
    actor: String,
    action: String,
    resource: Option<String>,
    details: serde_json::Value,
    prev_hmac: Vec<u8>,
    hmac: Vec<u8>,
}

impl AuditRow {
    fn into_stored(self) -> Result<StoredEntry, ChainError> {
        if self.prev_hmac.len() != HMAC_LEN || self.hmac.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }
        Ok(StoredEntry {
            id: self.id,
            entry: AuditEntry {
                ts: self.ts,
                tenant_id: self.tenant_id,
                actor: self.actor,
                action: self.action,
                resource: self.resource,
                details: self.details,
            },
            prev_hmac: self.prev_hmac,
            hmac: self.hmac,
        })
    }
}
