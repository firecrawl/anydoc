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
    /// `toDocument` is unsupported for PDFs. Scanned or image-only pages
    /// need OCR, which anydoc does not do: the document throws `needsOcr`
    /// naming them.
    Pdf = "pdf",
    Ppt = "ppt",
    Pptx = "pptx",
    Rtf = "rtf",
    Epub = "epub",
    Xlsx = "xlsx",
    Ods = "ods",
    Odp = "odp",
    Csv = "csv",
    Eml = "eml",
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
            Format::Eml => anydoc::Format::Eml,
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
            anydoc::Format::Eml => Format::Eml,
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
///
/// Throws an `Error` carrying a `ConvertErrorCode` on `code`.
#[wasm_bindgen(js_name = toMarkdownBytes)]
pub fn to_markdown_bytes(bytes: &[u8], format: Option<Format>) -> Result<String, JsValue> {
    anydoc::to_markdown_bytes(bytes, format.map(anydoc::Format::from)).map_err(convert_error)
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `toMarkdownBytes`.
///
/// Throws an `Error` carrying a `ConvertErrorCode` on `code`.
#[wasm_bindgen(js_name = toDocument, unchecked_return_type = "Document")]
pub fn to_document(bytes: &[u8], format: Option<Format>) -> Result<JsValue, JsValue> {
    let document =
        anydoc::to_document(bytes, format.map(anydoc::Format::from)).map_err(convert_error)?;
    serde_wasm_bindgen::to_value(&Document::from(document))
        .map_err(|error| js_sys::Error::new(&error.to_string()).into())
}

/// The thrown value: a JS `Error` carrying the crate's message, with the
/// variant name on `code` for callers to branch on, and for `needsOcr` the
/// pages behind it.
fn convert_error(error: anydoc::ConvertError) -> JsValue {
    let thrown = js_sys::Error::new(&error.to_string());
    // Only fails on a non-object target, which `thrown` is not.
    let set = |name: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&thrown, &JsValue::from_str(name), &value);
    };
    set("code", JsValue::from_str(error.code()));
    if let anydoc::ConvertError::NeedsOcr { pages, page_count } = &error {
        set(
            "pages",
            pages.iter().map(|&page| JsValue::from(page)).collect::<js_sys::Array>().into(),
        );
        set("pageCount", JsValue::from(*page_count));
    }
    thrown.into()
}
