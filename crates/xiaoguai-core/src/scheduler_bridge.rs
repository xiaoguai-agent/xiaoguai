//! v0.12.0/v0.12.1 scheduler bridge — wires `xiaoguai-scheduler` into
//! the operator binary.
//!
//! Five small pieces live here so the cycle-breaking story stays
//! visible:
//!
//! 1. [`WebhookSourceAdapter`] — implements `xiaoguai_api::WebhookPusher`
//!    by forwarding to a `xiaoguai_scheduler::WebhookSource`. This is
//!    the bridge that lets `xiaoguai-api` (which can't depend on
//!    `xiaoguai-scheduler` — see `crates/xiaoguai-api/src/scheduler.rs`)
//!    push events into the scheduler at runtime.
//!
//! 2. [`PgSchedulerAuditAppender`] — implements
//!    `xiaoguai_scheduler::AuditAppender` by forwarding to the
//!    `PgAuditSink` already wired in v0.6.5. Keeps every scheduler-driven
//!    run in the HMAC audit chain alongside REST and IM traffic.
//!
//! 3. [`build_runtime_ctx`] — assembles the `RuntimeContext` the
//!    `RuntimeJobExecutor` runs against. Shares the backend + toolbox +
//!    agent defaults already on `AppState`.
//!
//! 4. **v0.12.1** [`LlmNlJobCompiler`] — implements
//!    `xiaoguai_api::NlJobCompiler` by sending the user's free-form
//!    description through an `LlmBackend` together with a strict
//!    JSON-schema prompt, parsing the response back into a
//!    `ScheduledJob`. The generated `id` is replaced with a fresh
//!    `uuid::Uuid::new_v4()` so the model can't pick a colliding id.
//!
//! 5. **v0.12.1** [`PgScheduledJobUpserter`] — implements
//!    `xiaoguai_api::ScheduledJobUpserter` by deserialising the JSON
//!    body into a real `ScheduledJob` and calling `PgJobRepository::upsert`.
//!
//! 6. **v0.12.1** [`PgScheduledSessionWriter`] — implements
//!    `xiaoguai_scheduler::ScheduledSessionWriter`. Creates a
//!    synthetic session row (tenant + user + session) and persists the
//!    runtime's `new_messages` slice. The audit-first console joins
//!    `scheduled_job_runs.session_id` → `sessions.id` to render the
//!    scheduler-driven transcript.
//!
//! v0.12.2 adds two more, paired with the file-watch + RAG re-index work:
//!
//! 7. [`spawn_file_watch_source`] — instantiates a
//!    `xiaoguai_scheduler::FileWatchSource`, merges
//!    config-defined and DB-defined routes (`scheduled_jobs` rows whose
//!    `trigger.type == "file_watch"`), and starts it against the shared
//!    scheduler event channel.
//!
//! 8. [`RagReindexExecutor`] — alternate [`JobExecutor`] for jobs whose
//!    `payload.kind == "rag_reindex"`. Reads `collection_id` + `path`
//!    out of the payload and calls `RagClient::reindex_path`. NOT yet
//!    wired into the operator binary's executor selection — production
//!    still uses `RuntimeJobExecutor` exclusively. The
//!    payload-dispatching `CompositeExecutor` lands in v0.12.2.1; see
//!    `docs/plans/2026-05-24-v0.12.2.md` for the deferral note.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::scheduler::{
    NlJobCompileError, NlJobCompiler, ScheduledJobUpsertError, ScheduledJobUpserter,
    WebhookPushError, WebhookPusher,
};
use xiaoguai_audit::{chain::sink::PgAuditSink, AuditEntry};
use xiaoguai_config::FileWatchSettings;
use xiaoguai_llm::{ChatRequest, LlmBackend, Message as LlmMessage, Role as LlmRole};
use xiaoguai_rag::RagClient;
use xiaoguai_runtime::RuntimeContext;
use xiaoguai_scheduler::{
    AuditAppender, EventSender, ExecutionOutcome, FileWatchRoute, FileWatchSource, JobExecutor,
    JobRepository, PgJobRepository, ScheduledJob, ScheduledSessionWriter, Trigger, TriggerSource,
    WebhookSource,
};
use xiaoguai_storage::repositories::{MessageRepository, SessionRepository};
use xiaoguai_types::{
    ContentBlock, Message as DomainMessage, MessageId, MessageRole, Session, SessionId,
    SessionStatus, TenantId, UserId,
};

pub struct WebhookSourceAdapter {
    source: Arc<WebhookSource>,
}

impl WebhookSourceAdapter {
    #[must_use]
    pub fn new(source: Arc<WebhookSource>) -> Self {
        Self { source }
    }
}

#[async_trait]
impl WebhookPusher for WebhookSourceAdapter {
    async fn push(
        &self,
        route_id: &str,
        detail: serde_json::Value,
    ) -> Result<usize, WebhookPushError> {
        self.source
            .push(route_id, detail)
            .await
            .map_err(|e| WebhookPushError::Backend(e.to_string()))
    }
}

pub struct PgSchedulerAuditAppender {
    sink: Arc<PgAuditSink>,
}

impl PgSchedulerAuditAppender {
    #[must_use]
    pub fn new(sink: Arc<PgAuditSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl AuditAppender for PgSchedulerAuditAppender {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.sink
            .append(entry)
            .await
            .map(|_stored| ())
            .map_err(|e| e.to_string())
    }
}

#[must_use]
pub fn build_runtime_ctx(
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    agent_defaults: AgentConfig,
) -> Arc<RuntimeContext> {
    Arc::new(RuntimeContext::new(backend, toolbox, agent_defaults))
}

// ----------------------------------------------------------------------
// v0.12.1 — natural-language job compiler.
// ----------------------------------------------------------------------

/// System prompt for the compiler. Kept conservative: enumerate the
/// allowed trigger variants and require strict JSON output. The
/// schema-by-example pattern (rather than embedding a JSON Schema doc)
/// is what every weak-tool-use model actually follows reliably.
const NL_JOB_SYSTEM_PROMPT: &str = r#"You compile a user's natural-language description of a scheduled job into a strict JSON object.

The JSON MUST match this shape (extra keys are not allowed):

{
  "id": "PLACEHOLDER",                 // string; will be replaced server-side
  "tenant_id": null,                   // string or null
  "name": "short-kebab-name",          // string, required
  "description": null,                 // string or null
  "trigger": <Trigger>,                // see allowed shapes below
  "payload": { "prompt": "<...>" },    // object with a string `prompt` field at minimum
  "retry_policy": {
    "max_attempts": 3,
    "initial_backoff_secs": 5,
    "max_backoff_secs": 60,
    "multiplier": 2.0
  },
  "sinks": [],                         // array of strings; default empty unless the user named one
  "enabled": true,
  "next_fire_at": null,
  "last_fire_at": null,
  "created_at": "1970-01-01T00:00:00Z",
  "updated_at": "1970-01-01T00:00:00Z"
}

Allowed `trigger` shapes (pick ONE):

  { "type": "cron", "expr": "<6-field UTC cron>" }
  { "type": "interval", "secs": <positive integer> }
  { "type": "file_watch", "path": "<absolute path>" }
  { "type": "webhook", "route_id": "<slug>" }
  { "type": "git_push", "repo_url": "<https://...>", "branch": "<main|...>" }
  { "type": "db_poll", "query": "<SELECT ...>" }
  { "type": "proactive", "check_prompt": "<string>", "interval_secs": <positive integer> }

Cron uses 6-field UTC format (sec min hour day month dow). For "every day at 8am" use "0 0 8 * * *".

Output ONLY valid JSON, no markdown fences, no commentary. The first character of your reply must be `{`."#;

/// Production `NlJobCompiler` — wraps an `LlmBackend` and a default
/// model name.
pub struct LlmNlJobCompiler {
    backend: Arc<dyn LlmBackend>,
    model: String,
}

impl LlmNlJobCompiler {
    #[must_use]
    pub fn new(backend: Arc<dyn LlmBackend>, model: impl Into<String>) -> Self {
        Self {
            backend,
            model: model.into(),
        }
    }
}

#[async_trait]
impl NlJobCompiler for LlmNlJobCompiler {
    async fn compile(
        &self,
        description: &str,
        tenant_id: Option<&str>,
    ) -> Result<(serde_json::Value, String), NlJobCompileError> {
        if description.trim().is_empty() {
            return Err(NlJobCompileError::InvalidArgument(
                "description must not be empty".into(),
            ));
        }
        let messages = vec![
            LlmMessage {
                role: LlmRole::System,
                content: NL_JOB_SYSTEM_PROMPT.into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            LlmMessage::user(description),
        ];
        let mut req = ChatRequest::new(self.model.clone(), messages);
        req.temperature = Some(0.1);
        req.tenant_id = tenant_id.map(str::to_string);

        let mut stream = self
            .backend
            .chat_stream(req)
            .await
            .map_err(|e| NlJobCompileError::Backend(e.to_string()))?;

        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| NlJobCompileError::Backend(e.to_string()))?;
            buf.push_str(&chunk.delta);
        }

        let raw = buf.trim();
        // Be lenient about ```json fences if a model slips one in despite
        // the instruction. Strip them then trim.
        let cleaned = strip_code_fence(raw);

        let mut json: serde_json::Value = serde_json::from_str(cleaned)
            .map_err(|e| NlJobCompileError::Unparseable(format!("not valid JSON: {e}")))?;
        // Regenerate id so the model can't pick a colliding one.
        let new_id = uuid::Uuid::new_v4().to_string();
        if let Some(obj) = json.as_object_mut() {
            obj.insert("id".into(), serde_json::Value::String(new_id.clone()));
        }
        // Parse-back as ScheduledJob so we surface schema mismatches as
        // 400 instead of letting them flow through to the upserter.
        let parsed: ScheduledJob = serde_json::from_value(json.clone()).map_err(|e| {
            NlJobCompileError::Unparseable(format!("not a valid ScheduledJob: {e}"))
        })?;

        let rationale = build_rationale(&parsed);
        Ok((json, rationale))
    }
}

fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.trim_start().trim_end_matches("```").trim();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.trim_start().trim_end_matches("```").trim();
    }
    trimmed
}

fn build_rationale(job: &ScheduledJob) -> String {
    use xiaoguai_scheduler::Trigger;
    let trig = match &job.trigger {
        Trigger::Cron { expr } => format!("cron `{expr}` (UTC)"),
        Trigger::Interval { secs } => format!("every {secs}s"),
        Trigger::FileWatch { path } => format!("watch `{path}`"),
        Trigger::Webhook { route_id } => format!("webhook route `{route_id}`"),
        Trigger::GitPush { repo_url, branch } => format!("git push `{repo_url}`@`{branch}`"),
        Trigger::DbPoll { query } => format!("db poll `{query}`"),
        Trigger::Proactive {
            interval_secs,
            check_prompt,
        } => format!("proactive every {interval_secs}s — `{check_prompt}`"),
    };
    format!("compiled to {} (job name: {})", trig, job.name)
}

// ----------------------------------------------------------------------
// v0.12.1 — PG-backed ScheduledJob upserter.
// ----------------------------------------------------------------------

pub struct PgScheduledJobUpserter {
    repo: Arc<PgJobRepository>,
}

impl PgScheduledJobUpserter {
    #[must_use]
    pub fn new(repo: Arc<PgJobRepository>) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl ScheduledJobUpserter for PgScheduledJobUpserter {
    async fn upsert(&self, body: serde_json::Value) -> Result<(), ScheduledJobUpsertError> {
        let job: ScheduledJob = serde_json::from_value(body)
            .map_err(|e| ScheduledJobUpsertError::InvalidJob(e.to_string()))?;
        self.repo
            .upsert(&job)
            .await
            .map_err(|e| ScheduledJobUpsertError::Repository(e.to_string()))?;
        Ok(())
    }
}

// ----------------------------------------------------------------------
// v0.12.1 — PG-backed ScheduledSessionWriter.
// ----------------------------------------------------------------------

/// Synthetic user id for sessions created by scheduled-job runs. The
/// audit-first console reads this prefix to render a "scheduler" badge
/// rather than a real user avatar.
const SCHEDULER_USER_PREFIX: &str = "scheduler";

pub struct PgScheduledSessionWriter {
    sessions: Arc<dyn SessionRepository>,
    messages: Arc<dyn MessageRepository>,
}

impl PgScheduledSessionWriter {
    #[must_use]
    pub fn new(sessions: Arc<dyn SessionRepository>, messages: Arc<dyn MessageRepository>) -> Self {
        Self { sessions, messages }
    }
}

#[async_trait]
impl ScheduledSessionWriter for PgScheduledSessionWriter {
    async fn create_and_record(
        &self,
        job: &ScheduledJob,
        _prompt: &str,
        new_messages: &[LlmMessage],
    ) -> Result<String, String> {
        // Scheduler jobs without a tenant skip session creation — RLS
        // doesn't enforce a null tenant on sessions and the audit-first
        // console doesn't surface them by user_id anyway. Return a
        // deterministic synthetic id so the run row stays linkable.
        let Some(tenant_id_str) = job.tenant_id.as_deref() else {
            return Err("scheduled session requires a tenant_id on the job".into());
        };

        let session_id_str = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let user_id_str = format!("{SCHEDULER_USER_PREFIX}:{}", job.id);
        let model = job
            .payload
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("scheduler")
            .to_string();

        let session = Session {
            id: SessionId::from(session_id_str.clone()),
            tenant_id: TenantId::from(tenant_id_str.to_string()),
            user_id: UserId::from(user_id_str),
            title: Some(format!("scheduled: {}", job.name)),
            created_at: now,
            updated_at: now,
            model,
            status: SessionStatus::Active,
        };
        self.sessions
            .create(Some(tenant_id_str), &session)
            .await
            .map_err(|e| format!("session create: {e}"))?;

        for msg in new_messages {
            let domain = llm_to_domain(&session_id_str, msg);
            self.messages
                .append(Some(tenant_id_str), &domain)
                .await
                .map_err(|e| format!("message append: {e}"))?;
        }
        Ok(session_id_str)
    }
}

fn llm_to_domain(session_id: &str, msg: &LlmMessage) -> DomainMessage {
    let role = match msg.role {
        LlmRole::User => MessageRole::User,
        LlmRole::Assistant => MessageRole::Assistant,
        LlmRole::System => MessageRole::System,
        LlmRole::Tool => MessageRole::Tool,
    };
    DomainMessage {
        id: MessageId::new(),
        session_id: SessionId::from(session_id.to_string()),
        role,
        content: vec![ContentBlock::Text {
            text: msg.content.clone(),
        }],
        created_at: chrono::Utc::now(),
    }
}

// ----------------------------------------------------------------------
// v0.12.2 — file-watch source bootstrap + RAG re-index executor.
// ----------------------------------------------------------------------

/// v0.12.2 — instantiate and start a [`FileWatchSource`] against the
/// shared scheduler event channel.
///
/// Route sources are merged in this order: (a) the static
/// `cfg.routes` list (config-defined), (b) the persisted
/// `scheduled_jobs` rows whose trigger is `Trigger::FileWatch` (when
/// `cfg.load_routes_from_db` is true). Duplicates are de-duped by
/// `(job_id, path)`; the static list wins on conflict.
///
/// Per-route registration errors are logged but do not abort the
/// bootstrap — a single misconfigured path shouldn't kill the rest of
/// the source.
pub async fn spawn_file_watch_source(
    cfg: &FileWatchSettings,
    jobs: &dyn JobRepository,
    event_tx: EventSender,
) -> anyhow::Result<Arc<FileWatchSource>> {
    let source = Arc::new(FileWatchSource::new());

    let static_routes: Vec<FileWatchRoute> = cfg
        .routes
        .iter()
        .map(|r| FileWatchRoute::new(r.job_id.clone(), PathBuf::from(&r.path)))
        .collect();

    let db_routes: Vec<FileWatchRoute> = if cfg.load_routes_from_db {
        match jobs.list_reactive().await {
            Ok(rows) => rows
                .into_iter()
                .filter_map(|j| match &j.trigger {
                    Trigger::FileWatch { path } => {
                        Some(FileWatchRoute::new(j.id.clone(), PathBuf::from(path)))
                    }
                    _ => None,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "serve: file_watch list_reactive failed; only static routes will be registered");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Static routes first so they win on (job_id, path) conflict.
    let mut seen: std::collections::HashSet<(String, PathBuf)> = std::collections::HashSet::new();
    let mut registered = 0_usize;
    for route in static_routes.into_iter().chain(db_routes.into_iter()) {
        let key = (route.job_id.clone(), route.path.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Err(e) = source.add_route(route.clone()) {
            tracing::warn!(
                job_id = %route.job_id,
                path = %route.path.display(),
                error = %e,
                "serve: file_watch route registration failed"
            );
            continue;
        }
        registered += 1;
    }

    TriggerSource::start(source.as_ref(), event_tx)
        .await
        .map_err(|e| anyhow::anyhow!("file_watch source start: {e}"))?;

    tracing::info!(routes = registered, "serve: file_watch source started");
    Ok(source)
}

/// v0.12.2 — [`JobExecutor`] for jobs whose payload describes an
/// incremental RAG re-index instead of a chat-style prompt.
///
/// Payload shape:
/// ```json
/// {
///     "kind": "rag_reindex",
///     "collection_id": "notes",
///     "path": "/var/notes/topic.md"
/// }
/// ```
///
/// Returns an `ExecutionOutcome` whose `output_preview` reports the
/// number of chunks re-indexed.
///
/// **Wiring status (v0.12.2):** this executor compiles, is tested in
/// isolation, but is NOT yet selected by the operator binary — every
/// scheduled job today still runs through `RuntimeJobExecutor`. The
/// payload-dispatching `CompositeExecutor` that picks between the two
/// based on `payload.kind` is deferred to v0.12.2.1 per
/// `docs/plans/2026-05-24-v0.12.2.md`.
pub struct RagReindexExecutor {
    rag: Arc<dyn RagClient>,
}

impl RagReindexExecutor {
    // dead_code: see the v0.12.2.1 deferral note on `RagReindexExecutor`.
    // Tests exercise this constructor; the binary entry point won't until
    // the payload-dispatching CompositeExecutor lands.
    #[allow(dead_code)]
    #[must_use]
    pub fn new(rag: Arc<dyn RagClient>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl JobExecutor for RagReindexExecutor {
    async fn execute(&self, job: &ScheduledJob, _attempt: u32) -> Result<ExecutionOutcome, String> {
        let collection_id = job
            .payload
            .get("collection_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "rag_reindex payload missing string `collection_id`".to_string())?;
        let path = job
            .payload
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "rag_reindex payload missing string `path`".to_string())?;
        let n = self
            .rag
            .reindex_path(collection_id, std::path::Path::new(path))
            .await
            .map_err(|e| format!("rag reindex: {e}"))?;
        Ok(ExecutionOutcome {
            output_preview: format!("reindexed {n} chunk(s) from {path} into {collection_id}"),
            session_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use xiaoguai_llm::MockBackend;
    use xiaoguai_rag::InMemoryRagClient;
    use xiaoguai_scheduler::{ScheduledJob, Trigger};
    use xiaoguai_storage::repositories::{RepoError, RepoResult};

    fn make_compiler(response: &str) -> LlmNlJobCompiler {
        let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response(response));
        LlmNlJobCompiler::new(backend, "mock-model")
    }

    #[tokio::test]
    async fn compile_rejects_empty_description() {
        let c = make_compiler("ignored");
        let err = c.compile("   ", None).await.unwrap_err();
        assert!(matches!(err, NlJobCompileError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn compile_returns_unparseable_on_garbage_response() {
        let c = make_compiler("not json at all");
        let err = c.compile("daily scan", None).await.unwrap_err();
        assert!(matches!(err, NlJobCompileError::Unparseable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn compile_returns_unparseable_on_wrong_shape() {
        let c = make_compiler(r#"{"foo": "bar"}"#);
        let err = c.compile("daily scan", None).await.unwrap_err();
        assert!(matches!(err, NlJobCompileError::Unparseable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn compile_succeeds_and_regenerates_id() {
        let good = r#"{
            "id": "model-chose-this",
            "tenant_id": null,
            "name": "scan-hn",
            "description": null,
            "trigger": {"type": "cron", "expr": "0 0 8 * * *"},
            "payload": {"prompt": "scan HN"},
            "retry_policy": {"max_attempts": 3, "initial_backoff_secs": 5, "max_backoff_secs": 60, "multiplier": 2.0},
            "sinks": [],
            "enabled": true,
            "next_fire_at": null,
            "last_fire_at": null,
            "created_at": "2026-05-24T00:00:00Z",
            "updated_at": "2026-05-24T00:00:00Z"
        }"#;
        let c = make_compiler(good);
        let (json, rationale) = c.compile("每天 8 点扫 HN", None).await.unwrap();
        let id = json["id"].as_str().unwrap();
        assert_ne!(id, "model-chose-this");
        // uuid v4 has 36 chars with hyphens.
        assert_eq!(id.len(), 36);
        assert!(rationale.contains("cron"));
        assert!(rationale.contains("scan-hn"));
    }

    #[tokio::test]
    async fn compile_strips_code_fence() {
        let response = "```json\n{\"id\":\"x\",\"tenant_id\":null,\"name\":\"n\",\"description\":null,\"trigger\":{\"type\":\"interval\",\"secs\":60},\"payload\":{\"prompt\":\"p\"},\"retry_policy\":{\"max_attempts\":3,\"initial_backoff_secs\":5,\"max_backoff_secs\":60,\"multiplier\":2.0},\"sinks\":[],\"enabled\":true,\"next_fire_at\":null,\"last_fire_at\":null,\"created_at\":\"2026-05-24T00:00:00Z\",\"updated_at\":\"2026-05-24T00:00:00Z\"}\n```";
        let c = make_compiler(response);
        let (json, _) = c.compile("anything", None).await.unwrap();
        assert_eq!(json["name"], "n");
    }

    #[test]
    fn build_rationale_describes_each_trigger() {
        let cron_job = ScheduledJob::new(
            "j1",
            None,
            "the-name",
            Trigger::cron("0 0 8 * * *").unwrap(),
            serde_json::json!({"prompt": "x"}),
        );
        let r = build_rationale(&cron_job);
        assert!(r.contains("cron"));
        assert!(r.contains("the-name"));
        let interval_job = ScheduledJob::new(
            "j2",
            None,
            "n2",
            Trigger::interval(900).unwrap(),
            serde_json::json!({"prompt": "x"}),
        );
        assert!(build_rationale(&interval_job).contains("every 900s"));
    }

    // ------------------------------------------------------------------
    // ScheduledSessionWriter tests with in-memory repos.
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct MemSessionRepo {
        rows: Mutex<Vec<Session>>,
    }
    #[async_trait]
    impl SessionRepository for MemSessionRepo {
        async fn create(&self, _tenant: Option<&str>, session: &Session) -> RepoResult<()> {
            self.rows.lock().push(session.clone());
            Ok(())
        }
        async fn find_by_id(
            &self,
            _tenant: Option<&str>,
            _id: &str,
        ) -> RepoResult<Option<Session>> {
            Ok(None)
        }
        async fn list_by_user(
            &self,
            _tenant: Option<&str>,
            _user_id: &str,
            _limit: i64,
            _offset: i64,
        ) -> RepoResult<Vec<Session>> {
            Ok(Vec::new())
        }
        async fn touch(&self, _tenant: Option<&str>, _id: &str) -> RepoResult<()> {
            Ok(())
        }
        async fn archive(&self, _tenant: Option<&str>, _id: &str) -> RepoResult<()> {
            Ok(())
        }
        async fn delete(&self, _tenant: Option<&str>, _id: &str) -> RepoResult<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemMessageRepo {
        rows: Mutex<Vec<DomainMessage>>,
    }
    #[async_trait]
    impl MessageRepository for MemMessageRepo {
        async fn append(&self, _tenant: Option<&str>, message: &DomainMessage) -> RepoResult<()> {
            self.rows.lock().push(message.clone());
            Ok(())
        }
        async fn list_by_session(
            &self,
            _tenant: Option<&str>,
            _session_id: &str,
            _limit: i64,
            _offset: i64,
        ) -> RepoResult<Vec<DomainMessage>> {
            Ok(Vec::new())
        }
        async fn count_by_session(
            &self,
            _tenant: Option<&str>,
            _session_id: &str,
        ) -> RepoResult<i64> {
            Ok(0)
        }
        async fn delete_by_session(
            &self,
            _tenant: Option<&str>,
            _session_id: &str,
        ) -> RepoResult<u64> {
            Ok(0)
        }
    }

    fn make_job(tenant: Option<&str>) -> ScheduledJob {
        ScheduledJob::new(
            "j-scheduler",
            tenant.map(str::to_string),
            "daily-scan",
            Trigger::interval(60).unwrap(),
            serde_json::json!({ "prompt": "ping" }),
        )
    }

    #[tokio::test]
    async fn writer_persists_session_and_messages() {
        let sessions = Arc::new(MemSessionRepo::default());
        let messages = Arc::new(MemMessageRepo::default());
        let writer = PgScheduledSessionWriter::new(
            sessions.clone() as Arc<dyn SessionRepository>,
            messages.clone() as Arc<dyn MessageRepository>,
        );
        let job = make_job(Some("tenant-x"));
        let msgs = vec![
            LlmMessage::user("ping"),
            LlmMessage::assistant("pong from scheduler"),
        ];
        let sid = writer.create_and_record(&job, "ping", &msgs).await.unwrap();
        assert_eq!(sid.len(), 36);
        let rows = sessions.rows.lock();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title.as_deref(), Some("scheduled: daily-scan"));
        assert!(rows[0].user_id.as_str().starts_with("scheduler:"));
        let mrows = messages.rows.lock();
        assert_eq!(mrows.len(), 2);
        assert_eq!(mrows[0].role, MessageRole::User);
        assert_eq!(mrows[1].role, MessageRole::Assistant);
    }

    #[tokio::test]
    async fn writer_errors_when_tenant_missing() {
        let writer = PgScheduledSessionWriter::new(
            Arc::new(MemSessionRepo::default()) as Arc<dyn SessionRepository>,
            Arc::new(MemMessageRepo::default()) as Arc<dyn MessageRepository>,
        );
        let job = make_job(None);
        let err = writer
            .create_and_record(&job, "ping", &[])
            .await
            .unwrap_err();
        assert!(err.contains("tenant_id"));
    }

    #[tokio::test]
    async fn writer_surfaces_session_repo_error() {
        struct FailingSessions;
        #[async_trait]
        impl SessionRepository for FailingSessions {
            async fn create(&self, _t: Option<&str>, _s: &Session) -> RepoResult<()> {
                Err(RepoError::InvalidArgument("nope".into()))
            }
            async fn find_by_id(&self, _t: Option<&str>, _id: &str) -> RepoResult<Option<Session>> {
                Ok(None)
            }
            async fn list_by_user(
                &self,
                _t: Option<&str>,
                _u: &str,
                _l: i64,
                _o: i64,
            ) -> RepoResult<Vec<Session>> {
                Ok(Vec::new())
            }
            async fn touch(&self, _t: Option<&str>, _id: &str) -> RepoResult<()> {
                Ok(())
            }
            async fn archive(&self, _t: Option<&str>, _id: &str) -> RepoResult<()> {
                Ok(())
            }
            async fn delete(&self, _t: Option<&str>, _id: &str) -> RepoResult<()> {
                Ok(())
            }
        }
        let writer = PgScheduledSessionWriter::new(
            Arc::new(FailingSessions) as Arc<dyn SessionRepository>,
            Arc::new(MemMessageRepo::default()) as Arc<dyn MessageRepository>,
        );
        let job = make_job(Some("t"));
        let err = writer.create_and_record(&job, "x", &[]).await.unwrap_err();
        assert!(err.contains("session create"));
    }

    // ------------------------------------------------------------------
    // v0.12.2 — RagReindexExecutor + file-watch source tests.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn rag_reindex_executor_returns_chunk_count_in_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let rag: Arc<dyn RagClient> = Arc::new(InMemoryRagClient::new());
        let executor = RagReindexExecutor::new(rag);

        let job = ScheduledJob::new(
            "watch-1",
            None,
            "watch-1",
            Trigger::file_watch(path.to_string_lossy().to_string()).unwrap(),
            serde_json::json!({
                "kind": "rag_reindex",
                "collection_id": "notes",
                "path": path.to_string_lossy(),
            }),
        );

        let outcome = executor.execute(&job, 1).await.unwrap();
        assert!(outcome.output_preview.contains("3 chunk"));
        assert!(outcome.output_preview.contains("notes"));
    }

    #[tokio::test]
    async fn rag_reindex_executor_errors_on_missing_collection_id() {
        let rag: Arc<dyn RagClient> = Arc::new(InMemoryRagClient::new());
        let executor = RagReindexExecutor::new(rag);
        let job = ScheduledJob::new(
            "x",
            None,
            "x",
            Trigger::file_watch("/tmp/foo").unwrap(),
            serde_json::json!({"kind": "rag_reindex", "path": "/tmp/foo"}),
        );
        let err = executor.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("collection_id"));
    }

    #[tokio::test]
    async fn rag_reindex_executor_errors_on_missing_path() {
        let rag: Arc<dyn RagClient> = Arc::new(InMemoryRagClient::new());
        let executor = RagReindexExecutor::new(rag);
        let job = ScheduledJob::new(
            "x",
            None,
            "x",
            Trigger::file_watch("/tmp/foo").unwrap(),
            serde_json::json!({"kind": "rag_reindex", "collection_id": "notes"}),
        );
        let err = executor.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("path"));
    }

    #[tokio::test]
    async fn spawn_file_watch_source_starts_with_zero_routes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = FileWatchSettings {
            enabled: true,
            routes: vec![xiaoguai_config::FileWatchRoute {
                job_id: "j1".into(),
                path: dir.path().display().to_string(),
            }],
            load_routes_from_db: false,
        };
        let jobs = xiaoguai_scheduler::InMemoryJobRepository::new();
        let (tx, _rx) = xiaoguai_scheduler::event_channel();
        let source = spawn_file_watch_source(&cfg, &jobs, tx).await.unwrap();
        // The watcher is running; can't easily assert route count without
        // exposing internals — settling for "no error" + Debug print
        // showing started: true.
        let dbg = format!("{source:?}");
        assert!(dbg.contains("started: true"));
    }

    #[tokio::test]
    async fn spawn_file_watch_source_merges_db_routes() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = xiaoguai_scheduler::InMemoryJobRepository::new();
        let job = ScheduledJob::new(
            "watch-db",
            None,
            "watch-db",
            Trigger::file_watch(dir.path().display().to_string()).unwrap(),
            serde_json::json!({}),
        );
        jobs.upsert(&job).await.unwrap();

        let cfg = FileWatchSettings {
            enabled: true,
            routes: Vec::new(),
            load_routes_from_db: true,
        };
        let (tx, _rx) = xiaoguai_scheduler::event_channel();
        let source = spawn_file_watch_source(&cfg, &jobs, tx).await.unwrap();
        let dbg = format!("{source:?}");
        assert!(dbg.contains("route_count: 1"));
    }
}
