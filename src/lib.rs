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

pub use error::ConvertError;

use render::markdown::document_to_markdown;

use std::path::Path;

/// Input format. Selects the parser; container variants that share a parser
/// (docm, xlsm, ...) map onto these via [`Format::from_bytes`] or
/// [`Format::from_extension`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Binary Word 97-2003 (`.doc`).
    Doc,
    /// WordprocessingML (`.docx`, `.docm`), both Transitional and Strict.
    Docx,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// Converted with [pdf-inspector], which emits Markdown directly:
    /// [`to_document`] is unsupported for PDFs. Scanned or image-only pages
    /// need OCR, which anydoc does not do: the document errors with
    /// [`ConvertError::NeedsOcr`] naming them, or converts without them
    /// under [`Ocr::Skip`].
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
    /// Excel workbooks: `.xlsx`, `.xlsm`, binary `.xlsb`, and legacy
    /// OLE-based `.xls`.
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
            "doc" => Format::Doc,
            "docx" | "docm" => Format::Docx,
            "odt" => Format::Odt,
            "pdf" => Format::Pdf,
            "pptx" | "pptm" | "ppsx" | "ppsm" => Format::Pptx,
            "ppt" | "pps" | "pot" => Format::Ppt,
            "rtf" => Format::Rtf,
            "epub" => Format::Epub,
            "xlsx" | "xlsm" | "xlsb" | "xls" => Format::Excel,
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

/// What conversion does with PDF pages that need OCR.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Ocr {
    /// Fail with [`ConvertError::NeedsOcr`] naming the pages. The default:
    /// Markdown missing those pages would otherwise read as complete.
    #[default]
    Reject,
    /// Convert the pages that carry text instead of failing the document,
    /// and name the pages left out on [`Conversion::pages_needing_ocr`].
    ///
    /// The Markdown holds content and nothing else - the pages left out are
    /// reported as data, never written into the text. A document where no
    /// page yielded text still fails with [`ConvertError::NeedsOcr`]: there
    /// is nothing to hand back, and the error is what tells a caller the
    /// document needs OCR rather than better parsing.
    Skip,
}

/// Conversion options.
///
/// `#[non_exhaustive]`, so build from [`Options::default`] and set what you
/// need: `Options::default().ocr(Ocr::Skip)`.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Options {
    /// What to do with PDF pages that need OCR.
    pub ocr: Ocr,
}

impl Options {
    /// Set what happens to PDF pages that need OCR.
    #[must_use]
    pub fn ocr(mut self, ocr: Ocr) -> Self {
        self.ocr = ocr;
        self
    }
}

/// Markdown from a conversion, and what the conversion left out.
///
/// Returned by [`to_markdown_with`] and [`to_markdown_bytes_with`]. Only PDFs
/// paginate, so every other format reports no pages.
#[derive(Debug, Clone)]
pub struct Conversion {
    /// The Markdown. Content only: conversion never writes notes about
    /// itself into the output.
    pub markdown: String,
    /// 1-indexed PDF pages left unconverted because they need OCR. Only
    /// [`Ocr::Skip`] can leave any: otherwise the document would have
    /// errored. Empty for a document that converted whole.
    pub pages_needing_ocr: Vec<u32>,
    /// Pages in the document, or 0 for the formats that do not paginate.
    pub page_count: u32,
}

/// Convert a document file to Markdown. The format is detected from the
/// file content ([`Format::from_bytes`]); the extension is the fallback for
/// signature-less formats (CSV) and unrecognizable containers.
pub fn to_markdown(path: impl AsRef<Path>) -> Result<String, ConvertError> {
    to_markdown_with(path, Options::default()).map(|converted| converted.markdown)
}

/// [`to_markdown`] with [`Options`], reporting what it left out.
pub fn to_markdown_with(
    path: impl AsRef<Path>,
    options: Options,
) -> Result<Conversion, ConvertError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
        return Err(ConvertError::Unsupported(format!(
            "unrecognized file content and extension: {}",
            path.display()
        )));
    };
    to_markdown_bytes_with(&bytes, format, options)
}

/// Convert an in-memory document to Markdown. Pass a [`Format`] to select the
/// parser, or `None` to detect it from the content ([`Format::from_bytes`]),
/// which signature-less formats (CSV) have to name explicitly.
pub fn to_markdown_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    to_markdown_bytes_with(bytes, format, Options::default()).map(|converted| converted.markdown)
}

/// [`to_markdown_bytes`] with [`Options`], reporting what it left out.
pub fn to_markdown_bytes_with(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
    options: Options,
) -> Result<Conversion, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    // PDFs convert to Markdown directly (pdf-inspector) without passing
    // through the document model, and are the only format with pages to
    // leave out.
    if format == Format::Pdf {
        return formats::pdf::to_markdown(bytes, options.ocr);
    }
    let markdown = document_to_markdown(&to_document(bytes, format)?);
    Ok(Conversion { markdown, pages_needing_ocr: Vec::new(), page_count: 0 })
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
