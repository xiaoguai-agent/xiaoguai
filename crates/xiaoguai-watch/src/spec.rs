//! [`WatchSpec`] — declarative watcher definition.
//!
//! A spec ties together:
//! - a unique `id` (used in dedup fingerprints and log spans)
//! - a `source` (SQL query or HTTP poll)
//! - an optional cron/interval `schedule` (defaults to 60-second interval)
//! - an `on_match` action reference emitted as [`crate::runner::WatchEvent`]
//!
//! Specs are designed for YAML/JSON config files:
//!
//! ```yaml
//! id: ar-aging-alert
//! source:
//!   sql: "SELECT tenant_id, customer, dso FROM ar_aging WHERE dso > 60"
//! schedule:
//!   interval_secs: 86400
//! on_match:
//!   action: notify
//!   target: ops-channel
//! ```

use serde::{Deserialize, Serialize};

/// A reference to the action taken when a match is emitted.
/// Intentionally opaque — the action is dispatched by the caller
/// (scheduler integrator) that consumes `WatchEvent`s from the channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionRef {
    /// Logical action type (e.g. `"notify"`, `"create_task"`, `"webhook"`).
    pub action: String,
    /// Arbitrary target string interpreted by the action handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Extra key-value metadata forwarded verbatim in the `WatchEvent`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// Where to poll for matches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchSourceSpec {
    /// Run a SQL SELECT; each result row is a potential match.
    ///
    /// The query **must** be a SELECT — the runner validates that the
    /// normalised statement starts with `SELECT` (case-insensitive).
    Sql {
        /// SQL query string.  Use `$1`, `$2` … for bind params (none for
        /// `WatchSpec`-level queries; parameterised queries are reserved
        /// for v1.3.x dynamic-binding extension).
        query: String,
    },
    /// Poll an HTTP endpoint; `JSONPath` expression extracts match rows.
    Http {
        /// Target URL.
        url: String,
        /// `JSONPath` expression selecting an array of objects from the
        /// JSON response body.  Defaults to `"$[*]"` (top-level array).
        #[serde(default = "default_jsonpath")]
        jsonpath: String,
        /// HTTP method.  Defaults to `"GET"`.
        #[serde(default = "default_method")]
        method: String,
    },
}

fn default_jsonpath() -> String {
    "$[*]".to_string()
}

fn default_method() -> String {
    "GET".to_string()
}

/// How often the watcher ticks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchSchedule {
    /// ISO 8601 cron expression (6-field: sec min h dom mon dow).
    Cron { expr: String },
    /// Fixed interval in seconds.
    IntervalSecs { secs: u64 },
}

impl Default for WatchSchedule {
    fn default() -> Self {
        Self::IntervalSecs { secs: 60 }
    }
}

/// Declarative watcher specification.
///
/// Deserialises from YAML or JSON.  One `WatchSpec` drives one
/// [`crate::runner::WatchRunner`] watch slot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchSpec {
    /// Unique watcher ID.  Used as the dedup namespace so IDs must be
    /// stable across restarts (fingerprints are keyed by `id + row_hash`).
    pub id: String,
    /// Data source to poll.
    pub source: WatchSourceSpec,
    /// Poll frequency.  Defaults to a 60-second interval.
    #[serde(default)]
    pub schedule: WatchSchedule,
    /// Action to take when a new (non-deduplicated) match is found.
    pub on_match: ActionRef,
}

impl WatchSpec {
    /// Validate the spec, returning an error string on the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("WatchSpec.id must not be empty".into());
        }
        match &self.source {
            WatchSourceSpec::Sql { query } => {
                if query.trim().is_empty() {
                    return Err("WatchSpec.source.sql.query must not be empty".into());
                }
                let normalised = query.trim().to_ascii_uppercase();
                if !normalised.starts_with("SELECT") {
                    return Err(format!(
                        "WatchSpec.source.sql.query must be a SELECT statement (got: {normalised:.40}…)"
                    ));
                }
            }
            WatchSourceSpec::Http { url, .. } => {
                if url.is_empty() {
                    return Err("WatchSpec.source.http.url must not be empty".into());
                }
            }
        }
        if self.on_match.action.is_empty() {
            return Err("WatchSpec.on_match.action must not be empty".into());
        }
        if let WatchSchedule::IntervalSecs { secs } = self.schedule {
            if secs == 0 {
                return Err("WatchSpec.schedule.interval_secs must be > 0".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_sql_spec() -> WatchSpec {
        WatchSpec {
            id: "test-spec".into(),
            source: WatchSourceSpec::Sql {
                query: "SELECT id FROM events WHERE ts > now() - interval '1 hour'".into(),
            },
            schedule: WatchSchedule::default(),
            on_match: ActionRef {
                action: "notify".into(),
                target: Some("ops".into()),
                params: serde_json::Map::new(),
            },
        }
    }

    #[test]
    fn yaml_round_trip() {
        let spec = minimal_sql_spec();
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let back: WatchSpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn json_round_trip() {
        let spec = minimal_sql_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: WatchSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn validate_empty_id_rejected() {
        let mut spec = minimal_sql_spec();
        spec.id = String::new();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_non_select_rejected() {
        let mut spec = minimal_sql_spec();
        spec.source = WatchSourceSpec::Sql {
            query: "DELETE FROM events".into(),
        };
        let err = spec.validate().unwrap_err();
        assert!(err.contains("SELECT"), "expected SELECT mention in: {err}");
    }

    #[test]
    fn validate_empty_query_rejected() {
        let mut spec = minimal_sql_spec();
        spec.source = WatchSourceSpec::Sql {
            query: "   ".into(),
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_zero_interval_rejected() {
        let mut spec = minimal_sql_spec();
        spec.schedule = WatchSchedule::IntervalSecs { secs: 0 };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_empty_action_rejected() {
        let mut spec = minimal_sql_spec();
        spec.on_match.action = String::new();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn validate_http_empty_url_rejected() {
        let mut spec = minimal_sql_spec();
        spec.source = WatchSourceSpec::Http {
            url: String::new(),
            jsonpath: "$[*]".into(),
            method: "GET".into(),
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn default_schedule_is_60s_interval() {
        let sched = WatchSchedule::default();
        assert_eq!(sched, WatchSchedule::IntervalSecs { secs: 60 });
    }

    #[test]
    fn http_source_defaults() {
        let yaml = r#"
id: http-test
source:
  kind: http
  url: "https://example.com/api"
on_match:
  action: notify
"#;
        let spec: WatchSpec = serde_yaml::from_str(yaml).unwrap();
        match &spec.source {
            WatchSourceSpec::Http {
                jsonpath, method, ..
            } => {
                assert_eq!(jsonpath, "$[*]");
                assert_eq!(method, "GET");
            }
            other @ WatchSourceSpec::Sql { .. } => panic!("unexpected source: {other:?}"),
        }
        spec.validate().unwrap();
    }
}
