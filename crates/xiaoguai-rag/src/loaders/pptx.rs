//! PPTX loader — extracts text from each slide's XML in the Office Open XML
//! ZIP container.
//!
//! Walks `ppt/slides/slide*.xml` in slide-number order (sorted lexically).
//! Collects all `<a:t>` text runs (`DrawingML` namespace). One [`PageMeta`]
//! per slide, numbered 1-based in the order the files appear after sorting.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

/// Stateless PPTX loader.
#[derive(Debug, Default, Clone)]
pub struct PptxLoader;

impl PptxLoader {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Extract all `<a:t>` text runs from a single slide XML blob.
fn extract_slide_text(xml: &[u8]) -> Result<String, LoadError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text_start = false;
    reader.config_mut().trim_text_end = false;

    let mut slide_text = String::new();
    let mut in_at = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                if local_name(&name_bytes) == b"t" {
                    in_at = true;
                }
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                if local_name(&name_bytes) == b"t" {
                    in_at = false;
                }
            }
            Ok(Event::Text(ref e)) if in_at => {
                // quick-xml 0.40: decode() (charset) then escape::unescape()
                // replace the removed BytesText::unescape().
                let raw = e.decode().unwrap_or_default();
                let chunk = match quick_xml::escape::unescape(&raw) {
                    Ok(t) => t.into_owned(),
                    Err(_) => raw.into_owned(),
                };
                if !chunk.trim().is_empty() {
                    if !slide_text.is_empty() {
                        slide_text.push(' ');
                    }
                    slide_text.push_str(chunk.trim());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(LoadError::Malformed {
                    format: "pptx",
                    reason: format!("XML parse error in slide: {e}"),
                });
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(slide_text.trim().to_string())
}

fn local_name(qname: &[u8]) -> &[u8] {
    if let Some(pos) = qname.iter().position(|&b| b == b':') {
        &qname[pos + 1..]
    } else {
        qname
    }
}

impl Loader for PptxLoader {
    fn load(&self, bytes: &[u8]) -> LoadResult {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| LoadError::Malformed {
            format: "pptx",
            reason: format!("not a valid ZIP/PPTX container: {e}"),
        })?;

        // Collect all slide file names, then sort to get slide order.
        let mut slide_names: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let f = archive.by_index(i).ok()?;
                let name = f.name().to_string();
                if name.starts_with("ppt/slides/slide")
                    && name.to_ascii_lowercase().ends_with(".xml")
                {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        // Natural sort by the slide number embedded in the filename
        // (e.g. slide1.xml < slide2.xml < slide10.xml).
        slide_names.sort_by_key(|n| {
            // Extract the numeric suffix from "ppt/slides/slideN.xml".
            let stem = n
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml");
            stem.parse::<u32>().unwrap_or(0)
        });

        let mut pages: Vec<PageMeta> = Vec::new();

        for (idx, name) in slide_names.iter().enumerate() {
            let xml_bytes = {
                let mut entry = archive.by_name(name).map_err(|_| LoadError::Malformed {
                    format: "pptx",
                    reason: format!("slide entry '{name}' disappeared"),
                })?;
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                buf
            };

            let slide_text = extract_slide_text(&xml_bytes)?;
            pages.push(PageMeta {
                page_number: u32::try_from(idx + 1).unwrap_or(u32::MAX),
                text: slide_text,
            });
        }

        if pages.is_empty() {
            return Err(LoadError::Malformed {
                format: "pptx",
                reason: "no slide XML entries found in archive".into(),
            });
        }

        let full_text = pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut metadata = HashMap::new();
        metadata.insert("format".to_string(), "pptx".to_string());
        metadata.insert("slide_count".to_string(), pages.len().to_string());

        Ok(LoadedDoc {
            text: full_text,
            pages,
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
    fn pptx_extracts_slide_text() {
        let bytes = fixture("sample.pptx");
        let doc = PptxLoader::new().load(&bytes).expect("load ok");
        assert!(!doc.text.is_empty(), "text must not be empty");
        assert!(
            doc.text.to_lowercase().contains("quick brown fox"),
            "expected phrase not found in: {:?}",
            doc.text
        );
    }

    #[test]
    fn pptx_has_one_page_per_slide() {
        let bytes = fixture("sample.pptx");
        let doc = PptxLoader::new().load(&bytes).expect("load ok");
        // Fixture has 3 slides.
        assert_eq!(doc.pages.len(), 3, "expected 3 slides");
        assert_eq!(doc.pages[0].page_number, 1);
        assert_eq!(doc.pages[1].page_number, 2);
        assert_eq!(doc.pages[2].page_number, 3);
    }

    #[test]
    fn pptx_slide2_contains_lorem_ipsum() {
        let bytes = fixture("sample.pptx");
        let doc = PptxLoader::new().load(&bytes).expect("load ok");
        assert!(
            doc.pages[1].text.to_lowercase().contains("lorem ipsum"),
            "slide 2 expected lorem ipsum: {:?}",
            doc.pages[1].text
        );
    }

    #[test]
    fn pptx_metadata_has_slide_count() {
        let bytes = fixture("sample.pptx");
        let doc = PptxLoader::new().load(&bytes).expect("load ok");
        let count: usize = doc.metadata["slide_count"].parse().unwrap();
        assert_eq!(count, doc.pages.len());
    }

    #[test]
    fn malformed_bytes_return_error() {
        let err = PptxLoader::new()
            .load(b"not a pptx")
            .expect_err("must error on junk");
        assert!(matches!(err, LoadError::Malformed { .. }));
    }
}
