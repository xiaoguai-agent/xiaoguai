//! Triggers — when does a job fire.
//!
//! v0.10.0 ships two variants:
//!
//! * [`Trigger::Cron`] — a 6-field expression (sec min hour dom mon dow)
//!   parsed by the `cron` crate. UTC by design; tenants who want local
//!   time must offset in their expression (we deliberately don't carry
//!   per-job timezones — see decision in v0.9–v0.12 roadmap §5.3).
//! * [`Trigger::Interval`] — fire every `N` seconds after `last_fire`,
//!   or after `created_at` if the job hasn't fired yet.
//!
//! Reactive triggers (file watcher, webhook, git push) and proactive
//! triggers (LLM-initiated) are v0.10.1 / v0.10.2 — both will land as
//! additional variants on this enum so the runner doesn't fork.

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
}

/// When a job should fire.
///
/// Serde uses an internally-tagged representation so the JSON form
/// (which lands in `scheduled_jobs.trigger`) is human-friendly:
///
/// ```json
/// { "type": "cron", "expr": "0 0 * * * *" }
/// { "type": "interval", "secs": 3600 }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Cron { expr: String },
    Interval { secs: u64 },
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

    /// Compute the next fire time strictly after `after`.
    ///
    /// Returns `None` if the trigger has no future fire (e.g. a Cron
    /// expression that only matches dates already in the past — the
    /// `cron` crate can yield this for some hand-crafted exprs).
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
}
