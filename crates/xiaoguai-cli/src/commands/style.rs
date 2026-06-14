//! Tiny ANSI styling for the interactive CLI surface (`xiaoguai cli`).
//!
//! Colours are emitted only to a real terminal and suppressed under `NO_COLOR`
//! (<https://no-color.org>) so piped / CI output stays plain. The decision is
//! made once from stderr, where the prompt + notices are written.

use std::io::IsTerminal as _;
use std::sync::OnceLock;

/// Whether ANSI colour should be emitted: `NO_COLOR` unset **and** stderr is a
/// TTY. Cached on first use. Set `NO_COLOR=1` (or pipe stderr) to disable.
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal())
}

/// Wrap `s` in the SGR `code` when `on` (and `s` is non-empty). Pure — the
/// public helpers feed it [`enabled`]; tests feed it explicitly.
#[must_use]
pub fn paint_if(on: bool, code: &str, s: &str) -> String {
    if on && !s.is_empty() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn paint(code: &str, s: &str) -> String {
    paint_if(enabled(), code, s)
}

/// Bold cyan — the prompt marker.
#[must_use]
pub fn prompt(s: &str) -> String {
    paint("1;36", s)
}
/// Green — success / confirmations.
#[must_use]
pub fn ok(s: &str) -> String {
    paint("32", s)
}
/// Red — errors.
#[must_use]
pub fn err(s: &str) -> String {
    paint("31", s)
}
/// Yellow — warnings / notices.
#[must_use]
pub fn warn(s: &str) -> String {
    paint("33", s)
}
/// Dim — secondary / tool chatter.
#[must_use]
pub fn dim(s: &str) -> String {
    paint("2", s)
}
/// Magenta — the logo / brand accent.
#[must_use]
pub fn accent(s: &str) -> String {
    paint("35", s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_if_wraps_only_when_on() {
        assert_eq!(paint_if(true, "32", "ok"), "\x1b[32mok\x1b[0m");
        assert_eq!(paint_if(false, "32", "ok"), "ok"); // disabled → plain
        assert_eq!(paint_if(true, "32", ""), ""); // empty stays empty (no codes)
    }

    #[test]
    fn helpers_are_plain_when_disabled() {
        // Can't force a TTY in tests, but the pure core covers both branches;
        // here we just confirm the public helpers don't panic and round-trip
        // content (colour may or may not be on depending on the test TTY).
        assert!(ok("done").contains("done"));
        assert!(err("boom").contains("boom"));
        assert!(prompt(">").contains('>'));
    }
}
