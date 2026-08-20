//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. Scanned and
//! image-only PDFs need OCR, which is out of scope here; they error as
//! unsupported from [`to_markdown`]. Per-page extraction via
//! [`to_markdown_pages`] surfaces OCR needs on each page instead, so callers
//! can route those pages elsewhere. Pages flagged for OCR in an otherwise
//! text-based document degrade with a log from [`to_markdown`], consistent
//! with the crate-wide recovery policy.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::PdfError;

/// One PDF page converted to Markdown.
///
/// `page` is 0-indexed. When [`Self::needs_ocr`] is true, [`Self::markdown`]
/// is empty and [`Self::ocr_reason`] may name why extraction was unreliable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPage {
    /// 0-indexed page number.
    pub page: u32,
    /// Formatted Markdown for this page, or empty when OCR is required.
    pub markdown: String,
    /// True when text on this page is unreliable or missing.
    pub needs_ocr: bool,
    /// Machine-readable OCR reason when the cause is known.
    pub ocr_reason: Option<String>,
}

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    if !result.pages_needing_ocr.is_empty() {
        log::warn!(
            "{} of {} pages need OCR and were not extracted",
            result.pages_needing_ocr.len(),
            result.page_count
        );
    }
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
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

/// Extract Markdown for each page of a PDF.
///
/// Unlike [`to_markdown`], this never collapses the document into one string
/// and does not fail solely because some (or all) pages need OCR. Each page
/// carries its own Markdown and OCR flag so callers can mix direct extraction
/// with an external OCR path.
pub fn to_markdown_pages(bytes: &[u8]) -> Result<Vec<MarkdownPage>, ConvertError> {
    let result = pdf_inspector::extract_pages_markdown_mem(bytes, None).map_err(map_error)?;
    if !result.pages_needing_ocr.is_empty() {
        log::warn!(
            "{} of {} pages need OCR and were not extracted",
            result.pages_needing_ocr.len(),
            result.pages.len()
        );
    }
    Ok(result
        .pages
        .into_iter()
        .map(|page| MarkdownPage {
            page: page.page,
            markdown: page.markdown,
            needs_ocr: page.needs_ocr,
            ocr_reason: page.ocr_reason,
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn text_pdf() -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("pdf")
            .join("text.pdf");
        std::fs::read(path).expect("pdf fixture")
    }

    #[test]
    fn to_markdown_pages_returns_at_least_one_page() {
        let pages = to_markdown_pages(&text_pdf()).expect("extract pages");
        assert!(!pages.is_empty(), "expected at least one page");
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.page, i as u32);
            assert!(
                page.needs_ocr || !page.markdown.trim().is_empty(),
                "page {} should have markdown or need OCR",
                page.page
            );
        }
    }

    #[test]
    fn to_markdown_still_returns_a_single_blob() {
        let markdown = to_markdown(&text_pdf()).expect("extract markdown");
        assert!(!markdown.trim().is_empty());
    }
}
