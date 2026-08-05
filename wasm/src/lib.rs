//! WebAssembly bindings for anydoc.
//!
//! Mirrors the Rust lib API, minus the path-based `to_markdown`: wasm has no
//! filesystem, so conversion always starts from bytes.

use wasm_bindgen::prelude::*;

mod document;
mod typescript;

pub use document::*;

/// Input format, named after the extension that identifies it. Container
/// variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto
/// these.
#[wasm_bindgen]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Doc = "doc",
    Docx = "docx",
    Odt = "odt",
    /// Converted with pdf-inspector, which emits Markdown directly:
    /// `toDocument` is unsupported for PDFs. Scanned or image-only PDFs
    /// (needing OCR) error as unsupported.
    Pdf = "pdf",
    Ppt = "ppt",
    Pptx = "pptx",
    Rtf = "rtf",
    Epub = "epub",
    Xlsx = "xlsx",
    Ods = "ods",
    Odp = "odp",
    Csv = "csv",
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
            Format::__Invalid => unreachable!("wasm-bindgen rejects invalid enum strings"),
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

/// Detect the format from the content itself: the signature and identity each
/// container specification designates (PDF header, RTF open group, OLE stream
/// names, ZIP package mimetype/content types). Plain-text formats (CSV) carry
/// no signature and return `undefined`; so does anything unrecognized.
#[wasm_bindgen(js_name = formatFromBytes)]
pub fn format_from_bytes(bytes: &[u8]) -> Option<Format> {
    anydoc::Format::from_bytes(bytes).map(Format::from)
}

/// The format an extension names, with or without a leading dot.
#[wasm_bindgen(js_name = formatFromExtension)]
pub fn format_from_extension(extension: &str) -> Option<Format> {
    anydoc::Format::from_extension(extension.trim_start_matches('.')).map(Format::from)
}

/// The format a path's (or file name's) extension names.
#[wasm_bindgen(js_name = formatFromPath)]
pub fn format_from_path(path: &str) -> Option<Format> {
    anydoc::Format::from_path(std::path::Path::new(path)).map(Format::from)
}

/// Convert an in-memory document to Markdown. Without a format, it is
/// detected from the content, which signature-less formats (CSV) have to name
/// explicitly.
#[wasm_bindgen(js_name = toMarkdownBytes)]
pub fn to_markdown_bytes(bytes: &[u8], format: Option<Format>) -> Result<String, JsValue> {
    anydoc::to_markdown_bytes(bytes, format.map(anydoc::Format::from)).map_err(convert_error)
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `toMarkdownBytes`.
#[wasm_bindgen(js_name = toDocument, unchecked_return_type = "Document")]
pub fn to_document(bytes: &[u8], format: Option<Format>) -> Result<JsValue, JsValue> {
    let document =
        anydoc::to_document(bytes, format.map(anydoc::Format::from)).map_err(convert_error)?;
    serde_wasm_bindgen::to_value(&Document::from(document))
        .map_err(|error| js_sys::Error::new(&error.to_string()).into())
}

/// Stable, machine-readable name for a `ConvertError` variant.
///
/// `ConvertError` is `#[non_exhaustive]`, so the catch-all keeps this compiling
/// when a new variant lands.
fn error_kind(error: &anydoc::ConvertError) -> &'static str {
    use anydoc::ConvertError;
    match error {
        ConvertError::Unsupported(_) => "unsupported",
        ConvertError::Malformed { .. } => "malformed",
        ConvertError::Encrypted => "encrypted",
        ConvertError::ResourceLimit { .. } => "resourceLimit",
        ConvertError::MissingPart { .. } => "missingPart",
        ConvertError::Io(_) => "io",
        _ => "unsupported",
    }
}

/// Build the thrown error, carrying the `ConvertError` variant as `kind`.
///
/// The message alone forces a caller to match on English prose to tell an
/// encrypted document from an unreadable one. `kind` names the variant instead,
/// so a caller can branch on the cause and keep the message for humans.
fn convert_error(error: anydoc::ConvertError) -> JsValue {
    let js_error = js_sys::Error::new(&error.to_string());
    js_error.set_name("ConvertError");
    // Setting a plain property on a fresh object cannot fail, so drop the result
    // rather than panic inside an error path.
    let _ = js_sys::Reflect::set(
        &js_error,
        &JsValue::from_str("kind"),
        &JsValue::from_str(error_kind(&error)),
    );
    js_error.into()
}
