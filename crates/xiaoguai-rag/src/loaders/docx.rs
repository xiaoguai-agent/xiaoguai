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

use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

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
                let name_bytes = e.name().as_ref().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"p" => {
                        // New paragraph — reset accumulator.
                        para_buf.clear();
                        current_para_style = None;
                    }
                    b"t" => {
                        in_wt = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing elements like <w:pStyle w:val="Heading1"/>.
                let name_bytes = e.name().as_ref().to_vec();
                let local = local_name(&name_bytes);
                if local == b"pStyle" {
                    for attr in e.attributes().flatten() {
                        let key_bytes = attr.key.as_ref().to_vec();
                        if local_name(&key_bytes) == b"val" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            current_para_style = Some(val);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"t" => {
                        in_wt = false;
                    }
                    b"p" => {
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
                para_buf.push_str(&e.unescape().unwrap_or_default());
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

fn local_name(qname: &[u8]) -> &[u8] {
    // Strip the namespace prefix (everything before the first ':').
    if let Some(pos) = qname.iter().position(|&b| b == b':') {
        &qname[pos + 1..]
    } else {
        qname
    }
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
            let mut entry =
                archive
                    .by_name("word/document.xml")
                    .map_err(|_| LoadError::Malformed {
                        format: "docx",
                        reason: "word/document.xml not found in archive".into(),
                    })?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
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
}
