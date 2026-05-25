//! HTML loader — strips tags and extracts visible text via [`scraper`].
//!
//! Elements whose content is never visible to readers (`<script>`, `<style>`,
//! `<head>`, `<noscript>`) are excluded. Everything else is joined with
//! whitespace, collapsed, and trimmed.
//!
//! The `<title>` is stored in `metadata["title"]`.
//! The whole document is treated as a single "page".

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};

use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

/// Stateless HTML loader.
#[derive(Debug, Default, Clone)]
pub struct HtmlLoader;

impl HtmlLoader {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Tags whose text content is invisible to readers. We skip their subtrees.
const INVISIBLE_TAGS: &[&str] = &["script", "style", "head", "noscript", "template", "svg"];

fn is_invisible(el: &ElementRef<'_>) -> bool {
    let tag = el.value().name();
    INVISIBLE_TAGS.contains(&tag)
}

/// Recursively collect visible text from an element tree.
fn collect_text(el: ElementRef<'_>, out: &mut String) {
    if is_invisible(&el) {
        return;
    }
    for child in el.children() {
        if let Some(text_node) = child.value().as_text() {
            let chunk = text_node.trim();
            if !chunk.is_empty() {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(chunk);
            }
        } else if let Some(child_el) = ElementRef::wrap(child) {
            // Add a space before block-level elements to avoid word run-on.
            let tag = child_el.value().name();
            let is_block = matches!(
                tag,
                "p" | "div"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "li"
                    | "tr"
                    | "td"
                    | "th"
                    | "section"
                    | "article"
                    | "header"
                    | "footer"
                    | "main"
                    | "blockquote"
                    | "pre"
            );
            if is_block && !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            collect_text(child_el, out);
            if is_block && !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

impl Loader for HtmlLoader {
    fn load(&self, bytes: &[u8]) -> LoadResult {
        let html_str = std::str::from_utf8(bytes).map_err(|e| LoadError::Malformed {
            format: "html",
            reason: format!("not valid UTF-8: {e}"),
        })?;

        let document = Html::parse_document(html_str);

        // Extract <title> for metadata.
        let title = Selector::parse("title")
            .ok()
            .and_then(|sel| document.select(&sel).next())
            .map(|el| el.text().collect::<String>().trim().to_string());

        // Collect visible text from <body>, falling back to the root.
        let mut text = String::new();
        if let Ok(body_sel) = Selector::parse("body") {
            if let Some(body) = document.select(&body_sel).next() {
                collect_text(body, &mut text);
            }
        }
        if text.trim().is_empty() {
            // No <body> — collect from the root <html> element directly.
            collect_text(document.root_element(), &mut text);
        }

        // Collapse multiple whitespace runs but preserve newlines.
        let text = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        let mut metadata = HashMap::new();
        metadata.insert("format".to_string(), "html".to_string());
        if let Some(t) = title {
            if !t.is_empty() {
                metadata.insert("title".to_string(), t);
            }
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
    fn html_extracts_visible_text() {
        let bytes = fixture("sample.html");
        let doc = HtmlLoader::new().load(&bytes).expect("load ok");
        assert!(!doc.text.is_empty());
        assert!(
            doc.text.to_lowercase().contains("quick brown fox"),
            "expected phrase not found in: {:?}",
            doc.text
        );
    }

    #[test]
    fn html_strips_script_and_style() {
        let bytes = fixture("sample.html");
        let doc = HtmlLoader::new().load(&bytes).expect("load ok");
        // The fixture's <script> and <style> content must not appear.
        assert!(
            !doc.text.contains("should not appear"),
            "script content leaked into text: {:?}",
            doc.text
        );
        assert!(
            !doc.text.contains(".hidden"),
            "style content leaked into text: {:?}",
            doc.text
        );
    }

    #[test]
    fn html_extracts_title_into_metadata() {
        let bytes = fixture("sample.html");
        let doc = HtmlLoader::new().load(&bytes).expect("load ok");
        let title = doc.metadata.get("title").cloned().unwrap_or_default();
        assert!(
            title.contains("Sample HTML"),
            "expected title, got: {title:?}"
        );
    }

    #[test]
    fn html_produces_single_page() {
        let bytes = fixture("sample.html");
        let doc = HtmlLoader::new().load(&bytes).expect("load ok");
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].page_number, 1);
    }

    #[test]
    fn empty_html_returns_empty_text() {
        let doc = HtmlLoader::new()
            .load(b"<html><head></head><body></body></html>")
            .expect("load ok");
        assert!(doc.text.trim().is_empty());
    }
}
