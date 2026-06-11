//! In-memory [`IncidentStore`] for unit tests and integration harnesses.
//!
//! Not intended for production use. Mirrors `InMemoryTeamRepository`:
//! state behind a `parking_lot::Mutex` so the type is `Send + Sync`. The
//! unit tests below pin the trait semantics (dedup upsert, status
//! transitions, list ordering/filtering, detail joins); the `SQLite` tests
//! in `tests/incident_store_sqlite.rs` pin the SQL itself.

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::Value;
use uuid::Uuid;

use crate::incidents::Incident;

use super::{
    record_from_incident, IncidentDetails, IncidentRecord, IncidentStatus, IncidentStore,
    IncidentStoreError, IncidentStoreResult, IngestOutcome, RcaRecord, ReconciledIncident,
    RepairRecord,
};

#[derive(Default)]
struct Inner {
    /// Insertion order — `list` returns the reverse (newest first).
    incidents: Vec<IncidentRecord>,
    rcas: Vec<RcaRecord>,
    repairs: Vec<RepairRecord>,
}

/// Thread-safe in-memory store. All operations are synchronous under the
/// mutex.
#[derive(Default)]
pub struct InMemoryIncidentStore {
    state: Mutex<Inner>,
}

impl InMemoryIncidentStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl IncidentStore for InMemoryIncidentStore {
    async fn ingest(&self, incident: &Incident, raw: Value) -> IncidentStoreResult<IngestOutcome> {
        let mut g = self.state.lock();
        if let Some(existing) = g.incidents.iter_mut().find(|r| {
            r.source == incident.source && r.external_id == incident.id && !r.status.is_terminal()
        }) {
            // #284: a re-fired alert refreshes the payload + severity —
            // an escalated re-send must not be silently dropped.
            existing.raw_payload = raw;
            existing.severity = incident.severity.clone();
            existing.updated_at = Utc::now();
            return Ok(IngestOutcome {
                record: existing.clone(),
                was_duplicate: true,
            });
        }
        let record = record_from_incident(incident, raw);
        g.incidents.push(record.clone());
        Ok(IngestOutcome {
            record,
            was_duplicate: false,
        })
    }

    async fn get(&self, id: Uuid) -> IncidentStoreResult<IncidentRecord> {
        let g = self.state.lock();
        g.incidents
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or(IncidentStoreError::NotFound)
    }

    async fn list(
        &self,
        status: Option<IncidentStatus>,
    ) -> IncidentStoreResult<Vec<IncidentRecord>> {
        let g = self.state.lock();
        Ok(g.incidents
            .iter()
            .rev() // newest first
            .filter(|r| status.is_none_or(|s| r.status == s))
            .cloned()
            .collect())
    }

    async fn set_status(
        &self,
        id: Uuid,
        to: IncidentStatus,
    ) -> IncidentStoreResult<IncidentRecord> {
        let mut g = self.state.lock();
        let record = g
            .incidents
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(IncidentStoreError::NotFound)?;
        if !record.status.can_transition_to(to) {
            return Err(IncidentStoreError::InvalidTransition {
                from: record.status.as_str(),
                to: to.as_str(),
            });
        }
        record.status = to;
        record.updated_at = Utc::now();
        Ok(record.clone())
    }

    async fn insert_rca(&self, rca: &RcaRecord) -> IncidentStoreResult<()> {
        let mut g = self.state.lock();
        if !g.incidents.iter().any(|r| r.id == rca.incident_id) {
            return Err(IncidentStoreError::NotFound);
        }
        g.rcas.push(rca.clone());
        Ok(())
    }

    async fn insert_repair(&self, repair: &RepairRecord) -> IncidentStoreResult<()> {
        let mut g = self.state.lock();
        if !g.incidents.iter().any(|r| r.id == repair.incident_id)
            || !g.rcas.iter().any(|r| r.id == repair.rca_id)
        {
            return Err(IncidentStoreError::NotFound);
        }
        g.repairs.push(repair.clone());
        Ok(())
    }

    async fn get_with_details(&self, id: Uuid) -> IncidentStoreResult<IncidentDetails> {
        let g = self.state.lock();
        let incident = g
            .incidents
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or(IncidentStoreError::NotFound)?;
        let rcas = g
            .rcas
            .iter()
            .rev()
            .filter(|r| r.incident_id == id)
            .cloned()
            .collect();
        let repairs = g
            .repairs
            .iter()
            .rev()
            .filter(|r| r.incident_id == id)
            .cloned()
            .collect();
        Ok(IncidentDetails {
            incident,
            rcas,
            repairs,
        })
    }

    async fn reconcile_interrupted(&self) -> IncidentStoreResult<Vec<ReconciledIncident>> {
        // #284 boot reconcile: `analyzing → open` (retryable),
        // `repairing → failed` (partial mutations may have landed).
        let mut g = self.state.lock();
        let mut reconciled = Vec::new();
        for record in &mut g.incidents {
            let to = match record.status {
                IncidentStatus::Analyzing => IncidentStatus::Open,
                IncidentStatus::Repairing => IncidentStatus::Failed,
                _ => continue,
            };
            reconciled.push(ReconciledIncident {
                id: record.id,
                from: record.status,
                to,
            });
            record.status = to;
            record.updated_at = Utc::now();
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incidents::Severity;
    use chrono::{DateTime, Utc};
    use serde_json::json;

    fn sample_incident(external_id: &str) -> Incident {
        Incident {
            id: external_id.to_string(),
            title: "ZeroDivisionError: division by zero".to_string(),
            severity: Severity::High,
            source: "sentry".to_string(),
            occurred_at: "2026-06-10T01:02:03Z".parse::<DateTime<Utc>>().unwrap(),
            url: "https://sentry.example/issues/123/".to_string(),
            project: "backend".to_string(),
            environment: Some("production".to_string()),
            raw: json!({"k": "v"}),
        }
    }

    async fn drive_to(
        store: &InMemoryIncidentStore,
        id: Uuid,
        path: &[IncidentStatus],
    ) -> IncidentRecord {
        let mut last = store.get(id).await.unwrap();
        for s in path {
            last = store.set_status(id, *s).await.unwrap();
        }
        last
    }

    #[tokio::test]
    async fn ingest_creates_open_incident() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:123");
        let out = store.ingest(&inc, inc.raw.clone()).await.unwrap();
        assert!(!out.was_duplicate);
        assert_eq!(out.record.status, IncidentStatus::Open);
        assert_eq!(out.record.source, "sentry");
        assert_eq!(out.record.external_id, "sentry:123");
        assert_eq!(out.record.severity, Severity::High);
        assert_eq!(out.record.raw_payload, json!({"k": "v"}));
    }

    #[tokio::test]
    async fn duplicate_ingest_returns_existing_row() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:123");
        let first = store.ingest(&inc, inc.raw.clone()).await.unwrap();
        let second = store.ingest(&inc, inc.raw.clone()).await.unwrap();
        assert!(second.was_duplicate);
        assert_eq!(second.record.id, first.record.id);
        assert!(second.record.updated_at >= first.record.updated_at);
        assert_eq!(store.list(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_ingest_refreshes_payload_and_severity() {
        // #284: an escalated re-send must update the live row, not be
        // silently dropped.
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:123");
        let first = store.ingest(&inc, json!({"attempt": 1})).await.unwrap();
        assert_eq!(first.record.severity, Severity::High);

        let mut escalated = sample_incident("sentry:123");
        escalated.severity = Severity::Critical;
        let second = store
            .ingest(&escalated, json!({"attempt": 2}))
            .await
            .unwrap();
        assert!(second.was_duplicate);
        assert_eq!(second.record.id, first.record.id);
        assert_eq!(second.record.severity, Severity::Critical);
        assert_eq!(second.record.raw_payload, json!({"attempt": 2}));

        let fetched = store.get(first.record.id).await.unwrap();
        assert_eq!(fetched.severity, Severity::Critical);
        assert_eq!(fetched.raw_payload, json!({"attempt": 2}));
    }

    #[tokio::test]
    async fn reconcile_interrupted_reopens_analyzing_and_fails_repairing() {
        // #284: boot reconcile after a crash / dropped handler future.
        let store = InMemoryIncidentStore::new();
        let analyzing = store
            .ingest(&sample_incident("sentry:a"), json!({}))
            .await
            .unwrap()
            .record
            .id;
        store
            .set_status(analyzing, IncidentStatus::Analyzing)
            .await
            .unwrap();
        let repairing = store
            .ingest(&sample_incident("sentry:b"), json!({}))
            .await
            .unwrap()
            .record
            .id;
        drive_to(
            &store,
            repairing,
            &[
                IncidentStatus::Analyzing,
                IncidentStatus::AwaitingApproval,
                IncidentStatus::Repairing,
            ],
        )
        .await;
        let untouched = store
            .ingest(&sample_incident("sentry:c"), json!({}))
            .await
            .unwrap()
            .record
            .id;

        let moved = store.reconcile_interrupted().await.unwrap();
        assert_eq!(moved.len(), 2);
        let by_id = |id| moved.iter().find(|m| m.id == id).unwrap();
        assert_eq!(by_id(analyzing).from, IncidentStatus::Analyzing);
        assert_eq!(by_id(analyzing).to, IncidentStatus::Open);
        assert_eq!(by_id(repairing).from, IncidentStatus::Repairing);
        assert_eq!(by_id(repairing).to, IncidentStatus::Failed);

        assert_eq!(
            store.get(analyzing).await.unwrap().status,
            IncidentStatus::Open
        );
        assert_eq!(
            store.get(repairing).await.unwrap().status,
            IncidentStatus::Failed
        );
        assert_eq!(
            store.get(untouched).await.unwrap().status,
            IncidentStatus::Open
        );

        // Idempotent: a second pass finds nothing to move.
        assert!(store.reconcile_interrupted().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminal_incident_does_not_block_a_fresh_one() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:123");
        let first = store.ingest(&inc, inc.raw.clone()).await.unwrap();
        store
            .set_status(first.record.id, IncidentStatus::Dismissed)
            .await
            .unwrap();
        let second = store.ingest(&inc, inc.raw.clone()).await.unwrap();
        assert!(!second.was_duplicate, "terminal row must not dedup");
        assert_ne!(second.record.id, first.record.id);
    }

    #[tokio::test]
    async fn different_sources_do_not_dedup_each_other() {
        let store = InMemoryIncidentStore::new();
        let sentry = sample_incident("x:1");
        let mut datadog = sample_incident("x:1");
        datadog.source = "datadog".to_string();
        store.ingest(&sentry, json!({})).await.unwrap();
        let out = store.ingest(&datadog, json!({})).await.unwrap();
        assert!(!out.was_duplicate);
    }

    #[tokio::test]
    async fn full_happy_path_transitions() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:1");
        let id = store.ingest(&inc, json!({})).await.unwrap().record.id;
        let last = drive_to(
            &store,
            id,
            &[
                IncidentStatus::Analyzing,
                IncidentStatus::AwaitingApproval,
                IncidentStatus::Repairing,
                IncidentStatus::Resolved,
            ],
        )
        .await;
        assert_eq!(last.status, IncidentStatus::Resolved);
    }

    #[tokio::test]
    async fn illegal_transition_is_rejected() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:1");
        let id = store.ingest(&inc, json!({})).await.unwrap().record.id;
        // open → repairing skips two stages.
        let err = store
            .set_status(id, IncidentStatus::Repairing)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            IncidentStoreError::InvalidTransition {
                from: "open",
                to: "repairing"
            }
        ));
        // The row is untouched.
        assert_eq!(store.get(id).await.unwrap().status, IncidentStatus::Open);
    }

    #[tokio::test]
    async fn analysis_failure_reopens_and_terminal_is_immutable() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:1");
        let id = store.ingest(&inc, json!({})).await.unwrap().record.id;
        store
            .set_status(id, IncidentStatus::Analyzing)
            .await
            .unwrap();
        let reopened = store.set_status(id, IncidentStatus::Open).await.unwrap();
        assert_eq!(reopened.status, IncidentStatus::Open);

        let dismissed = store
            .set_status(id, IncidentStatus::Dismissed)
            .await
            .unwrap();
        assert_eq!(dismissed.status, IncidentStatus::Dismissed);
        let err = store
            .set_status(id, IncidentStatus::Open)
            .await
            .unwrap_err();
        assert!(matches!(err, IncidentStoreError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn set_status_on_unknown_incident_is_not_found() {
        let store = InMemoryIncidentStore::new();
        let err = store
            .set_status(Uuid::new_v4(), IncidentStatus::Analyzing)
            .await
            .unwrap_err();
        assert!(matches!(err, IncidentStoreError::NotFound));
    }

    #[tokio::test]
    async fn list_is_newest_first_with_optional_status_filter() {
        let store = InMemoryIncidentStore::new();
        let a = store
            .ingest(&sample_incident("sentry:a"), json!({}))
            .await
            .unwrap()
            .record
            .id;
        let b = store
            .ingest(&sample_incident("sentry:b"), json!({}))
            .await
            .unwrap()
            .record
            .id;
        store
            .set_status(a, IncidentStatus::Analyzing)
            .await
            .unwrap();

        let all = store.list(None).await.unwrap();
        assert_eq!(
            all.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b, a],
            "newest first"
        );

        let open = store.list(Some(IncidentStatus::Open)).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, b);
        assert!(store
            .list(Some(IncidentStatus::Resolved))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn details_join_rcas_and_repairs() {
        let store = InMemoryIncidentStore::new();
        let inc = sample_incident("sentry:1");
        let id = store.ingest(&inc, json!({})).await.unwrap().record.id;
        let rca = RcaRecord {
            id: Uuid::new_v4(),
            incident_id: id,
            session_id: format!("incident:{id}"),
            summary: "Checkout division by zero".to_string(),
            root_cause: "Empty cart divides by item count".to_string(),
            confidence: 0.9,
            action_items: json!(["guard divide", "add regression test"]),
            raw_markdown: "## RCA".to_string(),
            created_at: Utc::now(),
        };
        store.insert_rca(&rca).await.unwrap();
        let repair = RepairRecord {
            id: Uuid::new_v4(),
            incident_id: id,
            rca_id: rca.id,
            session_id: format!("incident:{id}"),
            ok: true,
            summary: "Guarded the divide".to_string(),
            created_at: Utc::now(),
        };
        store.insert_repair(&repair).await.unwrap();

        let details = store.get_with_details(id).await.unwrap();
        assert_eq!(details.incident.id, id);
        assert_eq!(details.rcas.len(), 1);
        assert_eq!(details.rcas[0].id, rca.id);
        assert_eq!(details.repairs.len(), 1);
        assert_eq!(details.repairs[0].id, repair.id);
    }

    #[tokio::test]
    async fn rca_and_repair_against_unknown_rows_are_not_found() {
        let store = InMemoryIncidentStore::new();
        let rca = RcaRecord {
            id: Uuid::new_v4(),
            incident_id: Uuid::new_v4(),
            session_id: "s".to_string(),
            summary: String::new(),
            root_cause: String::new(),
            confidence: 0.0,
            action_items: json!([]),
            raw_markdown: String::new(),
            created_at: Utc::now(),
        };
        assert!(matches!(
            store.insert_rca(&rca).await.unwrap_err(),
            IncidentStoreError::NotFound
        ));

        let inc = sample_incident("sentry:1");
        let id = store.ingest(&inc, json!({})).await.unwrap().record.id;
        let repair = RepairRecord {
            id: Uuid::new_v4(),
            incident_id: id,
            rca_id: Uuid::new_v4(), // unknown RCA
            session_id: "s".to_string(),
            ok: false,
            summary: String::new(),
            created_at: Utc::now(),
        };
        assert!(matches!(
            store.insert_repair(&repair).await.unwrap_err(),
            IncidentStoreError::NotFound
        ));
    }
}
