//! `WorkerHandle` — thin wrapper that holds a boxed `Worker` and adds
//! round-robin selection bookkeeping.
//!
//! The supervisor keeps a `Vec<WorkerHandle>` and calls `next()` to get the
//! next available worker in round-robin order.  In v1.1.5b this is always
//! index 0 when the pool has one worker; the abstraction is here so v1.2
//! parallel dispatch can reuse the same round-robin primitive.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::worker::Worker;

/// A pool of workers with round-robin dispatch.
pub struct WorkerPool {
    workers: Vec<Arc<dyn Worker>>,
    next_idx: AtomicUsize,
}

impl WorkerPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            next_idx: AtomicUsize::new(0),
        }
    }

    pub fn add(&mut self, w: Arc<dyn Worker>) {
        self.workers.push(w);
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// Return the next worker in round-robin order.
    /// Returns `None` if the pool is empty.
    pub fn next(&self) -> Option<Arc<dyn Worker>> {
        if self.workers.is_empty() {
            return None;
        }
        let idx = self.next_idx.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        Some(self.workers[idx].clone())
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OrchestratorError, Task, WorkerResult};
    use async_trait::async_trait;

    struct DummyWorker(u32);

    #[async_trait]
    impl Worker for DummyWorker {
        async fn execute(&self, _task: Task) -> Result<WorkerResult, OrchestratorError> {
            Ok(WorkerResult {
                output: self.0.to_string(),
                success: true,
            })
        }
    }

    #[test]
    fn empty_pool_returns_none() {
        let pool = WorkerPool::new();
        assert!(pool.next().is_none());
    }

    #[test]
    fn single_worker_always_returned() {
        let mut pool = WorkerPool::new();
        pool.add(Arc::new(DummyWorker(1)));
        assert!(pool.next().is_some());
        assert!(pool.next().is_some());
    }

    #[tokio::test]
    async fn round_robin_cycles() {
        let mut pool = WorkerPool::new();
        pool.add(Arc::new(DummyWorker(1)));
        pool.add(Arc::new(DummyWorker(2)));
        pool.add(Arc::new(DummyWorker(3)));

        let task = || Task {
            step_id: "t".into(),
            description: "t".into(),
            context: vec![],
        };

        let r1 = pool.next().unwrap().execute(task()).await.unwrap().output;
        let r2 = pool.next().unwrap().execute(task()).await.unwrap().output;
        let r3 = pool.next().unwrap().execute(task()).await.unwrap().output;
        let r4 = pool.next().unwrap().execute(task()).await.unwrap().output;

        // Must cycle: 1, 2, 3, 1
        assert_eq!(r1, "1");
        assert_eq!(r2, "2");
        assert_eq!(r3, "3");
        assert_eq!(r4, "1");
    }
}
