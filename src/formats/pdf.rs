//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. OCR is out of scope
//! here: by default a document with scanned or image-only pages errors naming
//! them, whether that is every page or one of a hundred, because output
//! missing those pages would read as complete.
//!
//! [`Ocr::Skip`] is the opt-out for callers who would rather have the readable
//! pages than nothing. It converts them page by page and names the pages it
//! left out on [`Conversion::pages_needing_ocr`], so the gap is reported as
//! data rather than written into the Markdown - output that is shown to
//! readers or fed to a model should carry content and nothing else. A document
//! where no page yielded text still errors: there is nothing to hand back.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::{ConvertError, page_ranges};
use crate::{Conversion, Ocr};
use pdf_inspector::PdfError;

pub fn to_markdown(bytes: &[u8], ocr: Ocr) -> Result<Conversion, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    if !result.pages_needing_ocr.is_empty() {
        // Detection samples content streams and over-reports short or
        // image-heavy text pages; extraction knows which of them yielded none.
        let flagged: Vec<u32> = result.pages_needing_ocr.iter().map(|page| page - 1).collect();
        let pages = pdf_inspector::extract_pages_markdown_mem(bytes, Some(&flagged))
            .map_err(map_error)?
            .pages_needing_ocr;
        if !pages.is_empty() {
            return match ocr {
                Ocr::Reject => Err(ConvertError::NeedsOcr { pages, page_count: result.page_count }),
                Ocr::Skip => skip_pages_needing_ocr(bytes, pages, result.page_count),
            };
        }
    }
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
    match result.markdown {
        Some(mut markdown) if !markdown.trim().is_empty() => {
            if !markdown.ends_with('\n') {
                markdown.push('\n');
            }
            Ok(Conversion {
                markdown,
                pages_needing_ocr: Vec::new(),
                page_count: result.page_count,
            })
        }
        _ => Err(ConvertError::Unsupported(format!(
            "PDF has no extractable text ({:?}, {} pages)",
            result.pdf_type, result.page_count
        ))),
    }
}

/// The readable pages of a document whose other pages need OCR.
///
/// Extraction runs over every page rather than the flagged ones alone: whole
/// document extraction gives up entirely on a document it classifies as
/// image-based, even where single pages do carry text, and detection samples
/// pages rather than reading all of them. The per-page verdicts are the ones
/// that decide both what is converted and what is reported, so the two can
/// never disagree.
fn skip_pages_needing_ocr(
    bytes: &[u8],
    confirmed: Vec<u32>,
    page_count: u32,
) -> Result<Conversion, ConvertError> {
    let extracted = pdf_inspector::extract_pages_markdown_mem(bytes, None).map_err(map_error)?;
    let mut markdown = String::new();
    let mut pages_needing_ocr = Vec::new();
    for page in &extracted.pages {
        // `page` is 0-indexed here; every page number anydoc reports is not.
        let number = page.page + 1;
        // Unreliable text (broken encodings, garbage, nothing at all) is
        // dropped rather than passed off as content.
        if page.needs_ocr {
            pages_needing_ocr.push(number);
            continue;
        }
        let text = page.markdown.trim_end();
        if text.is_empty() {
            continue;
        }
        if !markdown.is_empty() {
            markdown.push('\n');
        }
        markdown.push_str(text);
        markdown.push('\n');
    }
    if markdown.is_empty() {
        // Nothing readable came out, so there is no partial result to give:
        // this is the fully scanned document the default already rejects, and
        // it errors with the same pages the default would have named.
        let pages = if pages_needing_ocr.is_empty() { confirmed } else { pages_needing_ocr };
        return Err(ConvertError::NeedsOcr { pages, page_count });
    }
    log::warn!(
        "converted {} of {page_count} pages; pages {} need OCR and were left out",
        (page_count as usize).saturating_sub(pages_needing_ocr.len()),
        page_ranges(&pages_needing_ocr)
    );
    Ok(Conversion { markdown, pages_needing_ocr, page_count })
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
