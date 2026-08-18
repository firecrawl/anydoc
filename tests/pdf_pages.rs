//! `anydoc::pdf_pages` — the per-page entry point a caller routes OCR with.
//!
//! The load-bearing assertion here is the LAST one. `process_pdf_mem`, which
//! `to_markdown` reads, reports this fixture's page 2 as needing OCR; the
//! per-page API does not, and the per-page API is right — that page's text
//! layer yields real text. A caller that routed on the document-level flag
//! would send a text page to an OCR engine. If a pdf-inspector upgrade ever
//! makes these two agree, this test says so rather than leaving the
//! disagreement to be rediscovered.

// The fixture path is built here rather than through `tests/common`: this
// binary uses only the root, and importing the shared module would pull in its
// unused `walk` and fail the `-D warnings` gate.
fn text_pdf() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf/text.pdf");
    std::fs::read(path).expect("the pdf fixture")
}

#[test]
fn pages_come_back_in_document_order_from_zero() {
    let pages = anydoc::pdf_pages(&text_pdf()).expect("a readable pdf");
    assert_eq!(pages.len(), 2);
    let indices: Vec<u32> = pages.iter().map(|page| page.index).collect();
    assert_eq!(indices, vec![0, 1]);
}

#[test]
fn a_page_carries_the_markdown_of_its_own_text_layer() {
    let pages = anydoc::pdf_pages(&text_pdf()).expect("a readable pdf");
    assert!(pages[0].markdown.contains("Fixture Document"), "{}", pages[0].markdown);
    assert!(pages[1].markdown.contains("Endnote body text"), "{}", pages[1].markdown);
}

#[test]
fn a_non_pdf_is_malformed_rather_than_unsupported() {
    let error = anydoc::pdf_pages(b"PK\x03\x04not a pdf at all").expect_err("not a pdf");
    assert_eq!(error.code(), "malformed", "{error}");
}

/// See the module docstring: this is the disagreement, pinned.
#[test]
fn a_thin_but_real_text_page_is_not_routed_to_ocr() {
    let pages = anydoc::pdf_pages(&text_pdf()).expect("a readable pdf");
    let flagged: Vec<u32> =
        pages.iter().filter(|page| page.needs_ocr).map(|page| page.index).collect();
    assert!(
        flagged.is_empty(),
        "every page of this fixture has an extractable text layer, but {flagged:?} was flagged"
    );
}
