//! Owner identity memory (DEC-036, P1) — a persistent `USER.md` profile the
//! owner authors once and that is injected as a leading `System` message into
//! every chat session, so the agent always knows who it is working for and the
//! owner's standing preferences.
//!
//! This is distinct from episodic recall (the similarity-searched memory store):
//! it is always-on, owner-authored plaintext, and small. It is **not** secret
//! storage — secrets stay in `.env`.
//!
//! The file lives next to the `SQLite` store (`~/.xiaoguai/USER.md`, or
//! `$XIAOGUAI_HOME/xiaoguai/USER.md`); `XIAOGUAI_IDENTITY_PATH` overrides the
//! location. Loaded per-request so edits take effect without a restart.

use std::path::{Path, PathBuf};

/// Hard cap on injected identity text so a huge `USER.md` can't blow the context
/// budget (the surrounding history compaction handles the rest).
const MAX_IDENTITY_BYTES: usize = 8_192;

/// Resolve the `USER.md` path: `XIAOGUAI_IDENTITY_PATH` wins; otherwise it sits
/// in the per-user xiaoguai home (mirroring where `data.db` lives).
#[must_use]
pub fn resolve_identity_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("XIAOGUAI_IDENTITY_PATH") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    xiaoguai_home().map(|h| h.join("USER.md"))
}

/// The per-user xiaoguai home directory: `$XIAOGUAI_HOME/xiaoguai` when set,
/// else `~/.xiaoguai`.
fn xiaoguai_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("XIAOGUAI_HOME") {
        if !h.trim().is_empty() {
            return Some(PathBuf::from(h).join("xiaoguai"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .map(|h| PathBuf::from(h).join(".xiaoguai"))
}

/// Load + normalise the identity text from `path`. Returns `None` when the file
/// is absent, unreadable, or blank; trims and caps the content otherwise. Pure
/// (path in), so it is unit-testable without touching the environment.
#[must_use]
pub fn load_identity_from(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Cap on a char boundary so we never split a UTF-8 sequence.
    let capped = if trimmed.len() > MAX_IDENTITY_BYTES {
        let mut end = MAX_IDENTITY_BYTES;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };
    Some(capped.to_string())
}

/// Load the owner identity text from the resolved path, if any.
#[must_use]
pub fn load_identity() -> Option<String> {
    load_identity_from(&resolve_identity_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none() {
        assert_eq!(load_identity_from(Path::new("/no/such/USER.md")), None);
    }

    #[test]
    fn blank_file_is_none() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "   \n\t ").unwrap();
        assert_eq!(load_identity_from(f.path()), None);
    }

    #[test]
    fn nonempty_file_is_trimmed() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "\n  I am the owner. Prefer terse answers.\n\n").unwrap();
        assert_eq!(
            load_identity_from(f.path()).as_deref(),
            Some("I am the owner. Prefer terse answers.")
        );
    }

    #[test]
    fn oversized_file_is_capped_on_a_char_boundary() {
        let f = tempfile::NamedTempFile::new().unwrap();
        // Multi-byte chars near the cap must not panic or split.
        let body = "é".repeat(MAX_IDENTITY_BYTES); // 2 bytes each → well over the cap
        std::fs::write(f.path(), &body).unwrap();
        let loaded = load_identity_from(f.path()).unwrap();
        assert!(loaded.len() <= MAX_IDENTITY_BYTES);
        assert!(loaded.chars().all(|c| c == 'é')); // intact chars only
    }
}
