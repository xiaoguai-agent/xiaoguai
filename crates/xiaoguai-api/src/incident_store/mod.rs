//! Incident persistence — T6.1 (docs/plans/2026-06-10-self-healing.md §2.1).
//!
//! Mirrors the loops precedent (`xiaoguai-storage::repositories::loops`):
//! a small store trait the routes/pipeline consume, plain row types, and
//! one impl per backing store. It lives in `xiaoguai-api` (not
//! `xiaoguai-storage`) because the ingest contract is the existing
//! [`crate::incidents::Incident`] normalizer output — storage must stay
//! api-free.
//!
//! Status machine (plan §2.1):
//!
//! ```text
//! open → analyzing → awaiting_approval → repairing → resolved | failed
//!   ↑________|                                  (any non-terminal) → dismissed
//!  (analysis failure)
//! ```
//!
//! Dedup: at most one *live* (non-terminal) row per `(source, external_id)`
//! — a re-fired alert bumps `updated_at` on the existing row instead of
//! opening a twin ([`IncidentStore::ingest`] reports it via
//! [`IngestOutcome::was_duplicate`]).

pub mod memory;
pub mod sqlite;

pub use memory::InMemoryIncidentStore;
pub use sqlite::SqliteIncidentStore;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::incidents::{Incident, Severity};

// ---------------------------------------------------------------------------
// Status machine
// ---------------------------------------------------------------------------

/// Incident lifecycle states. `Resolved`, `Failed`, and `Dismissed` are
/// terminal; the rest are live and hold the dedup slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Open,
    Analyzing,
    AwaitingApproval,
    Repairing,
    Resolved,
    Failed,
    Dismissed,
}

impl IncidentStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Analyzing => "analyzing",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Repairing => "repairing",
            Self::Resolved => "resolved",
            Self::Failed => "failed",
            Self::Dismissed => "dismissed",
        }
    }

    /// Parse the wire/DB string. Returns `None` for unknown values.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "analyzing" => Some(Self::Analyzing),
            "awaiting_approval" => Some(Self::AwaitingApproval),
            "repairing" => Some(Self::Repairing),
            "resolved" => Some(Self::Resolved),
            "failed" => Some(Self::Failed),
            "dismissed" => Some(Self::Dismissed),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Resolved | Self::Failed | Self::Dismissed)
    }

    /// Legal transitions (plan §2.1 / task spec):
    /// `open→analyzing→awaiting_approval→repairing→resolved|failed`;
    /// any non-terminal → `dismissed`; `analyzing→open` on analysis failure.
    #[must_use]
    pub fn can_transition_to(self, to: Self) -> bool {
        match (self, to) {
            (Self::Open, Self::Analyzing)
            | (Self::Analyzing, Self::AwaitingApproval | Self::Open)
            | (Self::AwaitingApproval, Self::Repairing)
            | (Self::Repairing, Self::Resolved | Self::Failed) => true,
            (from, Self::Dismissed) => !from.is_terminal(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Records (mirror migration 0033)
// ---------------------------------------------------------------------------

/// One persisted incident (`incidents` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub id: Uuid,
    /// Source platform identifier (`"sentry"`, `"datadog"`, `"manual"`, …).
    pub source: String,
    /// Vendor-scoped id from the normalizer ([`Incident::id`], e.g.
    /// `"sentry:123"`). Dedup key together with `source`.
    pub external_id: String,
    pub title: String,
    pub severity: Severity,
    pub project: String,
    pub environment: Option<String>,
    pub occurred_at: DateTime<Utc>,
    /// Full raw webhook payload preserved for agent context injection.
    pub raw_payload: Value,
    pub status: IncidentStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One Analyst RCA pass (`incident_rcas` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RcaRecord {
    pub id: Uuid,
    pub incident_id: Uuid,
    /// Session the Analyst consult turn ran in (`incident:<id>` attribution).
    pub session_id: String,
    pub summary: String,
    pub root_cause: String,
    pub confidence: f64,
    /// JSON array of action items (the `RcaDraft` contract).
    pub action_items: Value,
    pub raw_markdown: String,
    pub created_at: DateTime<Utc>,
}

/// One Executor repair attempt (`incident_repairs` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairRecord {
    pub id: Uuid,
    pub incident_id: Uuid,
    /// The RCA whose action items this repair executed.
    pub rca_id: Uuid,
    pub session_id: String,
    pub ok: bool,
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

/// [`IncidentStore::ingest`] result: the persisted (or pre-existing) row
/// plus whether dedup matched an existing live incident.
#[derive(Debug, Clone, Serialize)]
pub struct IngestOutcome {
    pub record: IncidentRecord,
    pub was_duplicate: bool,
}

/// [`IncidentStore::get_with_details`] result — incident joined with its
/// RCA and repair history (each newest first).
#[derive(Debug, Clone, Serialize)]
pub struct IncidentDetails {
    pub incident: IncidentRecord,
    pub rcas: Vec<RcaRecord>,
    pub repairs: Vec<RepairRecord>,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum IncidentStoreError {
    #[error("not found")]
    NotFound,
    #[error("illegal status transition: {from} → {to}")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("backend: {0}")]
    Backend(String),
}

pub type IncidentStoreResult<T> = Result<T, IncidentStoreError>;

// ---------------------------------------------------------------------------
// Store trait
// ---------------------------------------------------------------------------

/// Persistence boundary for incidents. The ingest route (T6.2) and the
/// `IncidentPipeline` (T6.3/T6.4) consume this trait; production wires
/// [`SqliteIncidentStore`], tests use [`InMemoryIncidentStore`].
#[async_trait]
pub trait IncidentStore: Send + Sync {
    /// Dedup-upsert a normalized incident. When a *live* (non-terminal)
    /// row already exists for `(incident.source, incident.id)`, its
    /// `updated_at` is bumped and it is returned with
    /// `was_duplicate = true`; otherwise a fresh `open` row is inserted.
    async fn ingest(&self, incident: &Incident, raw: Value) -> IncidentStoreResult<IngestOutcome>;

    /// Fetch one incident. `NotFound` if absent.
    async fn get(&self, id: Uuid) -> IncidentStoreResult<IncidentRecord>;

    /// All incidents, newest first, optionally filtered by status.
    async fn list(
        &self,
        status: Option<IncidentStatus>,
    ) -> IncidentStoreResult<Vec<IncidentRecord>>;

    /// Move an incident to `to`, validating the transition against
    /// [`IncidentStatus::can_transition_to`]. Returns the updated row;
    /// `InvalidTransition` when illegal, `NotFound` when absent.
    async fn set_status(&self, id: Uuid, to: IncidentStatus)
        -> IncidentStoreResult<IncidentRecord>;

    /// Append an Analyst RCA. `NotFound` if the incident is absent.
    async fn insert_rca(&self, rca: &RcaRecord) -> IncidentStoreResult<()>;

    /// Append an Executor repair attempt. `NotFound` if the incident or
    /// referenced RCA is absent.
    async fn insert_repair(&self, repair: &RepairRecord) -> IncidentStoreResult<()>;

    /// Incident joined with its RCA + repair history (each newest first).
    async fn get_with_details(&self, id: Uuid) -> IncidentStoreResult<IncidentDetails>;
}

// ---------------------------------------------------------------------------
// Severity wire helpers (the DB stores the serde lowercase form)
// ---------------------------------------------------------------------------

#[must_use]
pub(crate) fn severity_as_str(s: &Severity) -> &'static str {
    match s {
        Severity::Critical => "critical",
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
    }
}

pub(crate) fn severity_parse(s: &str) -> Option<Severity> {
    match s {
        "critical" => Some(Severity::Critical),
        "high" => Some(Severity::High),
        "medium" => Some(Severity::Medium),
        "low" => Some(Severity::Low),
        _ => None,
    }
}

/// Build a fresh `open` [`IncidentRecord`] from a normalized [`Incident`].
/// Shared by both store impls so the field mapping cannot drift.
#[must_use]
pub(crate) fn record_from_incident(incident: &Incident, raw: Value) -> IncidentRecord {
    let now = Utc::now();
    IncidentRecord {
        id: Uuid::new_v4(),
        source: incident.source.clone(),
        external_id: incident.id.clone(),
        title: incident.title.clone(),
        severity: incident.severity.clone(),
        project: incident.project.clone(),
        environment: incident.environment.clone(),
        occurred_at: incident.occurred_at,
        raw_payload: raw,
        status: IncidentStatus::Open,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::IncidentStatus::{
        self, Analyzing, AwaitingApproval, Dismissed, Failed, Open, Repairing, Resolved,
    };

    const ALL: [IncidentStatus; 7] = [
        Open,
        Analyzing,
        AwaitingApproval,
        Repairing,
        Resolved,
        Failed,
        Dismissed,
    ];

    #[test]
    fn happy_path_transitions_are_legal() {
        assert!(Open.can_transition_to(Analyzing));
        assert!(Analyzing.can_transition_to(AwaitingApproval));
        assert!(AwaitingApproval.can_transition_to(Repairing));
        assert!(Repairing.can_transition_to(Resolved));
        assert!(Repairing.can_transition_to(Failed));
    }

    #[test]
    fn analysis_failure_drops_back_to_open() {
        assert!(Analyzing.can_transition_to(Open));
        // …but no other state re-opens.
        assert!(!AwaitingApproval.can_transition_to(Open));
        assert!(!Repairing.can_transition_to(Open));
    }

    #[test]
    fn any_non_terminal_can_be_dismissed_terminal_cannot() {
        for s in ALL {
            assert_eq!(
                s.can_transition_to(Dismissed),
                !s.is_terminal(),
                "dismiss from {s:?}"
            );
        }
    }

    #[test]
    fn terminal_states_are_immutable() {
        for from in [Resolved, Failed, Dismissed] {
            for to in ALL {
                assert!(!from.can_transition_to(to), "{from:?} → {to:?}");
            }
        }
    }

    #[test]
    fn skipping_stages_is_illegal() {
        assert!(!Open.can_transition_to(AwaitingApproval));
        assert!(!Open.can_transition_to(Repairing));
        assert!(!Open.can_transition_to(Resolved));
        assert!(!Analyzing.can_transition_to(Repairing));
        assert!(!AwaitingApproval.can_transition_to(Resolved));
    }

    #[test]
    fn status_round_trips_through_wire_string() {
        for s in ALL {
            assert_eq!(IncidentStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(IncidentStatus::parse("nope"), None);
    }
}
