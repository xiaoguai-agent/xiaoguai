//! v0.12.x.1 — payload-dispatching [`JobExecutor`].
//!
//! v0.12.2 shipped `RagReindexExecutor` (in `xiaoguai-core/scheduler_bridge.rs`)
//! but left it un-wired: the operator binary still used
//! `RuntimeJobExecutor` for every scheduled job. v0.12.x.1 lands the
//! tiny dispatcher that picks between the two based on
//! `job.payload.get("kind")`.
//!
//! Semantics:
//!
//! * If `payload.kind == "<key>"` and `<key>` is registered, dispatch
//!   to the matching executor.
//! * Otherwise (key absent OR not registered) fall through to the
//!   inner default executor.
//!
//! The default-fallback path is the load-bearing convention — every
//! existing scheduled job has no `kind` field and must keep working
//! against `RuntimeJobExecutor` (the v0.12.0 default).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::executor::{ExecutionOutcome, JobExecutor};
use crate::job::ScheduledJob;

pub struct CompositeExecutor {
    default: Arc<dyn JobExecutor>,
    by_kind: HashMap<String, Arc<dyn JobExecutor>>,
}

impl CompositeExecutor {
    /// Build with a default executor. Use [`Self::register`] to add
    /// kind-specific dispatchers.
    #[must_use]
    pub fn new(default: Arc<dyn JobExecutor>) -> Self {
        Self {
            default,
            by_kind: HashMap::new(),
        }
    }

    /// Register `executor` for jobs whose `payload.kind == kind`. Last
    /// registration wins on duplicate keys.
    #[must_use]
    pub fn register(mut self, kind: impl Into<String>, executor: Arc<dyn JobExecutor>) -> Self {
        self.by_kind.insert(kind.into(), executor);
        self
    }

    fn pick(&self, job: &ScheduledJob) -> &Arc<dyn JobExecutor> {
        if let Some(kind) = job.payload.get("kind").and_then(|v| v.as_str()) {
            if let Some(found) = self.by_kind.get(kind) {
                return found;
            }
        }
        &self.default
    }
}

#[async_trait]
impl JobExecutor for CompositeExecutor {
    async fn execute(&self, job: &ScheduledJob, attempt: u32) -> Result<ExecutionOutcome, String> {
        self.pick(job).execute(job, attempt).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::EchoExecutor;
    use crate::trigger::Trigger;
    use parking_lot::Mutex;

    /// Records every dispatch + returns a canned preview.
    struct TaggedExecutor {
        tag: &'static str,
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl JobExecutor for TaggedExecutor {
        async fn execute(
            &self,
            _job: &ScheduledJob,
            _attempt: u32,
        ) -> Result<ExecutionOutcome, String> {
            *self.calls.lock() += 1;
            Ok(ExecutionOutcome {
                output_preview: format!("from:{}", self.tag),
                session_id: None,
            })
        }
    }

    fn make_job(payload: serde_json::Value) -> ScheduledJob {
        ScheduledJob::new("j", "j", Trigger::interval(60).unwrap(), payload)
    }

    #[tokio::test]
    async fn falls_through_to_default_when_kind_missing() {
        let default: Arc<dyn JobExecutor> = Arc::new(EchoExecutor);
        let exec = CompositeExecutor::new(default);
        let job = make_job(serde_json::json!({ "prompt": "hello" }));
        let outcome = exec.execute(&job, 1).await.unwrap();
        assert!(outcome.output_preview.starts_with("echo:"));
    }

    #[tokio::test]
    async fn dispatches_by_kind_when_registered() {
        let default = Arc::new(TaggedExecutor {
            tag: "default",
            calls: Mutex::new(0),
        });
        let rag = Arc::new(TaggedExecutor {
            tag: "rag",
            calls: Mutex::new(0),
        });
        let exec = CompositeExecutor::new(default.clone() as Arc<dyn JobExecutor>)
            .register("rag_reindex", rag.clone() as Arc<dyn JobExecutor>);

        let outcome = exec
            .execute(&make_job(serde_json::json!({ "kind": "rag_reindex" })), 1)
            .await
            .unwrap();
        assert_eq!(outcome.output_preview, "from:rag");
        assert_eq!(*rag.calls.lock(), 1);
        assert_eq!(*default.calls.lock(), 0);

        // Unknown kind falls through to default.
        let outcome = exec
            .execute(&make_job(serde_json::json!({ "kind": "unknown" })), 1)
            .await
            .unwrap();
        assert_eq!(outcome.output_preview, "from:default");
        assert_eq!(*default.calls.lock(), 1);
    }

    #[tokio::test]
    async fn last_registration_wins_on_duplicate_key() {
        let default: Arc<dyn JobExecutor> = Arc::new(EchoExecutor);
        let first = Arc::new(TaggedExecutor {
            tag: "first",
            calls: Mutex::new(0),
        });
        let second = Arc::new(TaggedExecutor {
            tag: "second",
            calls: Mutex::new(0),
        });
        let exec = CompositeExecutor::new(default)
            .register("k", first as Arc<dyn JobExecutor>)
            .register("k", second.clone() as Arc<dyn JobExecutor>);
        let outcome = exec
            .execute(&make_job(serde_json::json!({ "kind": "k" })), 1)
            .await
            .unwrap();
        assert_eq!(outcome.output_preview, "from:second");
        assert_eq!(*second.calls.lock(), 1);
    }
}
