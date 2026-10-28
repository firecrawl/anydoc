//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. OCR extraction is out
//! of scope here: a document that yields no readable text at all errors
//! naming the pages that need it. A document that still has other readable
//! text keeps that text and logs a warning naming the confirmed-OCR pages
//! instead, since discarding the whole document over a handful of pages
//! would lose more than it protects (#162).
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::PdfError;

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    let has_text = result.markdown.as_deref().is_some_and(|m| !m.trim().is_empty());
    if !result.pages_needing_ocr.is_empty() {
        // Detection samples content streams and over-reports short or
        // image-heavy text pages; extraction knows which of them yielded none.
        let flagged: Vec<u32> = result.pages_needing_ocr.iter().map(|page| page - 1).collect();
        let pages = pdf_inspector::extract_pages_markdown_mem(bytes, Some(&flagged))
            .map_err(map_error)?
            .pages_needing_ocr;
        if !pages.is_empty() {
            if !has_text {
                return Err(ConvertError::NeedsOcr { pages, page_count: result.page_count });
            }
            // The document as a whole already classified as text-based and
            // extracted real content; a handful of confirmed pages should not
            // discard it (#162), so report them instead of failing the document.
            log::warn!(
                "{} of {} pages need OCR and were not extracted",
                pages.len(),
                result.page_count
            );
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
            Ok(markdown)
        }
        _ => Err(ConvertError::Unsupported(format!(
            "PDF has no extractable text ({:?}, {} pages)",
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
