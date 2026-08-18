//! Kotlin/Android bindings for anydoc.
//!
//! Mirrors the Rust, Node, and Python APIs. Conversion stays in the
//! `anydoc` crate; this crate only maps types across UniFFI.

use std::fmt;
use std::path::Path;

uniffi::setup_scaffolding!("anydoc");

mod document;

pub use document::*;

/// Input format, named after the extension that identifies it. Container
/// variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto
/// these.
#[derive(Clone, Copy, uniffi::Enum)]
pub enum Format {
    Doc,
    Docx,
    Odt,
    /// Converted with pdf-inspector, which emits Markdown directly:
    /// `to_document` is unsupported for PDFs. Scanned or image-only PDFs
    /// (needing OCR) error as unsupported.
    Pdf,
    Ppt,
    Pptx,
    Rtf,
    Epub,
    Xlsx,
    Ods,
    Odp,
    Csv,
}

impl From<Format> for anydoc::Format {
    fn from(format: Format) -> Self {
        match format {
            Format::Doc => anydoc::Format::Doc,
            Format::Docx => anydoc::Format::Docx,
            Format::Odt => anydoc::Format::Odt,
            Format::Pdf => anydoc::Format::Pdf,
            Format::Ppt => anydoc::Format::Ppt,
            Format::Pptx => anydoc::Format::Pptx,
            Format::Rtf => anydoc::Format::Rtf,
            Format::Epub => anydoc::Format::Epub,
            Format::Xlsx => anydoc::Format::Excel,
            Format::Ods => anydoc::Format::Ods,
            Format::Odp => anydoc::Format::Odp,
            Format::Csv => anydoc::Format::Csv,
        }
    }
}

impl From<anydoc::Format> for Format {
    fn from(format: anydoc::Format) -> Self {
        match format {
            anydoc::Format::Doc => Format::Doc,
            anydoc::Format::Docx => Format::Docx,
            anydoc::Format::Odt => Format::Odt,
            anydoc::Format::Pdf => Format::Pdf,
            anydoc::Format::Ppt => Format::Ppt,
            anydoc::Format::Pptx => Format::Pptx,
            anydoc::Format::Rtf => Format::Rtf,
            anydoc::Format::Epub => Format::Epub,
            anydoc::Format::Excel => Format::Xlsx,
            anydoc::Format::Ods => Format::Ods,
            anydoc::Format::Odp => Format::Odp,
            anydoc::Format::Csv => Format::Csv,
        }
    }
}

/// Why a conversion could not produce a useful result. Each variant is a
/// Kotlin exception subclass; `code` on the handwritten Kotlin extension
/// is the stable name callers branch on.
#[derive(Debug, uniffi::Error)]
pub enum ConvertError {
    Unsupported { detail: String },
    Malformed { part: Option<String>, detail: String },
    Encrypted,
    ResourceLimit { limit: String, detail: String },
    MissingPart { part: String },
    Io { detail: String },
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Unsupported { detail } => write!(f, "unsupported input: {detail}"),
            ConvertError::Malformed { part: Some(part), detail } => {
                write!(f, "malformed document ({part}): {detail}")
            }
            ConvertError::Malformed { part: None, detail } => {
                write!(f, "malformed document: {detail}")
            }
            ConvertError::Encrypted => write!(f, "document is encrypted"),
            ConvertError::ResourceLimit { limit, detail } => {
                write!(f, "resource limit exceeded ({limit}): {detail}")
            }
            ConvertError::MissingPart { part } => write!(f, "missing required part: {part}"),
            ConvertError::Io { detail } => write!(f, "io error: {detail}"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<anydoc::ConvertError> for ConvertError {
    fn from(error: anydoc::ConvertError) -> Self {
        match error {
            anydoc::ConvertError::Unsupported(detail) => ConvertError::Unsupported { detail },
            anydoc::ConvertError::Malformed { part, detail } => {
                ConvertError::Malformed { part, detail }
            }
            anydoc::ConvertError::Encrypted => ConvertError::Encrypted,
            anydoc::ConvertError::ResourceLimit { limit, detail } => {
                ConvertError::ResourceLimit { limit: limit.to_string(), detail }
            }
            anydoc::ConvertError::MissingPart { part } => ConvertError::MissingPart { part },
            anydoc::ConvertError::Io(error) => ConvertError::Io { detail: error.to_string() },
            other => ConvertError::Unsupported { detail: other.to_string() },
        }
    }
}

/// Detect the format from the content itself: the signature and identity each
/// container specification designates (PDF header, RTF open group, OLE stream
/// names, ZIP package mimetype/content types). Plain-text formats (CSV) carry
/// no signature and return `null`; so does anything unrecognized.
#[uniffi::export]
pub fn format_from_bytes(bytes: Vec<u8>) -> Option<Format> {
    anydoc::Format::from_bytes(&bytes).map(Format::from)
}

/// The format an extension names, with or without a leading dot.
#[uniffi::export]
pub fn format_from_extension(extension: String) -> Option<Format> {
    anydoc::Format::from_extension(extension.trim_start_matches('.')).map(Format::from)
}

/// The format a path's (or file name's) extension names.
#[uniffi::export]
pub fn format_from_path(path: String) -> Option<Format> {
    anydoc::Format::from_path(Path::new(&path)).map(Format::from)
}

/// Convert a document file to Markdown. The format is detected from the file
/// content; the extension is the fallback for signature-less formats (CSV)
/// and unrecognizable containers.
#[uniffi::export]
pub fn to_markdown(path: String) -> Result<String, ConvertError> {
    anydoc::to_markdown(&path).map_err(ConvertError::from)
}

/// Convert an in-memory document to Markdown. Without a format, it is
/// detected from the content, which signature-less formats (CSV) have to name
/// explicitly.
#[uniffi::export]
pub fn to_markdown_bytes(bytes: Vec<u8>, format: Option<Format>) -> Result<String, ConvertError> {
    anydoc::to_markdown_bytes(&bytes, format.map(anydoc::Format::from)).map_err(ConvertError::from)
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `to_markdown_bytes`.
#[uniffi::export]
pub fn to_document(bytes: Vec<u8>, format: Option<Format>) -> Result<Document, ConvertError> {
    anydoc::to_document(&bytes, format.map(anydoc::Format::from))
        .map(Document::from)
        .map_err(ConvertError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn read_fixture(name: &str) -> Vec<u8> {
        std::fs::read(fixture(name))
            .unwrap_or_else(|e| panic!("read {}: {e}", fixture(name).display()))
    }

    #[test]
    fn to_markdown_detects_the_format_from_the_file_content() {
        let markdown =
            to_markdown(fixture("docx/handmade-outline.docx").to_string_lossy().into_owned())
                .unwrap();
        assert!(markdown.lines().any(|line| line.starts_with("# ")), "{markdown}");
    }

    #[test]
    fn to_markdown_bytes_converts_in_memory() {
        let markdown =
            to_markdown_bytes(read_fixture("docx/handmade-rich.docx"), Some(Format::Docx)).unwrap();
        assert!(markdown.contains("| Quarter | Widgets |"), "{markdown}");
    }

    #[test]
    fn to_markdown_bytes_detects_the_format_when_none_is_named() {
        let markdown = to_markdown_bytes(read_fixture("docx/handmade-rich.docx"), None).unwrap();
        assert!(markdown.contains("| Quarter | Widgets |"), "{markdown}");
        let csv = read_fixture("csv/sheet.csv");
        match to_markdown_bytes(csv.clone(), None) {
            Err(ConvertError::Unsupported { detail }) => {
                assert!(detail.contains("unrecognized file content"), "{detail}");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
        let named = to_markdown_bytes(csv, Some(Format::Csv)).unwrap();
        assert!(named.contains("| --- |"), "{named}");
    }

    #[test]
    fn to_document_exposes_the_document_model() {
        let document =
            to_document(read_fixture("docx/handmade-outline.docx"), Some(Format::Docx)).unwrap();
        let heading = document
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Heading { level, content, .. } => Some((*level, content)),
                _ => None,
            })
            .expect("outline fixture has a heading");
        assert!((1..=6).contains(&heading.0));
        match heading.1.first() {
            Some(Inline::Text { text, style }) => {
                assert!(!text.is_empty());
                let _ = style.bold;
            }
            other => panic!("expected text inline, got {other:?}"),
        }
    }

    #[test]
    fn to_document_carries_embedded_assets_as_bytes() {
        let document =
            to_document(read_fixture("docx/handmade-rich.docx"), Some(Format::Docx)).unwrap();
        let image = document
            .assets
            .iter()
            .find(|asset| asset.media_type == "image/png")
            .expect("png asset");
        assert!(!image.data.is_empty());
        assert_eq!(
            image.id as usize,
            document.assets.iter().position(|a| a.id == image.id).unwrap()
        );
    }

    #[test]
    fn format_detection_reads_content_extension_and_path() {
        assert!(matches!(
            format_from_bytes(read_fixture("docx/handmade-rich.docx")),
            Some(Format::Docx)
        ));
        assert!(format_from_bytes(read_fixture("csv/sheet.csv")).is_none());
        assert!(matches!(format_from_extension(".pptm".into()), Some(Format::Pptx)));
        assert!(matches!(format_from_extension("xls".into()), Some(Format::Xlsx)));
        assert!(matches!(format_from_path("/tmp/report.odt".into()), Some(Format::Odt)));
        assert!(format_from_path("/tmp/report.unknown".into()).is_none());
    }

    #[test]
    fn conversion_errors_name_the_failure() {
        match to_markdown_bytes(b"not a document".to_vec(), Some(Format::Docx)) {
            Err(ConvertError::Malformed { part, .. }) => assert!(part.is_none()),
            other => panic!("expected malformed, got {other:?}"),
        }
        match to_markdown_bytes(read_fixture("csv/sheet.csv"), None) {
            Err(ConvertError::Unsupported { .. }) => {}
            other => panic!("expected unsupported, got {other:?}"),
        }
        match to_markdown_bytes(read_fixture("malformed/encrypted--errors.odt"), Some(Format::Odt))
        {
            Err(ConvertError::Encrypted) => {}
            other => panic!("expected encrypted, got {other:?}"),
        }
        match to_document(read_fixture("malformed/encrypted--errors.odt"), Some(Format::Odt)) {
            Err(ConvertError::Encrypted) => {}
            other => panic!("expected encrypted, got {other:?}"),
        }
        match to_markdown_bytes(read_fixture("abuse/zipbomb--errors.docx"), Some(Format::Docx)) {
            Err(ConvertError::ResourceLimit { limit, .. }) => assert_eq!(limit, "max_entry_bytes"),
            other => panic!("expected resource limit, got {other:?}"),
        }
        match to_markdown("no-such-file.docx".into()) {
            Err(ConvertError::Io { detail }) => assert!(!detail.is_empty()),
            other => panic!("expected io, got {other:?}"),
        }
    }

    #[test]
    fn missing_part_names_the_part() {
        let mut package = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut package));
            zip.start_file("[Content_Types].xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"<Types/>").unwrap();
            zip.finish().unwrap();
        }
        match to_markdown_bytes(package, Some(Format::Docx)) {
            Err(ConvertError::MissingPart { part }) => assert_eq!(part, "word/document.xml"),
            other => panic!("expected missing part, got {other:?}"),
        }
    }
}
