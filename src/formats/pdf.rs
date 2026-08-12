//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. Without a configured
//! OCR engine the pages it routes to OCR stay unavailable: image-only PDFs
//! error as unsupported, and pages flagged in an otherwise text-based document
//! degrade with a log, consistent with the crate-wide recovery policy. With an
//! engine, exactly those pages are rasterized one at a time and recognized,
//! then merged back into the untouched structured pages in source order. A
//! document classified as scanned that extracts to no text at all is read in
//! full instead, page by page: the little text such a page carries is usually
//! a watermark rather than its content, so the richer of the two wins.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::{PdfError, PdfProcessResult};

#[cfg(feature = "ocr")]
use crate::ocr::PageOcr;
#[cfg(feature = "ocr")]
use pdf_inspector::{PageMarkdown, PdfType, RenderOptions, RenderWarning};
#[cfg(feature = "ocr")]
use std::collections::BTreeMap;

#[cfg(all(test, feature = "ocr"))]
#[path = "../../tests/support/pdf_fixture.rs"]
mod pdf_fixture;

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    if !result.pages_needing_ocr.is_empty() {
        log::warn!(
            "{} of {} pages need OCR and were not extracted",
            result.pages_needing_ocr.len(),
            result.page_count
        );
    }
    warn_on_encoding_issues(&result);
    extracted_markdown(result)
}

/// Convert with local OCR for the pages that need it: the ones pdf-inspector
/// flags, or every page of a scanned document that extracts to nothing. A
/// document with no such page produces the same bytes as [`to_markdown`].
#[cfg(feature = "ocr")]
pub fn to_markdown_with_ocr(
    bytes: &[u8],
    engine: &crate::ocr::Engine,
) -> Result<String, ConvertError> {
    to_markdown_with_page_ocr(bytes, engine)
}

#[cfg(feature = "ocr")]
fn to_markdown_with_page_ocr<O: PageOcr>(bytes: &[u8], ocr: &O) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    warn_on_encoding_issues(&result);
    if result.pdf_type == PdfType::TextBased {
        return extracted_markdown(result);
    }

    let extracted = pdf_inspector::extract_pages_markdown_mem(bytes, None).map_err(map_error)?;
    // A scanned document that extracts to nothing at the document level can
    // still have text on its pages (a watermark, a tool footer) which the
    // per-page flags accept as real and skip OCR for. So every page of such
    // a document is recognized and whichever text carry more content wins.
    let scanned_without_text = matches!(result.pdf_type, PdfType::Scanned | PdfType::ImageBased)
        && has_no_extractable_text(&result);
    let recognize = |page: &PageMarkdown| scanned_without_text || page.needs_ocr;
    let required = extracted.pages.iter().filter(|page| recognize(page)).count();
    if required == 0 {
        // Nothing to recognize after all. Re-using the single-pass Markdown
        // costs a second parse but is worth it: the bytes match exactly a
        // converter built without an engine.
        return extracted_markdown(result);
    }

    // Page ids arrive 0-based in document order, so pages are recognized in
    // ascending order and the merge map is keyed by that same index.
    let mut blocks: BTreeMap<u32, String> = BTreeMap::new();
    let mut failed = 0_usize;
    for page in &extracted.pages {
        let extracted_text = page.markdown.trim();
        if !recognize(page) {
            if !extracted_text.is_empty() {
                blocks.insert(page.page, extracted_text.to_string());
            }
            continue;
        }
        // One page per render call keeps peak memory at a single page of
        // pixels, which are released before the next one is rasterized.
        let recognized = match recognize_page(bytes, page.page, ocr) {
            Ok(lines) => Some(lines.join("\n")),
            Err(reason) => {
                failed += 1;
                if extracted_text.is_empty() {
                    log::warn!("page {} was skipped: {reason}", page.page + 1);
                } else {
                    log::warn!("page {} kept its extracted text: {reason}", page.page + 1);
                }
                None
            }
        };
        match select_page_text(page.needs_ocr, extracted_text, recognized.as_deref()) {
            PageText::Recognized => {
                if let Some(recognized) = recognized {
                    if !page.needs_ocr {
                        log::debug!("OCR replaced the extracted text of page {}", page.page + 1);
                    }
                    blocks.insert(page.page, recognized);
                }
            }
            PageText::Extracted => {
                blocks.insert(page.page, extracted_text.to_string());
            }
            PageText::Nothing => {
                if recognized.is_some() {
                    log::debug!("no text was recognized on page {}", page.page + 1);
                }
            }
        }
    }

    if blocks.is_empty() {
        // Recognizing no text is an answer, not a failure: a page that was
        // rendered and read without finding any is a blank page. Only pages
        // that could not be processed at all make the document terminal.
        if failed == 0 {
            return Ok(String::new());
        }
        return Err(ConvertError::Ocr {
            reason: format!("{failed} of {required} pages could not be processed"),
        });
    }
    Ok(merged_pages(&blocks))
}

/// Recognize one page, or say why it has to be skipped.
#[cfg(feature = "ocr")]
fn recognize_page<O: PageOcr>(bytes: &[u8], page: u32, ocr: &O) -> Result<Vec<String>, String> {
    let mut rendered = pdf_inspector::render_pages_mem(bytes, &[page], RenderOptions::new())
        .map_err(|error| format!("the page could not be rendered: {error}"))?;
    let Some(rendered) = rendered.pop() else {
        return Err("the renderer returned no page".to_string());
    };
    if rendered.warnings.contains(&RenderWarning::ImageDecodeFailure) {
        return Err("an image on the page could not be decoded".to_string());
    }
    if rendered.warnings.contains(&RenderWarning::UnsupportedFont) {
        log::debug!("page {} uses an unsupported font; glyphs may be missing", page + 1);
    }
    let lines = ocr
        .recognize_page(rendered.width, rendered.height, &rendered.pixels)
        .map_err(|error| error.to_string())?;
    Ok(recognized_lines(&lines))
}

/// The Markdown lines recognized text contributes, shared by every OCR
/// frontend: line endings normalized, blank lines dropped, syntax escaped.
#[cfg(feature = "ocr")]
pub(super) fn recognized_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| line.split(['\r', '\n']))
        .filter(|line| !line.trim().is_empty())
        .map(escape_block_syntax)
        .collect()
}

/// Escape whatever a recognized line opens with that GFM would read as a
/// block. OCR carries no structure, so the text is otherwise left as-is.
#[cfg(feature = "ocr")]
fn escape_block_syntax(line: &str) -> String {
    const TRIGGERS: [char; 10] = ['#', '>', '-', '+', '*', '`', '|', '=', '_', '~'];

    let (indent, rest) = line.split_at(line.len() - line.trim_start().len());
    let mut escaped = String::with_capacity(line.len() + 1);
    escaped.push_str(indent);
    match rest.chars().next() {
        Some(first) if TRIGGERS.contains(&first) => {
            escaped.push('\\');
            escaped.push_str(rest);
        }
        // An ordered list opens with digits and either a dot or a paren.
        Some(first) if first.is_ascii_digit() => {
            let tail = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            escaped.push_str(&rest[..rest.len() - tail.len()]);
            let mut after_digits = tail.chars();
            match after_digits.next() {
                Some(marker @ ('.' | ')')) => {
                    escaped.push('\\');
                    escaped.push(marker);
                    escaped.push_str(after_digits.as_str());
                }
                _ => escaped.push_str(tail),
            }
        }
        _ => escaped.push_str(rest),
    }
    escaped
}

/// Page blocks in source order, one blank line apart, one trailing newline.
#[cfg(feature = "ocr")]
fn merged_pages(blocks: &BTreeMap<u32, String>) -> String {
    let mut markdown = blocks.values().map(String::as_str).collect::<Vec<_>>().join("\n\n");
    markdown.push('\n');
    markdown
}

/// Whether document-level extraction found nothing worth keeping, which is
/// what makes a converter without an engine call the document unsupported.
#[cfg(feature = "ocr")]
fn has_no_extractable_text(result: &PdfProcessResult) -> bool {
    result.markdown.as_ref().is_none_or(|markdown| markdown.trim().is_empty())
}

/// Which of a page's two readings ends up in the output.
#[cfg(feature = "ocr")]
#[derive(Debug, PartialEq, Eq)]
enum PageText {
    Recognized,
    Extracted,
    Nothing,
}

/// Pick between what the page extracted and what OCR read on it. A flagged
/// page always takes the recognition, anywhere else the richer text wins.
#[cfg(feature = "ocr")]
fn select_page_text(flagged: bool, extracted: &str, recognized: Option<&str>) -> PageText {
    let keeps_extraction = match recognized {
        None => true,
        Some(recognized) => !flagged && text_weight(recognized) <= text_weight(extracted),
    };
    if keeps_extraction {
        return if extracted.is_empty() { PageText::Nothing } else { PageText::Extracted };
    }
    match recognized {
        Some(recognized) if !recognized.is_empty() => PageText::Recognized,
        _ => PageText::Nothing,
    }
}

/// How much text a block carries. Counting alphanumerics ignores case and
/// layout for free, so neither can tip the comparison.
#[cfg(feature = "ocr")]
fn text_weight(text: &str) -> usize {
    text.chars().filter(|character| character.is_alphanumeric()).count()
}

/// The extracted Markdown with exactly one trailing newline, or the terminal
/// error a document without extractable text produces.
fn extracted_markdown(result: PdfProcessResult) -> Result<String, ConvertError> {
    match result.markdown {
        Some(mut markdown) if !markdown.trim().is_empty() => {
            if !markdown.ends_with('\n') {
                markdown.push('\n');
            }
            Ok(markdown)
        }
        _ => Err(ConvertError::Unsupported(format!(
            "PDF has no extractable text ({:?}, {} pages): OCR is required",
            result.pdf_type, result.page_count
        ))),
    }
}

fn warn_on_encoding_issues(result: &PdfProcessResult) {
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
}

fn map_error(e: PdfError) -> ConvertError {
    match e {
        PdfError::Encrypted => ConvertError::Encrypted,
        PdfError::Io(e) => ConvertError::Io(e),
        PdfError::NotAPdf(detail) => ConvertError::malformed(format!("not a PDF: {detail}")),
        PdfError::InvalidStructure => ConvertError::malformed("invalid PDF structure"),
        PdfError::Parse(detail) => ConvertError::malformed(detail),
    }
}

#[cfg(all(test, feature = "ocr"))]
mod tests {
    use super::pdf_fixture::{
        WATERMARK, image_only_pdf, mixed_text_and_image_pdf, text_only_pdf, watermarked_image_pdf,
    };
    use super::{
        ConvertError, PageText, PdfType, escape_block_syntax, select_page_text, text_weight,
        to_markdown, to_markdown_with_page_ocr,
    };
    use crate::ocr::{OcrPageError, PageOcr};
    use std::cell::RefCell;

    /// The corpus PDF, which pdf-inspector classifies as `Mixed` while
    /// flagging none of its pages: the second routing branch.
    fn corpus_text_pdf() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("pdf")
            .join("text.pdf");
        std::fs::read(path).unwrap()
    }

    /// Deterministic stand-in for the model-backed engine. It records the
    /// pages it was handed, checking that each one arrives as a complete RGBA
    /// buffer, and replays a scripted result.
    struct FakeOcr {
        result: Result<Vec<String>, ()>,
        pages: RefCell<Vec<(u32, u32)>>,
    }

    impl FakeOcr {
        fn recognizing(lines: &[&str]) -> Self {
            let lines = lines.iter().map(|line| (*line).to_string()).collect();
            Self { result: Ok(lines), pages: RefCell::new(Vec::new()) }
        }

        fn failing() -> Self {
            Self { result: Err(()), pages: RefCell::new(Vec::new()) }
        }

        fn pages(&self) -> Vec<(u32, u32)> {
            self.pages.borrow().clone()
        }
    }

    impl PageOcr for FakeOcr {
        fn recognize_page(
            &self,
            width: u32,
            height: u32,
            rgba: &[u8],
        ) -> Result<Vec<String>, OcrPageError> {
            assert_eq!(rgba.len(), width as usize * height as usize * 4);
            self.pages.borrow_mut().push((width, height));
            self.result
                .clone()
                .map_err(|()| OcrPageError::Recognition { detail: "fake failure".into() })
        }
    }

    /// The routing the whole pipeline rests on: pdf-inspector has to see the
    /// generated image page as scanned and both text pages as extractable.
    #[test]
    fn the_fixtures_are_classified_as_the_pipeline_expects() {
        let image_only = image_only_pdf();
        let classified = pdf_inspector::process_pdf_mem(&image_only).unwrap();
        assert_eq!(classified.pages_needing_ocr, [1]);
        let pages = pdf_inspector::extract_pages_markdown_mem(&image_only, None).unwrap();
        assert!(pages.pages[0].needs_ocr);

        let mixed = mixed_text_and_image_pdf();
        let pages = pdf_inspector::extract_pages_markdown_mem(&mixed, None).unwrap();
        let needing_ocr: Vec<u32> =
            pages.pages.iter().filter(|page| page.needs_ocr).map(|page| page.page).collect();
        assert_eq!(needing_ocr, [1]);
    }

    /// Configuring an engine may not change a document that has no page to
    /// recognize, whether the detector called it text-based (this case) or
    /// not (the corpus PDF below).
    #[test]
    fn a_text_document_converts_exactly_as_it_does_without_an_engine() {
        let pdf = text_only_pdf();
        let ocr = FakeOcr::recognizing(&["unexpected"]);
        let with_engine = to_markdown_with_page_ocr(&pdf, &ocr).unwrap();

        assert!(ocr.pages().is_empty());
        assert_eq!(with_engine, to_markdown(&pdf).unwrap());
        assert!(with_engine.contains("Structured first page"));
    }

    #[test]
    fn a_document_with_no_flagged_page_converts_exactly_as_it_does_without_an_engine() {
        let pdf = corpus_text_pdf();
        let ocr = FakeOcr::recognizing(&["unexpected"]);
        let with_engine = to_markdown_with_page_ocr(&pdf, &ocr).unwrap();

        assert!(ocr.pages().is_empty());
        assert_eq!(with_engine, to_markdown(&pdf).unwrap());
    }

    /// The mixed fixture is a scanned document that extracts to nothing at the
    /// document level, so every page is read. Its text pages extract far more
    /// than this recognizer returns and keep their Markdown; the scanned page
    /// has none to compare against and takes the recognized text.
    #[test]
    fn a_scanned_document_reads_every_page_and_keeps_the_richer_text() {
        let ocr = FakeOcr::recognizing(&["Recognized page"]);
        let markdown = to_markdown_with_page_ocr(&mixed_text_and_image_pdf(), &ocr).unwrap();

        // 612 x 792 points at the default 200 DPI, once per page.
        assert_eq!(ocr.pages(), [(1699, 2200), (1699, 2200), (1699, 2200)]);
        assert_eq!(
            markdown,
            "Structured first page with two extracted lines.\n\
             \n\
             Recognized page\n\
             \n\
             Structured third page with two extracted lines.\n"
        );
    }

    #[test]
    fn a_page_without_recognized_text_contributes_nothing() {
        let ocr = FakeOcr::recognizing(&[]);
        let markdown = to_markdown_with_page_ocr(&mixed_text_and_image_pdf(), &ocr).unwrap();

        assert_eq!(ocr.pages().len(), 3);
        assert_eq!(
            markdown,
            "Structured first page with two extracted lines.\n\
             \n\
             Structured third page with two extracted lines.\n"
        );
    }

    /// A page that cannot be recognized keeps whatever was extracted from it,
    /// so a failure never costs text the document already had.
    #[test]
    fn a_failed_page_falls_back_to_its_extracted_text() {
        let ocr = FakeOcr::failing();
        let markdown = to_markdown_with_page_ocr(&mixed_text_and_image_pdf(), &ocr).unwrap();

        // The scanned page had nothing to fall back to and is skipped.
        assert_eq!(
            markdown,
            "Structured first page with two extracted lines.\n\
             \n\
             Structured third page with two extracted lines.\n"
        );
    }

    /// A flagged page's extraction was already judged unusable, so however
    /// little OCR reads there, it is still the better of the two.
    #[test]
    fn a_flagged_page_takes_its_recognition_however_thin() {
        assert_eq!(select_page_text(true, WATERMARK, Some("Trial")), PageText::Recognized);
        assert_eq!(select_page_text(true, "", Some("Trial")), PageText::Recognized);
    }

    /// An unflagged page has text worth keeping, so recognition has to read
    /// strictly more to take its place.
    #[test]
    fn an_unflagged_page_takes_recognition_only_when_it_reads_richer() {
        let richer = "Open a ticket, assign it to yourself, fix the issue and close it";
        assert!(text_weight(richer) > text_weight(WATERMARK));

        assert_eq!(select_page_text(false, WATERMARK, Some(richer)), PageText::Recognized);
        assert_eq!(select_page_text(false, WATERMARK, Some("Trial")), PageText::Extracted);
        // A tie leaves the extraction in place.
        assert_eq!(select_page_text(false, WATERMARK, Some(WATERMARK)), PageText::Extracted);
    }

    /// Loss prevention: a page that could not be recognized keeps whatever
    /// was extracted from it, and contributes nothing only when it had none.
    #[test]
    fn a_failed_page_keeps_whatever_it_had() {
        assert_eq!(select_page_text(false, WATERMARK, None), PageText::Extracted);
        assert_eq!(select_page_text(true, WATERMARK, None), PageText::Extracted);
        assert_eq!(select_page_text(true, "", None), PageText::Nothing);
        assert_eq!(select_page_text(true, "", Some("")), PageText::Nothing);
    }

    /// The synthetic watermark fixture as pdf-inspector now reads it: the
    /// page is flagged with no extractable text of its own, so recognition
    /// stands alone. It no longer matches corpus 018's shape (unflagged, with
    /// the watermark as its text), which the ignored corpus test now owns.
    #[test]
    fn a_scanned_page_contributes_what_was_recognized_on_it() {
        let pdf = watermarked_image_pdf();

        let classified = pdf_inspector::process_pdf_mem(&pdf).unwrap();
        let extracted = pdf_inspector::extract_pages_markdown_mem(&pdf, None).unwrap();
        assert_eq!(classified.pdf_type, PdfType::ImageBased);
        assert!(classified.markdown.is_none());
        assert!(extracted.pages[0].needs_ocr);
        assert!(extracted.pages[0].markdown.is_empty());

        let recognized = ["Open a ticket, assign it to yourself,", "fix the issue and close it"];
        let ocr = FakeOcr::recognizing(&recognized);
        let markdown = to_markdown_with_page_ocr(&pdf, &ocr).unwrap();

        assert_eq!(ocr.pages(), [(1699, 2200)]);
        assert_eq!(markdown, format!("{}\n{}\n", recognized[0], recognized[1]));
        assert!(!markdown.contains("Trial Version"));
    }

    #[test]
    fn a_document_whose_pages_all_failed_is_a_terminal_error() {
        let failing = to_markdown_with_page_ocr(&image_only_pdf(), &FakeOcr::failing());

        let Err(ConvertError::Ocr { reason }) = failing else {
            panic!("a document with no recoverable content converted");
        };
        assert!(reason.contains("1 of 1 pages"), "{reason}");
    }

    /// Reading a page and finding nothing on it is not a failure, so the
    /// document converts to the empty output a content-free document has.
    #[test]
    fn a_document_whose_pages_recognize_no_text_converts_to_nothing() {
        let empty = to_markdown_with_page_ocr(&image_only_pdf(), &FakeOcr::recognizing(&[]));

        assert_eq!(empty.unwrap(), "");
    }

    #[test]
    fn recognized_lines_keep_their_text_but_not_block_syntax() {
        for (line, expected) in [
            ("# not a heading", "\\# not a heading"),
            ("> not a quote", "\\> not a quote"),
            ("- not a list", "\\- not a list"),
            ("+ not a list", "\\+ not a list"),
            ("* not a list", "\\* not a list"),
            ("`not code`", "\\`not code`"),
            ("| not | a table |", "\\| not | a table |"),
            ("1. not a list", "1\\. not a list"),
            ("  12. not a list", "  12\\. not a list"),
            // Underlining a line above turns it into a heading, and a run of
            // any of these opens a break or a fence.
            ("=== not an underline", "\\=== not an underline"),
            ("___ not a signature rule", "\\___ not a signature rule"),
            ("~~~ not a fence", "\\~~~ not a fence"),
            ("1) not a list", "1\\) not a list"),
            // A trigger is escaped wherever it opens the line, as the dash
            // already was: "~5 pages" reads the same either way.
            ("~5 pages", "\\~5 pages"),
            // Away from the start of the line these characters are ordinary.
            ("a - b", "a - b"),
            ("x #1", "x #1"),
            ("2 items", "2 items"),
            ("a = b", "a = b"),
            ("x_y", "x_y"),
            ("call (a) now", "call (a) now"),
            ("Größe 12 µm", "Größe 12 µm"),
        ] {
            assert_eq!(escape_block_syntax(line), expected, "escaping {line:?}");
        }
    }

    /// Only the characters that carry content count, so layout and case never
    /// decide which of two readings of a page is kept.
    #[test]
    fn text_weight_counts_content_characters_only() {
        assert_eq!(text_weight("Open a ticket"), 11);
        assert_eq!(text_weight("OPEN   A\nTICKET"), 11);
        assert_eq!(text_weight("  ...--- \t\n "), 0);
        assert_eq!(text_weight("Größe 12 µm"), 5 + 2 + 2);
    }
}
