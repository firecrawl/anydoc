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

/// Reusable document converter.
///
/// The default converter has the same behavior as the crate-level conversion
/// functions. Build a configured converter with [`Converter::builder`] when a
/// conversion capability needs reusable initialization.
///
/// Marked non-exhaustive so optional features can add fields without
/// breaking construction in downstream crates.
#[non_exhaustive]
pub struct Converter;

impl Converter {
    /// Create a converter with the default capabilities.
    pub fn new() -> Self {
        Self
    }

    /// Start building a configured converter.
    pub fn builder() -> ConverterBuilder {
        ConverterBuilder::new()
    }

    /// Convert a document file to Markdown. The format is detected from the
    /// file content ([`Format::from_bytes`]); the extension is the fallback for
    /// signature-less formats (CSV) and unrecognizable containers.
    pub fn to_markdown(&self, path: impl AsRef<Path>) -> Result<String, ConvertError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
            return Err(ConvertError::Unsupported(format!(
                "unrecognized file content and extension: {}",
                path.display()
            )));
        };
        self.to_markdown_bytes(&bytes, format)
    }

    /// Convert an in-memory document to Markdown. Pass a [`Format`] to select
    /// the parser, or `None` to detect it from the content
    /// ([`Format::from_bytes`]), which signature-less formats (CSV) have to
    /// name explicitly.
    pub fn to_markdown_bytes(
        &self,
        bytes: &[u8],
        format: impl Into<Option<Format>>,
    ) -> Result<String, ConvertError> {
        let format = resolve_format(bytes, format.into())?;
        // PDFs convert to Markdown directly (pdf-inspector) without passing
        // through the document model.
        if format == Format::Pdf {
            return formats::pdf::to_markdown(bytes);
        }
        Ok(document_to_markdown(&self.to_document(bytes, format)?))
    }

    /// Parse an in-memory document into the document model. Pass a [`Format`]
    /// to select the parser, or `None` to detect it from the content.
    ///
    /// Unsupported for [`Format::Pdf`]: PDF conversion produces Markdown
    /// directly and has no document-model form; use
    /// [`Converter::to_markdown_bytes`].
    pub fn to_document(
        &self,
        bytes: &[u8],
        format: impl Into<Option<Format>>,
    ) -> Result<model::Document, ConvertError> {
        formats::parse(bytes, resolve_format(bytes, format.into())?)
    }
}

impl Default for Converter {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for a reusable [`Converter`].
///
/// Marked non-exhaustive so optional features can add fields without
/// breaking construction in downstream crates.
#[non_exhaustive]
pub struct ConverterBuilder;

impl ConverterBuilder {
    /// Create a converter builder with the default capabilities.
    pub fn new() -> Self {
        Self
    }

    /// Build the converter.
    pub fn build(self) -> Converter {
        Converter::new()
    }
}

impl Default for ConverterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// [`to_document`] is unsupported for PDFs. Scanned/image-only PDFs
    /// (needing OCR) error as unsupported.
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
    /// `.xlsb`, and binary `.xls`.
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

/// Convert a document file to Markdown. The format is detected from the
/// file content ([`Format::from_bytes`]); the extension is the fallback for
/// signature-less formats (CSV) and unrecognizable containers.
pub fn to_markdown(path: impl AsRef<Path>) -> Result<String, ConvertError> {
    Converter::new().to_markdown(path)
}

/// Convert an in-memory document to Markdown. Pass a [`Format`] to select the
/// parser, or `None` to detect it from the content ([`Format::from_bytes`]),
/// which signature-less formats (CSV) have to name explicitly.
pub fn to_markdown_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    Converter::new().to_markdown_bytes(bytes, format)
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
    Converter::new().to_document(bytes, format)
}

fn resolve_format(bytes: &[u8], format: Option<Format>) -> Result<Format, ConvertError> {
    format.or_else(|| Format::from_bytes(bytes)).ok_or_else(|| {
        ConvertError::Unsupported("unrecognized file content: name the format explicitly".into())
    })
}

#[cfg(test)]
mod tests {
    use super::{Converter, Format};

    #[test]
    fn converter_matches_the_free_functions() {
        let input = br"{\rtf1 reusable converter}";
        let from_function = super::to_markdown_bytes(input, Format::Rtf).unwrap();
        let from_converter = Converter::new().to_markdown_bytes(input, Format::Rtf).unwrap();
        assert_eq!(from_converter, from_function);
    }

    #[test]
    fn builder_creates_the_default_converter() {
        let input = br"{\rtf1 converter builder}";
        let converter = Converter::builder().build();
        let markdown = converter.to_markdown_bytes(input, Format::Rtf).unwrap();
        assert!(markdown.contains("converter builder"));
    }
}
