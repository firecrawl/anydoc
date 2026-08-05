//! Node.js bindings for anydoc.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

mod document;

pub use document::*;

/// Input format, named after the extension that identifies it. Container
/// variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto
/// these.
#[napi(string_enum)]
#[derive(Clone, Copy)]
#[allow(non_camel_case_types)]
pub enum Format {
    doc,
    docx,
    odt,
    /// Converted with pdf-inspector, which emits Markdown directly:
    /// `toDocument` is unsupported for PDFs. Scanned or image-only PDFs
    /// (needing OCR) error as unsupported.
    pdf,
    ppt,
    pptx,
    rtf,
    epub,
    xlsx,
    ods,
    odp,
    csv,
    /// Raster image documents (PNG, JPEG, WebP, TIFF, BMP). Recognized with
    /// local OCR where it is configured, and unsupported otherwise;
    /// `toDocument` is unsupported for images the same way it is for PDFs.
    image,
}

impl From<Format> for anydoc::Format {
    fn from(format: Format) -> Self {
        match format {
            Format::doc => anydoc::Format::Doc,
            Format::docx => anydoc::Format::Docx,
            Format::odt => anydoc::Format::Odt,
            Format::pdf => anydoc::Format::Pdf,
            Format::ppt => anydoc::Format::Ppt,
            Format::pptx => anydoc::Format::Pptx,
            Format::rtf => anydoc::Format::Rtf,
            Format::epub => anydoc::Format::Epub,
            Format::xlsx => anydoc::Format::Excel,
            Format::ods => anydoc::Format::Ods,
            Format::odp => anydoc::Format::Odp,
            Format::csv => anydoc::Format::Csv,
            Format::image => anydoc::Format::Image,
        }
    }
}

impl From<anydoc::Format> for Format {
    fn from(format: anydoc::Format) -> Self {
        match format {
            anydoc::Format::Doc => Format::doc,
            anydoc::Format::Docx => Format::docx,
            anydoc::Format::Odt => Format::odt,
            anydoc::Format::Pdf => Format::pdf,
            anydoc::Format::Ppt => Format::ppt,
            anydoc::Format::Pptx => Format::pptx,
            anydoc::Format::Rtf => Format::rtf,
            anydoc::Format::Epub => Format::epub,
            anydoc::Format::Excel => Format::xlsx,
            anydoc::Format::Ods => Format::ods,
            anydoc::Format::Odp => Format::odp,
            anydoc::Format::Csv => Format::csv,
            anydoc::Format::Image => Format::image,
        }
    }
}

/// Detect the format from the content itself: the signature and identity each
/// container specification designates (PDF header, RTF open group, OLE stream
/// names, ZIP package mimetype/content types). Plain-text formats (CSV) carry
/// no signature and return `null`; so does anything unrecognized.
#[napi]
pub fn format_from_bytes(bytes: Uint8Array) -> Option<Format> {
    anydoc::Format::from_bytes(&bytes).map(Format::from)
}

/// The format an extension names, with or without a leading dot.
#[napi]
pub fn format_from_extension(extension: String) -> Option<Format> {
    anydoc::Format::from_extension(extension.trim_start_matches('.')).map(Format::from)
}

/// The format a path's extension names.
#[napi]
pub fn format_from_path(path: String) -> Option<Format> {
    anydoc::Format::from_path(std::path::Path::new(&path)).map(Format::from)
}

/// Convert a document file to Markdown. The format is detected from the file
/// content; the extension is the fallback for signature-less formats (CSV)
/// and unrecognizable containers.
///
/// Rejects with an `Error` carrying a `ConvertErrorCode` on `code`; a file
/// that cannot be read is `'io'`.
#[napi(ts_return_type = "Promise<string>")]
pub fn to_markdown(path: String) -> AsyncTask<MarkdownFileTask> {
    AsyncTask::new(MarkdownFileTask { path, failure: Failure::default() })
}

/// Convert an in-memory document to Markdown. Without a format, it is
/// detected from the content, which signature-less formats (CSV) have to name
/// explicitly.
///
/// Rejects with an `Error` carrying a `ConvertErrorCode` on `code`.
#[napi(ts_return_type = "Promise<string>")]
pub fn to_markdown_bytes(
    bytes: Uint8Array,
    format: Option<Format>,
) -> AsyncTask<MarkdownBytesTask> {
    AsyncTask::new(MarkdownBytesTask {
        bytes: bytes.to_vec(),
        format: format.map(Into::into),
        failure: Failure::default(),
    })
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `toMarkdownBytes`.
///
/// Rejects with an `Error` carrying a `ConvertErrorCode` on `code`.
#[napi(ts_return_type = "Promise<Document>")]
pub fn to_document(bytes: Uint8Array, format: Option<Format>) -> AsyncTask<DocumentTask> {
    AsyncTask::new(DocumentTask {
        bytes: bytes.to_vec(),
        format: format.map(Into::into),
        failure: Failure::default(),
    })
}

/// The RTen models local OCR needs, as bytes the caller supplies. anydoc
/// performs no download and no filesystem access of its own.
#[napi(object)]
pub struct OcrModels {
    /// The text-detection model.
    pub detection_model: Uint8Array,
    /// The text-recognition model.
    pub recognition_model: Uint8Array,
}

/// A reusable converter that carries local OCR.
///
/// The models are parsed once, when the converter is created, and every
/// conversion made with it shares that engine; nothing is rebuilt per call.
/// The module-level functions keep their own behavior, which never uses OCR.
#[napi]
pub struct Converter {
    // anydoc's converter is Send + Sync, so concurrent calls can share one.
    inner: Arc<anydoc::Converter>,
}

#[napi]
impl Converter {
    /// Parse the OCR models and build a converter that reuses them. Parsing
    /// runs off the event loop.
    ///
    /// Rejects with an `Error` carrying a `ConvertErrorCode` on `code`:
    /// `'ocrInit'` when the models cannot be loaded.
    #[napi(ts_return_type = "Promise<Converter>")]
    pub fn create(models: OcrModels) -> AsyncTask<ConverterTask> {
        AsyncTask::new(ConverterTask {
            detection_model: models.detection_model.to_vec(),
            recognition_model: models.recognition_model.to_vec(),
            failure: Failure::default(),
        })
    }

    /// Convert an in-memory document to Markdown, recognizing the pages that
    /// need OCR. Without a format, it is detected from the content, which
    /// signature-less formats (CSV) have to name explicitly. Conversion runs
    /// off the event loop.
    ///
    /// Rejects with an `Error` carrying a `ConvertErrorCode` on `code`.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn to_markdown_bytes(
        &self,
        bytes: Uint8Array,
        format: Option<Format>,
    ) -> AsyncTask<ConverterMarkdownTask> {
        AsyncTask::new(ConverterMarkdownTask {
            converter: Arc::clone(&self.inner),
            bytes: bytes.to_vec(),
            format: format.map(Into::into),
            failure: Failure::default(),
        })
    }
}

pub struct ConverterTask {
    detection_model: Vec<u8>,
    recognition_model: Vec<u8>,
    failure: Failure,
}

impl Task for ConverterTask {
    type Output = anydoc::Converter;
    type JsValue = Converter;

    fn compute(&mut self) -> Result<Self::Output> {
        let detection = std::mem::take(&mut self.detection_model);
        let recognition = std::mem::take(&mut self.recognition_model);
        let builder = anydoc::Converter::builder()
            .with_ocr_models(detection, recognition)
            .map_err(|e| self.failure.capture_init(e))?;
        Ok(builder.build())
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Converter { inner: Arc::new(output) })
    }

    fn reject(&mut self, env: Env, error: Error) -> Result<Self::JsValue> {
        Err(self.failure.reject(env, error))
    }
}

pub struct ConverterMarkdownTask {
    converter: Arc<anydoc::Converter>,
    bytes: Vec<u8>,
    format: Option<anydoc::Format>,
    failure: Failure,
}

impl Task for ConverterMarkdownTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        self.converter
            .to_markdown_bytes(&self.bytes, self.format)
            .map_err(|e| self.failure.capture(e))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }

    fn reject(&mut self, env: Env, error: Error) -> Result<Self::JsValue> {
        Err(self.failure.reject(env, error))
    }
}

/// The kind of a failed conversion, held between the two threads a rejection
/// crosses: `compute` runs on the libuv pool, where there is no `Env` to build
/// a JS error with, and `reject` runs on the JS thread, where there is.
#[derive(Default)]
struct Failure(Option<&'static str>);

impl Failure {
    /// Keep the kind, and hand napi the message to reject with.
    fn capture(&mut self, error: anydoc::ConvertError) -> Error {
        self.0 = Some(error.code());
        Error::from_reason(error.to_string())
    }

    /// Loading OCR models fails before any conversion starts, so it carries
    /// its own code rather than one of `ConvertError`'s.
    fn capture_init(&mut self, error: anydoc::OcrInitError) -> Error {
        self.0 = Some("ocrInit");
        Error::from_reason(error.to_string())
    }

    /// Rebuild the rejection as an error whose `code` is the `ConvertError`
    /// kind. napi fills `code` from the error's status, so the status here is
    /// a plain string rather than the `Status` enum it defaults to. Anything
    /// that did not come from `capture` is napi's own failure: pass it on.
    fn reject(&self, env: Env, error: Error) -> Error {
        let Some(code) = self.0 else {
            return error;
        };
        let coded = Error::new(code.to_owned(), error.reason.clone());
        Error::from(JsError::from(coded).into_unknown(env))
    }
}

pub struct MarkdownFileTask {
    path: String,
    failure: Failure,
}

impl Task for MarkdownFileTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        anydoc::to_markdown(&self.path).map_err(|e| self.failure.capture(e))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }

    fn reject(&mut self, env: Env, error: Error) -> Result<Self::JsValue> {
        Err(self.failure.reject(env, error))
    }
}

pub struct MarkdownBytesTask {
    bytes: Vec<u8>,
    format: Option<anydoc::Format>,
    failure: Failure,
}

impl Task for MarkdownBytesTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        anydoc::to_markdown_bytes(&self.bytes, self.format).map_err(|e| self.failure.capture(e))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }

    fn reject(&mut self, env: Env, error: Error) -> Result<Self::JsValue> {
        Err(self.failure.reject(env, error))
    }
}

pub struct DocumentTask {
    bytes: Vec<u8>,
    format: Option<anydoc::Format>,
    failure: Failure,
}

impl Task for DocumentTask {
    type Output = anydoc::model::Document;
    type JsValue = Document;

    fn compute(&mut self) -> Result<Self::Output> {
        anydoc::to_document(&self.bytes, self.format).map_err(|e| self.failure.capture(e))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into())
    }

    fn reject(&mut self, env: Env, error: Error) -> Result<Self::JsValue> {
        Err(self.failure.reject(env, error))
    }
}
