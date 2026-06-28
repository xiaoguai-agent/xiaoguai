//! `xiaoguai demo-seed` — populate the local `SQLite` store with lifelike
//! sample data so every admin/chat pane has something to show during a live
//! demo.
//!
//! Writes go straight at the **same** embedded store `xiaoguai serve` reads
//! (`settings.database.url`), the same pattern `provider` / `schedule` use, so
//! the data is visible the moment you point a browser at the running server —
//! no restart needed for the read-only panes.
//!
//! What it seeds (all tagged so `--reset` can find + remove only demo rows):
//! * **Audit chain** — ~12 rows appended through the real HMAC-chained
//!   [`SqliteAuditSink`], so the "activity history" verify badge is green.
//! * **Sessions + messages** — a couple of chats for Today / session lists.
//! * **Scheduled jobs** — a daily cron job + a webhook job.
//! * **`token_usage` baseline + spike** — a flat history with one obvious
//!   tail spike so an anomaly `fire-now` / back-test detects a z-score outlier.
//! * **Incident + RCA** — one resolved-loop incident with a root-cause
//!   analysis row for the Incidents pane.
//!
//! Idempotent: every run first clears prior demo rows (so re-running doesn't
//! pile up duplicates), then re-seeds. `--reset` clears and stops (leaves the
//! store demo-free). Non-demo data is never touched — the deletes are keyed on
//! the stable `demo_*` id prefixes / `source = 'demo'` markers below.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_audit::{AuditEntry, OWNER_TENANT_ID};
use xiaoguai_types::ContentBlock;

// ---------------------------------------------------------------------------
// Stable demo markers — every seeded row carries one of these so `clear_demo`
// removes exactly what `seed` wrote and nothing else (precise, reversible).
// ---------------------------------------------------------------------------

/// Prefix on every demo session id (and the messages/usage that reference it).
const DEMO_SESSION_PREFIX: &str = "demo_sess_";
/// Prefix on every demo scheduled-job id.
const DEMO_JOB_PREFIX: &str = "demo_job_";
/// `incidents.source` value used for demo rows (also matches the audit actor).
/// Demo incidents are keyed on this marker (their `id` is a real UUID, not a
/// text prefix), so `clear_demo` removes them precisely without touching real
/// incidents.
const DEMO_INCIDENT_SOURCE: &str = "demo";
/// `token_usage.request_id` marker so demo usage rows are removable without
/// disturbing real provider telemetry.
const DEMO_USAGE_REQUEST_ID: &str = "demo-seed";
/// Audit actor stamped on every demo audit row (informational — the audit
/// chain is append-only and is NOT cleared by `--reset`; see `clear_demo`).
const DEMO_AUDIT_ACTOR: &str = "cli:demo-seed";

/// Provider id the demo `token_usage` + audit cost rows attribute to. The
/// migrations seed a key-less `minimax` provider, so this resolves on a fresh
/// DB; the value is only a label here (no live call is made).
const DEMO_PROVIDER_ID: &str = "minimax";
const DEMO_MODEL: &str = "MiniMax-Text-01";

/// Baseline token-usage points to lay down before the spike. The default
/// Z-score detector needs `min_count: 10`, so a dozen flat points give it a
/// stable mean/σ to flag the spike against.
const BASELINE_POINTS: usize = 14;
/// Flat baseline magnitude (total tokens per request).
const BASELINE_TOKENS: i64 = 1_000;
/// Spike magnitude — ~8× the baseline, far past a 3σ threshold on a flat
/// series, so the anomaly fires unambiguously on stage.
const SPIKE_TOKENS: i64 = 8_200;

// ---------------------------------------------------------------------------
// Public entry — `run` (seed) / `clear_demo` (reset)
// ---------------------------------------------------------------------------

/// Outcome summary returned to `main.rs` so it can print the "what got seeded"
/// guide. Pure data — keeps this module testable without capturing stdout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub audit_rows: usize,
    pub sessions: usize,
    pub messages: usize,
    pub jobs: usize,
    pub usage_rows: usize,
    pub incidents: usize,
    pub baseline_tokens: i64,
    pub spike_tokens: i64,
}

/// Clear any prior demo data, then seed a fresh lifelike snapshot.
///
/// `audit` signs the demo audit rows into the real HMAC chain (so the verify
/// badge stays green). `now` is injected for deterministic tests.
///
/// # Errors
/// Returns an error if any SQL write or audit append fails. On a partial
/// failure the rows written so far remain; re-running cleans + reseeds.
pub async fn seed(
    pool: &SqlitePool,
    audit: &SqliteAuditSink,
    owner_user_id: &str,
    now: DateTime<Utc>,
) -> Result<SeedReport> {
    clear_demo(pool).await.context("clear prior demo data")?;

    let (sessions, messages) = seed_sessions(pool, owner_user_id, now)
        .await
        .context("seed sessions")?;
    let jobs = seed_jobs(pool, now).await.context("seed scheduled jobs")?;
    let usage_rows = seed_token_usage(pool, owner_user_id, now)
        .await
        .context("seed token_usage")?;
    let incidents = seed_incident(pool, now).await.context("seed incident")?;
    let audit_rows = seed_audit(audit, owner_user_id, now)
        .await
        .context("seed audit chain")?;

    Ok(SeedReport {
        audit_rows,
        sessions,
        messages,
        jobs,
        usage_rows,
        incidents,
        baseline_tokens: BASELINE_TOKENS,
        spike_tokens: SPIKE_TOKENS,
    })
}

/// Remove every row a prior `seed` wrote, keyed on the stable demo markers.
/// Non-demo data is untouched.
///
/// The **audit chain is intentionally NOT cleared**: it is an append-only,
/// HMAC-linked log — deleting interior rows would break `verify_chain` for the
/// whole chain (the next row's `prev_hmac` would no longer match). Demo audit
/// rows therefore stay; re-seeding simply appends a fresh batch. This is the
/// honest behaviour for an audit log and is called out in the printed guide.
///
/// # Errors
/// Returns an error if any delete fails.
pub async fn clear_demo(pool: &SqlitePool) -> Result<()> {
    // Order matters only where FKs cascade; we delete children first anyway so
    // the intent is explicit and the function is FK-pragma-agnostic.
    let sess_like = format!("{DEMO_SESSION_PREFIX}%");
    let job_like = format!("{DEMO_JOB_PREFIX}%");

    // Incident children (RCAs) then incidents. The incident `id` is a real UUID
    // (BLOB), not a text prefix, so demo rows are keyed on `source = 'demo'`;
    // RCAs carry no source column, so they're matched via their parent incident
    // and deleted first (while the parent still exists), making this correct
    // whether or not SQLite FK-cascade is enabled.
    sqlx::query(
        "DELETE FROM incident_rcas \
         WHERE incident_id IN (SELECT id FROM incidents WHERE source = ?1)",
    )
    .bind(DEMO_INCIDENT_SOURCE)
    .execute(pool)
    .await
    .context("delete demo incident_rcas")?;
    sqlx::query("DELETE FROM incidents WHERE source = ?1")
        .bind(DEMO_INCIDENT_SOURCE)
        .execute(pool)
        .await
        .context("delete demo incidents")?;

    // token_usage demo rows (marked by request_id).
    sqlx::query("DELETE FROM token_usage WHERE request_id = ?1")
        .bind(DEMO_USAGE_REQUEST_ID)
        .execute(pool)
        .await
        .context("delete demo token_usage")?;

    // Scheduled job runs then jobs.
    sqlx::query("DELETE FROM scheduled_job_runs WHERE job_id LIKE ?1")
        .bind(&job_like)
        .execute(pool)
        .await
        .context("delete demo scheduled_job_runs")?;
    sqlx::query("DELETE FROM scheduled_jobs WHERE id LIKE ?1")
        .bind(&job_like)
        .execute(pool)
        .await
        .context("delete demo scheduled_jobs")?;

    // Messages then sessions.
    sqlx::query("DELETE FROM messages WHERE session_id LIKE ?1")
        .bind(&sess_like)
        .execute(pool)
        .await
        .context("delete demo messages")?;
    sqlx::query("DELETE FROM sessions WHERE id LIKE ?1")
        .bind(&sess_like)
        .execute(pool)
        .await
        .context("delete demo sessions")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Sessions + messages
// ---------------------------------------------------------------------------

/// One scripted demo message (`role` / `content` / how many minutes before
/// `now` it was sent). A small named type so the chat fixtures don't trip
/// clippy's `type_complexity` on a nested tuple slice.
struct DemoMessage {
    role: &'static str,
    content: &'static str,
    mins_ago: i64,
}

/// One scripted demo chat: a session id + title + its messages.
struct DemoChat {
    id: &'static str,
    title: &'static str,
    messages: &'static [DemoMessage],
}

const fn msg(role: &'static str, content: &'static str, mins_ago: i64) -> DemoMessage {
    DemoMessage {
        role,
        content,
        mins_ago,
    }
}

/// Governance chat script (hoisted to a `const` so the `DemoChat.messages`
/// `&'static` slice borrows a promotable value, not a temporary).
const GOVERNANCE_MSGS: &[DemoMessage] = &[
    msg("user", "帮我把过去 24 小时的审计链导出成 SOC2 证据包。", 92),
    msg(
        "assistant",
        "已校验审计链完整（HMAC 链未断裂），正在按时间窗渲染 SOC2 bundle。导出包含会话创建、工具调用、HotL 升级与成本计费等条目。",
        91,
    ),
    msg("user", "顺便确认一下有没有被拒绝的写操作。", 90),
    msg(
        "assistant",
        "有 1 条 `consult.denied`：只读（consult）模式下，一次写工具调用被网关拦截，已记入审计链。",
        89,
    ),
];

/// Ops chat script (see [`GOVERNANCE_MSGS`] for the `const` rationale).
const OPS_MSGS: &[DemoMessage] = &[
    msg("user", "每天早上帮我巡检一下核心 dashboard，有异常就报警。", 40),
    msg(
        "assistant",
        "已安排一个每天 09:00 的定时巡检任务，并配了 token 用量的 z-score 异常监控；异常会推送到飞书。",
        39,
    ),
];

/// Two demo chats with a handful of messages each. Returns (sessions, messages).
///
/// `owner_user_id` is the authed owner (the basic-auth username) so the seeded
/// chats match real chats and surface in `GET /v1/sessions` (which filters by
/// the caller's identity) — not a synthetic id the session list never returns.
async fn seed_sessions(
    pool: &SqlitePool,
    owner_user_id: &str,
    now: DateTime<Utc>,
) -> Result<(usize, usize)> {
    let chats: &[DemoChat] = &[
        DemoChat {
            id: "demo_sess_governance",
            title: "审计与合规：导出 SOC2 证据包",
            messages: GOVERNANCE_MSGS,
        },
        DemoChat {
            id: "demo_sess_ops",
            title: "自动化运维：dashboard 巡检",
            messages: OPS_MSGS,
        },
    ];

    let mut session_count = 0usize;
    let mut message_count = 0usize;
    for chat in chats {
        let sid = chat.id;
        let created = ts(now - Duration::minutes(95));
        sqlx::query(
            "INSERT INTO sessions (id, user_id, title, model, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)",
        )
        .bind(sid)
        .bind(owner_user_id)
        .bind(chat.title)
        .bind(DEMO_MODEL)
        .bind(&created)
        .bind(ts(now - Duration::minutes(39)))
        .execute(pool)
        .await
        .with_context(|| format!("insert demo session {sid}"))?;
        session_count += 1;

        for (idx, m) in chat.messages.iter().enumerate() {
            // Deterministic per-message id so re-seeding is clean.
            let mid = format!("{sid}_msg_{idx:02}");
            // `messages.content` is read back as `Json<Vec<ContentBlock>>`, so a
            // plain string fails to decode (`/v1/sessions/{id}/messages` 500).
            // Serialize through the real `ContentBlock` type so the on-disk
            // shape can never drift from the read path.
            let content = serde_json::to_string(&[ContentBlock::Text {
                text: m.content.to_string(),
            }])
            .with_context(|| format!("serialize demo message content {mid}"))?;
            sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(&mid)
            .bind(sid)
            .bind(m.role)
            .bind(&content)
            .bind(ts(now - Duration::minutes(m.mins_ago)))
            .execute(pool)
            .await
            .with_context(|| format!("insert demo message {mid}"))?;
            message_count += 1;
        }
    }
    Ok((session_count, message_count))
}

// ---------------------------------------------------------------------------
// Scheduled jobs
// ---------------------------------------------------------------------------

/// A daily-cron job and a webhook-triggered job, primed so they show as
/// `active` with a sensible next-fire. Returns the count seeded.
async fn seed_jobs(pool: &SqlitePool, now: DateTime<Utc>) -> Result<usize> {
    // trigger / payload / retry_policy are TEXT-JSON columns (see
    // 0007_scheduled_jobs.sql + the scheduler's serde model). We write the
    // canonical shapes the JobRunner deserializes.
    let retry = json!({ "max_attempts": 3, "initial_backoff_secs": 30, "multiplier": 2.0, "max_backoff_secs": 3600 }).to_string();
    let next_fire = ts(now + Duration::hours(8));

    // Daily 09:00 UTC cron — dashboard health check.
    let cron_trigger = json!({ "type": "cron", "expr": "0 0 9 * * *" }).to_string();
    let cron_payload = json!({
        "prompt": "巡检核心运维 dashboard：拉取关键指标，若有红色告警则汇总根因并升级给值班人。"
    })
    .to_string();
    sqlx::query(
        "INSERT INTO scheduled_jobs \
            (id, name, description, trigger, payload, retry_policy, sinks, enabled, next_fire_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?9)",
    )
    .bind(format!("{DEMO_JOB_PREFIX}dashboard"))
    .bind("每日 dashboard 巡检")
    .bind("每天 09:00（UTC）检查核心 dashboard，异常推送飞书。")
    .bind(&cron_trigger)
    .bind(&cron_payload)
    .bind(&retry)
    .bind(json!(["feishu:ops-room"]).to_string())
    .bind(&next_fire)
    .bind(ts(now - Duration::hours(20)))
    .execute(pool)
    .await
    .context("insert demo cron job")?;

    // Webhook-triggered job — fires when an external system POSTs to the route.
    let hook_trigger = json!({ "type": "webhook", "route_id": "demo-deploy-hook" }).to_string();
    let hook_payload = json!({
        "prompt": "收到部署完成的 webhook：核对发布版本、跑一次冒烟检查，把结果回贴到发布频道。"
    })
    .to_string();
    sqlx::query(
        "INSERT INTO scheduled_jobs \
            (id, name, description, trigger, payload, retry_policy, sinks, enabled, next_fire_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, NULL, ?8, ?8)",
    )
    .bind(format!("{DEMO_JOB_PREFIX}deploy_hook"))
    .bind("部署完成回调冒烟")
    .bind("外部系统 POST /webhooks/demo-deploy-hook 时触发，跑冒烟并回贴结果。")
    .bind(&hook_trigger)
    .bind(&hook_payload)
    .bind(&retry)
    .bind(json!(["inbox:owner"]).to_string())
    .bind(ts(now - Duration::hours(18)))
    .execute(pool)
    .await
    .context("insert demo webhook job")?;

    Ok(2)
}

// ---------------------------------------------------------------------------
// token_usage baseline + spike
// ---------------------------------------------------------------------------

/// Lay down a flat token-usage history then one obvious tail spike, so an
/// anomaly back-test / fire-now flags a z-score outlier. Returns row count.
async fn seed_token_usage(
    pool: &SqlitePool,
    owner_user_id: &str,
    now: DateTime<Utc>,
) -> Result<usize> {
    let mut rows = 0usize;
    // Baseline: one point every 30 min going back, all ~BASELINE_TOKENS with a
    // tiny ±deterministic wobble so σ is non-zero (a perfectly flat series has
    // σ=0 and the z-score is undefined / infinite — a wobble keeps it well
    // defined while the spike still dwarfs it).
    for i in 0..BASELINE_POINTS {
        let mins_ago = (BASELINE_POINTS - i) as i64 * 30 + 30;
        // Deterministic wobble in [-40, +40] tokens.
        let wobble = ((i as i64 * 37) % 81) - 40;
        let total = BASELINE_TOKENS + wobble;
        let prompt = total * 7 / 10;
        let completion = total - prompt;
        insert_usage(
            pool,
            owner_user_id,
            "demo_sess_ops",
            prompt,
            completion,
            total,
            ts(now - Duration::minutes(mins_ago)),
        )
        .await?;
        rows += 1;
    }
    // The spike — most recent point, ~8× baseline.
    let spike_prompt = SPIKE_TOKENS * 7 / 10;
    let spike_completion = SPIKE_TOKENS - spike_prompt;
    insert_usage(
        pool,
        owner_user_id,
        "demo_sess_ops",
        spike_prompt,
        spike_completion,
        SPIKE_TOKENS,
        ts(now - Duration::minutes(2)),
    )
    .await?;
    rows += 1;

    Ok(rows)
}

async fn insert_usage(
    pool: &SqlitePool,
    owner_user_id: &str,
    session_id: &str,
    prompt: i64,
    completion: i64,
    total: i64,
    ts_str: String,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO token_usage \
            (ts, user_id, session_id, provider_id, model, prompt_tokens, completion_tokens, total_tokens, request_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(ts_str)
    .bind(owner_user_id)
    .bind(session_id)
    .bind(DEMO_PROVIDER_ID)
    .bind(DEMO_MODEL)
    .bind(prompt)
    .bind(completion)
    .bind(total)
    .bind(DEMO_USAGE_REQUEST_ID)
    .execute(pool)
    .await
    .context("insert demo token_usage row")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Incident + RCA
// ---------------------------------------------------------------------------

/// One resolved incident with a root-cause-analysis row, for the Incidents
/// pane. Returns the incident count (1).
async fn seed_incident(pool: &SqlitePool, now: DateTime<Utc>) -> Result<usize> {
    // `incidents.id` and `incident_rcas.id`/`incident_id` are read back as
    // `Uuid` (sqlx decodes a 16-byte BLOB), so a text id like "demo_inc_001"
    // makes `/v1/incidents` 500 (`decoding column "id": expected 16 bytes`).
    // Bind real UUIDs; a deterministic v5 id keeps re-seeding stable.
    let incident_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"xiaoguai-demo-incident-001");
    let occurred = ts(now - Duration::hours(3));
    let raw_payload = json!({
        "alert": "HighErrorRate",
        "service": "checkout-api",
        "env": "prod",
        "error_rate": 0.137,
        "threshold": 0.02,
        "window": "5m",
        "note": "demo-seed synthetic incident"
    })
    .to_string();

    // Resolved status so the live-dedup partial unique index is not occupied
    // (lets a fresh real alert with the same key still open) and the pane
    // shows a completed loop with an RCA.
    sqlx::query(
        "INSERT INTO incidents \
            (id, source, external_id, title, severity, project, environment, occurred_at, raw_payload, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'high', 'checkout-api', 'prod', ?5, ?6, 'resolved', ?7, ?8)",
    )
    .bind(incident_id)
    .bind(DEMO_INCIDENT_SOURCE)
    .bind("demo:checkout-error-rate")
    .bind("checkout-api 错误率突增至 13.7%（阈值 2%）")
    .bind(&occurred)
    .bind(&raw_payload)
    .bind(ts(now - Duration::hours(3)))
    .bind(ts(now - Duration::hours(2)))
    .execute(pool)
    .await
    .context("insert demo incident")?;

    let rca_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"xiaoguai-demo-rca-001");
    let action_items = json!([
        "回滚 checkout-api 至上一个稳定版本（已通过 HotL 审批执行）",
        "为下游支付超时增加熔断与降级",
        "补一条 token/错误率联合的 z-score 异常监控"
    ])
    .to_string();
    let raw_markdown = "## 根因分析\n\n上线的 checkout-api v2026.6.27 引入了对支付网关的同步调用，\
        在支付网关 P99 抖动时线程池被占满，导致 5 分钟窗口内错误率从基线 1% 飙升到 13.7%。\n\n\
        ## 处置\n\n已在 consult 模式完成根因定位，再经 HotL 审批后在 execute 模式回滚，错误率恢复正常。";

    sqlx::query(
        "INSERT INTO incident_rcas \
            (id, incident_id, session_id, summary, root_cause, confidence, action_items, raw_markdown, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(rca_id)
    .bind(incident_id)
    .bind("demo_sess_ops")
    .bind("新版本对支付网关的同步调用在其抖动时耗尽线程池，引发错误率突增；回滚后恢复。")
    .bind("checkout-api v2026.6.27 同步调用支付网关 + 网关 P99 抖动 → 线程池耗尽")
    .bind(0.86_f64)
    .bind(&action_items)
    .bind(raw_markdown)
    .bind(ts(now - Duration::hours(2)))
    .execute(pool)
    .await
    .context("insert demo incident RCA")?;

    Ok(1)
}

// ---------------------------------------------------------------------------
// Audit chain — through the real HMAC sink so verify stays green
// ---------------------------------------------------------------------------

/// Append a representative spread of audit events through the HMAC-chained
/// sink. Each append reads the prior row's hmac and signs over it, so the
/// chain verifies end-to-end. Returns the number of rows appended.
async fn seed_audit(
    audit: &SqliteAuditSink,
    owner_user_id: &str,
    now: DateTime<Utc>,
) -> Result<usize> {
    // The owner login row references the same identity the sessions use.
    let owner_resource = format!("user:{owner_user_id}");
    // (minutes_ago, action, resource, details) — ordered oldest→newest so the
    // appended chain timestamps are monotonically increasing like real life.
    // `append` re-signs over whatever the latest row is, so even interleaving
    // with real rows stays valid.
    let events: &[(i64, &str, &str, serde_json::Value)] = &[
        (
            210,
            "auth.login",
            owner_resource.as_str(),
            json!({ "method": "owner-basic", "ip": "127.0.0.1" }),
        ),
        (
            208,
            "session.create",
            "demo_sess_governance",
            json!({ "title": "审计与合规：导出 SOC2 证据包" }),
        ),
        (
            205,
            "memory.recall",
            "demo_sess_governance",
            json!({ "query": "上次的合规导出窗口", "hits": 3 }),
        ),
        (
            200,
            "tool.invoke",
            "tool:read_audit_window",
            json!({ "session_id": "demo_sess_governance", "mutation": "read" }),
        ),
        (
            160,
            "session.create",
            "demo_sess_ops",
            json!({ "title": "自动化运维：dashboard 巡检" }),
        ),
        (
            150,
            "cost.charge",
            "provider:minimax",
            json!({ "session_id": "demo_sess_ops", "model": DEMO_MODEL, "usd": 0.0123, "total_tokens": 1024 }),
        ),
        (
            140,
            "code.edit",
            "workspace:demo-checkout",
            json!({ "checkpoint": "a1b2c3d", "summary": "checkout.rs (+2 -1)" }),
        ),
        (
            138,
            "git.commit",
            "workspace:demo-checkout",
            json!({ "checkpoint": "a1b2c3d", "message": "fix: guard payment timeout" }),
        ),
        (
            95,
            "policy.deny",
            "tool:delete_records",
            json!({ "scope": "tool_call.delete_records", "reason": "consult mode: write tools are disabled" }),
        ),
        (
            60,
            "hotl.escalate",
            "scope:deploy",
            json!({ "session_id": "demo_sess_ops", "amount": 1.0, "escalate_to": "owner" }),
        ),
        (
            58,
            "hotl.decision",
            "scope:deploy",
            json!({ "decision": "approve", "decided_by": "owner" }),
        ),
        (
            30,
            "data.export",
            "audit:soc2",
            json!({ "framework": "soc2", "rows": 11, "window_hours": 24 }),
        ),
        (
            5,
            "audit.verify",
            "chain:owner",
            json!({ "result": "ok", "rows_checked": 11 }),
        ),
    ];

    let mut count = 0usize;
    for (mins_ago, action, resource, details) in events {
        let entry = AuditEntry {
            ts: now - Duration::minutes(*mins_ago),
            tenant_id: OWNER_TENANT_ID.to_string(),
            actor: DEMO_AUDIT_ACTOR.to_string(),
            action: (*action).to_string(),
            resource: Some((*resource).to_string()),
            details: details.clone(),
        };
        audit
            .append(entry)
            .await
            .map_err(|e| anyhow::anyhow!("audit append {action}: {e}"))?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Format a timestamp the way every migration's `DEFAULT` does
/// (`strftime('%Y-%m-%dT%H:%M:%SZ')`), so demo rows sort next to real ones.
fn ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Render the post-seed "what got seeded + where to look" guide. Pure so it is
/// unit-testable; `main.rs` prints the returned string.
#[must_use]
pub fn format_guide(report: &SeedReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "✓ demo 数据已就绪（写入 serve 使用的同一个 SQLite 库）："
    );
    let _ = writeln!(
        out,
        "  · 审计链      {:>3} 条（经真实 HMAC 链签名，verify 通过）",
        report.audit_rows
    );
    let _ = writeln!(
        out,
        "  · 会话/消息   {:>3} 个会话 / {} 条消息",
        report.sessions, report.messages
    );
    let _ = writeln!(
        out,
        "  · 定时任务    {:>3} 个（cron 巡检 + webhook 冒烟）",
        report.jobs
    );
    let _ = writeln!(
        out,
        "  · token 用量  {:>3} 条（基线 ~{} + 末尾 spike ~{}，可触发 z-score 异常）",
        report.usage_rows, report.baseline_tokens, report.spike_tokens
    );
    let _ = writeln!(
        out,
        "  · 事件/RCA    {:>3} 个事件（含根因分析，状态 resolved）",
        report.incidents
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "现场看哪些 pane：");
    let _ = writeln!(
        out,
        "  · 活动历史 / Activity   → 审计链 + 校验徽章（绿）；含 1 条 policy.deny（consult 拦截）"
    );
    let _ = writeln!(
        out,
        "  · 会话 / Today          → 两个示例对话（治理 / 运维）"
    );
    let _ = writeln!(
        out,
        "  · 定时任务 / Schedule   → 每日 dashboard 巡检 + 部署 webhook"
    );
    let _ = writeln!(
        out,
        "  · 异常监控 / Anomaly    → token 用量末尾 spike，fire-now / 回测可检出"
    );
    let _ = writeln!(
        out,
        "  · 事件 / Incidents      → checkout-api 错误率事件 + RCA"
    );
    let _ = writeln!(
        out,
        "  · 用量统计 / Stats      → `xiaoguai stats --by day` 可见 spike"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "提示：审计链是 append-only，`--reset` 不会删除已签名的审计行（删除会破坏 HMAC 链）；"
    );
    let _ = writeln!(
        out,
        "      其余 demo 数据（会话/任务/用量/事件）会被 --reset 清除。"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_storage::{connect, migrate};

    /// Spin up an isolated in-memory-ish temp DB, migrate, and return a pool +
    /// a signed audit sink (real HMAC key) for the seed tests.
    async fn fixture() -> (SqlitePool, SqliteAuditSink, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("demo.db");
        let url = format!("sqlite://{}?mode=rwc", db.display());
        let pool = connect(&url, 4).await.expect("connect");
        migrate(&pool).await.expect("migrate");
        let sink = SqliteAuditSink::new(
            pool.clone(),
            b"test-demo-seed-key-32-bytes-minimum!!".to_vec(),
        );
        (pool, sink, dir)
    }

    fn fixed_now() -> DateTime<Utc> {
        "2026-06-27T12:00:00Z".parse().expect("parse now")
    }

    #[tokio::test]
    async fn seed_populates_every_pane() {
        let (pool, sink, _dir) = fixture().await;
        let report = seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");

        assert_eq!(report.audit_rows, 13, "audit row count");
        assert_eq!(report.sessions, 2, "session count");
        assert_eq!(report.messages, 6, "message count");
        assert_eq!(report.jobs, 2, "job count");
        assert_eq!(
            report.usage_rows,
            BASELINE_POINTS + 1,
            "usage rows = baseline + spike"
        );
        assert_eq!(report.incidents, 1, "incident count");

        // Cross-check actual table contents.
        let n_sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id LIKE 'demo_sess_%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n_sessions, 2);
        let n_jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_jobs WHERE id LIKE 'demo_job_%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n_jobs, 2);
        let n_inc: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM incidents WHERE source = 'demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n_inc, 1);
        let n_rca: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM incident_rcas")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n_rca, 1);
    }

    #[tokio::test]
    async fn audit_chain_verifies_after_seed() {
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");
        // The whole chain (every appended demo row) must verify.
        sink.verify_tenant(OWNER_TENANT_ID)
            .await
            .expect("seeded audit chain must verify");
    }

    #[tokio::test]
    async fn seed_jobs_retry_policy_deserializes_into_scheduler_model() {
        // Regression: a demo job's `retry_policy` JSON must match the
        // scheduler's `RetryPolicy` serde model, or the JobRunner tick fails to
        // deserialize it (the original `backoff_secs` vs `initial_backoff_secs`/
        // `max_backoff_secs` field-name bug). Seed, then round-trip every demo
        // job's policy through the real type.
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");
        let policies: Vec<String> = sqlx::query_scalar(
            "SELECT retry_policy FROM scheduled_jobs WHERE id LIKE 'demo_job_%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(policies.len(), 2, "two demo jobs");
        for rp in policies {
            serde_json::from_str::<xiaoguai_scheduler::RetryPolicy>(&rp).unwrap_or_else(|e| {
                panic!("demo retry_policy must deserialize into scheduler RetryPolicy: {e}\n{rp}")
            });
        }
    }

    #[tokio::test]
    async fn seed_incident_ids_decode_as_uuid() {
        // Regression: `incidents.id` and `incident_rcas.id`/`incident_id` are
        // read back through `Uuid` (a 16-byte BLOB). A text id ("demo_inc_001")
        // made `/v1/incidents` 500 with `decoding column "id": expected 16
        // bytes`. Read every demo id through the SAME `Uuid` decode the store
        // uses — a text id panics here, exactly as the endpoint did.
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");

        let inc: (Uuid,) = sqlx::query_as("SELECT id FROM incidents WHERE source = 'demo'")
            .fetch_one(&pool)
            .await
            .expect("incident id must decode as a 16-byte UUID");
        let rca: (Uuid, Uuid) = sqlx::query_as("SELECT id, incident_id FROM incident_rcas")
            .fetch_one(&pool)
            .await
            .expect("rca id + incident_id must decode as UUIDs");
        assert_eq!(
            rca.1, inc.0,
            "rca.incident_id must reference the incident id"
        );
    }

    #[tokio::test]
    async fn seed_message_content_decodes_as_content_blocks() {
        // Regression: `messages.content` is read as `Json<Vec<ContentBlock>>`;
        // plain-text content made `/v1/sessions/{id}/messages` 500 with
        // `ColumnDecode "content"`. Decode every seeded message the exact way the
        // message repository's `MessageRow` does and confirm a Text block
        // round-trips.
        use sqlx::types::Json;
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");

        let rows: Vec<(Json<Vec<ContentBlock>>,)> = sqlx::query_as(
            "SELECT content FROM messages WHERE session_id LIKE 'demo_sess_%' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .expect("every message content must decode as Vec<ContentBlock>");
        assert_eq!(rows.len(), 6, "all demo messages decode");

        let Json(blocks) = &rows[0].0;
        match blocks.as_slice() {
            [ContentBlock::Text { text }] => assert!(!text.is_empty(), "text content present"),
            other => panic!("expected a single Text block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn seed_sessions_use_owner_user_id() {
        // Regression: demo sessions + usage must be stored under the authed owner
        // id (the basic-auth username), not a synthetic "usr_owner" — else they
        // never appear in `GET /v1/sessions`, which filters by the caller, and
        // the chat sidebar / Today pane look empty or inconsistent.
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");

        let session_users: Vec<String> =
            sqlx::query_scalar("SELECT user_id FROM sessions WHERE id LIKE 'demo_sess_%'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(session_users.len(), 2, "two demo sessions");
        assert!(
            session_users.iter().all(|u| u == "owner"),
            "demo sessions must use the owner user id, got {session_users:?}"
        );

        let usage_under_owner: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM token_usage WHERE request_id = 'demo-seed' AND user_id = 'owner'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            usage_under_owner as usize,
            BASELINE_POINTS + 1,
            "all demo usage attributes to the owner"
        );
    }

    #[tokio::test]
    async fn token_usage_has_detectable_spike() {
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");

        let max: i64 =
            sqlx::query_scalar("SELECT MAX(total_tokens) FROM token_usage WHERE request_id = ?1")
                .bind(DEMO_USAGE_REQUEST_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(max, SPIKE_TOKENS, "spike present");

        // The spike must be a clear z-score outlier vs the baseline. Compute
        // mean/σ over the baseline-only rows (exclude the spike) and confirm
        // the spike sits well past 3σ.
        let baseline: Vec<i64> = sqlx::query_scalar(
            "SELECT total_tokens FROM token_usage WHERE request_id = ?1 AND total_tokens < ?2 ORDER BY ts",
        )
        .bind(DEMO_USAGE_REQUEST_ID)
        .bind(SPIKE_TOKENS)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(baseline.len(), BASELINE_POINTS);
        let n = baseline.len() as f64;
        let mean = baseline.iter().map(|v| *v as f64).sum::<f64>() / n;
        let var = baseline
            .iter()
            .map(|v| (*v as f64 - mean).powi(2))
            .sum::<f64>()
            / n;
        let std = var.sqrt();
        assert!(
            std > 0.0,
            "baseline σ must be non-zero so z-score is defined"
        );
        let z = (SPIKE_TOKENS as f64 - mean) / std;
        assert!(z > 3.0, "spike must exceed 3σ (got z = {z:.1})");
    }

    #[tokio::test]
    async fn seed_is_idempotent() {
        let (pool, sink, _dir) = fixture().await;
        let r1 = seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("first seed");
        let r2 = seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("second seed");

        // Non-audit data is cleared+reseeded, so counts are identical (no
        // duplicate sessions/jobs/usage/incidents).
        assert_eq!(r1.sessions, r2.sessions);
        let n_sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id LIKE 'demo_sess_%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n_sessions, 2, "re-seed must not duplicate sessions");
        let n_usage: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM token_usage WHERE request_id = ?1")
                .bind(DEMO_USAGE_REQUEST_ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            n_usage as usize,
            BASELINE_POINTS + 1,
            "re-seed must not duplicate usage"
        );
    }

    #[tokio::test]
    async fn reset_clears_demo_data_but_not_audit() {
        let (pool, sink, _dir) = fixture().await;
        seed(&pool, &sink, "owner", fixed_now())
            .await
            .expect("seed");
        clear_demo(&pool).await.expect("clear");

        for (table, sql) in [
            (
                "sessions",
                "SELECT COUNT(*) FROM sessions WHERE id LIKE 'demo_sess_%'",
            ),
            (
                "scheduled_jobs",
                "SELECT COUNT(*) FROM scheduled_jobs WHERE id LIKE 'demo_job_%'",
            ),
            (
                "incidents",
                "SELECT COUNT(*) FROM incidents WHERE source = 'demo'",
            ),
            (
                "token_usage",
                "SELECT COUNT(*) FROM token_usage WHERE request_id = 'demo-seed'",
            ),
        ] {
            let n: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
            assert_eq!(n, 0, "{table} demo rows must be cleared by reset");
        }

        // Audit rows remain (append-only) and still verify.
        let n_audit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n_audit, 13, "audit rows are retained after reset");
        sink.verify_tenant(OWNER_TENANT_ID)
            .await
            .expect("chain still verifies");
    }

    #[test]
    fn guide_mentions_each_pane() {
        let report = SeedReport {
            audit_rows: 13,
            sessions: 2,
            messages: 6,
            jobs: 2,
            usage_rows: 15,
            incidents: 1,
            baseline_tokens: BASELINE_TOKENS,
            spike_tokens: SPIKE_TOKENS,
        };
        let guide = format_guide(&report);
        for needle in ["活动历史", "会话", "定时任务", "异常", "Incidents", "spike"] {
            assert!(guide.contains(needle), "guide should mention {needle}");
        }
    }
}
