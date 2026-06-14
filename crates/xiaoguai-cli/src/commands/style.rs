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

/// Colour a unified-diff block for readability: added (`+`) lines get a green
/// background, removed (`-`) lines a red background, `@@` hunk headers cyan.
/// File headers (`+++`/`---`) and context lines stay plain. Respects
/// [`enabled`]; non-diff text passes through unchanged.
#[must_use]
pub fn diff(text: &str) -> String {
    diff_with(enabled(), text)
}

/// Pure core of [`diff`] — `on` gates the SGR codes so it's unit-testable
/// without a TTY.
fn diff_with(on: bool, text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with("@@") {
                paint_if(on, "36", line)
            } else if line.starts_with("+++") || line.starts_with("---") {
                line.to_string()
            } else if line.starts_with('+') {
                paint_if(on, "42", line) // green background = added / new
            } else if line.starts_with('-') {
                paint_if(on, "41", line) // red background = removed / old
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    fn diff_backgrounds_add_and_remove_lines() {
        let d = diff_with(
            true,
            "@@ -1,2 +1,2 @@\n--- a/x\n+++ b/x\n-old line\n+new line\n unchanged",
        );
        assert!(d.contains("\x1b[42m+new line\x1b[0m")); // added → green bg
        assert!(d.contains("\x1b[41m-old line\x1b[0m")); // removed → red bg
        assert!(d.contains("\x1b[36m@@ -1,2 +1,2 @@\x1b[0m")); // hunk header → cyan
        assert!(d.contains("--- a/x")); // file header stays plain
        assert!(d.contains("+++ b/x"));
        assert!(d.contains(" unchanged")); // context stays plain
                                           // disabled → no codes at all
        assert_eq!(diff_with(false, "-old\n+new"), "-old\n+new");
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
