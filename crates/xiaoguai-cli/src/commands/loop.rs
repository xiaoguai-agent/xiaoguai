//! `xiaoguai loop {create,list,show,cancel}` output formatting (pure,
//! unit-testable). The REST wiring is in `commands::remote`; `main.rs`
//! drives it. `/loop` runs inside `xiaoguai serve`, so everything here is
//! a thin presentation layer over [`remote::LoopResponse`].

use crate::commands::remote::LoopResponse;

/// Characters of the loop id shown in the `list` table — long enough to be
/// unique in practice; `show`/`cancel` accept any unique prefix.
const SHORT_ID_LEN: usize = 12;

/// Render the `loop list` table.
#[must_use]
pub fn format_table(rows: &[LoopResponse]) -> String {
    use std::fmt::Write as _;
    if rows.is_empty() {
        return "no loops — create one with `xiaoguai loop create --session <id> --prompt <text>`\n"
            .to_string();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<14} {:<12} {:<10} {:<8} {:<17} SESSION",
        "ID", "STATUS", "TICKS", "EVERY", "NEXT TICK"
    );
    for r in rows {
        let ticks = format!("{}/{}", r.ticks_run, r.max_ticks);
        let _ = writeln!(
            out,
            "{:<14} {:<12} {:<10} {:<8} {:<17} {}",
            short_id(&r.id),
            r.status,
            ticks,
            format!("{}s", r.interval_secs),
            truncate(&r.next_tick_at, 17),
            r.session_id,
        );
    }
    out
}

/// Render the `loop show` / `loop create` detail view.
#[must_use]
pub fn format_detail(r: &LoopResponse) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "id:            {}", r.id);
    let _ = writeln!(out, "session:       {}", r.session_id);
    let _ = writeln!(out, "status:        {}", r.status);
    let _ = writeln!(out, "prompt:        {}", r.prompt);
    let _ = writeln!(out, "interval:      {}s", r.interval_secs);
    let _ = writeln!(out, "ticks:         {}/{}", r.ticks_run, r.max_ticks);
    let _ = writeln!(out, "ttl:           {}s", r.ttl_secs);
    let _ = writeln!(out, "next tick:     {}", r.next_tick_at);
    if r.consecutive_failures > 0 {
        let _ = writeln!(out, "failures:      {} consecutive", r.consecutive_failures);
    }
    if let Some(err) = &r.last_error {
        let _ = writeln!(out, "last error:    {err}");
    }
    out
}

fn short_id(id: &str) -> &str {
    let cut = id
        .char_indices()
        .nth(SHORT_ID_LEN)
        .map_or(id.len(), |(i, _)| i);
    &id[..cut]
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoopResponse {
        LoopResponse {
            id: "loop_0123456789abcdef".to_string(),
            session_id: "sess_a".to_string(),
            prompt: "check the CI run".to_string(),
            interval_secs: 300,
            max_ticks: 50,
            ttl_secs: 86_400,
            status: "active".to_string(),
            next_tick_at: "2026-06-08T10:30:00Z".to_string(),
            ticks_run: 3,
            consecutive_failures: 0,
            last_error: None,
        }
    }

    #[test]
    fn empty_table_teaches_create() {
        let out = format_table(&[]);
        assert!(out.contains("no loops"));
        assert!(out.contains("loop create"));
    }

    #[test]
    fn table_shows_short_id_and_tick_ratio() {
        let out = format_table(&[sample()]);
        assert!(out.contains("loop_0123456")); // 12-char short id
        assert!(!out.contains("loop_0123456789abcdef")); // full id truncated
        assert!(out.contains("3/50"));
        assert!(out.contains("active"));
    }

    #[test]
    fn detail_shows_full_id_and_prompt() {
        let out = format_detail(&sample());
        assert!(out.contains("loop_0123456789abcdef"));
        assert!(out.contains("check the CI run"));
        assert!(out.contains("3/50"));
        // No failures line when healthy.
        assert!(!out.contains("failures:"));
        assert!(!out.contains("last error:"));
    }

    #[test]
    fn detail_surfaces_failure_state() {
        let mut r = sample();
        r.status = "failed".to_string();
        r.consecutive_failures = 5;
        r.last_error = Some("provider timeout".to_string());
        let out = format_detail(&r);
        assert!(out.contains("5 consecutive"));
        assert!(out.contains("provider timeout"));
    }

    #[test]
    fn short_id_handles_short_input() {
        assert_eq!(short_id("loop_1"), "loop_1");
    }
}
