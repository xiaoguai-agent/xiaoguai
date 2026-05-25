# Compliance Gaps — Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai wave-3 (v1.3.x-prep / main @ 9970aa0)

This document is an honest inventory of NOT-YET-DONE compliance items as of wave-3.
Each gap names the responsible component, the relevant control, and a tracking note.

---

## Gap 1 — Right-to-Erasure Cascade Is Incomplete

**Controls affected**: GDPR Art. 17; SOC 2 CC6.3
**Severity**: High — blocks full GDPR erasure response

**What works**: `DELETE /v1/sessions/:id` cascades to the `messages` table.

**What is missing**:
1. `agent_outcomes` rows for the deleted session are **not deleted**. The `session_id` foreign key in `agent_outcomes` must be cascaded or the rows must be nulled.
2. `audit_chain` rows referencing the deleted session cannot be deleted (HMAC chain integrity). A **redaction path** is needed: replace PII fields in `details` JSON with `[redacted]` while re-computing HMACs for the affected tail. This is architecturally non-trivial.
3. Valkey IM dedup cache entries keyed by `(tenant_id, message_hash)` are not evicted on session/user deletion.
4. IM platform message history (Slack, Feishu, etc.) is not within Xiaoguai's deletion scope; operator must use platform-specific deletion APIs.

**Responsible component**: `xiaoguai-storage` (outcomes cascade), `xiaoguai-audit` (redaction path), `xiaoguai-im-gateway` (cache eviction).

**Tracking note**: No issue filed as of wave-3. Priority: must be resolved before any GDPR-regulated production deployment. Suggested approach: add `ON DELETE CASCADE` to `agent_outcomes.session_id` FK (migration); design `audit_chain::redact_entry()` that nulls `details` and re-seals the tail.

---

## Gap 2 — No Automated Retention Enforcement

**Controls affected**: GDPR Art. 5(1)(e) (storage limitation); SOC 2 CC6.3
**Severity**: Medium — operators must implement manually

**What works**: Retention period is configurable in the operator DPIA.

**What is missing**: Xiaoguai has no built-in cron job or scheduler job that automatically deletes sessions/messages/outcomes older than a configured retention window. The `xiaoguai-scheduler` crate exists and could host this, but no retention-enforcement job is wired.

**Responsible component**: `xiaoguai-scheduler` + `xiaoguai-storage` repository layer.

**Tracking note**: Suggested implementation — add a `RetentionEnforcerJob` that runs nightly, reads `retention_days` from tenant config, and issues batched `DELETE FROM sessions WHERE created_at < now() - interval '$n days' AND tenant_id = $t`. Must be opt-in per tenant to avoid surprising operators.

---

## Gap 3 — Hourly Outcome Bucketing Not Implemented

**Controls affected**: SOC 2 CC7.2 (continuous monitoring depth); internal ROI dashboard accuracy
**Severity**: Low — functional but coarse

**What works**: `OutcomeRecorder.record()` writes individual rows with a `recorded_at` timestamp.

**What is missing**: The admin UI Outcomes pane and Grafana dashboards currently aggregate over arbitrary time ranges using simple SUM queries. There is no pre-computed hourly/daily bucketing table (e.g. `agent_outcomes_hourly`). For tenants with high outcome volumes this will cause slow dashboard queries.

**Responsible component**: `xiaoguai-storage` (migration for bucketed view), `xiaoguai-core::outcomes_bridge` (background aggregation job).

**Tracking note**: Lower priority than erasure. Can be addressed with a Postgres materialized view refreshed hourly via `xiaoguai-scheduler`.

---

## Gap 4 — No DPA / RoPA Template Provided

**Controls affected**: GDPR Art. 28 (Data Processing Agreement), Art. 30 (Records of Processing Activities); SOC 2 CC2.3
**Severity**: Medium — operators must draft their own

**What works**: The DPIA template (`docs/compliance/gdpr/dpia-template.md`) covers internal risk assessment.

**What is missing**:
- A **Data Processing Agreement (DPA)** template that operators can execute with Xiaoguai (if they treat the Xiaoguai project team as a processor) or adapt for their own sub-processors.
- A **Records of Processing Activities (RoPA)** template that operators can complete to satisfy Art. 30 obligations using the technical artefacts Xiaoguai produces (audit chain + outcomes table).

**Responsible component**: Documentation / legal (not a code gap).

**Tracking note**: Create `docs/compliance/gdpr/dpa-template.md` and `docs/compliance/gdpr/ropa-template.md`. Should be reviewed by a qualified DPO before publication.

---

## Gap 5 — PgHotlPolicyStore and PgSkillPackRepository Not Wired

**Controls affected**: SOC 2 CC3.4, CC5.3, CC8.1 (production change tracking)
**Severity**: Medium — in-memory implementations work in tests, not in production

**What works**: `InMemoryHotlPolicyStore` and `InMemorySkillPackRepository` are fully tested. The API routes exist.

**What is missing**: Production Postgres-backed implementations (`PgHotlPolicyStore`, `PgSkillPackRepository`) are not yet wired in `xiaoguai-core`. Without them, HotL policies and skill-pack installations do not survive process restarts.

**Responsible component**: `xiaoguai-core` (bridge files, similar to `outcomes_bridge.rs` pattern).

**Tracking note**: High-priority for any staging/production deployment. Implementation pattern is established by `PgOutcomeRecorder` in `xiaoguai-core/src/outcomes_bridge.rs`.

---

## Gap 6 — Audit Log Retention / Archival Not Defined

**Controls affected**: GDPR Art. 30 (records of processing); SOC 2 CC6.6, CC7.2
**Severity**: Low — functional but unbounded

**What works**: Audit log is append-only with HMAC chain integrity.

**What is missing**: No defined maximum retention window or archival strategy for `audit_chain` rows. In long-running deployments the table will grow without bound. GDPR erasure tension (see Gap 1) makes a simple `DELETE` approach invalid.

**Responsible component**: `xiaoguai-audit` (archival sink), `xiaoguai-storage` (migration for partitioned audit table).

**Tracking note**: Suggested approach — Postgres table partitioning by month on `ts`; detach and archive cold partitions to S3/object storage with encryption. Redaction (Gap 1) must be solved first.

---

## Gap 7 — Breach Notification Template Not Provided

**Controls affected**: GDPR Art. 33 (breach notification to DPA); Art. 34 (notification to data subjects)
**Severity**: Low — detection works; formal response template is missing

**What works**: `xiaoguai-anomaly` fires alerts; IM adapters deliver them within minutes.

**What is missing**: A structured breach notification template that operators can adapt to notify:
- Their supervisory authority (DPA) within 72 hours (Art. 33)
- Affected data subjects when risk is high (Art. 34)

**Responsible component**: Documentation (not a code gap).

**Tracking note**: Create `docs/compliance/gdpr/breach-notification-template.md`.

---

## Gap Summary

| # | Gap | GDPR | SOC 2 | Severity | Owner |
|---|-----|:----:|:-----:|:--------:|-------|
| 1 | Right-to-erasure cascade | Art. 17 | CC6.3 | High | `xiaoguai-storage`, `xiaoguai-audit` |
| 2 | Automated retention enforcement | Art. 5(1)(e) | CC6.3 | Medium | `xiaoguai-scheduler` |
| 3 | Hourly outcome bucketing | — | CC7.2 | Low | `xiaoguai-storage` |
| 4 | DPA / RoPA template | Art. 28, 30 | CC2.3 | Medium | Docs / legal |
| 5 | PgHotlPolicyStore + PgSkillPackRepository wired | — | CC3.4, CC5.3, CC8.1 | Medium | `xiaoguai-core` |
| 6 | Audit log archival strategy | Art. 30 | CC6.6 | Low | `xiaoguai-audit` |
| 7 | Breach notification template | Art. 33, 34 | — | Low | Docs / legal |

**Total gaps: 7** (1 High, 3 Medium, 3 Low)
