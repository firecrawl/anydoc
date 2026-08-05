//! Node.js bindings for anydoc.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::collections::HashMap;

mod document;

pub use document::*;

/// Options that customize Markdown rendering.
#[napi(object)]
pub struct MarkdownOptions {
    /// Resolved URLs keyed by the string form of `Asset.id`.
    pub asset_urls: Option<HashMap<String, String>>,
}

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
#[napi(ts_return_type = "Promise<string>")]
pub fn to_markdown(path: String) -> AsyncTask<MarkdownFileTask> {
    AsyncTask::new(MarkdownFileTask { path })
}

/// Convert an in-memory document to Markdown. Without a format, it is
/// detected from the content, which signature-less formats (CSV) have to name
/// explicitly. Pass resolved URLs in `options.assetUrls`, keyed by `Asset.id`,
/// to render embedded images at their original document positions. Missing
/// mappings keep the default alt-text rendering.
#[napi(ts_return_type = "Promise<string>")]
pub fn to_markdown_bytes(
    bytes: Uint8Array,
    format: Option<Format>,
    options: Option<MarkdownOptions>,
) -> AsyncTask<MarkdownBytesTask> {
    AsyncTask::new(MarkdownBytesTask {
        bytes: bytes.to_vec(),
        format: format.map(Into::into),
        asset_urls: options.and_then(|options| options.asset_urls).unwrap_or_default(),
    })
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `toMarkdownBytes`.
#[napi(ts_return_type = "Promise<Document>")]
pub fn to_document(bytes: Uint8Array, format: Option<Format>) -> AsyncTask<DocumentTask> {
    AsyncTask::new(DocumentTask { bytes: bytes.to_vec(), format: format.map(Into::into) })
}

pub struct MarkdownFileTask {
    path: String,
}

impl Task for MarkdownFileTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        anydoc::to_markdown(&self.path).map_err(to_napi_error)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct MarkdownBytesTask {
    bytes: Vec<u8>,
    format: Option<anydoc::Format>,
    asset_urls: HashMap<String, String>,
}

impl Task for MarkdownBytesTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        let mut options = anydoc::MarkdownOptions::default();
        for (id, url) in &self.asset_urls {
            let id = id
                .parse::<usize>()
                .map_err(|_| Error::from_reason(format!("invalid asset id: {id}")))?;
            options.asset_urls.insert(anydoc::model::AssetId(id), url.clone());
        }
        anydoc::to_markdown_bytes_with_options(&self.bytes, self.format, &options)
            .map_err(to_napi_error)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct DocumentTask {
    bytes: Vec<u8>,
    format: Option<anydoc::Format>,
}

impl Task for DocumentTask {
    type Output = anydoc::model::Document;
    type JsValue = Document;

    fn compute(&mut self) -> Result<Self::Output> {
        anydoc::to_document(&self.bytes, self.format).map_err(to_napi_error)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into())
    }
}

fn to_napi_error(error: anydoc::ConvertError) -> Error {
    Error::from_reason(error.to_string())
}
