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

use super::ooxml::resolve_entity;
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

/// Per-slide extracted-text cap (bytes). A maliciously crafted slide with
/// megabytes of `<a:t>` runs would otherwise allocate unbounded memory during
/// ingest. Generous for real decks — the longest legitimate slides are a few KB.
const MAX_SLIDE_TEXT_BYTES: usize = 256 * 1024;

/// Cap on one slide's DECOMPRESSED XML (ZIP-bomb guard). Real slide XML runs
/// a few KB; 8 MiB is far beyond any legitimate deck.
const MAX_SLIDE_XML_BYTES: usize = 8 * 1024 * 1024;

/// Append one finished `<a:t>` run to the slide's text, space-joined.
///
/// Returns `true` when the per-slide cap was reached and collection for this
/// slide must stop.
fn push_run(slide_text: &mut String, run: &str) -> bool {
    let trimmed = run.trim();
    if trimmed.is_empty() {
        return false;
    }
    if !slide_text.is_empty() {
        slide_text.push(' ');
    }
    let remaining = MAX_SLIDE_TEXT_BYTES.saturating_sub(slide_text.len());
    if trimmed.len() <= remaining {
        slide_text.push_str(trimmed);
        return false;
    }
    // Cap reached (guards against one giant run too). Append a
    // char-boundary-safe prefix and stop collecting this slide.
    let mut end = remaining;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    slide_text.push_str(&trimmed[..end]);
    true
}

/// Extract all `<a:t>` text runs from a single slide XML blob.
fn extract_slide_text(xml: &[u8]) -> Result<String, LoadError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text_start = false;
    reader.config_mut().trim_text_end = false;

    let mut slide_text = String::new();
    let mut in_at = false;
    // One `<a:t>` run is buffered whole before it is appended. quick-xml 0.42
    // splits a run at every entity reference, so appending per event would turn
    // `R&amp;D` into `R & D` — the space-joining below is per RUN, not per event.
    let mut run_buf = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().local_name().as_ref() == "t" => {
                in_at = true;
                run_buf.clear();
            }
            Ok(Event::Text(ref e)) if in_at => {
                run_buf.push_str(e);
            }
            Ok(Event::GeneralRef(ref e)) if in_at => {
                run_buf.push_str(&resolve_entity(e));
            }
            Ok(Event::End(ref e)) if e.name().local_name().as_ref() == "t" => {
                in_at = false;
                if push_run(&mut slide_text, &run_buf) {
                    break;
                }
                run_buf.clear();
            }
            Ok(Event::Eof) => {
                // Buffering per run means an unterminated trailing `<a:t>` would
                // otherwise be dropped; the pre-0.42 loop wrote each text event
                // through immediately and never lost it.
                push_run(&mut slide_text, &run_buf);
                break;
            }
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
                let entry = archive.by_name(name).map_err(|_| LoadError::Malformed {
                    format: "pptx",
                    reason: format!("slide entry '{name}' disappeared"),
                })?;
                // ZIP-bomb guard: cap the DECOMPRESSED slide XML before it is
                // buffered — `MAX_SLIDE_TEXT_BYTES` only caps the extracted
                // text, after the whole XML already sat in memory.
                let mut buf = Vec::new();
                let mut limited = entry.take(MAX_SLIDE_XML_BYTES as u64 + 1);
                limited.read_to_end(&mut buf)?;
                if buf.len() > MAX_SLIDE_XML_BYTES {
                    return Err(LoadError::Malformed {
                        format: "pptx",
                        reason: format!(
                            "slide '{name}' decompresses past the {MAX_SLIDE_XML_BYTES}-byte cap"
                        ),
                    });
                }
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

    #[test]
    fn slide_text_is_capped() {
        // One oversized `<a:t>` run — must not allocate past the cap.
        let big = "x".repeat(MAX_SLIDE_TEXT_BYTES * 2);
        let xml = format!("<a:t>{big}</a:t>");
        let out = extract_slide_text(xml.as_bytes()).expect("parse ok");
        assert!(
            out.len() <= MAX_SLIDE_TEXT_BYTES,
            "extracted {} bytes, cap is {MAX_SLIDE_TEXT_BYTES}",
            out.len()
        );
    }

    #[test]
    fn slide_text_under_cap_is_intact() {
        let xml = "<a:t>hello</a:t><a:t>world</a:t>";
        let out = extract_slide_text(xml.as_bytes()).expect("parse ok");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn slide_text_unescapes_entities() {
        // quick-xml 0.42 emits every entity as its own GeneralRef event rather
        // than inside the text, so a loader that handles only Event::Text drops
        // them silently. Pins that they survive, and that a run split by an
        // entity is not space-joined back as "R & D".
        let xml = "<a:t>R&amp;D &lt;core&gt; &quot;x&quot;</a:t>";
        let out = extract_slide_text(xml.as_bytes()).expect("parse ok");
        assert_eq!(out, r#"R&D <core> "x""#);
    }

    #[test]
    fn unterminated_run_still_yields_its_text() {
        // Malformed slide: `<a:t>` never closes. Runs are buffered now, so the
        // EOF arm has to flush or the text vanishes without a parse error.
        let out = extract_slide_text(b"<a:t>trailing").expect("parse ok");
        assert_eq!(out, "trailing");
    }
}
