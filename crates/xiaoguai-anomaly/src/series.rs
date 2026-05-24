/// Sliding-window time-series buffer.
///
/// Holds `(timestamp, value)` pairs in insertion order.  Points older
/// than `window` are pruned lazily on every `push`.
use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};

/// A fixed-window time-series buffer.
#[derive(Debug, Clone)]
pub struct TimeSeries {
    /// Ordered buffer of `(timestamp, value)` pairs.
    pub points: VecDeque<(DateTime<Utc>, f64)>,
    /// How far back to retain data.
    pub window: Duration,
}

impl TimeSeries {
    /// Create an empty series with the given retention window.
    pub fn new(window: Duration) -> Self {
        Self {
            points: VecDeque::new(),
            window,
        }
    }

    /// Append a new observation and prune stale points.
    pub fn push(&mut self, ts: DateTime<Utc>, value: f64) {
        self.points.push_back((ts, value));
        self.prune(ts);
    }

    /// Remove points that fall outside `[now - window, now]`.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        while let Some(&(ts, _)) = self.points.front() {
            if ts < cutoff {
                self.points.pop_front();
            } else {
                break;
            }
        }
    }

    /// Number of retained points.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// `true` when no points are retained.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn push_retains_recent() {
        let mut ts = TimeSeries::new(Duration::seconds(60));
        ts.push(t(0), 1.0);
        ts.push(t(30), 2.0);
        ts.push(t(60), 3.0);
        assert_eq!(ts.len(), 3);
    }

    #[test]
    fn prune_removes_old() {
        let mut ts = TimeSeries::new(Duration::seconds(60));
        ts.push(t(0), 1.0);
        ts.push(t(30), 2.0);
        ts.push(t(70), 3.0);
        // At t=120: cutoff = 60. t=0 < 60 → evicted. t=30 < 60 → evicted.
        // t=70 >= 60 → retained. t=120 retained.
        ts.push(t(120), 4.0);
        assert_eq!(ts.len(), 2, "only t=70 and t=120 should remain");
        assert_eq!(ts.points.front().unwrap().0, t(70));
    }

    #[test]
    fn empty_series() {
        let ts = TimeSeries::new(Duration::seconds(60));
        assert!(ts.is_empty());
    }
}
