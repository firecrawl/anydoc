//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. Scanned and
//! image-only PDFs need OCR, which is out of scope here; they error as
//! unsupported. Pages flagged for OCR in an otherwise text-based document
//! degrade with a log, consistent with the crate-wide recovery policy.
//! The optional `pdf-images` feature can return positioned, rendered image
//! bytes together with matching file names in the Markdown image targets.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::{PdfError, PdfOptions, PdfProcessResult};

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let mut result =
        pdf_inspector::process_pdf_mem_with_options(bytes, PdfOptions::new()).map_err(map_error)?;
    take_markdown(&mut result)
}

#[cfg(feature = "pdf-images")]
pub fn to_markdown_with_images(bytes: &[u8]) -> Result<crate::PdfConversion, ConvertError> {
    use pdf_inspector::{MarkdownOptions, RenderOptions};

    let markdown_options = MarkdownOptions { include_images: true, ..MarkdownOptions::default() };
    let mut result = pdf_inspector::process_pdf_mem_with_options(
        bytes,
        PdfOptions::new().markdown(markdown_options),
    )
    .map_err(map_error)?;
    let mut markdown = take_markdown(&mut result)?;
    let mut total_bytes = 0_usize;
    let extracted =
        pdf_inspector::extract_images_mem(bytes, RenderOptions::new()).map_err(map_render_error)?;
    let mut images = Vec::with_capacity(extracted.len());
    for image in extracted {
        let markdown_target = format!("]({})", image.reference);
        if !markdown.contains(&markdown_target) {
            continue;
        }

        let filename = format!("p{}_i{}.png", image.page + 1, image.occurrence);
        markdown = markdown.replacen(&markdown_target, &format!("]({filename})"), 1);
        let data = encode_png(image.width, image.height, &image.pixels)?;
        total_bytes =
            total_bytes.checked_add(data.len()).ok_or_else(|| ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: "PDF image assets exceed the retained-bytes cap".into(),
            })?;
        if total_bytes > crate::package::limits::MAX_ASSET_TOTAL_BYTES {
            return Err(ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: "PDF image assets exceed the retained-bytes cap".into(),
            });
        }
        images.push(crate::PdfImage {
            filename,
            page: image.page + 1,
            bbox: image.bbox,
            width: image.width,
            height: image.height,
            data,
            warnings: image.warnings.into_iter().map(|warning| warning.code().to_owned()).collect(),
        });
    }

    Ok(crate::PdfConversion {
        markdown,
        images,
        page_count: result.page_count,
        pages_needing_ocr: result.pages_needing_ocr,
        ocr_reasons_by_page: result
            .ocr_reasons_by_page
            .into_iter()
            .map(|reason| crate::PdfOcrReasons { page: reason.page, reasons: reason.reasons })
            .collect(),
        pages_with_tables: result.layout.pages_with_tables,
        pages_with_columns: result.layout.pages_with_columns,
        is_complex_layout: result.layout.is_complex,
        has_encoding_issues: result.has_encoding_issues,
    })
}

fn take_markdown(result: &mut PdfProcessResult) -> Result<String, ConvertError> {
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
    match result.markdown.take() {
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

#[cfg(feature = "pdf-images")]
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| ConvertError::malformed(format!("cannot encode PDF image: {error}")))?;
    writer
        .write_image_data(pixels)
        .map_err(|error| ConvertError::malformed(format!("cannot encode PDF image: {error}")))?;
    writer
        .finish()
        .map_err(|error| ConvertError::malformed(format!("cannot encode PDF image: {error}")))?;
    Ok(data)
}

#[cfg(feature = "pdf-images")]
fn map_render_error(error: pdf_inspector::RenderError) -> ConvertError {
    use pdf_inspector::RenderError;

    match error {
        RenderError::Encrypted => ConvertError::Encrypted,
        RenderError::PageDimensionsTooLarge { .. }
        | RenderError::PagePixelsTooLarge { .. }
        | RenderError::OutputTooLarge { .. }
        | RenderError::TooManyPages { .. } => {
            ConvertError::ResourceLimit { limit: "max_pdf_image_output", detail: error.to_string() }
        }
        _ => ConvertError::malformed(error.to_string()),
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
