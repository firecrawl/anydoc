//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. OCR is out of scope
//! here: a document with scanned or image-only pages errors naming them,
//! whether that is every page or one of a hundred, because output missing
//! those pages would read as complete.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::PdfError;

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    let mut markdown = result.markdown;
    if !result.pages_needing_ocr.is_empty() {
        // Detection samples content streams and over-reports short or
        // image-heavy text pages; extraction knows which of them yielded none.
        // When process_pdf_mem omitted markdown (e.g. Scanned/ImageBased),
        // extract all pages in one pass so we both verify OCR needs and keep text.
        let needs_full_markdown = markdown.as_deref().is_none_or(|s| s.trim().is_empty());
        let flagged: Vec<u32> = result.pages_needing_ocr.iter().map(|page| page - 1).collect();
        let page_filter = (!needs_full_markdown).then_some(flagged.as_slice());
        let extracted =
            pdf_inspector::extract_pages_markdown_mem(bytes, page_filter).map_err(map_error)?;
        if !extracted.pages_needing_ocr.is_empty() {
            return Err(ConvertError::NeedsOcr {
                pages: extracted.pages_needing_ocr,
                page_count: result.page_count,
            });
        }
        if needs_full_markdown {
            let joined = extracted
                .pages
                .into_iter()
                .map(|p| p.markdown)
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if !joined.is_empty() {
                markdown = Some(joined);
            }
        }
    }
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
    match markdown {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn handmade_pdf(pages: &[&[u8]]) -> Vec<u8> {
        let n = pages.len();
        let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
        objs.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
        let mut kids = String::new();
        for i in 0..n {
            if i > 0 {
                kids.push(' ');
            }
            let _ = write!(kids, "{} 0 R", 5 + 2 * i);
        }
        objs.push((2, format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes()));
        objs.push((3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()));
        objs.push((
            4,
            b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\x80\nendstream".to_vec(),
        ));
        for (i, content) in pages.iter().enumerate() {
            let page = 5 + 2 * i;
            let stream = 6 + 2 * i;
            objs.push((
                page,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 4 0 R >> >> /Contents {stream} 0 R >>"
                )
                .into_bytes(),
            ));
            let mut s = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            s.extend_from_slice(content);
            s.extend_from_slice(b"\nendstream");
            objs.push((stream, s));
        }

        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objs.len());
        for (num, body) in &objs {
            offsets.push(out.len());
            out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn image_only_page_reports_needs_ocr() {
        let pdf = handmade_pdf(&[b"q 468 0 0 648 72 72 cm /Im1 Do Q"]);
        assert!(matches!(
            to_markdown(&pdf),
            Err(ConvertError::NeedsOcr { pages, page_count: 1 }) if pages == vec![1]
        ));
    }

    #[test]
    fn image_with_short_caption_extracts_caption_markdown() {
        let pdf = handmade_pdf(&[
            b"q 468 0 0 648 72 72 cm /Im1 Do Q\nBT /F1 12 Tf 72 50 Td (Photo 1) Tj ET",
        ]);
        let md = to_markdown(&pdf).expect("caption text should be extracted");
        assert!(md.contains("Photo 1"), "unexpected markdown: {md:?}");
    }
}
