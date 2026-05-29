//! `Scratchpad` — per-task ephemeral writes. Quarantine-keyed by
//! `TaskId` so Workers cannot read each other's drafts (DEC-021
//! §4.5).
//!
//! Append-only by design — once an entry is written it cannot be
//! mutated, only superseded by a later entry. This makes the audit
//! trail of "what did the Worker try" cheap to read; the Critic walks
//! the entries in order to decide whether the task is on track.
//!
//! The Critic has **read access** to the scratchpad it's reviewing
//! but **no write access** — handed in as `&Scratchpad`. Only the
//! `WorkerAgent` constructs `&mut Scratchpad`. This is the type-level
//! capability separation that prevents Critic-induced contamination.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::plan::TaskId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchEntry {
    pub at: DateTime<Utc>,
    /// Free-form content. Typically a tool result, a partial
    /// reasoning step, or a draft artefact. The Critic reads these
    /// in order to assess whether the Worker is on track.
    pub content: String,
    /// Optional cost-tracking attribution — Worker increments
    /// `cost_tokens` after each LLM call so `BudgetEnforcer` can
    /// gate before the next iteration.
    pub tokens_used: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScratchpadError {
    #[error("scratchpad belongs to task {expected}, refused write from task {actual}")]
    WrongTask { expected: TaskId, actual: TaskId },
    #[error("entry content is empty")]
    EmptyEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scratchpad {
    pub task_id: TaskId,
    /// Append-only log. The orchestrator hands out `&Scratchpad` to
    /// the Critic so it can read but not modify; only the Worker
    /// holds `&mut Scratchpad`.
    pub entries: Vec<ScratchEntry>,
    /// Running token count — sum of `tokens_used` for budget gating.
    pub cost_tokens: u64,
}

impl Scratchpad {
    #[must_use]
    pub fn new(task_id: TaskId) -> Self {
        Self {
            task_id,
            entries: Vec::new(),
            cost_tokens: 0,
        }
    }

    /// Append an entry. Refuses to write if the caller passes the
    /// wrong task id (defensive: catches misrouted Worker dispatches
    /// in tests).
    ///
    /// # Errors
    /// `WrongTask` if `task_id` doesn't match this scratchpad's
    /// quarantine key. `EmptyEntry` if `content` is empty.
    pub fn append(
        &mut self,
        task_id: TaskId,
        content: String,
        tokens_used: Option<u32>,
    ) -> Result<&ScratchEntry, ScratchpadError> {
        if task_id != self.task_id {
            return Err(ScratchpadError::WrongTask {
                expected: self.task_id,
                actual: task_id,
            });
        }
        if content.trim().is_empty() {
            return Err(ScratchpadError::EmptyEntry);
        }
        self.entries.push(ScratchEntry {
            at: Utc::now(),
            content,
            tokens_used,
        });
        if let Some(n) = tokens_used {
            self.cost_tokens = self.cost_tokens.saturating_add(u64::from(n));
        }
        Ok(self.entries.last().unwrap())
    }

    /// Read-only view. Critic gets `&Scratchpad` — there is no
    /// `&mut` accessor exposed beyond `append`, so the Critic
    /// cannot accidentally promote draft state.
    #[must_use]
    pub fn entries(&self) -> &[ScratchEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_with_matching_task_id_succeeds() {
        let id = TaskId::new();
        let mut s = Scratchpad::new(id);
        let entry = s.append(id, "first draft".into(), Some(120)).unwrap();
        assert_eq!(entry.content, "first draft");
        assert_eq!(entry.tokens_used, Some(120));
        assert_eq!(s.cost_tokens, 120);
        assert_eq!(s.entries().len(), 1);
    }

    #[test]
    fn append_with_wrong_task_id_refused() {
        let owner = TaskId::new();
        let other = TaskId::new();
        let mut s = Scratchpad::new(owner);
        let err = s.append(other, "leak".into(), None).unwrap_err();
        match err {
            ScratchpadError::WrongTask { expected, actual } => {
                assert_eq!(expected, owner);
                assert_eq!(actual, other);
            }
            _ => panic!("expected WrongTask"),
        }
        // Quarantine still intact — no entry was added.
        assert!(s.entries().is_empty());
    }

    #[test]
    fn append_empty_content_rejected() {
        let id = TaskId::new();
        let mut s = Scratchpad::new(id);
        let err = s.append(id, "   \n".into(), None).unwrap_err();
        assert!(matches!(err, ScratchpadError::EmptyEntry));
        assert!(s.entries().is_empty());
    }

    #[test]
    fn cost_tokens_accumulates() {
        let id = TaskId::new();
        let mut s = Scratchpad::new(id);
        s.append(id, "a".into(), Some(50)).unwrap();
        s.append(id, "b".into(), Some(75)).unwrap();
        s.append(id, "c".into(), None).unwrap();
        assert_eq!(s.cost_tokens, 125);
    }

    #[test]
    fn scratchpad_round_trips_through_serde() {
        let id = TaskId::new();
        let mut s = Scratchpad::new(id);
        s.append(id, "draft".into(), Some(10)).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: Scratchpad = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
