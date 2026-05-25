//! Markdown loader — parses `CommonMark` via [`pulldown_cmark`].
//!
//! Extracts plain text from all text events, preserving paragraph breaks.
//! YAML/TOML frontmatter (delimited by `---` or `+++` fence lines) is
//! stripped from the text but its raw content is stored in
//! `metadata["frontmatter"]`. The whole document is treated as a single page.
//!
//! Heading text is collected in `metadata["headings"]` (newline-separated).

use std::collections::HashMap;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

/// Stateless Markdown loader.
#[derive(Debug, Default, Clone)]
pub struct MarkdownLoader;

impl MarkdownLoader {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Strip YAML (`---`) or TOML (`+++`) frontmatter from the start of the
/// document. Returns `(frontmatter, remaining_markdown)`.
fn strip_frontmatter(source: &str) -> (Option<String>, &str) {
    let trimmed = source.trim_start_matches('\n');
    for fence in &["---", "+++"] {
        if let Some(rest) = trimmed.strip_prefix(fence) {
            // Must be followed by a newline (the fence must be alone on its line).
            if !rest.starts_with('\n') && !rest.starts_with('\r') {
                continue;
            }
            let inner = &rest[1..]; // skip the newline after the opening fence
            if let Some(end_pos) = inner.find(&format!("\n{fence}")) {
                let fm = &inner[..end_pos];
                // Skip the closing fence line.
                let after_close = &inner[end_pos + 1 + fence.len()..];
                // The character after the closing fence should be \n or end-of-str.
                let remaining = after_close
                    .strip_prefix('\n')
                    .unwrap_or(after_close)
                    .strip_prefix('\r')
                    .unwrap_or(after_close);
                return (Some(fm.to_string()), remaining);
            }
        }
    }
    (None, source)
}

impl Loader for MarkdownLoader {
    fn load(&self, bytes: &[u8]) -> LoadResult {
        let source = std::str::from_utf8(bytes).map_err(|e| LoadError::Malformed {
            format: "markdown",
            reason: format!("not valid UTF-8: {e}"),
        })?;

        let (frontmatter, md_body) = strip_frontmatter(source);

        let opts = Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_SMART_PUNCTUATION;
        let parser = Parser::new_ext(md_body, opts);

        let mut text = String::new();
        let mut headings: Vec<String> = Vec::new();

        // State machine: we're inside a heading when this is Some(level).
        let mut heading_buf: Option<(HeadingLevel, String)> = None;
        // Paragraph separator tracking.
        let mut last_was_newline = false;

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    heading_buf = Some((level, String::new()));
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some((_, ref buf)) = heading_buf {
                        let h = buf.trim().to_string();
                        if !h.is_empty() {
                            headings.push(h.clone());
                            if !text.is_empty() && !last_was_newline {
                                text.push('\n');
                            }
                            text.push_str(&h);
                            text.push('\n');
                            last_was_newline = true;
                        }
                    }
                    heading_buf = None;
                }
                Event::Start(Tag::Paragraph | Tag::Item | Tag::BlockQuote(_)) => {
                    if !text.is_empty() && !last_was_newline {
                        text.push('\n');
                        last_was_newline = true;
                    }
                }
                Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::BlockQuote(_)) => {
                    if !text.ends_with('\n') {
                        text.push('\n');
                        last_was_newline = true;
                    }
                }
                Event::Text(ref t) | Event::Code(ref t) => {
                    if let Some((_, ref mut buf)) = heading_buf {
                        buf.push_str(t);
                    } else {
                        text.push_str(t);
                        last_was_newline = t.ends_with('\n');
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if heading_buf.is_none() {
                        text.push(' ');
                        last_was_newline = false;
                    }
                }
                _ => {}
            }
        }

        // Trim trailing whitespace.
        let text = text.trim().to_string();

        let mut metadata = HashMap::new();
        metadata.insert("format".to_string(), "markdown".to_string());
        if let Some(fm) = frontmatter {
            metadata.insert("frontmatter".to_string(), fm);
        }
        if !headings.is_empty() {
            metadata.insert("headings".to_string(), headings.join("\n"));
        }

        let page = PageMeta {
            page_number: 1,
            text: text.clone(),
        };

        Ok(LoadedDoc {
            text,
            pages: vec![page],
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/loaders")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
    }

    #[test]
    fn markdown_extracts_body_text() {
        let bytes = fixture("sample.md");
        let doc = MarkdownLoader::new().load(&bytes).expect("load ok");
        assert!(!doc.text.is_empty());
        assert!(
            doc.text.to_lowercase().contains("quick brown fox"),
            "expected phrase not found in: {:?}",
            doc.text
        );
    }

    #[test]
    fn markdown_strips_frontmatter_from_text() {
        let bytes = fixture("sample.md");
        let doc = MarkdownLoader::new().load(&bytes).expect("load ok");
        // Frontmatter keys must not appear in the main text.
        assert!(
            !doc.text.contains("author: Xiaoguai"),
            "frontmatter leaked into text: {:?}",
            doc.text
        );
    }

    #[test]
    fn markdown_stores_frontmatter_in_metadata() {
        let bytes = fixture("sample.md");
        let doc = MarkdownLoader::new().load(&bytes).expect("load ok");
        let fm = doc.metadata.get("frontmatter").cloned().unwrap_or_default();
        assert!(
            fm.contains("Sample Document"),
            "frontmatter metadata expected title, got: {fm:?}"
        );
    }

    #[test]
    fn markdown_collects_headings() {
        let bytes = fixture("sample.md");
        let doc = MarkdownLoader::new().load(&bytes).expect("load ok");
        let headings = doc.metadata.get("headings").cloned().unwrap_or_default();
        assert!(
            headings.contains("Introduction"),
            "expected 'Introduction' heading, got: {headings:?}"
        );
    }

    #[test]
    fn markdown_single_page() {
        let bytes = fixture("sample.md");
        let doc = MarkdownLoader::new().load(&bytes).expect("load ok");
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].page_number, 1);
    }

    #[test]
    fn strip_frontmatter_yaml() {
        let src = "---\ntitle: Test\nauthor: Me\n---\n\nBody text here.";
        let (fm, body) = strip_frontmatter(src);
        assert!(fm.is_some(), "frontmatter must be detected");
        assert!(fm.unwrap().contains("title: Test"));
        assert!(body.contains("Body text here."));
        assert!(!body.contains("---"));
    }

    #[test]
    fn strip_frontmatter_no_fence() {
        let src = "# Just a heading\n\nSome text.";
        let (fm, body) = strip_frontmatter(src);
        assert!(fm.is_none(), "no frontmatter expected");
        assert!(body.contains("Just a heading"));
    }
}
