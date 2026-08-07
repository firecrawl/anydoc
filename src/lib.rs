//! anydoc converts documents to GitHub-Flavored Markdown.
//!
//! Recovery and skipped-content events are reported through the [`log`]
//! facade (debug/warn level); logging never changes conversion behavior and
//! its messages are not a stable API.

#![warn(missing_docs)]

pub mod model;

mod error;
mod formats;
#[cfg(feature = "ocr")]
mod ocr;
mod package;
mod render;
mod shared;

pub use error::ConvertError;
#[cfg(feature = "ocr")]
pub use ocr::OcrInitError;

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
pub struct Converter {
    #[cfg(feature = "ocr")]
    ocr: Option<ocr::Engine>,
}

impl Converter {
    /// Create a converter with the default capabilities.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "ocr")]
            ocr: None,
        }
    }

    /// Start building a configured converter.
    pub fn builder() -> ConverterBuilder {
        ConverterBuilder::new()
    }

    /// Whether this converter has local OCR configured.
    pub fn has_ocr(&self) -> bool {
        #[cfg(feature = "ocr")]
        {
            self.ocr.is_some()
        }
        #[cfg(not(feature = "ocr"))]
        {
            false
        }
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
        // PDFs and images convert to Markdown directly (pdf-inspector and
        // local OCR) without passing through the document model.
        if format == Format::Pdf {
            return self.pdf_to_markdown(bytes);
        }
        if format == Format::Image {
            return self.image_to_markdown(bytes);
        }
        Ok(document_to_markdown(&self.to_document(bytes, format)?))
    }

    /// Route the PDF through local OCR when this converter has an engine; the
    /// unconfigured path is the crate's default behavior.
    fn pdf_to_markdown(&self, bytes: &[u8]) -> Result<String, ConvertError> {
        #[cfg(feature = "ocr")]
        if let Some(engine) = self.ocr.as_ref() {
            return formats::pdf::to_markdown_with_ocr(bytes, engine);
        }
        formats::pdf::to_markdown(bytes)
    }

    /// Recognize an image document when this converter has an engine. An
    /// image has no text layer to fall back on, so without one there is
    /// nothing to convert.
    fn image_to_markdown(&self, bytes: &[u8]) -> Result<String, ConvertError> {
        #[cfg(feature = "ocr")]
        if let Some(engine) = self.ocr.as_ref() {
            return formats::image::to_markdown_with_ocr(bytes, engine);
        }
        #[cfg(not(feature = "ocr"))]
        let _ = bytes;
        Err(ConvertError::Unsupported("image has no extractable text: OCR is required".to_string()))
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
pub struct ConverterBuilder {
    #[cfg(feature = "ocr")]
    ocr: Option<ocr::Engine>,
}

impl ConverterBuilder {
    /// Create a converter builder with the default capabilities.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "ocr")]
            ocr: None,
        }
    }

    /// Load the RTen detection and recognition models used for local OCR.
    ///
    /// Model bytes are parsed immediately and retained by the converter built
    /// from this builder. The library performs no download or filesystem
    /// access. This method is available with the `ocr` Cargo feature. The
    /// size limit is checked after the bytes are owned, so an oversized
    /// borrowed slice is still copied once before rejection.
    #[cfg(feature = "ocr")]
    pub fn with_ocr_models(
        mut self,
        detection_model: impl Into<Vec<u8>>,
        recognition_model: impl Into<Vec<u8>>,
    ) -> Result<Self, OcrInitError> {
        self.ocr = Some(ocr::Engine::from_bytes(detection_model.into(), recognition_model.into())?);
        Ok(self)
    }

    /// Build the converter.
    pub fn build(self) -> Converter {
        Converter {
            #[cfg(feature = "ocr")]
            ocr: self.ocr,
        }
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
    /// [`to_document`] is unsupported for PDFs. Pages that carry no
    /// extractable text need OCR: a converter without an engine reports them
    /// as unsupported, while one built with OCR models (the `ocr` feature)
    /// recognizes exactly those pages and merges them with the rest.
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
    /// Raster image documents (PNG, JPEG, WebP, TIFF, BMP), detected by their
    /// content signature. An image carries no text layer, so it converts with
    /// local OCR when the converter has an engine configured and returns
    /// [`ConvertError::Unsupported`] when it does not. [`to_document`] is
    /// unsupported, as for [`Format::Pdf`]: recognition produces Markdown
    /// directly.
    ///
    /// EXIF orientation is not applied in this version, so a photo stored
    /// rotated is recognized in the orientation it is stored in.
    Image,
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
            "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" => Format::Image,
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

    /// Enough of a PNG to be detected; nothing here is ever decoded.
    const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn an_image_without_an_engine_is_unsupported() {
        let error = Converter::new().to_markdown_bytes(PNG_HEADER, None).unwrap_err();

        assert_eq!(error.code(), "unsupported");
        assert!(error.to_string().contains("OCR is required"), "{error}");
    }

    /// A spreadsheet export whose first column is BMI opens with `BM`, the
    /// bitmap signature. It carries no signature of its own, so detection has
    /// to leave it to the extension rather than claim it as an image.
    #[test]
    fn a_csv_starting_with_bm_still_converts() {
        let csv = b"BMI,weight,height\n22.5,70,1.76\n";

        assert_eq!(Format::from_bytes(csv), None);
        let markdown = Converter::new().to_markdown_bytes(csv, Format::Csv).unwrap();
        assert!(markdown.contains("BMI"), "{markdown}");
    }

    #[test]
    fn images_have_no_document_model() {
        let error = Converter::new().to_document(PNG_HEADER, None).unwrap_err();

        assert_eq!(error.code(), "unsupported");
        assert!(error.to_string().contains("directly to Markdown"), "{error}");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn builder_rejects_invalid_ocr_models() {
        let result = Converter::builder().with_ocr_models(vec![0, 1], vec![2, 3]);
        let Err(error) = result else { panic!("invalid models were accepted") };
        assert!(error.to_string().contains("detection model"));
    }
}
