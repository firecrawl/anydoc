//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer.
//!
//! A PDF is the one input whose pages fail independently: a scanned page in an
//! otherwise text-based document extracts to nothing while its neighbours
//! extract cleanly. [`to_markdown`] returns one string and has nowhere to put
//! that verdict, so it degrades with a log, consistent with the crate-wide
//! recovery policy. [`pages`] returns the verdict per page, which is what a
//! caller able to OCR the remainder needs.
//!
//! **The two entry points do not share a routing verdict, and must not.**
//! [`to_markdown`] reads `process_pdf_mem`'s document-level `pages_needing_ocr`;
//! [`pages`] reads `extract_pages_markdown_mem`'s per-page `needs_ocr`. They
//! disagree, measured on this crate's own `tests/fixtures/pdf/text.pdf`: the
//! document-level call flags page 2, whose text layer in fact yields
//! `"i Endnote body text."` and which the per-page call correctly passes. Route
//! pages by [`pages`] alone — the per-page API is the one documented for it,
//! and the document-level flag sends text pages to an OCR engine.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::PdfError;

/// One page of a PDF, as its own text layer answered for it.
#[derive(Debug, Clone)]
pub struct Page {
    /// 0-indexed page number, in document order.
    pub index: u32,
    /// Markdown extracted from this page's text layer. Empty when the text
    /// layer answered for nothing.
    pub markdown: String,
    /// `true` when the text layer cannot be trusted here: no text at all,
    /// GID-encoded fonts, broken encodings, or garbage output.
    pub needs_ocr: bool,
    /// Machine-readable reason for `needs_ocr`, where the cause is known.
    pub ocr_reason: Option<String>,
}

/// Extract a PDF page by page, keeping the per-page OCR verdict.
///
/// Font statistics are computed across the whole document, so heading
/// thresholds do not depend on how the caller later slices the result. The
/// per-page Markdown is not the same as [`to_markdown`]'s: the whole-document
/// path sees structure across a page break that a page on its own cannot, so
/// joining these strings produces a flatter document than [`to_markdown`] does.
/// Use this to decide what to OCR and to keep what extracted, not to reproduce
/// [`to_markdown`].
pub fn pages(bytes: &[u8]) -> Result<Vec<Page>, ConvertError> {
    let extracted = pdf_inspector::extract_pages_markdown_mem(bytes, None).map_err(map_error)?;
    Ok(extracted
        .pages
        .into_iter()
        .map(|page| Page {
            index: page.page,
            markdown: page.markdown,
            needs_ocr: page.needs_ocr,
            ocr_reason: page.ocr_reason,
        })
        .collect())
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

fn map_error(e: PdfError) -> ConvertError {
    match e {
        PdfError::Encrypted => ConvertError::Encrypted,
        PdfError::Io(e) => ConvertError::Io(e),
        PdfError::NotAPdf(detail) => ConvertError::malformed(format!("not a PDF: {detail}")),
        PdfError::InvalidStructure => ConvertError::malformed("invalid PDF structure"),
        PdfError::Parse(detail) => ConvertError::malformed(detail),
    }
}
