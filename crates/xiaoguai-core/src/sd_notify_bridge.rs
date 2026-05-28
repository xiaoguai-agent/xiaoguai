//! Thin helpers around the `sd-notify` crate (v1.1.6.2).
//!
//! All public items are present on every platform but are no-ops outside
//! Linux. This lets call sites use them unconditionally without a mass of
//! `#[cfg(…)]` scattered through `main.rs`.
//!
//! ## Watchdog ticker
//!
//! [`spawn_watchdog_ticker`] returns an `Option<tokio::task::JoinHandle<()>>`:
//! - On Linux, when `WATCHDOG_USEC` is set by systemd, it spawns a task
//!   that sends `WATCHDOG=1` at roughly half the watchdog interval.
//! - On Linux, when `WATCHDOG_USEC` is absent, or on non-Linux, returns
//!   `None` so the caller can skip the `abort()` step.
//!
//! ## Usage
//!
//! ```ignore
//! // After all subsystems are ready (called from xiaoguai_core::run_serve):
//! sd_notify_bridge::notify_ready();
//!
//! // Spawn the watchdog ticker (holds the JoinHandle until shutdown):
//! let wd = sd_notify_bridge::spawn_watchdog_ticker();
//!
//! // … run the server …
//!
//! // On graceful shutdown:
//! sd_notify_bridge::notify_stopping();
//! if let Some(h) = wd { h.abort(); }
//! ```

/// Send `READY=1` to the systemd supervisor socket.
///
/// No-op on non-Linux or when `NOTIFY_SOCKET` is not set.
pub fn notify_ready() {
    #[cfg(target_os = "linux")]
    {
        use sd_notify::NotifyState;
        if let Err(e) = sd_notify::notify(&[NotifyState::Ready]) {
            tracing::warn!(error = %e, "sd_notify: failed to send READY=1 (non-fatal)");
        } else {
            tracing::debug!("sd_notify: READY=1 sent");
        }
    }
}

/// Send `STOPPING=1` to the systemd supervisor socket.
///
/// No-op on non-Linux or when `NOTIFY_SOCKET` is not set.
pub fn notify_stopping() {
    #[cfg(target_os = "linux")]
    {
        use sd_notify::NotifyState;
        if let Err(e) = sd_notify::notify(&[NotifyState::Stopping]) {
            tracing::warn!(error = %e, "sd_notify: failed to send STOPPING=1 (non-fatal)");
        } else {
            tracing::debug!("sd_notify: STOPPING=1 sent");
        }
    }
}

/// Spawn an async watchdog ping task when systemd has set `WATCHDOG_USEC`.
///
/// Returns `Some(handle)` if a task was spawned; `None` otherwise (including
/// on non-Linux platforms, so callers never call `.abort()` on nothing).
///
/// The ping interval is `WATCHDOG_USEC / 2` so we stay comfortably inside
/// the deadline under moderate load.
#[must_use]
pub fn spawn_watchdog_ticker() -> Option<tokio::task::JoinHandle<()>> {
    #[cfg(target_os = "linux")]
    {
        // sd-notify 0.5: watchdog_enabled() -> Option<Duration>, returning the
        // watchdog interval directly (was a (bool, &mut u64) out-param in 0.4).
        if let Some(watchdog) = sd_notify::watchdog_enabled() {
            let ping_interval = watchdog / 2;
            tracing::info!(
                watchdog_usec = u64::try_from(watchdog.as_micros()).unwrap_or(u64::MAX),
                ping_interval_ms = ping_interval.as_millis(),
                "sd_notify: watchdog enabled — spawning ping task"
            );
            let handle = tokio::spawn(async move {
                use sd_notify::NotifyState;
                let mut ticker = tokio::time::interval(ping_interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    if let Err(e) = sd_notify::notify(&[NotifyState::Watchdog]) {
                        tracing::warn!(
                            error = %e,
                            "sd_notify: WATCHDOG=1 ping failed — watchdog will expire"
                        );
                    }
                }
            });
            return Some(handle);
        }
        tracing::debug!("sd_notify: WATCHDOG_USEC not set — watchdog ticker not started");
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Non-Linux compile tests -----------------------------------------
    // These just verify the no-op path builds and doesn't panic.

    #[test]
    fn notify_ready_is_noop_without_socket() {
        // NOTIFY_SOCKET is not set in the test environment.
        // On Linux this exercises the sd_notify path (which silently
        // does nothing when NOTIFY_SOCKET is absent). On non-Linux the
        // cfg gate keeps it a true no-op.
        notify_ready();
    }

    #[test]
    fn notify_stopping_is_noop_without_socket() {
        notify_stopping();
    }

    #[test]
    fn spawn_watchdog_ticker_returns_none_without_usec() {
        // WATCHDOG_USEC is not set in the test environment.
        let handle = spawn_watchdog_ticker();
        assert!(
            handle.is_none(),
            "expected None when WATCHDOG_USEC is not set"
        );
    }

    // ---- Linux-only socket test ------------------------------------------
    // Spins up a real Unix-domain socket in a tempdir, sets NOTIFY_SOCKET,
    // calls notify_ready(), and checks the socket received the payload.

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn notify_ready_sends_payload_to_socket() {
        use std::os::unix::net::UnixDatagram;
        use tempfile::tempdir;

        let dir = tempdir().expect("tempdir");
        let sock_path = dir.path().join("notify.sock");

        // Bind the receiving end.
        let receiver = UnixDatagram::bind(&sock_path).expect("bind unix datagram socket");
        receiver.set_nonblocking(true).expect("set_nonblocking");

        // Point sd-notify at our socket. (set_var is safe in edition 2021.)
        std::env::set_var("NOTIFY_SOCKET", sock_path.as_os_str());

        notify_ready();

        // Give the kernel a moment to deliver the datagram.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let mut buf = [0u8; 256];
        let n = receiver.recv(&mut buf).expect("recv from notify socket");
        let payload = std::str::from_utf8(&buf[..n]).expect("utf8");

        // Restore env for other tests.
        std::env::remove_var("NOTIFY_SOCKET");

        assert!(
            payload.contains("READY=1"),
            "expected READY=1 in payload, got: {payload:?}"
        );
    }
}
