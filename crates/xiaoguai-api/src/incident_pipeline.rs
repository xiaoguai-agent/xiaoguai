//! Incident self-healing pipeline — T6.3 (Analyst) + T6.4 (Executor),
//! `docs/plans/2026-06-10-self-healing.md` §2.3/§2.4/§2.5.
//!
//! Two governed agent turns over the persisted incident state:
//!
//! * [`IncidentPipeline::analyze`] — the **Analyst** turn runs in consult
//!   mode (read-only toolbox + `ConsultGate`, exactly the T5 layers from
//!   [`crate::consult`]), produces an RCA in the [`RcaDraft`] **JSON**
//!   contract (that struct's only parser is serde — there is no markdown
//!   parser; see [`parse_rca_reply`]), persists it, and moves the incident
//!   `open → analyzing → awaiting_approval`. Any run/parse failure drops
//!   the incident back to `open` (retryable) and audits
//!   `incident.analysis_failed`.
//! * [`IncidentPipeline::approve_repair`] — the explicit human approval
//!   point. The request names the RCA being approved (`rca_id`, #284) and
//!   it must be the incident's latest one — the approval binds to that
//!   analysis, not to the incident. Moves `awaiting_approval → repairing`
//!   (the store transition IS the guard — anything else is a 409 at the
//!   route), runs the
//!   **Executor** turn in normal execute mode (full toolbox, the
//!   configured `HotL` gate rides in on `agent_defaults`), records the
//!   attempt, and lands on `resolved` / `failed` with the matching audit.
//!
//! Both turns run **in-process** via [`xiaoguai_runtime::run_to_completion`]
//! — the same single-turn mechanism `OrchestrateMemberRunner` uses (no
//! session HTTP, no SSE). The routes `await` the pipeline directly: the
//! runs are single agent turns, mirroring how the orchestrate handler keeps
//! the request open for the whole run; nothing here needs a detached task.
//! The flip side (#284): a client disconnect drops the handler future and
//! cancels the turn mid-flight, stranding the incident on
//! `analyzing`/`repairing` — `IncidentStore::reconcile_interrupted` runs at
//! boot (the `xiaoguai-core` serve path) to recover those rows.
//!
//! Attribution: every model call is stamped `incident:<id>` (see
//! [`incident_attribution_label`]) — disjoint from `sess_*`, `orch:`,
//! `scheduler:`, and `im:` labels, so exact-match budget sums never absorb
//! incident usage.
//!
//! The pipeline holds only `Arc`s/clones and is **constructed per request**
//! by the routes from existing `AppState` fields — deliberately NOT another
//! `AppState` field (which would touch every test fixture for zero gain;
//! construction is a handful of `Arc::clone`s).

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use xiaoguai_agent::{AgentConfig, StopReason, Toolbox};
use xiaoguai_llm::{LlmBackend, Message as LlmMessage};
use xiaoguai_runtime::{run_to_completion, RuntimeContext};

use crate::consult::{consult_agent_config, read_only_toolbox};
use crate::hotl::audit::HotlAuditSink;
use crate::incident_store::{
    IncidentDetails, IncidentRecord, IncidentStatus, IncidentStore, IncidentStoreError, RcaRecord,
    RepairRecord,
};
use crate::incidents::{render_rca_markdown, truncate_str, Incident, RcaDraft};

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Token-usage attribution label for both pipeline turns.
///
/// Contract: the `incident:` prefix is disjoint from session ids (`sess_*`)
/// and from the other synthetic labels (`orch:<run>:<persona>`,
/// `scheduler:<job_id>`, `im:<provider>:<conv>`) — exact-match budget sums
/// stay unaffected. Pure.
#[must_use]
pub fn incident_attribution_label(incident_id: Uuid) -> String {
    format!("incident:{incident_id}")
}

/// Max bytes of raw alert payload injected into the Analyst prompt
/// (#284). The payload is attacker-influenceable (end users of the
/// monitored app reach error messages / user agents without any webhook
/// token), so cap it to bound both token blowup and the injection
/// surface. ~8 KiB keeps every realistic Sentry/Datadog payload intact.
pub const MAX_RAW_PAYLOAD_PROMPT_BYTES: usize = 8 * 1024;

/// Build the Analyst prompt: the incident's high-signal fields + raw
/// payload (truncated to [`MAX_RAW_PAYLOAD_PROMPT_BYTES`], #284), an
/// investigate-read-only instruction, and the **exact JSON
/// contract [`RcaDraft`] deserializes** (its only parser is serde JSON —
/// `incidents.rs` has no markdown parser, so the reply contract is a single
/// JSON object, not headings). Pure.
#[must_use]
pub fn build_analyst_prompt(incident: &IncidentRecord) -> String {
    let raw_full = incident.raw_payload.to_string();
    let raw_capped = truncate_str(&raw_full, MAX_RAW_PAYLOAD_PROMPT_BYTES);
    let truncation_note = if raw_capped.len() < raw_full.len() {
        "\n[payload truncated]"
    } else {
        ""
    };
    format!(
        "You are the incident Analyst. Investigate the incident below using \
         your read-only tools and produce a root-cause analysis. You cannot \
         and must not mutate anything in this phase.\n\
         \n\
         Incident:\n\
         - id: {id}\n\
         - source: {source} ({external_id})\n\
         - title: {title}\n\
         - severity: {severity:?}\n\
         - project: {project}\n\
         - environment: {environment}\n\
         - occurred_at: {occurred_at}\n\
         \n\
         Raw alert payload:\n\
         {raw}{truncation_note}\n\
         \n\
         Reply with ONLY one JSON object — no prose before or after, no \
         markdown fences — with exactly these fields:\n\
         {{\n\
           \"summary\": \"one-paragraph incident summary\",\n\
           \"impact\": \"who/what was affected and for how long\",\n\
           \"root_cause\": \"the underlying cause\",\n\
           \"timeline\": [{{\"time\": \"ISO-8601\", \"event\": \"what happened\"}}],\n\
           \"action_items\": [{{\"assignee\": \"who\", \"action\": \"what to do\", \"priority\": \"P0|P1|P2\"}}],\n\
           \"confidence\": \"high|medium|low\",\n\
           \"evidence_refs\": [\"commit:..., log:..., etc.\"]\n\
         }}",
        id = incident.id,
        source = incident.source,
        external_id = incident.external_id,
        title = incident.title,
        severity = incident.severity,
        project = incident.project,
        environment = incident.environment.as_deref().unwrap_or("unknown"),
        occurred_at = incident.occurred_at,
        raw = raw_capped,
    )
}

/// Build the Executor prompt: the repair goal, the approved RCA (root
/// cause and action items), and the safety contract — checkpoint before
/// mutations, report exactly what was done. Pure.
#[must_use]
pub fn build_executor_prompt(incident: &IncidentRecord, rca: &RcaRecord) -> String {
    format!(
        "You are the incident Executor. The owner has APPROVED repairing the \
         incident below according to the root-cause analysis. Apply the \
         action items using your tools.\n\
         \n\
         Incident:\n\
         - id: {id}\n\
         - title: {title}\n\
         - severity: {severity:?}\n\
         - project: {project}\n\
         - environment: {environment}\n\
         \n\
         Approved RCA (confidence {confidence:.2}):\n\
         - summary: {summary}\n\
         - root cause: {root_cause}\n\
         \n\
         Action items:\n\
         {action_items}\n\
         \n\
         Safety contract:\n\
         1. BEFORE any mutation, create a workspace checkpoint with the \
         checkpoint tool (when available) so the change can be rolled back.\n\
         2. Apply only what the action items require — no opportunistic \
         changes.\n\
         3. Finish with a plain-text report of exactly what was done and \
         how to verify it.",
        id = incident.id,
        title = incident.title,
        severity = incident.severity,
        project = incident.project,
        environment = incident.environment.as_deref().unwrap_or("unknown"),
        confidence = rca.confidence,
        summary = rca.summary,
        root_cause = rca.root_cause,
        action_items = rca.action_items,
    )
}

/// Parse the Analyst reply against the [`RcaDraft`] serde contract.
/// Tolerates a json-fenced block or surrounding prose (first `{` to
/// last `}`) — models add both despite instructions. Pure.
///
/// # Errors
/// Returns the serde error message when no parseable JSON object matching
/// the contract is found.
pub fn parse_rca_reply(reply: &str) -> Result<RcaDraft, String> {
    let trimmed = reply.trim();
    if let Ok(draft) = serde_json::from_str::<RcaDraft>(trimmed) {
        return Ok(draft);
    }
    // Fallback: extract the outermost JSON object (handles fences/prose).
    let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) else {
        return Err("reply contains no JSON object".to_string());
    };
    if start >= end {
        return Err("reply contains no JSON object".to_string());
    }
    serde_json::from_str::<RcaDraft>(&trimmed[start..=end]).map_err(|e| e.to_string())
}

/// Map the [`RcaDraft`] qualitative confidence onto the numeric column
/// (`incident_rcas.confidence REAL`). Unknown labels land mid-scale. Pure.
#[must_use]
pub fn confidence_score(label: &str) -> f64 {
    match label.trim().to_ascii_lowercase().as_str() {
        "high" => 0.9,
        "medium" => 0.6,
        "low" => 0.3,
        _ => 0.5,
    }
}

// ---------------------------------------------------------------------------
// Report rendering (GLUE-4) — composes the EXISTING RCA renderer
// ---------------------------------------------------------------------------

/// Render the full incident report as markdown: status header, then the
/// shipped 5-section RCA body ([`render_rca_markdown`] — the existing
/// renderer, not a reimplementation), then a repairs section. No composite
/// renderer existed, so this pure fn assembles the pieces.
#[must_use]
pub fn render_incident_report(details: &IncidentDetails) -> String {
    let incident = &details.incident;
    let mut md = format!(
        "> **Status**: {status}\n\n",
        status = incident.status.as_str()
    );

    match details.rcas.first() {
        Some(rca) => {
            let draft = rca_draft_for_report(rca);
            md.push_str(&render_rca_markdown(
                &incident_for_report(incident),
                &draft,
                env!("CARGO_PKG_VERSION"),
            ));
        }
        None => {
            md.push_str(&format!(
                "# Incident RCA: {title}\n\n\
                 > **Incident ID**: `{external_id}`  \n\
                 > **Source**: {source}  \n\
                 > **Severity**: {severity:?}  \n\n\
                 _No RCA recorded yet — run `POST /v1/incidents/{id}/analyze`._\n",
                title = incident.title,
                external_id = incident.external_id,
                source = incident.source,
                severity = incident.severity,
                id = incident.id,
            ));
        }
    }

    md.push_str("\n## 6. Repairs\n\n");
    if details.repairs.is_empty() {
        md.push_str("_No repair attempts yet._\n");
    } else {
        md.push_str("| Time (UTC) | Outcome | Summary |\n|---|---|---|\n");
        for r in &details.repairs {
            md.push_str(&format!(
                "| `{}` | {} | {} |\n",
                r.created_at,
                if r.ok { "ok" } else { "failed" },
                r.summary.replace('\n', " "),
            ));
        }
    }
    md
}

/// Rebuild the normalizer-shape [`Incident`] the existing renderer takes
/// from the persisted row. `url` is not a stored column — the renderer
/// does not use it, so empty is fine.
fn incident_for_report(record: &IncidentRecord) -> Incident {
    Incident {
        id: record.external_id.clone(),
        title: record.title.clone(),
        severity: record.severity.clone(),
        source: record.source.clone(),
        occurred_at: record.occurred_at,
        url: String::new(),
        project: record.project.clone(),
        environment: record.environment.clone(),
        raw: record.raw_payload.clone(),
    }
}

/// Recover the [`RcaDraft`] for rendering: pipeline-written rows carry the
/// full JSON reply in `raw_markdown` (parse it back); rows seeded another
/// way fall back to the stored columns (impact/timeline/evidence lost).
fn rca_draft_for_report(rca: &RcaRecord) -> RcaDraft {
    parse_rca_reply(&rca.raw_markdown).unwrap_or_else(|_| RcaDraft {
        summary: rca.summary.clone(),
        impact: String::new(),
        root_cause: rca.root_cause.clone(),
        timeline: Vec::new(),
        action_items: serde_json::from_value(rca.action_items.clone()).unwrap_or_default(),
        confidence: format!("{:.2}", rca.confidence),
        evidence_refs: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PipelineError {
    /// Store-level failure — the route maps it through the same table as
    /// the T6.2 handlers (404 / 409 / 400 / 500).
    #[error(transparent)]
    Store(#[from] IncidentStoreError),
    /// The Analyst turn errored or stopped without a usable reply. The
    /// incident was reverted to `open` (retryable).
    #[error("analyst turn failed: {0}")]
    AnalysisRun(String),
    /// The Analyst reply did not match the `RcaDraft` JSON contract. The
    /// incident was reverted to `open` (retryable).
    #[error("analyst reply did not match the RCA contract: {0}")]
    RcaParse(String),
    /// `approve_repair` found no RCA on an `awaiting_approval` incident —
    /// state inconsistency (should be impossible through the pipeline).
    #[error("incident has no RCA to execute")]
    NoRca,
    /// #284: the `rca_id` in the approval request is not the incident's
    /// *latest* RCA — the owner approved a stale analysis. No state was
    /// changed; re-read the incident and approve the current RCA.
    #[error("rca_id {requested} is not the latest RCA ({latest}) for this incident")]
    StaleRca { requested: Uuid, latest: Uuid },
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Glue between the incident store and the two governed agent turns. Cheap
/// to construct (all `Arc`s) — the routes build one per request from
/// `AppState` fields.
pub struct IncidentPipeline {
    store: Arc<dyn IncidentStore>,
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    agent_defaults: AgentConfig,
    team_audit: Option<Arc<dyn HotlAuditSink>>,
}

impl IncidentPipeline {
    #[must_use]
    pub fn new(
        store: Arc<dyn IncidentStore>,
        backend: Arc<dyn LlmBackend>,
        toolbox: Arc<Toolbox>,
        agent_defaults: AgentConfig,
        team_audit: Option<Arc<dyn HotlAuditSink>>,
    ) -> Self {
        Self {
            store,
            backend,
            toolbox,
            agent_defaults,
            team_audit,
        }
    }

    /// GLUE-2 — the Analyst consult turn. See module docs for the flow.
    ///
    /// # Errors
    /// `Store` for transition/persistence failures (`InvalidTransition`
    /// when not `open` — 409 at the route); `AnalysisRun` / `RcaParse`
    /// after reverting the incident to `open`.
    pub async fn analyze(&self, incident_id: Uuid) -> Result<RcaRecord, PipelineError> {
        // open → analyzing; InvalidTransition surfaces as 409 at the route.
        let incident = self
            .store
            .set_status(incident_id, IncidentStatus::Analyzing)
            .await?;

        let label = incident_attribution_label(incident_id);
        // T5 consult layers: read-only toolbox (visibility) + ConsultGate
        // wrap (enforcement) — same composition as run_turn's consult mode.
        // #286: denials audit as `consult.denied` keyed on the incident's
        // attribution label (this turn has no chat session id).
        let consult_toolbox = Arc::new(read_only_toolbox(&self.toolbox));
        let consult_config = consult_agent_config(
            &self.agent_defaults,
            &self.toolbox,
            self.team_audit.clone(),
            &label,
        );
        let ctx = RuntimeContext::new(self.backend.clone(), consult_toolbox, consult_config)
            .with_attribution(Some(label.clone()), Some("owner".to_string()));

        let prompt = build_analyst_prompt(&incident);
        let outcome = run_to_completion(
            &ctx,
            vec![LlmMessage::user(prompt)],
            CancellationToken::new(),
        )
        .await;

        let reply = match outcome {
            Ok(o)
                if matches!(o.stop_reason, StopReason::Completed)
                    && !o.reply_text.trim().is_empty() =>
            {
                o.reply_text
            }
            Ok(o) => {
                let reason = format!(
                    "analyst turn stopped without a usable reply (stop_reason: {:?})",
                    o.stop_reason
                );
                self.analysis_failed(incident_id, &reason).await;
                return Err(PipelineError::AnalysisRun(reason));
            }
            Err(e) => {
                let reason = e.to_string();
                self.analysis_failed(incident_id, &reason).await;
                return Err(PipelineError::AnalysisRun(reason));
            }
        };

        let draft = match parse_rca_reply(&reply) {
            Ok(d) => d,
            Err(e) => {
                self.analysis_failed(incident_id, &e).await;
                return Err(PipelineError::RcaParse(e));
            }
        };

        let rca = RcaRecord {
            id: Uuid::new_v4(),
            incident_id,
            session_id: label,
            summary: draft.summary.clone(),
            root_cause: draft.root_cause.clone(),
            confidence: confidence_score(&draft.confidence),
            action_items: serde_json::to_value(&draft.action_items).unwrap_or_else(|_| json!([])),
            raw_markdown: reply,
            created_at: Utc::now(),
        };
        if let Err(e) = self.store.insert_rca(&rca).await {
            self.analysis_failed(incident_id, &format!("persisting RCA failed: {e}"))
                .await;
            return Err(e.into());
        }
        self.store
            .set_status(incident_id, IncidentStatus::AwaitingApproval)
            .await?;

        self.audit(
            "incident.analyzed",
            format!("incident:{incident_id}"),
            json!({
                "rca_id": rca.id,
                "confidence": rca.confidence,
                "root_cause": rca.root_cause,
            }),
        )
        .await;
        Ok(rca)
    }

    /// GLUE-3 — the human approval point + Executor execute turn. See
    /// module docs for the flow. A repair attempt that runs but does not
    /// succeed is NOT an `Err` — it is a recorded `ok: false` repair with
    /// the incident on `failed`.
    ///
    /// #284: the caller must name the RCA being approved (`rca_id`); the
    /// approval binds to *that analysis*, not to the incident. A mismatch
    /// against the latest RCA is rejected before any state transition.
    ///
    /// # Errors
    /// `StaleRca` when `rca_id` is not the incident's latest RCA (409 at
    /// the route, nothing transitioned); `Store` for
    /// transition/persistence failures (`InvalidTransition` unless
    /// `awaiting_approval` — 409 at the route); `NoRca` on state
    /// inconsistency.
    pub async fn approve_repair(
        &self,
        incident_id: Uuid,
        rca_id: Uuid,
    ) -> Result<RepairRecord, PipelineError> {
        // #284: validate the approval targets the LATEST RCA before any
        // transition. No newer RCA can appear afterwards: analyze requires
        // `open`, which is unreachable from `awaiting_approval`/`repairing`.
        let details = self.store.get_with_details(incident_id).await?;
        if let Some(latest) = details.rcas.first() {
            if latest.id != rca_id {
                return Err(PipelineError::StaleRca {
                    requested: rca_id,
                    latest: latest.id,
                });
            }
        }
        // awaiting_approval → repairing IS the approval guard.
        let incident = self
            .store
            .set_status(incident_id, IncidentStatus::Repairing)
            .await?;
        // Newest RCA first (store contract).
        let Some(rca) = details.rcas.first() else {
            // Inconsistent state — record the failed attempt path so the
            // incident does not stay stuck on `repairing`.
            let _ = self
                .store
                .set_status(incident_id, IncidentStatus::Failed)
                .await;
            self.audit(
                "incident.repair_failed",
                format!("incident:{incident_id}"),
                json!({"reason": "no RCA recorded"}),
            )
            .await;
            return Err(PipelineError::NoRca);
        };

        let label = incident_attribution_label(incident_id);
        // Execute mode: full toolbox, the configured HotL gate rides in on
        // `agent_defaults` (per-tool gating exactly like a normal turn).
        let ctx = RuntimeContext::new(
            self.backend.clone(),
            self.toolbox.clone(),
            self.agent_defaults.clone(),
        )
        .with_attribution(Some(label.clone()), Some("owner".to_string()));

        let prompt = build_executor_prompt(&incident, rca);
        let outcome = run_to_completion(
            &ctx,
            vec![LlmMessage::user(prompt)],
            CancellationToken::new(),
        )
        .await;

        let (ok, summary) = match outcome {
            Ok(o) => {
                let ok = matches!(o.stop_reason, StopReason::Completed)
                    && !o.reply_text.trim().is_empty();
                let summary = if o.reply_text.trim().is_empty() {
                    format!(
                        "executor produced no report (stop_reason: {:?})",
                        o.stop_reason
                    )
                } else {
                    o.reply_text
                };
                (ok, summary)
            }
            Err(e) => (false, format!("executor turn failed: {e}")),
        };

        let repair = RepairRecord {
            id: Uuid::new_v4(),
            incident_id,
            rca_id: rca.id,
            session_id: label,
            ok,
            summary,
            created_at: Utc::now(),
        };
        self.store.insert_repair(&repair).await?;
        let final_status = if ok {
            IncidentStatus::Resolved
        } else {
            IncidentStatus::Failed
        };
        self.store.set_status(incident_id, final_status).await?;

        self.audit(
            if ok {
                "incident.repaired"
            } else {
                "incident.repair_failed"
            },
            format!("incident:{incident_id}"),
            json!({
                "repair_id": repair.id,
                "rca_id": rca.id,
                "ok": ok,
            }),
        )
        .await;
        Ok(repair)
    }

    /// Best-effort failure path: revert to `open` (retryable per plan
    /// §2.3) and audit `incident.analysis_failed` with the reason.
    async fn analysis_failed(&self, incident_id: Uuid, reason: &str) {
        if let Err(e) = self
            .store
            .set_status(incident_id, IncidentStatus::Open)
            .await
        {
            tracing::warn!(
                error = %e, %incident_id,
                "incident pipeline: could not revert to open after analysis failure"
            );
        }
        self.audit(
            "incident.analysis_failed",
            format!("incident:{incident_id}"),
            json!({"reason": reason}),
        )
        .await;
    }

    /// Best-effort audit append — same posture as the T6.2 route helper:
    /// failures are logged, never propagated.
    async fn audit(&self, action: &str, resource: String, details: serde_json::Value) {
        crate::audit_util::audit_event(&self.team_audit, action, resource, details).await;
    }
}

// ---------------------------------------------------------------------------
// Unit tests (pure fns)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::incident_store::record_from_incident;
    use crate::incidents::Severity;

    fn fixture_record() -> IncidentRecord {
        let incident = Incident {
            id: "sentry:123".to_string(),
            title: "ZeroDivisionError: division by zero".to_string(),
            severity: Severity::High,
            source: "sentry".to_string(),
            occurred_at: Utc::now(),
            url: "https://sentry.example/123".to_string(),
            project: "backend".to_string(),
            environment: Some("production".to_string()),
            raw: json!({"data": {"issue": {"id": "123"}}}),
        };
        record_from_incident(&incident, incident.raw.clone())
    }

    fn fixture_rca(incident_id: Uuid, raw_markdown: &str) -> RcaRecord {
        RcaRecord {
            id: Uuid::new_v4(),
            incident_id,
            session_id: incident_attribution_label(incident_id),
            summary: "Division guard missing".to_string(),
            root_cause: "Empty cart divides by zero".to_string(),
            confidence: 0.9,
            action_items: json!([{"assignee": "backend", "action": "add guard", "priority": "P0"}]),
            raw_markdown: raw_markdown.to_string(),
            created_at: Utc::now(),
        }
    }

    fn rca_json() -> String {
        json!({
            "summary": "Payment processor crashed on empty carts.",
            "impact": "200 users for 12 minutes.",
            "root_cause": "Divide-by-zero in discount_calc.",
            "timeline": [{"time": "2026-06-10T01:00:00Z", "event": "deploy"}],
            "action_items": [{"assignee": "backend", "action": "add guard", "priority": "P0"}],
            "confidence": "high",
            "evidence_refs": ["commit:abc123"]
        })
        .to_string()
    }

    // ── attribution label contract ──────────────────────────────────────────

    #[test]
    fn attribution_label_is_exact_format() {
        let id = Uuid::nil();
        assert_eq!(incident_attribution_label(id), format!("incident:{id}"));
    }

    #[test]
    fn attribution_label_prefix_is_disjoint_from_other_labels() {
        let label = incident_attribution_label(Uuid::new_v4());
        assert!(label.starts_with("incident:"), "got {label}");
        for foreign in ["sess_", "orch:", "scheduler:", "im:"] {
            assert!(
                !label.starts_with(foreign),
                "{label} collides with {foreign}"
            );
        }
    }

    // ── analyst prompt ──────────────────────────────────────────────────────

    #[test]
    fn analyst_prompt_carries_incident_fields() {
        let record = fixture_record();
        let prompt = build_analyst_prompt(&record);
        assert!(prompt.contains(&record.id.to_string()));
        assert!(prompt.contains("ZeroDivisionError: division by zero"));
        assert!(prompt.contains("sentry:123"));
        assert!(prompt.contains("backend"));
        assert!(prompt.contains("production"));
        // Raw payload is injected for agent context.
        assert!(prompt.contains("\"issue\""));
    }

    #[test]
    fn analyst_prompt_truncates_oversized_raw_payload() {
        // #284: the raw payload is attacker-influenceable — an oversized
        // one must be capped, with a visible truncation marker.
        let mut record = fixture_record();
        // Array keeps serialization order regardless of serde_json's map
        // representation: the marker is guaranteed to sit past the cap.
        record.raw_payload = json!([
            "x".repeat(MAX_RAW_PAYLOAD_PROMPT_BYTES * 2),
            "SHOULD-NOT-APPEAR",
        ]);
        let prompt = build_analyst_prompt(&record);
        assert!(prompt.contains("[payload truncated]"));
        assert!(!prompt.contains("SHOULD-NOT-APPEAR"));
        // The prompt stays bounded: payload cap + fixed template slack.
        assert!(prompt.len() < MAX_RAW_PAYLOAD_PROMPT_BYTES + 4096);

        // Small payloads pass through whole, no marker.
        let small = fixture_record();
        let prompt = build_analyst_prompt(&small);
        assert!(prompt.contains("\"issue\""));
        assert!(!prompt.contains("[payload truncated]"));
    }

    #[test]
    fn analyst_prompt_states_the_rca_draft_json_contract() {
        // The contract MUST match what `parse_rca_reply` (serde RcaDraft)
        // accepts — every field name, spelled exactly.
        let prompt = build_analyst_prompt(&fixture_record());
        for field in [
            "\"summary\"",
            "\"impact\"",
            "\"root_cause\"",
            "\"timeline\"",
            "\"action_items\"",
            "\"confidence\"",
            "\"evidence_refs\"",
        ] {
            assert!(prompt.contains(field), "prompt missing {field}");
        }
        assert!(prompt.contains("JSON object"));
        // Consult posture is spelled out.
        assert!(prompt.contains("read-only"));
    }

    // ── executor prompt ─────────────────────────────────────────────────────

    #[test]
    fn executor_prompt_carries_goal_rca_and_checkpoint_instruction() {
        let record = fixture_record();
        let rca = fixture_rca(record.id, &rca_json());
        let prompt = build_executor_prompt(&record, &rca);
        assert!(prompt.contains("ZeroDivisionError: division by zero"));
        assert!(prompt.contains("Empty cart divides by zero"));
        assert!(prompt.contains("add guard"));
        assert!(prompt.contains("checkpoint"));
        assert!(prompt.contains("report"));
    }

    // ── reply parsing ───────────────────────────────────────────────────────

    #[test]
    fn parse_accepts_bare_json() {
        let draft = parse_rca_reply(&rca_json()).expect("bare JSON parses");
        assert_eq!(draft.confidence, "high");
        assert_eq!(draft.action_items.len(), 1);
    }

    #[test]
    fn parse_accepts_fenced_or_prose_wrapped_json() {
        let fenced = format!("```json\n{}\n```", rca_json());
        assert!(parse_rca_reply(&fenced).is_ok(), "fenced JSON must parse");
        let prose = format!("Here is the RCA:\n{}\nLet me know!", rca_json());
        assert!(
            parse_rca_reply(&prose).is_ok(),
            "prose-wrapped JSON must parse"
        );
    }

    #[test]
    fn parse_rejects_non_contract_replies() {
        assert!(parse_rca_reply("I cannot determine the root cause.").is_err());
        assert!(parse_rca_reply("{\"summary\": \"missing the rest\"}").is_err());
        assert!(parse_rca_reply("").is_err());
    }

    // ── confidence mapping ──────────────────────────────────────────────────

    #[test]
    fn confidence_labels_map_onto_the_numeric_column() {
        assert!((confidence_score("high") - 0.9).abs() < f64::EPSILON);
        assert!((confidence_score("MEDIUM") - 0.6).abs() < f64::EPSILON);
        assert!((confidence_score(" low ") - 0.3).abs() < f64::EPSILON);
        assert!((confidence_score("certain") - 0.5).abs() < f64::EPSILON);
    }

    // ── report rendering ────────────────────────────────────────────────────

    #[test]
    fn report_composes_existing_rca_renderer_plus_repairs() {
        let record = fixture_record();
        let rca = fixture_rca(record.id, &rca_json());
        let details = IncidentDetails {
            incident: record.clone(),
            rcas: vec![rca.clone()],
            repairs: vec![RepairRecord {
                id: Uuid::new_v4(),
                incident_id: record.id,
                rca_id: rca.id,
                session_id: incident_attribution_label(record.id),
                ok: true,
                summary: "guard added".to_string(),
                created_at: Utc::now(),
            }],
        };
        let md = render_incident_report(&details);
        // Existing renderer's section headings, not reinvented ones.
        assert!(md.contains("# Incident RCA: ZeroDivisionError"));
        assert!(md.contains("## 1. Summary"));
        assert!(md.contains("Payment processor crashed on empty carts."));
        assert!(md.contains("## 5. Action Items"));
        // The composed repairs section.
        assert!(md.contains("## 6. Repairs"));
        assert!(md.contains("guard added"));
        assert!(md.contains("> **Status**: open"));
    }

    #[test]
    fn report_without_rca_says_so_and_still_renders() {
        let record = fixture_record();
        let details = IncidentDetails {
            incident: record.clone(),
            rcas: vec![],
            repairs: vec![],
        };
        let md = render_incident_report(&details);
        assert!(md.contains(&record.title));
        assert!(md.contains("No RCA recorded yet"));
        assert!(md.contains("No repair attempts yet"));
    }

    #[test]
    fn report_falls_back_to_stored_columns_for_foreign_raw_markdown() {
        // RCA seeded outside the pipeline: raw_markdown is not JSON.
        let record = fixture_record();
        let rca = fixture_rca(record.id, "## RCA\nhand-written");
        let details = IncidentDetails {
            incident: record,
            rcas: vec![rca],
            repairs: vec![],
        };
        let md = render_incident_report(&details);
        assert!(md.contains("Division guard missing"));
        assert!(md.contains("Empty cart divides by zero"));
    }
}
