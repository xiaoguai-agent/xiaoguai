//! DOCX loader — extracts text by parsing `word/document.xml` from the
//! Office Open XML ZIP container.
//!
//! Extracts all `<w:t>` text runs and preserves heading hierarchy in
//! `metadata["headings"]` (newline-separated list). The entire document is
//! treated as a single "page" (DOCX has no fixed page boundaries in XML).

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

use super::ooxml::resolve_entity;
use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

/// Cap on the DECOMPRESSED `word/document.xml` (ZIP-bomb guard). Real
/// documents run well under this; 16 MiB is far beyond any legitimate file.
const MAX_DOCUMENT_XML_BYTES: usize = 16 * 1024 * 1024;

/// Stateless DOCX loader.
#[derive(Debug, Default, Clone)]
pub struct DocxLoader;

impl DocxLoader {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Extract text and headings from `word/document.xml` bytes.
fn parse_document_xml(xml: &[u8]) -> Result<(String, Vec<String>), LoadError> {
    let mut reader = Reader::from_reader(xml);
    // Keep whitespace intact — `<w:t xml:space="preserve">` relies on it.
    reader.config_mut().trim_text_start = false;
    reader.config_mut().trim_text_end = false;

    let mut full_text = String::new();
    let mut headings: Vec<String> = Vec::new();

    // Track current paragraph's style to detect headings.
    let mut current_para_style: Option<String> = None;
    // Buffer for the current paragraph's text runs.
    let mut para_buf = String::new();
    // We are inside <w:t> when this is true.
    let mut in_wt = false;
    // Depth tracker for nested elements that share the same local name.
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.name().local_name().as_ref() {
                    "p" => {
                        // New paragraph — reset accumulator.
                        para_buf.clear();
                        current_para_style = None;
                    }
                    "t" => {
                        in_wt = true;
                    }
                    _ => {}
                }
            }
            // Self-closing elements like <w:pStyle w:val="Heading1"/>.
            Ok(Event::Empty(ref e)) if e.name().local_name().as_ref() == "pStyle" => {
                for attr in e.attributes().flatten() {
                    if attr.key.local_name().as_ref() == "val" {
                        // Style names carry no entities; keep the raw value
                        // exactly as the previous `from_utf8_lossy` did.
                        current_para_style = Some(attr.value.into_owned());
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().local_name().as_ref() {
                    "t" => {
                        in_wt = false;
                    }
                    "p" => {
                        // End of paragraph — commit to output.
                        let trimmed = para_buf.trim().to_string();
                        if !trimmed.is_empty() {
                            // Check if this is a heading style.
                            if let Some(ref style) = current_para_style {
                                let lower = style.to_lowercase();
                                if lower.starts_with("heading") || lower.starts_with("title") {
                                    headings.push(trimmed.clone());
                                }
                            }
                            if !full_text.is_empty() {
                                full_text.push('\n');
                            }
                            full_text.push_str(&trimmed);
                        }
                        para_buf.clear();
                        current_para_style = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if in_wt => {
                // quick-xml 0.42 folded charset decoding into the reader, so
                // BytesText derefs straight to &str and decode() is gone. The
                // text no longer carries entities either — they arrive as their
                // own GeneralRef event, handled below.
                para_buf.push_str(e);
            }
            Ok(Event::GeneralRef(ref e)) if in_wt => {
                para_buf.push_str(&resolve_entity(e));
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(LoadError::Malformed {
                    format: "docx",
                    reason: format!("XML parse error: {e}"),
                });
            }
            _ => {}
        }
        buf.clear();
    }

    Ok((full_text, headings))
}

impl Loader for DocxLoader {
    fn load(&self, bytes: &[u8]) -> LoadResult {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| LoadError::Malformed {
            format: "docx",
            reason: format!("not a valid ZIP/DOCX container: {e}"),
        })?;

        // Find word/document.xml (case-sensitive per OOXML spec).
        let xml_bytes = {
            let entry = archive
                .by_name("word/document.xml")
                .map_err(|_| LoadError::Malformed {
                    format: "docx",
                    reason: "word/document.xml not found in archive".into(),
                })?;
            // ZIP-bomb guard: cap the DECOMPRESSED document XML (see the
            // pptx loader's MAX_SLIDE_XML_BYTES for rationale).
            let mut buf = Vec::new();
            let mut limited = entry.take(MAX_DOCUMENT_XML_BYTES as u64 + 1);
            limited.read_to_end(&mut buf)?;
            if buf.len() > MAX_DOCUMENT_XML_BYTES {
                return Err(LoadError::Malformed {
                    format: "docx",
                    reason: format!(
                        "document.xml decompresses past the {MAX_DOCUMENT_XML_BYTES}-byte cap"
                    ),
                });
            }
            buf
        };

        let (text, headings) = parse_document_xml(&xml_bytes)?;

        let mut metadata = HashMap::new();
        metadata.insert("format".to_string(), "docx".to_string());
        if !headings.is_empty() {
            metadata.insert("headings".to_string(), headings.join("\n"));
        }

        // DOCX has no fixed page boundaries in XML — expose as a single page.
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
    fn docx_extracts_paragraph_text() {
        let bytes = fixture("sample.docx");
        let doc = DocxLoader::new().load(&bytes).expect("load ok");
        assert!(!doc.text.is_empty(), "text must not be empty");
        assert!(
            doc.text.to_lowercase().contains("quick brown fox"),
            "expected phrase not found in: {:?}",
            doc.text
        );
    }

    #[test]
    fn docx_extracts_headings_into_metadata() {
        let bytes = fixture("sample.docx");
        let doc = DocxLoader::new().load(&bytes).expect("load ok");
        let headings = doc.metadata.get("headings").cloned().unwrap_or_default();
        // The fixture has "DOCX Sample Heading One" and section headings.
        assert!(
            headings.to_lowercase().contains("heading")
                || headings.to_lowercase().contains("section"),
            "expected heading content in: {headings:?}"
        );
    }

    #[test]
    fn docx_produces_single_page() {
        let bytes = fixture("sample.docx");
        let doc = DocxLoader::new().load(&bytes).expect("load ok");
        // DOCX XML has no page boundaries — always one logical page.
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].page_number, 1);
    }

    #[test]
    fn docx_metadata_includes_format_key() {
        let bytes = fixture("sample.docx");
        let doc = DocxLoader::new().load(&bytes).expect("load ok");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("docx"));
    }

    #[test]
    fn malformed_bytes_return_error() {
        let err = DocxLoader::new()
            .load(b"not a zip file at all")
            .expect_err("must error on junk");
        assert!(matches!(err, LoadError::Malformed { .. }));
    }

    #[test]
    fn text_is_unescaped_and_namespace_prefix_is_stripped() {
        // Pins both halves of the 0.42 port: QName moved to &str so `w:`-prefixed
        // names must still match, and entities now arrive as separate GeneralRef
        // events that a Text-only loader would drop silently.
        let xml = b"<w:p><w:r><w:t>R&amp;D &lt;core&gt;</w:t></w:r></w:p>";
        let (text, _) = parse_document_xml(xml).expect("parse ok");
        assert_eq!(text, "R&D <core>");
    }

    #[test]
    fn heading_style_attribute_is_read_from_prefixed_attr() {
        // `<w:pStyle w:val="Heading1"/>` — the attribute key is prefixed too,
        // and its value now arrives as Cow<str> rather than raw bytes.
        let xml = br#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title Here</w:t></w:r></w:p>"#;
        let (_, headings) = parse_document_xml(xml).expect("parse ok");
        assert_eq!(headings, vec!["Title Here".to_string()]);
    }
}
