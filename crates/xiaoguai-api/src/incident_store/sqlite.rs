//! `SQLite` [`IncidentStore`] over migration `0033_incidents.sql`.
//!
//! Follows the `SqliteLoopRepository` conventions: plain `sqlx::query_as`
//! row shapes converted into the public records, and guard-based UPDATEs
//! so status transitions are race-safe at the SQL layer (`set_status`
//! only fires when the row still holds the status the transition was
//! validated against). The dedup invariant is double-enforced: a fast
//! SELECT first, and the partial unique index
//! `incidents_live_dedup_idx` as the concurrent-ingest backstop.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::incidents::Incident;

use super::{
    record_from_incident, severity_as_str, severity_parse, IncidentDetails, IncidentRecord,
    IncidentStatus, IncidentStore, IncidentStoreError, IncidentStoreResult, IngestOutcome,
    RcaRecord, ReconciledIncident, RepairRecord,
};

/// Production [`IncidentStore`] backed by the shared `SQLite` pool.
#[derive(Debug, Clone)]
pub struct SqliteIncidentStore {
    pool: SqlitePool,
}

impl SqliteIncidentStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Live (non-terminal) row for the dedup key, if any.
    async fn find_live(
        &self,
        source: &str,
        external_id: &str,
    ) -> IncidentStoreResult<Option<IncidentRecord>> {
        let row: Option<IncidentDbRow> = sqlx::query_as(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM incidents \
             WHERE source = ? AND external_id = ? \
               AND status NOT IN ('resolved', 'failed', 'dismissed')"
        ))
        .bind(source)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(IncidentRecord::try_from).transpose()
    }

    async fn fetch(&self, id: Uuid) -> IncidentStoreResult<IncidentRecord> {
        let row: Option<IncidentDbRow> = sqlx::query_as(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM incidents WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(IncidentRecord::try_from)
            .transpose()?
            .ok_or(IncidentStoreError::NotFound)
    }
}

fn backend(e: sqlx::Error) -> IncidentStoreError {
    IncidentStoreError::Backend(e.to_string())
}

fn json_column(s: &str, column: &str) -> IncidentStoreResult<Value> {
    serde_json::from_str(s)
        .map_err(|e| IncidentStoreError::Backend(format!("corrupt JSON in {column}: {e}")))
}

const INCIDENT_COLUMNS: &str = "id, source, external_id, title, severity, project, \
                                environment, occurred_at, raw_payload, status, \
                                created_at, updated_at";

#[derive(sqlx::FromRow)]
struct IncidentDbRow {
    id: Uuid,
    source: String,
    external_id: String,
    title: String,
    severity: String,
    project: String,
    environment: Option<String>,
    occurred_at: DateTime<Utc>,
    raw_payload: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<IncidentDbRow> for IncidentRecord {
    type Error = IncidentStoreError;

    fn try_from(r: IncidentDbRow) -> Result<Self, IncidentStoreError> {
        let status = IncidentStatus::parse(&r.status).ok_or_else(|| {
            IncidentStoreError::Backend(format!("unknown incident status in DB: {}", r.status))
        })?;
        let severity = severity_parse(&r.severity).ok_or_else(|| {
            IncidentStoreError::Backend(format!("unknown severity in DB: {}", r.severity))
        })?;
        Ok(Self {
            id: r.id,
            source: r.source,
            external_id: r.external_id,
            title: r.title,
            severity,
            project: r.project,
            environment: r.environment,
            occurred_at: r.occurred_at,
            raw_payload: json_column(&r.raw_payload, "incidents.raw_payload")?,
            status,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const RCA_COLUMNS: &str = "id, incident_id, session_id, summary, root_cause, confidence, \
                           action_items, raw_markdown, created_at";

#[derive(sqlx::FromRow)]
struct RcaDbRow {
    id: Uuid,
    incident_id: Uuid,
    session_id: String,
    summary: String,
    root_cause: String,
    confidence: f64,
    action_items: String,
    raw_markdown: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<RcaDbRow> for RcaRecord {
    type Error = IncidentStoreError;

    fn try_from(r: RcaDbRow) -> Result<Self, IncidentStoreError> {
        Ok(Self {
            id: r.id,
            incident_id: r.incident_id,
            session_id: r.session_id,
            summary: r.summary,
            root_cause: r.root_cause,
            confidence: r.confidence,
            action_items: json_column(&r.action_items, "incident_rcas.action_items")?,
            raw_markdown: r.raw_markdown,
            created_at: r.created_at,
        })
    }
}

const REPAIR_COLUMNS: &str = "id, incident_id, rca_id, session_id, ok, summary, created_at";

#[derive(sqlx::FromRow)]
struct RepairDbRow {
    id: Uuid,
    incident_id: Uuid,
    rca_id: Uuid,
    session_id: String,
    ok: bool,
    summary: String,
    created_at: DateTime<Utc>,
}

impl From<RepairDbRow> for RepairRecord {
    fn from(r: RepairDbRow) -> Self {
        Self {
            id: r.id,
            incident_id: r.incident_id,
            rca_id: r.rca_id,
            session_id: r.session_id,
            ok: r.ok,
            summary: r.summary,
            created_at: r.created_at,
        }
    }
}

#[async_trait]
impl IncidentStore for SqliteIncidentStore {
    async fn ingest(&self, incident: &Incident, raw: Value) -> IncidentStoreResult<IngestOutcome> {
        // Fast path: a live row already holds the dedup slot — refresh it.
        if let Some(existing) = self.find_live(&incident.source, &incident.id).await? {
            return self.bump_duplicate(existing.id, incident, &raw).await;
        }
        let record = record_from_incident(incident, raw);
        let raw_text = serde_json::to_string(&record.raw_payload)
            .map_err(|e| IncidentStoreError::InvalidArgument(format!("raw payload: {e}")))?;
        let inserted = sqlx::query(
            "INSERT INTO incidents \
             (id, source, external_id, title, severity, project, environment, \
              occurred_at, raw_payload, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.id)
        .bind(&record.source)
        .bind(&record.external_id)
        .bind(&record.title)
        .bind(severity_as_str(&record.severity))
        .bind(&record.project)
        .bind(record.environment.as_deref())
        .bind(record.occurred_at)
        .bind(&raw_text)
        .bind(record.status.as_str())
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => Ok(IngestOutcome {
                record,
                was_duplicate: false,
            }),
            // Concurrent ingest backstop: the partial unique index fired —
            // someone else inserted the live row between our SELECT and
            // INSERT. Resolve to the duplicate path.
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                match self.find_live(&incident.source, &incident.id).await? {
                    Some(existing) => {
                        self.bump_duplicate(existing.id, incident, &record.raw_payload)
                            .await
                    }
                    None => Err(IncidentStoreError::Backend(
                        "dedup index violated but no live row found".to_string(),
                    )),
                }
            }
            Err(e) => Err(backend(e)),
        }
    }

    async fn get(&self, id: Uuid) -> IncidentStoreResult<IncidentRecord> {
        self.fetch(id).await
    }

    async fn list(
        &self,
        status: Option<IncidentStatus>,
    ) -> IncidentStoreResult<Vec<IncidentRecord>> {
        let rows: Vec<IncidentDbRow> = match status {
            Some(s) => {
                sqlx::query_as(&format!(
                    "SELECT {INCIDENT_COLUMNS} FROM incidents WHERE status = ? \
                     ORDER BY created_at DESC, rowid DESC"
                ))
                .bind(s.as_str())
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as(&format!(
                    "SELECT {INCIDENT_COLUMNS} FROM incidents \
                     ORDER BY created_at DESC, rowid DESC"
                ))
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(backend)?;
        rows.into_iter().map(IncidentRecord::try_from).collect()
    }

    async fn set_status(
        &self,
        id: Uuid,
        to: IncidentStatus,
    ) -> IncidentStoreResult<IncidentRecord> {
        let current = self.fetch(id).await?;
        if !current.status.can_transition_to(to) {
            return Err(IncidentStoreError::InvalidTransition {
                from: current.status.as_str(),
                to: to.as_str(),
            });
        }
        // Guard on the observed status so a concurrent transition loses
        // cleanly instead of silently overwriting.
        let result = sqlx::query(
            "UPDATE incidents \
             SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ? AND status = ?",
        )
        .bind(to.as_str())
        .bind(id)
        .bind(current.status.as_str())
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        if result.rows_affected() == 0 {
            let now = self.fetch(id).await?;
            return Err(IncidentStoreError::InvalidTransition {
                from: now.status.as_str(),
                to: to.as_str(),
            });
        }
        self.fetch(id).await
    }

    async fn insert_rca(&self, rca: &RcaRecord) -> IncidentStoreResult<()> {
        let action_items = serde_json::to_string(&rca.action_items)
            .map_err(|e| IncidentStoreError::InvalidArgument(format!("action items: {e}")))?;
        sqlx::query(
            "INSERT INTO incident_rcas \
             (id, incident_id, session_id, summary, root_cause, confidence, \
              action_items, raw_markdown, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(rca.id)
        .bind(rca.incident_id)
        .bind(&rca.session_id)
        .bind(&rca.summary)
        .bind(&rca.root_cause)
        .bind(rca.confidence)
        .bind(&action_items)
        .bind(&rca.raw_markdown)
        .bind(rca.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_fk_to_not_found)?;
        Ok(())
    }

    async fn insert_repair(&self, repair: &RepairRecord) -> IncidentStoreResult<()> {
        sqlx::query(
            "INSERT INTO incident_repairs \
             (id, incident_id, rca_id, session_id, ok, summary, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(repair.id)
        .bind(repair.incident_id)
        .bind(repair.rca_id)
        .bind(&repair.session_id)
        .bind(repair.ok)
        .bind(&repair.summary)
        .bind(repair.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_fk_to_not_found)?;
        Ok(())
    }

    async fn get_with_details(&self, id: Uuid) -> IncidentStoreResult<IncidentDetails> {
        let incident = self.fetch(id).await?;
        let rcas: Vec<RcaDbRow> = sqlx::query_as(&format!(
            "SELECT {RCA_COLUMNS} FROM incident_rcas WHERE incident_id = ? \
             ORDER BY created_at DESC, rowid DESC"
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let repairs: Vec<RepairDbRow> = sqlx::query_as(&format!(
            "SELECT {REPAIR_COLUMNS} FROM incident_repairs WHERE incident_id = ? \
             ORDER BY created_at DESC, rowid DESC"
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        Ok(IncidentDetails {
            incident,
            rcas: rcas
                .into_iter()
                .map(RcaRecord::try_from)
                .collect::<Result<_, _>>()?,
            repairs: repairs.into_iter().map(RepairRecord::from).collect(),
        })
    }

    async fn reconcile_interrupted(&self) -> IncidentStoreResult<Vec<ReconciledIncident>> {
        // #284 boot reconcile: collect the stranded rows first so the
        // caller can audit each, then move them with status-guarded
        // UPDATEs (same race-safe posture as `set_status`). Both moves
        // are legal transitions: `analyzing → open` (retryable) and
        // `repairing → failed` (Executor may have partially mutated).
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, status FROM incidents WHERE status IN ('analyzing', 'repairing')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        let mut reconciled = Vec::with_capacity(rows.len());
        for (id, status) in rows {
            let from = IncidentStatus::parse(&status).ok_or_else(|| {
                IncidentStoreError::Backend(format!("unknown incident status in DB: {status}"))
            })?;
            let to = match from {
                IncidentStatus::Analyzing => IncidentStatus::Open,
                _ => IncidentStatus::Failed,
            };
            let result = sqlx::query(
                "UPDATE incidents \
                 SET status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
                 WHERE id = ? AND status = ?",
            )
            .bind(to.as_str())
            .bind(id)
            .bind(from.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
            if result.rows_affected() > 0 {
                reconciled.push(ReconciledIncident { id, from, to });
            }
        }
        Ok(reconciled)
    }
}

impl SqliteIncidentStore {
    /// Duplicate path (#284): refresh `raw_payload` + `severity` from the
    /// re-fired alert (an escalated re-send must not be silently dropped)
    /// and bump `updated_at` on the live row, then return it.
    async fn bump_duplicate(
        &self,
        id: Uuid,
        incident: &Incident,
        raw: &Value,
    ) -> IncidentStoreResult<IngestOutcome> {
        let raw_text = serde_json::to_string(raw)
            .map_err(|e| IncidentStoreError::InvalidArgument(format!("raw payload: {e}")))?;
        sqlx::query(
            "UPDATE incidents \
             SET raw_payload = ?, severity = ?, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ?",
        )
        .bind(&raw_text)
        .bind(severity_as_str(&incident.severity))
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(IngestOutcome {
            record: self.fetch(id).await?,
            was_duplicate: true,
        })
    }
}

/// FK violations on rca/repair inserts mean the parent incident (or RCA)
/// is gone — surface as `NotFound`, matching the in-memory impl.
fn map_fk_to_not_found(e: sqlx::Error) -> IncidentStoreError {
    match &e {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => IncidentStoreError::NotFound,
        _ => backend(e),
    }
}
