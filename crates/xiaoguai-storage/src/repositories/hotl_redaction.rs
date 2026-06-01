//! sprint-13 S13-3 + sprint-14 S14-2: `HotlRedactionRepo` — per-tenant access
//! to `hotl_redaction_policies` (DEC-HLD-014/017, guardrails.md §3.1).
//!
//! Sprint-13 shipped this as read-only. Sprint-14 adds insert-only revision
//! CRUD (DEC-030/DEC-HLD-017):
//!
//! * [`insert_policy`] — create a brand new active rule for a (tenant, scope,
//!   jsonpath) triple. Fails with [`RepoError::DuplicateKey`] if an active
//!   rule already exists for that triple (enforced by the partial unique
//!   index from migration 0028).
//! * [`supersede_policy`] — atomic 2-statement tx that deactivates the prior
//!   row and writes a new revision pointing at it via `supersedes_policy_id`.
//!   Returns [`RepoError::StaleRevision`] if the prior is already inactive.
//! * [`deactivate_policy`] — single-statement UPDATE setting `active = false`.
//!   The row stays in the table so audit FKs from earlier `hotl_pending`
//!   events still resolve.
//! * [`get_revisions`] — walks the supersedes chain reverse-chronologically.
//!
//! ## Transaction ordering (CRITICAL)
//!
//! `supersede_policy` MUST run UPDATE-prior-active=false **before**
//! INSERT-new-active=true under READ COMMITTED + the partial unique index
//! `WHERE active = true`. INSERT-first would have both the prior and the
//! new row satisfying the partial-index predicate simultaneously → 23505
//! `unique_violation`. PostgreSQL's `CREATE UNIQUE INDEX ... WHERE` cannot be
//! `DEFERRABLE` — only conventional `ALTER TABLE ... ADD CONSTRAINT ...
//! DEFERRABLE` can, and that path doesn't accept partial predicates. So
//! sequencing inside the tx is the only correct mechanism. Step-3 review
//! caught this twice; the constraint is encoded in
//! `tests/hotl_redaction_revisions.rs::concurrent_supersedes_against_same_prior`.
//!
//! ## RLS
//!
//! `hotl_redaction_policies` is RLS-enabled (migration 0027). The read path
//! ([`load_for_tenant`]) sets `app.current_tenant_id` via [`begin_tenant_tx`]
//! to play within tenant isolation. The mutation methods (S14-2) are invoked
//! from the admin API (S14-3), which authenticates as an admin role and
//! does **not** rely on RLS for tenant scoping — the API handler authorises
//! the `tenant_id` explicitly. The repo therefore uses the raw pool for
//! mutations, consistent with `HotlEscalationStore` in
//! `repositories/hotl_escalations.rs`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::repositories::error::{RepoError, RepoResult};
use crate::repositories::tenant_ctx::begin_tenant_tx;

/// A single row from `hotl_redaction_policies`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct RedactionPolicyRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// `*` is the catch-all; any other value is an exact scope match
    /// (e.g. `tool_call.execute_python`).
    pub scope: String,
    /// `JSONPath` selector (e.g. `$.password`, `$.headers.authorization`).
    pub jsonpath: String,
    /// Where the redaction applies — typical values are `sse` and `audit`.
    pub applies_to: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// True when this row is the current head of its revision chain.
    /// Sprint-14 (migration 0028) added this column with default `true`.
    #[sqlx(default)]
    pub active: bool,
    /// Operator (or `'system'` for v1.10.x backfill rows) who created
    /// this revision. Sprint-14 column.
    #[sqlx(default)]
    pub created_by: String,
    /// FK to the prior revision this row supersedes; `None` for the
    /// initial revision of a chain. Sprint-14 column.
    #[sqlx(default)]
    pub supersedes_policy_id: Option<Uuid>,
}

/// Payload for [`HotlRedactionRepo::supersede_policy`] — the fields the
/// new revision should carry. Kept separate from `RedactionPolicyRow` so
/// callers don't have to populate `id`/`created_at`/`active`/etc.
#[derive(Debug, Clone)]
pub struct SupersedeFields {
    pub tenant_id: Uuid,
    pub scope: String,
    pub jsonpath: String,
    pub applies_to: Vec<String>,
}

/// Trait surface used by the admin API (sprint-14 S14-3) and the SSE
/// read path (sprint-13 S13-4). Object-safe so it can be stored as
/// `Arc<dyn HotlRedactionRepo>` on `AppState`.
#[async_trait]
pub trait HotlRedactionRepo: Send + Sync {
    /// Return every **active** policy row for `tenant_id`, sorted
    /// exact-scope first then `*` catch-all, with a secondary sort on
    /// `scope ASC` for determinism when multiple exact scopes coexist.
    ///
    /// The S13-4 consumer (`xiaoguai-auth::redaction::RedactionRules`)
    /// iterates and picks the first matching rule for a given scope,
    /// so the `*` rule only applies when no exact match exists.
    async fn load_for_tenant(&self, tenant_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>>;

    /// Create a brand-new revision (no prior). Fails with
    /// `RepoError::DuplicateKey` if an active rule already exists for
    /// `(tenant_id, scope, jsonpath)` per the migration 0028 partial
    /// unique index.
    async fn insert_policy(
        &self,
        tenant_id: Uuid,
        scope: String,
        jsonpath: String,
        applies_to: Vec<String>,
        created_by: String,
    ) -> RepoResult<RedactionPolicyRow>;

    /// Atomic supersede: UPDATE prior active=false, INSERT new
    /// active=true with `supersedes_policy_id = prior_id`. Returns the
    /// new row.
    ///
    /// Errors:
    /// * `RepoError::StaleRevision { current_head_id }` — the prior is
    ///   no longer the active head (another supersede won the race or
    ///   the row was deactivated). `current_head_id` is the active head
    ///   id at the time of the check, or `prior_id` itself when the
    ///   prior was deactivated without supersedes.
    /// * `RepoError::NotFound` — `prior_id` doesn't exist.
    /// * `RepoError::DuplicateKey` — the new (tenant, scope, jsonpath)
    ///   would collide with another active row that isn't the prior;
    ///   transaction rolls back and the prior remains active.
    async fn supersede_policy(
        &self,
        prior_id: Uuid,
        new: SupersedeFields,
        created_by: String,
    ) -> RepoResult<RedactionPolicyRow>;

    /// Single-statement UPDATE setting `active = false`. Idempotent —
    /// if the row is already inactive the call still succeeds. The row
    /// stays in the table so audit FKs continue to resolve.
    ///
    /// `actor` is recorded by the S14-4 audit hook (Wave 3) — this repo
    /// accepts the argument but does not yet write it; the column for
    /// `deactivated_by` doesn't exist on the table (audit-log only).
    async fn deactivate_policy(&self, policy_id: Uuid, actor: String) -> RepoResult<()>;

    /// Walk the supersedes chain starting at any id in the chain.
    /// Returns rows in reverse-chronological order (newest first).
    /// `RepoError::NotFound` if `policy_id` doesn't exist.
    async fn get_revisions(&self, policy_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>>;
}

/// Postgres-backed implementation.
#[derive(Debug, Clone)]
pub struct PgHotlRedactionRepo {
    pool: PgPool,
}

impl PgHotlRedactionRepo {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Column list for `SELECT` round-trips. Sprint-14 adds three columns
/// (`active`, `created_by`, `supersedes_policy_id`) beyond the sprint-13
/// shape.
const ALL_COLUMNS: &str =
    "id, tenant_id, scope, jsonpath, applies_to, created_at, active, created_by, supersedes_policy_id";

#[async_trait]
impl HotlRedactionRepo for PgHotlRedactionRepo {
    async fn load_for_tenant(&self, tenant_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>> {
        // `app.current_tenant_id` is a TEXT GUC; the RLS policy compares
        // `tenant_id::text = current_setting(...)`. Format the UUID as a
        // plain hyphenated string to match Postgres's default UUID cast.
        let tenant_str = tenant_id.to_string();
        let mut tx = begin_tenant_tx(&self.pool, Some(&tenant_str)).await?;
        let rows = sqlx::query_as::<_, RedactionPolicyRow>(&format!(
            "SELECT {ALL_COLUMNS} \
             FROM hotl_redaction_policies \
             WHERE tenant_id = $1 AND active = TRUE \
             ORDER BY (scope = '*') ASC, scope ASC"
        ))
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(rows)
    }

    async fn insert_policy(
        &self,
        tenant_id: Uuid,
        scope: String,
        jsonpath: String,
        applies_to: Vec<String>,
        created_by: String,
    ) -> RepoResult<RedactionPolicyRow> {
        let row = sqlx::query_as::<_, RedactionPolicyRow>(&format!(
            "INSERT INTO hotl_redaction_policies \
             (tenant_id, scope, jsonpath, applies_to, created_by, active) \
             VALUES ($1, $2, $3, $4, $5, TRUE) \
             RETURNING {ALL_COLUMNS}"
        ))
        .bind(tenant_id)
        .bind(&scope)
        .bind(&jsonpath)
        .bind(&applies_to)
        .bind(&created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(row)
    }

    async fn supersede_policy(
        &self,
        prior_id: Uuid,
        new: SupersedeFields,
        created_by: String,
    ) -> RepoResult<RedactionPolicyRow> {
        // ORDERING IS LOAD-BEARING — see module docstring.
        //
        // 1. BEGIN
        // 2. SELECT prior FOR UPDATE — row-lock the prior so concurrent
        //    supersede attempts serialise here.
        // 3. If !active → StaleRevision { current_head_id }.
        // 4. UPDATE prior SET active = false.
        // 5. INSERT new SET active = true, supersedes_policy_id = prior_id.
        // 6. COMMIT.
        //
        // Step 5 fires the partial-unique index `WHERE active = true`. If
        // any *other* active row exists for (tenant, scope, jsonpath) the
        // INSERT fails 23505 → tx rolled back → prior stays active.
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        // Step 2 + 3: lock prior + freshness check.
        //
        // We lock the prior row; concurrent `supersede_policy(prior_id)`
        // callers block here until our tx commits/rolls back. The losing
        // task then reads `active = false` and returns StaleRevision.
        let prior: Option<(bool, Uuid)> = sqlx::query_as(
            "SELECT active, tenant_id FROM hotl_redaction_policies \
             WHERE id = $1 FOR UPDATE",
        )
        .bind(prior_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let Some((prior_active, _prior_tenant)) = prior else {
            return Err(RepoError::NotFound);
        };

        if !prior_active {
            // Find the current head of the chain rooted at `prior_id`.
            //
            // Strategy: walk forward through `supersedes_policy_id` until
            // we hit a row that no other row supersedes. If the prior was
            // simply deactivated (no successor), the head **is** the
            // prior itself.
            let head: (Uuid,) = sqlx::query_as(
                "WITH RECURSIVE forward (id) AS ( \
                     SELECT id FROM hotl_redaction_policies WHERE id = $1 \
                     UNION ALL \
                     SELECT p.id FROM hotl_redaction_policies p \
                     JOIN forward f ON p.supersedes_policy_id = f.id \
                 ) \
                 SELECT id FROM forward \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM hotl_redaction_policies p2 \
                     WHERE p2.supersedes_policy_id = forward.id \
                 ) \
                 LIMIT 1",
            )
            .bind(prior_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
            // Roll back the tx (no writes yet, but be explicit).
            tx.rollback().await.map_err(RepoError::from_sqlx)?;
            return Err(RepoError::StaleRevision {
                current_head_id: head.0,
            });
        }

        // Step 4: deactivate prior.
        sqlx::query("UPDATE hotl_redaction_policies SET active = FALSE WHERE id = $1")
            .bind(prior_id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;

        // Step 5: INSERT new revision pointing at prior.
        let new_row = sqlx::query_as::<_, RedactionPolicyRow>(&format!(
            "INSERT INTO hotl_redaction_policies \
             (tenant_id, scope, jsonpath, applies_to, created_by, active, supersedes_policy_id) \
             VALUES ($1, $2, $3, $4, $5, TRUE, $6) \
             RETURNING {ALL_COLUMNS}"
        ))
        .bind(new.tenant_id)
        .bind(&new.scope)
        .bind(&new.jsonpath)
        .bind(&new.applies_to)
        .bind(&created_by)
        .bind(prior_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(new_row)
    }

    async fn deactivate_policy(&self, policy_id: Uuid, _actor: String) -> RepoResult<()> {
        // `_actor` is consumed by the S14-4 audit hook (Wave 3) — kept in
        // the signature so S14-4 doesn't need a breaking-API rebase.
        let result = sqlx::query("UPDATE hotl_redaction_policies SET active = FALSE WHERE id = $1")
            .bind(policy_id)
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }

    async fn get_revisions(&self, policy_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>> {
        // The migration-0028 view `hotl_redaction_policy_revisions` walks
        // backward from each anchor via `supersedes_policy_id`. To get the
        // full chain from any node, we anchor at the chain head (the only
        // node that no other row supersedes) and walk back.
        //
        // Strategy: starting from `policy_id`, find the chain head by
        // following the "is superseded by" direction (any row with
        // supersedes_policy_id = ?), then return everything reachable
        // backward from that head.
        //
        // Two queries (one to find head, one to walk back) keep the SQL
        // simple and the plans predictable on a small chain.
        let exists: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM hotl_redaction_policies WHERE id = $1")
                .bind(policy_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(RepoError::from_sqlx)?;
        if exists.is_none() {
            return Err(RepoError::NotFound);
        }

        // Find the head: walk forward through `supersedes_policy_id`
        // (i.e. each step: "which row supersedes this one?"). Recursive
        // CTE terminates when no successor exists.
        let head: (Uuid,) = sqlx::query_as(
            "WITH RECURSIVE forward (id, parent_id) AS ( \
                 SELECT id, supersedes_policy_id FROM hotl_redaction_policies WHERE id = $1 \
                 UNION ALL \
                 SELECT p.id, p.supersedes_policy_id \
                 FROM hotl_redaction_policies p \
                 JOIN forward f ON p.supersedes_policy_id = f.id \
             ) \
             SELECT id FROM forward \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM hotl_redaction_policies p2 \
                 WHERE p2.supersedes_policy_id = forward.id \
             ) \
             LIMIT 1",
        )
        .bind(policy_id)
        .fetch_one(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;

        // Now walk backward from the head via the view.
        let rows = sqlx::query_as::<_, RedactionPolicyRow>(&format!(
            "SELECT {ALL_COLUMNS} \
             FROM hotl_redaction_policy_revisions \
             WHERE anchor_id = $1 \
             ORDER BY created_at DESC"
        ))
        .bind(head.0)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(rows)
    }
}
