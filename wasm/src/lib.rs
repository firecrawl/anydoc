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
    /// `toDocument` is unsupported for PDFs. Pages with no extractable text
    /// need OCR: the module-level functions report them as unsupported,
    /// while a `Converter`, in a build with the `ocr` feature, recognizes
    /// exactly those pages.
    Pdf = "pdf",
    Ppt = "ppt",
    Pptx = "pptx",
    Rtf = "rtf",
    Epub = "epub",
    Xlsx = "xlsx",
    Ods = "ods",
    Odp = "odp",
    Csv = "csv",
    /// Raster image documents (PNG, JPEG, WebP, TIFF, BMP). Recognized with
    /// local OCR where it is configured, and unsupported otherwise;
    /// `toDocument` is unsupported for images the same way it is for PDFs.
    Image = "image",
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
            Format::Image => anydoc::Format::Image,
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
            anydoc::Format::Image => Format::Image,
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

/// A reusable converter that carries local OCR.
///
/// The models are parsed once, when the converter is constructed, and every
/// conversion made with it shares that engine; nothing is rebuilt per call.
/// The module-level functions keep their own behavior, which never uses OCR.
///
/// Recognition is CPU-bound and blocks whatever thread it runs on, so a page
/// that runs it in the UI thread freezes: see the package README for the Web
/// Worker setup.
#[cfg(feature = "ocr")]
#[wasm_bindgen]
pub struct Converter {
    inner: anydoc::Converter,
}

#[cfg(feature = "ocr")]
#[wasm_bindgen]
impl Converter {
    /// Parse the RTen detection and recognition models and keep them. anydoc
    /// performs no download of its own: both are `Uint8Array`s the caller
    /// fetched.
    ///
    /// Throws an `Error` with `'ocrInit'` on `code` when a model is missing,
    /// is not a `Uint8Array`, or cannot be loaded.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "OcrModels")] models: JsValue,
    ) -> Result<Converter, JsValue> {
        let detection = model_bytes(&models, "detectionModel")?;
        let recognition = model_bytes(&models, "recognitionModel")?;
        let built = anydoc::Converter::builder()
            .with_ocr_models(detection, recognition)
            .map_err(|error| coded_error(&error.to_string(), "ocrInit"))?;
        Ok(Converter { inner: built.build() })
    }

    /// Convert an in-memory document to Markdown, recognizing the pages that
    /// need OCR. Without a format, it is detected from the content, which
    /// signature-less formats (CSV) have to name explicitly.
    ///
    /// Throws an `Error` carrying a `ConvertErrorCode` on `code`.
    #[wasm_bindgen(js_name = toMarkdownBytes)]
    pub fn to_markdown_bytes(
        &self,
        bytes: &[u8],
        format: Option<Format>,
    ) -> Result<String, JsValue> {
        self.inner.to_markdown_bytes(bytes, format.map(anydoc::Format::from)).map_err(convert_error)
    }
}

/// One model out of the options object, as owned bytes.
#[cfg(feature = "ocr")]
fn model_bytes(models: &JsValue, key: &str) -> Result<Vec<u8>, JsValue> {
    use wasm_bindgen::JsCast as _;

    let value = js_sys::Reflect::get(models, &JsValue::from_str(key))
        .map_err(|_| coded_error(&format!("{key} could not be read"), "ocrInit"))?;
    if value.is_undefined() || value.is_null() {
        return Err(coded_error(&format!("{key} is required"), "ocrInit"));
    }
    value
        .dyn_into::<js_sys::Uint8Array>()
        .map(|model| model.to_vec())
        .map_err(|_| coded_error(&format!("{key} has to be a Uint8Array"), "ocrInit"))
}

/// The thrown value: a JS `Error` carrying the crate's message, with the
/// variant name on `code` for callers to branch on.
fn convert_error(error: anydoc::ConvertError) -> JsValue {
    coded_error(&error.to_string(), error.code())
}

fn coded_error(message: &str, code: &str) -> JsValue {
    let thrown = js_sys::Error::new(message);
    // Only fails on a non-object target, which `thrown` is not.
    let _ = js_sys::Reflect::set(&thrown, &JsValue::from_str("code"), &JsValue::from_str(code));
    thrown.into()
}
