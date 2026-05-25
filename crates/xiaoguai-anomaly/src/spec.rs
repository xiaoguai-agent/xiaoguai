/// Declarative wiring spec for an anomaly monitor.
///
/// `AnomalySpec` ties together a KPI query, time window, detector
/// configuration, and the action to take when an anomaly fires.
/// At runtime the scheduler reads these specs from the registry and
/// wires them into a polling or push-based observation loop.
use chrono::Duration;
use serde::{Deserialize, Serialize};

// ── ActionRef ──────────────────────────────────────────────────────────────

/// Reference to the action that should execute when an anomaly fires.
///
/// Mirrors the `ActionRef` concept used by the scheduler, kept intentionally
/// minimal here so that `xiaoguai-anomaly` stays free of heavy workspace
/// dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionRef {
    /// Wake a named session with a pre-built prompt.
    WakeSession {
        /// Session name or ID pattern.
        session: String,
        /// Prompt template.  Use `{anomaly}` as a placeholder.
        prompt_template: String,
    },
    /// Emit a notification to an IM channel.
    Notify {
        /// Target channel identifier (e.g. `"feishu:#ops-alert"`).
        channel: String,
    },
    /// Run a named webhook trigger.
    Webhook {
        /// Webhook route ID as registered in the scheduler.
        route_id: String,
    },
}

// ── DetectorKind ───────────────────────────────────────────────────────────

/// Declarative detector selection baked into a spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetectorKind {
    ZScore {
        /// Alert threshold in standard deviations (default: 3.0).
        sigma_threshold: f64,
        /// Minimum number of observations before arming (default: 10).
        min_count: u64,
    },
    Ewma {
        /// Smoothing factor α in (0, 1) (default: 0.1).
        alpha: f64,
        /// Alert threshold in EWMA standard deviations (default: 3.0).
        sigma_threshold: f64,
        /// Minimum number of observations before arming (default: 10).
        min_count: u64,
    },
}

impl Default for DetectorKind {
    fn default() -> Self {
        Self::ZScore {
            sigma_threshold: 3.0,
            min_count: 10,
        }
    }
}

// ── AnomalySpec ────────────────────────────────────────────────────────────

/// Complete declarative spec for one anomaly monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalySpec {
    /// Unique name for this anomaly monitor.
    pub id: String,
    /// KPI query string — interpretation is left to the surrounding system
    /// (e.g. a Prometheus `instant_vector` expression or a SQL snippet).
    pub kpi_query: String,
    /// Rolling window for the time-series buffer.
    #[serde(with = "duration_secs")]
    pub window: Duration,
    /// Detector configuration.
    pub detector: DetectorKind,
    /// Cooldown between successive alerts for this spec.
    #[serde(with = "duration_secs")]
    pub cool_off: Duration,
    /// What to do when an anomaly fires.
    pub on_anomaly: ActionRef,
}

// ── Duration serde helper ──────────────────────────────────────────────────

mod duration_secs {
    use chrono::Duration;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.num_seconds().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = i64::deserialize(d)?;
        Ok(Duration::seconds(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_json() {
        let spec = AnomalySpec {
            id: "orders_anomaly".to_string(),
            kpi_query: "SELECT COUNT(*) FROM orders WHERE created_at > NOW() - INTERVAL '1 minute'"
                .to_string(),
            window: Duration::hours(2),
            detector: DetectorKind::ZScore {
                sigma_threshold: 3.0,
                min_count: 10,
            },
            cool_off: Duration::minutes(15),
            on_anomaly: ActionRef::WakeSession {
                session: "ops-agent".to_string(),
                prompt_template: "Anomaly detected: {anomaly}".to_string(),
            },
        };

        let json = serde_json::to_string(&spec).expect("serialize");
        let back: AnomalySpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, spec.id);
        assert_eq!(back.window, spec.window);
        assert_eq!(back.cool_off, spec.cool_off);
    }

    #[test]
    fn ewma_spec_round_trips() {
        let spec = AnomalySpec {
            id: "latency_ewma".to_string(),
            kpi_query: "avg(http_request_duration_seconds)".to_string(),
            window: Duration::minutes(30),
            detector: DetectorKind::Ewma {
                alpha: 0.15,
                sigma_threshold: 2.5,
                min_count: 5,
            },
            cool_off: Duration::minutes(10),
            on_anomaly: ActionRef::Notify {
                channel: "feishu:#ops-alert".to_string(),
            },
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: AnomalySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, spec.id);
        match back.detector {
            DetectorKind::Ewma { alpha, .. } => {
                assert!((alpha - 0.15).abs() < 1e-12);
            }
            DetectorKind::ZScore { .. } => panic!("unexpected ZScore detector kind"),
        }
    }
}
