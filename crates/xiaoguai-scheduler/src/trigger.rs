//! Triggers — when does a job fire.
//!
//! v0.10.0 shipped two scheduled variants ([`Trigger::Cron`] and
//! [`Trigger::Interval`]); v0.10.1 adds four reactive variants:
//!
//! * [`Trigger::FileWatch`] — fire when a path changes (via the
//!   `notify` crate; production source lives in [`crate::sources`]).
//! * [`Trigger::Webhook`] — fire when xiaoguai-api receives an
//!   inbound HTTP call routed by `route_id`. v0.10.1 ships the
//!   in-process source; the HTTP route wiring lands when the runtime
//!   extraction in v0.12.0 brings axum into the same dependency band.
//! * [`Trigger::GitPush`] — placeholder for v0.10.1.x; lets a job be
//!   stored against a `(repo_url, branch)` pair so the data model is
//!   stable. No concrete source ships yet.
//! * [`Trigger::DbPoll`] — placeholder for v0.10.1.x once the PG
//!   scheduler repos land in v0.12.0. Data only.
//!
//! Reactive variants return `None` from [`Trigger::next_fire_after`]
//! — they don't have a wall-clock schedule. The runner skips them in
//! the timer loop and only fires them when a matching event arrives
//! on the [`crate::trigger_source::TriggerEvent`] channel.
//!
//! v0.10.2 adds the *proactive* third leg of the
//! `passive → reactive → proactive` ladder (roadmap §3): a cheap-model
//! check-prompt runs every `interval_secs`, and the executor only fires
//! when the check returns a non-empty reason. Proactive triggers are
//! [`Trigger::is_scheduled`] — they show up in `list_due` like Interval
//! — but the runner routes them through a separate
//! [`crate::proactive::ProactiveChecker`] seam before deciding whether
//! to actually run the job. Per-user budgets and reason-required push
//! payloads are non-negotiable per roadmap §5.5.
//!
//! Cron stays UTC by design; tenants who want local time offset in
//! their expression (see roadmap §5.3).

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TriggerError {
    #[error("cron parse: {0}")]
    CronParse(String),
    #[error("interval must be > 0 seconds")]
    InvalidInterval,
    #[error("file watch path must be non-empty")]
    EmptyPath,
    #[error("webhook route_id must be non-empty")]
    EmptyRouteId,
    #[error("git repo_url must be non-empty")]
    EmptyRepoUrl,
    #[error("db poll query must be non-empty")]
    EmptyQuery,
    #[error("proactive check prompt must be non-empty")]
    EmptyCheckPrompt,
    #[error("proactive interval must be > 0 seconds")]
    InvalidProactiveInterval,
}

/// When a job should fire.
///
/// Serde uses an internally-tagged representation so the JSON form
/// (which lands in `scheduled_jobs.trigger`) is human-friendly:
///
/// ```json
/// { "type": "cron", "expr": "0 0 * * * *" }
/// { "type": "interval", "secs": 3600 }
/// { "type": "file_watch", "path": "/var/notes" }
/// { "type": "webhook", "route_id": "deploy-prod" }
/// { "type": "git_push", "repo_url": "https://...", "branch": "main" }
/// { "type": "db_poll", "query": "SELECT id FROM ..." }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Cron {
        expr: String,
    },
    Interval {
        secs: u64,
    },
    /// Fire when the file system path changes. The matching
    /// `FileWatchSource` translates `notify` events into
    /// `TriggerEvent`s targeted at every job whose `path` is an
    /// ancestor of (or equal to) the changed path.
    FileWatch {
        path: String,
    },
    /// Fire when a webhook routed by `route_id` arrives. v0.10.1
    /// ships the in-process producer; the HTTP route is wired in
    /// `xiaoguai-api` in a later tag.
    Webhook {
        route_id: String,
    },
    /// Fire on a git push to `(repo_url, branch)`. v0.10.1 ships the
    /// data variant only — concrete polling/webhook source lands in
    /// v0.10.1.x.
    GitPush {
        repo_url: String,
        branch: String,
    },
    /// Fire when a SQL query returns a non-empty row set. v0.10.1
    /// ships the data variant only — concrete poller lands in
    /// v0.10.1.x alongside the PG repository impls.
    DbPoll {
        query: String,
    },
    /// Proactive: every `interval_secs` the runner asks a cheap model
    /// (via [`crate::proactive::ProactiveChecker`]) whether the
    /// `check_prompt` warrants firing the real executor. The job only
    /// runs (and only consumes a push-budget slot) when the checker
    /// returns `Some(reason)`. See roadmap §5.5 for the budget /
    /// reason-required contract.
    Proactive {
        check_prompt: String,
        interval_secs: u64,
    },
}

impl Trigger {
    /// Construct a Cron trigger, validating the expression eagerly.
    pub fn cron(expr: impl Into<String>) -> Result<Self, TriggerError> {
        let expr = expr.into();
        cron::Schedule::from_str(&expr).map_err(|e| TriggerError::CronParse(e.to_string()))?;
        Ok(Self::Cron { expr })
    }

    /// Construct an Interval trigger. `secs` must be non-zero.
    pub fn interval(secs: u64) -> Result<Self, TriggerError> {
        if secs == 0 {
            return Err(TriggerError::InvalidInterval);
        }
        Ok(Self::Interval { secs })
    }

    /// Construct a [`Trigger::FileWatch`] trigger.
    pub fn file_watch(path: impl Into<String>) -> Result<Self, TriggerError> {
        let path = path.into();
        if path.is_empty() {
            return Err(TriggerError::EmptyPath);
        }
        Ok(Self::FileWatch { path })
    }

    /// Construct a [`Trigger::Webhook`] trigger.
    pub fn webhook(route_id: impl Into<String>) -> Result<Self, TriggerError> {
        let route_id = route_id.into();
        if route_id.is_empty() {
            return Err(TriggerError::EmptyRouteId);
        }
        Ok(Self::Webhook { route_id })
    }

    /// Construct a [`Trigger::GitPush`] trigger.
    pub fn git_push(
        repo_url: impl Into<String>,
        branch: impl Into<String>,
    ) -> Result<Self, TriggerError> {
        let repo_url = repo_url.into();
        let branch = branch.into();
        if repo_url.is_empty() {
            return Err(TriggerError::EmptyRepoUrl);
        }
        Ok(Self::GitPush { repo_url, branch })
    }

    /// Construct a [`Trigger::DbPoll`] trigger.
    pub fn db_poll(query: impl Into<String>) -> Result<Self, TriggerError> {
        let query = query.into();
        if query.is_empty() {
            return Err(TriggerError::EmptyQuery);
        }
        Ok(Self::DbPoll { query })
    }

    /// Construct a [`Trigger::Proactive`] trigger. Both arguments must
    /// be non-trivial: empty `check_prompt` or zero `interval_secs`
    /// would make the trigger silently degenerate.
    pub fn proactive(
        check_prompt: impl Into<String>,
        interval_secs: u64,
    ) -> Result<Self, TriggerError> {
        let check_prompt = check_prompt.into();
        if check_prompt.is_empty() {
            return Err(TriggerError::EmptyCheckPrompt);
        }
        if interval_secs == 0 {
            return Err(TriggerError::InvalidProactiveInterval);
        }
        Ok(Self::Proactive {
            check_prompt,
            interval_secs,
        })
    }

    /// True iff the trigger has a wall-clock schedule. Scheduled
    /// triggers are visible to `JobRepository::list_due`; reactive
    /// triggers are only fired via the event channel.
    ///
    /// Proactive triggers count as scheduled — they tick on their own
    /// `interval_secs` — but the runner routes them through the
    /// [`crate::proactive::ProactiveChecker`] before deciding to fire
    /// the executor.
    #[must_use]
    pub const fn is_scheduled(&self) -> bool {
        matches!(
            self,
            Self::Cron { .. } | Self::Interval { .. } | Self::Proactive { .. }
        )
    }

    /// True iff the trigger fires on external events rather than on a
    /// schedule.
    #[must_use]
    pub const fn is_reactive(&self) -> bool {
        !self.is_scheduled()
    }

    /// True iff the trigger needs to go through the proactive checker
    /// before firing. The runner uses this to decide between the
    /// straight-fire path and the check-first path.
    #[must_use]
    pub const fn is_proactive(&self) -> bool {
        matches!(self, Self::Proactive { .. })
    }

    /// Compute the next fire time strictly after `after`.
    ///
    /// Returns `None` if the trigger has no future fire — either
    /// because it's a reactive trigger (no wall-clock schedule) or
    /// because the cron expression only matches dates already in the
    /// past.
    pub fn next_fire_after(
        &self,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, TriggerError> {
        match self {
            Self::Cron { expr } => {
                let schedule = cron::Schedule::from_str(expr)
                    .map_err(|e| TriggerError::CronParse(e.to_string()))?;
                Ok(schedule.after(&after).next())
            }
            Self::Interval { secs } => {
                let d = Duration::seconds(i64::try_from(*secs).unwrap_or(i64::MAX));
                Ok(Some(after + d))
            }
            Self::Proactive { interval_secs, .. } => {
                let d = Duration::seconds(i64::try_from(*interval_secs).unwrap_or(i64::MAX));
                Ok(Some(after + d))
            }
            Self::FileWatch { .. }
            | Self::Webhook { .. }
            | Self::GitPush { .. }
            | Self::DbPoll { .. } => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cron_parses_six_field_expression() {
        // Every minute, on the second 0.
        let t = Trigger::cron("0 * * * * *").unwrap();
        let after = Utc.with_ymd_and_hms(2026, 5, 23, 10, 30, 15).unwrap();
        let next = t.next_fire_after(after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 23, 10, 31, 0).unwrap());
    }

    #[test]
    fn cron_top_of_hour_lands_at_next_hour() {
        // Top of every hour.
        let t = Trigger::cron("0 0 * * * *").unwrap();
        let after = Utc.with_ymd_and_hms(2026, 5, 23, 10, 30, 0).unwrap();
        let next = t.next_fire_after(after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 23, 11, 0, 0).unwrap());
    }

    #[test]
    fn cron_rejects_garbage() {
        let err = Trigger::cron("not a cron").unwrap_err();
        assert!(matches!(err, TriggerError::CronParse(_)));
    }

    #[test]
    fn interval_advances_by_secs() {
        let t = Trigger::interval(60).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap();
        let next = t.next_fire_after(after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 23, 10, 1, 0).unwrap());
    }

    #[test]
    fn interval_zero_rejected() {
        assert!(matches!(
            Trigger::interval(0).unwrap_err(),
            TriggerError::InvalidInterval
        ));
    }

    #[test]
    fn serde_round_trip_cron() {
        let t = Trigger::cron("0 0 * * * *").unwrap();
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, r#"{"type":"cron","expr":"0 0 * * * *"}"#);
        let back: Trigger = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn serde_round_trip_interval() {
        let t = Trigger::interval(3600).unwrap();
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, r#"{"type":"interval","secs":3600}"#);
        let back: Trigger = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn file_watch_rejects_empty_path() {
        assert!(matches!(
            Trigger::file_watch("").unwrap_err(),
            TriggerError::EmptyPath
        ));
    }

    #[test]
    fn webhook_rejects_empty_route_id() {
        assert!(matches!(
            Trigger::webhook("").unwrap_err(),
            TriggerError::EmptyRouteId
        ));
    }

    #[test]
    fn git_push_rejects_empty_repo() {
        assert!(matches!(
            Trigger::git_push("", "main").unwrap_err(),
            TriggerError::EmptyRepoUrl
        ));
    }

    #[test]
    fn db_poll_rejects_empty_query() {
        assert!(matches!(
            Trigger::db_poll("").unwrap_err(),
            TriggerError::EmptyQuery
        ));
    }

    #[test]
    fn reactive_triggers_have_no_next_fire() {
        let fw = Trigger::file_watch("/tmp").unwrap();
        let wh = Trigger::webhook("deploy").unwrap();
        let gp = Trigger::git_push("https://x", "main").unwrap();
        let dp = Trigger::db_poll("SELECT 1").unwrap();
        let now = Utc::now();
        for t in [&fw, &wh, &gp, &dp] {
            assert!(t.is_reactive());
            assert!(!t.is_scheduled());
            assert!(!t.is_proactive());
            assert_eq!(t.next_fire_after(now).unwrap(), None);
        }
    }

    #[test]
    fn proactive_rejects_empty_prompt() {
        assert!(matches!(
            Trigger::proactive("", 60).unwrap_err(),
            TriggerError::EmptyCheckPrompt
        ));
    }

    #[test]
    fn proactive_rejects_zero_interval() {
        assert!(matches!(
            Trigger::proactive("any change in inbox?", 0).unwrap_err(),
            TriggerError::InvalidProactiveInterval
        ));
    }

    #[test]
    fn proactive_is_scheduled_and_ticks_on_interval() {
        let t = Trigger::proactive("Is there anything new?", 300).unwrap();
        assert!(t.is_scheduled());
        assert!(t.is_proactive());
        assert!(!t.is_reactive());
        let after = Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap();
        let next = t.next_fire_after(after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 23, 10, 5, 0).unwrap());
    }

    #[test]
    fn serde_round_trip_proactive() {
        let t = Trigger::proactive("Scan HN for new ML posts", 1800).unwrap();
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(
            s,
            r#"{"type":"proactive","check_prompt":"Scan HN for new ML posts","interval_secs":1800}"#
        );
        let back: Trigger = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn serde_round_trip_reactive_variants() {
        let cases = [
            (
                Trigger::file_watch("/var/notes").unwrap(),
                r#"{"type":"file_watch","path":"/var/notes"}"#,
            ),
            (
                Trigger::webhook("deploy-prod").unwrap(),
                r#"{"type":"webhook","route_id":"deploy-prod"}"#,
            ),
            (
                Trigger::git_push("https://github.com/x/y", "main").unwrap(),
                r#"{"type":"git_push","repo_url":"https://github.com/x/y","branch":"main"}"#,
            ),
            (
                Trigger::db_poll("SELECT 1").unwrap(),
                r#"{"type":"db_poll","query":"SELECT 1"}"#,
            ),
        ];
        for (t, want) in cases {
            let s = serde_json::to_string(&t).unwrap();
            assert_eq!(s, want, "encode {t:?}");
            let back: Trigger = serde_json::from_str(&s).unwrap();
            assert_eq!(back, t, "decode {want}");
        }
    }
}
