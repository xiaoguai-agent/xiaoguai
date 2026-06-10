//! First-run guidance banners (T8.1 / T8.4,
//! `docs/plans/2026-06-10-install-polish.md`).
//!
//! Pure string builders so they are unit-testable; `run_serve` decides when
//! to print them. The tracing logs stay untouched — these are *operator*
//! lines on stdout/stderr, not telemetry.

use std::net::SocketAddr;

/// Post-bind success banner, printed to **stdout** once the listener is up.
///
/// One ✓ line plus one next-step line (chat UI URL / first-message command).
/// No auto-open browser — server installs are headless (plan §1 T8.1).
#[must_use]
pub fn serve_banner(local: &SocketAddr) -> String {
    let url = display_url(local);
    format!(
        "✓ xiaoguai running at {url}\n  Open the chat UI at {url}/ — or send a first message: xiaoguai repl"
    )
}

/// Actionable message for a failed bind on an already-occupied port,
/// printed to **stderr**. Three remedies: kill, `--port`, lsof hint.
#[must_use]
pub fn addr_in_use_message(host: &str, port: u16) -> String {
    format!(
        "✗ cannot start: port {port} on {host} is already in use.\n\
         \n\
         Three ways out:\n\
         \x20 1. another xiaoguai may already be running — check: curl http://localhost:{port}/healthz\n\
         \x20 2. serve on a different port:               XIAOGUAI_SERVER__PORT=7601 xiaoguai serve\n\
         \x20 3. find and stop whatever holds the port:   lsof -i :{port}  (then kill <pid>)"
    )
}

/// Returns true when `err`'s chain contains an `io::Error` with kind
/// [`std::io::ErrorKind::AddrInUse`]. Portable — no raw OS error codes.
#[must_use]
pub fn is_addr_in_use(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    })
}

/// Multi-line **stderr** banner for an empty `llm_providers` table.
///
/// The mock fallback itself stays in place (tests/e2e rely on it); this
/// banner just makes the posture loud and gives the two real paths
/// (local Ollama / `xiaoguai init`). Suppressed by the caller when
/// `XIAOGUAI_LLM__MOCK=true` — explicit opt-in stays quiet (plan §1 T8.4).
#[must_use]
pub fn empty_providers_banner() -> String {
    "\n\
     ! No LLM providers are configured — replies will come from the built-in\n\
     ! deterministic mock until you pick one of these:\n\
     !\n\
     !   local (no API key):  install Ollama, then:  ollama pull qwen2.5-coder\n\
     !   cloud provider:      xiaoguai init   (interactive key setup)\n\
     !\n\
     ! Re-check anytime with:  xiaoguai doctor\n"
        .to_string()
}

/// `http://host:port` for a bound address, mapping the unroutable
/// wildcard hosts (`0.0.0.0` / `::`) to `localhost` so the printed URL is
/// actually clickable.
fn display_url(local: &SocketAddr) -> String {
    if local.ip().is_unspecified() {
        format!("http://localhost:{}", local.port())
    } else {
        format!("http://{local}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn serve_banner_has_check_url_and_next_step() {
        let local: SocketAddr = "127.0.0.1:7600".parse().unwrap();
        let b = serve_banner(&local);
        assert!(b.starts_with("✓ xiaoguai running at http://127.0.0.1:7600"));
        assert!(b.contains("http://127.0.0.1:7600/"));
        assert!(b.contains("xiaoguai repl"));
        assert_eq!(b.lines().count(), 2);
    }

    #[test]
    fn serve_banner_maps_wildcard_to_localhost() {
        let local: SocketAddr = "0.0.0.0:7600".parse().unwrap();
        assert!(serve_banner(&local).contains("http://localhost:7600"));
        let v6: SocketAddr = "[::]:7600".parse().unwrap();
        assert!(serve_banner(&v6).contains("http://localhost:7600"));
    }

    #[test]
    fn addr_in_use_message_lists_three_remedies() {
        let m = addr_in_use_message("127.0.0.1", 7600);
        assert!(m.contains("port 7600"));
        assert!(m.contains("healthz")); // remedy 1: already running?
        assert!(m.contains("XIAOGUAI_SERVER__PORT")); // remedy 2: change port
        assert!(m.contains("lsof -i :7600")); // remedy 3: find holder
    }

    #[test]
    fn is_addr_in_use_detects_kind_through_context_chain() {
        let io_err = io::Error::new(io::ErrorKind::AddrInUse, "in use");
        let err = anyhow::Error::from(io_err).context("bind 127.0.0.1:7600");
        assert!(is_addr_in_use(&err));
    }

    #[test]
    fn is_addr_in_use_rejects_other_errors() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        let err = anyhow::Error::from(io_err).context("bind");
        assert!(!is_addr_in_use(&err));
        assert!(!is_addr_in_use(&anyhow::anyhow!("not io at all")));
    }

    #[test]
    fn empty_providers_banner_names_both_paths_and_doctor() {
        let b = empty_providers_banner();
        assert!(b.contains("ollama pull qwen2.5-coder"));
        assert!(b.contains("xiaoguai init"));
        assert!(b.contains("xiaoguai doctor"));
        assert!(b.lines().count() >= 5, "must be a multi-line banner");
    }
}
