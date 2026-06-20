//! Shared primitives for the `xiaoguai-im-*` webhook adapters (Phase D / DEC-041).
//!
//! These were copy-pasted across the feishu / dingtalk / discord / slack /
//! wecom / telegram / mattermost adapters. They are **security-sensitive** (constant-time
//! signature comparison + SEC-05 replay window), so a single audited
//! implementation is the whole point: one place to get the timing-safe compare
//! and the freshness check right, rather than N slightly-different copies.

#![forbid(unsafe_code)]

/// Replay window (seconds) for inbound webhook timestamps — SEC-05. A request
/// whose claimed timestamp is more than this from local wall-clock is rejected.
pub const TIMESTAMP_TOLERANCE_SECS: i64 = 300;

/// Current Unix time in seconds. Falls back to `0` when the system clock
/// reports a pre-epoch time, which pushes every inbound timestamp outside the
/// replay window (fail-closed).
#[must_use]
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// True when `ts` is within ±[`TIMESTAMP_TOLERANCE_SECS`] of `now` (both Unix
/// seconds) — i.e. inside the SEC-05 replay window.
#[must_use]
pub fn timestamp_within_tolerance(ts: i64, now: i64) -> bool {
    (ts - now).abs() <= TIMESTAMP_TOLERANCE_SECS
}

/// Constant-time byte-slice equality. Unequal lengths short-circuit to `false`
/// (length is not secret); equal-length inputs are compared in time independent
/// of where they first differ, so signature checks never leak the match
/// position through timing.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_identical() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn ct_eq_rejects_different_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn ct_eq_rejects_different_length() {
        assert!(!constant_time_eq(b"hi", b"hix"));
    }

    #[test]
    fn tolerance_window_edges() {
        let now = 1_000_000;
        assert!(timestamp_within_tolerance(now, now));
        assert!(timestamp_within_tolerance(
            now - TIMESTAMP_TOLERANCE_SECS,
            now
        ));
        assert!(timestamp_within_tolerance(
            now + TIMESTAMP_TOLERANCE_SECS,
            now
        ));
        assert!(!timestamp_within_tolerance(
            now - TIMESTAMP_TOLERANCE_SECS - 1,
            now
        ));
        assert!(!timestamp_within_tolerance(
            now + TIMESTAMP_TOLERANCE_SECS + 1,
            now
        ));
    }
}
