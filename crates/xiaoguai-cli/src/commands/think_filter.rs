//! Pure, streaming filter for the interactive CLI's reply rendering.
//!
//! Reasoning models (the `MiniMax` M-series) emit `<think>…</think>` blocks
//! inside the streamed text — that's the model's scratch reasoning, not the
//! answer the user asked for. The raw stream has also leaked terminal control
//! bytes (ESC `^[`, `^R`, …). [`ThinkStripper`] removes both so only the final
//! answer prints.
//!
//! It lives in the library (not `main.rs`) so the logic is unit-tested by
//! `cargo test -p xiaoguai-cli --lib`; `main.rs` threads one instance per turn
//! through `render_remote_event`.

/// Opening / closing reasoning tags emitted by reasoning models (`MiniMax`
/// M-series). Everything between an open and its matching close is reasoning,
/// not the answer, and is dropped from the streamed output.
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Drop non-printable C0 control characters from `s`, keeping only `\n` and
/// `\t`. The raw model stream has leaked ESC (`\x1b` → `^[`) and other controls
/// (e.g. `\x12` → `^R`) into the terminal; this strips them so only readable
/// text prints. (Newline + tab are preserved so formatting survives.)
#[must_use]
pub fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

/// Length of the longest suffix of `buf` that is a (non-empty, proper) prefix
/// of `tag` — i.e. how many trailing chars of `buf` could be the start of a
/// `tag` whose remainder hasn't arrived yet. Used to decide how much of the
/// buffer to hold back so a tag split across deltas (`<thi` + `nk>`) is still
/// matched once it completes.
fn partial_tag_suffix_len(buf: &str, tag: &str) -> usize {
    let max = buf.len().min(tag.len() - 1);
    (1..=max)
        .rev()
        .find(|&n| {
            // Only consider suffixes that start on a char boundary so we never
            // slice mid-UTF-8.
            buf.is_char_boundary(buf.len() - n) && tag.starts_with(&buf[buf.len() - n..])
        })
        .unwrap_or(0)
}

/// Stateful, streaming filter that removes `<think>…</think>` reasoning blocks
/// from text arriving in arbitrary chunks. Tags may span multiple deltas and
/// may be split mid-token (`<thi` + `nk>`), so unflushed text that could be the
/// start of a tag is buffered in `hold` until the next delta resolves it.
///
/// One instance per turn; create a fresh one (or [`reset`](Self::reset))
/// between turns so `in_think` state never leaks across turns.
#[derive(Default)]
pub struct ThinkStripper {
    /// Currently inside a `<think>…</think>` block (output suppressed).
    in_think: bool,
    /// Carry-over buffer: text not yet safe to emit because it might be the
    /// leading fragment of a `<think>`/`</think>` tag, plus (when `in_think`)
    /// the partial reasoning text we're still scanning for a close tag.
    hold: String,
}

impl ThinkStripper {
    /// Clear all carried state, returning the filter to its initial condition.
    /// Call between turns when reusing an instance.
    pub fn reset(&mut self) {
        self.in_think = false;
        self.hold.clear();
    }

    /// Feed one streamed delta; return only the text OUTSIDE any reasoning
    /// block, with control characters stripped. A trailing fragment that might
    /// be the start of a tag is held back and reconsidered on the next call.
    #[must_use]
    pub fn push(&mut self, delta: &str) -> String {
        self.hold.push_str(delta);
        let mut out = String::new();
        loop {
            if self.in_think {
                // Scanning for a close tag — drop everything up to and
                // including it.
                if let Some(pos) = self.hold.find(THINK_CLOSE) {
                    self.hold.drain(..pos + THINK_CLOSE.len());
                    self.in_think = false;
                    continue; // re-scan the remainder for the next open tag
                }
                // No close yet. Keep only a possible partial close-tag suffix;
                // the rest is reasoning and discarded.
                let keep = partial_tag_suffix_len(&self.hold, THINK_CLOSE);
                let drop_to = self.hold.len() - keep;
                self.hold.drain(..drop_to);
                break;
            }
            // Outside a block — emit text up to the next open tag.
            if let Some(pos) = self.hold.find(THINK_OPEN) {
                out.push_str(&self.hold[..pos]);
                self.hold.drain(..pos + THINK_OPEN.len());
                self.in_think = true;
                continue; // re-scan the remainder for the close tag
            }
            // No open tag. Emit everything except a trailing fragment that
            // could be the start of one.
            let keep = partial_tag_suffix_len(&self.hold, THINK_OPEN);
            let emit_to = self.hold.len() - keep;
            out.push_str(&self.hold[..emit_to]);
            self.hold.drain(..emit_to);
            break;
        }
        strip_control_chars(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::{strip_control_chars, ThinkStripper};

    /// Feed a sequence of deltas; concatenate the visible output.
    fn run(deltas: &[&str]) -> String {
        let mut s = ThinkStripper::default();
        deltas.iter().map(|d| s.push(d)).collect()
    }

    #[test]
    fn passthrough_without_think_tags() {
        assert_eq!(run(&["hello ", "world"]), "hello world");
    }

    #[test]
    fn drops_a_whole_think_block_in_one_delta() {
        assert_eq!(
            run(&["<think>reasoning here</think>the answer"]),
            "the answer"
        );
    }

    #[test]
    fn think_block_spanning_multiple_deltas() {
        // Open in one delta, body in several, close + answer in a later one.
        let out = run(&[
            "Answer: ",
            "<think>step 1",
            " then step 2",
            " then step 3</think>",
            "42",
        ]);
        assert_eq!(out, "Answer: 42");
    }

    #[test]
    fn open_tag_split_mid_token_across_deltas() {
        // `<thi` + `nk>` — the split must NOT leak the partial tag as text.
        let out = run(&[
            "before ",
            "<thi",
            "nk>secret",
            " thoughts</thi",
            "nk>",
            "after",
        ]);
        assert_eq!(out, "before after");
        assert!(!out.contains("<thi"), "leaked partial open tag: {out:?}");
        assert!(!out.contains("secret"), "leaked reasoning: {out:?}");
    }

    #[test]
    fn close_tag_split_mid_token_across_deltas() {
        // `</thi` + `nk>` close split across deltas.
        let out = run(&["<think>reason</thi", "nk>visible"]);
        assert_eq!(out, "visible");
    }

    #[test]
    fn single_char_per_delta_drip() {
        // Worst case: one byte at a time. Tags must still be recognised.
        let chars: Vec<&str> = ["<think>hi</think>OK"]
            .iter()
            .flat_map(|s| s.split(""))
            .filter(|s| !s.is_empty())
            .collect();
        let mut s = ThinkStripper::default();
        let out: String = chars.iter().map(|c| s.push(c)).collect();
        assert_eq!(out, "OK");
    }

    #[test]
    fn lone_angle_bracket_is_not_held_forever() {
        // A `<` that turns out NOT to be a think tag must still be emitted once
        // the next char disproves the tag.
        assert_eq!(run(&["a < b", " c"]), "a < b c");
        // A `<think`-like prefix that resolves to a different word flushes once
        // the disproving char arrives.
        assert_eq!(run(&["x<thinking different"]), "x<thinking different");
    }

    #[test]
    fn multiple_think_blocks() {
        assert_eq!(run(&["a<think>r1</think>b<think>r2</think>c"]), "abc");
    }

    #[test]
    fn reset_clears_in_think_state_between_turns() {
        let mut s = ThinkStripper::default();
        // Turn 1 opens a block but never closes it (truncated turn).
        let _ = s.push("<think>unfinished reasoning");
        s.reset();
        // Turn 2 must start clean — plain text passes straight through.
        assert_eq!(s.push("fresh answer"), "fresh answer");
    }

    #[test]
    fn strips_control_chars_keeps_newline_and_tab() {
        // ESC (^[), \x12 (^R) dropped; \n and \t kept.
        assert_eq!(strip_control_chars("a\x1bb\x12c\nd\te"), "abc\nd\te");
    }

    #[test]
    fn control_chars_stripped_from_streamed_text() {
        let out = run(&["hi\x1b[2K", "\x12 there"]);
        // The bracket/digits after ESC are ordinary chars and survive; only the
        // control bytes themselves are removed.
        assert_eq!(out, "hi[2K there");
    }
}
