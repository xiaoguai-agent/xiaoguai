//! `SqliteIncidentStore` round-trip tests against a real temp `SQLite` DB.
//!
//! The in-memory tests (`src/incident_store/memory.rs`) pin trait
//! semantics; these pin migration `0033_incidents.sql` and the SQL itself —
//! column lists, the dedup partial unique index, JSON TEXT round-trips,
//! status-guarded UPDATEs, and the FK → `NotFound` mapping. No Docker —
//! temp file + crate migrations, like `xiaoguai-personas/tests/team_sqlite.rs`.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;
use xiaoguai_api::incident_store::{
    IncidentStatus, IncidentStore, IncidentStoreError, RcaRecord, RepairRecord, SqliteIncidentStore,
};
use xiaoguai_api::incidents::{Incident, Severity};
use xiaoguai_storage::db;

async fn test_setup() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let pool = db::connect(path.to_str().expect("utf8 path"), 5)
        .await
        .expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, dir)
}

fn sample_incident(external_id: &str) -> Incident {
    Incident {
        id: external_id.to_string(),
        title: "ZeroDivisionError: division by zero".to_string(),
        severity: Severity::Critical,
        source: "sentry".to_string(),
        occurred_at: "2026-06-10T01:02:03Z".parse::<DateTime<Utc>>().unwrap(),
        url: "https://sentry.example/issues/123/".to_string(),
        project: "backend".to_string(),
        environment: Some("production".to_string()),
        raw: json!({"data": {"issue": {"id": "123"}}}),
    }
}

#[tokio::test]
async fn sqlite_incident_full_roundtrip() {
    let (pool, _guard) = test_setup().await;
    let store = SqliteIncidentStore::new(pool);
    let inc = sample_incident("sentry:123");

    // Insert + read back: every column round-trips, JSON included.
    let out = store.ingest(&inc, inc.raw.clone()).await.unwrap();
    assert!(!out.was_duplicate);
    let fetched = store.get(out.record.id).await.unwrap();
    assert_eq!(fetched.source, "sentry");
    assert_eq!(fetched.external_id, "sentry:123");
    assert_eq!(fetched.title, inc.title);
    assert_eq!(fetched.severity, Severity::Critical);
    assert_eq!(fetched.project, "backend");
    assert_eq!(fetched.environment.as_deref(), Some("production"));
    assert_eq!(fetched.occurred_at, inc.occurred_at);
    assert_eq!(fetched.raw_payload, inc.raw);
    assert_eq!(fetched.status, IncidentStatus::Open);

    // Dedup: re-ingest bumps the same row.
    let dup = store.ingest(&inc, inc.raw.clone()).await.unwrap();
    assert!(dup.was_duplicate);
    assert_eq!(dup.record.id, out.record.id);
    assert_eq!(store.list(None).await.unwrap().len(), 1);

    // Status machine through the SQL guard.
    let r = store
        .set_status(out.record.id, IncidentStatus::Analyzing)
        .await
        .unwrap();
    assert_eq!(r.status, IncidentStatus::Analyzing);
    let err = store
        .set_status(out.record.id, IncidentStatus::Repairing)
        .await
        .unwrap_err();
    assert!(matches!(err, IncidentStoreError::InvalidTransition { .. }));

    // RCA + repair round-trip and the details join.
    let rca = RcaRecord {
        id: Uuid::new_v4(),
        incident_id: out.record.id,
        session_id: format!("incident:{}", out.record.id),
        summary: "Guard missing".to_string(),
        root_cause: "Empty cart".to_string(),
        confidence: 0.85,
        action_items: json!(["add guard", "regression test"]),
        raw_markdown: "## RCA".to_string(),
        created_at: Utc::now(),
    };
    store.insert_rca(&rca).await.unwrap();
    store
        .insert_repair(&RepairRecord {
            id: Uuid::new_v4(),
            incident_id: out.record.id,
            rca_id: rca.id,
            session_id: rca.session_id.clone(),
            ok: true,
            summary: "guarded".to_string(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let details = store.get_with_details(out.record.id).await.unwrap();
    assert_eq!(details.rcas.len(), 1);
    assert_eq!(
        details.rcas[0].action_items,
        json!(["add guard", "regression test"])
    );
    assert!((details.rcas[0].confidence - 0.85).abs() < f64::EPSILON);
    assert_eq!(details.repairs.len(), 1);
    assert!(details.repairs[0].ok);
}

#[tokio::test]
async fn sqlite_duplicate_ingest_refreshes_payload_and_severity() {
    // #284: an escalated re-send must update the live row's raw_payload
    // and severity through the SQL duplicate path, not be dropped.
    let (pool, _guard) = test_setup().await;
    let store = SqliteIncidentStore::new(pool);

    let mut inc = sample_incident("sentry:7");
    inc.severity = Severity::Low;
    let first = store.ingest(&inc, json!({"attempt": 1})).await.unwrap();
    assert_eq!(first.record.severity, Severity::Low);

    let mut escalated = sample_incident("sentry:7");
    escalated.severity = Severity::Critical;
    let second = store
        .ingest(&escalated, json!({"attempt": 2}))
        .await
        .unwrap();
    assert!(second.was_duplicate);
    assert_eq!(second.record.id, first.record.id);
    assert_eq!(second.record.severity, Severity::Critical);
    assert_eq!(second.record.raw_payload, json!({"attempt": 2}));

    // Round-trips through a fresh read.
    let fetched = store.get(first.record.id).await.unwrap();
    assert_eq!(fetched.severity, Severity::Critical);
    assert_eq!(fetched.raw_payload, json!({"attempt": 2}));
}

#[tokio::test]
async fn sqlite_reconcile_interrupted_moves_stranded_statuses() {
    // #284 boot reconcile: `analyzing → open`, `repairing → failed`,
    // everything else untouched; second pass is a no-op.
    let (pool, _guard) = test_setup().await;
    let store = SqliteIncidentStore::new(pool);

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
    for s in [
        IncidentStatus::Analyzing,
        IncidentStatus::AwaitingApproval,
        IncidentStatus::Repairing,
    ] {
        store.set_status(repairing, s).await.unwrap();
    }

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

    assert!(store.reconcile_interrupted().await.unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_terminal_row_frees_the_dedup_slot() {
    let (pool, _guard) = test_setup().await;
    let store = SqliteIncidentStore::new(pool);
    let inc = sample_incident("sentry:9");

    let first = store.ingest(&inc, json!({})).await.unwrap();
    store
        .set_status(first.record.id, IncidentStatus::Dismissed)
        .await
        .unwrap();

    // The partial unique index excludes terminal rows: a fresh incident opens.
    let second = store.ingest(&inc, json!({})).await.unwrap();
    assert!(!second.was_duplicate);
    assert_ne!(second.record.id, first.record.id);
    assert_eq!(store.list(None).await.unwrap().len(), 2);

    // Status filter.
    let dismissed = store.list(Some(IncidentStatus::Dismissed)).await.unwrap();
    assert_eq!(dismissed.len(), 1);
    assert_eq!(dismissed[0].id, first.record.id);
}

#[tokio::test]
async fn sqlite_fk_violations_surface_as_not_found() {
    let (pool, _guard) = test_setup().await;
    let store = SqliteIncidentStore::new(pool);

    let rca = RcaRecord {
        id: Uuid::new_v4(),
        incident_id: Uuid::new_v4(), // no such incident
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

    assert!(matches!(
        store.get(Uuid::new_v4()).await.unwrap_err(),
        IncidentStoreError::NotFound
    ));
    assert!(matches!(
        store
            .set_status(Uuid::new_v4(), IncidentStatus::Analyzing)
            .await
            .unwrap_err(),
        IncidentStoreError::NotFound
    ));
}
