//! anydoc converts documents to GitHub-Flavored Markdown.
//!
//! Recovery and skipped-content events are reported through the [`log`]
//! facade (debug/warn level); logging never changes conversion behavior and
//! its messages are not a stable API.

#![warn(missing_docs)]

pub mod model;

mod error;
mod formats;
mod package;
mod render;
mod shared;
mod transform;

pub use error::ConvertError;
pub use transform::images::{ExtractedImage, ImageOptions, ImageReport, extract_images};

use render::markdown::document_to_markdown_with;
use render::text::document_to_text_with;

use std::path::Path;

/// Input format. Selects the parser; container variants that share a parser
/// (docm, xlsm, ...) map onto these via [`Format::from_bytes`] or
/// [`Format::from_extension`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Binary Word 97-2003 (`.doc`), and the OLE2-based Kingsoft WPS Writer
    /// / Microsoft Works wrapper that shares its `WordDocument` stream (`.wps`).
    Doc,
    /// WordprocessingML (`.docx`, `.docm`), both Transitional and Strict.
    Docx,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// Converted with [pdf-inspector], which emits Markdown directly:
    /// [`to_document`] is unsupported for PDFs. Scanned or image-only pages
    /// need OCR, which anydoc does not do: the document errors with
    /// [`ConvertError::NeedsOcr`] naming them.
    ///
    /// [pdf-inspector]: https://github.com/firecrawl/pdf-inspector
    Pdf,
    /// Binary PowerPoint 97-2003 (`.ppt`, `.pps`, `.pot`).
    Ppt,
    /// PresentationML (`.pptx`, `.pptm`, `.ppsx`, `.ppsm`).
    Pptx,
    /// Rich Text Format (`.rtf`).
    Rtf,
    /// EPUB 2 and 3 (`.epub`).
    Epub,
    /// Excel workbooks in every container calamine reads: `.xlsx`, `.xlsm`,
    /// `.xlsb`, binary `.xls`, and the OLE2-based Kingsoft WPS Spreadsheet
    /// wrapper that shares its `Workbook`/`Book` stream (`.et`).
    Excel,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
    /// OpenDocument Presentation (`.odp`).
    Odp,
    /// Delimiter-separated text (`.csv`). Carries no signature, so it has to
    /// be named rather than detected.
    Csv,
}

impl Format {
    /// Detect the format from the content itself: the signature and identity
    /// each container specification designates (PDF header, RTF open group,
    /// OLE stream names, ZIP package mimetype/content types). Plain-text
    /// formats (CSV) carry no signature and return `None`; so does anything
    /// unrecognized.
    pub fn from_bytes(bytes: &[u8]) -> Option<Format> {
        formats::detect::from_bytes(bytes)
    }

    /// The format a bare extension names (no leading dot), matched
    /// case-insensitively. `None` for anything unrecognized.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            // `.wps` is the Kingsoft WPS Writer / Microsoft Works wrapper,
            // an OLE2 compound file with a `WordDocument` stream like `.doc`.
            "doc" | "wps" => Format::Doc,
            "docx" | "docm" => Format::Docx,
            "odt" => Format::Odt,
            "pdf" => Format::Pdf,
            "pptx" | "pptm" | "ppsx" | "ppsm" => Format::Pptx,
            "ppt" | "pps" | "pot" => Format::Ppt,
            "rtf" => Format::Rtf,
            "epub" => Format::Epub,
            // `.et` is the Kingsoft WPS Spreadsheet wrapper; modern files are
            // an OLE2 compound file with a `Workbook`/`Book` stream like `.xls`.
            "xlsx" | "xlsm" | "xlsb" | "xls" | "et" => Format::Excel,
            "ods" => Format::Ods,
            "odp" => Format::Odp,
            "csv" => Format::Csv,
            _ => return None,
        })
    }

    /// The format a path's extension names. `None` when the path has no
    /// extension or names nothing recognized.
    pub fn from_path(path: &Path) -> Option<Format> {
        path.extension().and_then(|e| e.to_str()).and_then(Format::from_extension)
    }
}

/// Convert a document file to Markdown. The format is detected from the
/// file content ([`Format::from_bytes`]); the extension is the fallback for
/// signature-less formats (CSV) and unrecognizable containers.
pub fn to_markdown(path: impl AsRef<Path>) -> Result<String, ConvertError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
        return Err(ConvertError::Unsupported(format!(
            "unrecognized file content and extension: {}",
            path.display()
        )));
    };
    to_markdown_bytes(&bytes, format)
}

/// Convert an in-memory document to Markdown. Pass a [`Format`] to select the
/// parser, or `None` to detect it from the content ([`Format::from_bytes`]),
/// which signature-less formats (CSV) have to name explicitly.
pub fn to_markdown_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    to_markdown_bytes_opts(bytes, format, true)
}

/// Fork (sznb): like [`to_markdown_bytes`], with the table form selected by
/// `html_tables` — `true` renders HTML `<table>` carrying merged-cell spans,
/// `false` renders GFM pipe tables.
pub fn to_markdown_bytes_opts(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    html_tables: bool,
) -> Result<String, ConvertError> {
    to_markdown_bytes_with(bytes, format, ConvertOptions { html_tables, ..Default::default() })
}

/// Fork (sznb): output-shaping options — the conversion service's
/// `table_as_html` / `dedup_table_cells` / `content` parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvertOptions {
    /// Markdown tables as HTML `<table>` carrying merged-cell spans (true)
    /// or GFM pipe tables (false). Plain text ignores it.
    pub html_tables: bool,
    /// Merged cells show their content once, on the origin (true), or repeat
    /// it in every position the merge covers (false; spans collapse).
    pub dedup_cells: bool,
    /// Keep the table of contents (true) or drop it (false): Word `Table of
    /// Contents` SDTs and bare `TOC` field results, with the 目录 title just
    /// before. Only the Word frontends (docx, doc / wps) carry a TOC.
    pub include_toc: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        ConvertOptions { html_tables: true, dedup_cells: true, include_toc: true }
    }
}

/// Fork (sznb): [`to_markdown_bytes`] with every output-shaping option.
pub fn to_markdown_bytes_with(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    opts: ConvertOptions,
) -> Result<String, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    // PDFs convert to Markdown directly (pdf-inspector) without passing
    // through the document model; the shaping options do not apply.
    if format == Format::Pdf {
        return formats::pdf::to_markdown(bytes);
    }
    let doc = formats::parse_with(bytes, format, opts.include_toc)?;
    Ok(document_to_markdown_with(&doc, opts.html_tables, opts.dedup_cells))
}

/// Fork (sznb): convert an in-memory document to plain text (no Markdown
/// syntax; tables render as tab-joined rows carrying merged content once).
///
/// Unsupported for [`Format::Pdf`]: PDF conversion produces Markdown
/// directly and has no document-model form; use [`to_markdown_bytes`].
pub fn to_text_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    to_text_bytes_with(bytes, format, ConvertOptions::default())
}

/// Fork (sznb): [`to_text_bytes`] with `dedup_cells` / `include_toc`
/// (`html_tables` does not apply to plain text).
pub fn to_text_bytes_with(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    opts: ConvertOptions,
) -> Result<String, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    if format == Format::Pdf {
        return Err(ConvertError::Unsupported(
            "pdf has no document-model form; use to_markdown_bytes".into(),
        ));
    }
    let doc = formats::parse_with(bytes, format, opts.include_toc)?;
    Ok(document_to_text_with(&doc, opts.dedup_cells))
}

/// Fork (sznb): [`to_markdown_bytes_with`] plus image extraction — embedded
/// images become placeholder tokens in the Markdown (small ones are dropped)
/// and their payloads come back in the [`ImageReport`]; see
/// [`extract_images`]. PDFs convert without the document model, so they
/// yield their Markdown and an empty report.
pub fn to_markdown_with_images(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    opts: ConvertOptions,
    images: &ImageOptions,
) -> Result<(String, ImageReport), ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    if format == Format::Pdf {
        return Ok((formats::pdf::to_markdown(bytes)?, ImageReport::default()));
    }
    let mut doc = formats::parse_with(bytes, format, opts.include_toc)?;
    let report = extract_images(&mut doc, images);
    Ok((document_to_markdown_with(&doc, opts.html_tables, opts.dedup_cells), report))
}

/// Fork (sznb): [`to_text_bytes_with`] plus image extraction (as for
/// [`to_markdown_with_images`]). Unsupported for [`Format::Pdf`].
pub fn to_text_with_images(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    opts: ConvertOptions,
    images: &ImageOptions,
) -> Result<(String, ImageReport), ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    if format == Format::Pdf {
        return Err(ConvertError::Unsupported(
            "pdf has no document-model form; use to_markdown_bytes".into(),
        ));
    }
    let mut doc = formats::parse_with(bytes, format, opts.include_toc)?;
    let report = extract_images(&mut doc, images);
    Ok((document_to_text_with(&doc, opts.dedup_cells), report))
}

/// Parse an in-memory document into the document model. Pass a [`Format`] to
/// select the parser, or `None` to detect it from the content.
///
/// Unsupported for [`Format::Pdf`]: PDF conversion produces Markdown
/// directly and has no document-model form; use [`to_markdown_bytes`].
pub fn to_document(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<model::Document, ConvertError> {
    formats::parse(bytes, resolve_format(bytes, format.into())?)
}

fn resolve_format(bytes: &[u8], format: Option<Format>) -> Result<Format, ConvertError> {
    format.or_else(|| Format::from_bytes(bytes)).ok_or_else(|| {
        ConvertError::Unsupported("unrecognized file content: name the format explicitly".into())
    })
}
